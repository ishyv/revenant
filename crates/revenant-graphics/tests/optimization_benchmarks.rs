//! Fixed graphics workloads, driven by the explicit benchmark runner protocol.
use revenant_graphics::{Primitive, Scene2D, SceneChange, Shape};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

#[cfg(all(windows, feature = "gpu"))]
#[path = "support/optimization_gpu.rs"]
mod native;

struct AllocationCounter;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
#[global_allocator]
static ALLOCATOR: AllocationCounter = AllocationCounter;
unsafe impl GlobalAlloc for AllocationCounter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
        }
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[derive(Deserialize)]
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
impl Request {
    fn validate(&self, idle_confirmed: bool) -> Result<(), String> {
        if !matches!(self.mode.as_str(), "describe" | "verify" | "measure") {
            return Err("Unknown benchmark mode".into());
        }
        if !matches!(self.profile.as_str(), "standard" | "gpu") {
            return Err("Unknown graphics profile".into());
        }
        if self.mode != "describe" && self.profile == "gpu" && !cfg!(all(windows, feature = "gpu"))
        {
            return Err(
                "GPU profile requires a Windows executable built with the GPU feature".into(),
            );
        }
        if self.warmups != 3 || self.repeats != 15 {
            return Err("Workload version 1 requires three warmups and fifteen repetitions".into());
        }
        if !self.output.is_absolute() || !self.work_directory.is_absolute() {
            return Err("Output and work directory must be absolute".into());
        }
        if self.mode == "measure" && !idle_confirmed {
            return Err("Measurements require REVENANT_BENCHMARK_IDLE_CONFIRMED=1".into());
        }
        let described = descriptors();
        for id in &self.cases {
            let case = described
                .iter()
                .find(|case| case["id"] == *id)
                .ok_or_else(|| format!("Unknown graphics benchmark case: {id}"))?;
            if self.mode != "describe" && case["requiredProfile"] == "gpu" && self.profile != "gpu"
            {
                return Err(format!("{id} requires an explicitly selected GPU profile"));
            }
        }
        Ok(())
    }
    fn measured(&self) -> bool {
        self.mode == "measure"
    }
    fn primitives(&self) -> usize {
        if self.measured() { 50_000 } else { 256 }
    }
    fn changes(&self) -> usize {
        if self.measured() { 500 } else { 4 }
    }
    fn iterations(&self) -> usize {
        if self.measured() {
            self.warmups + self.repeats
        } else {
            1
        }
    }
}

fn metric(name: &str, unit: &str, samples: Vec<u64>) -> Value {
    json!({"name":name,"unit":unit,"better":"lower","samples":samples})
}
fn metadata(name: &str, unit: &str) -> Value {
    json!({"name":name,"unit":unit,"better":"lower"})
}
fn descriptor(id: &str, parameters: Value, metrics: Vec<Value>, profile: &str) -> Value {
    json!({"id":id,"requiredProfile":profile,"parameters":parameters,"metrics":metrics})
}
fn descriptors() -> Vec<Value> {
    let mut cases = vec![
        descriptor(
            "graphics.scene.replace",
            json!({"primitives":50_000,"labels":"Fixture {index}","gridColumns":250,"inputPreparation":"excluded","oracle":"ordered keys and geometry"}),
            vec![
                metadata("publication", "ns"),
                metadata("allocation_calls", "count"),
                metadata("allocation_requested", "bytes"),
            ],
            "standard",
        ),
        descriptor(
            "graphics.scene.patch",
            json!({"primitives":50_000,"updates":500,"stride":100,"inputPreparation":"excluded","oracle":"immutable base, preserved key order, updated geometry, stale revision rejection"}),
            vec![
                metadata("publication", "ns"),
                metadata("allocation_calls", "count"),
                metadata("allocation_requested", "bytes"),
            ],
            "standard",
        ),
        descriptor(
            "graphics.scene.pick",
            json!({"primitives":50_000,"queries":256,"queryPattern":"first, middle, last, miss","oracle":"known nonoverlapping grid key"}),
            vec![metadata("pick_batch", "ns")],
            "standard",
        ),
        descriptor(
            "graphics.scene.lifecycle",
            json!({"cycles":20,"primitivesPerCycle":50_000,"subscriptionsPerCycle":1,"oracle":"callback unregister, immutable snapshot survives owner drop"}),
            vec![metadata("lifecycle_batch", "ns")],
            "standard",
        ),
    ];
    #[cfg(all(windows, feature = "gpu"))]
    cases.extend(native::descriptors());
    // Also builds without the GPU feature; no native device is implied by that build.
    cases.shrink_to_fit();
    cases
}

