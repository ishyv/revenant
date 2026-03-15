use anyhow::{Context, Result};
use colored::Colorize;

use crate::config::{self, RevenantConfig, WEB_SUBDIR};
use crate::errors::RevenantError;
use crate::toolchain::process;
use crate::toolchain::wasm::{WasmBuilder, WasmPackBuilder};

/// Run a production build: WASM release build via wasm-pack, then the web
/// framework's build command. Fails fast if either stage errors.
pub fn run(verbose: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let root = config::find_project_root(&cwd).ok_or(RevenantError::ProjectNotFound)?;
    let config = RevenantConfig::load(&root)?;

    println!("\n{}", "Revenant — Production build".bold());
    println!();

    let web_dir = root.join(WEB_SUBDIR);

    if !web_dir.join("node_modules").exists() {
        anyhow::bail!(
            "node_modules not found in {}. Run 'npm install' in the web directory first, \
             or use 'revenant dev' which installs automatically.",
            web_dir.display()
        );
    }

    let builder = WasmPackBuilder::new(&root, &config.toolchain.wasm_target, verbose);

    // Step 1: WASM release build
    println!("  {} Building WASM (release)...", "▸".green().bold());
    builder.build_release()?;
    println!("  {} WASM build complete", "✓".green().bold());

    // Step 2: Web build
    println!("  {} Building web app...", "▸".green().bold());
    process::run_blocking(&web_dir, &config.toolchain.pkg_manager, &["run", "build"], verbose)
        .context(RevenantError::WebBuildFailed)?;
    println!("  {} Web build complete", "✓".green().bold());

    println!();
    println!("  {} Production build finished!", "✓".green().bold());
    println!();
    println!("  Preview with:");
    println!("    cd web && npm run preview");
    println!();

    Ok(())
}
