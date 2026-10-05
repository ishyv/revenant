//! Each task owns its lease cache. Callbacks run outside the data lock; subscriptions deliver the current snapshot immediately.
use super::*;
pub(super) struct TaskData {
    pub(super) snapshot: TaskSnapshot,
    pub(super) started: bool,
    pub(super) leases: HashMap<String, ResourceLease>,
    pub(super) listeners: BTreeMap<u64, Arc<dyn SnapshotListener>>,
    pub(super) next_listener: u64,
    pub(super) cancellation_probe: Option<Arc<dyn CancellationProbe>>,
}
pub(super) struct Task {
    pub(super) id: String,
    pub(super) scope: String,
    pub(super) data: Mutex<TaskData>,
    pub(super) resources: ResourceRegistry,
    pub(super) parent: Option<BatchParent>,
}
pub(super) struct BatchParent {
    pub(super) context: TaskContext,
    pub(super) index: u32,
    pub(super) total: u32,
}

/// Synchronous snapshot observer shared across desktop threads.
/// Callbacks run outside the task lock and should return promptly; callbacks from
/// concurrent mutations may overlap, and panics propagate to the publishing caller.
pub trait SnapshotListener: Send + Sync + 'static {
    /// Receives an owned point-in-time snapshot, including one immediate delivery on subscribe.
    fn changed(&self, snapshot: TaskSnapshot);
}
impl<F: Fn(TaskSnapshot) + Send + Sync + 'static> SnapshotListener for F {
    fn changed(&self, snapshot: TaskSnapshot) {
        self(snapshot)
    }
}
/// Cloneable access to one task and its captured leases. Clones share lifecycle state.
/// Finalization empties the cache; dropping the last context also releases its leases.
#[derive(Clone)]
pub struct TaskContext {
    pub(super) task: Arc<Task>,
}
/// Compatibility name for TaskContext, exposing the same control and execution API.
pub type TaskControl = TaskContext;
/// Registration guard retaining a listener until dropped. It holds a weak task reference.
/// Dropping stops future lookup of the listener; a callback already cloned for delivery
/// may still run. Keeping this guard alive is required to continue receiving updates.
pub struct Subscription {
    task: Weak<Task>,
    id: u64,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(task) = self.task.upgrade() {
            lock(&task.data).listeners.remove(&self.id);
        }
    }
}

