//! Bounded owner-fair cooperative CPU and blocking-I/O execution.
use crate::{Error, Result};
use revenant_core::{CancellationToken, WorkClass, WorkJob, WorkService, WorkStats};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
struct Chunk {
    ticket: Arc<()>,
    window: Arc<()>,
    class: WorkClass,
    cancellation: CancellationToken,
    job: WorkJob,
    queued: Instant,
    result: mpsc::Sender<Result<()>>,
}
#[derive(Default)]
struct Queue {
    owners: VecDeque<String>,
    jobs: HashMap<String, VecDeque<Chunk>>,
    count: usize,
}
struct Shared {
    cpu: Arc<Semaphore>,
    heavy: Arc<Semaphore>,
    io: Arc<Semaphore>,
    reserve: bool,
    queue: Mutex<Queue>,
    changed: Condvar,
    stats: Mutex<WorkStats>,
    capacity: usize,
    closed: Mutex<bool>,
}
pub(crate) struct Permit {
    shared: Arc<Shared>,
    class: WorkClass,
    _slots: Vec<OwnedSemaphorePermit>,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut stats = lock(&self.shared.stats);
        match self.class {
            WorkClass::Io => stats.io_active -= 1,
            _ => stats.cpu_active -= 1,
        }
        drop(stats);
        self._slots.clear();
        self.shared.changed.notify_all();
    }
}
thread_local! { static EXECUTION: RefCell<Vec<(usize, Option<Permit>)>> = const { RefCell::new(Vec::new()) }; }
pub(crate) struct ExecutionGuard;
impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        EXECUTION.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

