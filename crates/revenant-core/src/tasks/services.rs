//! Immutable host services and bounded native work; bulk values never cross JSON.
use super::*;
use std::path::PathBuf;

/// Actual resolved storage locations. These paths convey information, not authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationPaths {
    /// Host-resolved application identity.
    pub app_id: String,
    /// Actual application data directory, including configured overrides.
    pub data_dir: PathBuf,
    /// Actual cache directory, including configured overrides.
    pub cache_dir: PathBuf,
}

/// Host scheduling class. Hints never increase host limits or operation authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum WorkClass {
    /// Ordinary CPU work.
    #[default]
    Cpu,
    /// Blocking input/output with its own bounded workers.
    Io,
    /// Explicitly declared latency-sensitive read work.
    Interactive,
}

/// Numeric-only executor observations; arbitrary external allocations are unknown.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct WorkStats {
    /// Currently executing CPU stages, including admitted parent operations.
    pub cpu_active: usize,
    /// Currently executing blocking-I/O stages.
    pub io_active: usize,
    /// Queued chunks, excluding callers waiting for bounded admission.
    pub queued: usize,
    /// Highest observed concurrent CPU execution.
    pub cpu_peak: usize,
    /// Highest observed concurrent blocking-I/O execution.
    pub io_peak: usize,
    /// Number of settled chunks.
    pub completed: u64,
    /// Number of chunks cancelled before running.
    pub cancelled: u64,
    /// Aggregate chunk queue wait, in nanoseconds.
    pub queue_wait_ns: u64,
    /// Reserved inline input/result element-capacity bytes in work windows.
    pub scratch_bytes: usize,
    /// Maximum reserved inline input/result capacity bytes across work windows.
    pub scratch_peak: usize,
}

/// Bounded preparation and result limits for typed work helpers.
#[derive(Clone, Copy, Debug)]
pub struct WorkLimits {
    /// Items materialized per stream window, never the whole input iterator.
    pub window_items: usize,
    /// Maximum reserved inline `T`/`Option<R>` element-capacity bytes in one window.
    /// Job/control storage, allocator overhead and application pointees are separate.
    pub scratch_bytes: usize,
    /// Maximum known inline bytes in an ordered result.
    pub result_bytes: usize,
}
impl Default for WorkLimits {
    fn default() -> Self {
        Self {
            window_items: 32,
            scratch_bytes: 8 * 1024 * 1024,
            result_bytes: 64 * 1024 * 1024,
        }
    }
}

/// One owned, cooperative native chunk; hosts settle every submitted chunk before return.
pub type WorkJob = Box<dyn FnOnce() -> Result<()> + Send + 'static>;

/// Host-independent scheduling interface implemented by desktop or an explicit embedder.
pub trait WorkService: Send + Sync + 'static {
    /// Executes a bounded window, joining all started work even after failure.
    /// Supported nested calls must not require a permit held by the waiting parent.
    fn execute(
        &self,
        owner: &str,
        class: WorkClass,
        cancellation: CancellationToken,
        jobs: Vec<WorkJob>,
    ) -> Result<()>;
    /// Returns immutable helper bounds.
    fn limits(&self) -> WorkLimits {
        WorkLimits::default()
    }
    /// Samples known execution counters, without application payloads.
    fn stats(&self) -> WorkStats;
    /// Cooperatively waits for an external native completion without retaining
    /// the waiting parent's execution capacity. Callbacks run without host locks.
    fn wait(&self, ready: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        while !ready() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(())
    }
    /// Accounts a known work window until its returned guard is dropped.
    /// Allocations behind application-owned pointers are not observable here.
    fn reserve_scratch(&self, _bytes: usize) -> Result<Box<dyn Send>> {
        Ok(Box::new(()))
    }
}

/// Immutable services inherited by all batch children of an admitted task.
#[derive(Default)]
pub struct TaskServices {
    paths: Option<ApplicationPaths>,
    work: Option<Arc<dyn WorkService>>,
}
impl TaskServices {
    /// Constructs explicitly supplied services; missing services remain unavailable.
    pub fn new(paths: Option<ApplicationPaths>, work: Option<Arc<dyn WorkService>>) -> Self {
        Self { paths, work }
    }
}

