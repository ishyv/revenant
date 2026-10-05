#![allow(dead_code)]

pub mod benchmark;

use revenant_core::{BoxFuture, FileEntry, TaskContext, TaskSnapshot};
use revenant_desktop::{DesktopOptions, DesktopRuntime, Error, Result, UpdateSink};
use revenant_sdk::{
    Application, OperationRegistration, Provider, ProviderContext, ProviderDescriptor,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

// Deadlines detect hung executors; none of the assertions grade performance.
pub const WATCHDOG: Duration = Duration::from_secs(3600);

pub struct Gate {
    pub entered: AtomicUsize,
    pub permits: Semaphore,
    pub at: usize,
}
impl Gate {
    pub fn new(at: usize) -> Arc<Self> {
        Arc::new(Self {
            entered: AtomicUsize::new(0),
            permits: Semaphore::new(0),
            at,
        })
    }
    pub async fn enter(&self, ctx: &TaskContext) -> Result<()> {
        let ordinal = self.entered.fetch_add(1, Ordering::SeqCst) + 1;
        if self.at == 0 || ordinal == self.at {
            loop {
                ctx.checkpoint()?;
                tokio::select! {
                    permit = self.permits.acquire() => { permit.unwrap().forget(); break; }
                    _ = tokio::time::sleep(Duration::from_millis(5)) => {}
                }
            }
        }
        ctx.checkpoint()
    }
    pub async fn wait(&self, count: usize) {
        tokio::time::timeout(WATCHDOG, async {
            while self.entered.load(Ordering::SeqCst) < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("operation failed to enter gate");
    }
}

#[derive(Clone)]
pub struct TestProvider {
    pub gate: Arc<Gate>,
    pub fail: bool,
    pub incompatible: bool,
    pub cleaned: Arc<AtomicUsize>,
}
impl TestProvider {
    pub fn new(gate: Arc<Gate>) -> Self {
        Self {
            gate,
            fail: false,
            incompatible: false,
            cleaned: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl Provider for TestProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "acceptance".into(),
            description: "Deterministic native acceptance gates".into(),
            dependencies: vec!["files".into()],
            configuration_schema: json!({"type":"object","additionalProperties":false}),
        }
    }
    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let gate = self.gate.clone();
        let file = OperationRegistration::new::<FileEntry, String, _, _>(
            "acceptance.file",
            "Reads captured metadata without file content",
            move |input, ctx| {
                let gate = gate.clone();
                async move {
                    gate.enter(&ctx).await?;
                    Ok(ctx.file_metadata(&input.handle)?.name)
                }
            },
        )?;
        let gate = self.gate.clone();
        let hold = OperationRegistration::new::<u32, u32, _, _>(
            "acceptance.hold",
            "Occupies one native worker until release or cancellation",
            move |input, ctx| {
                let gate = gate.clone();
                async move {
                    gate.enter(&ctx).await?;
                    Ok(input)
                }
            },
        )?;
        let echo = OperationRegistration::new::<FileEntry, FileEntry, _, _>(
            "acceptance.echo",
            "Returns a typed native resource for retained-output acceptance",
            |input, ctx| async move {
                ctx.checkpoint()?;
                Ok(ctx.file_metadata(&input.handle)?.entry(input.handle)?)
            },
        )?;
        if self.incompatible {
            Ok(vec![
                file,
                echo,
                OperationRegistration::new::<u32, String, _, _>(
                    "acceptance.hold",
                    "Deliberately incompatible candidate",
                    |input, _| async move { Ok(input.to_string()) },
                )?,
            ])
        } else {
            Ok(vec![file, hold, echo])
        }
    }
    fn prepare(&self, _context: ProviderContext, _config: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.fail {
                Err(Error::new(
                    "acceptance_prepare_failed",
                    "Candidate intentionally failed",
                ))
            } else {
                Ok(())
            }
        })
    }
    fn cleanup(&self) {
        self.cleaned.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn application(gate: Arc<Gate>) -> Application {
    let provider = TestProvider::new(gate);
    let good = provider.clone();
    let mut failed = provider.clone();
    failed.fail = true;
    let mut incompatible = provider.clone();
    incompatible.incompatible = true;
    Application::new()
        .capability(provider, json!({}))
        .replacement("good", move || Arc::new(good.clone()))
        .replacement("failed", move || Arc::new(failed.clone()))
        .replacement("incompatible", move || Arc::new(incompatible.clone()))
}

#[derive(Default)]
pub struct Observations {
    pub views: HashMap<String, Value>,
    pub publications: usize,
    pub max_rows: usize,
}
pub struct Harness {
    // Runtime is dropped before its paths, including on assertion failure.
    pub runtime: DesktopRuntime,
    pub scope: String,
    pub gate: Arc<Gate>,
    pub observed: Arc<Mutex<Observations>>,
    pub directory: tempfile::TempDir,
    pub fixture: PathBuf,
}
impl Harness {
    pub async fn new(
        count: usize,
        gate_at: usize,
        concurrency: usize,
        queue_capacity: usize,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture");
        std::fs::create_dir(&fixture).unwrap();
        for ordinal in 0..count {
            std::fs::write(fixture.join(format!("file-{ordinal:04}.txt")), b"abc").unwrap();
        }
        Self::from_directory(directory, fixture, gate_at, concurrency, queue_capacity).await
    }
    pub async fn supplied(fixture: PathBuf) -> Self {
        let state = fixture
            .parent()
            .expect("fixture needs a parent directory")
            .join("acceptance-state");
        std::fs::create_dir_all(&state).unwrap();
        Self::from_directory(
            tempfile::Builder::new()
                .prefix("native-")
                .tempdir_in(state)
                .unwrap(),
            fixture,
            5,
            2,
            4,
        )
        .await
    }
    async fn from_directory(
        directory: tempfile::TempDir,
        fixture: PathBuf,
        gate_at: usize,
        concurrency: usize,
        queue_capacity: usize,
    ) -> Self {
        let gate = Gate::new(gate_at);
        let options = DesktopOptions {
            data_dir: Some(directory.path().join("data")),
            cache_dir: Some(directory.path().join("cache")),
            concurrency,
            queue_capacity,
            ..Default::default()
        };
        let runtime = DesktopRuntime::new(application(gate.clone()), options)
            .await
            .unwrap();
        let connected = runtime
            .dispatch("acceptance-window", json!({"action":"root.connect"}), None)
            .await
            .unwrap();
        Self {
            runtime,
            scope: connected["scope"].as_str().unwrap().into(),
            gate,
            observed: Arc::new(Mutex::new(Observations::default())),
            directory,
            fixture,
        }
    }
    pub fn sink(&self) -> UpdateSink {
        let observed = self.observed.clone();
        Arc::new(move |value| {
            if let Some(view) = value["view"].as_str() {
                let mut seen = observed.lock().unwrap();
                seen.publications += 1;
                seen.max_rows = seen
                    .max_rows
                    .max(value["items"].as_array().map_or(0, Vec::len));
                if seen
                    .views
                    .get(view)
                    .is_none_or(|old| old["revision"].as_u64() <= value["revision"].as_u64())
                {
                    seen.views.insert(view.into(), value);
                }
            }
        })
    }
    pub async fn request(&self, mut request: Value) -> Result<Value> {
        request["scope"] = json!(self.scope);
        self.runtime
            .dispatch("acceptance-window", request, Some(self.sink()))
            .await
    }
    pub async fn ack(&self, snapshot: &Value) {
        self.request(
            json!({"action":"view.ack","view":snapshot["view"],"revision":snapshot["revision"]}),
        )
        .await
        .unwrap();
    }
    pub async fn open(&self, limit: u32) -> (String, Value) {
        let folder = self
            .runtime
            .open_folder(&self.scope, self.fixture.clone())
            .unwrap();
        let view = self.request(json!({"action":"folder.query","folder":folder["folder"],"limit":limit,"options":{"recursive":true}})).await.unwrap();
        (
            folder["folder"].as_str().unwrap().into(),
            view["snapshot"].clone(),
        )
    }
    pub async fn ready(&self, mut snapshot: Value) -> Value {
        let deadline = Instant::now() + WATCHDOG;
        loop {
            assert!(
                Instant::now() < deadline,
                "folder scan stopped responding: {snapshot}"
            );
            if snapshot["state"] == "ready" {
                return snapshot;
            }
            assert_ne!(
                snapshot["state"], "failed",
                "folder scan failed: {snapshot}"
            );
            self.ack(&snapshot).await;
            let latest = self
                .observed
                .lock()
                .unwrap()
                .views
                .get(snapshot["view"].as_str().unwrap())
                .cloned();
            if let Some(latest) = latest {
                if latest["revision"].as_u64() > snapshot["revision"].as_u64() {
                    snapshot = latest;
                    continue;
                }
            }
            match self.request(json!({"action":"view.window","view":snapshot["view"],"offset":snapshot["offset"],"limit":snapshot["limit"],"generation":snapshot["generation"]})).await {
                Ok(next) => snapshot = next,
                Err(error) if error.code == "window_pending" => {}
                Err(error) => panic!("scan observation failed: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    pub async fn window(&self, previous: &Value, offset: u64, limit: u32) -> Value {
        self.ack(previous).await;
        self.request(json!({"action":"view.window","view":previous["view"],"offset":offset,"limit":limit,"generation":previous["generation"]})).await.unwrap()
    }
    pub async fn terminal(&self, task: &Value) -> TaskSnapshot {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                let value = self
                    .request(json!({"action":"task.snapshot","task":task["id"]}))
                    .await
                    .unwrap();
                let snapshot: TaskSnapshot = serde_json::from_value(value).unwrap();
                if snapshot.state.is_terminal() {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("native task failed to finalize")
    }
    pub async fn inspect(&self, provider: &str) -> Value {
        self.request(json!({"action":"runtime.inspect"}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == provider)
            .unwrap()
            .clone()
    }
    pub async fn released(&self) {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                if self.inspect("files").await["resources"] == "0"
                    && std::fs::read_dir(self.directory.path().join("cache"))
                        .unwrap()
                        .next()
                        .is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("scoped resources or SQLite stores leaked");
    }
}

pub fn rows(snapshot: &Value) -> &Vec<Value> {
    snapshot["items"].as_array().unwrap()
}
pub fn error(result: Result<Value>, code: &str) -> Error {
    let error = result.expect_err("request unexpectedly succeeded");
    assert_eq!(error.code, code, "{error}");
    error
}
