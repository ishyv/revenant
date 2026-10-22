//! Explicit benchmark protocol. Normal tests check correctness and never measure performance.
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

mod optimization_benchmark_support;
use optimization_benchmark_support::{CASE_IDS, describe_case, run_case};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    mode: String,
    output: PathBuf,
    work_directory: PathBuf,
    warmups: usize,
    repeats: usize,
    profile: String,
    cases: Vec<String>,
}

fn validate_request(request: &Request, idle_confirmed: bool) -> Result<(), String> {
    if !matches!(
        request.mode.as_str(),
        "describe" | "verify" | "diagnose" | "measure"
    ) {
        return Err("Unknown mode".into());
    }
    if !request.output.is_absolute() || !request.work_directory.is_absolute() {
        return Err("Output and workDirectory must be absolute".into());
    }
    if request.profile != "standard" || request.warmups != 3 || request.repeats != 15 {
        return Err(
            "Protocol v1 requires standard profile, three warmups and fifteen repeats".into(),
        );
    }
    if request.mode == "measure" && !idle_confirmed {
        return Err("Measurement requires REVENANT_BENCHMARK_IDLE_CONFIRMED=1".into());
    }
    let mut selected = std::collections::HashSet::new();
    for id in &request.cases {
        if !CASE_IDS.contains(&id.as_str()) || !selected.insert(id) {
            return Err(format!("Unknown or duplicate case: {id}"));
        }
        if request.mode == "diagnose" && !id.starts_with("task.progress.") {
            return Err("Allocation diagnosis only accepts task.progress cases".into());
        }
    }
    Ok(())
}

fn run_protocol(request: &Request) -> Value {
    let selected: Vec<&str> = if request.cases.is_empty() {
        CASE_IDS
            .iter()
            .copied()
            .filter(|id| request.mode != "diagnose" || id.starts_with("task.progress."))
            .collect()
    } else {
        request.cases.iter().map(String::as_str).collect()
    };
    let cases: Vec<_> = selected
        .into_iter()
        .map(|id| {
            if request.mode == "describe" {
                describe_case(id)
            } else {
                run_case(
                    id,
                    request.mode == "measure",
                    &request.work_directory,
                    request.warmups,
                    request.repeats,
                    request.mode == "diagnose",
                )
            }
        })
        .collect();
    let (warmups, repeats) = match request.mode.as_str() {
        "describe" => (0, 0),
        "verify" | "diagnose" => (0, 1),
        _ => (request.warmups, request.repeats),
    };
    json!({"schemaVersion":1,"suite":"revenant-optimization","component":"native",
    "mode":request.mode,"workloadVersion":1,"warmups":warmups,"repeats":repeats,
    "cases":cases,"limitations":[
        "Embedded DesktopRuntime dispatch includes JSON encode/decode, window/scope admission, native operation execution, terminal/result observation and task disposal; it excludes WebView IPC, Svelte publication and OS presentation.",
        "Known scratch and retained JSON counters exclude allocation payloads behind arbitrary pointers, allocator overhead, OS/process memory and graphics driver allocations.",
        "Simulated blocking I/O sleeps do not measure application filesystem scans or real disk throughput.",
        "Chunk wait observations span helper-item materialization through chunk entry, including admission and scheduling; they are not pure internal queue residence times.",
        "Functional verification uses reduced workloads and records no performance samples; its artifacts are not performance baselines."
        ,"Snapshot allocation diagnosis is a single current-thread request-count observation, not retained/peak process memory or a timing comparison. Counting and timing run in separate passes; the inactive allocator wrapper still checks TLS."
    ]})
}

#[test]
#[ignore = "explicit describe/verify/measure runner protocol; measurements additionally require idle confirmation"]
fn optimization_benchmark_protocol() {
    let request: Request = serde_json::from_str(
        &std::env::var("REVENANT_BENCHMARK_REQUEST").expect("Missing benchmark request"),
    )
    .expect("Malformed benchmark request");
    validate_request(
        &request,
        std::env::var("REVENANT_BENCHMARK_IDLE_CONFIRMED").as_deref() == Ok("1"),
    )
    .expect("Rejected benchmark request");
    assert!(
        request.mode != "measure" || !cfg!(debug_assertions),
        "Measurement requires a release build"
    );
    let report = run_protocol(&request);
    std::fs::create_dir_all(request.output.parent().unwrap()).unwrap();
    std::fs::write(&request.output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}

fn verification_request() -> Request {
    let work_directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/benchmark-functional-native");
    Request {
        mode: "verify".into(),
        output: work_directory.join("unused.json"),
        work_directory,
        warmups: 3,
        repeats: 15,
        profile: "standard".into(),
        cases: vec![],
    }
}

#[test]
fn protocol_rejects_unconfirmed_measurement_and_relative_paths() {
    let mut request = verification_request();
    request.mode = "measure".into();
    assert!(validate_request(&request, false).is_err());
    assert!(validate_request(&request, true).is_ok());
    request.output = PathBuf::from("relative.json");
    assert!(validate_request(&request, true).is_err());
}

#[test]
fn protocol_rejects_unknown_duplicate_cases_and_profile_drift() {
    let mut request = verification_request();
    request.cases = vec!["cpu.unknown".into()];
    assert!(validate_request(&request, false).is_err());
    request.cases = vec!["cpu.500k.sessions1".into(); 2];
    assert!(validate_request(&request, false).is_err());
    request.cases.clear();
    request.repeats = 14;
    assert!(validate_request(&request, false).is_err());
}

#[test]
fn description_is_sample_free_and_case_selection_is_preserved() {
    let mut request = verification_request();
    request.mode = "describe".into();
    request.cases = vec!["cpu.500k.sessions4".into(), "dispatch.records512".into()];
    let report = run_protocol(&request);
    assert_eq!(report["warmups"], 0);
    assert_eq!(report["repeats"], 0);
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0]["id"], "cpu.500k.sessions4");
    assert_eq!(cases[0]["parameters"]["entriesPerSession"], 500_000);
    assert_eq!(cases[1]["parameters"]["records"], 512);
    for case in cases {
        assert!(case.get("correctness").is_none());
        for metric in case["metrics"].as_array().unwrap() {
            assert!(metric.get("samples").is_none());
        }
    }
}

#[test]
fn reduced_oracles_cover_every_native_case_without_performance_metrics() {
    let report = run_protocol(&verification_request());
    assert_eq!(report["warmups"], 0);
    assert_eq!(report["repeats"], 1);
    assert_eq!(report["cases"].as_array().unwrap().len(), CASE_IDS.len());
    for case in report["cases"].as_array().unwrap() {
        assert_eq!(case["correctness"]["passed"], true);
        assert!(case["metrics"].as_array().unwrap().is_empty());
    }
}

#[test]
fn allocation_diagnosis_has_no_timing_samples_and_restricts_its_workload() {
    let mut request = verification_request();
    request.mode = "diagnose".into();
    request.cases = vec!["dispatch.light".into()];
    assert!(validate_request(&request, false).is_err());
    request.cases = vec!["task.progress.bytes60000.listeners0".into()];
    assert!(validate_request(&request, false).is_ok());
    let report = run_protocol(&request);
    let case = &report["cases"][0];
    assert!(case["metrics"].as_array().unwrap().is_empty());
    assert_eq!(case["correctness"]["passed"], true);
    let observation = &case["rawObservations"][0];
    assert_eq!(observation["kind"], "allocation-diagnosis");
    assert_eq!(observation["iterations"], 100);
    assert!(observation["allocationCalls"].is_u64());
    assert!(observation.get("elapsed").is_none());
}
