//! Desktop ownership, cooperative task execution, and lossless JSON contracts.
//! Resource admission and task cancellation are separate: admitted leases retain
//! their capabilities, while task checkpoints enforce disposal and cancellation.
#![deny(missing_docs)]
mod compatibility;
/// Restricted opt-in numerical kernels with checked CPU and WGSL semantics.
#[cfg(feature = "experimental-compute")]
pub mod compute;
mod contract;
mod error;
mod latest;
mod projection;
mod resources;
mod tasks;
mod wire;

pub use compatibility::{
    BRIDGE_PROTOCOL_VERSION, CONTRACT_MANIFEST_VERSION, SDK_COMPATIBILITY_REVISION,
};
pub use contract::{
    Contract, PreparedValidator, TypeDefinition, WireSafe, type_definition,
    validate_contract_schema, validate_json,
};
pub use error::{Error, Result};
pub use latest::{LatestJobs, LatestSubmission};
pub use projection::WireProjection;
pub use resources::{
    ChunkReader, FolderHandle, NativeFolder, NativeLease, NativeResourceLease, Resource,
    ResourceCleanup, ResourceLease, ResourcePort, ResourceRegistry,
};
pub use tasks::{
    ApplicationPaths, BatchSummary, CancellationProbe, CancellationToken, MAX_TASK_UPDATE_BYTES,
    Outcome, Progress, SnapshotListener, Subscription, TaskContext, TaskControl, TaskExecution,
    TaskManager, TaskServices, TaskSnapshot, TaskState, TaskUpdate, Work, WorkClass, WorkJob,
    WorkLimits, WorkService, WorkStats,
};
pub use wire::{DecimalI64, DecimalU64, FileEntry, FileMetadata, Handle};
pub use {schemars, serde, serde_json, ts_rs};

/// A heap-allocated Send future borrowing its inputs for at most `'a`.
/// Desktop resource ports and executors can move pending work between threads.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
