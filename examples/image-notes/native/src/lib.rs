//! Luma: a persistent image catalog expressed through Revenant operations.
mod catalog;
use revenant::{Application, operations};

/// Compose the catalog with Revenant's built-in picker, tasks and settings.
pub fn app() -> Application {
    Application::new().operations(operations![
        catalog::bootstrap,
        catalog::scan,
        catalog::query,
        catalog::annotate,
        catalog::render,
        catalog::remove_root,
    ])
}
