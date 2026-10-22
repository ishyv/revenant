//! Explicit SDK inspection and backed-up application-local recovery commands.
use crate::{
    config::{self, RevenantConfig},
    errors::RevenantError,
    scaffold::sdk,
};
use anyhow::Result;
use std::path::{Path, PathBuf};

fn root() -> Result<PathBuf> {
    let root = config::find_project_root(&std::env::current_dir()?)
        .ok_or(RevenantError::ProjectNotFound)?;
    Ok(dunce::canonicalize(root)?)
}

/// Validate sources and the installed client independently, without file writes.
pub fn check() -> Result<()> {
    sdk::check(&root()?)?;
    println!(
        "SDK and installed client are compatible (revision {}, framework 0.4.0).",
        sdk::COMPATIBILITY_REVISION
    );
    Ok(())
}

/// Back up current state, replace SDK/dependencies, and validate native contracts.
pub fn upgrade(verbose: bool) -> Result<()> {
    let root = root()?;
    let config = RevenantConfig::load(&root)?;
    let backup = sdk::upgrade(&root, &config, verbose)?;
    println!(
        "SDK upgraded and native contracts validated. Previous state: {}",
        backup.display()
    );
    Ok(())
}

/// Restore a backup from this application; retain an inverse backup for redo.
pub fn restore(backup: &Path) -> Result<()> {
    let inverse = sdk::restore(&root()?, backup)?;
    println!(
        "SDK state restored. Recovery/redo record: {}",
        inverse.display()
    );
    Ok(())
}
