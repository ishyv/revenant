//! Shared task ownership and lifecycle. No task callbacks or resource cleanup run under the task data lock.
use crate::{
    DecimalU64, Error, FileMetadata, Handle, ResourceLease, ResourceRegistry, Result, lock,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, Weak},
};
use ts_rs::TS;

mod cancellation;
mod context;
mod execution;
mod manager;
mod models;
pub use cancellation::CancellationProbe;
pub use context::{SnapshotListener, Subscription, TaskContext, TaskControl};
use context::{Task, TaskData};
pub use execution::TaskExecution;
pub use manager::TaskManager;
pub use models::{BatchSummary, Outcome, Progress, TaskSnapshot, TaskState};
