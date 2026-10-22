//! Frozen workloads shared by the guarded protocol and reduced functional checks.
use revenant_core::{
    DecimalU64, FileEntry, Handle, PreparedValidator, ResourceRegistry, TaskContext, TaskManager,
    TaskServices, WorkClass, WorkService, validate_json,
};
use revenant_desktop::{DesktopOptions, DesktopRuntime, NativeWorkService};
use revenant_sdk::{Application, Contract, Result, operation, operations};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    path::Path,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

mod allocations;
mod progress;

pub const CASE_IDS: [&str; 22] = [
    "validation.small",
    "validation.records512",
    "ownership.churn",
    "ownership.deep-disposal",
    "cpu.500k.sessions1",
    "cpu.500k.sessions4",
    "cpu.1m.sessions1",
    "cpu.1m.sessions4",
    "dispatch.light",
    "dispatch.records512",
    "dispatch.interactive-contention",
    "lifecycle.runtime20",
    "task.progress.bytes0.listeners0",
    "task.progress.bytes0.listeners1",
    "task.progress.bytes0.listeners8",
    "task.progress.bytes1024.listeners0",
    "task.progress.bytes1024.listeners1",
    "task.progress.bytes1024.listeners8",
    "task.progress.bytes60000.listeners0",
    "task.progress.bytes60000.listeners1",
    "task.progress.bytes60000.listeners8",
    "dispatch.progress",
];
const SEED: u32 = 378;
const CHUNK: u32 = 1024;
const CONTENTION_ENTRIES: u32 = 8_000_000;
// One finite window per owner avoids counting normal between-window lending
// gaps as continuous contention. Numerical throughput fixtures keep CHUNK.
const CONTENTION_CPU_CHUNK: u32 = 262_144;
const CONTENTION_IO_CHUNKS: u32 = 256;
const CONTENTION_IO_OPERATIONS_PER_CHUNK: u32 = 64;
static CONTENTION_COMPLETED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadLoad {
    cpu_background_tasks: usize,
    io_background_tasks: usize,
    cpu_active_workers: usize,
    io_active_workers: usize,
}
impl ReadLoad {
    fn observed(inspect: &Value) -> Self {
        let running = inspect["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|task| task["state"] == "running" && task["executionFinished"] != true)
            .collect::<Vec<_>>();
        Self {
            cpu_background_tasks: running
                .iter()
                .filter(|task| task["operationId"] == "optimization.cpu")
                .count(),
            io_background_tasks: running
                .iter()
                .filter(|task| task["operationId"] == "optimization.io")
                .count(),
            cpu_active_workers: inspect["work"]["cpu_active"].as_u64().unwrap() as usize,
            io_active_workers: inspect["work"]["io_active"].as_u64().unwrap() as usize,
        }
    }
}
fn read_load_valid(before: ReadLoad, after: ReadLoad) -> bool {
    [before, after].into_iter().all(|load| {
        load.cpu_background_tasks > 0
            && load.io_background_tasks > 0
            && load.cpu_active_workers > 0
            && load.io_active_workers > 0
    })
}

fn initial_load_valid(load: ReadLoad) -> bool {
    load.cpu_active_workers >= 3
        && load.io_active_workers >= 4
        && load.cpu_background_tasks > 0
        && load.io_background_tasks > 0
}

#[cfg(test)]
mod load_tests {
    use super::{ReadLoad, initial_load_valid, read_load_valid};
    use serde_json::json;

    #[test]
    fn load_observation_excludes_queued_finished_and_interactive_tasks() {
        let inspect = json!({"tasks":[
            {"operationId":"optimization.cpu","state":"running","executionFinished":false},
            {"operationId":"optimization.io","state":"running","executionFinished":false},
            {"operationId":"optimization.cpu","state":"queued","executionFinished":false},
            {"operationId":"optimization.io","state":"succeeded","executionFinished":false},
            {"operationId":"optimization.cpu","state":"running","executionFinished":true},
            {"operationId":"optimization.light","state":"running","executionFinished":false}
        ],"work":{"cpu_active":3,"io_active":4}});
        let observed = ReadLoad::observed(&inspect);
        assert_eq!(observed.cpu_background_tasks, 1);
        assert_eq!(observed.io_background_tasks, 1);
        assert_eq!(observed.cpu_active_workers, 3);
        assert_eq!(observed.io_active_workers, 4);
    }

    #[test]
    fn a_read_requires_each_background_class_before_and_after() {
        let loaded = ReadLoad {
            cpu_background_tasks: 1,
            io_background_tasks: 1,
            cpu_active_workers: 1,
            io_active_workers: 1,
        };
        assert!(read_load_valid(loaded, loaded));
        for missing_field in 0..4 {
            let mut unloaded = loaded;
            match missing_field {
                0 => unloaded.cpu_background_tasks = 0,
                1 => unloaded.io_background_tasks = 0,
                2 => unloaded.cpu_active_workers = 0,
                _ => unloaded.io_active_workers = 0,
            }
            assert!(!read_load_valid(unloaded, loaded));
            assert!(!read_load_valid(loaded, unloaded));
        }
    }

