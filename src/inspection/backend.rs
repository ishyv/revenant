//! Developer MCP authority is project-scoped; live data is authenticated through
//! a development connection. Packaged app capability exposure remains separate.
use super::{Cancellation, MAX_RESULTS, ProjectInspector, RESULT_BYTES};
use revenant_agent::{AgentTool, Backend, BackendFuture, InstanceSelector, RemoteBackend};
use revenant_core::Error;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
use tokio::sync::{Mutex, Semaphore, oneshot};

#[derive(Default)]
struct Workers {
    closed: bool,
    jobs: tokio::task::JoinSet<Option<String>>,
    failure: Option<String>,
    failure_count: usize,
}
impl Workers {
    fn record_join(&mut self, result: Result<Option<String>, tokio::task::JoinError>) {
        match result {
            Ok(Some(error)) => self.record_failure(error),
            Err(error) => self.record_failure(error),
            _ => {}
        }
    }
    fn record_failure(&mut self, error: impl std::fmt::Display) {
        self.failure_count = self.failure_count.saturating_add(1);
        if self.failure.is_none() {
            self.failure = Some(error.to_string().chars().take(2048).collect());
        }
    }
}

pub(crate) fn instances_for(root: &Path) -> Value {
    match revenant_agent::discover_instances() {
        Ok(instances) => json!(
            instances
                .into_iter()
                .filter(|instance| instance
                    .project_root
                    .as_ref()
                    .and_then(|path| dunce::canonicalize(path).ok())
                    .is_some_and(|path| path == root))
                .take(32)
                .collect::<Vec<_>>()
        ),
        Err(_) => json!([]),
    }
}

