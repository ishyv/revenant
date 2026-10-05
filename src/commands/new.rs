use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::config::{RevenantConfig, WEB_SUBDIR};
use crate::errors::RevenantError;
use crate::scaffold;
use crate::toolchain::detect::{Tool, require_tools};
use crate::toolchain::process;

/// Validate a project name: no slashes, spaces, or leading dots.
pub(crate) fn validate_name(name: &str) -> Result<(), RevenantError> {
    if name.is_empty() {
        return Err(RevenantError::InvalidProjectName {
            name: name.to_string(),
            reason: "name cannot be empty".to_string(),
        });
    }
    if name.starts_with('.') {
        return Err(RevenantError::InvalidProjectName {
            name: name.to_string(),
            reason: "name cannot start with a dot".to_string(),
        });
    }
    if name.contains('/') || name.contains('\\') {
        return Err(RevenantError::InvalidProjectName {
            name: name.to_string(),
            reason: "name cannot contain slashes".to_string(),
        });
    }
    if name.contains(' ') {
        return Err(RevenantError::InvalidProjectName {
            name: name.to_string(),
            reason: "name cannot contain spaces".to_string(),
        });
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || !name.starts_with(|c: char| c.is_ascii_alphabetic())
    {
        return Err(RevenantError::InvalidProjectName {
            name: name.into(),
            reason: "start with an ASCII letter and use letters, numbers, hyphens or underscores"
                .into(),
        });
    }
    if matches!(
        name.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        return Err(RevenantError::InvalidProjectName {
            name: name.into(),
            reason: "reserved Windows directory name".into(),
        });
    }
    Ok(())
}

/// Scaffold a new desktop project with no required native application source.
///
/// Validates the name, checks for required tools, writes the project template,
/// installs frontend tools, then compiles the hidden host and publishes its API.
pub fn run(name: &str, verbose: bool) -> Result<()> {
    validate_name(name)?;

    let target = Path::new(name);
    if target.exists() {
        return Err(RevenantError::DirectoryExists(target.to_path_buf()).into());
    }

    // Check all required tools before doing any work
    require_tools(&[Tool::Cargo, Tool::Node, Tool::Npm])?;

    println!(
        "\n{}",
        format!("Revenant — Creating project \"{name}\"").bold()
    );
    println!();

    scaffold::create_project(target, name)
        .with_context(|| format!("failed to scaffold project '{name}'"))?;
    println!("  {} Project scaffolded", "✓".green().bold());

    // Install web dependencies immediately so the project is ready to use
    let web_dir = target.join(WEB_SUBDIR);
    println!("  {} Installing web dependencies...", "▸".green().bold());
    process::run_blocking(&web_dir, "npm", &["install"], verbose)
        .with_context(|| format!("npm install failed in {}", web_dir.display()))?;
    println!("  {} Dependencies installed", "✓".green().bold());
    let config = RevenantConfig::load(target)?;
    super::build::build_rust(target, &config, false, verbose)
        .context("initial desktop contract build failed; scaffold is preserved")?;

    println!();
    println!("Ready. To start developing:");
    println!();
    println!("  cd {name}");
    println!("  revenant dev");
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::RevenantError;

    #[test]
    fn valid_names_pass() {
        for name in ["my-app", "hello_world", "app123", "a"] {
            assert!(validate_name(name).is_ok(), "expected '{name}' to be valid");
        }
    }

    #[test]
    fn empty_name_rejected() {
        let err = validate_name("").unwrap_err();
        assert!(matches!(err, RevenantError::InvalidProjectName { .. }));
    }

    #[test]
    fn leading_dot_rejected() {
        let err = validate_name(".hidden").unwrap_err();
        assert!(matches!(err, RevenantError::InvalidProjectName { .. }));
    }

    #[test]
    fn forward_slash_rejected() {
        let err = validate_name("a/b").unwrap_err();
        assert!(matches!(err, RevenantError::InvalidProjectName { .. }));
    }

    #[test]
    fn backslash_rejected() {
        let err = validate_name("a\\b").unwrap_err();
        assert!(matches!(err, RevenantError::InvalidProjectName { .. }));
    }

    #[test]
    fn spaces_rejected() {
        let err = validate_name("my app").unwrap_err();
        assert!(matches!(err, RevenantError::InvalidProjectName { .. }));
    }
}
