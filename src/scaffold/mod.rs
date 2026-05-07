// TODO(v2): support template variants (e.g. "minimal", "full") via a --template flag

pub mod templates;

use anyhow::{Context, Result};
use std::path::Path;

use crate::config::{RUST_SUBDIR, WEB_SUBDIR, wasm_package_name};

fn substitute(template: &str, project_name: &str) -> String {
    template
        .replace("{project_name}", project_name)
        .replace("{wasm_name}", &wasm_package_name(project_name))
}

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
            substitute(templates::REVENANT_TOML, name),
        ),
        (
            root.join(RUST_SUBDIR).join("Cargo.toml"),
            substitute(templates::RUST_CARGO_TOML, name),
        ),
        (
            root.join(RUST_SUBDIR).join("src").join("lib.rs"),
            substitute(templates::RUST_LIB_RS, name),
        ),
        (
            root.join(WEB_SUBDIR).join("package.json"),
            substitute(templates::WEB_PACKAGE_JSON, name),
        ),
        (
            root.join(WEB_SUBDIR).join("svelte.config.js"),
            substitute(templates::WEB_SVELTE_CONFIG, name),
        ),
        (
            root.join(WEB_SUBDIR).join("vite.config.js"),
            substitute(templates::WEB_VITE_CONFIG, name),
        ),
        (
            root.join(WEB_SUBDIR).join("src").join("app.html"),
            substitute(templates::WEB_APP_HTML, name),
        ),
        (
            root.join(WEB_SUBDIR)
                .join("src")
                .join("routes")
                .join("+page.svelte"),
            substitute(templates::WEB_PAGE_SVELTE, name),
        ),
        (
            root.join(WEB_SUBDIR)
                .join("src")
                .join("lib")
                .join("wasm.ts"),
            substitute(templates::WEB_WASM_TS, name),
        ),
        (
            root.join("GETTING_STARTED.md"),
            substitute(templates::GETTING_STARTED_MD, name),
        ),
    ];

    for (path, content) in &files {
        std::fs::write(path, content)
            .with_context(|| format!("failed to write file: {}", path.display()))?;
    }

    Ok(())
}
