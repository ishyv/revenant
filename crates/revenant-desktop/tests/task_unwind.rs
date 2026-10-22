//! Logical settlement must not acknowledge exit of outstanding blocking readers.
use revenant_core::{Result, TaskSnapshot, TaskState};
use revenant_desktop::{DesktopOptions, DesktopRuntime, UpdateSink};
use revenant_sdk::{Application, OperationRegistration, Provider, ProviderDescriptor};
use serde_json::json;
use std::{
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

fn bounded(name: &str, exercise: impl FnOnce()) {
    if std::env::var("REVENANT_TASK_UNWIND_CASE").as_deref() == Ok(name) {
        exercise();
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("REVENANT_TASK_UNWIND_CASE", name)
        .env("RUST_BACKTRACE", "0")
        .env("TEMP", temporary.path())
        .env("TMP", temporary.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{name}: child failed/aborted: {status}");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{name}: unwind lifecycle blocked (watchdog, not a speed assertion)");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Gates {
    entered: Semaphore,
    fast: Semaphore,
    slow: Semaphore,
    slow_exited: AtomicBool,
}
struct GateProvider {
    gates: Arc<Gates>,
    independent: bool,
}
impl Provider for GateProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "unwind".into(),
            description: "Controlled panic and exit gates".into(),
            dependencies: vec![],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let gates = self.gates.clone();
        let mut operation = OperationRegistration::new::<u32, u32, _, _>(
            "unwind.gated",
            "Native exit is independent of disposal acknowledgement",
            move |input, context| {
                let gates = gates.clone();
                async move {
                    gates.entered.add_permits(1);
                    if input == 1 {
                        // Deliberately noncooperative until explicitly released.
                        gates.slow.acquire().await.unwrap().forget();
                        gates.slow_exited.store(true, Ordering::Release);
                    } else {
                        gates.fast.acquire().await.unwrap().forget();
                    }
                    if input == 2 {
                        panic!("native operation panic");
                    }
                    context.checkpoint()?;
                    Ok(input)
                }
            },
        )?;
        operation.descriptor.independent = self.independent;
        Ok(vec![operation])
    }
}
struct Harness {
    _directory: tempfile::TempDir,
    runtime: DesktopRuntime,
    scope: String,
    gates: Arc<Gates>,
}
impl Harness {
    async fn new(independent: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let gates = Arc::new(Gates {
            entered: Semaphore::new(0),
            fast: Semaphore::new(0),
            slow: Semaphore::new(0),
            slow_exited: AtomicBool::new(false),
        });
        let runtime = DesktopRuntime::new(
            Application::new().capability(
                GateProvider {
                    gates: gates.clone(),
                    independent,
                },
                json!({}),
            ),
            DesktopOptions {
                data_dir: Some(directory.path().join("data")),
                cache_dir: Some(directory.path().join("cache")),
                // Two ordinary slots plus the runtime's reserved interactive slot.
                concurrency: 3,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let scope = runtime.connect("main").unwrap()["scope"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            _directory: directory,
            runtime,
            scope,
            gates,
        }
    }
    async fn entered(&self, count: u32) {
        tokio::time::timeout(
            Duration::from_secs(3),
            self.gates.entered.acquire_many(count),
        )
        .await
        .expect("workers must reach exit gates")
        .unwrap()
        .forget();
    }
    async fn terminal(&self, task: &TaskSnapshot) -> TaskSnapshot {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let snapshot = self.runtime.task_snapshot(&self.scope, &task.id).unwrap();
                if snapshot.state.is_terminal() {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("logical task must settle")
    }
    async fn removed(&self, task: &TaskSnapshot) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match self.runtime.task_snapshot(&self.scope, &task.id) {
                    Err(error) => {
                        assert_eq!(error.code, "task_disposed");
                        return;
                    }
                    Ok(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                }
            }
        })
        .await
        .expect("actual exit must reclaim disposed history");
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.gates.fast.add_permits(512);
        self.gates.slow.add_permits(512);
        let _ = self.runtime.dispose_scope(&self.scope);
    }
}
fn panic_on_running() -> UpdateSink {
    Arc::new(|value| {
        if value["state"] == "running" {
            panic!("running sink");
        }
    })
}

#[test]
fn scope_sink_panic_finishes_descendant_cleanup_and_other_cancellations() {
    bounded(
        "scope_sink_panic_finishes_descendant_cleanup_and_other_cancellations",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let h = Harness::new(false).await;
                let child = h.runtime.create_scope(&h.scope).unwrap();
                let fixture = h._directory.path().join("fixture");
                std::fs::create_dir(&fixture).unwrap();
                for scope in [&h.scope, &child] {
                    let folder = h.runtime.open_folder(scope, fixture.clone()).unwrap();
                    let view = h
                        .runtime
                        .query_folder(scope, folder["folder"].as_str().unwrap(), json!({}), None)
                        .unwrap();
                    h.runtime
                        .set_selection(
                            scope,
                            view["snapshot"]["view"].as_str().unwrap(),
                            None,
                            vec![],
                        )
                        .unwrap();
                }
                let called = Arc::new(AtomicBool::new(false));
                let seen = called.clone();
                let sink: UpdateSink = Arc::new(move |value| {
                    if value["state"] == "cancelling" && !seen.swap(true, Ordering::AcqRel) {
                        panic!("scope cancellation sink");
                    }
                });
                let first = h
                    .runtime
                    .run_task(&h.scope, "unwind.gated", json!(1), Some(sink))
                    .unwrap();
                let second = h
                    .runtime
                    .run_task(&child, "unwind.gated", json!(1), None)
                    .unwrap();
                h.entered(2).await;
                // Cross the documented sink throttle while noncooperative gates
                // guarantee cancellation can only originate from disposal here.
                tokio::time::sleep(Duration::from_millis(60)).await;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    h.runtime.dispose_scope(&h.scope)
                }));
                assert!(called.load(Ordering::Acquire));
                assert!(
                    result.is_err(),
                    "observer panic still propagates after cleanup"
                );
                h.runtime.dispose_scope(&h.scope).unwrap();
                let ownership = h.runtime.runtime_inspect().unwrap()["ownership"].clone();
                for key in ["scopes", "views", "folders", "selections", "previews"] {
                    assert_eq!(ownership[key], 0, "{key}: {ownership}");
                }
                assert_eq!(
                    h.runtime.task_snapshot(&child, &second.id).unwrap().state,
                    TaskState::Cancelling
                );
                h.gates.slow.add_permits(2);
                h.removed(&first).await;
                tokio::time::timeout(Duration::from_secs(3), async {
                    while h.runtime.task_snapshot(&child, &second.id).is_ok() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                h.runtime.shutdown().await.unwrap();
                assert_eq!(
                    h.runtime.runtime_inspect().unwrap()["ownership"]["tasks"],
                    0
                );
            });
        },
    );
}