/// Cheap cloneable cancellation view tied to its task, parent and scope lifetime.
#[derive(Clone)]
pub struct CancellationToken(TaskContext);
impl CancellationToken {
    /// Checks cancellation/disposal, including external host probes and parents.
    /// A terminal successful task is no longer eligible for new work.
    pub fn is_cancelled(&self) -> bool {
        self.0.checkpoint().is_err()
    }
    /// Returns the existing classified cooperative checkpoint error.
    pub fn checkpoint(&self) -> Result<()> {
        self.0.checkpoint()
    }
}

/// Native work helpers capturing the task's admitted leases until all work settles.
#[derive(Clone)]
pub struct Work {
    context: TaskContext,
    service: Arc<dyn WorkService>,
}
impl Work {
    /// Waits for a native completion while lending the parent's host capacity.
    pub fn wait(&self, ready: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        self.service.wait(ready)
    }
    /// Samples host execution counters.
    pub fn stats(&self) -> WorkStats {
        self.service.stats()
    }

    /// Maps a finite iterator in input order, failing on the first failed window.
    /// Known result bytes are bounded; allocations inside arbitrary T/R are unknown.
    pub fn map_ordered<T, R, I, F>(&self, class: WorkClass, input: I, function: F) -> Result<Vec<R>>
    where
        T: Send + 'static,
        R: Send + 'static,
        I: IntoIterator<Item = T>,
        F: Fn(T, CancellationToken) -> Result<R> + Send + Sync + 'static,
    {
        let limit = self.service.limits().result_bytes;
        let mut result = Vec::new();
        self.stream_ordered(class, input, function, |item| {
            let count = result
                .len()
                .checked_add(1)
                .ok_or_else(|| Error::new("work_result_limit", "Result length exhausted"))?;
            if count > limit / std::mem::size_of::<R>().max(1) {
                return Err(Error::new(
                    "work_result_limit",
                    "Use streaming work for results exceeding the known byte bound",
                ));
            }
            if result.len() == result.capacity() {
                let maximum = limit / std::mem::size_of::<R>().max(1);
                let capacity = result.capacity().saturating_mul(2).max(32).min(maximum);
                result
                    .try_reserve_exact(capacity - result.len())
                    .map_err(|_| Error::new("work_result_limit", "Result allocation failed"))?;
            }
            result.push(item);
            Ok(())
        })?;
        Ok(result)
    }

