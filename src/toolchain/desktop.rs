//! Native host compilation and source watching; no alternate transport backend.
use crate::{config::RevenantConfig, scaffold, toolchain::process};
use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
};

/// Generated host and native compiler arguments for one configuration.
pub struct DesktopBuild {
    pub host: PathBuf,
    pub executable: PathBuf,
    args: Vec<String>,
    source_identity: Option<String>,
    build_attempt: Option<String>,
    project_lock: Mutex<Option<std::fs::File>>,
}
impl DesktopBuild {
    /// Locate an executable without preparing a host or starting a build attempt.
    pub fn executable_path(root: &Path, config: &RevenantConfig, release: bool) -> PathBuf {
        root.join(".revenant/target")
            .join(if release { "release" } else { "debug" })
            .join(format!(
                "{}{}",
                config.project.name,
                if cfg!(windows) { ".exe" } else { "" }
            ))
    }
    /// Prepare the hidden host. No compilation or contract publication occurs here.
    pub fn new(root: &Path, config: &RevenantConfig, release: bool) -> Result<Self> {
        let root = dunce::canonicalize(root)?;
        let project_lock = crate::inspection::acquire_project_lock(&root)?;
        Self::with_project_lock(&root, config, release, project_lock)
    }
    pub(crate) fn with_project_lock(
        root: &Path,
        config: &RevenantConfig,
        release: bool,
        project_lock: std::fs::File,
    ) -> Result<Self> {
        let root = dunce::canonicalize(root)?;
        scaffold::sdk::ensure_sources(&root)?;
        let host = scaffold::prepare_host(&root, config)?;
        // The generated host owns a separate dependency lock. Resolve it before
        // capturing source truth: compilation must not add/change this indexed
        // lock and make an otherwise untouched application immediately stale.
        let (resolved, _, stderr) = crate::inspection::bounded_process(
            &host,
            "cargo",
            &["fetch"],
            &[],
            std::time::Duration::from_secs(300),
        )
        .context("resolve desktop host lockfile before capturing source identity")?;
        anyhow::ensure!(resolved, "resolve desktop host lockfile failed: {stderr}");
        crate::inspection::record_build_start(
            &root,
            if release {
                "native:release"
            } else {
                "native:development"
            },
        )?;
        let source_identity = crate::inspection::latest_build_source_fingerprint(&root)?;
        let build_attempt =
            crate::inspection::ProjectInspector::open(&root)?.build_attempt()?["id"]
                .as_str()
                .map(String::from);
        // Dependency resolution needs a host first; compilation must use the
        // identity just captured, not the attempt used for that initial render.
        scaffold::prepare_host(&root, config)?;
        let target = root.join(".revenant/target");
        let executable = Self::executable_path(&root, config, release);
        let mut args = vec![
            "build".into(),
            "--message-format=json-render-diagnostics".into(),
            "--bin".into(),
            config.project.name.clone(),
            "--target-dir".into(),
            target.to_string_lossy().into_owned(),
        ];
        if release {
            args.push("--release".into());
        }
        Ok(Self {
            host,
            executable,
            args,
            source_identity,
            build_attempt,
            project_lock: Mutex::new(Some(project_lock)),
        })
    }
    /// Compile without Tauri's custom-protocol embedding feature. This allows
    /// first contract extraction before the SPA exists, with no placeholder app.
    pub fn build(&self, verbose: bool) -> Result<()> {
        let root = self
            .host
            .parent()
            .and_then(Path::parent)
            .context("generated host root")?;
        let outcome = crate::inspection::bounded_process(
            &self.host,
            "cargo",
            &self.args.iter().map(String::as_str).collect::<Vec<_>>(),
            &[],
            std::time::Duration::from_secs(3600),
        );
        match outcome {
            Ok((succeeded, stdout, stderr)) => {
                let mut diagnostics = crate::inspection::parse_cargo_diagnostics(root, &stdout);
                if verbose {
                    eprint!("{stderr}");
                    for line in stdout
                        .lines()
                        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    {
                        if let Some(rendered) = line["message"]["rendered"].as_str() {
                            eprint!("{rendered}");
                        }
                    }
                }
                if !succeeded
                    && !diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.severity == "error")
                {
                    diagnostics.push(crate::inspection::Diagnostic::error(
                        "rust",
                        "native_build_failed",
                        stderr.chars().take(4000).collect::<String>(),
                    ));
                }
                crate::inspection::record_build_finish(root, succeeded, diagnostics)?;
                if !succeeded {
                    self.release_build_lock();
                }
                anyhow::ensure!(
                    succeeded,
                    "compile desktop host failed; diagnostics available through revenant inspect/check: {stderr}"
                );
                Ok(())
            }
            Err(error) => {
                crate::inspection::record_build_finish(
                    root,
                    false,
                    vec![crate::inspection::Diagnostic::error(
                        "rust",
                        "native_build_failed",
                        format!("{error:#}"),
                    )],
                )?;
                self.release_build_lock();
                Err(error)
                    .context("compile desktop host; see https://v2.tauri.app/start/prerequisites/")
            }
        }
    }
    /// Compile asynchronously so dev can respond to interrupts and new edits.
    pub fn spawn(&self, logs: mpsc::Sender<process::LogLine>) -> Result<process::ManagedProcess> {
        let result = process::ManagedProcess::spawn(
            &self.host,
            "cargo",
            &self.args.iter().map(String::as_str).collect::<Vec<_>>(),
            "native-build",
            logs,
        );
        if result.is_err() {
            self.release_build_lock();
        }
        result
    }
    /// Publish bindings with the source identity captured before this compilation.
    pub fn publish(&self, root: &Path, verbose: bool) -> Result<String> {
        let result = (|| {
            crate::compiled::BuildStage::new_with_source_identity(
                root,
                self.source_identity.clone(),
                self.build_attempt.clone(),
            )?
            .publish(&self.executable, verbose)
        })();
        self.release_build_lock();
        result
    }
    /// End the preparation/compilation transaction before an idle development loop.
    pub(crate) fn release_build_lock(&self) {
        self.project_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
    /// Launch the compiled window; the CLI retains ownership of its process tree.
    pub fn launch(
        &self,
        root: &Path,
        logs: mpsc::Sender<process::LogLine>,
    ) -> Result<DesktopProcess> {
        #[cfg(windows)]
        let browser_switches = dev_browser_switches(std::env::var_os("REVENANT_DEVTOOLS_PORT"))?;
        let mut arguments = vec!["--revenant-dev"];
        #[cfg(windows)]
        if let Some(switches) = browser_switches.as_deref() {
            arguments.push(switches);
        }
        // Windows locks running executables. Launch an owned copy so Cargo can
        // rebuild the target while the previous app remains usable on failure.
        let root = dunce::canonicalize(root)?;
        let runs = root.join(".revenant/run");
        std::fs::create_dir_all(&runs)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        // Preserve the app basename for platform identity and companion assets.
        // Allocate exclusively: cleanup may only remove a directory we created.
        let directory = runs.join(format!("{}-{stamp}", std::process::id()));
        std::fs::create_dir(&directory)?;
        let snapshot = RunDirectory(directory);
        let executable = snapshot.0.join(
            self.executable
                .file_name()
                .context("desktop executable name")?,
        );
        std::fs::copy(&self.executable, &executable)?;
        let child = process::ManagedProcess::spawn(
            &root,
            &executable.to_string_lossy(),
            &arguments,
            "desktop",
            logs,
        )?;
        Ok(DesktopProcess {
            child,
            _snapshot: snapshot,
        })
    }
}

