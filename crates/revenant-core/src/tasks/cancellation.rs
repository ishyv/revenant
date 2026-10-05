//! Cancellation is cooperative. Children checkpoint their parent, and disposed task scopes request cancellation.
use super::*;
/// Thread-safe host cancellation signal polled at task checkpoints.
/// The probe is shared with callers and must not block while checking its signal.
pub trait CancellationProbe: Send + Sync + 'static {
    /// Returns whether the host requests cancellation; true is acknowledged at a checkpoint.
    fn is_cancelled(&self) -> bool;
}
impl<F: Fn() -> bool + Send + Sync + 'static> CancellationProbe for F {
    fn is_cancelled(&self) -> bool {
        self()
    }
}

impl TaskContext {
    /// Replaces the host probe until finalization. Returns `task_terminal` for a
    /// finalized task; setting a probe does not itself request cancellation.
    pub fn set_cancellation_probe(&self, probe: Arc<dyn CancellationProbe>) -> Result<()> {
        let mut data = lock(&self.task.data);
        Self::mutable(&data)?;
        data.cancellation_probe = Some(probe);
        Ok(())
    }
    /// Requests cooperative cancellation. Queued tasks become Cancelled and release
    /// leases immediately; started tasks become Cancelling until finalized.
    /// Already terminal tasks return their current immutable snapshot unchanged.
    pub fn request_cancel(&self) -> Result<TaskSnapshot> {
        if self.snapshot().state.is_terminal() {
            return Ok(self.snapshot());
        }
        let mut leases = HashMap::new();
        let result = self.change(|data| {
            if data.snapshot.state.is_terminal() {
                return Ok(());
            }
            data.snapshot.cancel_requested = true;
            if data.started {
                data.snapshot.state = TaskState::Cancelling;
            } else {
                data.snapshot.state = TaskState::Cancelled;
                data.snapshot.error = Some(Error::cancelled());
                leases = std::mem::take(&mut data.leases);
            }
            Ok(())
        });
        drop(leases);
        result
    }
    /// Cooperatively acknowledges a cancellation request. Finalization belongs to
    /// the executor, so recorded batch counters and legacy outcomes remain available.
    /// Checks parent cancellation, the host probe, and task scope disposal. Returns
    /// `cancelled` after requesting cancellation, or `task_terminal` if already finalized.
    pub fn checkpoint(&self) -> Result<()> {
        let (terminal, cancelled, probe) = {
            let data = lock(&self.task.data);
            (
                data.snapshot.state.is_terminal(),
                data.snapshot.cancel_requested,
                data.cancellation_probe.clone(),
            )
        };
        if terminal {
            return Err(Error::new("task_terminal", "Task has already finished"));
        }
        if let Some(parent) = &self.task.parent {
            if let Err(error) = parent.context.checkpoint() {
                // Finalizing a cancelled parent must not turn a child's next
                // checkpoint into an unrelated task_terminal failure.
                let parent_cancelled = lock(&parent.context.task.data).snapshot.cancel_requested;
                if error.is_cancelled() || parent_cancelled {
                    self.request_cancel()?;
                    return Err(Error::cancelled());
                }
                return Err(error);
            }
        }
        if cancelled || probe.is_some_and(|p| p.is_cancelled()) {
            self.request_cancel()?;
            return Err(Error::cancelled());
        }
        if !self.task.resources.is_scope_active(self.scope()) {
            self.request_cancel()?;
            return Err(Error::cancelled());
        }
        Ok(())
    }
}
