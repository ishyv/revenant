//! Minimal author files and the hidden framework-owned desktop bootstrap.
//!
//! The native library is detected from `native/Cargo.toml`; its package name may
//! differ from the application name. It must export `app() -> Application` and
//! depend on the same SDK source as the generated host.
mod assets;
pub mod sdk;
mod security;
pub mod templates;
mod zed;

use crate::{compiled::atomic_write, config::RevenantConfig};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Scaffold handwritten configuration and Svelte only, plus hidden SDK sources.
pub fn create_project(root: &Path, name: &str) -> Result<()> {
    std::fs::create_dir_all(root)?;
    sdk::materialize(root)?;
    for (relative, template) in templates::APP {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().context("template parent")?)?;
        let content = templates::render(
            template,
            &[
                ("project_name", name),
                ("identifier_name", &name.replace('_', "-")),
            ],
        );
        std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
    }
    std::fs::create_dir_all(root.join("web/src/lib"))?;
    // A real facade is published only after compiled manifest extraction succeeds.
    std::fs::write(root.join("web/.npmrc"), "install-links=true\n")?;
    prepare_host(root, &RevenantConfig::load(root)?)?;
    Ok(())
}

/// Generate only framework-owned files, preserving native source and frontend.
/// SDK snapshots already present in the app are preserved, including local edits.
pub fn prepare_host(root: &Path, config: &RevenantConfig) -> Result<PathBuf> {
    sdk::ensure_sources(root)?;
    // An installed client is a distinct snapshot, including npm's copied file
    // dependencies. Reject it before touching host files; first installs follow.
    sdk::check_installed_client(root, false)?;
    let icons = assets::resolve(root, config.desktop.icons.as_deref())?;
    let windows_icon = if icons.is_empty() {
        Some("icons/icon.ico".to_owned())
    } else {
        icons
            .iter()
            .find(|(path, _)| {
                Path::new(path)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("ico"))
            })
            .map(|(path, _)| path.clone())
    };
    let host = root.join(".revenant/desktop");
    std::fs::create_dir_all(host.join("src"))?;
    let native_manifest = root.join("native/Cargo.toml");
    let (application, dependency) = if native_manifest.is_file() {
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&native_manifest)?)?;
        let package = doc
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            .context("native/Cargo.toml needs [package] name")?;
        anyhow::ensure!(
            root.join("native/src/lib.rs").is_file()
                || doc
                    .get("lib")
                    .and_then(|lib| lib.get("path"))
                    .and_then(toml::Value::as_str)
                    .is_some_and(|p| root.join("native").join(p).is_file()),
            "native/Cargo.toml must define a library exporting pub fn app() -> revenant::Application"
        );
        (
            "revenant_app::app()",
            format!(
                "revenant-app = {{ package = {}, path = \"../../native\" }}",
                toml::Value::String(package.into())
            ),
        )
    } else {
        ("revenant::Application::new()", String::new())
    };
    let app_id = serde_json::to_string(&config.identifier())?;
    let agents_enabled = config.agents.enabled.to_string();
    let native_surfaces = config.desktop.native_surfaces.to_string();
    let desktop_features = if config.desktop.native_surfaces {
        "\"native-surfaces\""
    } else {
        ""
    };
    let fingerprint = crate::inspection::latest_build_source_fingerprint(root)?;
    let source_fingerprint = fingerprint
        .map(|value| serde_json::to_string(&value).map(|value| format!("Some({value}.into())")))
        .transpose()?
        .unwrap_or_else(|| "None".into());
    let build_attempt = crate::inspection::ProjectInspector::open(root)
        .ok()
        .and_then(|inspector| inspector.build_attempt().ok())
        .and_then(|attempt| attempt["id"].as_str().map(String::from))
        .map(|id| serde_json::to_string(&id).map(|id| format!("Some({id}.into())")))
        .transpose()?
        .unwrap_or_else(|| "None".into());
    let tokens = [
        ("project_name", config.project.name.as_str()),
        ("application_version", config.application_version()),
        ("application", application),
        ("native_dependency", dependency.as_str()),
        ("app_id", app_id.as_str()),
        ("agents_enabled", agents_enabled.as_str()),
        ("native_surfaces", native_surfaces.as_str()),
        ("desktop_features", desktop_features),
        ("source_fingerprint", source_fingerprint.as_str()),
        ("build_attempt", build_attempt.as_str()),
    ];
    for (relative, template) in [
        ("Cargo.toml", templates::HOST_CARGO),
        ("src/main.rs", templates::HOST_MAIN),
        ("build.rs", templates::HOST_BUILD),
    ] {
        write_if_changed(
            &host.join(relative),
            templates::render(template, &tokens).as_bytes(),
        )?;
    }
    for (relative, bytes) in templates::HOST_ASSETS {
        let path = host.join(relative);
        std::fs::create_dir_all(path.parent().context("host asset parent")?)?;
        write_if_changed(&path, bytes)?;
    }
    for (relative, bytes) in &icons {
        write_if_changed(&host.join(relative), bytes)?;
    }
    let conf = serde_json::json!({
        "$schema": "https://schema.tauri.app/config/2",
        "productName": config.project.name,
        "version": config.application_version(),
        "identifier": config.identifier(),
        "build": {
            "frontendDist": "../../build/web",
            "devUrl": format!("http://127.0.0.1:{}", config.desktop.dev_port)
        },
        "app": {
            "withGlobalTauri": true,
            "windows": [{"label": "main", "title": config.desktop.title.as_ref().unwrap_or(&config.project.name),
                "width": config.desktop.width, "height": config.desktop.height}],
            "security": {
                "csp": security::policy(config, false),
                "devCsp": security::policy(config, true),
                // Revenant's async confirmation uses the current message command.
                // Folder selection still runs through Revenant's scoped file API.
                "capabilities": [{
                    "identifier": "desktop-confirmation",
                    "windows": ["main"],
                    "permissions": ["dialog:allow-message", "core:event:allow-listen",
                        "core:event:allow-unlisten", "core:window:allow-destroy"]
                }]
            }
        },
        "bundle": {"active": true, "targets": if cfg!(windows) { serde_json::json!(["nsis"]) } else { serde_json::json!("all") },
            "icon": if icons.is_empty() { vec!["icons/icon.png".to_owned(), "icons/icon.ico".to_owned(), "icons/icon.icns".to_owned()] }
                else { icons.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>() },
            "windows": {"webviewInstallMode": {"type": "offlineInstaller"},
                "nsis": {"installerIcon": windows_icon, "uninstallerIcon": windows_icon}}}
    });
    write_if_changed(
        &host.join("tauri.conf.json"),
        &serde_json::to_vec_pretty(&conf)?,
    )?;
    // Editor configuration is additive and never required to compile an app.
    // Invalid user JSONC must not prevent native source recovery or publication.
    if let Err(error) = zed::prepare(root, native_manifest.is_file()) {
        eprintln!("Zed settings preserved; desktop discovery could not be updated: {error:#}");
    }
    Ok(host)
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).ok().as_deref() != Some(bytes) {
        atomic_write(path, bytes)?;
    }
    Ok(())
}