fn primitives(count: usize, revision: usize) -> Vec<Primitive> {
    (0..count)
        .map(|index| Primitive {
            key: format!("primitive-{index}"),
            shape: if index % 2 == 0 {
                Shape::Rectangle
            } else {
                Shape::Circle
            },
            bounds: [
                (index % 250) as f32 * 4.,
                (index / 250) as f32 * 3.,
                3.8 - (revision % 2) as f32 * 0.1,
                2.85,
            ],
            color: [0.2, 0.5, 0.7, 1.],
            label: format!("Fixture {index}"),
        })
        .collect()
}
fn changes(base: &[Primitive], count: usize, revision: usize) -> Vec<SceneChange> {
    let stride = base.len() / count;
    (0..count)
        .map(|index| {
            let mut p = base[index * stride].clone();
            p.color[0] = if revision.is_multiple_of(2) { 0.3 } else { 0.4 };
            SceneChange::Update(p)
        })
        .collect()
}

// Verification never reads a clock or enables the allocation observer.
fn observed<T>(measured: bool, operation: impl FnOnce() -> T) -> (T, [u64; 3]) {
    if !measured {
        return (operation(), [0; 3]);
    }
    struct StopCounting;
    impl Drop for StopCounting {
        fn drop(&mut self) {
            COUNTING.store(false, Ordering::SeqCst);
        }
    }
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::SeqCst);
    let guard = StopCounting;
    let start = Instant::now();
    let result = operation();
    let elapsed = u64::try_from(start.elapsed().as_nanos()).expect("Nanosecond sample overflow");
    drop(guard);
    (
        result,
        [
            elapsed,
            ALLOCATIONS.load(Ordering::Relaxed),
            ALLOCATED_BYTES.load(Ordering::Relaxed),
        ],
    )
}
fn scene_case(request: &Request, mut case: Value) -> Value {
    let id = case["id"].as_str().unwrap().to_owned();
    let count = request.primitives();
    let measured = request.measured();
    let mut samples = [vec![], vec![], vec![]];
    for repetition in 0..request.iterations() {
        let record = measured && repetition >= request.warmups;
        let scene = Scene2D::new();
        let base = primitives(count, 0);
        let revision = scene.replace(base.clone()).unwrap();
        let before = scene.snapshot();
        let (_, observation) = match id.as_str() {
            "graphics.scene.replace" => {
                let replacement = primitives(count, 1);
                let (next, stats) = observed(record, || scene.replace(replacement).unwrap());
                assert_eq!(next, revision + 1);
                let after = scene.snapshot();
                assert_eq!(after.primitives, primitives(count, 1));
                assert_eq!(before.primitives, base);
                ((), stats)
            }
            "graphics.scene.patch" => {
                let edits = changes(&base, request.changes(), 1);
                let (next, stats) = observed(record, || scene.patch(revision, edits).unwrap());
                assert_eq!(next, revision + 1);
                let after = scene.snapshot();
                assert_eq!(after.primitives.len(), count);
                for (index, p) in after.primitives.iter().enumerate() {
                    assert_eq!(p.key, base[index].key);
                    assert_eq!(p.bounds, base[index].bounds);
                    assert_eq!(
                        p.color[0],
                        if index % (count / request.changes()) == 0
                            && index / (count / request.changes()) < request.changes()
                        {
                            0.4
                        } else {
                            0.2
                        }
                    );
                }
                assert!(scene.patch(revision, vec![]).is_err());
                assert_eq!(scene.snapshot().revision, next);
                assert_eq!(before.primitives, base);
                ((), stats)
            }
            "graphics.scene.pick" => {
                let queries: Vec<_> = (0..256)
                    .map(|index| {
                        let picked = [0, count / 2, count - 1][index % 3];
                        if index % 4 == 3 {
                            ([-10., -10.], None)
                        } else {
                            let p = &base[picked];
                            (
                                [
                                    p.bounds[0] + p.bounds[2] / 2.,
                                    p.bounds[1] + p.bounds[3] / 2.,
                                ],
                                Some(p.key.clone()),
                            )
                        }
                    })
                    .collect();
                let (hits, stats) = observed(record, || {
                    queries
                        .iter()
                        .map(|(point, _)| black_box(scene.pick(*point)))
                        .collect::<Vec<_>>()
                });
                assert_eq!(
                    hits,
                    queries
                        .iter()
                        .map(|(_, key)| key.clone())
                        .collect::<Vec<_>>()
                );
                ((), stats)
            }
            "graphics.scene.lifecycle" => {
                let cycles = if measured { 20 } else { 2 };
                let (_, stats) = observed(record, || {
                    for _ in 0..cycles {
                        let lifecycle = Scene2D::new();
                        let calls = std::sync::Arc::new(AtomicU64::new(0));
                        let callback = calls.clone();
                        let subscription = lifecycle.subscribe(std::sync::Arc::new(move || {
                            callback.fetch_add(1, Ordering::Relaxed);
                        }));
                        lifecycle.replace(base.clone()).unwrap();
                        assert_eq!(calls.load(Ordering::Relaxed), 1);
                        let held = lifecycle.snapshot();
                        drop(subscription);
                        lifecycle.patch(held.revision, vec![]).unwrap();
                        assert_eq!(calls.load(Ordering::Relaxed), 1);
                        drop(lifecycle);
                        assert_eq!(held.primitives, base);
                    }
                });
                ((), stats)
            }
            _ => unreachable!(),
        };
        if record {
            for (index, value) in observation.into_iter().enumerate() {
                samples[index].push(value);
            }
        }
    }
    if measured {
        let name = match id.as_str() {
            "graphics.scene.pick" => "pick_batch",
            "graphics.scene.lifecycle" => "lifecycle_batch",
            _ => "publication",
        };
        case["metrics"] = json!([metric(name, "ns", samples[0].clone())]);
        if matches!(
            id.as_str(),
            "graphics.scene.replace" | "graphics.scene.patch"
        ) {
            case["metrics"].as_array_mut().unwrap().extend([
                metric("allocation_calls", "count", samples[1].clone()),
                metric("allocation_requested", "bytes", samples[2].clone()),
            ]);
        }
    } else {
        case["metrics"] = json!([]);
    }
    case["correctness"] =
        json!({"passed":true,"oracle":case["parameters"]["oracle"],"primitiveCount":count});
    if !measured {
        case["parameters"]["verificationReduced"] = json!(true);
        if id == "graphics.scene.lifecycle" {
            case["parameters"]["cycles"] = json!(2);
            case["parameters"]["primitivesPerCycle"] = json!(count);
        } else {
            case["parameters"]["primitives"] = json!(count);
        }
        if id == "graphics.scene.patch" {
            case["parameters"]["updates"] = json!(request.changes());
            case["parameters"]["stride"] = json!(count / request.changes());
        }
    }
    case
}

