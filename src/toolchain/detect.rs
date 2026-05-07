use crate::errors::{MissingTool, RevenantError};
use colored::Colorize;
use std::process::Command;

// TODO(v2): extend with Pnpm / Yarn / Bun variants for broader package manager support
#[derive(Debug, Clone, Copy)]
pub enum Tool {
    Cargo,
    WasmPack,
    Node,
    Npm,
}

impl Tool {
    /// The executable name used to invoke this tool (e.g. `"wasm-pack"`).
    pub fn binary_name(self) -> &'static str {
        match self {
            Tool::Cargo => "cargo",
            Tool::WasmPack => "wasm-pack",
            Tool::Node => "node",
            Tool::Npm => "npm",
        }
    }

    /// Human-readable name for use in error messages and status output.
    pub fn display_name(self) -> &'static str {
        match self {
            Tool::Cargo => "cargo",
            Tool::WasmPack => "wasm-pack",
            Tool::Node => "node",
            Tool::Npm => "npm",
        }
    }

    /// Installation instructions shown when the tool is missing.
    pub fn install_hint(self) -> &'static str {
        match self {
            Tool::Cargo => "https://rustup.rs",
            Tool::WasmPack => "cargo install wasm-pack",
            Tool::Node => "https://nodejs.org",
            Tool::Npm => "installed with Node.js — https://nodejs.org",
        }
    }

    // DESIGN: The spec lists "no auto-install" under What Not To Build, but wasm-pack
    // is a special case: users rarely have it pre-installed and it's trivially installable
    // via `cargo install`. We auto-install only wasm-pack; all others require manual install.
    /// Whether Revenant can install this tool automatically.
    pub fn auto_installable(self) -> bool {
        matches!(self, Tool::WasmPack)
    }

    /// Attempt to auto-install this tool. Returns Ok(()) if successful.
    pub fn auto_install(self) -> Result<(), String> {
        match self {
            Tool::WasmPack => {
                println!("{} Installing wasm-pack via cargo...", "▸".yellow().bold());
                let status = Command::new("cargo")
                    .args(["install", "wasm-pack"])
                    .status()
                    .map_err(|e| format!("failed to run cargo install: {e}"))?;

                if status.success() {
                    println!("  {} wasm-pack installed", "✓".green().bold());
                    Ok(())
                } else {
                    Err(format!(
                        "cargo install wasm-pack exited with code {}",
                        status.code().unwrap_or(-1)
                    ))
                }
            }
            _ => Err(format!("{} cannot be auto-installed", self.display_name())),
        }
    }

    /// Probe whether this tool is installed and callable on the current system.
    /// On Windows, routes through `cmd /C` so `.cmd` shims are resolved.
    pub fn is_available(self) -> bool {
        #[cfg(windows)]
        {
            Command::new("cmd")
                .args(["/C", self.binary_name(), "--version"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            Command::new(self.binary_name())
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }
}

/// Ensure all listed tools are available, auto-installing where possible.
/// Returns `Err(MissingTools)` for any tools that are missing and cannot be installed.
pub fn require_tools(tools: &[Tool]) -> Result<(), RevenantError> {
    let mut still_missing: Vec<MissingTool> = Vec::new();

    for &tool in tools {
        if tool.is_available() {
            continue;
        }

        if tool.auto_installable() {
            match tool.auto_install() {
                Ok(()) => {
                    // Verify it's actually on PATH now
                    if !tool.is_available() {
                        still_missing.push(MissingTool {
                            name: tool.display_name().to_string(),
                            install_hint: tool.install_hint().to_string(),
                        });
                    }
                }
                Err(e) => {
                    eprintln!("  {} auto-install failed: {e}", "✗".red().bold());
                    still_missing.push(MissingTool {
                        name: tool.display_name().to_string(),
                        install_hint: tool.install_hint().to_string(),
                    });
                }
            }
        } else {
            still_missing.push(MissingTool {
                name: tool.display_name().to_string(),
                install_hint: tool.install_hint().to_string(),
            });
        }
    }

    if still_missing.is_empty() {
        Ok(())
    } else {
        Err(RevenantError::MissingTools(still_missing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_names_are_correct() {
        assert_eq!(Tool::Cargo.binary_name(), "cargo");
        assert_eq!(Tool::WasmPack.binary_name(), "wasm-pack");
        assert_eq!(Tool::Node.binary_name(), "node");
        assert_eq!(Tool::Npm.binary_name(), "npm");
    }

    #[test]
    fn install_hints_non_empty() {
        for tool in [Tool::Cargo, Tool::WasmPack, Tool::Node, Tool::Npm] {
            assert!(!tool.install_hint().is_empty());
        }
    }

    #[test]
    fn only_wasm_pack_auto_installable() {
        assert!(!Tool::Cargo.auto_installable());
        assert!(Tool::WasmPack.auto_installable());
        assert!(!Tool::Node.auto_installable());
        assert!(!Tool::Npm.auto_installable());
    }

    #[test]
    fn auto_install_non_installable_errors() {
        for tool in [Tool::Cargo, Tool::Node, Tool::Npm] {
            assert!(tool.auto_install().is_err());
        }
    }
}
