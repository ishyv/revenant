//! Task identifiers are unique among retained entries. Removing a terminal entry does not invalidate existing context clones.
use super::*;
/// Shared collection of task contexts bound to one resource registry.
/// Completed tasks remain retained until explicitly removed by the host.
#[derive(Clone)]
pub struct TaskManager {
    resources: ResourceRegistry,
    tasks: Arc<Mutex<HashMap<String, TaskContext>>>,
}
impl TaskManager {
    /// Creates an empty manager sharing the supplied registry and its scope lifetimes.
    pub fn new(resources: ResourceRegistry) -> Self {
        Self {
            resources,
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    /// Borrows the registry used for admission by all managed tasks.
    pub fn resources(&self) -> &ResourceRegistry {
        &self.resources
    }
    /// Creates a queued task with zero progress and batch counters. Returns
    /// `scope_disposed` for an inactive scope and `duplicate_task` for an empty
    /// or retained task identifier. Task IDs are manager-wide, not scope-local.
    pub fn create(&self, scope: &str, task_id: &str) -> Result<TaskContext> {
        if !self.resources.is_scope_active(scope) {
            return Err(Error::new(
                "scope_disposed",
                "Cannot create a task in an inactive scope",
            ));
        }
        let mut tasks = lock(&self.tasks);
        if task_id.is_empty() || tasks.contains_key(task_id) {
            return Err(Error::new(
                "duplicate_task",
                "Task id is empty or already exists",
            ));
        }
        let snapshot = TaskSnapshot {
            id: task_id.into(),
            scope: scope.into(),
            state: TaskState::Queued,
            progress: Progress::default(),
            outcomes: Vec::new(),
            summary: BatchSummary::default(),
            result: None,
            error: None,
            cancel_requested: false,
        };
        let context = TaskContext {
            task: Arc::new(Task {
                id: task_id.into(),
                scope: scope.into(),
                resources: self.resources.clone(),
                parent: None,
                data: Mutex::new(TaskData {
                    snapshot,
                    started: false,
                    leases: HashMap::new(),
                    listeners: BTreeMap::new(),
                    next_listener: 1,
                    cancellation_probe: None,
                }),
            }),
        };
        tasks.insert(task_id.into(), context.clone());
        Ok(context)
    }
    /// Clones a retained context after verifying its owning scope. Returns
    /// `task_missing` or `scope_mismatch`; terminal contexts remain inspectable.
    pub fn context(&self, scope: &str, task_id: &str) -> Result<TaskContext> {
        let tasks = lock(&self.tasks);
        let context = tasks
            .get(task_id)
            .ok_or_else(|| Error::new("task_missing", "Unknown task"))?;
        if context.scope() != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Task belongs to another scope",
            ));
        }
        Ok(context.clone())
    }
    /// Returns an owned current snapshot; propagates context lookup and scope errors.
    pub fn snapshot(&self, scope: &str, task_id: &str) -> Result<TaskSnapshot> {
        Ok(self.context(scope, task_id)?.snapshot())
    }
    /// Requests cancellation of a retained task; propagates lookup and scope errors.
    /// Returns the current snapshot immediately, without waiting for the executor.
    pub fn cancel(&self, scope: &str, task_id: &str) -> Result<TaskSnapshot> {
        self.context(scope, task_id)?.request_cancel()
    }
    /// Clones snapshots belonging exactly to the given scope in unspecified order.
    /// Includes terminal tasks; does not traverse descendant scopes.
    pub fn list(&self, scope: &str) -> Vec<TaskSnapshot> {
        let contexts: Vec<_> = lock(&self.tasks)
            .values()
            .filter(|ctx| ctx.scope() == scope)
            .cloned()
            .collect();
        contexts.into_iter().map(|ctx| ctx.snapshot()).collect()
    }
    /// Removes a terminal task from manager lookup. Returns `task_busy` for work
    /// still queued or executing, and propagates lookup or scope errors. Existing
    /// context clones remain alive and task IDs may be used again after removal.
    pub fn remove(&self, scope: &str, task_id: &str) -> Result<()> {
        let ctx = self.context(scope, task_id)?;
        if !ctx.snapshot().state.is_terminal() {
            return Err(Error::new("task_busy", "Running tasks cannot be removed"));
        }
        lock(&self.tasks).remove(task_id);
        Ok(())
    }
    /// Disposes registry ownership for this scope and descendants, then requests
    /// cancellation for every retained task in an inactive scope. Running work
    /// finishes cooperatively; unknown scopes return `scope_missing`.
    pub fn dispose_scope(&self, scope: &str) -> Result<()> {
        self.resources.dispose_scope(scope)?;
        let contexts: Vec<_> = lock(&self.tasks)
            .values()
            .filter(|ctx| !self.resources.is_scope_active(ctx.scope()))
            .cloned()
            .collect();
        for context in contexts {
            context.request_cancel()?;
        }
        Ok(())
    }
}
