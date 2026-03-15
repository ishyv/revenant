// TODO(v2): support template variants (e.g. "minimal", "full") via a --template flag

pub mod templates;

use anyhow::{Context, Result};
use std::path::Path;

use crate::config::{RUST_SUBDIR, WEB_SUBDIR};

/// Create the full project directory tree and write all scaffold files.
pub fn create_project(root: &Path, name: &str) -> Result<()> {
    // Create directory structure
    let dirs = [
        root.to_path_buf(),
        root.join(RUST_SUBDIR),
        root.join(RUST_SUBDIR).join("src"),
        root.join(WEB_SUBDIR),
        root.join(WEB_SUBDIR).join("src"),
        root.join(WEB_SUBDIR).join("src").join("lib"),
        root.join(WEB_SUBDIR).join("src").join("routes"),
    ];

    for dir in &dirs {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create directory: {}", dir.display()))?;
    }

    // Write all template files with {project_name} replaced
    let files: Vec<(std::path::PathBuf, String)> = vec![
        (
            root.join("revenant.toml"),
            templates::REVENANT_TOML.replace("{project_name}", name),
        ),
        (
            root.join(RUST_SUBDIR).join("Cargo.toml"),
            templates::RUST_CARGO_TOML.replace("{project_name}", name),
        ),
        (
            root.join(RUST_SUBDIR).join("src").join("lib.rs"),
            templates::RUST_LIB_RS.replace("{project_name}", name),
        ),
        (
            root.join(WEB_SUBDIR).join("package.json"),
            templates::WEB_PACKAGE_JSON.replace("{project_name}", name),
        ),
        (
            root.join(WEB_SUBDIR).join("svelte.config.js"),
            templates::WEB_SVELTE_CONFIG.replace("{project_name}", name),
        ),
        (
            root.join(WEB_SUBDIR).join("vite.config.js"),
            templates::WEB_VITE_CONFIG.replace("{project_name}", name),
        ),
        (
            root.join(WEB_SUBDIR).join("src").join("app.html"),
            templates::WEB_APP_HTML.replace("{project_name}", name),
        ),
        (
            root.join(WEB_SUBDIR)
                .join("src")
                .join("routes")
                .join("+page.svelte"),
            templates::WEB_PAGE_SVELTE.replace("{project_name}", name),
        ),
        (
            root.join(WEB_SUBDIR)
                .join("src")
                .join("lib")
                .join("wasm.ts"),
            templates::WEB_WASM_TS.replace("{project_name}", name),
        ),
        (
            root.join("GETTING_STARTED.md"),
            templates::GETTING_STARTED_MD.replace("{project_name}", name),
        ),
    ];

    for (path, content) in &files {
        std::fs::write(path, content)
            .with_context(|| format!("failed to write file: {}", path.display()))?;
    }

    Ok(())
}