pub struct DevBackend {
    inspector: ProjectInspector,
    connections: Mutex<BTreeMap<String, Arc<RemoteBackend>>>,
    semantic: Arc<Semaphore>,
    checks: Arc<Semaphore>,
    metadata: Arc<Semaphore>,
    workers: std::sync::Mutex<Workers>,
    closing: Mutex<()>,
    cancelled: Arc<AtomicBool>,
}
impl DevBackend {
    pub fn new(inspector: ProjectInspector) -> Self {
        Self {
            inspector,
            connections: Mutex::new(BTreeMap::new()),
            semantic: Arc::new(Semaphore::new(2)),
            checks: Arc::new(Semaphore::new(1)),
            metadata: Arc::new(Semaphore::new(4)),
            workers: std::sync::Mutex::new(Workers::default()),
            closing: Mutex::new(()),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    async fn remote(&self, instance: Option<&str>) -> revenant_core::Result<Arc<RemoteBackend>> {
        let key = instance.unwrap_or("<default>").to_owned();
        let mut connections = self.connections.lock().await;
        if self
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .closed
        {
            return Err(Error::new(
                "session_closed",
                "Developer inspection session is closed",
            ));
        }
        if let Some(connection) = connections.get(&key) {
            return Ok(connection.clone());
        }
        if connections.len() >= 32 {
            return Err(Error::new(
                "instance_limit",
                "Developer session has reached its 32-instance connection budget",
            ));
        }
        let remote = Arc::new(
            RemoteBackend::connect(InstanceSelector {
                project_root: Some(self.inspector.root().to_owned()),
                instance_id: instance.map(String::from),
                development_only: true,
                ..Default::default()
            })
            .await?,
        );
        connections.insert(key, remote.clone());
        Ok(remote)
    }
    async fn run_blocking(&self, name: &str, args: Value) -> revenant_core::Result<Value> {
        let inspector = self.inspector.clone();
        let lane = match name {
            "code.query" | "operation.explain" => "semantic",
            "checks.run" => "checks",
            _ => "metadata",
        };
        let name = name.to_owned();
        let work = move |cancellation| -> anyhow::Result<Value> {
            let inspector = ProjectInspector {
                cancellation,
                ..inspector
            };
            match name.as_str() {
                "project.inspect" => inspector.snapshot(),
                "code.query" => inspector.query(args),
                "operation.explain" => inspector.explain(
                    args["operation"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("operation is required"))?,
                ),
                "diagnostics.list" => {
                    let mut diagnostics = inspector
                        .diagnostics()?
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    if let Some(stage) = args["stage"].as_str() {
                        diagnostics.retain(|entry| entry["stage"] == stage);
                    }
                    let max = args["maxResults"]
                        .as_u64()
                        .unwrap_or(100)
                        .clamp(1, MAX_RESULTS as u64) as usize;
                    let total = diagnostics.len();
                    diagnostics.truncate(max);
                    Ok(
                        json!({"diagnostics":diagnostics,"truncated":total>max,"source":"latest_build_attempt"}),
                    )
                }
                "checks.run" => inspector.check(args["profile"].as_str().unwrap_or("all")),
                _ => anyhow::bail!("unknown developer tool"),
            }
        };
        self.submit(lane, work).await
    }
    async fn submit<F>(&self, lane: &str, work: F) -> revenant_core::Result<Value>
    where
        F: FnOnce(Cancellation) -> anyhow::Result<Value> + Send + 'static,
    {
        let request = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(request.clone());
        let cancellation = Cancellation(Some((request, self.cancelled.clone())));
        let receiver = {
            let mut workers = self
                .workers
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if workers.closed {
                return Err(Error::new(
                    "session_closed",
                    "Developer inspection session is closed",
                ));
            }
            while let Some(result) = workers.jobs.try_join_next() {
                workers.record_join(result);
            }
            let budget = match lane {
                "semantic" => &self.semantic,
                "checks" => &self.checks,
                _ => &self.metadata,
            };
            let permit = budget.clone().try_acquire_owned().map_err(|_| {
                Error::new(
                    "inspection_busy",
                    "Developer inspection lane is busy; retry after current work finishes",
                )
            })?;
            let (sender, receiver) = oneshot::channel();
            workers.jobs.spawn_blocking(move || {
                // Request cancellation must not release capacity while its worker still runs.
                let _permit = permit;
                let outcome = work(cancellation);
                let cleanup = outcome
                    .as_ref()
                    .err()
                    .filter(|error| super::process_cleanup_failure(error))
                    .map(|error| format!("{error:#}").chars().take(2048).collect());
                let _ = sender.send(outcome);
                cleanup
            });
            receiver
        };
        receiver
            .await
            .map_err(|error| Error::new("inspection_task", error.to_string()))?
            .map_err(|error| Error::new("inspection_failed", format!("{error:#}")))
    }
    async fn live_call(&self, name: &str, args: &Value) -> revenant_core::Result<Value> {
        let selected = args["instanceId"].as_str();
        let mut remote = self.remote(selected).await?;
        let mut forwarded = args.clone();
        if let Some(object) = forwarded.as_object_mut() {
            object.remove("instanceId");
        }
        let operation = forwarded["operationId"].as_str().map(String::from);
        let task = forwarded["taskId"].as_str().map(String::from);
        if name == "trace.query"
            && let Some(object) = forwarded.as_object_mut()
        {
            object.remove("operationId");
            object.remove("taskId");
        }
        let mut result = match remote.call(name, forwarded.clone()).await {
            Ok(value) => value,
            Err(error)
                if matches!(name, "runtime.inspect" | "trace.query")
                    && matches!(error.code.as_str(), "bridge_io" | "session_closed") =>
            {
                // These two native tools only read state. Evict their dead
                // connection and rediscover once; operation calls never pass
                // through this path, so mutations cannot be retried here.
                let key = selected.unwrap_or("<default>");
                let mut connections = self.connections.lock().await;
                if connections
                    .get(key)
                    .is_some_and(|entry| Arc::ptr_eq(entry, &remote))
                {
                    connections.remove(key);
                }
                drop(connections);
                let _ = remote.close().await;
                remote = self.remote(selected).await?;
                remote.call(name, forwarded).await?
            }
            Err(error) => return Err(error),
        };
        if name == "trace.query" {
            let page = &mut result;
            page["instance"] = json!({"instanceId":remote.info().instance_id,"contractDigest":remote.info().contract_digest});
            if operation.is_some() || task.is_some() {
                if let Some(events) = page["events"].as_array_mut() {
                    events.retain(|event| {
                        operation
                            .as_ref()
                            .is_none_or(|id| event["operationId"] == *id)
                            && task.as_ref().is_none_or(|id| event["taskId"] == *id)
                    });
                }
                page["filterCoverage"] = json!("returned_bounded_page");
            }
        }
        Ok(result)
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn tool(name: &str, description: &str, input_schema: Value, read_only: bool) -> AgentTool {
    AgentTool {
        name: name.into(),
        description: description.into(),
        input_schema,
        output_schema: None,
        read_only,
    }
}
impl Backend for DevBackend {
    fn tools(&self) -> Vec<AgentTool> {
        let limit = json!({"type":"integer","minimum":1,"maximum":MAX_RESULTS,"default":100});
        vec![
            tool(
                "project.inspect",
                "Inspect source fingerprint, SDK compatibility, published contracts, latest build diagnostics and the authenticated development runtime.",
                schema(json!({"instanceId":{"type":"string"}}), &[]),
                true,
            ),
            tool(
                "code.query",
                "Query compiler/LSP definitions, references and cross-language impact. Supply an operation ID, or a project-relative file with one-based line and column. Unavailable tools and dynamic references are explicit.",
                schema(
                    json!({"operation":{"type":"string"},"file":{"type":"string"},"line":{"type":"integer","minimum":1},"column":{"type":"integer","minimum":1},"symbol":{"type":"string"},"maxResults":limit}),
                    &[],
                ),
                true,
            ),
            tool(
                "operation.explain",
                "Explain an operation's compiled contracts, effects, scheduling, agent exposure, Rust source and semantic frontend consumers.",
                schema(
                    json!({"operation":{"type":"string"},"includeConsumers":{"type":"boolean","default":true}}),
                    &["operation"],
                ),
                true,
            ),
            tool(
                "diagnostics.list",
                "Read current structured compiler, frontend, contract and runtime diagnostics from the latest build attempt. Run checks.run to refresh compiler evidence.",
                schema(json!({"stage":{"type":"string"},"maxResults":limit}), &[]),
                true,
            ),
            tool(
                "runtime.inspect",
                "Inspect native tasks, execution budgets, ownership, cleanup failures and bounded frontend state from a running development instance.",
                schema(json!({"instanceId":{"type":"string"}}), &[]),
                true,
            ),
            tool(
                "trace.query",
                "Read a bounded payload-free trace page. Optional operation/task filtering applies to the returned page, with pagination and dropped-event evidence retained.",
                schema(
                    json!({"instanceId":{"type":"string"},"afterSequence":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"maximum":200,"default":100},"operationId":{"type":"string"},"taskId":{"type":"string"}}),
                    &[],
                ),
                true,
            ),
            tool(
                "checks.run",
                "Run one predefined check profile (all, rust, frontend or sdk), collect stable diagnostics and preserve authored source and the last published facade. No arbitrary shell commands.",
                schema(
                    json!({"profile":{"type":"string","enum":["all","rust","frontend","sdk"],"default":"all"}}),
                    &[],
                ),
                false,
            ),
        ]
    }
    fn call<'a>(&'a self, name: &'a str, args: Value) -> BackendFuture<'a, Value> {
        Box::pin(async move {
            let tool = self
                .tools()
                .into_iter()
                .find(|tool| tool.name == name)
                .ok_or_else(|| Error::new("unknown_tool", "Unknown developer tool"))?;
            // Enforce schema at this authority boundary; MCP metadata alone is not validation.
            revenant_core::validate_json(&tool.input_schema, &args)?;
            let result = match name {
                "runtime.inspect" | "trace.query" => self.live_call(name, &args).await?,
                "checks.run" => self.run_blocking(name, args).await?,
                "project.inspect" => {
                    let mut snapshot = self.run_blocking(name, args.clone()).await?;
                    match self.live_call("runtime.inspect", &args).await {
                        Ok(state) => super::attach_runtime(&mut snapshot, state),
                        Err(error) => {
                            snapshot["runtime"] = json!({"status":"unavailable","error":error});
                            snapshot["consistency"]["sourceToRuntime"] = json!("unknown");
                            snapshot["consistency"]["selectedFacadeToRuntime"] = json!("unknown");
                        }
                    }
                    snapshot
                }
                "operation.explain" => {
                    let mut explanation = self.run_blocking(name, args.clone()).await?;
                    if args["includeConsumers"] != false {
                        explanation["index"] = self
                            .run_blocking(
                                "code.query",
                                json!({"operation":args["operation"],"maxResults":100}),
                            )
                            .await?;
                    }
                    explanation
                }
                "code.query" => {
                    let mut query = self.run_blocking(name, args.clone()).await?;
                    if let Some(operation) = args["operation"].as_str() {
                        match self
                            .live_call(
                                "trace.query",
                                &json!({"operationId":operation,"limit":100,"afterSequence":0}),
                            )
                            .await
                        {
                            Ok(page) => super::attach_runtime_traces(&mut query, page),
                            Err(error) => {
                                query["runtimeTraces"] =
                                    json!({"status":"unavailable","error":error})
                            }
                        };
                    }
                    query
                }
                "diagnostics.list" => {
                    let mut report = self.run_blocking(name, args.clone()).await?;
                    match self.live_call("runtime.inspect", &json!({})).await {
                        Ok(state) => {
                            let mut live = super::runtime_diagnostics(&state);
                            if let Some(stage) = args["stage"].as_str() {
                                live.retain(|entry| entry["stage"] == stage);
                            }
                            let limit = args["maxResults"].as_u64().unwrap_or(100) as usize;
                            let current = report["diagnostics"].as_array().map_or(0, Vec::len);
                            if current + live.len() > limit {
                                report["truncated"] = json!(true);
                            }
                            if let Some(diagnostics) = report["diagnostics"].as_array_mut() {
                                diagnostics
                                    .extend(live.into_iter().take(limit.saturating_sub(current)));
                            }
                            report["runtime"] =
                                json!({"status":"connected","instanceId":state["instanceId"]});
                        }
                        Err(error) => {
                            report["runtime"] = json!({"status":"unavailable","error":error})
                        }
                    }
                    report["source"] = json!("latest_build_and_check_attempts_and_live_runtime");
                    report
                }
                _ => self.run_blocking(name, args).await?,
            };
            if serde_json::to_vec(&result)?.len() > RESULT_BYTES {
                return Err(Error::new(
                    "inspection_result_limit",
                    "Inspection result exceeds 1 MiB; narrow the query or result limit",
                ));
            }
            Ok(result)
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            let _closing = self.closing.lock().await;
            self.cancelled.store(true, Ordering::Release);
            self.workers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .closed = true;
            // Keep joins in the backend even if this close future is dropped.
            while let Some(result) = std::future::poll_fn(|cx| {
                self.workers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .jobs
                    .poll_join_next(cx)
            })
            .await
            {
                self.workers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .record_join(result);
            }
            loop {
                let next = self
                    .connections
                    .lock()
                    .await
                    .iter()
                    .next()
                    .map(|(key, connection)| (key.clone(), connection.clone()));
                let Some((key, connection)) = next else { break };
                if let Err(error) = connection.close().await {
                    self.workers
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .record_failure(error);
                }
                // Retain ownership across the await so a cancelled close can retry.
                self.connections.lock().await.remove(&key);
            }
            let workers = self
                .workers
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if workers.failure.is_none() {
                Ok(())
            } else {
                Err(Error::new(
                    "inspection_cleanup",
                    format!(
                        "{} cleanup failure(s); first: {}",
                        workers.failure_count,
                        workers.failure.as_deref().unwrap_or("unknown")
                    ),
                ))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_request_retains_cleanup_failure_for_close() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'cleanup-error'\n",
        )
        .unwrap();
        let backend = Arc::new(DevBackend::new(
            ProjectInspector::open(root.path()).unwrap(),
        ));
        let (entered, started) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let worker_backend = backend.clone();
        let worker = tokio::spawn(async move {
            worker_backend
                .submit("checks", move |_| {
                    let _ = entered.send(());
                    wait.recv()?;
                    Err(
                        Error::new("inspection_process_cleanup", "injected termination failure")
                            .into(),
                    )
                })
                .await
        });
        started.await.unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        let error = backend
            .close()
            .await
            .expect_err("cleanup failure must survive a dropped response receiver");
        assert!(error.to_string().contains("injected termination failure"));
    }

    #[tokio::test]
    async fn repeated_worker_panics_keep_bounded_failure_evidence() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'panic-budget'\n",
        )
        .unwrap();
        let backend = DevBackend::new(ProjectInspector::open(root.path()).unwrap());
        for _ in 0..12 {
            assert!(
                backend
                    .submit("metadata", |_| panic!("fixture worker panic"))
                    .await
                    .is_err()
            );
        }
        assert!(backend.close().await.is_err());
        let workers = backend.workers.lock().unwrap();
        assert!(
            workers
                .failure
                .as_ref()
                .is_some_and(|message| message.len() <= 8192),
            "only the first cleanup failure needs retained text"
        );
    }

    #[tokio::test]
    async fn cancelling_remote_cleanup_retains_registry_for_retry() {
        struct PendingRuntime(Arc<tokio::sync::Notify>);
        impl Backend for PendingRuntime {
            fn tools(&self) -> Vec<AgentTool> {
                vec![AgentTool {
                    name: "runtime.inspect".into(),
                    description: "probe".into(),
                    input_schema: json!({"type":"object"}),
                    output_schema: None,
                    read_only: true,
                }]
            }
            fn call<'a>(&'a self, _: &'a str, _: Value) -> BackendFuture<'a, Value> {
                Box::pin(async move {
                    self.0.notify_one();
                    std::future::pending().await
                })
            }
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'remote-close'\n",
        )
        .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let server = revenant_agent::BridgeServer::start(
            revenant_agent::InstanceInfo::new(
                "close-probe",
                "sha256:fixture",
                Some(root.path().to_owned()),
                true,
            ),
            Arc::new(move |_| Ok(Arc::new(PendingRuntime(signal.clone())))),
        )
        .await
        .unwrap();
        let backend = DevBackend::new(ProjectInspector::open(root.path()).unwrap());
        let remote = backend
            .remote(Some(&server.info().instance_id))
            .await
            .unwrap();
        let call = tokio::spawn(async move { remote.call("runtime.inspect", json!({})).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        let mut close = backend.close();
        let pending =
            std::future::poll_fn(|cx| std::task::Poll::Ready(close.as_mut().poll(cx).is_pending()))
                .await;
        drop(close);
        let retained = backend.connections.lock().await.len();
        backend.close().await.unwrap();
        let _ = call.await.unwrap();
        server.stop().await.unwrap();
        assert!(pending, "remote cleanup seam must be pending");
        assert_eq!(
            retained, 1,
            "pending cleanup must stay owned by the registry"
        );
        assert!(backend.connections.lock().await.is_empty());
    }

    #[tokio::test]
    async fn dropped_request_signals_worker_cancellation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'cancel'\n",
        )
        .unwrap();
        let backend = Arc::new(DevBackend::new(
            ProjectInspector::open(root.path()).unwrap(),
        ));
        let (entered, started) = tokio::sync::oneshot::channel();
        let (observe, observed) = tokio::sync::oneshot::channel();
        let worker_backend = backend.clone();
        let worker = tokio::spawn(async move {
            worker_backend
                .submit("checks", move |cancellation| {
                    let _ = entered.send(());
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    while cancellation.checkpoint().is_ok() && std::time::Instant::now() < deadline
                    {
                        std::thread::yield_now();
                    }
                    let _ = observe.send(cancellation.checkpoint().is_err());
                    Ok(Value::Null)
                })
                .await
        });
        started.await.unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        let cancelled = observed.await.unwrap();
        backend.close().await.unwrap();
        assert!(
            cancelled,
            "dropping a request must signal its owned blocking worker"
        );
    }

    #[tokio::test]
    async fn dropped_request_retains_admission_and_close_joins_worker() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'admission'\n",
        )
        .unwrap();
        let backend = Arc::new(DevBackend::new(
            ProjectInspector::open(root.path()).unwrap(),
        ));
        let (release, wait) = std::sync::mpsc::channel();
        let (entered, started) = tokio::sync::oneshot::channel();
        let worker_backend = backend.clone();
        let worker = tokio::spawn(async move {
            worker_backend
                .submit("checks", move |_| {
                    let _ = entered.send(());
                    wait.recv()?;
                    Ok(Value::Null)
                })
                .await
        });
        started.await.unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        let extra = backend.submit("checks", |_| Ok(Value::Null)).await;
        let mut close = backend.close();
        let closed_early =
            std::future::poll_fn(|cx| std::task::Poll::Ready(close.as_mut().poll(cx).is_ready()))
                .await;
        release.send(()).unwrap();
        if !closed_early {
            close.await.unwrap();
        }
        assert!(
            extra.is_err(),
            "dropped requester must not free worker capacity"
        );
        assert!(
            !closed_early,
            "close must join admitted work, not detach it"
        );
        assert!(
            backend
                .submit("metadata", |_| Ok(Value::Null))
                .await
                .is_err(),
            "closed backends must reject work"
        );
    }

    #[tokio::test]
    async fn semantic_admission_is_bounded_without_serializing_metadata() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("revenant.toml"),
            "version = 3\n[project]\nname = 'admission'\n",
        )
        .unwrap();
        let backend = Arc::new(DevBackend::new(
            ProjectInspector::open(root.path()).unwrap(),
        ));
        let mut releases = Vec::new();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let (release, wait) = std::sync::mpsc::channel();
            let (entered, started) = tokio::sync::oneshot::channel();
            let backend = backend.clone();
            workers.push(tokio::spawn(async move {
                backend
                    .submit("semantic", move |_| {
                        let _ = entered.send(());
                        wait.recv()?;
                        Ok(Value::Null)
                    })
                    .await
            }));
            started.await.unwrap();
            releases.push(release);
        }
        let extra = backend.submit("semantic", |_| Ok(Value::Null)).await;
        let metadata = backend.submit("metadata", |_| Ok(Value::Null)).await;
        // Release before asserting so a failed regression cannot strand blocking workers.
        for release in releases {
            release.send(()).unwrap();
        }
        for worker in workers {
            worker.await.unwrap().unwrap();
        }
        assert!(
            extra.is_err(),
            "third semantic worker must be rejected, not queued"
        );
        assert!(
            metadata.is_ok(),
            "semantic saturation must not block metadata"
        );
    }
}
