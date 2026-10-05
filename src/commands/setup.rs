//! Read-only desktop prerequisites; installing dependencies is always explicit.
use crate::toolchain::{
    detect::{Tool, require_tools},
    process,
};
use anyhow::Result;
use colored::Colorize;

/// Diagnose compiler, frontend tools and Tauri's native platform requirements.
pub fn run() -> Result<()> {
    println!("\n{}", "Revenant — Desktop toolchain diagnosis".bold());
    let tools = [Tool::Cargo, Tool::Node, Tool::Npm];
    for tool in tools {
        if tool.is_available() {
            println!("  {} {} found", "✓".green(), tool.display_name());
        } else {
            println!(
                "  {} {} missing; install explicitly: {}",
                "✗".red(),
                tool.display_name(),
                tool.install_hint()
            );
        }
    }
    require_tools(&tools)?;
    let cwd = std::env::current_dir()?;
    let compiler = process::run_capture(&cwd, "rustc", &["-vV"], false)?;
    println!("{compiler}");
    println!("  Platform prerequisites: https://v2.tauri.app/start/prerequisites/");
    #[cfg(windows)]
    {
        anyhow::ensure!(
            compiler
                .lines()
                .any(|line| line.starts_with("host:") && line.ends_with("-pc-windows-msvc")),
            "Windows desktop packaging requires the MSVC Rust toolchain. Install Visual Studio C++ Build Tools and select a *-pc-windows-msvc toolchain."
        );
        let webview = [
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients",
            r"HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients",
            r"HKCU\SOFTWARE\Microsoft\EdgeUpdate\Clients",
        ]
        .iter()
        .any(|key| {
            process::run_capture(
                &cwd,
                "reg.exe",
                &[
                    "query",
                    key,
                    "/s",
                    "/f",
                    "Microsoft Edge WebView2 Runtime",
                    "/d",
                ],
                false,
            )
            .is_ok()
        });
        println!(
            "  WebView2 registration: {}",
            if webview {
                "found"
            } else {
                "not found; install the runtime for local development"
            }
        );
        let locator = std::env::var_os("ProgramFiles(x86)")
            .map(std::path::PathBuf::from)
            .map(|base| base.join("Microsoft Visual Studio/Installer/vswhere.exe"));
        let cpp = locator
            .filter(|path| path.is_file())
            .and_then(|path| {
                process::run_capture(
                    &cwd,
                    &path.to_string_lossy(),
                    &[
                        "-latest",
                        "-products",
                        "*",
                        "-requires",
                        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                        "-property",
                        "installationPath",
                    ],
                    false,
                )
                .ok()
            })
            .is_some_and(|path| !path.trim().is_empty())
            || process::run_capture(&cwd, "where.exe", &["cl.exe"], false).is_ok();
        println!(
            "  MSVC C++ tools: {}",
            if cpp {
                "installation found"
            } else {
                "not found"
            }
        );
        println!("  Native compilation verifies Windows SDK headers and linker availability.");
        anyhow::ensure!(
            cpp && webview,
            "desktop development prerequisites are incomplete: install Visual Studio C++ Build Tools with a Windows SDK and the WebView2 runtime; see https://v2.tauri.app/start/prerequisites/"
        );
    }
    #[cfg(target_os = "linux")]
    process::run_blocking(
        &cwd,
        "pkg-config",
        &["--exists", "webkit2gtk-4.1", "gtk+-3.0"],
        false,
    )?;
    #[cfg(target_os = "macos")]
    process::run_blocking(&cwd, "xcode-select", &["-p"], false)?;
    println!(
        "  Tauri CLI 2 is installed as an application devDependency during revenant new/dev/build."
    );
    Ok(())
}
