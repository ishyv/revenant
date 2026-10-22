//! Frozen native-service measurements. Run explicitly with `--release -- --ignored --nocapture`.
//! These fixtures do not measure a webview, filesystem traversal, or application scans.
use revenant_core::{
    ResourceRegistry, TaskContext, TaskManager, TaskServices, WorkClass, WorkService, WorkStats,
};
use revenant_desktop::{DesktopOptions, DesktopRuntime, NativeWorkService};
use revenant_sdk::{Application, Result, operation, operations};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SEED: u32 = 378;
const CHUNK: u32 = 1024;
const WARMUPS: usize = 2;
const REPEATS: usize = 5;

fn transform(index: u32) -> u32 {
    let mut value = index.wrapping_add(SEED);
    for _ in 0..32 {
        // Both paths execute exactly the same arithmetic. Prevent the optimizer
        // from collapsing all 32 rounds into one affine expression.
        value = black_box(value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    }
    value
}

fn serial_checksum(entries: u32) -> u64 {
    (0..entries).map(|index| u64::from(transform(index))).sum()
}

fn checksum(context: &TaskContext, entries: u32) -> u64 {
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
            |total, value| {
                *total += value;
                Ok(())
            },
        )
        .unwrap()
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * percentile).ceil() as usize).saturating_sub(1)]
}

fn timings(values: &[f64]) -> Value {
    json!({"milliseconds":values,"p50Milliseconds":percentile(values,0.50),
        "p95Milliseconds":percentile(values,0.95),"samples":values.len(),
        "percentileMethod":"nearest rank; five-repeat p95 is the worst observed repeat"})
}

fn task(manager: &TaskManager, name: &str, class: WorkClass) -> TaskContext {
    let scope = manager.resources().create_scope(None).unwrap();
    let context = manager.create(&scope, name).unwrap();
    context.configure_work_class(class).unwrap();
    context
}

fn settle(manager: &TaskManager, context: &TaskContext) {
    manager.remove(context.scope(), context.id()).unwrap();
    manager.dispose_scope(context.scope()).unwrap();
}

fn assert_idle(stats: WorkStats) {
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
    assert!(stats.scratch_peak <= 8 * 1024 * 1024);
}

fn cpu_measurement(entries: u32, sessions: usize) -> Value {
    let service = Arc::new(NativeWorkService::new(4, 4, 32).unwrap());
    let manager = TaskManager::with_services(
        ResourceRegistry::new(),
        Arc::new(TaskServices::new(None, Some(service.clone()))),
    );
    let expected = serial_checksum(entries);
    let mut serial = Vec::new();
    let mut bounded = Vec::new();
    let mut per_session = Vec::new();
    for round in 0..WARMUPS + REPEATS {
        // Alternate order to reduce a systematic first-run/cache bias.
        let mut pair = [0.0; 2];
        for bounded_path in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let barrier = Arc::new(Barrier::new(sessions + 1));
            let mut workers = Vec::new();
            for session in 0..sessions {
                let context = task(
                    &manager,
                    &format!("cpu-{round}-{bounded_path}-{session}"),
                    WorkClass::Cpu,
                );
                let barrier = barrier.clone();
                let manager = manager.clone();
                workers.push(thread::spawn(move || {
                    let execution = context.begin().unwrap();
                    barrier.wait();
                    let start = Instant::now();
                    let result = if bounded_path {
                        checksum(&context, entries)
                    } else {
                        serial_checksum(entries)
                    };
                    let milliseconds = start.elapsed().as_secs_f64() * 1000.0;
                    assert_eq!(result, expected);
                    execution.finish(Ok(json!(result.to_string()))).unwrap();
                    settle(&manager, &context);
                    milliseconds
                }));
            }
            let start = Instant::now();
            barrier.wait();
            let session_times: Vec<_> = workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect();
            pair[usize::from(bounded_path)] = start.elapsed().as_secs_f64() * 1000.0;
            if bounded_path && round >= WARMUPS {
                per_session.push(session_times);
            }
            assert_idle(service.stats());
        }
        if round >= WARMUPS {
            serial.push(pair[0]);
            bounded.push(pair[1]);
        }
    }
    assert_eq!(manager.resources().active_scope_count(), 0);
    json!({"entriesPerSession":entries,"sessions":sessions,"exactChecksumPerSession":expected.to_string(),
        "serial":timings(&serial),"bounded":timings(&bounded),"boundedSessionMilliseconds":per_session,
        "durationBasis":"barrier release through all session joins and task/scope cleanup; pool creation excluded",
        "serialPolicy":"one ordinary native thread per session; no helper queue",
        "boundedPolicy":"one shared CPU4/IO4/queue32 service; CPU reserve leaves at most three bulk CPU stages",
        "boundedOverSerialP50":percentile(&bounded,0.5)/percentile(&serial,0.5),
        "countersIncludingWarmups":service.stats(),"exactEquivalence":true})
}