impl TaskContext {
    /// Borrows this task identifier for the lifetime of the context borrow.
    pub fn id(&self) -> &str {
        &self.task.id
    }
    /// Borrows the original task scope identifier; capture requires an exact match.
    pub fn scope(&self) -> &str {
        &self.task.scope
    }
    /// Clones the current snapshot under the task lock; the returned view is independent.
    pub fn snapshot(&self) -> TaskSnapshot {
        lock(&self.task.data).snapshot.clone()
    }
    pub(super) fn change(
        &self,
        f: impl FnOnce(&mut TaskData) -> Result<()>,
    ) -> Result<TaskSnapshot> {
        let (snapshot, listeners) = {
            let mut data = lock(&self.task.data);
            f(&mut data)?;
            (
                data.snapshot.clone(),
                data.listeners.values().cloned().collect::<Vec<_>>(),
            )
        };
        for listener in listeners {
            listener.changed(snapshot.clone());
        }
        Ok(snapshot)
    }
    /// Registers a listener and synchronously delivers the current snapshot before returning.
    /// Retain the returned guard to remain subscribed. Concurrent publication may
    /// interleave with initial delivery; listener callbacks run outside the data lock.
    pub fn subscribe(&self, listener: Arc<dyn SnapshotListener>) -> Subscription {
        let (id, snapshot) = {
            let mut data = lock(&self.task.data);
            let id = data.next_listener;
            data.next_listener += 1;
            data.listeners.insert(id, listener.clone());
            (id, data.snapshot.clone())
        };
        listener.changed(snapshot);
        Subscription {
            task: Arc::downgrade(&self.task),
            id,
        }
    }
    pub(super) fn mutable(data: &TaskData) -> Result<()> {
        if data.snapshot.state.is_terminal() {
            return Err(Error::new(
                "task_terminal",
                "Terminal task snapshots are immutable",
            ));
        }
        Ok(())
    }
    /// Publishes work in the units established by the operation. Requires a started,
    /// nonterminal task. Returns `invalid_progress` for a decrease or completion
    /// beyond total. Child updates forward item position and message to the parent,
    /// preserving the child's own work units; parent publication can also fail.
    pub fn progress(&self, progress: Progress) -> Result<()> {
        let message = progress.message.clone();
        self.change(|data| {
            Self::mutable(data)?;
            if !data.started {
                return Err(Error::new(
                    "task_not_running",
                    "Progress requires an active executor",
                ));
            }
            if progress.completed < data.snapshot.progress.completed
                || progress.total.is_some_and(|t| progress.completed > t)
            {
                return Err(Error::new(
                    "invalid_progress",
                    "Progress must be monotonic and bounded by total",
                ));
            }
            data.snapshot.progress = progress;
            Ok(())
        })?;
        if let Some(parent) = &self.task.parent {
            parent.context.progress(Progress {
                completed: u64::from(parent.index).into(),
                total: Some(u64::from(parent.total).into()),
                message,
            })?;
        }
        Ok(())
    }
    /// Retains a legacy item result and sorts results by index. Requires an active
    /// executor. Rejects duplicate indices and results with both or neither output
    /// and error. Does not update summary; streaming hosts should record counters
    /// with record_batch_outcome to avoid retaining item payloads.
    pub fn record_outcome(&self, outcome: Outcome) -> Result<()> {
        self.change(|data| {
            Self::mutable(data)?;
            if !data.started {
                return Err(Error::new(
                    "task_not_running",
                    "Outcomes require an active executor",
                ));
            }
            if outcome.output.is_some() == outcome.error.is_some() {
                return Err(Error::new(
                    "invalid_outcome",
                    "An outcome has exactly one of output or error",
                ));
            }
            if data
                .snapshot
                .outcomes
                .iter()
                .any(|old| old.index == outcome.index)
            {
                return Err(Error::new(
                    "duplicate_outcome",
                    "Batch index has already completed",
                ));
            }
            data.snapshot.outcomes.push(outcome);
            data.snapshot.outcomes.sort_by_key(|o| o.index);
            Ok(())
        })
        .map(|_| ())
    }
    /// Records one completed item without retaining its input, output, or error.
    /// Requires an active executor. Returns `batch_exhausted` without changing any
    /// counter if a u64 counter would overflow. Call once per item; indices are not tracked.
    pub fn record_batch_outcome(&self, success: bool) -> Result<()> {
        self.change(|data| {
            Self::mutable(data)?;
            if !data.started {
                return Err(Error::new(
                    "task_not_running",
                    "Batch accounting requires an active executor",
                ));
            }
            let summary = &mut data.snapshot.summary;
            let completed = summary.completed.get().checked_add(1).ok_or_else(|| {
                Error::new("batch_exhausted", "Batch completed counter is exhausted")
            })?;
            let counter = if success {
                &mut summary.succeeded
            } else {
                &mut summary.failed
            };
            let count = counter.get().checked_add(1).ok_or_else(|| {
                Error::new("batch_exhausted", "Batch outcome counter is exhausted")
            })?;
            *counter = count.into();
            summary.completed = completed.into();
            Ok(())
        })
        .map(|_| ())
    }
    /// Captures a lease already authorized by a trusted host, including after owner release.
    /// The original handle must be a file in this task's scope. Returns `scope_mismatch`,
    /// `wrong_kind`, or `stale_handle` for incompatible capture; cancelled or terminal
    /// tasks reject capture. The cached reference is released at task finalization.
    pub fn capture(&self, lease: ResourceLease) -> Result<()> {
        self.checkpoint()?;
        let handle = lease.handle();
        if handle.scope != self.scope() {
            return Err(Error::new(
                "scope_mismatch",
                "Captured file belongs to another scope",
            ));
        }
        if handle.kind != "file" {
            return Err(Error::new("wrong_kind", "Expected a captured file lease"));
        }
        let mut data = lock(&self.task.data);
        Self::mutable(&data)?;
        if let Some(existing) = data.leases.get(&handle.id) {
            if existing.handle() != handle {
                return Err(Error::new(
                    "stale_handle",
                    "Captured handle conflicts with an existing lease",
                ));
            }
            return Ok(());
        }
        data.leases.insert(handle.id.clone(), lease);
        Ok(())
    }
    /// Reuses this task's captured lease before attempting fresh registry admission.
    /// New admission validates every handle field and the task scope. Owner release
    /// does not invalidate a cached lease; cancellation and scope disposal still fail
    /// the checkpoint. Batch children own their caches independently of their parent.
    pub fn lease(&self, handle: &Handle) -> Result<ResourceLease> {
        self.checkpoint()?;
        {
            let data = lock(&self.task.data);
            Self::mutable(&data)?;
            if let Some(lease) = data.leases.get(&handle.id) {
                if lease.handle() != handle {
                    return Err(Error::new(
                        "stale_handle",
                        "Handle does not match captured lease",
                    ));
                }
                return Ok(lease.clone());
            }
        }
        let lease = self.task.resources.lease(self.scope(), handle)?;
        self.capture(lease.clone())?;
        Ok(lease)
    }
    /// Clones metadata from a cached or newly admitted lease; propagates lease errors.
    pub fn file_metadata(&self, handle: &Handle) -> Result<FileMetadata> {
        Ok(self.lease(handle)?.metadata().clone())
    }
    /// Checks an admitted resource without reading bytes, then checks cancellation.
    /// Native ports detect open failures and changed metadata even for empty files.
    pub async fn validate_resource(&self, handle: &Handle) -> Result<()> {
        self.lease(handle)?.validate().await?;
        self.checkpoint()
    }
    /// Reads at a byte offset with a requested byte length between 1 and 4 MiB.
    /// Leases are cached before host access. Propagates admission or host read errors
    /// and checks cancellation again after the read; cancellation discards returned bytes.
    pub async fn read(&self, handle: &Handle, offset: u64, length: u32) -> Result<Vec<u8>> {
        let lease = self.lease(handle)?;
        let bytes = lease.read(offset, length).await?;
        self.checkpoint()?;
        Ok(bytes)
    }
    /// A local operation context whose byte progress never overwrites the parent's
    /// batch-item counters. Cancellation propagates from the parent; leases belong only to this child
    /// and are released on finalization or when the last child context is dropped.
    /// Index is zero-based and total counts items. Returns `invalid_batch_index`
    /// when index is not below total, or propagates cancellation/checkpoint errors.
    /// The host must begin and finish the child execution separately.
    pub fn batch_child(&self, index: u32, total: u32) -> Result<TaskContext> {
        self.checkpoint()?;
        if index >= total {
            return Err(Error::new(
                "invalid_batch_index",
                "Child index exceeds batch size",
            ));
        }
        let id = format!("{}/{}", self.id(), index);
        let scope = self.scope().to_owned();
        let snapshot = TaskSnapshot {
            id: id.clone(),
            scope: scope.clone(),
            state: TaskState::Queued,
            progress: Progress::default(),
            outcomes: Vec::new(),
            summary: BatchSummary::default(),
            result: None,
            error: None,
            cancel_requested: false,
        };
        Ok(TaskContext {
            task: Arc::new(Task {
                id,
                scope,
                resources: self.task.resources.clone(),
                parent: Some(BatchParent {
                    context: self.clone(),
                    index,
                    total,
                }),
                data: Mutex::new(TaskData {
                    snapshot,
                    started: false,
                    leases: HashMap::new(),
                    listeners: BTreeMap::new(),
                    next_listener: 1,
                    cancellation_probe: None,
                }),
            }),
        })
    }
}
