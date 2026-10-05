//! Development window, Vite HMR, and interruptible native rebuild ownership.
use crate::{
    compiled::BuildStage,
    config::{self, DEBOUNCE_MS, RevenantConfig},
    errors::RevenantError,
    toolchain::{
        desktop::{DesktopBuild, NativeWatcher},
        process::{self, LogLine, ManagedProcess},
    },
};
use anyhow::{Context, Result};
use std::{
    net::{Ipv4Addr, SocketAddr, TcpStream},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Launch a native window and rebuild its compiled contract on source edits.
/// Vite remains running for frontend HMR. Failed rebuilds keep the previous
/// immutable facade and current window. All owned descendants stop on any exit.
pub fn run(verbose: bool) -> Result<()> {
    process::install_signal_handler()?;
    let root = config::find_project_root(&std::env::current_dir()?)
        .ok_or(RevenantError::ProjectNotFound)?;
    let root = dunce::canonicalize(root)?;
    let mut config = RevenantConfig::load(&root)?;
    super::build::ensure_frontend(&root, &config, verbose)?;
    let mut build = DesktopBuild::new(&root, &config, false)?;
    let (watch_tx, watch_rx) = mpsc::channel();
    let mut watcher = NativeWatcher::start(&root, watch_tx.clone())?;
    build.build(verbose)?;
    BuildStage::new(&root)?.publish(&build.executable, verbose)?;
    let (logs, output) = mpsc::channel::<LogLine>();
    let vite = super::build::frontend_binary(&root, "vite", "vite")?;
    let port = config.desktop.dev_port.to_string();
    let mut frontend = ManagedProcess::spawn(
        &root.join("web"),
        "node",
        &[
            &vite.to_string_lossy(),
            "--host",
            "127.0.0.1",
            "--port",
            &port,
            "--strictPort",
        ],
        "vite",
        logs.clone(),
    )?;
    wait_for_frontend(&mut frontend, &output, config.desktop.dev_port)?;
    let mut desktop = build.launch(&root, logs.clone())?;
    let mut rebuild: Option<ManagedProcess> = None;
    let mut pending: Option<Instant> = None;
    println!(
        "Desktop running. Frontend edits use HMR; native edits rebuild. Ctrl+C stops owned processes."
    );
    loop {
        drain_logs(&output);
        if process::interrupted() {
            break;
        }
        if let Some(status) = frontend.try_wait()? {
            anyhow::bail!("Vite exited with {status}");
        }
        if let Some(status) = desktop.try_wait()? {
            anyhow::ensure!(status.success(), "desktop exited with {status}");
            break;
        }
        while let Ok(event) = watch_rx.try_recv() {
            event.context("native watcher failed")?;
            pending = Some(Instant::now());
        }
        if let Some(child) = rebuild.as_mut() {
            if let Some(status) = child.try_wait()? {
                rebuild = None;
                let result = if status.success() {
                    BuildStage::new(&root)?
                        .publish(&build.executable, verbose)
                        .map(|_| ())
                } else {
                    Err(anyhow::anyhow!("native compiler exited with {status}"))
                };
                match result {
                    Ok(()) => {
                        desktop.terminate_with_timeout();
                        desktop = build.launch(&root, logs.clone())?;
                        println!("Native contract published; desktop restarted.");
                    }
                    Err(error) => {
                        eprintln!("Native rebuild failed; previous generation retained: {error:#}")
                    }
                }
            }
        }
        if rebuild.is_none()
            && pending.is_some_and(|when| when.elapsed() >= Duration::from_millis(DEBOUNCE_MS))
        {
            pending = None;
            let next = (|| -> Result<DesktopBuild> {
                let next_config = RevenantConfig::load(&root)?;
                anyhow::ensure!(
                    next_config.desktop.dev_port == config.desktop.dev_port,
                    "desktop.dev_port changed; restart revenant dev to move Vite and the window together"
                );
                let next = DesktopBuild::new(&root, &next_config, false)?;
                let next_watcher = NativeWatcher::start(&root, watch_tx.clone())?;
                config = next_config;
                watcher = next_watcher;
                Ok(next)
            })();
            match next {
                Ok(next) => match next.spawn(logs.clone()) {
                    Ok(child) => {
                        build = next;
                        rebuild = Some(child);
                        println!("Rebuilding native application…");
                    }
                    Err(error) => eprintln!("Native rebuild could not start: {error:#}"),
                },
                Err(error) => eprintln!("Native rebuild configuration rejected: {error:#}"),
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(rebuild);
    drop(desktop);
    drop(frontend);
    drop(watcher);
    drain_logs(&output);
    println!("All owned desktop and frontend processes stopped.");
    Ok(())
}
fn drain_logs(output: &mpsc::Receiver<LogLine>) {
    while let Ok(log) = output.try_recv() {
        if log.is_stderr {
            eprintln!("[{}] {}", log.prefix, log.line);
        } else {
            println!("[{}] {}", log.prefix, log.line);
        }
    }
}
fn wait_for_frontend(
    frontend: &mut ManagedProcess,
    output: &mpsc::Receiver<LogLine>,
    port: u16,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    loop {
        drain_logs(output);
        anyhow::ensure!(
            !process::interrupted(),
            "interrupted while waiting for Vite"
        );
        if let Some(status) = frontend.try_wait()? {
            anyhow::bail!("Vite failed to start: {status}");
        }
        if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Vite did not listen on {address} within 30 seconds"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
