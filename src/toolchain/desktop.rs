//! Native host compilation and source watching; no alternate transport backend.
use crate::{config::RevenantConfig, scaffold, toolchain::process};
use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::mpsc,
};

/// Generated host and native compiler arguments for one configuration.
pub struct DesktopBuild {
    pub host: PathBuf,
    pub executable: PathBuf,
    args: Vec<String>,
}
impl DesktopBuild {
    /// Prepare the hidden host. No compilation or contract publication occurs here.
    pub fn new(root: &Path, config: &RevenantConfig, release: bool) -> Result<Self> {
        let root = dunce::canonicalize(root)?;
        let host = scaffold::prepare_host(&root, config)?;
        let target = root.join(".revenant/target");
        let executable = target
            .join(if release { "release" } else { "debug" })
            .join(format!(
                "{}{}",
                config.project.name,
                if cfg!(windows) { ".exe" } else { "" }
            ));
        let mut args = vec![
            "build".into(),
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
        })
    }
    /// Compile without Tauri's custom-protocol embedding feature. This allows
    /// first contract extraction before the SPA exists, with no placeholder app.
    pub fn build(&self, verbose: bool) -> Result<()> {
        process::run_blocking(
            &self.host,
            "cargo",
            &self.args.iter().map(String::as_str).collect::<Vec<_>>(),
            verbose,
        )
        .context("compile desktop host; see https://v2.tauri.app/start/prerequisites/")
    }
    /// Compile asynchronously so dev can respond to interrupts and new edits.
    pub fn spawn(&self, logs: mpsc::Sender<process::LogLine>) -> Result<process::ManagedProcess> {
        process::ManagedProcess::spawn(
            &self.host,
            "cargo",
            &self.args.iter().map(String::as_str).collect::<Vec<_>>(),
            "native-build",
            logs,
        )
    }
    /// Launch the compiled window; the CLI retains ownership of its process tree.
    pub fn launch(
        &self,
        root: &Path,
        logs: mpsc::Sender<process::LogLine>,
    ) -> Result<DesktopProcess> {
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
            &[],
            "desktop",
            logs,
        )?;
        Ok(DesktopProcess {
            child,
            _snapshot: snapshot,
        })
    }
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
    /// Stop the window tree before replacing its native generation.
    pub fn terminate_with_timeout(&mut self) {
        self.child.terminate_with_timeout();
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
impl Drop for RunDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!(
                "Could not remove owned desktop snapshot {}: {error}",
                self.0.display()
            );
        }
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
                    ) =>
                {
                    if event.need_rescan()
                        || event.paths.iter().any(|p| {
                            // Match generated directories relative to the app,
                            // not an ancestor or an app literally named desktop.
                            let relative = p.strip_prefix(&project_root).unwrap_or(p);
                            !relative.starts_with(".revenant/desktop")
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
                        })
                    {
                        let _ = tx.send(Ok(()));
                    }
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
