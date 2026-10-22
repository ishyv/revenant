//! Scope-owned capture/compute/commit coordination with bounded replacement.
use crate::{Error, ResourceRegistry, Result, TaskContext, lock};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Final disposition of one latest-work request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatestSubmission {
    /// The request committed under the application lock.
    Committed,
    /// A newer request or changed application version prevented publication.
    Superseded,
}
#[derive(Default)]
struct Completion(Mutex<Option<Result<LatestSubmission>>>);
impl Completion {
    fn settle(&self, outcome: Result<LatestSubmission>) {
        *lock(&self.0) = Some(outcome);
    }
    fn outcome(&self) -> Option<Result<LatestSubmission>> {
        lock(&self.0).clone()
    }
}
struct Request<I, V> {
    generation: u64,
    input: I,
    version: V,
    context: TaskContext,
    completion: Arc<Completion>,
}
struct Slot<I, V> {
    current: u64,
    pending: Option<Request<I, V>>,
}
struct Jobs<I, V> {
    sequence: u64,
    slots: HashMap<String, Slot<I, V>>,
}
type Compute<I, O> = dyn Fn(I, TaskContext) -> Result<O> + Send + Sync;
type Version<S, V> = dyn Fn(&S) -> V + Send + Sync;
type Publish<S, O> = dyn Fn(&mut S, O) -> Result<()> + Send + Sync;

