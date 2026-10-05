//! Native authority for scopes, folder views, tasks, and application persistence.
//!
//! Transport handlers only admit requests here. Query metadata, execution state,
//! and cleanup remain native even when the webview reloads or drops subscriptions.

mod dispatch;
mod jobs;
mod scopes;
mod views;

use crate::{DesktopOptions, Error, Result};
use revenant_core::{ResourceRegistry, TaskManager};
use revenant_sdk::{Application, Provider, Registry};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Semaphore;

pub(crate) use jobs::JobRecord;
pub(crate) use jobs::SelectionRecord;
pub(crate) use scopes::ScopeRecord;
pub(crate) use views::{FolderRecord, ViewRecord};

/// Receives bounded native snapshots; transport adapters may drop closed channels.
pub type UpdateSink = Arc<dyn Fn(Value) + Send + Sync>;

/// Shared native application runtime, independent of Tauri's webview transport.
///
/// Use the generated bootstrap for ordinary applications. This interface is also
/// available for embedding and exercising native workflows without a webview.
#[derive(Clone)]
pub struct DesktopRuntime {
    pub(crate) inner: Arc<RuntimeInner>,
}

pub(crate) struct RuntimeInner {
    pub options: DesktopOptions,
    pub registry: Registry,
    pub resources: ResourceRegistry,
    pub tasks: TaskManager,
    pub scopes: Mutex<HashMap<String, ScopeRecord>>,
    pub folders: Mutex<HashMap<String, Arc<FolderRecord>>>,
    pub views: Mutex<HashMap<String, Arc<ViewRecord>>>,
    pub jobs: Mutex<HashMap<String, Arc<JobRecord>>>,
    pub selections: Mutex<HashMap<String, Arc<SelectionRecord>>>,
    pub replacements:
        std::collections::BTreeMap<String, Arc<dyn Fn() -> Arc<dyn Provider> + Send + Sync>>,
    pub workers: Arc<Semaphore>,
    pub admission: Arc<Semaphore>,
    pub control_workers: Arc<Semaphore>,
    pub control_admission: Arc<Semaphore>,
    #[cfg(feature = "tauri-host")]
    pub pickers: Arc<Semaphore>,
    pub closing: AtomicBool,
    pub settings: crate::settings::SettingsStore,
    pub previews: crate::previews::PreviewStore,
    pub cache_dir: PathBuf,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub(crate) fn id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}
pub(crate) fn io_error(error: impl std::fmt::Display) -> Error {
    Error::new("native_io", error.to_string())
}

impl DesktopRuntime {
    /// Validates contracts and prepares native providers before serving UI requests.
    ///
    /// Data/cache paths must be supplied by the platform adapter or embedder.
    /// No folder contents or file bytes are touched during initialization.
    pub async fn new(application: Application, options: DesktopOptions) -> Result<Self> {
        application.manifest()?;
        if options.concurrency == 0 || options.max_scopes == 0 || options.max_views == 0 {
            return Err(Error::new(
                "invalid_options",
                "Native concurrency and ownership limits must be positive",
            ));
        }
        let data_dir = options.data_dir.clone().ok_or_else(|| {
            Error::new(
                "data_directory",
                "Application data directory was not supplied",
            )
        })?;
        let cache_dir = options.cache_dir.clone().ok_or_else(|| {
            Error::new(
                "cache_directory",
                "Application cache directory was not supplied",
            )
        })?;
        std::fs::create_dir_all(&data_dir).map_err(io_error)?;
        std::fs::create_dir_all(&cache_dir).map_err(io_error)?;
        let settings = crate::settings::SettingsStore::new(&data_dir)?;
        let previews = crate::previews::PreviewStore::new();
        let resources = ResourceRegistry::new();
        let registry = Registry::with_resources(resources.clone());
        let (operations, capabilities, replacements) = application.into_parts();
        registry
            .install(
                Arc::new(StandardProvider { kind: "files" }),
                serde_json::json!({}),
            )
            .await?;
        registry
            .install(
                Arc::new(StandardProvider { kind: "media" }),
                serde_json::json!({}),
            )
            .await?;
        registry
            .install(
                Arc::new(StandardProvider { kind: "settings" }),
                serde_json::json!({}),
            )
            .await?;
        for operation in operations {
            registry.register(operation()?)?;
        }
        for capability in capabilities {
            registry
                .install(capability.provider, capability.config)
                .await?;
        }
        let tasks = TaskManager::new(resources.clone());
        let workers = Arc::new(Semaphore::new(options.concurrency));
        let admission = Arc::new(Semaphore::new(options.concurrency + options.queue_capacity));
        Ok(Self {
            inner: Arc::new(RuntimeInner {
                options,
                registry,
                resources,
                tasks,
                scopes: Mutex::new(HashMap::new()),
                folders: Mutex::new(HashMap::new()),
                views: Mutex::new(HashMap::new()),
                jobs: Mutex::new(HashMap::new()),
                selections: Mutex::new(HashMap::new()),
                replacements,
                workers,
                admission,
                closing: AtomicBool::new(false),
                settings,
                previews,
                cache_dir,
                control_workers: Arc::new(Semaphore::new(2)),
                control_admission: Arc::new(Semaphore::new(34)),
                #[cfg(feature = "tauri-host")]
                pickers: Arc::new(Semaphore::new(1)),
            }),
        })
    }