    /// Processes a lazy iterator in bounded windows and consumes results in input order.
    /// A failed window publishes none of its results; already consumed earlier windows
    /// are not rolled back. Cancellation cannot undo external effects performed by F.
    /// Scratch admission precedes materialization and spans consumer settlement. It
    /// accounts planned input `T` and result `Option<R>` vector capacities, with at least
    /// one byte per item for zero-sized values. Job/control allocations, temporary
    /// captures of T, allocator overhead and allocations behind pointers are separate.
    pub fn stream_ordered<T, R, I, F, C>(
        &self,
        class: WorkClass,
        input: I,
        function: F,
        mut consume: C,
    ) -> Result<()>
    where
        T: Send + 'static,
        R: Send + 'static,
        I: IntoIterator<Item = T>,
        F: Fn(T, CancellationToken) -> Result<R> + Send + Sync + 'static,
        C: FnMut(R) -> Result<()>,
    {
        if class == WorkClass::Interactive && self.context.work_class() != WorkClass::Interactive {
            return Err(Error::new(
                "work_class_forbidden",
                "Interactive work requires explicit read-operation admission",
            ));
        }
        let bounds = self.service.limits();
        if bounds.window_items == 0 {
            return Err(Error::new(
                "invalid_work_limits",
                "Work window must be positive",
            ));
        }
        let item_bytes = std::mem::size_of::<T>()
            .saturating_add(std::mem::size_of::<Option<R>>())
            .max(1);
        let window = bounds.window_items.min(bounds.scratch_bytes / item_bytes);
        let function = Arc::new(function);
        let mut input = input.into_iter();
        loop {
            self.context.checkpoint()?;
            // Admit the planned item and result capacities before the iterator
            // can allocate or advance. Exact small/empty iterators need no full
            // window reservation; unknown iterators require the bounded window.
            let remaining = input.size_hint().1;
            if remaining == Some(0) {
                return Ok(());
            }
            if window == 0 {
                return Err(Error::new(
                    "work_scratch_limit",
                    "One work item exceeds the known scratch bound",
                ));
            }
            let capacity = remaining.map_or(window, |remaining| window.min(remaining));
            let _scratch = self.service.reserve_scratch(capacity * item_bytes)?;
            let mut items = Vec::new();
            items
                .try_reserve_exact(capacity)
                .map_err(|_| Error::new("work_scratch_limit", "Work item allocation failed"))?;
            let mut values = Vec::new();
            values
                .try_reserve_exact(capacity)
                .map_err(|_| Error::new("work_scratch_limit", "Work result allocation failed"))?;
            for _ in 0..capacity {
                self.context.checkpoint()?;
                let Some(item) = input.next() else {
                    break;
                };
                items.push(item);
            }
            if items.is_empty() {
                return Ok(());
            }
            values.resize_with(items.len(), || None);
            let results = Arc::new(Mutex::new(values));
            let mut jobs = Vec::new();
            jobs.try_reserve_exact(items.len())
                .map_err(|_| Error::new("work_scratch_limit", "Work job allocation failed"))?;
            for (index, item) in items.into_iter().enumerate() {
                let token = self.context.cancellation();
                let function = function.clone();
                let results = results.clone();
                jobs.push(Box::new(move || {
                    token.checkpoint()?;
                    let result = function(item, token.clone())?;
                    token.checkpoint()?;
                    lock(&results)[index] = Some(result);
                    Ok(())
                }) as WorkJob);
            }
            self.service.execute(
                self.context.scope(),
                class,
                self.context.cancellation(),
                jobs,
            )?;
            self.context.checkpoint()?;
            // Move values out before invoking application consumers; never run a
            // callback under a framework data lock.
            let values = std::mem::take(&mut *lock(&results));
            for value in values {
                self.context.checkpoint()?;
                consume(value.ok_or_else(|| {
                    Error::new("work_incomplete", "Host did not settle every work item")
                })?)?;
            }
        }
    }

    /// Ordered reduction through bounded streaming, without retaining all outputs.
    pub fn map_reduce<T, R, A, I, F, C>(
        &self,
        class: WorkClass,
        input: I,
        function: F,
        mut accumulator: A,
        mut reduce: C,
    ) -> Result<A>
    where
        T: Send + 'static,
        R: Send + 'static,
        I: IntoIterator<Item = T>,
        F: Fn(T, CancellationToken) -> Result<R> + Send + Sync + 'static,
        C: FnMut(&mut A, R) -> Result<()>,
    {
        self.stream_ordered(class, input, function, |value| {
            reduce(&mut accumulator, value)
        })?;
        Ok(accumulator)
    }
}

impl TaskContext {
    pub(crate) fn resource_registry(&self) -> ResourceRegistry {
        self.task.resources.clone()
    }
    /// Waits for native request settlement, lending host capacity when supplied.
    /// An explicit embedder without work services uses cooperative polling.
    /// Cancellation must be acknowledged by `ready` after started work settles;
    /// this wait never releases task leases prematurely on a cancellation signal.
    pub fn wait_until(&self, ready: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        if let Some(service) = &self.task.services.work {
            service.wait(ready)
        } else {
            while !ready() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ok(())
        }
    }
    /// Borrows resolved native storage information without granting path authority.
    pub fn application_paths(&self) -> Result<&ApplicationPaths> {
        self.checkpoint()?;
        self.task.services.paths.as_ref().ok_or_else(|| {
            Error::new(
                "paths_unavailable",
                "Host application paths were not supplied",
            )
        })
    }
    /// Clones a cooperative task/parent/scope cancellation view.
    pub fn cancellation(&self) -> CancellationToken {
        CancellationToken(self.clone())
    }
    /// Captures the bounded host work service and admitted task leases.
    pub fn work(&self) -> Result<Work> {
        self.checkpoint()?;
        Ok(Work {
            context: self.clone(),
            service: self.task.services.work.clone().ok_or_else(|| {
                Error::new("work_unavailable", "Host work service was not supplied")
            })?,
        })
    }
}
