/// Handles verification and installation of packages.
/// In this context, "packages" are tools like "git", "node", "npm", etc.
/// These are external dependencies that the app relies on to function correctly.

use crate::consts::REQUIRED_PACKAGES;
use crate::data::AppContext;
use rayon::prelude::*;
use std::collections::HashMap;

pub fn is_installed(package: &str) -> bool {
    // Simple check: try to run `package --version` and see if it succeeds.
    match crate::commands::run(&format!("{} --version", package)) {
        Ok(_) => true,
        Err(_) => false,
    }
}


/// Will go through a list of required packages and verify if they are installed.
/// Fast path: verify in parallel. Slow path: install sequentially for safety.
pub fn verify_and_install_packages(app_context: &AppContext) {
    // 1) Parallel verification
    //
    // We evaluate `is_installed` for each package on a Rayon thread-pool.
    // This avoids serial process-spawn/IO latency and scales with core count.
    // We store results in a map so we can report in the original order later.
    let results: HashMap<&'static str, bool> = REQUIRED_PACKAGES
        .par_iter()
        .map(|&pkg| (pkg, is_installed(pkg))) // compute in parallel
        .collect();

    // 2) Report results in a stable, deterministic order and collect missing items.
    // Printing from many threads looks chaotic; we print from the main thread here.
    let mut missing: Vec<&'static str> = Vec::new();
    for &pkg in REQUIRED_PACKAGES.iter() {
        let present = results.get(pkg).copied().unwrap_or(false);
        if present {
            println!("[ + ] Found required package: {}", pkg);
        } else {
            // If auto-install is off, we exactly preserve your messaging.
            if !app_context.auto_install {
                eprintln!("[ - ] Missing required package: {}", pkg);
                eprintln!("\tPlease install it and try again.");
            }
            missing.push(pkg);
        }
    }

    // 3) Optional installation
    //
    // Installing multiple packages concurrently can corrupt package-manager state
    // (locked DBs, half-written caches) or trigger interactive prompts in stereo.
    // We therefore install sequentially. If you later want controlled parallelism,
    // introduce a bounded job queue with a small worker pool (2–3 workers max).
    if app_context.auto_install {
        for pkg in missing {
            // Keep your "todo" spirit but provide a sane place to plug it in.
            // Replace this call with your real installer (winget/brew/apt/etc.).
            match install_package(pkg, app_context) {
                Ok(()) => println!("[ + ] Installed package: {}", pkg),
                Err(err) => {
                    eprintln!("[ ! ] Failed to install {}: {}", pkg, err);
                    eprintln!("\tPlease install it manually and try again.");
                }
            }
        }
    }
}

/// Installs a missing package. Sequential by design.
/// Plug your own logic here (detect package manager, build command, call your `run()`).
fn install_package(pkg: &str, _ctx: &AppContext) -> std::io::Result<()> {
    // Placeholder to preserve your original “auto-install” intent without removing it.
    // Example sketch (uncomment and adapt to your environment):
    //
    // #[cfg(target_os = "windows")]
    // {
    //     // Prefer winget if available; fall back to scoop/choco if that’s your world.
    //     if is_installed("winget") {
    //         let cmd = format!(r#"winget install --id {} --accept-source-agreements --accept-package-agreements"#, pkg);
    //         return run(&cmd).map(|_| ());
    //     }
    // }
    // #[cfg(target_os = "macos")]
    // {
    //     if is_installed("brew") {
    //         let cmd = format!(r#"brew install {}"#, pkg);
    //         return run(&cmd).map(|_| ());
    //     }
    // }
    // #[cfg(target_os = "linux")]
    // {
    //     if is_installed("apt") {
        //     let cmd = format!(r#"sudo apt-get update && sudo apt-get install -y {}"#, pkg);
        //     return run(&cmd).map(|_| ());
        // }
    //     // Add dnf/pacman/etc. as needed.
    // }
    //
    // For now, signal unimplemented to match your earlier `todo!` intent.
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("auto-install not implemented for '{}'", pkg),
    ))
}
