//! User sinks may call back into the runtime. A child process bounds deadlock regressions.
mod support;

use serde_json::json;
use std::{
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn bounded_case(name: &str, exercise: impl FnOnce()) {
    if std::env::var("REVENANT_REENTRY_CASE").as_deref() == Ok(name) {
        exercise();
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("REVENANT_REENTRY_CASE", name)
        .env("TEMP", temporary.path())
        .env("TMP", temporary.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{name}: callback reentry deadlocked (watchdog, not a performance assertion)");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn task_sink_can_cancel_its_running_task() {
    bounded_case("task_sink_can_cancel_its_running_task", || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let harness = support::Harness::new(0, 0, 1, 2).await;
            let runtime = harness.runtime.clone();
            let scope = harness.scope.clone();
            let called = Arc::new(AtomicBool::new(false));
            let completed = called.clone();
            let sink = Arc::new(move |value: serde_json::Value| {
                if value["state"] == "running" {
                    runtime
                        .cancel_task(&scope, value["id"].as_str().unwrap())
                        .unwrap();
                    completed.store(true, Ordering::Release);
                }
            });
            let task = harness
                .runtime
                .run_task(&harness.scope, "acceptance.hold", json!(1), Some(sink))
                .unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let snapshot = harness
                        .runtime
                        .task_snapshot(&harness.scope, &task.id)
                        .unwrap();
                    if snapshot.state.is_terminal() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert!(called.load(Ordering::Acquire));
            harness.runtime.dispose_scope(&harness.scope).unwrap();
        });
    });
}

#[test]
fn output_retention_probe_can_admit_a_stale_native_handle() {
    bounded_case(
        "output_retention_probe_can_admit_a_stale_native_handle",
        || {
            use revenant_core::{Resource, Result};
            use revenant_desktop::{DesktopOptions, DesktopRuntime};
            use revenant_sdk::{Application, OperationRegistration, Provider, ProviderDescriptor};
            use std::sync::{Mutex, atomic::AtomicUsize};
            struct Tracked(Arc<AtomicUsize>);
            impl Drop for Tracked {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
            type Target = Arc<Mutex<Option<(DesktopRuntime, String)>>>;
            struct Fixture {
                target: Target,
                called: Arc<AtomicBool>,
                drops: Arc<AtomicUsize>,
            }
            impl Provider for Fixture {
                fn descriptor(&self) -> ProviderDescriptor {
                    ProviderDescriptor {
                        id: "reentry".into(),
                        description: "Retention fixture".into(),
                        dependencies: vec![],
                        configuration_schema: json!({"type":"object"}),
                    }
                }
                fn operations(&self) -> Result<Vec<OperationRegistration>> {
                    let target = self.target.clone();
                    let called = self.called.clone();
                    let drops = self.drops.clone();
                    let create = OperationRegistration::new::<(), Resource<Tracked>, _, _>(
                        "reentry.create",
                        "Create",
                        move |_, context| {
                            let target = target.clone();
                            let called = called.clone();
                            let drops = drops.clone();
                            async move {
                                let resource = context.own(Tracked(drops))?;
                                let mut stale = serde_json::to_value(&resource)?;
                                stale["generation"] = json!(resource.handle().generation + 1);
                                let (runtime, scope) =
                                    target.lock().unwrap().as_ref().unwrap().clone();
                                context.set_cancellation_probe(Arc::new(move || {
                                    assert_eq!(
                                        runtime
                                            .run_task(&scope, "reentry.hold", stale.clone(), None)
                                            .unwrap_err()
                                            .code,
                                        "stale_handle"
                                    );
                                    called.store(true, Ordering::Release);
                                    false
                                }))?;
                                Ok(resource)
                            }
                        },
                    )?;
                    let hold = OperationRegistration::new::<Resource<Tracked>, (), _, _>(
                        "reentry.hold",
                        "Never admitted",
                        |_, _| async { panic!("stale input executed") },
                    )?;
                    Ok(vec![create, hold])
                }
            }
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let directory = tempfile::tempdir().unwrap();
                let target: Target = Arc::new(Mutex::new(None));
                let called = Arc::new(AtomicBool::new(false));
                let drops = Arc::new(AtomicUsize::new(0));
                let runtime = DesktopRuntime::new(
                    Application::new().capability(
                        Fixture {
                            target: target.clone(),
                            called: called.clone(),
                            drops: drops.clone(),
                        },
                        json!({}),
                    ),
                    DesktopOptions {
                        data_dir: Some(directory.path().join("data")),
                        cache_dir: Some(directory.path().join("cache")),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
                let scope = runtime.connect("main").unwrap()["scope"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                *target.lock().unwrap() = Some((runtime.clone(), scope.clone()));
                let task = runtime
                    .run_task(&scope, "reentry.create", serde_json::Value::Null, None)
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        let snapshot = runtime.task_snapshot(&scope, &task.id).unwrap();
                        if snapshot.state.is_terminal() {
                            assert_eq!(
                                snapshot.state,
                                revenant_core::TaskState::Succeeded,
                                "{snapshot:?}"
                            );
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                assert!(called.load(Ordering::Acquire));
                runtime.dispose_scope(&scope).unwrap();
                target.lock().unwrap().take();
                runtime.shutdown().await.unwrap();
                let ownership = runtime.runtime_inspect().unwrap()["ownership"].clone();
                assert_eq!(ownership["nativeOwners"], "0");
                assert_eq!(ownership["nativeLive"], "0");
                assert_eq!(drops.load(Ordering::SeqCst), 1);
            });
        },
    );
}

#[test]
fn view_sink_can_dispose_its_view() {
    bounded_case("view_sink_can_dispose_its_view", || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let harness = support::Harness::new(100, 0, 1, 2).await;
            let folder = harness
                .runtime
                .open_folder(&harness.scope, harness.fixture.clone())
                .unwrap();
            let runtime = harness.runtime.clone();
            let scope = harness.scope.clone();
            let called = Arc::new(AtomicBool::new(false));
            let completed = called.clone();
            let sink = Arc::new(move |value: serde_json::Value| {
                runtime
                    .dispose_view(&scope, value["view"].as_str().unwrap())
                    .unwrap();
                completed.store(true, Ordering::Release);
            });
            let initial = harness
                .runtime
                .query_folder(
                    &harness.scope,
                    folder["folder"].as_str().unwrap(),
                    json!({}),
                    Some(sink),
                )
                .unwrap();
            let view = initial["snapshot"]["view"].as_str().unwrap();
            harness
                .runtime
                .acknowledge_view(
                    &harness.scope,
                    view,
                    initial["snapshot"]["revision"].as_u64().unwrap(),
                )
                .unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                while !called.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                harness
                    .runtime
                    .dispose_view(&harness.scope, view)
                    .unwrap_err()
                    .code,
                "view_disposed"
            );
            harness.runtime.dispose_scope(&harness.scope).unwrap();
        });
    });
}