#[cfg(any(windows, test))]
fn dev_browser_switches(port: Option<std::ffi::OsString>) -> Result<Option<String>> {
    let Some(port) = port else { return Ok(None) };
    let port = port.into_string().map_err(|_| {
        anyhow::anyhow!("REVENANT_DEVTOOLS_PORT must be an integer from 1 through 65535")
    })?;
    anyhow::ensure!(
        !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()),
        "REVENANT_DEVTOOLS_PORT must be an integer from 1 through 65535"
    );
    let port: u16 = port
        .parse()
        .context("REVENANT_DEVTOOLS_PORT must be an integer from 1 through 65535")?;
    anyhow::ensure!(
        port != 0,
        "REVENANT_DEVTOOLS_PORT must be an integer from 1 through 65535"
    );
    Ok(Some(format!(
        "--edge-webview-switches=--remote-debugging-port={port} --remote-debugging-address=127.0.0.1"
    )))
}

/// An owned desktop process and its private executable snapshot.
pub struct DesktopProcess {
    child: process::ManagedProcess,
    _snapshot: RunDirectory,
}
impl DesktopProcess {
    /// Poll the window process while retaining descendant cleanup ownership.
    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }
    /// Observe exact descendant membership during Windows restart diagnostics.
    #[cfg(windows)]
    pub fn owns_process(&self, process_id: u32) -> Result<bool> {
        self.child.owns_process(process_id)
    }
    /// Stop the window tree before replacing its native generation.
    pub fn terminate_with_timeout(&mut self) {
        self.child.terminate_with_timeout();
    }
    /// Stop and await the owned tree before admitting a replacement window.
    pub fn terminate_checked(&mut self) -> Result<()> {
        self.child.terminate_checked()
    }
}
impl Drop for DesktopProcess {
    fn drop(&mut self) {
        self.child.terminate_with_timeout();
        // Fields drop after this method, in declaration order: the managed
        // process/job closes before the private directory is reclaimed.
    }
}

