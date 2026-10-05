//! Application-wide native budgets, separate from transport configuration.

use std::{path::PathBuf, time::Duration};

/// Native execution and storage defaults used by generated desktop bootstrap.
///
/// These limits bound simultaneous work, not the number of files in a folder.
/// Advanced applications can change them without replacing IPC or task handling.
#[derive(Clone, Debug)]
pub struct DesktopOptions {
    /// Stable application identifier used to isolate native data and cache files.
    pub app_id: String,
    /// Explicit settings directory; Tauri supplies its application-data path by default.
    pub data_dir: Option<PathBuf>,
    /// Explicit metadata/result cache directory; Tauri supplies its app-cache path.
    pub cache_dir: Option<PathBuf>,
    /// Maximum simultaneously executing operations or filesystem scans.
    pub concurrency: usize,
    /// Maximum queued native operations before admission reports `queue_full`.
    pub queue_capacity: usize,
    /// Maximum active UI ownership scopes.
    pub max_scopes: usize,
    /// Maximum simultaneously owned native query views.
    pub max_views: usize,
    /// Maximum retained task records; callers dispose completed history to release space.
    pub max_tasks: usize,
    /// Maximum input or result JSON bytes for one custom operation.
    pub max_contract_bytes: usize,
    /// Maximum time to await cooperative cancellation during application shutdown.
    pub shutdown_grace: Duration,
}

impl Default for DesktopOptions {
    fn default() -> Self {
        Self {
            app_id: "dev.revenant.application".into(),
            data_dir: None,
            cache_dir: None,
            concurrency: 4,
            queue_capacity: 32,
            max_scopes: 128,
            max_views: 64,
            max_tasks: 256,
            max_contract_bytes: 1024 * 1024,
            shutdown_grace: Duration::from_secs(10),
        }
    }
}
