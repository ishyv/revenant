//! Exactly one execution guard owns finalization. Dropping an unfinished guard records interruption and releases cached leases.
use super::*;
impl TaskContext {
    /// Records host interruption after a shutdown deadline or lost execution host.
    /// This finalizes the logical task but cannot kill a native thread. A worker
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
        self.change(|data| {
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
        Ok(TaskExecution {
            context: self.clone(),
            finished: false,
        })
    }
    pub(super) fn finish(
        &self,
        result: Result<Value>,
        interrupted: bool,
        batch_result: Option<Value>,
    ) -> Result<TaskSnapshot> {
        let leases = {
            let mut data = lock(&self.task.data);
            Self::mutable(&data)?;
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
            std::mem::take(&mut data.leases)
        };
        drop(leases);
        self.change(|_| Ok(()))
    }
}
/// Dropping an executing future records interruption and releases its resource leases.
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
        let snapshot = self.context.finish(result, false, None)?;
        self.finished = true;
        Ok(snapshot)
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
        let snapshot = self.context.finish(outcome, false, batch_result)?;
        self.finished = true;
        Ok(snapshot)
    }
}
impl Drop for TaskExecution {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.context.finish(
                Err(Error::new(
                    "interrupted",
                    "Executor dropped before reporting completion",
                )),
                true,
                None,
            );
        }
    }
}