fn run(request: &Request) -> Value {
    let (warmups, repeats) = match request.mode.as_str() {
        "describe" => (0, 0),
        "verify" => (0, 1),
        "measure" => (request.warmups, request.repeats),
        _ => unreachable!("Request mode validated before execution"),
    };
    let mut cases = descriptors();
    if request.mode != "describe" {
        cases.retain(|case| case["requiredProfile"] == "standard" || request.profile == "gpu");
    }
    if !request.cases.is_empty() {
        cases.retain(|case| request.cases.iter().any(|id| case["id"] == *id));
    }
    if request.mode != "describe" {
        cases = cases
            .into_iter()
            .map(|case| {
                if case["requiredProfile"] == "gpu" {
                    #[cfg(all(windows, feature = "gpu"))]
                    {
                        return native::run_case(request, case);
                    }
                    #[cfg(not(all(windows, feature = "gpu")))]
                    {
                        unreachable!("GPU cases are never described in a CPU build");
                    }
                }
                scene_case(request, case)
            })
            .collect();
    }
    json!({"schemaVersion":1,"suite":"revenant-optimization","component":"graphics","mode":request.mode,"workloadVersion":1,"warmups":warmups,"repeats":repeats,"cases":cases,"limitations":["Publication timings exclude deterministic input preparation. Allocation bytes are requested allocations, including reallocations, not retained memory; allocation instrumentation adds overhead.","GPU metrics exist only in an explicitly selected Windows GPU profile. CPU render-call timing includes submission and present calls, not compositor timestamps, 60 Hz proof, physical reset or driver memory.","Known graphics bytes exclude driver allocations and pipeline internals. CPU scene lifecycle has no total-process memory oracle."]})
}

