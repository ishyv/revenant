//! Exactly one execution guard owns actual execution and cached lease release.
use super::context::Publication;
use super::*;
impl TaskContext {
    /// Records host interruption after a shutdown deadline or lost execution host.
    /// This finalizes the logical task but retains started execution leases until
    /// its execution guard exits; it cannot kill a native thread. A worker
    /// that later reports completion receives `task_terminal`; partial counters
    /// remain available. Already terminal tasks return their existing snapshot.
    pub fn interrupt(&self) -> Result<TaskSnapshot> {
        if self.snapshot().state.is_terminal() {
            return Ok(self.snapshot());
        }
        self.finish(
            Err(Error::new(
                "interrupted",
                "Native execution exceeded the shutdown grace period",
            )),
            true,
            None,
        )
    }
    /// Claims the sole execution guard and marks the task Running. Returns
    /// `task_started` if already claimed and `task_terminal` if finalized.
    /// Hosts should checkpoint before performing work; begin does not poll cancellation.
    pub fn begin(&self) -> Result<TaskExecution> {
        let publication = self.prepare_change(|data| {
            Self::mutable(data)?;
            if data.started {
                return Err(Error::new("task_started", "Task already has an executor"));
            }
            data.started = true;
            if !data.snapshot.cancel_requested {
                data.snapshot.state = TaskState::Running;
            }
            Ok(())
        })?;
        let execution = TaskExecution {
            context: self.clone(),
            finished: false,
        };
        publication.publish();
        Ok(execution)
    }
    pub(super) fn finish(
        &self,
        result: Result<Value>,
        interrupted: bool,
        batch_result: Option<Value>,
    ) -> Result<TaskSnapshot> {
        Ok(self
            .prepare_finish(result, interrupted, batch_result)?
            .publish())
    }
    fn prepare_finish(
        &self,
        result: Result<Value>,
        interrupted: bool,
        batch_result: Option<Value>,
    ) -> Result<Publication> {
        let (leases, created, probe, schema, publication) = {
            let mut data = lock(&self.task.data);
            Self::mutable(&data)?;
            let next_revision = data.snapshot.revision.get().checked_add(1).ok_or_else(|| {
                Error::new(
                    "task_revision_exhausted",
                    "Task revision counter is exhausted",
                )
            })?;
            let successful = data.snapshot.summary.succeeded.get() > 0
                || data.snapshot.outcomes.iter().any(|o| o.output.is_some());
            let failed = data.snapshot.summary.failed.get() > 0
                || data.snapshot.outcomes.iter().any(|o| o.error.is_some());
            match result {
                Ok(value) => {
                    data.snapshot.state = if failed {
                        if successful {
                            TaskState::Partial
                        } else {
                            TaskState::Failed
                        }
                    } else {
                        TaskState::Succeeded
                    };
                    data.snapshot.result = Some(value);
                }
                Err(error) => {
                    data.snapshot.state = if interrupted {
                        TaskState::Interrupted
                    } else if successful {
                        TaskState::Partial
                    } else if error.is_cancelled() {
                        TaskState::Cancelled
                    } else {
                        TaskState::Failed
                    };
                    data.snapshot.error = Some(error);
                }
            }
            if let Some(value) = batch_result {
                data.snapshot.result = Some(value);
            }
            data.snapshot.revision = next_revision.into();
            let schema = data.update_schema.take();
            let probe = data.cancellation_probe.take();
            let returned = data.snapshot.result.is_some()
                || data
                    .snapshot
                    .outcomes
                    .iter()
                    .any(|outcome| outcome.output.is_some());
            let created = std::mem::take(&mut data.created_native);
            let leases = if interrupted && data.started {
                // A shutdown deadline settles the logical snapshot, not the
                // native execution. Its guard still owns actual completion.
                (HashMap::new(), HashMap::new())
            } else {
                (
                    std::mem::take(&mut data.leases),
                    std::mem::take(&mut data.native_leases),
                )
            };
            (
                leases,
                if returned { HashMap::new() } else { created },
                probe,
                schema,
                data.publication(),
            )
        };
        self.rollback_created(created);
        drop(leases);
        drop(probe);
        drop(schema);
        Ok(publication)
    }
    fn release_execution_leases(&self) {
        let leases = {
            let mut data = lock(&self.task.data);
            (
                std::mem::take(&mut data.leases),
                std::mem::take(&mut data.native_leases),
            )
        };
        // Destruction may reenter the task or registry after actual execution.
        drop(leases);
    }
}
/// Dropping an executing future records interruption and releases its resource leases.
/// Observer unwinds during destruction are contained after cleanup; explicit
/// finish calls retain ordinary observer panic propagation. Custom resource and
/// callback-capture destructors must not panic during cleanup.
pub struct TaskExecution {
    context: TaskContext,
    finished: bool,
}
impl TaskExecution {
    /// Consumes the guard, finalizes the snapshot, and releases cached leases.
    /// Summary counters and retained legacy outcomes both determine partial or failed
    /// batch state. Errors with recorded success become Partial; cancellation without
    /// success becomes Cancelled, with no result value. A cancellation request
    /// alone does not override a successful return: the executor must acknowledge
    /// cancellation with its error. Returns `task_terminal` if already finalized.
    pub fn finish(mut self, result: Result<Value>) -> Result<TaskSnapshot> {
        let publication = self.context.prepare_finish(result, false, None)?;
        self.finished = true;
        Ok(publication.publish())
    }
    /// Finalizes a batch while retaining its result descriptor in the snapshot.
    /// `cancelled` is an explicit acknowledgement by the host, not the task's
    /// cancellation-request flag. When true, the state is Partial if any item
    /// succeeded, otherwise Cancelled, and the error is `cancelled`. The supplied
    /// result and recorded summary remain available in either case.
    /// When false, finalization follows the same summary/outcome rules as
    /// finish(Ok(result)); an ignored cancellation request does not override success.
    /// Consumes the guard and releases cached leases. Returns `task_terminal`
    /// if the task was already finalized. Hosts should supply a bounded result
    /// descriptor rather than embedding all batch outcomes in this value.
    pub fn finish_batch(mut self, result: Value, cancelled: bool) -> Result<TaskSnapshot> {
        let (outcome, batch_result) = if cancelled {
            (Err(Error::cancelled()), Some(result))
        } else {
            (Ok(result), None)
        };
        let publication = self.context.prepare_finish(outcome, false, batch_result)?;
        self.finished = true;
        Ok(publication.publish())
    }
}
impl Drop for TaskExecution {
    fn drop(&mut self) {
        let publication = if !self.finished {
            self.context
                .prepare_finish(
                    Err(Error::new(
                        "interrupted",
                        "Executor dropped before reporting completion",
                    )),
                    true,
                    None,
                )
                .ok()
        } else {
            None
        };
        self.context.release_execution_leases();
        if let Some(publication) = publication {
            publication.publish_cleanup();
        }
    }
}
