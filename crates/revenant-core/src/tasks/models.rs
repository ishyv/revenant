//! Wire snapshots use item counters for batches and caller-defined units for progress. Terminal snapshots are immutable.
use super::*;
/// Lifecycle state reported by snapshots; only the execution guard finalizes running work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    /// Created and awaiting an executor.
    Queued,
    /// An executor owns the task and may publish progress.
    Running,
    /// Cancellation was requested; the executor has not finalized yet.
    Cancelling,
    /// Execution returned successfully with no recorded item failures.
    Succeeded,
    /// At least one item succeeded, while another item or execution failed.
    Partial,
    /// Execution or recorded items failed without any recorded success.
    Failed,
    /// Cancellation was acknowledged without recorded successful items.
    Cancelled,
    /// The execution guard dropped before reporting completion.
    Interrupted,
}
impl TaskState {
    /// Returns true for finalized states whose snapshots reject further mutation.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Partial | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}
/// Monotonic completed work in caller-selected units. Byte readers use bytes;
/// batch parents use items. A new task starts at zero with unknown total.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Progress {
    /// Completed work in the same units as total; updates may not decrease it.
    pub completed: DecimalU64,
    /// Optional upper bound in completed-work units; omitted when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub total: Option<DecimalU64>,
    /// Optional current status text, omitted when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
}
/// A legacy retained item result. Exactly one of output or error must be present
/// when recorded. Streaming hosts use BatchSummary instead of retaining these values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Outcome {
    /// Zero-based item index; recording an existing index returns `duplicate_outcome`.
    pub index: u32,
    /// Successful item output; mutually exclusive with error, omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output: Option<Value>,
    /// Item failure; mutually exclusive with output, omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<Error>,
}
/// Constant-size batch accounting; each completed item is either successful or failed.
/// Counters count items (not bytes) and serialize as canonical decimal strings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchSummary {
    /// Number of recorded items; equals succeeded plus failed. Initially zero.
    pub completed: DecimalU64,
    /// Number of items recorded with a successful result. Initially zero.
    pub succeeded: DecimalU64,
    /// Number of items recorded with an error result. Initially zero.
    pub failed: DecimalU64,
}

/// Owned point-in-time task view delivered to subscribers. Terminal views are immutable.
/// Streaming batch accounting occupies constant space through summary; legacy
/// outcomes grow only when a caller explicitly records full item results.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskSnapshot {
    /// Task identifier unique among the manager entries currently retained.
    pub id: String,
    /// Owning task scope used for resource admission and cancellation checkpoints.
    pub scope: String,
    /// Current lifecycle state; terminal states prohibit task mutation.
    pub state: TaskState,
    /// Latest monotonic work estimate, distinct from recorded batch result counts.
    pub progress: Progress,
    /// Legacy explicitly retained item results; streaming hosts leave this empty.
    pub outcomes: Vec<Outcome>,
    /// Constant-size item totals, initialized to zero for a newly queued task.
    #[serde(default)]
    pub summary: BatchSummary,
    /// Executor return value after successful return, including partial batch completion.
    /// Explicit finish_batch also retains its descriptor after acknowledged cancellation;
    /// ordinary finish with an error leaves this absent. Streaming hosts keep it bounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub result: Option<Value>,
    /// Executor failure or interruption diagnostic; absent when execution returned Ok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<Error>,
    /// Whether cancellation has been requested; running executors must checkpoint to acknowledge it.
    pub cancel_requested: bool,
}