/// Coordinates derived work across admitted tasks in one native scope.
/// The creation task may finish; the controller retains scope identity, not that
/// task's execution. Each key has one active and one replaceable pending input.
/// Every submitting task waits for its own final disposition, retaining its
/// leases until started computation settles. Hosts lend waiting worker permits.
///
/// Capture and publication share the application lock. Compute receives its own
/// request's context and runs outside both locks. Application input/output
/// allocations remain application costs; the controller spawns no threads.
pub struct LatestJobs<S, I, O, V> {
    resources: ResourceRegistry,
    scope: String,
    application: Arc<Mutex<S>>,
    jobs: Mutex<Jobs<I, V>>,
    limit: usize,
    compute: Arc<Compute<I, O>>,
    version: Arc<Version<S, V>>,
    publish: Arc<Publish<S, O>>,
}
impl<S, I, O, V> LatestJobs<S, I, O, V>
where
    S: Send + 'static,
    I: Send + 'static,
    O: Send + 'static,
    V: Eq + Send + 'static,
{
    /// Creates a controller for the creator's scope with 1..=32 active keys.
    pub fn new(
        owner: TaskContext,
        application: Arc<Mutex<S>>,
        max_keys: usize,
        compute: impl Fn(I, TaskContext) -> Result<O> + Send + Sync + 'static,
        version: impl Fn(&S) -> V + Send + Sync + 'static,
        publish: impl Fn(&mut S, O) -> Result<()> + Send + Sync + 'static,
    ) -> Result<Self> {
        owner.checkpoint()?;
        if !(1..=32).contains(&max_keys) {
            return Err(Error::new(
                "latest_limit",
                "Latest work supports 1..=32 active keys",
            ));
        }
        Ok(Self {
            resources: owner.resource_registry(),
            scope: owner.scope().into(),
            application,
            jobs: Mutex::new(Jobs {
                sequence: 0,
                slots: HashMap::new(),
            }),
            limit: max_keys,
            compute: Arc::new(compute),
            version: Arc::new(version),
            publish: Arc::new(publish),
        })
    }
    fn admit(&self, context: &TaskContext) -> Result<()> {
        if context.scope() != self.scope {
            return Err(Error::new(
                "scope_mismatch",
                "Latest work belongs to another native scope",
            ));
        }
        if !self.resources.is_scope_active(&self.scope) {
            return Err(Error::cancelled());
        }
        context.checkpoint()
    }
    /// Captures input and version together, then waits for this request to settle.
    pub fn capture(
        &self,
        context: &TaskContext,
        key: &str,
        capture: impl FnOnce(&S) -> Result<I>,
    ) -> Result<LatestSubmission> {
        self.admit(context)?;
        let (input, version) = {
            let state = lock(&self.application);
            (capture(&state)?, (self.version)(&state))
        };
        self.submit(context, key, input, version)
    }
    /// Submits an immutable input/version pair. The first caller drives the key;
    /// replacement callers lend their permits while waiting. Replacing a pending
    /// request settles that caller as Superseded without executing its input.
    /// Cancelling an active request never abandons its native computation.
    pub fn submit(
        &self,
        context: &TaskContext,
        key: &str,
        input: I,
        version: V,
    ) -> Result<LatestSubmission> {
        self.admit(context)?;
        if key.is_empty() || key.len() > 128 {
            return Err(Error::new(
                "latest_key",
                "Latest keys require 1..=128 UTF-8 bytes",
            ));
        }
        let completion = Arc::new(Completion::default());
        let (request, generation, replaced) = {
            let admission = lock(&self.application);
            let mut jobs = lock(&self.jobs);
            if !jobs.slots.contains_key(key) && jobs.slots.len() >= self.limit {
                return Err(Error::new("latest_limit", "Latest key capacity exhausted"));
            }
            jobs.sequence = jobs
                .sequence
                .checked_add(1)
                .ok_or_else(|| Error::new("latest_exhausted", "Latest generation exhausted"))?;
            let generation = jobs.sequence;
            let request = Request {
                generation,
                input,
                version,
                context: context.clone(),
                completion: completion.clone(),
            };
            let admitted = if let Some(slot) = jobs.slots.get_mut(key) {
                slot.current = generation;
                (None, generation, slot.pending.replace(request))
            } else {
                jobs.slots.insert(
                    key.into(),
                    Slot {
                        current: generation,
                        pending: None,
                    },
                );
                (Some(request), generation, None)
            };
            drop(jobs);
            drop(admission);
            admitted
        };
        if let Some(replaced) = replaced {
            replaced.completion.settle(Ok(LatestSubmission::Superseded));
            drop(replaced);
        }
        if let Some(request) = request {
            self.drive(key, request);
        } else {
            context.wait_until(&|| {
                if completion.outcome().is_some() {
                    return true;
                }
                if let Err(error) = context.checkpoint() {
                    // Remove only a still-pending request. If the driver already
                    // started it, its caller must retain leases until completion.
                    let removed = {
                        let mut jobs = lock(&self.jobs);
                        jobs.slots.get_mut(key).and_then(|slot| {
                            if slot
                                .pending
                                .as_ref()
                                .is_some_and(|pending| pending.generation == generation)
                            {
                                slot.pending.take()
                            } else {
                                None
                            }
                        })
                    };
                    if let Some(removed) = removed {
                        removed.completion.settle(Err(error));
                        drop(removed);
                    }
                }
                completion.outcome().is_some()
            })?;
        }
        completion
            .outcome()
            .expect("latest request settled before returning")
    }
    fn drive(&self, key: &str, request: Request<I, V>) {
        // Callback unwinding settles both active and pending callers and releases
        // their values outside coordination locks.
        struct Finish<'a, I, V> {
            jobs: &'a Mutex<Jobs<I, V>>,
            key: &'a str,
            active: Arc<Completion>,
            armed: bool,
        }
        impl<I, V> Drop for Finish<'_, I, V> {
            fn drop(&mut self) {
                if self.armed {
                    let removed = {
                        let mut jobs = lock(self.jobs);
                        let removed = jobs.slots.remove(self.key);
                        if jobs.slots.is_empty() {
                            jobs.slots.shrink_to_fit();
                        }
                        removed
                    };
                    let error = Error::new(
                        "latest_interrupted",
                        "Latest driver unwound before settlement",
                    );
                    self.active.settle(Err(error.clone()));
                    if let Some(pending) = removed.as_ref().and_then(|slot| slot.pending.as_ref()) {
                        pending.completion.settle(Err(error));
                    }
                    drop(removed);
                }
            }
        }
        let mut finish = Finish {
            jobs: &self.jobs,
            key,
            active: request.completion.clone(),
            armed: true,
        };
        let mut request = request;
        loop {
            let Request {
                generation,
                input,
                version,
                context,
                completion,
            } = request;
            let result = context
                .checkpoint()
                .and_then(|()| (self.compute)(input, context.clone()))
                .and_then(|output| {
                    // Checkpoints may invoke probes and cancellation observers;
                    // none may run while we hold the application's state lock.
                    let mut application = loop {
                        context.checkpoint()?;
                        match self.application.try_lock() {
                            Ok(application) => break application,
                            Err(std::sync::TryLockError::Poisoned(error)) => {
                                break error.into_inner();
                            }
                            Err(std::sync::TryLockError::WouldBlock) => {
                                std::thread::sleep(std::time::Duration::from_millis(2));
                            }
                        }
                    };
                    let snapshot = context.snapshot();
                    if snapshot.cancel_requested || !self.resources.is_scope_active(&self.scope) {
                        return Err(Error::cancelled());
                    }
                    if snapshot.state.is_terminal() {
                        return Err(Error::new("task_terminal", "Task has already finished"));
                    }
                    let current = lock(&self.jobs)
                        .slots
                        .get(key)
                        .is_some_and(|slot| slot.current == generation);
                    if !current || (self.version)(&application) != version {
                        return Ok(LatestSubmission::Superseded);
                    }
                    (self.publish)(&mut application, output)?;
                    Ok(LatestSubmission::Committed)
                });
            completion.settle(result);
            let (pending, removed) = {
                let mut jobs = lock(&self.jobs);
                let pending = jobs
                    .slots
                    .get_mut(key)
                    .expect("active driver owns key")
                    .pending
                    .take();
                if let Some(pending) = pending {
                    finish.active = pending.completion.clone();
                    (Some(pending), None)
                } else {
                    let removed = jobs.slots.remove(key);
                    if jobs.slots.is_empty() {
                        jobs.slots.shrink_to_fit();
                    }
                    finish.armed = false;
                    (None, removed)
                }
            };
            drop(removed);
            match pending {
                Some(next) => request = next,
                None => return,
            }
        }
    }
    /// Number of active keys, independent of pending input.
    pub fn active_count(&self) -> usize {
        lock(&self.jobs).slots.len()
    }
    /// Number of retained pending replacements, at most one per active key.
    pub fn pending_count(&self) -> usize {
        lock(&self.jobs)
            .slots
            .values()
            .filter(|slot| slot.pending.is_some())
            .count()
    }
}
