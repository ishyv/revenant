// TODO(v2): consider a Vite plugin that triggers wasm-pack directly, removing
// the need for our own file watcher

use anyhow::{Context, Result};
use colored::Colorize;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::bindings;
use crate::config::{self, RevenantConfig, WEB_SUBDIR};
use crate::errors::RevenantError;
use crate::toolchain::process::{self, LogLine, ManagedProcess};
use crate::toolchain::wasm::{self, WasmBuilder, WasmPackBuilder};

/// Start the development server: initial WASM build, file watcher on `rust/src/`,
/// and the web dev server. Runs until Ctrl+C or the web process exits.
pub fn run(verbose: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let root = config::find_project_root(&cwd).ok_or(RevenantError::ProjectNotFound)?;
    let config = RevenantConfig::load(&root)?;

    println!("\n{}", "Revenant — Development server".bold());
    println!();

    let web_dir = root.join(WEB_SUBDIR);

    // Install web deps if node_modules is absent
    if !web_dir.join("node_modules").exists() {
        println!(
            "  {} node_modules not found — running npm install...",
            "▸".yellow().bold()
        );
        process::run_blocking(&web_dir, "npm", &["install"], verbose)
            .with_context(|| format!("npm install failed in {}", web_dir.display()))?;
        println!("  {} Dependencies installed", "✓".green().bold());
    }

    // Shutdown channel (ctrlc → main loop)
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
    ctrlc::set_handler(move || {
        let _ = shutdown_tx.send(());
    })
    .context("failed to set Ctrl+C handler")?;

    // Step 1: Initial blocking WASM dev build
    println!("  {} Running initial WASM build...", "▸".cyan().bold());
    let builder = WasmPackBuilder::new(&root, &config.toolchain.wasm_target, verbose);
    builder.build_dev()?;
    bindings::sync_bindings(&root, &config.project.name)
        .context("failed to generate Revenant bindings after the initial WASM build")?;
    println!("  {} WASM build complete", "✓".green().bold());

    // Step 2: Start file watcher for rust/src/
    let (rebuild_tx, rebuild_rx) = mpsc::channel::<()>();
    let _watcher =
        wasm::WasmWatcher::start(&root, rebuild_tx).context("failed to start file watcher")?;

    // Step 3: Spawn npm run dev
    let (log_tx, log_rx) = mpsc::channel::<LogLine>();
    let mut web_proc = ManagedProcess::spawn(
        &web_dir,
        &config.toolchain.pkg_manager,
        &["run", "dev"],
        "web",
        log_tx.clone(),
    )?;

    println!(
        "{} Dev server started — watching for changes",
        "▸".cyan().bold()
    );

    // Track in-flight wasm-pack rebuild process
    let mut wasm_proc: Option<ManagedProcess> = None;
    let loop_start = Instant::now();

    // Event loop
    loop {
        // Drain log lines
        while let Ok(log) = log_rx.try_recv() {
            let prefix = if log.prefix == "wasm" {
                format!("[{}]", log.prefix).yellow().to_string()
            } else {
                format!("[{}]", log.prefix).cyan().to_string()
            };

            if log.is_stderr {
                eprintln!("{} {}", prefix, log.line);
            } else {
                println!("{} {}", prefix, log.line);
            }
        }

        // Check shutdown signal
        if shutdown_rx.try_recv().is_ok() {
            println!();
            println!("{} Shutting down...", "▸".yellow().bold());
            break;
        }

        // Check if web process exited
        if let Ok(Some(status)) = web_proc.try_wait() {
            // Drain remaining buffered log lines before reporting
            std::thread::sleep(Duration::from_millis(50));
            let mut stderr_lines = Vec::new();
            while let Ok(log) = log_rx.try_recv() {
                let prefix = format!("[{}]", log.prefix).red().to_string();
                eprintln!("{} {}", prefix, log.line);
                if log.is_stderr {
                    stderr_lines.push(log.line);
                }
            }

            if !status.success() {
                // Detect port-in-use if the server crashed quickly
                if loop_start.elapsed() < Duration::from_secs(3) {
                    let port_conflict = stderr_lines
                        .iter()
                        .any(|l| l.contains("EADDRINUSE") || l.contains("address already in use"));
                    if port_conflict {
                        return Err(RevenantError::DevServerCrashed.into());
                    }
                }

                let code = status.code().unwrap_or(-1);
                return Err(anyhow::anyhow!(
                    "dev server exited with code {code} — check output above"
                ));
            }
            break;
        }

        // Check if wasm rebuild finished
        if let Some(ref mut proc) = wasm_proc
            && let Ok(Some(status)) = proc.try_wait()
        {
            if status.success() {
                if let Err(err) = bindings::sync_bindings(&root, &config.project.name) {
                    eprintln!(
                        "  {} Failed to regenerate bindings: {err}",
                        "✗".red().bold()
                    );
                }
                println!("  {} WASM rebuild complete", "✓".green().bold());
            } else {
                eprintln!("  {} WASM rebuild failed", "✗".red().bold());
            }
            wasm_proc = None;
        }

        // Check for rebuild requests
        if rebuild_rx.try_recv().is_ok() {
            if let Some(ref mut proc) = wasm_proc {
                proc.terminate_with_timeout();
            }

            println!(
                "{} Rust source changed — rebuilding WASM...",
                "▸".yellow().bold()
            );

            match wasm::spawn_wasm_build(&root, &config.toolchain.wasm_target, log_tx.clone()) {
                Ok(proc) => wasm_proc = Some(proc),
                Err(e) => eprintln!("  {} Failed to start WASM rebuild: {e}", "✗".red().bold()),
            }
        }

        // NOTE: 100ms tick keeps CPU usage negligible while still feeling responsive
        // for log output and rebuild triggers. Lower wastes cycles; higher adds latency.
        std::thread::sleep(Duration::from_millis(100));
    }

    // Teardown: kill processes in reverse order
    if let Some(ref mut proc) = wasm_proc {
        proc.terminate_with_timeout();
    }
    web_proc.terminate_with_timeout();

    println!("  {} All processes stopped", "✓".green().bold());
    Ok(())
}