/// Desktop's bounded work service, also available to explicit native embedders.
/// Waiting parents lend their permits; nested work therefore completes at budget one.
pub struct NativeWorkService {
    shared: Arc<Shared>,
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
}
impl NativeWorkService {
    /// Creates finite CPU/I/O pools and an owner-fair bounded chunk queue.
    pub fn new(cpu_workers: usize, io_workers: usize, queue_capacity: usize) -> Result<Self> {
        if !(1..=256).contains(&cpu_workers) {
            return Err(Error::new(
                "invalid_work_limits",
                "CPU work pool requires 1..=256 workers",
            ));
        }
        Self::with_budget(
            Arc::new(Semaphore::new(cpu_workers)),
            cpu_workers,
            io_workers,
            queue_capacity,
        )
    }
    pub(crate) fn with_budget(
        cpu: Arc<Semaphore>,
        cpu_workers: usize,
        io_workers: usize,
        queue_capacity: usize,
    ) -> Result<Self> {
        if cpu_workers == 0
            || io_workers == 0
            || queue_capacity == 0
            || cpu_workers > 256
            || io_workers > 256
        {
            return Err(Error::new(
                "invalid_work_limits",
                "Work pools require 1..=256 workers and positive queue capacity",
            ));
        }
        let shared = Arc::new(Shared {
            cpu,
            heavy: Arc::new(Semaphore::new(cpu_workers.saturating_sub(1).max(1))),
            io: Arc::new(Semaphore::new(io_workers)),
            reserve: cpu_workers > 1,
            queue: Mutex::new(Queue::default()),
            changed: Condvar::new(),
            stats: Mutex::new(WorkStats::default()),
            capacity: queue_capacity,
            closed: Mutex::new(false),
        });
        let mut threads = Vec::new();
        for _ in 0..cpu_workers + io_workers {
            let state = shared.clone();
            let spawned = thread::Builder::new()
                .name("revenant-work".into())
                .spawn(move || {
                    loop {
                        if !help(&state, None) {
                            let queue = lock(&state.queue);
                            if *lock(&state.closed) && queue.count == 0 {
                                break;
                            }
                            drop(
                                state
                                    .changed
                                    .wait_timeout(queue, Duration::from_millis(2))
                                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                            );
                        }
                    }
                });
            match spawned {
                Ok(handle) => threads.push(handle),
                Err(error) => {
                    *lock(&shared.closed) = true;
                    shared.changed.notify_all();
                    for handle in threads {
                        let _ = handle.join();
                    }
                    return Err(Error::new("work_threads", error.to_string()));
                }
            }
        }
        Ok(Self {
            shared,
            threads: Mutex::new(threads),
        })
    }
    pub(crate) async fn acquire(&self, class: WorkClass) -> Result<Permit> {
        let slots = match class {
            WorkClass::Io => vec![
                self.shared
                    .io
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| Error::new("app_closing", "Work pool closed"))?,
            ],
            WorkClass::Cpu if self.shared.reserve => {
                let heavy = self
                    .shared
                    .heavy
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| Error::new("app_closing", "Work pool closed"))?;
                vec![
                    heavy,
                    self.shared
                        .cpu
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| Error::new("app_closing", "Work pool closed"))?,
                ]
            }
            _ => vec![
                self.shared
                    .cpu
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| Error::new("app_closing", "Work pool closed"))?,
            ],
        };
        Ok(count_permit(&self.shared, class, slots))
    }
    pub(crate) fn enter(&self, permit: Permit) -> ExecutionGuard {
        enter(&self.shared, permit)
    }
    fn lending<T>(&self, action: impl FnOnce() -> T) -> T {
        let identity = Arc::as_ptr(&self.shared) as usize;
        let suspended = EXECUTION.with(|frames| {
            frames
                .borrow_mut()
                .last_mut()
                .filter(|(id, _)| *id == identity)
                .and_then(|(_, permit)| permit.take())
        });
        let resume_class = suspended.as_ref().map(|permit| permit.class);
        drop(suspended);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
        if let Some(class) = resume_class {
            loop {
                if let Some(permit) = try_permit(&self.shared, class) {
                    EXECUTION.with(|frames| {
                        frames.borrow_mut().last_mut().expect("suspended parent").1 = Some(permit)
                    });
                    break;
                }
                wait_for_change(&self.shared);
            }
        }
        match outcome {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}
fn enter(shared: &Arc<Shared>, permit: Permit) -> ExecutionGuard {
    EXECUTION.with(|frames| {
        frames
            .borrow_mut()
            .push((Arc::as_ptr(shared) as usize, Some(permit)))
    });
    ExecutionGuard
}
fn count_permit(
    shared: &Arc<Shared>,
    class: WorkClass,
    slots: Vec<OwnedSemaphorePermit>,
) -> Permit {
    let mut stats = lock(&shared.stats);
    match class {
        WorkClass::Io => {
            stats.io_active += 1;
            stats.io_peak = stats.io_peak.max(stats.io_active);
        }
        _ => {
            stats.cpu_active += 1;
            stats.cpu_peak = stats.cpu_peak.max(stats.cpu_active);
        }
    }
    drop(stats);
    Permit {
        shared: shared.clone(),
        class,
        _slots: slots,
    }
}
fn try_permit(shared: &Arc<Shared>, class: WorkClass) -> Option<Permit> {
    let slots = match class {
        WorkClass::Io => vec![shared.io.clone().try_acquire_owned().ok()?],
        WorkClass::Cpu if shared.reserve => vec![
            shared.heavy.clone().try_acquire_owned().ok()?,
            shared.cpu.clone().try_acquire_owned().ok()?,
        ],
        _ => vec![shared.cpu.clone().try_acquire_owned().ok()?],
    };
    Some(count_permit(shared, class, slots))
}
fn wait_for_change(shared: &Shared) {
    let queue = lock(&shared.queue);
    drop(
        shared
            .changed
            .wait_timeout(queue, Duration::from_millis(2))
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
}
// Callback panics must become settled chunk failures, not unwind past the
// caller's admitted window or retire a finite pool worker.
fn checked_checkpoint(token: &CancellationToken) -> Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| token.checkpoint())).unwrap_or_else(
        |_| {
            Err(Error::new(
                "work_panicked",
                "Native work checkpoint panicked",
            ))
        },
    )
}
fn help(shared: &Arc<Shared>, window: Option<&Arc<()>>) -> bool {
    // Snapshot only identities/tokens. Host cancellation probes may reenter the
    // executor, so neither probes nor application destructors run under its lock.
    let candidates = {
        let queue = lock(&shared.queue);
        let mut candidates = Vec::new();
        for owner in &queue.owners {
            let mut seen = [false; 3];
            for chunk in &queue.jobs[owner] {
                if window.is_some_and(|window| !Arc::ptr_eq(window, &chunk.window)) {
                    continue;
                }
                let class = match chunk.class {
                    WorkClass::Cpu => 0,
                    WorkClass::Io => 1,
                    WorkClass::Interactive => 2,
                };
                if !seen[class] {
                    candidates.push((
                        owner.clone(),
                        chunk.ticket.clone(),
                        chunk.class,
                        chunk.cancellation.clone(),
                    ));
                    seen[class] = true;
                }
            }
        }
        candidates
    };
    let mut selected = None;
    for (owner, ticket, class, cancellation) in candidates {
        let cancelled = checked_checkpoint(&cancellation).err();
        let permit = if cancelled.is_none() {
            let Some(permit) = try_permit(shared, class) else {
                continue;
            };
            Some(permit)
        } else {
            None
        };
        let mut queue = lock(&shared.queue);
        if let Some(jobs) = queue.jobs.get_mut(&owner)
            && let Some(index) = jobs
                .iter()
                .position(|chunk| Arc::ptr_eq(&chunk.ticket, &ticket))
        {
            let chunk = jobs.remove(index).expect("located chunk");
            let empty = jobs.is_empty();
            queue.owners.retain(|value| value != &owner);
            if empty {
                queue.jobs.remove(&owner);
            } else {
                queue.owners.push_back(owner);
            }
            queue.count -= 1;
            selected = Some((chunk, permit, cancelled));
            break;
        }
    }
    let Some((chunk, permit, cancelled)) = selected else {
        return false;
    };
    shared.changed.notify_all();
    run_chunk(shared, chunk, permit, cancelled);
    true
}
fn run_chunk(shared: &Arc<Shared>, chunk: Chunk, permit: Option<Permit>, cancelled: Option<Error>) {
    let queue_wait = chunk.queued.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    let execution = permit.map(|permit| enter(shared, permit));
    let result = cancelled
        .map_or_else(|| checked_checkpoint(&chunk.cancellation), Err)
        .and_then(|()| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(chunk.job))
                .unwrap_or_else(|_| Err(Error::new("work_panicked", "Native work chunk panicked")))
        });
    {
        let mut stats = lock(&shared.stats);
        stats.completed = stats.completed.saturating_add(1);
        stats.queue_wait_ns = stats.queue_wait_ns.saturating_add(queue_wait);
        if result.as_ref().is_err_and(|error| error.is_cancelled()) {
            stats.cancelled = stats.cancelled.saturating_add(1);
        }
    }
    drop(execution);
    let _ = chunk.result.send(result);
}
impl WorkService for NativeWorkService {
    fn wait(&self, ready: &(dyn Fn() -> bool + Sync)) -> Result<()> {
        self.lending(|| {
            while !ready() {
                wait_for_change(&self.shared);
            }
            Ok(())
        })
    }
    fn reserve_scratch(&self, bytes: usize) -> Result<Box<dyn Send>> {
        let mut stats = lock(&self.shared.stats);
        let total = stats
            .scratch_bytes
            .checked_add(bytes)
            .ok_or_else(|| Error::new("work_scratch_limit", "Work scratch counter exhausted"))?;
        if total > 8 * 1024 * 1024 {
            return Err(Error::new(
                "work_scratch_limit",
                "Shared known scratch budget exhausted",
            ));
        }
        stats.scratch_bytes = total;
        stats.scratch_peak = stats.scratch_peak.max(total);
        drop(stats);
        struct Scratch {
            shared: Arc<Shared>,
            bytes: usize,
        }
        impl Drop for Scratch {
            fn drop(&mut self) {
                lock(&self.shared.stats).scratch_bytes -= self.bytes;
            }
        }
        Ok(Box::new(Scratch {
            shared: self.shared.clone(),
            bytes,
        }))
    }
    fn execute(
        &self,
        owner: &str,
        class: WorkClass,
        cancellation: CancellationToken,
        jobs: Vec<WorkJob>,
    ) -> Result<()> {
        self.lending(|| {
            let (sender, receiver) = mpsc::channel();
            let window = Arc::new(());
            let mut admitted = 0;
            let mut failure = None;
            for job in jobs {
                let mut chunk = Some(Chunk {
                    ticket: Arc::new(()),
                    window: window.clone(),
                    class,
                    cancellation: cancellation.clone(),
                    job,
                    queued: Instant::now(),
                    result: sender.clone(),
                });
                loop {
                    if let Err(error) = checked_checkpoint(&cancellation) {
                        failure = Some(error);
                        break;
                    }
                    let mut queue = lock(&self.shared.queue);
                    if *lock(&self.shared.closed) {
                        failure = Some(Error::new("app_closing", "Work service closed"));
                        break;
                    }
                    let limit = if class == WorkClass::Cpu
                        && self.shared.reserve
                        && self.shared.capacity > 1
                    {
                        self.shared.capacity - 1
                    } else {
                        self.shared.capacity
                    };
                    if queue.count < limit {
                        if !queue.jobs.contains_key(owner) {
                            queue.owners.push_back(owner.into());
                        }
                        queue
                            .jobs
                            .entry(owner.into())
                            .or_default()
                            .push_back(chunk.take().expect("single admission"));
                        queue.count += 1;
                        admitted += 1;
                        drop(queue);
                        self.shared.changed.notify_all();
                        break;
                    }
                    drop(queue);
                    if class == WorkClass::Interactive
                        && let Some(permit) = try_permit(&self.shared, class)
                    {
                        admitted += 1;
                        run_chunk(
                            &self.shared,
                            chunk.take().expect("single inline admission"),
                            Some(permit),
                            None,
                        );
                        break;
                    }
                    if !help(&self.shared, Some(&window)) {
                        wait_for_change(&self.shared);
                    }
                }
                if failure.is_some() {
                    break;
                }
            }
            drop(sender);
            let mut settled = 0;
            while settled < admitted {
                match receiver.try_recv() {
                    Ok(result) => {
                        settled += 1;
                        if failure.is_none() {
                            failure = result.err();
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        if !help(&self.shared, Some(&window)) {
                            wait_for_change(&self.shared);
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        failure = Some(Error::new("work_incomplete", "Work executor disconnected"));
                        break;
                    }
                }
            }
            failure.map_or(Ok(()), Err)
        })
    }
    fn stats(&self) -> WorkStats {
        let queue = lock(&self.shared.queue);
        let mut stats = *lock(&self.shared.stats);
        stats.queued = queue.count;
        stats
    }
}
impl Drop for NativeWorkService {
    fn drop(&mut self) {
        *lock(&self.shared.closed) = true;
        self.shared.changed.notify_all();
        for thread in lock(&self.threads).drain(..) {
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}
