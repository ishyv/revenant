//! Self-contained embedded SDK snapshot for generated desktop applications.
//!
//! New apps receive core, SDK, macros, desktop, client and the vendored HyvUI
//! archive. Existing snapshots are preserved by host preparation, so a local SDK
//! edit is not silently overwritten on each development rebuild.
use anyhow::{Context, Result};
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/sdk_sources.rs"));

/// Materialize a self-contained source workspace, never a path to the CLI checkout.
pub fn materialize(root: &Path) -> Result<()> {
    for required in [
        "crates/revenant-core/Cargo.toml",
        "crates/revenant-sdk/Cargo.toml",
        "crates/revenant-macros/Cargo.toml",
        "crates/revenant-desktop/Cargo.toml",
        "packages/client/package.json",
        "vendor/hyvui-1.0.0.tgz",
    ] {
        anyhow::ensure!(
            EMBEDDED.iter().any(|(p, _)| *p == required),
            "CLI was built without {required}; rebuild Revenant with the complete SDK and vendor archive"
        );
    }
    let sdk = root.join(".revenant/sdk");
    for (relative, bytes) in EMBEDDED {
        let path = sdk.join(relative);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, bytes).with_context(|| format!("materialize {}", path.display()))?;
    }
    // Preserve all shared dependency declarations used by the source crates.
    let workspace: toml::Value = toml::from_str(include_str!("../../Cargo.toml"))?;
    let mut table = workspace["workspace"].clone();
    table
        .as_table_mut()
        .context("workspace table")?
        .remove("exclude");
    let mut doc = toml::map::Map::new();
    doc.insert("workspace".into(), table);
    std::fs::write(sdk.join("Cargo.toml"), toml::to_string_pretty(&doc)?)?;
    Ok(())
}
