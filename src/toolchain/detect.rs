//! Toolchain diagnostics for desktop development. Probes never install tools.
use crate::errors::{MissingTool, RevenantError};

/// Native compiler and frontend tools required by the desktop CLI.
#[derive(Debug, Clone, Copy)]
pub enum Tool {
    Cargo,
    Node,
    Npm,
}
impl Tool {
    /// Program resolved by the lifecycle-aware process runner.
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Node => "node",
            Self::Npm => "npm",
        }
    }
    /// Display name used in actionable diagnostics.
    pub fn display_name(self) -> &'static str {
        self.binary_name()
    }
    /// Explicit installation instructions; desktop platform prerequisites are separate.
    pub fn install_hint(self) -> &'static str {
        match self {
            Self::Cargo => "https://rustup.rs",
            Self::Node => "https://nodejs.org",
            Self::Npm => "installed with Node.js: https://nodejs.org",
        }
    }
    /// Availability only: uses managed child processes and the Windows shim route.
    pub fn is_available(self) -> bool {
        let cwd = std::env::current_dir().unwrap_or_default();
        crate::toolchain::process::run_capture(&cwd, self.binary_name(), &["--version"], false)
            .is_ok()
    }
}
/// Diagnose all missing tools in a single error without machine changes.
pub fn require_tools(tools: &[Tool]) -> Result<(), RevenantError> {
    let missing: Vec<_> = tools
        .iter()
        .filter(|tool| !tool.is_available())
        .map(|tool| MissingTool {
            name: tool.display_name().into(),
            install_hint: tool.install_hint().into(),
        })
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(RevenantError::MissingTools(missing))
    }
}
