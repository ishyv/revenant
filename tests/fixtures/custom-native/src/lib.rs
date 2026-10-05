//! Author-owned application library used by the CLI portability proof.
use revenant::{Application, operations};
mod records;

/// Compose a custom operation with the framework's native built-ins.
pub fn app() -> Application {
    Application::new().operations(operations![records::normalize])
}
