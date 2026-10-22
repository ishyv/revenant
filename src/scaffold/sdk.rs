//! SDK compatibility and recoverable, application-local replacement.
//!
//! Compatibility revisions describe the host/client contract independently of
//! the framework package version. Compatible source edits are retained.
mod client;
mod recovery;
use anyhow::{Context, Result, ensure};
pub use recovery::{restore, upgrade};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

include!(concat!(env!("OUT_DIR"), "/sdk_sources.rs"));

/// Host/client compatibility revision, independent of package version 0.4.0.
pub const COMPATIBILITY_REVISION: u32 = revenant_core::SDK_COMPATIBILITY_REVISION;

#[derive(Serialize, Deserialize)]
struct Compatibility {
    sdk: u32,
    client: u32,
    manifest: u32,
    protocol: u32,
}

/// Materialize a self-contained source workspace. The destination must be absent.
pub fn materialize(root: &Path) -> Result<()> {
    let sdk = root.join(".revenant/sdk");
    ensure!(
        absent(&sdk)?,
        "SDK already exists; use `revenant sdk upgrade` to preserve it before replacement"
    );
    if root.join(".revenant").try_exists()? {
        ensure!(
            dunce::canonicalize(root.join(".revenant"))?.starts_with(dunce::canonicalize(root)?),
            "SDK parent escapes application root"
        );
    }
    for required in [
        "crates/revenant-core/Cargo.toml",
        "crates/revenant-sdk/Cargo.toml",
        "crates/revenant-macros/Cargo.toml",
        "crates/revenant-desktop/Cargo.toml",
        "crates/revenant-agent/Cargo.toml",
        "crates/revenant-graphics/Cargo.toml",
        "crates/revenant-graphics/build.rs",
        "crates/revenant-graphics/src/compiler.rs",
        "crates/revenant-graphics/src/compiler_bundle.rs",
        "crates/revenant-graphics/src/scene.wgsl",
        "crates/revenant-graphics/src/gpu_shared.rs",
        "crates/revenant-graphics/src/atlas_build.rs",
        "crates/revenant-graphics/src/compute.rs",
        "crates/revenant-core/src/tasks/services.rs",
        "crates/revenant-core/src/latest.rs",
        "crates/revenant-core/src/compute.rs",
        "crates/revenant-core/src/compute/fixtures.rs",
        "crates/revenant-core/src/projection.rs",
        "crates/revenant-sdk/src/compute.rs",
        "crates/revenant-macros/src/projection.rs",
        "crates/revenant-desktop/src/work.rs",
        "crates/revenant-graphics/assets/AtkinsonHyperlegible-Regular.ttf",
        "crates/revenant-graphics/assets/OFL.txt",
        "crates/revenant-graphics/assets/dxc/LICENCE-MIT.txt",
        "crates/revenant-graphics/assets/dxc/LICENSE-LLVM.txt",
        "crates/revenant-graphics/assets/dxc/LICENSE-MS.txt",
        "crates/revenant-graphics/assets/dxc/PROVENANCE.md",
        "packages/client/package.json",
        "packages/client/vite.js",
        "packages/client/vite.d.ts",
        "packages/client/src/query-group.ts",
        "packages/client/src/overlay.ts",
        "packages/client/src/ui/CoveredModal.svelte",
        "vendor/hyvui-1.0.0.tgz",
    ] {
        ensure!(
            EMBEDDED.iter().any(|(p, _)| *p == required),
            "CLI was built without {required}; rebuild Revenant with the complete SDK and vendor archive"
        );
    }
    for (relative, bytes) in EMBEDDED {
        let path = sdk.join(relative);
        fs::create_dir_all(path.parent().context("SDK source parent")?)?;
        fs::write(&path, bytes).with_context(|| format!("materialize {}", path.display()))?;
    }
    let workspace: toml::Value = toml::from_str(include_str!("../../Cargo.toml"))?;
    let mut table = workspace["workspace"].clone();
    table
        .as_table_mut()
        .context("workspace table")?
        .remove("exclude");
    let mut doc = toml::map::Map::new();
    doc.insert("workspace".into(), table);
    fs::write(sdk.join("Cargo.toml"), toml::to_string_pretty(&doc)?)?;
    fs::write(
        sdk.join("compatibility.json"),
        serde_json::to_vec_pretty(&Compatibility {
            sdk: COMPATIBILITY_REVISION,
            client: COMPATIBILITY_REVISION,
            manifest: revenant_core::CONTRACT_MANIFEST_VERSION,
            protocol: revenant_core::BRIDGE_PROTOCOL_VERSION,
        })?,
    )?;
    Ok(())
}

