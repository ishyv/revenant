//! Errors cross the JSON boundary without transporting native error objects. Cancellation is distinguished by its machine code.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// A core operation result carrying a serializable machine code and diagnostic context.
pub type Result<T> = std::result::Result<T, Error>;

/// A boundary-safe failure. Codes are stable dispatch keys; messages are diagnostics.
/// Optional details contain JSON context. Retryability is a host hint, not an automatic retry policy.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Error {
    /// Machine-readable failure category, such as `stale_handle` or `cancelled`.
    pub code: String,
    /// Human-readable diagnostic explaining this failure; callers should branch on code.
    pub message: String,
    /// Optional structured JSON context; omitted on the wire when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub details: Option<Value>,
    /// Whether a host may reasonably retry after addressing the cause; false by default.
    pub retryable: bool,
}

impl Error {
    /// Constructs a failure with no details and retryability disabled.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
            retryable: false,
        }
    }
    /// Attaches structured diagnostic context and returns the modified error.
    pub fn details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
    /// Sets the retry hint without scheduling work or changing the failure code.
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
    /// Builds the cancellation acknowledgement used by cooperative checkpoints.
    pub fn cancelled() -> Self {
        Self::new("cancelled", "The operation acknowledged cancellation")
    }
    /// Checks the stable `cancelled` code, independent of diagnostic wording.
    pub fn is_cancelled(&self) -> bool {
        self.code == "cancelled"
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new("invalid_json", e.to_string())
    }
}
