//! Development window, Vite HMR, and interruptible native rebuild ownership.
use crate::{
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
    build.publish(&root, verbose)?;
    let (logs, output) = mpsc::channel::<LogLine>();
    let vite = super::build::frontend_binary(&root, "vite", "vite")?;
    let frontend_config = frontend_config(&root)?;
    let port = config.desktop.dev_port.to_string();
    let mut frontend = ManagedProcess::spawn(
        &root.join("web"),
        "node",
        &[
            &vite.to_string_lossy(),
            "--config",
            &frontend_config.to_string_lossy(),
            "--host",
            "127.0.0.1",
            "--port",
            &port,
            "--strictPort",
        ],
        "vite",
        logs.clone(),
    )?;
    wait_for_frontend(&mut frontend, &output, config.desktop.dev_port, &root)?;
    let mut desktop = build.launch(&root, logs.clone())?;
    let mut rebuild: Option<ManagedProcess> = None;
    let mut pending: Option<Instant> = None;
    let mut native_diagnostics = Vec::new();
    println!(
        "Desktop running. Frontend edits use HMR; native edits rebuild. Ctrl+C stops owned processes."
    );
    loop {
        drain_logs(&output, &root, &mut native_diagnostics);
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
        if let Some(child) = rebuild.as_mut()
            && let Some(status) = child.try_wait()?
        {
            rebuild = None;
            drain_logs(&output, &root, &mut native_diagnostics);
            crate::inspection::record_build_finish(
                &root,
                status.success(),
                std::mem::take(&mut native_diagnostics),
            )?;
            let result = if status.success() {
                build.publish(&root, verbose).map(|_| ())
            } else {
                build.release_build_lock();
                Err(anyhow::anyhow!("native compiler exited with {status}"))
            };
            match result {
                Ok(()) => {
                    desktop.terminate_checked().context("previous desktop generation did not terminate; replacement was not started")?;
                    desktop = build.launch(&root, logs.clone())?;
                    println!("Native contract published; desktop restarted.");
                }
                Err(error) => {
                    eprintln!("Native rebuild failed; previous generation retained: {error:#}")
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
                        native_diagnostics.clear();
                        build = next;
                        rebuild = Some(child);
                        println!("Rebuilding native application…");
                    }
                    Err(error) => {
                        crate::inspection::record_build_finish(
                            &root,
                            false,
                            vec![crate::inspection::Diagnostic::error(
                                "rust",
                                "native_build_start_failed",
                                format!("{error:#}"),
                            )],
                        )?;
                        eprintln!("Native rebuild could not start: {error:#}");
                    }
                },
                Err(error) => eprintln!("Native rebuild configuration rejected: {error:#}"),
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if rebuild.is_some() {
        crate::inspection::record_build_finish(
            &root,
            false,
            vec![crate::inspection::Diagnostic::error(
                "rust",
                "native_build_interrupted",
                "Development build was interrupted",
            )],
        )?;
    }
    drop(rebuild);
    drop(desktop);
    drop(frontend);
    drop(watcher);
    drain_logs(&output, &root, &mut native_diagnostics);
    println!("All owned desktop and frontend processes stopped.");
    Ok(())
}

/// Wrap authored Vite configuration without editing it. Existing scaffolds also
/// receive SDK source cache invalidation on the normal development launch path.
fn frontend_config(root: &std::path::Path) -> Result<std::path::PathBuf> {
    let directory = root.join(".revenant/frontend");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("vite.config.mjs");
    // Match Vite's default discovery order. Leave loading/bundling the authored
    // module to Vite, so TypeScript and CommonJS configurations keep working.
    let authored = ["js", "mjs", "ts", "cjs", "mts", "cts"]
        .into_iter()
        .map(|extension| format!("vite.config.{extension}"))
        .find(|name| root.join("web").join(name).is_file());
    let import = match authored {
        Some(name) => format!(
            "import authored from {};\n",
            serde_json::to_string(&format!("../../web/{name}"))?
        ),
        None => "const authored = {};\n".to_owned(),
    };
    std::fs::write(
        &path,
        format!(
            "{import}import {{ revenantSdk }} from '../../web/node_modules/@revenant/client/vite.js';\nexport default async environment => {{\n  const config = await (typeof authored === 'function' ? authored(environment) : authored);\n  return {{ ...config, plugins: [...(config.plugins ?? []), revenantSdk()] }};\n}};\n"
        ),
    )?;
    Ok(path)
}
fn drain_logs(
    output: &mpsc::Receiver<LogLine>,
    root: &std::path::Path,
    diagnostics: &mut Vec<crate::inspection::Diagnostic>,
) {
    while let Ok(log) = output.try_recv() {
        if log.prefix == "native-build" && !log.is_stderr {
            for diagnostic in crate::inspection::parse_cargo_diagnostics(root, &log.line) {
                if diagnostics.len() < 200 {
                    diagnostics.push(diagnostic);
                }
            }
            if let Ok(message) = serde_json::from_str::<serde_json::Value>(&log.line) {
                if let Some(rendered) = message["message"]["rendered"].as_str() {
                    eprint!("{rendered}");
                }
                continue;
            }
        }
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
    root: &std::path::Path,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    loop {
        drain_logs(output, root, &mut Vec::new());
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

#[cfg(test)]
mod frontend_config_tests {
    #[test]
    fn cache_wrapper_loads_typed_configuration_without_changing_its_origin() {
        for extension in ["ts", "mts"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let client = root.join("web/node_modules/@revenant/client");
            std::fs::create_dir_all(&client).unwrap();
            std::fs::write(root.join("web/package.json"), r#"{"type":"module"}"#).unwrap();
            std::fs::write(client.join("package.json"), r#"{"type":"module"}"#).unwrap();
            std::fs::write(
                client.join("vite.js"),
                "export function revenantSdk() { return { name: 'sdk-digest' }; }",
            )
            .unwrap();
            let authored_path = root.join(format!("web/vite.config.{extension}"));
            let authored = "const port: number = 8124; export default async (env: { mode: string }) => ({ server: { port }, mode: env.mode, origin: import.meta.url });";
            std::fs::write(&authored_path, authored).unwrap();
            super::frontend_config(root).unwrap();
            let output = crate::toolchain::process::run_capture(root, "node", &["--input-type=module", "-e", "const { default: load } = await import('./.revenant/frontend/vite.config.mjs'); console.log(JSON.stringify(await load({mode:'development'})));"], false).unwrap();
            let result: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(result["server"]["port"], 8124);
            assert_eq!(result["mode"], "development");
            assert!(
                result["origin"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("/web/vite.config.{extension}"))
            );
            assert_eq!(std::fs::read_to_string(authored_path).unwrap(), authored);
        }
    }

    #[test]
    #[ignore = "requires installed demo frontend dependencies"]
    fn actual_typescript_demo_configs_load_through_vite_wrapper() {
        for project in ["file-media", "image-notes"] {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join(project);
            super::frontend_config(&root).unwrap();
            let output = crate::toolchain::process::run_capture(&root.join("web"), "node", &["--input-type=module", "-e", "import {loadConfigFromFile} from './node_modules/vite/dist/node/index.js'; const loaded = await loadConfigFromFile({command:'serve', mode:'development'}, '../.revenant/frontend/vite.config.mjs', process.cwd()); const plugins = (await Promise.all(loaded.config.plugins.flat(Infinity))).flat(Infinity).filter(Boolean); if (!plugins.some(p=>p.name?.startsWith('revenant-sdk-'))) throw Error('SDK plugin missing'); console.log('typed demo config loaded');"], false).unwrap();
            assert!(
                output.contains("typed demo config loaded"),
                "{project}: {output}"
            );
        }
    }

    #[test]
    fn cache_wrapper_preserves_async_authored_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let client = root.join("web/node_modules/@revenant/client");
        std::fs::create_dir_all(&client).unwrap();
        std::fs::write(root.join("web/package.json"), r#"{"type":"module"}"#).unwrap();
        std::fs::write(client.join("package.json"), r#"{"type":"module"}"#).unwrap();
        std::fs::write(
            client.join("vite.js"),
            "export function revenantSdk() { return { name: 'sdk-digest' }; }",
        )
        .unwrap();
        let authored = "export default async env => ({ server: { port: 8123 }, mode: env.mode, plugins: [{ name: 'authored' }] });";
        std::fs::write(root.join("web/vite.config.js"), authored).unwrap();
        super::frontend_config(root).unwrap();
        let output = crate::toolchain::process::run_capture(root, "node", &["--input-type=module", "-e", "const { default: load } = await import('./.revenant/frontend/vite.config.mjs'); console.log(JSON.stringify(await load({mode:'development'})));"], false).unwrap();
        let result: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["server"]["port"], 8123);
        assert_eq!(result["mode"], "development");
        assert_eq!(
            result["plugins"],
            serde_json::json!([{ "name": "authored" }, { "name": "sdk-digest" }])
        );
        assert_eq!(
            std::fs::read_to_string(root.join("web/vite.config.js")).unwrap(),
            authored
        );
    }
}