/// Create only a wholly absent SDK, then check sources without overwriting edits.
pub fn ensure_sources(root: &Path) -> Result<()> {
    if absent(&root.join(".revenant/sdk"))? {
        materialize(root)?;
    }
    check_sources(root)
}

/// Check compatibility metadata, package versions, complete sources and client API.
/// This is read-only and never repairs a partial snapshot automatically.
pub fn check_sources(root: &Path) -> Result<()> {
    let sdk = root.join(".revenant/sdk");
    let result = (|| -> Result<()> {
        let canonical = dunce::canonicalize(&sdk)?;
        ensure!(
            canonical.starts_with(dunce::canonicalize(root)?),
            "SDK source snapshot escapes application root"
        );
        let compatibility: Compatibility =
            serde_json::from_slice(&fs::read(sdk.join("compatibility.json"))?)?;
        ensure!(
            compatibility.sdk == COMPATIBILITY_REVISION
                && compatibility.client == COMPATIBILITY_REVISION
                && compatibility.manifest == revenant_core::CONTRACT_MANIFEST_VERSION
                && compatibility.protocol == revenant_core::BRIDGE_PROTOCOL_VERSION,
            "unsupported SDK compatibility revision"
        );
        ensure!(
            sdk.join("Cargo.toml").is_file(),
            "missing SDK workspace manifest"
        );
        for (relative, _) in EMBEDDED {
            let path = sdk.join(relative);
            ensure!(path.is_file(), "partial SDK: missing {relative}");
            ensure!(
                dunce::canonicalize(path)?.starts_with(&canonical),
                "SDK source escapes snapshot: {relative}"
            );
        }
        for name in [
            "revenant-core",
            "revenant-sdk",
            "revenant-macros",
            "revenant-desktop",
            "revenant-agent",
            "revenant-graphics",
        ] {
            let doc: toml::Value = toml::from_str(&fs::read_to_string(
                sdk.join(format!("crates/{name}/Cargo.toml")),
            )?)?;
            ensure!(
                doc.get("package")
                    .and_then(|p| p.get("version"))
                    .and_then(toml::Value::as_str)
                    == Some("0.4.0"),
                "SDK snapshot predates desktop 0.4 or has an unsupported {name} version"
            );
        }
        client::check(&sdk.join("packages/client"))
    })();
    result.context("SDK sources are incompatible or incomplete; run `revenant sdk upgrade` (existing source edits and contracts will be backed up)")
}

/// Validate the installed client independently of the SDK source package.
/// Missing dependencies may be allowed before the first frontend installation.
pub fn check_installed_client(root: &Path, required: bool) -> Result<()> {
    let client = root.join("web/node_modules/@revenant/client");
    if !client.try_exists()? && !required {
        return Ok(());
    }
    client::check(&client).context("installed @revenant/client is incompatible or incomplete; run `revenant sdk upgrade` or reinstall frontend dependencies; existing contracts are preserved")
}

/// Check both SDK sources and an installed client without writing any files.
pub fn check(root: &Path) -> Result<()> {
    check_sources(root)?;
    check_installed_client(root, true)
}

