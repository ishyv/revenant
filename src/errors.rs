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
        "WASM build failed. Run with --verbose to see full compiler output. Common causes: syntax errors in rust/src/lib.rs, or missing #[wasm_bindgen] on exported functions"
    )]
    WasmBuildFailed,

    #[error(
        "web build failed. Run with --verbose for full output. Ensure dependencies are installed (npm install) and the WASM package in pkg/ exists"
    )]
    WebBuildFailed,

    #[error(
        "dev server crashed unexpectedly. If the port is in use, stop other dev servers or set a custom port in web/vite.config.js"
    )]
    DevServerCrashed,

    #[error("dev server exited with code {0} — check output above")]
    DevServerExited(i32),
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