#[test]
fn single_running_sink_panic_settles_and_removes_disposed_history() {
    bounded(
        "single_running_sink_panic_settles_and_removes_disposed_history",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let h = Harness::new(false).await;
                let task = h
                    .runtime
                    .run_task(&h.scope, "unwind.gated", json!(0), Some(panic_on_running()))
                    .unwrap();
                assert_eq!(h.terminal(&task).await.state, TaskState::Interrupted);
                h.runtime.dispose_task(&h.scope, &task.id).unwrap();
                h.removed(&task).await;
                h.runtime.shutdown().await.unwrap();
            });
        },
    );
}

#[test]
fn batch_running_sink_panic_settles_and_removes_disposed_history() {
    bounded(
        "batch_running_sink_panic_settles_and_removes_disposed_history",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                for independent in [false, true] {
                    let h = Harness::new(independent).await;
                    let task = h
                        .runtime
                        .run_batch(
                            &h.scope,
                            "unwind.gated",
                            json!({"inputs":[0]}),
                            Some(panic_on_running()),
                        )
                        .unwrap();
                    assert_eq!(h.terminal(&task).await.state, TaskState::Interrupted);
                    h.runtime.dispose_task(&h.scope, &task.id).unwrap();
                    h.removed(&task).await;
                    h.runtime.shutdown().await.unwrap();
                }
            });
        },
    );
}

