//! Handwritten desktop configuration. Version 3 is an explicit migration boundary.
//!
//! A minimal app needs only `version = 3` and `[project] name = "my-app"`.
//! The frontend lives in `web/`; a `native/Cargo.toml` library is optional. Old
//! WASM targets and server sections are rejected rather than silently ignored.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Author-owned application configuration, located by walking up from cwd.
pub const REVENANT_CONFIG_FILE: &str = "revenant.toml";
/// Optional author-owned Rust application library.
pub const NATIVE_SUBDIR: &str = "native";
/// Author-owned SvelteKit frontend.
pub const WEB_SUBDIR: &str = "web";
/// Trailing debounce for native source and configuration edits.
pub const DEBOUNCE_MS: u64 = 500;
/// Maximum graceful process-group shutdown time on Unix.
pub const SHUTDOWN_TIMEOUT_SECS: u64 = 5;

/// Version 3 application configuration. Unknown keys are errors.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RevenantConfig {
    pub version: u32,
    pub project: ProjectConfig,
    #[serde(default)]
    pub toolchain: ToolchainConfig,
    #[serde(default)]
    pub desktop: DesktopConfig,
}

/// Application identity. `identifier` defaults to `app.revenant.<name>`.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
    pub identifier: Option<String>,
}

/// Package manager used for installation, Vite, and Tauri CLI packaging.
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct ToolchainConfig {
    pub pkg_manager: String,
}
impl Default for ToolchainConfig {
    fn default() -> Self {
        Self {
            pkg_manager: "npm".into(),
        }
    }
}

/// Native window defaults and the local frontend address.
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct DesktopConfig {
    pub title: Option<String>,
    pub width: u32,
    pub height: u32,
    pub dev_port: u16,
}
impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            title: None,
            width: 1100,
            height: 760,
            dev_port: 5173,
        }
    }
}

impl RevenantConfig {
    /// Parse and validate before generating files or starting child processes.
    /// Version errors include instructions even for unversioned applications.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(REVENANT_CONFIG_FILE);
        let content =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let doc: toml::Value =
            toml::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
        anyhow::ensure!(
            doc.get("version").and_then(toml::Value::as_integer) == Some(3),
            "Revenant 0.3 requires version = 3 in revenant.toml. Version 2 and unversioned WASM/server apps are unsupported. Create a desktop scaffold with `revenant new <name>`, copy your Svelte UI into web/, and move Rust operations into an optional native/ library exporting `pub fn app() -> revenant::Application`. Remove wasm_target, app.server and server sections; changing the version alone is not a migration."
        );
        let config: Self =
            toml::from_str(&content).context("parse version 3 desktop configuration")?;
        super::commands::new::validate_name(&config.project.name)?;
        anyhow::ensure!(
            matches!(
                config.toolchain.pkg_manager.as_str(),
                "npm" | "pnpm" | "yarn" | "bun"
            ),
            "pkg_manager must be npm, pnpm, yarn or bun"
        );
        anyhow::ensure!(
            config.desktop.width > 0 && config.desktop.height > 0 && config.desktop.dev_port > 0,
            "desktop width, height and dev_port must be positive"
        );
        let identifier = config.identifier();
        anyhow::ensure!(
            identifier.split('.').count() >= 2
                && identifier.split('.').all(|part| !part.is_empty()
                    && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')),
            "project.identifier must be a reverse-domain identifier using letters, numbers, hyphens and dots"
        );
        Ok(config)
    }

    /// Stable Tauri bundle identity; set explicitly before distributing an app.
    pub fn identifier(&self) -> String {
        self.project
            .identifier
            .clone()
            .unwrap_or_else(|| format!("app.revenant.{}", self.project.name.replace('_', "-")))
    }
}

/// Find the nearest author-owned config without assuming invocation from root.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(REVENANT_CONFIG_FILE).is_file() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}
