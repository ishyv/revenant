//! Luma: a persistent image catalog expressed through Revenant operations.
mod catalog;
mod contracts;
mod error;
mod filesystem;
mod operations;
mod rendition;
mod scanning;
use revenant::Application;

/// Compose the catalog with Revenant's built-in picker, tasks and settings.
pub fn app() -> Application {
    Application::new().operations(revenant::operations![
        operations::bootstrap,
        operations::scan,
        operations::query,
        operations::annotate,
        operations::render,
        operations::remove_root,
    ])
}
