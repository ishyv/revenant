use super::{Gate, Harness, Observations, WATCHDOG, rows};
use revenant_core::{FileEntry, TaskSnapshot, TaskState};
use revenant_desktop::{DesktopOptions, DesktopRuntime, Result};
use revenant_sdk::{
    Application, OperationEffect, OperationRegistration, Provider, ProviderDescriptor,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const PROBE_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy)]
pub struct ComparisonOptions {
    pub items: u32,
    pub cpu_iterations: u32,
    pub file_bytes: u32,
    pub repeats: u32,
    pub workers: usize,
}
impl Default for ComparisonOptions {
    fn default() -> Self {
        Self {
            items: 32,
            cpu_iterations: 2_000_000,
            file_bytes: 8 * 1024 * 1024,
            repeats: 3,
            workers: 4,
        }
    }
}

#[derive(Default)]
struct ExecutionCounters {
    active: AtomicUsize,
    maximum: AtomicUsize,
}
struct Executing(Arc<ExecutionCounters>);
impl Executing {
    fn begin(counters: Arc<ExecutionCounters>) -> Self {
        let active = counters.active.fetch_add(1, Ordering::SeqCst) + 1;
        counters.maximum.fetch_max(active, Ordering::SeqCst);
        Self(counters)
    }
}
impl Drop for Executing {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

struct MeasurementProvider {
    iterations: u32,
    counters: Arc<ExecutionCounters>,
}
impl Provider for MeasurementProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "measurement".into(),
            description: "Equal workloads under the two scheduling declarations".into(),
            dependencies: vec!["files".into()],
            configuration_schema: json!({"type":"object","additionalProperties":false}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let mut operations = Vec::new();
        for (policy, independent) in [("sequential", false), ("independent", true)] {
            let counters = self.counters.clone();
            let iterations = self.iterations;
            let mut cpu = OperationRegistration::new::<u32, String, _, _>(
                format!("measurement.cpu.{policy}"),
                "Integer mixing with bounded cancellation checkpoints",
                move |seed, context| {
                    let counters = counters.clone();
                    async move {
                        let _executing = Executing::begin(counters);
                        let mut state = seed as u64 + 1;
                        for ordinal in 0..iterations {
                            if ordinal % 65_536 == 0 {
                                context.checkpoint()?;
                            }
                            state = std::hint::black_box(
                                state.rotate_left(13).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                                    ^ ordinal as u64,
                            );
                        }
                        Ok(format!("{state:016x}"))
                    }
                },
            )?;
            cpu.descriptor.independent = independent;
            cpu.descriptor.effect = OperationEffect::ReadOnly;
            operations.push(cpu);
            let counters = self.counters.clone();
            let mut io = OperationRegistration::new::<FileEntry, String, _, _>(
                format!("measurement.filesystem.{policy}"),
                "Read all captured bytes with a bounded 256KiB native buffer",
                move |input, context| {
                    let counters = counters.clone();
                    async move {
                        let _executing = Executing::begin(counters);
                        context.validate_resource(&input.handle).await?;
                        let size = context.file_metadata(&input.handle)?.size.get();
                        let mut offset = 0;
                        let mut evidence = 0u64;
                        while offset < size {
                            context.checkpoint()?;
                            let bytes = context.read(&input.handle, offset, 256 * 1024).await?;
                            assert!(!bytes.is_empty(), "admitted file must reach captured size");
                            evidence = std::hint::black_box(
                                evidence
                                    .wrapping_add(bytes[0] as u64 + bytes[bytes.len() - 1] as u64),
                            );
                            offset += bytes.len() as u64;
                        }
                        Ok(format!("{offset}:{evidence}"))
                    }
                },
            )?;
            io.descriptor.independent = independent;
            io.descriptor.effect = OperationEffect::ReadOnly;
            operations.push(io);
        }
        Ok(operations)
    }
}

