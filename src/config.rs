use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const REVENANT_CONFIG_FILE: &str = "revenant.toml";
pub const RUST_SUBDIR: &str = "rust";
pub const WEB_SUBDIR: &str = "web";
pub const WASM_OUT_DIR: &str = "pkg";
pub const WASM_PACK_TARGET: &str = "bundler";
pub const DEBOUNCE_MS: u64 = 500;
pub const SHUTDOWN_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Deserialize)]
pub struct RevenantConfig {
    pub project: ProjectConfig,
    pub toolchain: ToolchainConfig,
}

#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
}

// TODO(v2): add optional fields for wasi_target, custom watch paths, build flags
#[derive(Debug, Deserialize)]
pub struct ToolchainConfig {
    pub wasm_target: String,
    pub pkg_manager: String,
}

impl RevenantConfig {
    /// Load and parse `revenant.toml` from the given project root directory.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(REVENANT_CONFIG_FILE);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let config: RevenantConfig = toml::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(config)
    }
}

/// Walk up from `start` looking for `revenant.toml`. Returns the directory containing it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_valid_config(dir: &Path) {
        let content = r#"
[project]
name = "test-project"

[toolchain]
wasm_target = "bundler"
pkg_manager = "npm"
"#;
        std::fs::write(dir.join(REVENANT_CONFIG_FILE), content).unwrap();
    }

    #[test]
    fn load_valid_config() {
        let tmp = TempDir::new().unwrap();
        write_valid_config(tmp.path());
        let config = RevenantConfig::load(tmp.path()).unwrap();
        assert_eq!(config.project.name, "test-project");
        assert_eq!(config.toolchain.wasm_target, "bundler");
        assert_eq!(config.toolchain.pkg_manager, "npm");
    }

    #[test]
    fn load_missing_file_errors() {
        let tmp = TempDir::new().unwrap();
        let err = RevenantConfig::load(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("failed to read"));
    }

    #[test]
    fn load_invalid_toml_errors() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(REVENANT_CONFIG_FILE), "not valid toml {{{}").unwrap();
        let err = RevenantConfig::load(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("failed to parse"));
    }

    #[test]
    fn find_project_root_in_current_dir() {
        let tmp = TempDir::new().unwrap();
        write_valid_config(tmp.path());
        assert_eq!(find_project_root(tmp.path()), Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_project_root_nested() {
        let tmp = TempDir::new().unwrap();
        write_valid_config(tmp.path());
        let nested = tmp.path().join("sub").join("dir");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_project_root(&nested), Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_project_root_not_found() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(find_project_root(tmp.path()), None);
    }
}
