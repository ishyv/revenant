//! File/media desktop example: built-in native files, previews and checksum.
//!
//! Only record normalization is application-specific. The framework owns its
//! desktop bootstrap, task executor, settings storage and contract extraction.
use revenant::{Application, operations};
pub mod records;

/// Compose custom operations with Revenant's default built-in capabilities.
pub fn app() -> Application {
    Application::new().operations(operations![records::normalize])
}
