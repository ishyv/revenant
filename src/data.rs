
/// Handles app "state".
/// In other words, this module defines the structure that holds the app's data.
/// Made to not only store data, but also provide utilities to work with it.

use std::{path::{Path, PathBuf}};

/// Represents the global app data.
/// Allows different parts of the project to know "where" and "how" to do things necessary
/// for the app to run.
/// - Ex. Where to store output files, where to find config files, etc.
#[derive(Clone)]
pub struct AppContext {
    /// Marely for aesthetics, not used for anything important.
    pub project_name: String,

    /// If true, will attempt to auto-install missing packages.
    /// Otherwise will just error out if a required package is missing.
    pub auto_install: bool,

    /// Base folder for all outputs/configs/etc.
    pub root_path: PathBuf,

    /// Path to the Svelte project folder.
    pub svelte_path: PathBuf,

}


impl AppContext {
    pub fn new(root: impl Into<PathBuf> ) -> Self {

        let root = root.into(); // Ensure we have a PathBuf, used to be able to create relative paths.

        Self {
            project_name: "Revenant".to_string(),
            auto_install: false, // TODO: once auto-install is implemented, make this configurable.
            root_path: root.clone(),
            svelte_path: root.join("svelte"),
        }
    }
}