#[test]
#[ignore = "Runner protocol only; actual measurements require explicit idle-machine confirmation"]
fn optimization_benchmark_protocol() {
    let request: Request = serde_json::from_str(
        &std::env::var("REVENANT_BENCHMARK_REQUEST").expect("Runner request missing"),
    )
    .expect("Invalid runner request");
    request
        .validate(std::env::var("REVENANT_BENCHMARK_IDLE_CONFIRMED").as_deref() == Ok("1"))
        .expect("Unsafe or unsupported runner request");
    assert!(
        request.work_directory.is_dir(),
        "Runner must create its work directory"
    );
    let output = run(&request);
    std::fs::write(&request.output, serde_json::to_vec_pretty(&output).unwrap()).unwrap();
}

#[test]
fn smoke_verifies_scene_oracles_without_metrics() {
    let request = Request {
        mode: "verify".into(),
        output: std::env::current_dir().unwrap().join("unused.json"),
        work_directory: std::env::current_dir().unwrap(),
        warmups: 3,
        repeats: 15,
        profile: "standard".into(),
        cases: vec![],
    };
    request.validate(false).unwrap();
    let output = run(&request);
    assert_eq!(output["cases"].as_array().unwrap().len(), 4);
    assert_eq!(output["warmups"], 0);
    assert_eq!(output["repeats"], 1);
    for case in output["cases"].as_array().unwrap() {
        assert_eq!(case["correctness"]["passed"], true);
        assert_eq!(case["metrics"], json!([]));
        assert_eq!(case["parameters"]["verificationReduced"], true);
    }
}

#[test]
fn protocol_rejects_unconfirmed_measurement_and_unknown_cases() {
    let mut request = Request {
        mode: "measure".into(),
        output: std::env::current_dir().unwrap().join("unused.json"),
        work_directory: std::env::current_dir().unwrap(),
        warmups: 3,
        repeats: 15,
        profile: "standard".into(),
        cases: vec![],
    };
    assert!(
        request
            .validate(false)
            .unwrap_err()
            .contains("IDLE_CONFIRMED")
    );
    request.cases.push("invented".into());
    assert!(
        request
            .validate(true)
            .unwrap_err()
            .contains("Unknown graphics")
    );
    request.mode = "describe".into();
    request.cases.clear();
    let output = run(&request);
    assert_eq!(output["warmups"], 0);
    assert_eq!(output["repeats"], 0);
    assert!(output["cases"].as_array().unwrap().iter().all(|case| {
        case.get("correctness").is_none()
            && case["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .all(|metric| metric.get("samples").is_none())
    }));
}

#[test]
fn protocol_rejects_unsupported_profiles_and_relative_paths() {
    let mut request = Request {
        mode: "verify".into(),
        output: std::env::current_dir().unwrap().join("unused.json"),
        work_directory: std::env::current_dir().unwrap(),
        warmups: 3,
        repeats: 15,
        profile: "standard".into(),
        cases: vec![],
    };
    request.output = PathBuf::from("relative.json");
    assert!(request.validate(false).unwrap_err().contains("absolute"));
    request.output = std::env::current_dir().unwrap().join("unused.json");
    request.repeats = 1;
    assert!(request.validate(false).unwrap_err().contains("fifteen"));
    request.repeats = 15;
    request.profile = "invented".into();
    assert!(request.validate(false).unwrap_err().contains("profile"));
    request.profile = "gpu".into();
    if cfg!(all(windows, feature = "gpu")) {
        request.profile = "standard".into();
        request.cases.push("graphics.gpu.patch".into());
        assert!(request.validate(false).unwrap_err().contains("explicitly"));
    } else {
        assert!(request.validate(false).unwrap_err().contains("Windows"));
    }
}
