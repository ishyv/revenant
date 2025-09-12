//! Application context (paths, configuration flags), shared across steps.
//!
//! Keep this lean and focused on values that many parts of the program need.
//! Prefer deriving small helpers instead of storing redundant fields.

use std::path::PathBuf;

/// Represents the global app data.
/// Allows different parts of the project to know "where" and "how" to do things necessary
/// for the app to run.
/// - Ex. Where to store output files, where to find config files, etc.
#[derive(Clone)]
pub struct AppContext {
    /// If true, will attempt to auto-install missing packages.
    /// Otherwise will just error out if a required package is missing.
    pub auto_install: bool,

    /// Base folder for all outputs/configs/etc.
    pub root_path: PathBuf,

    /// Path to the Svelte project folder.
    pub svelte_path: PathBuf,

}


impl AppContext {
    /// Constructs a new `AppContext` rooted at `root`.
    ///
    /// - `root_path` becomes the output directory (see `consts::DEFAULT_ROOT`).
    /// - `svelte_path` is derived as `<root>/svelte`.
    /// - `auto_install` defaults to `false` (explicit install preferred for safety).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            auto_install: false,
            root_path: root.clone(),
            svelte_path: root.join("svelte"),
        }
    }
}

