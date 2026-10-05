//! Actionable CLI diagnostics; nested stage errors retain compiler/tool output.
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RevenantError {
    #[error(
        "directory already exists: {0}. Choose a different name or delete the existing directory"
    )]
    DirectoryExists(PathBuf),

    #[error(
        "invalid project name \"{name}\": {reason}. Use only alphanumeric characters, hyphens, and underscores"
    )]
    InvalidProjectName { name: String, reason: String },

    #[error("missing required tools:\n{}\n\nRun 'revenant setup' for guided installation help", format_missing(.0))]
    MissingTools(Vec<MissingTool>),

    #[error(
        "could not find revenant.toml in current or parent directories. Run this command from inside a Revenant project, or create one with 'revenant new'"
    )]
    ProjectNotFound,

    #[error(
        "desktop build failed. Run with --verbose for compiler output; check native operations and Tauri platform prerequisites"
    )]
    DesktopBuildFailed,

    #[error(
        "web build failed. Run with --verbose for the failing stage; check Svelte diagnostics and web dependencies"
    )]
    WebBuildFailed,

    #[error(
        "Vite crashed unexpectedly. If the port is in use, stop the conflicting process or set desktop.dev_port in revenant.toml and restart revenant dev"
    )]
    DevServerCrashed,
}

#[derive(Debug)]
pub struct MissingTool {
    pub name: String,
    pub install_hint: String,
}

fn format_missing(tools: &[MissingTool]) -> String {
    tools
        .iter()
        .map(|t| format!("  - {}: install via `{}`", t.name, t.install_hint))
        .collect::<Vec<_>>()
        .join("\n")
}
