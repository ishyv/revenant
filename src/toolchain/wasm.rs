use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::config::{DEBOUNCE_MS, RUST_SUBDIR, WASM_OUT_DIR};
use crate::errors::RevenantError;
use crate::toolchain::process;

// TODO(v2): add a WasiBuilder variant for server-side WASM targets

/// Abstraction over the WASM build backend.
pub trait WasmBuilder {
    fn build_dev(&self) -> Result<()>;
    fn build_release(&self) -> Result<()>;
}

/// Concrete implementation using wasm-pack.
pub struct WasmPackBuilder {
    rust_dir: PathBuf,
    out_dir: PathBuf,
    target: String,
    verbose: bool,
}

impl WasmPackBuilder {
    pub fn new(project_root: &Path, target: &str, verbose: bool) -> Self {
        Self {
            rust_dir: project_root.join(RUST_SUBDIR),
            out_dir: project_root.join(WASM_OUT_DIR),
            target: target.to_string(),
            verbose,
        }
    }
}

impl WasmBuilder for WasmPackBuilder {
    fn build_dev(&self) -> Result<()> {
        let out = self.out_dir.to_string_lossy().to_string();
        process::run_blocking(
            &self.rust_dir,
            "wasm-pack",
            &["build", "--dev", "--target", &self.target, "--out-dir", &out],
            self.verbose,
        )
        .map_err(|e| anyhow::anyhow!(RevenantError::WasmBuildFailed).context(e))
    }

    fn build_release(&self) -> Result<()> {
        let out = self.out_dir.to_string_lossy().to_string();
        process::run_blocking(
            &self.rust_dir,
            "wasm-pack",
            &[
                "build",
                "--release",
                "--target",
                &self.target,
                "--out-dir",
                &out,
            ],
            self.verbose,
        )
        .map_err(|e| anyhow::anyhow!(RevenantError::WasmBuildFailed).context(e))
    }
}

/// Spawns a `wasm-pack build --dev` as a ManagedProcess (non-blocking).
pub fn spawn_wasm_build(
    project_root: &Path,
    target: &str,
    log_tx: mpsc::Sender<process::LogLine>,
) -> Result<process::ManagedProcess> {
    let rust_dir = project_root.join(RUST_SUBDIR);
    let out_dir = project_root.join(WASM_OUT_DIR);
    let out = out_dir.to_string_lossy().to_string();

    process::ManagedProcess::spawn(
        &rust_dir,
        "wasm-pack",
        &["build", "--dev", "--target", target, "--out-dir", &out],
        "wasm",
        log_tx,
    )
}

/// File watcher for `rust/src/` that sends a signal on the rebuild channel
/// after a debounce period.
pub struct WasmWatcher {
    _watcher: RecommendedWatcher,
}

impl WasmWatcher {
    /// Start watching `rust/src/` under the given project root.
    /// Sends `()` on `rebuild_tx` whenever Rust source files change (debounced).
    pub fn start(project_root: &Path, rebuild_tx: mpsc::Sender<()>) -> Result<Self> {
        let watch_dir = project_root.join(RUST_SUBDIR).join("src");

        let debounce = Duration::from_millis(DEBOUNCE_MS);
        let mut last_event = Instant::now() - debounce;

        // DESIGN: debounce is handled inside the notify callback rather than in the
        // consumer because notify fires per-file events — a single save can produce
        // 3-5 events within milliseconds. Filtering here avoids redundant rebuild signals.
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let dominated_by = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                );
                if dominated_by {
                    let now = Instant::now();
                    if now.duration_since(last_event) >= debounce {
                        last_event = now;
                        let _ = rebuild_tx.send(());
                    }
                }
            }
        })
        .context("failed to create file watcher")?;

        watcher
            .watch(&watch_dir, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", watch_dir.display()))?;

        Ok(Self { _watcher: watcher })
    }
}
