use revenant_core::{FileEntry, Result, TaskSnapshot, TaskState};
use revenant_desktop::{DesktopOptions, DesktopRuntime};
use revenant_sdk::{Application, OperationRegistration, Provider, ProviderDescriptor};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Semaphore;

struct GateProvider {
    independent: bool,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
impl Provider for GateProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "gates".into(),
            description: "Real execution gates".into(),
            dependencies: vec![],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let active = self.active.clone();
        let maximum = self.maximum.clone();
        let entered = self.entered.clone();
        let release = self.release.clone();
        let mut operation = OperationRegistration::new::<u32, u32, _, _>(
            "gates.hold",
            "Hold until released",
            move |input, context| {
                let active = active.clone();
                let maximum = maximum.clone();
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(count, Ordering::SeqCst);
                    entered.add_permits(1);
                    loop {
                        if let Err(error) = context.checkpoint() {
                            active.fetch_sub(1, Ordering::SeqCst);
                            return Err(error);
                        }
                        tokio::select! {
                            permit = release.acquire() => { permit.unwrap().forget(); break; }
                            _ = tokio::time::sleep(Duration::from_millis(5)) => {}
                        }
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(input)
                }
            },
        )?;
        operation.descriptor.independent = self.independent;
        operation.descriptor.agent = true;
        Ok(vec![
            operation,
            OperationRegistration::new::<u32, u32, _, _>(
                "gates.hidden",
                "Not exposed",
                |input, _| async move { Ok(input) },
            )?,
        ])
    }
}