fn interactive_round(round: usize) -> Value {
    let service = Arc::new(NativeWorkService::new(4, 4, 32).unwrap());
    let manager = TaskManager::with_services(
        ResourceRegistry::new(),
        Arc::new(TaskServices::new(None, Some(service.clone()))),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let queued_peak = Arc::new(AtomicUsize::new(0));
    let observe = service.clone();
    let observer_stop = stop.clone();
    let observed_queue = queued_peak.clone();
    let observer = thread::spawn(move || {
        while !observer_stop.load(Ordering::Acquire) {
            observed_queue.fetch_max(observe.stats().queued, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(1));
        }
    });
    let cpu_completions = Arc::new([const { AtomicUsize::new(0) }; 4]);
    let io_completions = Arc::new([const { AtomicUsize::new(0) }; 4]);
    let mut workers = Vec::new();
    let owners = (0..4)
        .map(|_| manager.resources().create_scope(None).unwrap())
        .collect::<Vec<_>>();
    for session in 0..4 {
        for class in [WorkClass::Cpu, WorkClass::Io] {
            let context = manager
                .create(
                    &owners[session],
                    &format!("mixed-{round}-{session}-{class:?}"),
                )
                .unwrap();
            context.configure_work_class(class).unwrap();
            let manager = manager.clone();
            let stop = stop.clone();
            let completed = if class == WorkClass::Cpu {
                cpu_completions.clone()
            } else {
                io_completions.clone()
            };
            workers.push(thread::spawn(move || {
                let execution = context.begin().unwrap();
                while !stop.load(Ordering::Acquire) {
                    if class == WorkClass::Cpu {
                        // Finite 32-chunk windows keep all bulk owners competing.
                        let work = context.work().unwrap();
                        let completed = completed.clone();
                        work.stream_ordered(
                            WorkClass::Cpu,
                            0..32u32,
                            move |chunk, token| {
                                token.checkpoint()?;
                                let result = (0..CHUNK)
                                    .map(|index| u64::from(transform(index + chunk * CHUNK)))
                                    .sum::<u64>();
                                completed[session].fetch_add(1, Ordering::Relaxed);
                                Ok(result)
                            },
                            |value| {
                                black_box(value);
                                Ok(())
                            },
                        )
                        .unwrap();
                    } else {
                        let completed = completed.clone();
                        context
                            .work()
                            .unwrap()
                            .stream_ordered(
                                WorkClass::Io,
                                0..4u32,
                                move |value, token| {
                                    token.checkpoint()?;
                                    thread::sleep(Duration::from_millis(250));
                                    token.checkpoint()?;
                                    completed[session].fetch_add(1, Ordering::Relaxed);
                                    Ok(value)
                                },
                                |_| Ok(()),
                            )
                            .unwrap();
                    }
                }
                execution.finish(Ok(Value::Null)).unwrap();
                manager.remove(context.scope(), context.id()).unwrap();
            }));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    // Retain the same coherent sample that satisfies the condition; sampling
    // again after leaving the wait races with ordinary chunk completion.
    let initial = loop {
        let stats = service.stats();
        if (stats.cpu_active >= 3 && stats.io_active >= 4) || Instant::now() >= deadline {
            break stats;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let saturated = initial.cpu_active >= 3 && initial.io_active >= 4;
    let context = manager
        .create(&owners[0], &format!("interactive-{round}"))
        .unwrap();
    context
        .configure_work_class(WorkClass::Interactive)
        .unwrap();
    let execution = context.begin().unwrap();
    let mut reads = Vec::new();
    for value in 0..100u32 {
        let start = Instant::now();
        assert_eq!(
            context
                .work()
                .unwrap()
                .map_ordered(WorkClass::Interactive, [value], |value, token| {
                    token.checkpoint()?;
                    Ok(value)
                })
                .unwrap(),
            [value]
        );
        reads.push(start.elapsed().as_secs_f64() * 1000.0);
        thread::sleep(Duration::from_millis(5));
    }
    execution.finish(Ok(Value::Null)).unwrap();
    manager.remove(context.scope(), context.id()).unwrap();
    let cpu_at_last_read = cpu_completions
        .iter()
        .map(|count| count.load(Ordering::Acquire))
        .collect::<Vec<_>>();
    let io_at_last_read = io_completions
        .iter()
        .map(|count| count.load(Ordering::Acquire))
        .collect::<Vec<_>>();
    stop.store(true, Ordering::Release);
    for worker in workers {
        worker.join().unwrap();
    }
    observer.join().unwrap();
    for owner in owners {
        manager.dispose_scope(&owner).unwrap();
    }
    assert_idle(service.stats());
    assert_eq!(manager.resources().active_scope_count(), 0);
    assert!(saturated, "mixed workload did not reach CPU/IO saturation");
    let p95 = percentile(&reads, 0.95);
    json!({"round":round,"warmup":round<WARMUPS,"interactive":timings(&reads),
        "targetP95Milliseconds":50,"targetMet":p95<=50.0,"initialCpuAndIoSaturationObserved":saturated,
        "initialSaturationCounters":initial,"ownership":"four session scopes; CPU/IO share each scope; interactive read uses session zero",
        "cpuChunksPerOwnerAtLastRead":cpu_at_last_read,"ioChunksPerOwnerAtLastRead":io_at_last_read,
        "cpuChunksPerOwnerAfterSettlement":cpu_completions.iter().map(|count|count.load(Ordering::Acquire)).collect::<Vec<_>>(),
        "ioChunksPerOwnerAfterSettlement":io_completions.iter().map(|count|count.load(Ordering::Acquire)).collect::<Vec<_>>(),
        "queuePeakSampledAt1ms":queued_peak.load(Ordering::Relaxed),"counters":service.stats(),
        "fairnessBasis":"per-owner completed chunks; no formal starvation bound inferred"})
}

#[operation(id = "benchmark.nested", read_only, work = "io")]
fn nested_parent(context: TaskContext) -> Result<u32> {
    let nested = context.work()?;
    let work = nested.clone();
    context.own([0u8; 4096])?;
    work.map_reduce(
        WorkClass::Io,
        [7u32],
        move |value, token| {
            token.checkpoint()?;
            nested.map_reduce(
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

async fn runtime_cycles(cycles: usize) -> Value {
    let mut observations = Vec::new();
    for cycle in 0..cycles {
        let directory = tempfile::tempdir().unwrap();
        let runtime = DesktopRuntime::new(
            Application::new().operations(operations![nested_parent]),
            DesktopOptions {
                data_dir: Some(directory.path().join("data")),
                cache_dir: Some(directory.path().join("cache")),
                concurrency: 1,
                blocking_io_concurrency: 1,
                work_queue_capacity: 2,
                ..DesktopOptions::default()
            },
        )
        .await
        .unwrap();
        let identity = runtime.identity();
        let connected = runtime.connect("fixture").unwrap();
        let scope = connected["scope"].as_str().unwrap();
        let admitted = runtime
            .run_task(scope, "benchmark.nested", json!({}), None)
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = runtime.task_snapshot(scope, &admitted.id).unwrap();
                let state = runtime.runtime_inspect().unwrap();
                if snapshot.state.is_terminal()
                    && state["tasks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|task| task["executionFinished"] == true)
                {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("budget-one IO parent / IO child / CPU grandchild failed to settle");
        assert_eq!(result.result, Some(json!(9)));
        let before_disposal = runtime.runtime_inspect().unwrap();
        assert_eq!(before_disposal["ownership"]["nativeLive"], "1");
        runtime.dispose_task(scope, &admitted.id).unwrap();
        runtime.dispose_scope(scope).unwrap();
        runtime.shutdown().await.unwrap();
        let after = runtime.runtime_inspect().unwrap();
        for key in ["scopes", "tasks", "views", "folders", "selections"] {
            assert_eq!(after["ownership"][key], 0);
        }
        for key in ["resources", "nativeOwners", "nativeLive"] {
            assert_eq!(after["ownership"][key], "0");
        }
        for key in ["cpu_active", "io_active", "queued", "scratch_bytes"] {
            assert_eq!(after["work"][key], 0);
        }
        assert!(after["work"]["cpu_peak"].as_u64().unwrap() <= 1);
        assert!(after["work"]["io_peak"].as_u64().unwrap() <= 1);
        observations.push(
            json!({"cycle":cycle,"identity":identity,"beforeDisposal":before_disposal["ownership"],
            "afterShutdown":after["ownership"],"work":after["work"]}),
        );
    }
    json!({"cycles":cycles,"observations":observations,
        "scope":"complete embedded DesktopRuntime recreation, admitted operation, scoped resource, disposal and shutdown; no OS process/webview restart"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_io_parent_lends_budget_one_to_io_child_and_cpu_grandchild() {
    runtime_cycles(1).await;
}

fn source_identity(source: &str) -> String {
    let hash = source.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("fnv1a64:{hash:016x}")
}

#[test]
#[ignore = "explicit release measurement; writes target/workflow-benchmark.json"]
fn frozen_native_workflow_measurements() {
    if cfg!(debug_assertions) {
        panic!("run this measurement with --release");
    }
    let mut cpu = Vec::new();
    for entries in [500_000, 1_000_000] {
        for sessions in [1, 4] {
            cpu.push(cpu_measurement(entries, sessions));
            println!("measured {entries} entries / {sessions} sessions");
        }
    }
    let mut interactive = Vec::new();
    for round in 0..WARMUPS + REPEATS {
        interactive.push(interactive_round(round));
        println!("measured mixed service round {round}");
    }
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let lifecycle = executor.block_on(runtime_cycles(20));
    let manifest = Application::new()
        .operations(operations![nested_parent])
        .manifest()
        .unwrap();
    let measured_p95 = interactive
        .iter()
        .skip(WARMUPS)
        .map(|round| round["interactive"]["p95Milliseconds"].as_f64().unwrap())
        .collect::<Vec<_>>();
    let report = json!({"schemaVersion":1,"measuredAtUnixSeconds":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        "scope":"native framework scheduler and embedded runtime fixture; no application, IPC, filesystem or webview performance claim",
        "build":{"packageVersion":env!("CARGO_PKG_VERSION"),"release":true,"os":std::env::consts::OS,
            "arch":std::env::consts::ARCH,"availableParallelism":thread::available_parallelism().unwrap().get(),
            "executable":std::env::current_exe().unwrap(),"manifest":manifest.version,"protocol":manifest.protocol,
            "sdkRevision":revenant_core::SDK_COMPATIBILITY_REVISION,"fixtureContractDigest":manifest.digest},
        "sourceIdentity":{"algorithm":"FNV-1a 64-bit; deterministic, non-cryptographic; root acceptance records SHA-256 separately",
            "fixture":source_identity(include_str!("workflow_benchmark.rs")),
            "scheduler":source_identity(include_str!("../src/work.rs")),
            "helpers":source_identity(include_str!("../../revenant-core/src/tasks/services.rs")),
            "runtimeAdmission":source_identity(include_str!("../src/runtime/jobs.rs"))},
        "conditions":{"seed":SEED,"wrappingU32Rounds":32,"chunkEntries":CHUNK,"cpuWorkers":4,"ioWorkers":4,"queueCapacity":32,
            "warmups":WARMUPS,"measuredRepeats":REPEATS,"slowIoMilliseconds":250,"bulkOwners":4,"interactiveSamplesPerRound":100,
            "interactiveIntervalMilliseconds":5,"cpuCacheState":"two unrecorded warmups per matrix; alternating serial/bounded order",
            "observer":"1ms chunk-queue sampling in mixed workload; counters/1ms polling in runtime lifecycle; none in CPU timing",
            "concurrentExternalActivity":"not controlled; other workstation/build activity may affect observed timing"},
        "memory":{"knownScratchBudgetBytes":8*1024*1024,"orderedResultBudgetBytes":64*1024*1024,"streamWindowItems":32,
            "bulkInputMaterialized":false,"exactAccounting":"service scratch counter covers inline item and Option-result vectors only",
            "closuresChannelsThreadStacksAllocatorMetadataOsMemoryAndExternalAllocations":Value::Null,
            "processResidentMemory":Value::Null,"totalMemoryCeilingEstablished":false},
        "cpu":cpu,"interactive":interactive,"interactiveMeasuredRoundP95":timings(&measured_p95),
        "interactiveTargetMetEveryMeasuredRound":measured_p95.iter().all(|latency|*latency<=50.0),"lifecycle":lifecycle});
    let output =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/workflow-benchmark.json");
    std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("report: {}", output.display());
}