#[test]
fn independent_parent_sink_unwind_keeps_history_until_held_child_exits() {
    bounded(
        "independent_parent_sink_unwind_keeps_history_until_held_child_exits",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let h = Harness::new(true).await;
                let sink: UpdateSink = Arc::new(|value| {
                    if value["state"] == "running" && value["progress"]["completed"] == "1" {
                        panic!("parent progress sink");
                    }
                });
                let task = h
                    .runtime
                    .run_batch(
                        &h.scope,
                        "unwind.gated",
                        json!({"inputs":[0,1]}),
                        Some(sink),
                    )
                    .unwrap();
                h.entered(2).await;
                // Cross the documented 50 ms publication throttle; this is not a benchmark.
                tokio::time::sleep(Duration::from_millis(80)).await;
                h.gates.fast.add_permits(1);
                assert_eq!(h.terminal(&task).await.state, TaskState::Interrupted);
                h.runtime.dispose_task(&h.scope, &task.id).unwrap();
                assert!(!h.gates.slow_exited.load(Ordering::Acquire));
                for _ in 0..20 {
                    assert!(
                        h.runtime.task_snapshot(&h.scope, &task.id).is_ok(),
                        "supervisor exit is not blocking child exit"
                    );
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                h.gates.slow.add_permits(1);
                h.removed(&task).await;
                assert!(h.gates.slow_exited.load(Ordering::Acquire));
                h.runtime.shutdown().await.unwrap();
            });
        },
    );
}

#[test]
fn independent_item_panic_and_disposal_drain_held_children() {
    bounded(
        "independent_item_panic_and_disposal_drain_held_children",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let h = Harness::new(true).await;
                let task = h
                    .runtime
                    .run_batch(&h.scope, "unwind.gated", json!({"inputs":[2,1]}), None)
                    .unwrap();
                h.entered(2).await;
                h.gates.fast.add_permits(1);
                h.runtime.dispose_task(&h.scope, &task.id).unwrap();
                assert!(!h.gates.slow_exited.load(Ordering::Acquire));
                assert!(h.runtime.task_snapshot(&h.scope, &task.id).is_ok());
                h.gates.slow.add_permits(1);
                h.removed(&task).await;
                assert!(h.gates.slow_exited.load(Ordering::Acquire));
                h.runtime.shutdown().await.unwrap();
            });
        },
    );
}

#[test]
fn disposal_acknowledgement_preserves_history_of_noncooperative_worker() {
    bounded(
        "disposal_acknowledgement_preserves_history_of_noncooperative_worker",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let h = Harness::new(false).await;
                let task = h
                    .runtime
                    .run_task(&h.scope, "unwind.gated", json!(1), None)
                    .unwrap();
                h.entered(1).await;
                h.runtime
                    .dispatch(
                        "main",
                        json!({"scope":h.scope,"action":"task.dispose","task":task.id}),
                        None,
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    h.runtime.task_snapshot(&h.scope, &task.id).unwrap().state,
                    TaskState::Cancelling
                );
                assert!(!h.gates.slow_exited.load(Ordering::Acquire));
                h.gates.slow.add_permits(1);
                h.removed(&task).await;
                h.runtime.shutdown().await.unwrap();
            });
        },
    );
}

#[test]
fn terminal_sink_panic_preserves_success_for_single_and_batch_jobs() {
    bounded(
        "terminal_sink_panic_preserves_success_for_single_and_batch_jobs",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                for batch in [false, true] {
                    let h = Harness::new(false).await;
                    h.gates.fast.add_permits(1);
                    let sink: UpdateSink = Arc::new(|value| {
                        if value["state"] == "succeeded" {
                            panic!("terminal sink");
                        }
                    });
                    let task = if batch {
                        h.runtime.run_batch(
                            &h.scope,
                            "unwind.gated",
                            json!({"inputs":[0]}),
                            Some(sink),
                        )
                    } else {
                        h.runtime
                            .run_task(&h.scope, "unwind.gated", json!(0), Some(sink))
                    }
                    .unwrap();
                    assert_eq!(h.terminal(&task).await.state, TaskState::Succeeded);
                    h.runtime.dispose_task(&h.scope, &task.id).unwrap();
                    h.removed(&task).await;
                    h.runtime.shutdown().await.unwrap();
                }
            });
        },
    );
}