struct Harness {
    _directory: tempfile::TempDir,
    runtime: DesktopRuntime,
    scope: String,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
impl Harness {
    async fn new(independent: bool, concurrency: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let runtime = DesktopRuntime::new(
            Application::new().capability(
                GateProvider {
                    independent,
                    active: active.clone(),
                    maximum: maximum.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                },
                json!({}),
            ),
            DesktopOptions {
                data_dir: Some(directory.path().join("data")),
                cache_dir: Some(directory.path().join("cache")),
                concurrency,
                agents_enabled: true,
                trace_capacity: 16,
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
            active,
            maximum,
            entered,
            release,
        }
    }
    async fn entered(&self, count: u32) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.acquire_many(count))
            .await
            .expect("declared independent items must overlap")
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
        .unwrap()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.release.add_permits(512);
        let _ = self.runtime.dispose_scope(&self.scope);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_batch_items_overlap_under_the_global_worker_budget() {
    // Two bulk slots plus the reserved interactive slot.
    let h = Harness::new(true, 3).await;
    let batch = h
        .runtime
        .run_batch(&h.scope, "gates.hold", json!({"inputs":[0,1,2,3]}), None)
        .unwrap();
    h.entered(2).await;
    let single = h
        .runtime
        .run_task(&h.scope, "gates.hold", json!(9), None)
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(h.active.load(Ordering::SeqCst), 2);
    h.release.add_permits(8);
    assert_eq!(h.terminal(&batch).await.summary.succeeded.get(), 4);
    assert_eq!(h.terminal(&single).await.state, TaskState::Succeeded);
    assert_eq!(h.maximum.load(Ordering::SeqCst), 2);
    let page = h.runtime.task_results(&h.scope, &batch.id, 0, 128).unwrap();
    let ordinals: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["ordinal"].as_str().unwrap())
        .collect();
    assert_eq!(ordinals, ["0", "1", "2", "3"]);
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unmarked_batch_items_remain_sequential() {
    let h = Harness::new(false, 3).await;
    let batch = h
        .runtime
        .run_batch(&h.scope, "gates.hold", json!({"inputs":[0,1,2]}), None)
        .unwrap();
    h.entered(1).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(h.active.load(Ordering::SeqCst), 1);
    h.release.add_permits(3);
    assert_eq!(h.terminal(&batch).await.summary.succeeded.get(), 3);
    assert_eq!(h.maximum.load(Ordering::SeqCst), 1);
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_parallel_batch_retains_completed_items() {
    let h = Harness::new(true, 3).await;
    let batch = h
        .runtime
        .run_batch(&h.scope, "gates.hold", json!({"inputs":[0,1,2,3]}), None)
        .unwrap();
    h.entered(2).await;
    h.release.add_permits(1);
    h.entered(1).await;
    h.runtime.cancel_task(&h.scope, &batch.id).unwrap();
    let done = h.terminal(&batch).await;
    assert_eq!(done.state, TaskState::Partial);
    assert_eq!(done.summary.succeeded.get(), 1);
    assert_eq!(
        h.runtime.task_results(&h.scope, &batch.id, 0, 128).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(h.active.load(Ordering::SeqCst), 0);
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_sessions_enforce_exposure_and_task_scope_isolation() {
    let h = Harness::new(false, 2).await;
    let first = h.runtime.open_agent_session().unwrap();
    let second = h.runtime.open_agent_session().unwrap();
    assert_eq!(first.list_tools().unwrap().len(), 1);
    assert_eq!(
        first.run("gates.hidden", json!(1)).unwrap_err().code,
        "operation_not_exposed"
    );
    assert_eq!(
        h.runtime
            .validate_window(first.scope(), "main")
            .unwrap_err()
            .code,
        "scope_mismatch"
    );
    let task = first.run("gates.hold", json!(7)).unwrap();
    h.entered(1).await;
    assert_eq!(
        second.snapshot(&task.id).unwrap_err().code,
        "scope_mismatch"
    );
    assert_eq!(second.cancel(&task.id).unwrap_err().code, "scope_mismatch");
    first.close().await.unwrap();
    assert_eq!(h.active.load(Ordering::SeqCst), 0);
    second.close().await.unwrap();
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_agent_session_requests_cancellation_and_releases_history() {
    let h = Harness::new(false, 2).await;
    let session = h.runtime.open_agent_session().unwrap();
    let scope = session.scope().to_owned();
    let task = session.run("gates.hold", json!(7)).unwrap();
    h.entered(1).await;
    drop(session);
    tokio::time::timeout(Duration::from_secs(2), async {
        while h.active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        while h.runtime.task_snapshot(&scope, &task.id).is_ok() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        h.runtime.runtime_inspect().unwrap()["ownership"]["scopes"],
        1
    );
    h.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn agent_access_is_disabled_by_default() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = DesktopRuntime::new(
        Application::new(),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(matches!(runtime.open_agent_session(), Err(error) if error.code == "agents_disabled"));
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_snapshot_and_trace_are_bounded_and_payload_free() {
    let h = Harness::new(false, 2).await;
    let task = h.runtime.dispatch("main", json!({"requestId":"request-1", "scope":h.scope, "action":"task.run", "operation":"gates.hold", "input":42}), None).await.unwrap();
    h.entered(1).await;
    let snapshot = h.runtime.runtime_inspect().unwrap();
    assert_eq!(snapshot["appId"], "dev.revenant.application");
    assert!(
        snapshot["instanceId"]
            .as_str()
            .unwrap()
            .starts_with("instance-")
    );
    assert!(
        snapshot["contractDigest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(snapshot["execution"]["activeWorkers"], 1);
    assert_eq!(snapshot["ownership"]["scopes"], 1);
    h.release.add_permits(1);
    let typed: TaskSnapshot = serde_json::from_value(task).unwrap();
    h.terminal(&typed).await;
    let correlated = h.runtime.trace_query(0, 128).unwrap();
    assert!(
        correlated["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["requestId"] == "request-1"
                && event["taskId"] == typed.id
                && event["operationId"] == "gates.hold"
                && event["origin"] == "frontend"
                && event["state"] == "succeeded")
    );
    for _ in 0..20 {
        h.runtime.dispatch("main", json!({"requestId":"observe", "scope":h.scope,"action":"task.snapshot","task":typed.id}), None).await.unwrap();
    }
    let trace = h.runtime.trace_query(0, 512).unwrap();
    assert!(trace["events"].as_array().unwrap().len() <= 16);
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(!serialized.contains("\"input\""));
    assert!(!serialized.contains("\"result\""));
    assert!(trace["dropped"].as_u64().unwrap() > 0);
    h.runtime.shutdown().await.unwrap();
}

#[derive(Clone)]
struct FileHoldingProvider {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    read_after_disposal: Arc<AtomicUsize>,
}
impl Provider for FileHoldingProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "holding".into(),
            description: "Hold captured native files".into(),
            dependencies: vec!["files".into()],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let gates = self.clone();
        let mut operation = OperationRegistration::new::<FileEntry, String, _, _>(
            "holding.read",
            "Read an admitted file after owner disposal",
            move |input, context| {
                let gates = gates.clone();
                async move {
                    let lease = context.lease(&input.handle)?;
                    gates.entered.add_permits(1);
                    gates.release.acquire().await.unwrap().forget();
                    assert_eq!(lease.read(0, 3).await?, b"abc");
                    gates.read_after_disposal.fetch_add(1, Ordering::SeqCst);
                    Ok(input.name)
                }
            },
        )?;
        operation.descriptor.independent = true;
        operation.descriptor.agent = true;
        Ok(vec![operation])
    }
    fn reconfigured(&self, _config: serde_json::Value) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(self.clone()))
    }
}

async fn file_view(
    runtime: &DesktopRuntime,
    scope: &str,
    path: std::path::PathBuf,
) -> serde_json::Value {
    let folder = runtime.open_folder(scope, path).unwrap();
    let publications = Arc::new(Mutex::new(None));
    let seen = publications.clone();
    let mut snapshot = runtime
        .query_folder(
            scope,
            folder["folder"].as_str().unwrap(),
            json!({}),
            Some(Arc::new(move |snapshot| {
                *seen.lock().unwrap() = Some(snapshot);
            })),
        )
        .unwrap()["snapshot"]
        .clone();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if snapshot["state"] == "ready" {
                return snapshot;
            }
            runtime
                .acknowledge_view(
                    scope,
                    snapshot["view"].as_str().unwrap(),
                    snapshot["revision"].as_u64().unwrap(),
                )
                .unwrap();
            if let Some(latest) = publications.lock().unwrap().take() {
                snapshot = latest;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_file_readers_retain_provider_and_file_leases_until_exit() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("files");
    std::fs::create_dir(&fixture).unwrap();
    for name in ["a", "b"] {
        std::fs::write(fixture.join(name), b"abc").unwrap();
    }
    let provider = FileHoldingProvider {
        entered: Arc::new(Semaphore::new(0)),
        release: Arc::new(Semaphore::new(0)),
        read_after_disposal: Arc::new(AtomicUsize::new(0)),
    };
    let runtime = DesktopRuntime::new(
        Application::new().capability(provider.clone(), json!({})),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            concurrency: 3,
            agents_enabled: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scope = runtime.connect("files").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let view = file_view(&runtime, &scope, fixture.clone()).await;
    let first_session = runtime.open_agent_session().unwrap();
    assert_eq!(
        first_session
            .run("holding.read", view["items"][0].clone())
            .unwrap_err()
            .code,
        "scope_mismatch"
    );
    first_session.close().await.unwrap();
    let task = runtime
        .run_batch(
            &scope,
            "holding.read",
            json!({"view":view["view"],"allMatching":true,"generation":view["generation"]}),
            None,
        )
        .unwrap();
    let overlap =
        tokio::time::timeout(Duration::from_secs(2), provider.entered.acquire_many(2)).await;
    if overlap.is_err() {
        provider.release.add_permits(2);
    }
    overlap
        .expect("independent file readers must overlap")
        .unwrap()
        .forget();
    assert_eq!(runtime.dispatch("files", json!({"scope":scope,"action":"runtime.configure","provider":"holding","config":{}}), None).await.unwrap_err().code, "provider_busy");
    runtime.dispose_scope(&scope).unwrap();
    let snapshot = runtime.runtime_inspect().unwrap();
    assert_eq!(snapshot["ownership"]["resources"], "2");
    assert_eq!(snapshot["execution"]["activeWorkers"], 2);
    assert!(
        snapshot["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|current| current["id"] == task.id && current["executionFinished"] == false)
    );
    provider.release.add_permits(2);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if runtime.runtime_inspect().unwrap()["ownership"]["resources"] == "0" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(provider.read_after_disposal.load(Ordering::SeqCst), 2);
    runtime.shutdown().await.unwrap();
}

struct OrderingProvider {
    entered: Arc<Semaphore>,
    first_release: Arc<Semaphore>,
    finished: Arc<Mutex<Vec<u32>>>,
}
impl Provider for OrderingProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "ordering".into(),
            description: "Finish later ordinals first".into(),
            dependencies: vec![],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let entered = self.entered.clone();
        let release = self.first_release.clone();
        let finished = self.finished.clone();
        let mut operation = OperationRegistration::new::<u32, u32, _, _>(
            "ordering.run",
            "Release ordinal zero last",
            move |input, _| {
                let entered = entered.clone();
                let release = release.clone();
                let finished = finished.clone();
                async move {
                    entered.add_permits(1);
                    if input == 0 {
                        release.acquire().await.unwrap().forget();
                    }
                    finished.lock().unwrap().push(input);
                    Ok(input)
                }
            },
        )?;
        operation.descriptor.independent = true;
        Ok(vec![operation])
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn out_of_order_completions_are_returned_in_source_ordinal_order() {
    let directory = tempfile::tempdir().unwrap();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let finished = Arc::new(Mutex::new(Vec::new()));
    let runtime = DesktopRuntime::new(
        Application::new().capability(
            OrderingProvider {
                entered: entered.clone(),
                first_release: release.clone(),
                finished: finished.clone(),
            },
            json!({}),
        ),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            concurrency: 3,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scope = runtime.connect("ordering").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = runtime
        .run_batch(&scope, "ordering.run", json!({"inputs":[0,1,2,3]}), None)
        .unwrap();
    let overlaps = tokio::time::timeout(Duration::from_secs(3), entered.acquire_many(4)).await;
    release.add_permits(1);
    overlaps
        .expect("later items must finish while ordinal zero is still held")
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !runtime
            .task_snapshot(&scope, &task.id)
            .unwrap()
            .state
            .is_terminal()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_ne!(finished.lock().unwrap()[0], 0);
    let page = runtime.task_results(&scope, &task.id, 0, 128).unwrap();
    assert_eq!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["output"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    runtime.shutdown().await.unwrap();
}

struct UncooperativeGateProvider {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    exited: Arc<Semaphore>,
}
impl Provider for UncooperativeGateProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "uncooperative".into(),
            description: "Native cleanup deadline gate".into(),
            dependencies: vec![],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let exited = self.exited.clone();
        let mut operation = OperationRegistration::new::<u32, u32, _, _>(
            "uncooperative.hold",
            "Ignores cancellation until released",
            move |input, _context| {
                let entered = entered.clone();
                let release = release.clone();
                let exited = exited.clone();
                async move {
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    exited.add_permits(1);
                    Ok(input)
                }
            },
        )?;
        operation.descriptor.agent = true;
        operation.descriptor.independent = true;
        Ok(vec![operation])
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnected_uncooperative_agent_keeps_workers_until_exit_and_reports_cleanup_timeout() {
    let directory = tempfile::tempdir().unwrap();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let exited = Arc::new(Semaphore::new(0));
    let runtime = DesktopRuntime::new(
        Application::new().capability(
            UncooperativeGateProvider {
                entered: entered.clone(),
                release: release.clone(),
                exited: exited.clone(),
            },
            json!({}),
        ),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            concurrency: 2,
            agents_enabled: true,
            shutdown_grace: Duration::from_millis(30),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let session = runtime.open_agent_session().unwrap();
    let task = session.run("uncooperative.hold", json!(7)).unwrap();
    entered.acquire().await.unwrap().forget();
    let closing = session.close().await;
    let live = runtime.runtime_inspect().unwrap();
    let cleanup = live["cleanupFailures"].clone();
    // Always release native gates before assertions, including a failed implementation.
    release.add_permits(1);
    exited.acquire().await.unwrap().forget();
    assert_eq!(closing.unwrap_err().code, "session_cleanup_timeout");
    assert_eq!(live["execution"]["activeWorkers"], 1);
    assert_eq!(live["ownership"]["scopes"], 0);
    assert!(
        live["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|current| current["id"] == task.id && current["executionFinished"] == false)
    );
    assert!(
        cleanup
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error["code"] == "session_cleanup_timeout")
    );
    runtime.shutdown().await.unwrap();
}

struct AdmissionGate {
    entered: Semaphore,
    released: (Mutex<bool>, std::sync::Condvar),
}
struct AdmissionExecutor {
    inner: Arc<dyn revenant_sdk::OperationExecutor>,
    gate: Arc<AdmissionGate>,
}
impl revenant_sdk::OperationExecutor for AdmissionExecutor {
    fn validate_input(
        &self,
        input: &serde_json::Value,
        context: &revenant_core::TaskContext,
    ) -> Result<()> {
        self.inner.validate_input(input, context)
    }
    fn input_handles(&self, input: &serde_json::Value) -> Result<Vec<revenant_core::Handle>> {
        self.gate.entered.add_permits(1);
        let (released, changed) = &self.gate.released;
        let mut released = released.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
        self.inner.input_handles(input)
    }
    fn output_handles(&self, output: &serde_json::Value) -> Result<Vec<revenant_core::Handle>> {
        self.inner.output_handles(output)
    }
    fn execute(
        &self,
        input: serde_json::Value,
        context: revenant_core::TaskContext,
    ) -> revenant_core::BoxFuture<'_, Result<serde_json::Value>> {
        self.inner.execute(input, context)
    }
}
struct AdmissionProvider(Arc<AdmissionGate>);
impl Provider for AdmissionProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "admission".into(),
            description: "Controlled capability admission".into(),
            dependencies: vec![],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let mut registration = OperationRegistration::new::<u32, u32, _, _>(
            "admission.echo",
            "Echo after input admission",
            |input, _| async move { Ok(input) },
        )?;
        registration.descriptor.agent = true;
        registration.executor = Arc::new(AdmissionExecutor {
            inner: registration.executor,
            gate: self.0.clone(),
        });
        Ok(vec![registration])
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_agent_close_waits_for_input_admission_before_collecting_owned_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let gate = Arc::new(AdmissionGate {
        entered: Semaphore::new(0),
        released: (Mutex::new(false), std::sync::Condvar::new()),
    });
    let runtime = DesktopRuntime::new(
        Application::new().capability(AdmissionProvider(gate.clone()), json!({})),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            agents_enabled: true,
            shutdown_grace: Duration::from_millis(30),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let session = Arc::new(runtime.open_agent_session().unwrap());
    let runner = session.clone();
    let running = tokio::task::spawn_blocking(move || runner.run("admission.echo", json!(7)));
    gate.entered.acquire().await.unwrap().forget();
    let closing = session.clone();
    let closed = tokio::spawn(async move { closing.close().await });
    // Input admission is deliberately longer than the cancellation grace period.
    // Closing must capture an admitted task, never cancel a private preflight job.
    tokio::time::sleep(Duration::from_millis(100)).await;
    *gate.released.0.lock().unwrap() = true;
    gate.released.1.notify_all();
    let admitted = running.await.unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(2), closed)
        .await
        .unwrap()
        .unwrap();
    assert!(
        admitted.is_ok(),
        "admission begun before disconnect must complete before closing: {admitted:?}"
    );
    closed.expect("disconnect must await the admitted executor rather than time out on an abandoned private job");
    assert_eq!(runtime.runtime_inspect().unwrap()["ownership"]["tasks"], 0);
    assert_eq!(
        session.run("admission.echo", json!(8)).unwrap_err().code,
        "session_closed"
    );
    runtime.shutdown().await.unwrap();
}

struct FailureProvider;
impl Provider for FailureProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "failures".into(),
            description: "Batch diagnostic evidence".into(),
            dependencies: vec![],
            configuration_schema: json!({"type":"object"}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let mut operation = OperationRegistration::new::<String, String, _, _>(
            "failures.item",
            "Controlled item failures with private error payloads",
            |input, _| async move {
                if input.starts_with("fail") {
                    Err(
                        revenant_core::Error::new("fixture_failure", "private-error-message")
                            .details(json!({"private-error-details":input})),
                    )
                } else {
                    Ok(format!("private-output:{input}"))
                }
            },
        )?;
        operation.descriptor.independent = true;
        Ok(vec![operation])
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_and_failed_frontend_batches_trace_item_errors_without_payloads() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = DesktopRuntime::new(
        Application::new().capability(FailureProvider, json!({})),
        DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            concurrency: 2,
            trace_capacity: 16,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let scope = runtime.connect("diagnostic-window").unwrap()["scope"]
        .as_str()
        .unwrap()
        .to_owned();
    for (request, inputs, expected_state, ordinals) in [
        (
            "mixed-request",
            vec![
                "okay-private-input",
                "fail-private-input-1",
                "fail-private-input-2",
            ],
            TaskState::Partial,
            vec!["1", "2"],
        ),
        (
            "failed-request",
            vec!["fail-private-input-1", "fail-private-input-2"],
            TaskState::Failed,
            vec!["0", "1"],
        ),
    ] {
        let task = runtime.dispatch("diagnostic-window", json!({"requestId":request,"scope":scope,"action":"task.batch","operation":"failures.item","selection":{"inputs":inputs}}), None).await.unwrap();
        let task_id = task["id"].as_str().unwrap();
        let snapshot = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let snapshot = runtime.task_snapshot(&scope, task_id).unwrap();
                if snapshot.state.is_terminal() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(snapshot.state, expected_state);
        assert!(
            snapshot.error.is_none(),
            "per-item errors belong to native outcomes"
        );
        let trace = runtime.trace_query(0, 512).unwrap();
        let mut failed = trace["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["action"] == "task.item_failed" && event["taskId"] == task_id)
            .collect::<Vec<_>>();
        failed.sort_by_key(|event| event["ordinal"].as_str().unwrap());
        assert_eq!(
            failed.len(),
            ordinals.len(),
            "each noncancellation item failure must be observable"
        );
        for (event, ordinal) in failed.into_iter().zip(ordinals) {
            assert_eq!(event["ordinal"], ordinal);
            assert_eq!(event["errorCode"], "fixture_failure");
            assert_eq!(event["operationId"], "failures.item");
            assert_eq!(event["requestId"], request);
            assert_eq!(event["origin"], "frontend");
            assert_eq!(event["principal"], "diagnostic-window");
        }
        assert!(!serde_json::to_string(&trace).unwrap().contains("private-"));
        runtime.dispose_task(&scope, task_id).unwrap();
    }
    let saturated = runtime.dispatch("diagnostic-window", json!({"requestId":"bounded-request","scope":scope,"action":"task.batch","operation":"failures.item","selection":{"inputs":vec!["fail-private-input";64]}}), None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !runtime
            .task_snapshot(&scope, saturated["id"].as_str().unwrap())
            .unwrap()
            .state
            .is_terminal()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let trace = runtime.trace_query(0, 512).unwrap();
    assert!(trace["events"].as_array().unwrap().len() <= 16);
    assert!(trace["dropped"].as_u64().unwrap() > 0);
    assert!(!serde_json::to_string(&trace).unwrap().contains("private-"));
    runtime.shutdown().await.unwrap();
}