struct RunDirectory(PathBuf);

fn remove_owned_snapshot(directory: &Path, timeout: std::time::Duration) -> std::io::Result<()> {
    #[cfg(windows)]
    let deadline = std::time::Instant::now() + timeout;
    #[cfg(not(windows))]
    let _ = timeout;
    loop {
        match std::fs::remove_dir_all(directory) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                #[cfg(windows)]
                // Process-tree exit can precede release of an executable's file lock.
                // Only retry Windows access/sharing/lock errors on this owned snapshot.
                if matches!(error.raw_os_error(), Some(5 | 32 | 33)) {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if !remaining.is_zero() {
                        std::thread::sleep(remaining.min(std::time::Duration::from_millis(10)));
                        continue;
                    }
                }
                return Err(error);
            }
        }
    }
}

impl Drop for RunDirectory {
    fn drop(&mut self) {
        let started = std::time::Instant::now();
        if let Err(error) = remove_owned_snapshot(
            &self.0,
            std::time::Duration::from_secs(crate::config::SHUTDOWN_TIMEOUT_SECS),
        ) {
            eprintln!(
                "Could not remove owned desktop snapshot {} after {:?}: {error}",
                self.0.display(),
                started.elapsed()
            );
        }
    }
}

#[cfg(all(test, windows))]
mod snapshot_cleanup_tests {
    use super::remove_owned_snapshot;
    use std::{
        fs::{self, File, OpenOptions},
        os::windows::fs::OpenOptionsExt,
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };
    use tempfile::TempDir;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    fn locked_snapshot() -> (TempDir, PathBuf, File) {
        let temp = TempDir::new().unwrap();
        let directory = temp.path().join("owned-snapshot");
        fs::create_dir(&directory).unwrap();
        let image = directory.join("app.exe");
        fs::write(&image, "snapshot").unwrap();
        // A real Windows handle permits reading and writing but blocks deletion.
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&image)
            .unwrap();
        let error = fs::remove_dir_all(&directory).unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32 | 33)));
        (temp, directory, lock)
    }

    #[test]
    fn delayed_windows_file_lock_is_reclaimed() {
        let (_temp, directory, lock) = locked_snapshot();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            drop(lock);
        });
        let result = remove_owned_snapshot(&directory, Duration::from_secs(2));
        release.join().unwrap();
        result.unwrap();
        assert!(!directory.exists());
    }

    #[test]
    fn persistent_windows_file_lock_respects_deadline() {
        let (_temp, directory, lock) = locked_snapshot();
        let timeout = Duration::from_millis(50);
        let started = Instant::now();
        let result = remove_owned_snapshot(&directory, timeout);
        let elapsed = started.elapsed();
        drop(lock);
        let error = result.unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32 | 33)));
        assert!(
            elapsed >= timeout,
            "cleanup returned before its retry deadline"
        );
        assert!(elapsed < timeout + Duration::from_secs(1));
        assert!(directory.exists());
    }

    #[test]
    fn missing_snapshot_is_already_reclaimed() {
        let temp = TempDir::new().unwrap();
        remove_owned_snapshot(&temp.path().join("missing"), Duration::ZERO).unwrap();
    }

    #[test]
    fn cleanup_preserves_unrelated_directories() {
        let temp = TempDir::new().unwrap();
        let owned = temp.path().join("owned");
        let unrelated = temp.path().join("unrelated");
        fs::create_dir(&owned).unwrap();
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("keep"), "unrelated snapshot").unwrap();
        remove_owned_snapshot(&owned, Duration::from_secs(1)).unwrap();
        assert!(!owned.exists());
        assert_eq!(
            fs::read_to_string(unrelated.join("keep")).unwrap(),
            "unrelated snapshot"
        );
    }

    #[test]
    fn non_lock_error_returns_immediately() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("not-a-directory");
        fs::write(&file, "keep").unwrap();
        let started = Instant::now();
        let error = remove_owned_snapshot(&file, Duration::from_secs(5)).unwrap_err();
        assert!(!matches!(error.raw_os_error(), Some(5 | 32 | 33)));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
    }
}

