use anyhow::Result;
use colored::Colorize;
use std::io::{self, Write};

use crate::toolchain::detect::Tool;

/// Interactive guided setup: check each required tool and provide OS-specific
/// install instructions for any that are missing.
pub fn run() -> Result<()> {
    println!("\n{}", "Revenant — Setup".bold());
    println!();
    println!("  Checking required tools...");
    println!();

    let tools = [Tool::Cargo, Tool::WasmPack, Tool::Node, Tool::Npm];
    let mut all_ok = true;

    for tool in &tools {
        if tool.is_available() {
            println!("  {} {} found", "✓".green().bold(), tool.display_name());
        } else {
            all_ok = false;
            println!("  {} {} not found", "✗".red().bold(), tool.display_name());
            println!();
            print_install_instructions(*tool);
            println!();

            wait_for_user("  Press Enter after installing, or Ctrl+C to quit...")?;
            println!();

            // Re-check
            if tool.is_available() {
                println!(
                    "  {} {} now available",
                    "✓".green().bold(),
                    tool.display_name()
                );
            } else {
                println!(
                    "  {} {} still not found — continuing anyway",
                    "✗".yellow().bold(),
                    tool.display_name()
                );
            }
        }
    }

    println!();
    if all_ok {
        println!("  {} All tools ready!", "✓".green().bold());
        println!();
        println!("  Create a project with:");
        println!("    revenant new my-app");
    } else {
        // Re-check everything at the end
        let still_missing: Vec<_> = tools.iter().filter(|t| !t.is_available()).collect();
        if still_missing.is_empty() {
            println!("  {} All tools ready!", "✓".green().bold());
            println!();
            println!("  Create a project with:");
            println!("    revenant new my-app");
        } else {
            println!(
                "  {} Some tools are still missing. Install them and run 'revenant setup' again.",
                "▸".yellow().bold()
            );
        }
    }
    println!();

    Ok(())
}

fn wait_for_user(prompt: &str) -> Result<()> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    Ok(())
}

fn print_install_instructions(tool: Tool) {
    let os = std::env::consts::OS;
    match (tool, os) {
        (Tool::Cargo, "windows") => {
            println!("    Install Rust:");
            println!("      Download from https://rustup.rs");
            println!("      Or run: winget install Rustlang.Rustup");
        }
        (Tool::Cargo, "macos") => {
            println!("    Install Rust:");
            println!("      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh");
            println!("      Or: brew install rustup && rustup-init");
        }
        (Tool::Cargo, _) => {
            println!("    Install Rust:");
            println!("      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh");
        }
        (Tool::WasmPack, _) => {
            println!("    Install wasm-pack:");
            println!("      cargo install wasm-pack");
            println!("      (or skip this — 'revenant new'/'revenant dev' auto-install it for you)");
        }
        (Tool::Node | Tool::Npm, "windows") => {
            println!("    Install Node.js (includes npm):");
            println!("      Download from https://nodejs.org");
            println!("      Or run: winget install OpenJS.NodeJS.LTS");
        }
        (Tool::Node | Tool::Npm, "macos") => {
            println!("    Install Node.js (includes npm):");
            println!("      Download from https://nodejs.org");
            println!("      Or: brew install node");
        }
        (Tool::Node | Tool::Npm, _) => {
            println!("    Install Node.js (includes npm):");
            println!("      https://nodejs.org");
            println!(
                "      Or via your package manager (apt install nodejs, pacman -S nodejs, etc.)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::toolchain::detect::Tool;

    #[test]
    fn cargo_is_available_in_test_env() {
        // cargo is always available when running cargo test
        assert!(Tool::Cargo.is_available());
    }
}