fn absent(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::TempDir;
    #[test]
    fn native_capability_snapshot_is_complete_and_has_current_compatibility() {
        let temp = TempDir::new().unwrap();
        materialize(temp.path()).unwrap();
        let sdk = temp.path().join(".revenant/sdk");
        let metadata: Value =
            serde_json::from_slice(&fs::read(sdk.join("compatibility.json")).unwrap()).unwrap();
        assert_eq!(
            metadata,
            serde_json::json!({"sdk":4,"client":4,"manifest":6,"protocol":5})
        );
        for asset in [
            "Cargo.toml",
            "src/lib.rs",
            "src/gpu.rs",
            "src/scene.wgsl",
            "assets/AtkinsonHyperlegible-Regular.ttf",
            "assets/OFL.txt",
            "build.rs",
            "src/compiler.rs",
            "src/compiler_bundle.rs",
            "assets/dxc/LICENCE-MIT.txt",
            "assets/dxc/LICENSE-LLVM.txt",
            "assets/dxc/LICENSE-MS.txt",
            "assets/dxc/PROVENANCE.md",
        ] {
            assert!(
                sdk.join("crates/revenant-graphics").join(asset).is_file(),
                "Missing graphics source asset {asset}"
            );
        }
        let workspace: toml::Value =
            toml::from_str(&fs::read_to_string(sdk.join("Cargo.toml")).unwrap()).unwrap();
        assert!(
            workspace["workspace"]["members"]
                .as_array()
                .unwrap()
                .contains(&toml::Value::String("crates/revenant-graphics".into()))
        );
        check_sources(temp.path()).unwrap();
    }

    #[test]
    fn older_capability_contract_is_rejected_without_replacing_local_sources() {
        let temp = TempDir::new().unwrap();
        materialize(temp.path()).unwrap();
        let sdk = temp.path().join(".revenant/sdk");
        let authored = sdk.join("crates/revenant-sdk/src/lib.rs");
        let saved = b"// A compatible local edit must survive a rejected snapshot check.\n";
        fs::write(&authored, saved).unwrap();
        fs::write(
            sdk.join("compatibility.json"),
            br#"{"sdk":3,"client":3,"manifest":5,"protocol":4}"#,
        )
        .unwrap();
        assert!(ensure_sources(temp.path()).is_err());
        assert_eq!(fs::read(authored).unwrap(), saved);
    }
    pub(super) fn install(root: &Path) {
        fn copy(source: &Path, destination: &Path) {
            fs::create_dir_all(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let target = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        copy(
            &root.join(".revenant/sdk/packages/client"),
            &root.join("web/node_modules/@revenant/client"),
        );
    }
    #[test]
    fn partial_snapshot_is_not_repaired_and_compatible_edits_survive() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        materialize(root).unwrap();
        let edited = root.join(".revenant/sdk/crates/revenant-sdk/src/lib.rs");
        let original = fs::read_to_string(&edited).unwrap();
        let modified = format!("{original}\n// compatible local edit\n");
        fs::write(&edited, &modified).unwrap();
        ensure_sources(root).unwrap();
        assert_eq!(fs::read_to_string(&edited).unwrap(), modified);
        fs::remove_file(root.join(".revenant/sdk/Cargo.toml")).unwrap();
        assert!(ensure_sources(root).is_err());
        assert!(!root.join(".revenant/sdk/Cargo.toml").exists());
    }
    #[test]
    fn same_version_missing_export_and_stale_installation_fail_independently() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        materialize(root).unwrap();
        install(root);
        check(root).unwrap();
        let entry = root.join("web/node_modules/@revenant/client/src/svelte.ts");
        fs::write(&entry, "// export function setupApp() {}\nexport function provideApp() {}\nexport function useApp() {}\n").unwrap();
        check_sources(root).unwrap();
        assert!(
            format!("{:#}", check_installed_client(root, true).unwrap_err()).contains("setupApp")
        );
        install(root);
        let package = root.join("web/node_modules/@revenant/client/package.json");
        let mut metadata: Value = serde_json::from_slice(&fs::read(&package).unwrap()).unwrap();
        metadata["revenant"]["compatibility"] = serde_json::json!(0);
        fs::write(package, serde_json::to_vec(&metadata).unwrap()).unwrap();
        check_sources(root).unwrap();
        assert!(check_installed_client(root, true).is_err());
    }

    #[test]
    fn incompatible_sources_or_installed_client_preserve_generated_host() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("compatible-app");
        crate::scaffold::create_project(&root, "compatible-app").unwrap();
        install(&root);
        let config = crate::config::RevenantConfig::load(&root).unwrap();
        let host = root.join(".revenant/desktop/src/main.rs");
        fs::write(&host, "last successful host").unwrap();
        let client_entry = root.join("web/node_modules/@revenant/client/src/svelte.ts");
        fs::write(client_entry, "export function useApp() {}\n").unwrap();
        assert!(crate::scaffold::prepare_host(&root, &config).is_err());
        assert_eq!(fs::read_to_string(&host).unwrap(), "last successful host");
        install(&root);
        fs::write(
            root.join(".revenant/sdk/packages/client/src/svelte.ts"),
            "export function useApp() {}\n",
        )
        .unwrap();
        assert!(crate::scaffold::prepare_host(&root, &config).is_err());
        assert_eq!(fs::read_to_string(host).unwrap(), "last successful host");
    }
}