/// Watch native source, handwritten configuration and path dependencies.
/// Frontend changes are handled by Vite HMR; generated outputs never trigger a build.
pub struct NativeWatcher {
    _watcher: RecommendedWatcher,
}

impl NativeWatcher {
    /// Install watchers before the first compilation to retain edits during builds.
    pub fn start(root: &Path, tx: mpsc::Sender<Result<()>>) -> Result<Self> {
        let project_root = root.to_path_buf();
        let mut watcher =
            notify::recommended_watcher(move |result: Result<Event, notify::Error>| match result {
                Err(error) => {
                    let _ = tx.send(Err(error.into()));
                }
                Ok(event)
                    if matches!(
                        event.kind,
                        EventKind::Create(_)
                            | EventKind::Modify(_)
                            | EventKind::Remove(_)
                            | EventKind::Other
                    ) && (event.need_rescan()
                        || event.paths.iter().any(|p| {
                            // Match generated directories relative to the app,
                            // not an ancestor or an app literally named desktop.
                            let relative = p.strip_prefix(&project_root).unwrap_or(p);
                            !relative.starts_with(".revenant/desktop")
                                // Writing generated state can change directory timestamps.
                                // Keep source writes and directory creation/removal observable.
                                && !(matches!(event.kind, EventKind::Modify(_)) && p.is_dir())
                                && !relative.components().any(|part| {
                                    matches!(
                                        part.as_os_str().to_str(),
                                        Some("target" | "node_modules" | ".git" | "build")
                                    )
                                })
                                && (p
                                    .extension()
                                    .is_some_and(|ext| ext == "rs" || ext == "toml")
                                    || p.extension().is_none())
                        })) =>
                {
                    let _ = tx.send(Ok(()));
                }
                _ => {}
            })
            .context("create native source watcher")?;
        let mut paths = BTreeSet::new();
        if root.join("native/Cargo.toml").is_file() {
            local_dependencies(&root.join("native"), &mut paths)?;
        }
        if root.join(".revenant/sdk/Cargo.toml").is_file() {
            paths.insert(dunce::canonicalize(root.join(".revenant/sdk"))?);
        }
        let mut parents = BTreeSet::new();
        for directory in &paths {
            for ancestor in directory.ancestors().skip(1) {
                if let Ok(contents) = std::fs::read_to_string(ancestor.join("Cargo.toml")) {
                    let value: toml::Value = toml::from_str(&contents)?;
                    if value.get("workspace").is_some() {
                        parents.insert(ancestor.to_path_buf());
                        break;
                    }
                }
            }
        }
        for parent in parents {
            if !paths.contains(&parent) {
                watcher.watch(&parent, RecursiveMode::NonRecursive)?;
            }
        }
        for directory in paths {
            watcher
                .watch(&directory, RecursiveMode::Recursive)
                .with_context(|| format!("watch {}", directory.display()))?;
        }
        watcher.watch(root, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher })
    }
}
fn local_dependencies(directory: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<()> {
    let directory = dunce::canonicalize(directory)?;
    if !paths.insert(directory.clone()) {
        return Ok(());
    }
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(directory.join("Cargo.toml"))?)?;
    fn visit(value: &toml::Value, base: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<()> {
        if let Some(table) = value.as_table() {
            if let Some(path) = table.get("path").and_then(toml::Value::as_str) {
                let dependency = base.join(path);
                if dependency.join("Cargo.toml").is_file() {
                    local_dependencies(&dependency, paths)?;
                }
            }
            for (key, child) in table {
                if !matches!(key.as_str(), "package" | "lib" | "bin") {
                    visit(child, base, paths)?;
                }
            }
        }
        Ok(())
    }
    visit(&manifest, &directory, paths)
}

#[cfg(test)]
mod browser_debug_tests {
    #[test]
    fn dev_browser_port_is_explicit_validated_and_loopback_only() {
        assert!(super::dev_browser_switches(None).unwrap().is_none());
        assert_eq!(
            super::dev_browser_switches(Some("9333".into()))
                .unwrap()
                .as_deref(),
            Some(
                "--edge-webview-switches=--remote-debugging-port=9333 --remote-debugging-address=127.0.0.1"
            )
        );
        for invalid in [
            "",
            "0",
            "65536",
            "-1",
            "+9333",
            "9333 --disable-web-security",
            " 9333",
        ] {
            assert!(
                super::dev_browser_switches(Some(invalid.into())).is_err(),
                "accepted {invalid}"
            );
        }
    }
}
