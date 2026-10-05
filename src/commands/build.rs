//! Desktop production pipeline: native compile, manifest export, SPA, packaging.
use crate::{
    compiled::BuildStage,
    config::{self, RevenantConfig},
    errors::RevenantError,
    toolchain::{desktop::DesktopBuild, process},
};
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

/// Compile the actual desktop executable and publish its compiled API atomically.
/// There is no WASM, source-AST, or optional server branch.
pub fn build_rust(
    root: &Path,
    config: &RevenantConfig,
    release: bool,
    verbose: bool,
) -> Result<String> {
    let build = DesktopBuild::new(root, config, release)?;
    build.build(verbose)?;
    BuildStage::new(root)?.publish(&build.executable, verbose)
}

/// Install the selected frontend manager's dependencies when not present.
/// The scaffold declares Tauri CLI 2, so packaging does not require cargo install.
pub fn ensure_frontend(root: &Path, config: &RevenantConfig, verbose: bool) -> Result<()> {
    crate::scaffold::prepare_host(root, config)?;
    let web = root.join("web");
    if !web.join("node_modules").is_dir() {
        process::run_blocking(&web, &config.toolchain.pkg_manager, &["install"], verbose)
            .context("install frontend dependencies; the application scaffold is preserved")?;
    }
    anyhow::ensure!(
        web.join("node_modules/@tauri-apps/cli/package.json")
            .is_file(),
        "missing Tauri CLI 2; add @tauri-apps/cli = ^2 as a web devDependency and run {} install in web/",
        config.toolchain.pkg_manager
    );
    let client = web.join("node_modules/@revenant/client/package.json");
    let installed: serde_json::Value = serde_json::from_slice(&std::fs::read(&client).context(
        "missing @revenant/client; run the frontend package manager's install in web/",
    )?)?;
    anyhow::ensure!(
        installed["version"].as_str() == Some("0.3.0"),
        "installed @revenant/client is not 0.3.0; preserve your SDK changes, update web/package.json to the 0.3 client snapshot, and reinstall frontend dependencies in web/. Existing contracts are preserved."
    );
    Ok(())
}

/// Resolve a local JS tool using its installed package metadata, never a global shim.
pub(crate) fn frontend_binary(root: &Path, package: &str, binary: &str) -> Result<PathBuf> {
    let module = dunce::canonicalize(root.join("web/node_modules").join(package))?;
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(module.join("package.json"))?)?;
    let path = metadata["bin"]
        .as_str()
        .or_else(|| metadata["bin"][binary].as_str())
        .with_context(|| format!("{package} does not declare {binary}"))?;
    let program = dunce::canonicalize(module.join(path))?;
    anyhow::ensure!(
        program.starts_with(&module),
        "frontend tool binary escapes its package"
    );
    Ok(program)
}

/// Build and package a static frontend with the compiled native application.
pub fn run(verbose: bool) -> Result<()> {
    let root = config::find_project_root(&std::env::current_dir()?)
        .ok_or(RevenantError::ProjectNotFound)?;
    let root = dunce::canonicalize(root)?;
    let config = RevenantConfig::load(&root)?;
    println!("\n{}", "Revenant — Desktop production build".bold());
    ensure_frontend(&root, &config, verbose)?;
    let generation = build_rust(&root, &config, true, verbose)?;
    process::run_blocking(
        &root.join("web"),
        &config.toolchain.pkg_manager,
        &["run", "build"],
        verbose,
    )
    .context(RevenantError::WebBuildFailed)?;
    let frontend = root.join("build/web");
    anyhow::ensure!(
        frontend.join("index.html").is_file(),
        "static frontend missing at {}; configure adapter-static with pages/assets '../build/web' and fallback 'index.html'",
        frontend.display()
    );
    let cli = frontend_binary(&root, "@tauri-apps/cli", "tauri")?;
    let target = root.join(".revenant/target");
    let host = root.join(".revenant/desktop");
    process::run_capture_with_env(
        &host,
        "node",
        &[&cli.to_string_lossy(), "build", "--no-bundle", "--ci"],
        &[("CARGO_TARGET_DIR", &target.to_string_lossy())],
        verbose,
    )
    .context("Tauri static desktop build failed; compiled contracts and SPA are preserved")?;
    // Tauri enables embedded assets and may compile again. Verify the final
    // executable before bundling so source edits during the frontend build
    // cannot produce an installer with an outdated generated contract.
    let executable = DesktopBuild::new(&root, &config, true)?.executable;
    let final_contract = process::run_capture(
        &root,
        &executable.to_string_lossy(),
        &["--revenant-contract"],
        verbose,
    )?;
    let expected: serde_json::Value = serde_json::from_slice(&std::fs::read(
        root.join("web/src/lib/.revenant")
            .join(generation)
            .join("revenant.contract.json"),
    )?)?;
    let actual: serde_json::Value = serde_json::from_str(&final_contract)?;
    anyhow::ensure!(
        actual == expected,
        "native contract changed during production build; rerun revenant build before distributing. No installer was generated for this build."
    );
    process::run_capture_with_env(
        &host,
        "node",
        &[&cli.to_string_lossy(), "bundle", "--ci"],
        &[("CARGO_TARGET_DIR", &target.to_string_lossy())],
        verbose,
    )
    .context("Tauri desktop bundling failed; compiled contracts and SPA are preserved")?;
    let bundles = target.join("release/bundle");
    anyhow::ensure!(
        bundles.is_dir(),
        "Tauri completed without desktop bundles at {}",
        bundles.display()
    );
    #[cfg(windows)]
    {
        let nsis = bundles.join("nsis");
        anyhow::ensure!(
            nsis.is_dir()
                && std::fs::read_dir(&nsis)?.any(|entry| entry.ok().is_some_and(|entry| entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "exe"))),
            "Tauri completed without an NSIS installer at {}",
            nsis.display()
        );
    }
    println!("  {} Desktop packages: {}", "✓".green(), bundles.display());
    Ok(())
}