    pub(crate) fn accepting(&self) -> Result<()> {
        if self.inner.closing.load(Ordering::Acquire) {
            return Err(Error::new(
                "app_closing",
                "The application is shutting down",
            ));
        }
        Ok(())
    }

    /// Reads one bounded preview response from an opaque native preview token.
    /// Tokens are revoked by preview or scope disposal. A single HTTP byte range
    /// is accepted; invalid ranges retain total-size details for a 416 response.
    pub async fn read_preview(
        &self,
        token: String,
        range: Option<String>,
    ) -> Result<crate::PreviewResponse> {
        self.control(move |runtime| {
            tokio::runtime::Handle::current()
                .block_on(runtime.inner.previews.read(&token, range.as_deref()))
        })
        .await
    }

    /// Runs bounded blocking control work independently of traversal and application jobs.
    /// A full control queue fails with a retryable error rather than allocating more workers.
    pub(crate) async fn control<T, F>(&self, function: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Self) -> Result<T> + Send + 'static,
    {
        let admitted = self
            .inner
            .control_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                Error::new("control_queue_full", "Native control queue is full").retryable(true)
            })?;
        let worker = self
            .inner
            .control_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new("app_closing", "Native control executor is closed"))?;
        let runtime = self.clone();
        let executor = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let _admitted = admitted;
            let _worker = worker;
            let _entered = executor.enter();
            function(runtime)
        })
        .await
        .map_err(|e| Error::new("native_executor", e.to_string()))?
    }

    /// Stops admission, requests cancellation, and awaits owned work up to its grace limit.
    ///
    /// Cleanup and settings flush share the same deadline as task cancellation.
    /// Cleanup, persistence, and timeout failures are reported; unfinished tasks
    /// become interrupted. The process adapter owns final termination because
    /// Rust cannot safely kill an uncooperative worker.
    pub async fn shutdown(&self) -> Result<()> {
        if self.inner.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + self.inner.options.shutdown_grace;
        let jobs: Vec<_> = lock(&self.inner.jobs).values().cloned().collect();
        for job in &jobs {
            let _ = job.context.request_cancel();
        }
        let scopes: Vec<_> = lock(&self.inner.scopes).keys().cloned().collect();
        let cleanup = tokio::time::timeout_at(
            deadline,
            self.control(move |runtime| {
                for scope in scopes {
                    let _ = runtime.dispose_scope(&scope);
                }
                runtime.inner.settings.flush()
            }),
        )
        .await;
        while self.inner.admission.available_permits()
            < self.inner.options.concurrency + self.inner.options.queue_capacity
        {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // Retain the jobs captured before teardown: completed/disposed history
        // may already have left the map. Late executors cannot revise this state.
        for job in jobs {
            if !job.context.snapshot().state.is_terminal() {
                let _ = job.context.interrupt();
            }
        }
        cleanup.map_err(|_| {
            Error::new(
                "shutdown_timeout",
                "Native cleanup exceeded the shutdown grace period",
            )
        })?
    }
}

struct StandardProvider {
    kind: &'static str,
}
impl Provider for StandardProvider {
    fn descriptor(&self) -> revenant_sdk::ProviderDescriptor {
        revenant_sdk::ProviderDescriptor {
            id: self.kind.into(),
            description: format!(
                "Native {} capability owned by the desktop application",
                self.kind
            ),
            dependencies: if self.kind == "media" {
                vec!["files".into()]
            } else {
                vec![]
            },
            configuration_schema: serde_json::json!({"type":"object","additionalProperties":false}),
        }
    }
    fn operations(&self) -> Result<Vec<revenant_sdk::OperationRegistration>> {
        Ok(match self.kind {
            "files" => vec![revenant_sdk::builtins::checksum_operation()?],
            "media" => vec![revenant_sdk::builtins::read_metadata_operation()?],
            _ => vec![],
        })
    }
}