#[derive(Default)]
struct Measurements {
    latencies: VecDeque<f64>,
    samples: u64,
    max_latency: f64,
    maximum_workers: u64,
    working_set_before: Option<u64>,
    sampled_peak_working_set: Option<u64>,
}
impl Measurements {
    fn before() -> Self {
        let working = process_memory().map(|memory| memory.0);
        Self {
            working_set_before: working,
            sampled_peak_working_set: working,
            ..Default::default()
        }
    }
    fn observe(&mut self, runtime: &DesktopRuntime, latency: f64) {
        self.samples += 1;
        self.max_latency = self.max_latency.max(latency);
        if self.latencies.len() == 4096 {
            self.latencies.pop_front();
        }
        self.latencies.push_back(latency);
        self.maximum_workers = self.maximum_workers.max(
            runtime.runtime_inspect().unwrap()["execution"]["activeWorkers"]
                .as_u64()
                .unwrap(),
        );
        if let Some((working, _)) = process_memory() {
            self.sampled_peak_working_set =
                Some(self.sampled_peak_working_set.unwrap_or(0).max(working));
        }
    }
    fn report(&self) -> Value {
        let mut sorted: Vec<_> = self.latencies.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let percentile = |fraction: f64| {
            sorted
                .get(((sorted.len().saturating_sub(1)) as f64 * fraction).round() as usize)
                .copied()
        };
        json!({
            "controlResponsiveness":{"probe":"native frontend task.snapshot dispatch; excludes WebView rendering","samples":self.samples,"retainedPercentileSamples":sorted.len(),"percentileWindow":"latest at most 4096 samples","medianMilliseconds":percentile(0.5),"p95Milliseconds":percentile(0.95),"maximumMilliseconds":self.max_latency},
            "memory":{"workingSetBeforeBytes":self.working_set_before,"sampledPeakWorkingSetBytes":self.sampled_peak_working_set,"processPeakWorkingSetBytes":peak_memory(),"samplingIntervalMilliseconds":PROBE_INTERVAL.as_millis(),"scope":"native process; sampled peak may miss transients; OS peak is cumulative across runs; excludes WebView"},
            "maximumSampledSharedWorkers":self.maximum_workers
        })
    }
}

async fn measured_terminal(h: &Harness, task: &Value) -> (TaskSnapshot, Value) {
    let mut measurements = Measurements::before();
    let deadline = Instant::now() + WATCHDOG;
    let mut last_notice = Instant::now();
    loop {
        assert!(
            Instant::now() < deadline,
            "measurement exceeded native watchdog"
        );
        let started = Instant::now();
        let snapshot: TaskSnapshot = serde_json::from_value(
            h.request(json!({"action":"task.snapshot","task":task["id"]}))
                .await
                .unwrap(),
        )
        .unwrap();
        measurements.observe(&h.runtime, started.elapsed().as_secs_f64() * 1000.0);
        if snapshot.state.is_terminal() {
            return (snapshot, measurements.report());
        }
        if last_notice.elapsed() >= Duration::from_secs(10) {
            eprintln!(
                "Batch progress: {} / {:?}",
                snapshot.progress.completed, snapshot.progress.total
            );
            last_notice = Instant::now();
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

type TerminalClock = Arc<Mutex<Option<Duration>>>;

async fn measured_admission(
    h: &Harness,
    operation: &str,
    selection: Value,
    started: Instant,
) -> (Value, TerminalClock) {
    let clock = Arc::new(Mutex::new(None));
    let published = clock.clone();
    let forward = h.sink();
    let sink = Arc::new(move |snapshot: Value| {
        if matches!(
            snapshot["state"].as_str(),
            Some("succeeded" | "partial" | "failed" | "cancelled" | "interrupted")
        ) {
            let mut clock = published.lock().unwrap();
            if clock.is_none() {
                *clock = Some(started.elapsed());
            }
        }
        forward(snapshot);
    });
    let task = h
        .runtime
        .dispatch(
            "acceptance-window",
            json!({"action":"task.batch","scope":h.scope,"operation":operation,"selection":selection}),
            Some(sink),
        )
        .await
        .unwrap();
    (task, clock)
}

fn native_milliseconds(clock: &TerminalClock) -> f64 {
    clock
        .lock()
        .unwrap()
        .expect("terminal publication must precede its snapshot")
        .as_secs_f64()
        * 1000.0
}

fn provenance() -> Value {
    json!({
        "nativeSource":std::env::var("REVENANT_BENCHMARK_SOURCE").ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()),
        "hardware":std::env::var("REVENANT_BENCHMARK_HARDWARE").ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()),
        "os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"logicalProcessors":std::thread::available_parallelism().ok().map(|count| count.get()),
        "rust":std::process::Command::new("rustc").arg("--version").output().ok().filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()),
        "profile":if cfg!(debug_assertions) { "debug" } else { "release" },
        "timestampUnixMs":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string()
    })
}