    #[test]
    fn reserved_workers_do_not_prove_background_execution_has_started() {
        let mut load = ReadLoad {
            cpu_background_tasks: 1,
            io_background_tasks: 0,
            cpu_active_workers: 3,
            io_active_workers: 4,
        };
        assert!(!initial_load_valid(load));
        load.io_background_tasks = 1;
        assert!(initial_load_valid(load));
        load.cpu_background_tasks = 0;
        assert!(!initial_load_valid(load));
    }
}

fn metric(name: &str, unit: &str, better: &str) -> Value {
    json!({"name":name,"unit":unit,"better":better})
}
fn metric_definitions(id: &str) -> Vec<Value> {
    match id {
        "validation.small" | "validation.records512" => [
            "prepare",
            "prepared_validate",
            "ad_hoc_validate",
            "prepared_roundtrip",
            "ad_hoc_roundtrip",
        ]
        .iter()
        .map(|name| metric(name, "ns", "lower"))
        .collect(),
        "ownership.churn" => vec![
            metric("create_dispose", "ns", "lower"),
            metric("active_scopes_after", "count", "lower"),
        ],
        "ownership.deep-disposal" => vec![
            metric("dispose_tree", "ns", "lower"),
            metric("active_scopes_after", "count", "lower"),
        ],
        id if id.starts_with("cpu.") => vec![
            metric("serial_all_sessions", "ns", "lower"),
            metric("bounded_all_sessions", "ns", "lower"),
            metric("known_scratch_peak", "bytes", "lower"),
        ],
        "dispatch.light" | "dispatch.records512" | "dispatch.progress" => vec![
            metric("dispatch_roundtrip_mean", "ns", "lower"),
            metric("retained_result_json", "bytes", "lower"),
        ],
        "dispatch.interactive-contention" => {
            let mut definitions = vec![
                metric("interactive_read_p95", "ns", "lower"),
                metric("known_scratch_peak", "bytes", "lower"),
                metric("queue_wait_total", "ns", "lower"),
                metric("background_active_at_read_min", "count", "higher"),
            ];
            for owner in 0..4 {
                for class in ["cpu", "io"] {
                    definitions.push(metric(
                        &format!("owner{owner}_{class}_chunks"),
                        "count",
                        "higher",
                    ));
                    definitions.push(metric(
                        &format!("owner{owner}_{class}_max_chunk_wait"),
                        "ns",
                        "lower",
                    ));
                }
            }
            definitions
        }
        "lifecycle.runtime20" => vec![
            metric("complete_lifecycle20_total", "ns", "lower"),
            metric("active_scopes_after", "count", "lower"),
            metric("retained_result_json_after", "bytes", "lower"),
            metric("known_scratch_after", "bytes", "lower"),
        ],
        id if id.starts_with("task.progress.") => vec![
            metric("progress_publication_total", "ns", "lower"),
            metric("allocation_calls", "count", "lower"),
            metric("cumulative_requested_bytes", "bytes", "lower"),
            metric("retained_update_json", "bytes", "lower"),
        ],
        _ => panic!("Unknown case {id}"),
    }
}

fn parameters(id: &str, measure: bool) -> Value {
    let standard = measure;
    let mut parameters = match id {
        "validation.small" | "validation.records512" => {
            json!({"records":if id == "validation.small" {1} else if standard {512} else {4},
            "iterationsPerStage":if standard {100} else {1}, "model":"core FileEntry; max u64 decimal string and alternating minimum i64/None", "schema":"FileEntry or Vec<FileEntry> ordinary Contract definition", "preparedLifetime":"one immutable validator per repetition; preparation measured separately", "roundtripBoundary":"typed model -> JSON Value -> schema validation -> UTF-8 JSON -> Value -> typed model"})
        }
        "ownership.churn" => {
            json!({"identities":if standard {100_000} else {32}, "topology":"repeated root issue/dispose on one registry", "timingBoundary":"scope creation plus disposal; registry construction excluded"})
        }
        "ownership.deep-disposal" => {
            json!({"depth":if standard {8_000} else {32}, "unrelatedRoots":if standard {1_000} else {4}, "timingBoundary":"dispose root and all indexed active descendants; tree construction excluded"})
        }
        id if id.starts_with("cpu.") => {
            json!({"entriesPerSession":if standard { if id.contains("500k") {500_000} else {1_000_000}} else {4096},
            "sessions":if id.ends_with("sessions4") {4} else {1}, "seed":SEED,"wrappingU32Rounds":32,"chunkEntries":CHUNK,"cpuWorkers":4,"ioWorkers":4,"queueCapacity":32,
            "oracle":"independent u128 modular arithmetic, then exact u64 sum", "comparisonOrder":"alternating serial/bounded by repetition", "timingBoundary":"barrier release to joined session task/scope cleanup; pool construction excluded"})
        }
        "dispatch.light" | "dispatch.records512" => {
            json!({"records":if id == "dispatch.light" {1} else if standard {512} else {4}, "iterationsPerRepetition":if standard {16} else {1},
            "model":"source-owned typed MetadataRecord with exact wide integers; light case is u32 echo", "timingBoundary":"encode/decode run request -> dispatch admission -> typed operation -> JSON terminal/result snapshots -> JSON task disposal", "completionObservation":"snapshot polling with yield_now; no artificial polling sleep"})
        }
        "dispatch.interactive-contention" => {
            let io_operations = if standard { CONTENTION_IO_CHUNKS } else { 4 };
            json!({"owners":4,"cpuEntriesPerOwner":if standard {CONTENTION_ENTRIES} else {4096}, "ioOperationsPerOwner":io_operations,
            "ioChunksPerOwner":io_operations.div_ceil(CONTENTION_IO_OPERATIONS_PER_CHUNK),
            "ioOperationsPerChunk":CONTENTION_IO_OPERATIONS_PER_CHUNK,"ioSleepMillisecondsPerOperation":if standard {10} else {1},
            "interactiveReadsPerRepetition":if standard {32} else {4}, "seed":SEED,"wrappingU32Rounds":32,"chunkEntries":CONTENTION_CPU_CHUNK,"cpuWorkers":4,"ioWorkers":4,"queueCapacity":32,
            "cpuRoundOptimizationBarrier":"black_box after each round, confined to the contention fixture",
            "cpuLoadWindow":"one finite ordered window per owner; larger chunks sustain computation across reads without between-window lending gaps",
            "ioLoadWindow":"one finite ordered window per owner; 64 cooperative slow operations per chunk keep each worker occupied across reads",
            "saturationRequired":standard,"initialSaturation":"cpu_active >= 3, io_active >= 4 and a running unfinished CPU and IO task before first read; reserved permits alone are insufficient",
            "everyReadLoadInvariant":"at both read boundaries: running CPU background tasks >= 1, running IO background tasks >= 1, cpu_active >= 1 and io_active >= 1; reject the whole repetition on any failure after settling finite work",
            "loadObservationBoundary":"runtime_inspect before timing and after timing of every complete read; endpoint observations do not prove continuous saturation between samples",
            "ownerWaitBasis":"worst item-materialization to chunk-entry time, per owner and work class", "progressBasis":"all finite background chunks complete with exact results; per-class before/after task and worker counts retained for every read; no post-hoc read selection"})
        }
        "lifecycle.runtime20" => {
            json!({"cyclesPerRepetition":if standard {20} else {2},"cpuWorkers":1,"ioWorkers":1,"queueCapacity":2,
            "timingBoundary":"embedded runtime construction, connect, budget-one IO/CPU nested owned task, actual settlement, task/scope disposal and runtime shutdown", "processRestart":false})
        }
        id if id.starts_with("task.progress.") => {
            let (bytes, observers) = progress::spec(id);
            json!({"progressCalls":if standard {5000} else {100},"updateCharacters":bytes,
                "observers":observers,"concurrency":1,"retainedUpdate":"unchanged ASCII string; absent for size zero",
                "countBoundary":"one separate progress-only pass; current-thread allocation requests including reallocations",
                "timingBoundary":"another progress-only pass with counting disabled; preparation and correctness excluded",
                "allocatorInstrumentation":"same thread-local System wrapper on both sides; inactive wrapper still checks TLS",
                "observerWork":"constant-time length/endpoints/revision checks and atomic counters; no added snapshot clones"})
        }
        "dispatch.progress" => {
            json!({"progressCalls":if standard {5000} else {100},"updateCharacters":60000,
            "iterationsPerRepetition":if standard {16} else {1},"model":"typed u32 input/result with native string task updates",
            "timingBoundary":"JSON dispatch admission, update preparation, progress publication, JSON terminal/result observation and disposal acknowledgement",
            "completionObservation":"snapshot polling with yield_now; no WebView IPC/Svelte/display presentation"})
        }
        _ => panic!("Unknown case {id}"),
    };
    parameters["sizeProfile"] = json!(if standard {
        "standard"
    } else {
        "reduced-verification"
    });
    parameters["sampling"] = json!({"independentSample":"one aggregate per complete repetition","samplesPerMeasuredRepetition":1,"warmupSamplesRetained":false,"interactiveP95Method":"nearest rank of 32 reads per repetition; raw reads retained separately"});
    parameters
}

pub fn describe_case(id: &str) -> Value {
    json!({"id":id,"parameters":parameters(id,true),"metrics":metric_definitions(id)})
}

struct Samples {
    enabled: bool,
    definitions: Vec<Value>,
    repetition: usize,
    observations: Vec<Value>,
}
impl Samples {
    fn new(id: &str, enabled: bool) -> Self {
        Self {
            enabled,
            repetition: 0,
            observations: vec![],
            definitions: metric_definitions(id)
                .into_iter()
                .map(|mut value| {
                    value["samples"] = json!([]);
                    value
                })
                .collect(),
        }
    }
    fn push(&mut self, name: &str, value: u64) {
        if self.enabled {
            self.definitions
                .iter_mut()
                .find(|metric| metric["name"] == name)
                .unwrap()["samples"]
                .as_array_mut()
                .unwrap()
                .push(json!(value));
        }
    }
    fn time<T>(&mut self, name: &str, count: usize, mut operation: impl FnMut() -> T) -> T {
        let start = self.enabled.then(Instant::now);
        let mut result = operation();
        for _ in 1..count {
            result = operation();
        }
        if let Some(start) = start {
            self.push(name, (start.elapsed().as_nanos() / count as u128) as u64);
        }
        result
    }
    fn finish(self) -> Vec<Value> {
        if self.enabled {
            self.definitions
        } else {
            vec![]
        }
    }
}

pub fn run_case(
    id: &str,
    measure: bool,
    directory: &Path,
    warmups: usize,
    repeats: usize,
    diagnose: bool,
) -> Value {
    let mut samples = Samples::new(id, measure);
    let mut oracle = Value::Null;
    for repetition in 0..if measure { warmups + repeats } else { 1 } {
        samples.enabled = measure && repetition >= warmups;
        samples.repetition = repetition.saturating_sub(warmups);
        oracle = match id {
            "validation.small" | "validation.records512" => validation(id, measure, &mut samples),
            "ownership.churn" | "ownership.deep-disposal" => ownership(id, measure, &mut samples),
            id if id.starts_with("cpu.") => cpu(id, measure, repetition, &mut samples),
            id if id.starts_with("task.progress.") => {
                progress::run(id, measure, diagnose, &mut samples)
            }
            _ => {
                let executor = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .unwrap();
                executor.block_on(async {
                    let directory = directory.join(id).join(format!("repetition-{repetition}"));
                    if id == "lifecycle.runtime20" {
                        lifecycle(measure, &directory, &mut samples).await
                    } else if id == "dispatch.interactive-contention" {
                        contention(measure, &directory, &mut samples).await
                    } else {
                        dispatch_case(id, measure, &directory, &mut samples).await
                    }
                })
            }
        };
        if oracle["passed"] != true {
            break;
        }
    }
    // Every repetition asserts its oracle; the summary is deterministic and carries
    // no instance IDs, timestamps or implementation hashes into workload identity.
    samples.enabled = measure;
    let observations = std::mem::take(&mut samples.observations);
    let mut case = json!({"id":id,"parameters":parameters(id,measure),"metrics":samples.finish(),"correctness":oracle});
    if !observations.is_empty() {
        case["rawObservations"] = json!(observations);
    }
    if measure && case["correctness"]["passed"] == true {
        for metric in case["metrics"].as_array().unwrap() {
            assert_eq!(
                metric["samples"].as_array().unwrap().len(),
                repeats,
                "Each metric must retain one sample per independent repetition: {}",
                metric["name"]
            );
        }
    }
    case
}

fn entry(index: u32) -> FileEntry {
    FileEntry {
        handle: Handle {
            id: format!("fixture-{index}"),
            scope: "scope-validation-fixture".into(),
            generation: 1,
            kind: "file".into(),
        },
        name: format!("fixture-{index}.bin"),
        relative_path: format!("group-{}/fixture-{index}.bin", index % 8),
        size: u64::MAX.into(),
        mime: "application/octet-stream".into(),
        modified: if index.is_multiple_of(2) {
            Some(i64::MIN.into())
        } else {
            None
        },
    }
}
fn validation(id: &str, measure: bool, samples: &mut Samples) -> Value {
    if id == "validation.small" {
        validation_model(entry(0), measure, samples)
    } else {
        validation_model(
            (0..if measure { 512 } else { 4 })
                .map(entry)
                .collect::<Vec<_>>(),
            measure,
            samples,
        )
    }
}
fn validation_model<T: revenant_core::Contract>(
    model: T,
    measure: bool,
    samples: &mut Samples,
) -> Value {
    let schema = T::definition().unwrap().schema;
    let expected = serde_json::to_value(&model).unwrap();
    let prepared = samples.time("prepare", 1, || {
        PreparedValidator::new(schema.clone()).unwrap()
    });
    prepared.validate(&expected).unwrap();
    validate_json(&schema, &expected).unwrap();
    let count = if measure { 100 } else { 1 };
    samples.time("prepared_validate", count, || {
        prepared.validate(black_box(&expected)).unwrap()
    });
    samples.time("ad_hoc_validate", count, || {
        validate_json(black_box(&schema), black_box(&expected)).unwrap()
    });
    for use_prepared in [true, false] {
        let decoded: T = samples.time(
            if use_prepared {
                "prepared_roundtrip"
            } else {
                "ad_hoc_roundtrip"
            },
            count,
            || {
                let value = serde_json::to_value(black_box(&model)).unwrap();
                if use_prepared {
                    prepared.validate(&value).unwrap()
                } else {
                    validate_json(&schema, &value).unwrap()
                }
                let bytes = serde_json::to_vec(&value).unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                serde_json::from_value(value).unwrap()
            },
        );
        assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
    }
    json!({"passed":true,"losslessRoundtrip":true,"payloadBytes":serde_json::to_vec(&expected).unwrap().len(),"preparedConstructorsPerRepetition":1,"validatedValuesPerPath":count*2})
}

fn ownership(id: &str, measure: bool, samples: &mut Samples) -> Value {
    let resources = ResourceRegistry::new();
    if id == "ownership.churn" {
        let count = if measure { 100_000 } else { 32 };
        samples.time("create_dispose", 1, || {
            let mut previous = None;
            for _ in 0..count {
                let scope = resources.create_scope(None).unwrap();
                assert_ne!(previous.as_ref(), Some(&scope));
                resources.dispose_scope(&scope).unwrap();
                resources.dispose_scope(&scope).unwrap();
                previous = Some(scope);
            }
        });
    } else {
        let roots = (0..if measure { 1_000 } else { 4 })
            .map(|_| resources.create_scope(None).unwrap())
            .collect::<Vec<_>>();
        let root = resources.create_scope(None).unwrap();
        let mut parent = root.clone();
        for _ in 1..if measure { 8_000 } else { 32 } {
            parent = resources.create_scope(Some(&parent)).unwrap();
        }
        samples.time("dispose_tree", 1, || {
            resources.dispose_scope(&root).unwrap()
        });
        assert_eq!(resources.active_scope_count(), roots.len());
        assert!(!resources.is_scope_active(&parent));
        for root in roots {
            assert!(resources.is_scope_active(&root));
            resources.dispose_scope(&root).unwrap();
        }
    }
    let foreign = ResourceRegistry::new().create_scope(None).unwrap();
    assert!(resources.dispose_scope(&foreign).is_err());
    assert_eq!(resources.active_scope_count(), 0);
    samples.push("active_scopes_after", 0);
    json!({"passed":true,"activeScopesAfter":0,"foreignIdentityRejected":true,"retiredIdentityIdempotent":id=="ownership.churn"})
}

fn transform(index: u32) -> u32 {
    let mut value = index.wrapping_add(SEED);
    for _ in 0..32 {
        value = black_box(value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    }
    value
}
fn exact_oracle(entries: u32) -> u64 {
    (0..entries)
        .map(|index| {
            let mut value = u128::from(index) + u128::from(SEED);
            for _ in 0..32 {
                value = (value * 1_664_525 + 1_013_904_223) % (1u128 << 32);
            }
            value as u64
        })
        .sum()
}
fn serial(entries: u32) -> u64 {
    (0..entries).map(|index| u64::from(transform(index))).sum()
}
fn bounded(context: &TaskContext, entries: u32) -> u64 {
    context
        .work()
        .unwrap()
        .map_reduce(
            WorkClass::Cpu,
            0..entries.div_ceil(CHUNK),
            move |chunk, token| {
                token.checkpoint()?;
                Ok((chunk * CHUNK..((chunk + 1) * CHUNK).min(entries))
                    .map(|index| u64::from(transform(index)))
                    .sum::<u64>())
            },
            0u64,
            |sum, value| {
                *sum += value;
                Ok(())
            },
        )
        .unwrap()
}
fn cpu(id: &str, measure: bool, repetition: usize, samples: &mut Samples) -> Value {
    let entries = if measure {
        if id.contains("500k") {
            500_000
        } else {
            1_000_000
        }
    } else {
        4096
    };
    let sessions = if id.ends_with("sessions4") { 4 } else { 1 };
    let expected = exact_oracle(entries);
    let service = Arc::new(NativeWorkService::new(4, 4, 32).unwrap());
    let manager = TaskManager::with_services(
        ResourceRegistry::new(),
        Arc::new(TaskServices::new(None, Some(service.clone()))),
    );
    for helper in if repetition.is_multiple_of(2) {
        [false, true]
    } else {
        [true, false]
    } {
        let barrier = Arc::new(Barrier::new(sessions + 1));
        let mut workers = Vec::new();
        for session in 0..sessions {
            let scope = manager.resources().create_scope(None).unwrap();
            let context = manager
                .create(&scope, &format!("cpu-{helper}-{session}"))
                .unwrap();
            let manager = manager.clone();
            let barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                let execution = context.begin().unwrap();
                barrier.wait();
                let result = if helper {
                    bounded(&context, entries)
                } else {
                    serial(entries)
                };
                execution.finish(Ok(json!(result.to_string()))).unwrap();
                manager.remove(context.scope(), context.id()).unwrap();
                manager.dispose_scope(&scope).unwrap();
                result
            }));
        }
        let results = samples.time(
            if helper {
                "bounded_all_sessions"
            } else {
                "serial_all_sessions"
            },
            1,
            || {
                barrier.wait();
                std::mem::take(&mut workers)
                    .into_iter()
                    .map(|worker| worker.join().unwrap())
                    .collect::<Vec<_>>()
            },
        );
        assert!(results.iter().all(|result| *result == expected));
    }
    let stats = service.stats();
    assert_eq!(
        (
            stats.cpu_active,
            stats.io_active,
            stats.queued,
            stats.scratch_bytes
        ),
        (0, 0, 0, 0)
    );
    assert!(stats.cpu_peak <= 4 && stats.io_peak <= 4);
    assert_eq!(manager.resources().active_scope_count(), 0);
    samples.push("known_scratch_peak", stats.scratch_peak as u64);
    json!({"passed":true,"exactChecksumPerSession":expected.to_string(),"sessions":sessions,"activeScopesAfter":0,"workSettled":true})
}