pub async fn compare(options: ComparisonOptions) -> Value {
    assert!((1..=128).contains(&options.items));
    assert!(options.workers > 0 && options.repeats > 0 && options.file_bytes > 0);
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("fixture");
    std::fs::create_dir(&fixture).unwrap();
    let mut content = vec![0x61; options.file_bytes as usize];
    for ordinal in 0..options.items {
        content[0] = ordinal as u8;
        std::fs::write(fixture.join(format!("sample-{ordinal:04}.bin")), &content).unwrap();
    }
    drop(content);
    let counters = Arc::new(ExecutionCounters::default());
    let runtime = DesktopRuntime::new(
        Application::new().capability(
            MeasurementProvider {
                iterations: options.cpu_iterations,
                counters: counters.clone(),
            },
            json!({}),
        ),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            concurrency: options.workers,
            queue_capacity: 4,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scope = runtime.connect("acceptance-window").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let h = Harness {
        runtime,
        scope,
        gate: Gate::new(usize::MAX),
        observed: Arc::new(Mutex::new(Observations::default())),
        directory,
        fixture,
    };
    let (folder, initial) = h.open(128).await;
    let view = h.ready(initial).await;
    assert_eq!(view["total"], options.items);
    let mut runs = Vec::new();
    for workload in ["cpu", "filesystem"] {
        let mut expected = None;
        for repeat in 0..options.repeats {
            let policies = if repeat % 2 == 0 {
                ["sequential", "independent"]
            } else {
                ["independent", "sequential"]
            };
            for policy in policies {
                counters.maximum.store(0, Ordering::SeqCst);
                let selection = if workload == "cpu" {
                    json!({"inputs":(0..options.items).collect::<Vec<_>>()})
                } else {
                    json!({"view":view["view"],"allMatching":true,"generation":view["generation"]})
                };
                eprintln!("Measuring {workload}, {policy}, repeat {}", repeat + 1);
                let started = Instant::now();
                let (task, terminal_clock) = measured_admission(
                    &h,
                    &format!("measurement.{workload}.{policy}"),
                    selection,
                    started,
                )
                .await;
                let (finished, observations) = measured_terminal(&h, &task).await;
                let observation_ms = started.elapsed().as_secs_f64() * 1000.0;
                let milliseconds = native_milliseconds(&terminal_clock);
                assert_eq!(finished.state, TaskState::Succeeded);
                assert_eq!(finished.summary.completed.get(), options.items as u64);
                assert_eq!(finished.summary.failed.get(), 0);
                let page = h
                    .request(json!({"action":"task.results","task":task["id"],"limit":128}))
                    .await
                    .unwrap();
                let outputs: Vec<_> = rows(&page)
                    .iter()
                    .enumerate()
                    .map(|(ordinal, row)| {
                        assert_eq!(row["ordinal"], ordinal.to_string());
                        if workload == "filesystem" {
                            assert_eq!(
                                row["output"].as_str().unwrap().split(':').next().unwrap(),
                                options.file_bytes.to_string()
                            );
                        }
                        row["output"].clone()
                    })
                    .collect();
                match &expected {
                    Some(expected) => assert_eq!(&outputs, expected),
                    None => expected = Some(outputs),
                }
                assert!(counters.maximum.load(Ordering::SeqCst) <= options.workers);
                runs.push(json!({"workload":workload,"policy":policy,"repeat":repeat+1,"milliseconds":milliseconds,"durationBasis":"native terminal publication","observationMilliseconds":observation_ms,"itemsPerSecond":options.items as f64/(milliseconds/1000.0),"bytesRead":if workload=="filesystem" { Some((options.items as u64*options.file_bytes as u64).to_string()) } else { None },"completed":finished.summary.completed,"failed":finished.summary.failed,"nativeExecution":{"maximumObserved":counters.maximum.load(Ordering::SeqCst),"maximumSampledSharedWorkers":observations["maximumSampledSharedWorkers"],"workerLimit":options.workers},"controlResponsiveness":observations["controlResponsiveness"],"memory":observations["memory"]}));
                h.request(json!({"action":"task.dispose","task":task["id"]}))
                    .await
                    .unwrap();
            }
        }
    }
    h.request(json!({"action":"view.dispose","view":view["view"]}))
        .await
        .unwrap();
    h.request(json!({"action":"folder.dispose","folder":folder}))
        .await
        .unwrap();
    h.released().await;
    h.runtime.shutdown().await.unwrap();
    json!({"schemaVersion":2,"comparison":"current scheduling declarations","provenance":provenance(),"runtimeIdentity":h.runtime.identity(),"settings":{"items":options.items,"cpuIterationsPerItem":options.cpu_iterations,"fileBytesPerItem":options.file_bytes,"repeats":options.repeats,"sharedWorkerBudget":options.workers,"policyOrder":"sequential/independent on odd repeats, reversed on even repeats"},"filesystemCache":"freshly written files, repeated reads; OS cache not flushed; excludes storage cold-cache claims","runs":runs,"equivalentOrderedOutcomes":true,"cleanup":{"resources":"0","cacheStores":0},"performanceThresholds":false})
}

// Read-only observations of metadata stores, never an extra filesystem walk.
pub fn scan_epochs(h: &Harness) -> Value {
    let mut epochs = Vec::new();
    for directory in std::fs::read_dir(h.directory.path().join("cache")).unwrap() {
        let path = directory.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("revenant-index-")
        {
            let db = Connection::open_with_flags(
                path.join("store.sqlite3"),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let epoch: i64 = db
                .query_row("SELECT coalesce(max(epoch),0) FROM files", [], |row| {
                    row.get(0)
                })
                .unwrap();
            epochs.push(epoch);
        }
    }
    json!({"scanCount":epochs.iter().sum::<i64>(),"indexStores":epochs.len(),"epochs":epochs,"source":"max committed SQLite epoch per index; empty indexes report zero"})
}

fn indexed_fixture_sizes(h: &Harness) -> Value {
    let mut files = 0i64;
    let mut empty = 0i64;
    let mut bytes = 0i64;
    for directory in std::fs::read_dir(h.directory.path().join("cache")).unwrap() {
        let path = directory.unwrap().path();
        if !path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("revenant-index-")
        {
            continue;
        }
        let db = Connection::open_with_flags(
            path.join("store.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let counts: (i64,i64,i64) = db.query_row(
            "SELECT count(*),coalesce(sum(CAST(json_extract(metadata,'$.size') AS INTEGER)=0),0),coalesce(sum(CAST(json_extract(metadata,'$.size') AS INTEGER)),0) FROM files WHERE epoch=(SELECT coalesce(max(epoch),0) FROM files)",
            [],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
        files += counts.0;
        empty += counts.1;
        bytes += counts.2;
    }
    json!({"indexedFiles":files.to_string(),"emptyFiles":empty.to_string(),"capturedBytes":bytes.to_string(),"source":"aggregate captured metadata from the completed native index; no extra fixture walk","limitation":if files==empty { Some("all indexed files are empty: this checksum run measures metadata, native admission and result storage, without nonempty-byte hashing throughput") } else { None }})
}

pub async fn run(fixture: PathBuf) -> Value {
    assert!(fixture.is_dir(), "fixture must be an existing directory");
    let h = Harness::supplied(fixture).await;
    let started = Instant::now();
    let folder = h.runtime.open_folder(&h.scope, h.fixture.clone()).unwrap();
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let before_query = scan_epochs(&h);
    eprintln!("Folder admitted in {open_ms:.3} ms; starting metadata discovery");
    let started = Instant::now();
    let query = h.request(json!({"action":"folder.query","folder":folder["folder"],"limit":128,"options":{"recursive":true}})).await.unwrap();
    let initial_ms = started.elapsed().as_secs_f64() * 1000.0;
    let ready = h.ready(query["snapshot"].clone()).await;
    let scan_ms = started.elapsed().as_secs_f64() * 1000.0;
    let total = ready["total"].as_u64().unwrap();
    let fixture_sizes = indexed_fixture_sizes(&h);
    assert_eq!(fixture_sizes["indexedFiles"], total.to_string());
    eprintln!("Indexed {total} files in {scan_ms:.3} ms");
    assert!(rows(&ready).len() <= 128);
    assert_eq!(ready["total"], ready["discovered"]);
    let mut windows = Vec::new();
    let mut snapshot = ready;
    for offset in [0, 128, total.saturating_sub(128)] {
        let started = Instant::now();
        snapshot = h.window(&snapshot, offset, 128).await;
        assert!(rows(&snapshot).len() <= 128);
        windows.push(json!({"offset":offset,"rows":rows(&snapshot).len(),"milliseconds":started.elapsed().as_secs_f64()*1000.0}));
    }
    let mut searches = Vec::new();
    for (index, search) in ["", "file", ".txt", "__revenant_acceptance_no_match__"]
        .iter()
        .enumerate()
    {
        h.ack(&snapshot).await;
        let started = Instant::now();
        let next = h.request(json!({"action":"view.query","view":snapshot["view"],"generation":index+2,"limit":128,"options":{"recursive":true,"search":search}})).await.unwrap();
        h.request(json!({"action":"view.dispose","view":snapshot["view"]}))
            .await
            .unwrap();
        snapshot = h.ready(next["snapshot"].clone()).await;
        assert!(rows(&snapshot).len() <= 128);
        searches.push(json!({"search":search,"total":snapshot["total"],"rows":rows(&snapshot).len(),"milliseconds":started.elapsed().as_secs_f64()*1000.0}));
    }
    h.ack(&snapshot).await;
    let reset = h.request(json!({"action":"view.query","view":snapshot["view"],"generation":6,"limit":128,"options":{"recursive":true}})).await.unwrap();
    h.request(json!({"action":"view.dispose","view":snapshot["view"]}))
        .await
        .unwrap();
    snapshot = h.ready(reset["snapshot"].clone()).await;
    let after_search = scan_epochs(&h);
    assert_eq!(before_query["indexStores"], 0);
    assert_eq!(after_search["indexStores"], 1);
    assert_eq!(after_search["scanCount"], if total == 0 { 0 } else { 1 });
    let cancellation = if total >= 5 {
        let started = Instant::now();
        let task = h.request(json!({"action":"task.batch","operation":"acceptance.file","selection":{"view":snapshot["view"],"allMatching":true,"generation":6}})).await.unwrap();
        h.gate.wait(5).await;
        let cancellation_started = Instant::now();
        h.request(json!({"action":"task.cancel","task":task["id"]}))
            .await
            .unwrap();
        let cancelled = h.terminal(&task).await;
        let cancellation_ms = cancellation_started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(cancelled.state, TaskState::Partial);
        assert_eq!(cancelled.summary.completed.get(), 4);
        let page = h
            .request(json!({"action":"task.results","task":task["id"],"limit":128}))
            .await
            .unwrap();
        assert_eq!(rows(&page).len(), 4);
        h.request(json!({"action":"task.dispose","task":task["id"]}))
            .await
            .unwrap();
        json!({"state":"partial","completed":"4","pageRows":4,"milliseconds":started.elapsed().as_secs_f64()*1000.0,"requestToTerminalMilliseconds":cancellation_ms})
    } else {
        json!({"skipped":"fixture has fewer than five files"})
    };
    let started = Instant::now();
    let (task, terminal_clock) = measured_admission(
        &h,
        "files.checksum",
        json!({"view":snapshot["view"],"allMatching":true,"generation":6}),
        started,
    )
    .await;
    eprintln!("Processing {total} files; completed outcomes remain on disk");
    let (final_task, processing_observations) = measured_terminal(&h, &task).await;
    let observation_ms = started.elapsed().as_secs_f64() * 1000.0;
    let process_ms = native_milliseconds(&terminal_clock);
    eprintln!("Batch finished in {process_ms:.3} ms; validating cleanup");
    assert_eq!(final_task.summary.completed.get(), total);
    let first = h
        .request(json!({"action":"task.results","task":task["id"],"offset":0,"limit":128}))
        .await
        .unwrap();
    let last = h.request(json!({"action":"task.results","task":task["id"],"offset":total.saturating_sub(128),"limit":128})).await.unwrap();
    assert!(rows(&first).len() <= 128 && rows(&last).len() <= 128);
    for (page, offset) in [(&first, 0), (&last, total.saturating_sub(128))] {
        for (index, outcome) in rows(page).iter().enumerate() {
            assert_eq!(outcome["ordinal"], (offset + index as u64).to_string());
        }
    }
    let (publications, max_rows) = {
        let observed = h.observed.lock().unwrap();
        (observed.publications, observed.max_rows)
    };
    assert!(max_rows <= 128);
    let scan_after_process = scan_epochs(&h);
    h.request(json!({"action":"task.dispose","task":task["id"]}))
        .await
        .unwrap();
    h.request(json!({"action":"view.dispose","view":snapshot["view"]}))
        .await
        .unwrap();
    h.request(json!({"action":"folder.dispose","folder":folder["folder"]}))
        .await
        .unwrap();
    h.released().await;
    h.runtime.shutdown().await.unwrap();
    json!({
        "schemaVersion":2,"fixture":h.fixture,"total":total.to_string(),"discovered":snapshot["discovered"],"provenance":provenance(),"runtimeIdentity":h.runtime.identity(),"fixtureSizes":fixture_sizes,
        "settings":{"sharedWorkerBudget":2,"checksumScheduling":"declared-independent","batchOutcomeBuffer":128,"resultPageBound":128,"controlSamplingIntervalMilliseconds":PROBE_INTERVAL.as_millis()},
        "opening":{"milliseconds":open_ms,"indexStoresBeforeQuery":before_query["indexStores"]},
        "query":{"initialMilliseconds":initial_ms,"readyMilliseconds":scan_ms,"scanCount":scan_after_process["scanCount"],"scanEvidence":scan_after_process,
            "fileBytesRead":null,"fileBytesReadEvidence":"not instrumented through public DesktopRuntime; open/index/search paths only inspect metadata and open bounded file ports"},
        "windows":windows,"searches":searches,"sinkPublications":publications,"maxSinkRows":max_rows,
        "cancellation":cancellation,
        "checksumBatch":{"state":final_task.state,"summary":final_task.summary,"milliseconds":process_ms,"durationBasis":"native terminal publication","observationMilliseconds":observation_ms,"itemsPerSecond":if process_ms>0.0 { total as f64/(process_ms/1000.0) } else { 0.0 },"firstPageRows":rows(&first).len(),"lastPageRows":rows(&last).len(),"counts":first["counts"],"maximumSampledSharedWorkers":processing_observations["maximumSampledSharedWorkers"]},
        "controlResponsiveness":processing_observations["controlResponsiveness"],"memory":processing_observations["memory"],
        "cleanup":{"fileResources":"0","cacheStores":0},"peakProcessMemoryBytes":peak_memory(),
        "performanceThresholds":false
    })
}

#[cfg(windows)]
fn process_memory() -> Option<(u64, u64)> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_working: usize,
        working: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    // The C layout matches PROCESS_MEMORY_COUNTERS on both Windows pointer widths.
    let mut counters: Counters = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<Counters>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (ok != 0).then_some((counters.working as u64, counters.peak_working as u64))
}
#[cfg(not(windows))]
fn process_memory() -> Option<(u64, u64)> {
    None
}
pub fn peak_memory() -> Option<u64> {
    process_memory().map(|memory| memory.1)
}