#[derive(Contract, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MetadataRecord {
    name: String,
    relative_path: String,
    size: DecimalU64,
    mime: String,
    modified: Option<revenant_core::DecimalI64>,
}
fn metadata(index: u32) -> MetadataRecord {
    let entry = entry(index);
    MetadataRecord {
        name: entry.name,
        relative_path: entry.relative_path,
        size: entry.size,
        mime: entry.mime,
        modified: entry.modified,
    }
}
#[operation(id = "optimization.light", read_only, work = "interactive")]
fn light(input: u32, context: TaskContext) -> Result<u32> {
    let values = context
        .work()?
        .map_ordered(WorkClass::Interactive, [input], |value, token| {
            token.checkpoint()?;
            Ok(value + 1)
        })?;
    Ok(values[0])
}
#[operation(id = "optimization.records", read_only)]
fn records(input: Vec<MetadataRecord>) -> Vec<MetadataRecord> {
    input
}
#[operation(id = "optimization.progress", read_only, updates = String)]
fn progress_updates(input: u32, context: TaskContext) -> Result<u32> {
    context.publish("x".repeat(60000))?;
    for completed in 1..=input {
        context.progress(revenant_core::Progress {
            completed: u64::from(completed).into(),
            total: Some(u64::from(input).into()),
            message: None,
        })?;
    }
    Ok(input)
}
#[derive(Contract, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackgroundInput {
    entries: u32,
    chunks: u32,
    sleep_ms: u32,
    record_waits: bool,
}
#[derive(Contract)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackgroundOutput {
    checksum: DecimalU64,
    chunks: u32,
    waits_ns: Vec<DecimalU64>,
}
#[operation(id = "optimization.cpu", read_only)]
fn background_cpu(input: BackgroundInput, context: TaskContext) -> Result<BackgroundOutput> {
    let entries = input.entries;
    let values = context.work()?.map_ordered(
        WorkClass::Cpu,
        (0..entries.div_ceil(CONTENTION_CPU_CHUNK))
            .map(move |chunk| (chunk, input.record_waits.then(Instant::now))),
        move |(chunk, start), token| {
            token.checkpoint()?;
            let wait = start.map(|start| start.elapsed().as_nanos() as u64);
            let sum = (chunk * CONTENTION_CPU_CHUNK
                ..((chunk + 1) * CONTENTION_CPU_CHUNK).min(entries))
                .map(|index| {
                    let mut value = index.wrapping_add(SEED);
                    for _ in 0..32 {
                        // Otherwise release compilation collapses all 32 rounds
                        // to one affine expression and exhausts the stress load.
                        value =
                            black_box(value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
                    }
                    u64::from(value)
                })
                .sum::<u64>();
            CONTENTION_COMPLETED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok((sum, wait))
        },
    )?;
    Ok(BackgroundOutput {
        checksum: values.iter().map(|(sum, _)| sum).sum::<u64>().into(),
        chunks: values.len() as u32,
        waits_ns: values
            .into_iter()
            .filter_map(|(_, wait)| wait.map(Into::into))
            .collect(),
    })
}
#[operation(id = "optimization.io", read_only, work = "io")]
fn background_io(input: BackgroundInput, context: TaskContext) -> Result<BackgroundOutput> {
    let sleep_ms = input.sleep_ms;
    let values = context.work()?.map_ordered(
        WorkClass::Io,
        (0..input.chunks.div_ceil(CONTENTION_IO_OPERATIONS_PER_CHUNK))
            .map(move |chunk| (chunk, input.record_waits.then(Instant::now))),
        move |(chunk, start), token| {
            token.checkpoint()?;
            let wait = start.map(|start| start.elapsed().as_nanos() as u64);
            let mut sum = 0;
            for operation in chunk * CONTENTION_IO_OPERATIONS_PER_CHUNK
                ..((chunk + 1) * CONTENTION_IO_OPERATIONS_PER_CHUNK).min(input.chunks)
            {
                token.checkpoint()?;
                thread::sleep(Duration::from_millis(u64::from(sleep_ms)));
                sum += u64::from(operation);
            }
            token.checkpoint()?;
            Ok((sum, wait))
        },
    )?;
    Ok(BackgroundOutput {
        checksum: values.iter().map(|(sum, _)| sum).sum::<u64>().into(),
        chunks: values.len() as u32,
        waits_ns: values
            .into_iter()
            .filter_map(|(_, wait)| wait.map(Into::into))
            .collect(),
    })
}
#[operation(id = "optimization.nested", read_only, work = "io")]
fn nested(context: TaskContext) -> Result<u32> {
    context.own([0u8; 4096])?;
    let inner = context.work()?;
    context.work()?.map_reduce(
        WorkClass::Io,
        [7u32],
        move |value, token| {
            token.checkpoint()?;
            inner.map_reduce(
                WorkClass::Cpu,
                [value, 2],
                |value, _| Ok(value),
                0,
                |sum, value| {
                    *sum += value;
                    Ok(())
                },
            )
        },
        0,
        |sum, value| {
            *sum += value;
            Ok(())
        },
    )
}

async fn runtime(directory: &Path, budget_one: bool) -> DesktopRuntime {
    DesktopRuntime::new(
        Application::new().operations(operations![
            light,
            records,
            background_cpu,
            background_io,
            nested,
            progress_updates
        ]),
        DesktopOptions {
            data_dir: Some(directory.join("data")),
            cache_dir: Some(directory.join("cache")),
            app_id: "dev.revenant.optimization-benchmark".into(),
            concurrency: if budget_one { 1 } else { 4 },
            blocking_io_concurrency: if budget_one { 1 } else { 4 },
            work_queue_capacity: if budget_one { 2 } else { 32 },
            ..DesktopOptions::default()
        },
    )
    .await
    .unwrap()
}
async fn json_dispatch(runtime: &DesktopRuntime, request: Value) -> Value {
    let bytes = serde_json::to_vec(black_box(&request)).unwrap();
    let response = runtime
        .dispatch(
            "optimization",
            serde_json::from_slice(&bytes).unwrap(),
            None,
        )
        .await
        .unwrap();
    serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap()
}
async fn connect(runtime: &DesktopRuntime) -> String {
    json_dispatch(runtime, json!({"action":"root.connect"})).await["scope"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn admit(runtime: &DesktopRuntime, scope: &str, operation: &str, input: Value) -> String {
    let input = if operation == "optimization.nested" {
        input
    } else {
        json!({"input":input})
    };
    json_dispatch(
        runtime,
        json!({"action":"task.run","scope":scope,"operation":operation,"input":input}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn terminal(runtime: &DesktopRuntime, scope: &str, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let snapshot = json_dispatch(
                runtime,
                json!({"action":"task.snapshot","scope":scope,"task":id}),
            )
            .await;
            if matches!(
                snapshot["state"].as_str(),
                Some("succeeded" | "failed" | "cancelled" | "interrupted")
            ) {
                return snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Finite benchmark operation failed to settle")
}
async fn roundtrip(
    runtime: &DesktopRuntime,
    scope: &str,
    operation: &str,
    input: Value,
) -> (Value, u64) {
    let id = admit(runtime, scope, operation, input).await;
    let snapshot = terminal(runtime, scope, &id).await;
    assert_eq!(
        snapshot["state"], "succeeded",
        "operation failed: {snapshot}"
    );
    let retained = serde_json::to_vec(&snapshot["result"]).unwrap().len() as u64;
    json_dispatch(
        runtime,
        json!({"action":"task.dispose","scope":scope,"task":id}),
    )
    .await;
    (snapshot["result"].clone(), retained)
}
async fn settled(runtime: &DesktopRuntime) {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let inspect = runtime.runtime_inspect().unwrap();
            if inspect["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|task| task["executionFinished"] == true)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Actual native execution failed to settle");
}
fn assert_clean(inspect: &Value) {
    for key in ["scopes", "tasks", "views", "folders", "selections"] {
        assert_eq!(inspect["ownership"][key], 0);
    }
    for key in ["resources", "nativeOwners", "nativeLive"] {
        assert_eq!(inspect["ownership"][key], "0");
    }
    for key in ["cpu_active", "io_active", "queued", "scratch_bytes"] {
        assert_eq!(inspect["work"][key], 0);
    }
    assert_eq!(inspect["retainedResults"]["serializedJsonBytes"], 0);
    assert!(inspect["cleanupFailures"].as_array().unwrap().is_empty());
}
async fn cleanup(runtime: &DesktopRuntime, scope: &str) {
    settled(runtime).await;
    json_dispatch(runtime, json!({"action":"scope.dispose","scope":scope})).await;
    runtime.shutdown().await.unwrap();
    assert_clean(&runtime.runtime_inspect().unwrap());
}
async fn dispatch_case(id: &str, measure: bool, directory: &Path, samples: &mut Samples) -> Value {
    let runtime = runtime(directory, false).await;
    let scope = connect(&runtime).await;
    let (operation, input) = if id == "dispatch.light" {
        ("optimization.light", json!(41))
    } else if id == "dispatch.progress" {
        (
            "optimization.progress",
            json!(if measure { 5000 } else { 100 }),
        )
    } else {
        (
            "optimization.records",
            serde_json::to_value(
                (0..if measure { 512 } else { 4 })
                    .map(metadata)
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
    };
    let expected = if id == "dispatch.light" {
        json!(42)
    } else {
        input.clone()
    };
    let count = if measure { 16 } else { 1 };
    let mut elapsed_ns = 0u128;
    let mut retained_bytes = None;
    for _ in 0..count {
        let start = samples.enabled.then(Instant::now);
        let (result, retained) = roundtrip(&runtime, &scope, operation, input.clone()).await;
        if let Some(start) = start {
            elapsed_ns += start.elapsed().as_nanos();
        }
        assert_eq!(result, expected);
        assert!(retained_bytes.is_none_or(|bytes| bytes == retained));
        retained_bytes = Some(retained);
    }
    samples.push(
        "dispatch_roundtrip_mean",
        (elapsed_ns / count as u128) as u64,
    );
    samples.push("retained_result_json", retained_bytes.unwrap());
    cleanup(&runtime, &scope).await;
    json!({"passed":true,"exactOutput":true,"completedRoundtrips":count,"allKnownCountersAfter":0})
}

async fn contention(measure: bool, directory: &Path, samples: &mut Samples) -> Value {
    let runtime = runtime(directory, false).await;
    let root = connect(&runtime).await;
    let entries = if measure { CONTENTION_ENTRIES } else { 4096 };
    let chunks = if measure { CONTENTION_IO_CHUNKS } else { 4 };
    let expected_cpu = exact_oracle(entries);
    let expected_io = u64::from(chunks) * (u64::from(chunks) - 1) / 2;
    let mut owners = Vec::new();
    let mut tasks = Vec::new();
    CONTENTION_COMPLETED.store(0, std::sync::atomic::Ordering::Relaxed);
    for owner in 0..4 {
        let scope =
            json_dispatch(&runtime, json!({"action":"scope.create","parent":root})).await["scope"]
                .as_str()
                .unwrap()
                .to_owned();
        for (class, operation) in [("cpu", "optimization.cpu"), ("io", "optimization.io")] {
            let input = serde_json::to_value(BackgroundInput {
                entries,
                chunks,
                sleep_ms: if measure { 10 } else { 1 },
                record_waits: measure,
            })
            .unwrap();
            let id = admit(&runtime, &scope, operation, input).await;
            tasks.push((owner, class, scope.clone(), id));
        }
        owners.push(scope);
    }
    if measure {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let inspect = runtime.runtime_inspect().unwrap();
                if initial_load_valid(ReadLoad::observed(&inspect)) {
                    break;
                }
                if inspect["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|task| task["executionFinished"] == true)
                {
                    panic!("Background load finished before required saturation");
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Required CPU/IO saturation not observed; repetition is invalid");
    }
    let mut read_times = Vec::new();
    let mut background_counts = Vec::new();
    let mut before_loads = Vec::new();
    let mut after_loads = Vec::new();
    let mut invalid_reads = Vec::new();
    let mut probe_times = Vec::new();
    let mut completed_chunks = Vec::new();
    for read in 0..if measure { 32 } else { 4 } {
        let probe_start = Instant::now();
        let inspect = runtime.runtime_inspect().unwrap();
        let before_load = ReadLoad::observed(&inspect);
        let before_probe_ns = probe_start.elapsed().as_nanos() as u64;
        let start = Instant::now();
        let (result, _) =
            roundtrip(&runtime, &owners[read % 4], "optimization.light", json!(41)).await;
        read_times.push(start.elapsed().as_nanos() as u64);
        let probe_start = Instant::now();
        let after_load = ReadLoad::observed(&runtime.runtime_inspect().unwrap());
        probe_times.push((before_probe_ns, probe_start.elapsed().as_nanos() as u64));
        completed_chunks.push(CONTENTION_COMPLETED.load(std::sync::atomic::Ordering::Relaxed));
        background_counts
            .push((before_load.cpu_background_tasks + before_load.io_background_tasks) as u64);
        background_counts
            .push((after_load.cpu_background_tasks + after_load.io_background_tasks) as u64);
        if measure && !read_load_valid(before_load, after_load) {
            invalid_reads.push(read);
        }
        before_loads.push(before_load);
        after_loads.push(after_load);
        assert_eq!(result, 42);
    }
    if samples.enabled {
        let mut sorted = read_times.clone();
        sorted.sort_unstable();
        let rank = (sorted.len() * 95).div_ceil(100) - 1;
        samples.push("interactive_read_p95", sorted[rank]);
        samples.push(
            "background_active_at_read_min",
            *background_counts.iter().min().unwrap(),
        );
        samples.observations.push(json!({"repetition":samples.repetition,"interactiveReadNanoseconds":read_times,
            "backgroundActiveAtReadBoundaries":background_counts,"loadBeforeEachRead":before_loads,"loadAfterEachRead":after_loads,"invalidReadIndices":invalid_reads}));
    }
    for (owner, class, scope, id) in tasks {
        let snapshot = terminal(&runtime, &scope, &id).await;
        assert_eq!(snapshot["state"], "succeeded");
        let output: BackgroundOutput = serde_json::from_value(snapshot["result"].clone()).unwrap();
        assert_eq!(
            output.checksum.get(),
            if class == "cpu" {
                expected_cpu
            } else {
                expected_io
            }
        );
        assert_eq!(
            output.chunks,
            if class == "cpu" {
                entries.div_ceil(CONTENTION_CPU_CHUNK)
            } else {
                chunks.div_ceil(CONTENTION_IO_OPERATIONS_PER_CHUNK)
            }
        );
        assert_eq!(
            output.waits_ns.len(),
            if measure { output.chunks as usize } else { 0 }
        );
        samples.push(
            &format!("owner{owner}_{class}_chunks"),
            u64::from(output.chunks),
        );
        samples.push(
            &format!("owner{owner}_{class}_max_chunk_wait"),
            output
                .waits_ns
                .iter()
                .map(|value| value.get())
                .max()
                .unwrap_or(0),
        );
        json_dispatch(
            &runtime,
            json!({"action":"task.dispose","scope":scope,"task":id}),
        )
        .await;
    }
    settled(&runtime).await;
    let inspect = runtime.runtime_inspect().unwrap();
    samples.push(
        "known_scratch_peak",
        inspect["work"]["scratch_peak"].as_u64().unwrap(),
    );
    samples.push(
        "queue_wait_total",
        inspect["work"]["queue_wait_ns"].as_u64().unwrap(),
    );
    cleanup(&runtime, &root).await;
    let passed = invalid_reads.is_empty();
    let mut oracle = json!({"passed":passed,"owners":4,"cpuChecksumPerOwner":expected_cpu.to_string(),"ioChecksumPerOwner":expected_io.to_string(),"allFiniteOwnerChunksCompleted":true,"allKnownCountersAfter":0,"saturationObserved":measure,"perReadLoadInvariantEnforced":measure});
    if !passed {
        oracle["failure"] = json!(
            "CPU or IO background load was absent at a read boundary; reject this entire repetition"
        );
        oracle["invalidReadIndices"] = json!(invalid_reads);
        oracle["failedLoadSamples"] =
            json!({"before":before_loads,"after":after_loads,"warmup":!samples.enabled});
        // Rejected warmups need enough detail to diagnose load exhaustion. These
        // observations are never admitted as independent performance samples.
        oracle["failedReadNanoseconds"] = json!(read_times);
        oracle["failedProbeNanoseconds"] = json!(probe_times);
        oracle["failedCompletedCpuChunks"] = json!(completed_chunks);
        oracle["expectedCpuChunks"] = json!(4 * entries.div_ceil(CONTENTION_CPU_CHUNK));
    }
    oracle
}

async fn lifecycle(measure: bool, directory: &Path, samples: &mut Samples) -> Value {
    let cycles = if measure { 20 } else { 2 };
    let mut previous_identity = None;
    let start = samples.enabled.then(Instant::now);
    for cycle in 0..cycles {
        let runtime = runtime(&directory.join(format!("cycle-{cycle}")), true).await;
        assert_ne!(previous_identity.as_ref(), Some(&runtime.identity()));
        previous_identity = Some(runtime.identity());
        let scope = connect(&runtime).await;
        let id = admit(&runtime, &scope, "optimization.nested", json!({})).await;
        let snapshot = terminal(&runtime, &scope, &id).await;
        assert_eq!(snapshot["state"], "succeeded");
        assert_eq!(snapshot["result"], 9);
        settled(&runtime).await;
        assert_eq!(
            runtime.runtime_inspect().unwrap()["ownership"]["nativeLive"],
            "1"
        );
        json_dispatch(
            &runtime,
            json!({"action":"task.dispose","scope":scope,"task":id}),
        )
        .await;
        cleanup(&runtime, &scope).await;
    }
    if let Some(start) = start {
        samples.push(
            "complete_lifecycle20_total",
            start.elapsed().as_nanos() as u64,
        );
    }
    samples.push("active_scopes_after", 0);
    samples.push("retained_result_json_after", 0);
    samples.push("known_scratch_after", 0);
    json!({"passed":true,"cycles":cycles,"uniqueInstanceIdentity":true,"budgetOneNestedResult":9,"allKnownCountersAfter":0})
}
