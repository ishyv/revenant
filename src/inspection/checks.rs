use super::{
    Diagnostic, MAX_RESULTS, ProjectInspector, acquire_project_lock, cancellable_process,
    finish_attempt, inspection_directory, relative, start_attempt,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

fn contained_cargo_path(root: &Path, path: &Path) -> Result<()> {
    // Cargo can create the target directory, so validate every existing parent
    // as well as an existing leaf. Canonical paths detect Windows junctions.
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => ensure!(
                dunce::canonicalize(ancestor)?.starts_with(root),
                "Cargo check path escapes project root: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if ancestor == root {
            break;
        }
    }
    Ok(())
}

fn sync_frontend(inspector: &ProjectInspector) -> Result<bool> {
    let root = inspector.root();
    inspector.cancellation.checkpoint()?;
    let web = root.join("web");
    if !web
        .join("node_modules/@sveltejs/kit/package.json")
        .is_file()
    {
        return Ok(false);
    }
    // Fresh dependency installations do not contain Kit's generated $app config
    // and types. Use the installed tool directly, never an arbitrary npm script.
    let binary = crate::commands::build::frontend_binary(root, "@sveltejs/kit", "svelte-kit")?;
    let (succeeded, stdout, stderr) = cancellable_process(
        &web,
        "node",
        &[&binary.to_string_lossy(), "sync"],
        &[],
        Duration::from_secs(60),
        &inspector.cancellation,
    )?;
    ensure!(
        succeeded,
        "SvelteKit config/type preparation failed: {}{}",
        stderr.chars().take(4000).collect::<String>(),
        stdout.chars().take(1000).collect::<String>()
    );
    Ok(true)
}

/// Parse Cargo's compiler JSON without mixing logs into machine diagnostics.
pub fn parse_cargo_diagnostics(root: &Path, output: &str) -> Vec<Diagnostic> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-message")
        .filter_map(|message| {
            let diagnostic = &message["message"];
            let spans = diagnostic["spans"].as_array();
            let span = spans.and_then(|spans| {
                spans
                    .iter()
                    .find(|span| span["is_primary"] == true)
                    .or_else(|| spans.first())
            });
            let file = span
                .and_then(|span| span["file_name"].as_str())
                .map(|file| {
                    let path = PathBuf::from(file);
                    let candidate = if path.is_absolute() {
                        path
                    } else {
                        [
                            root.join(&path),
                            root.join("native").join(&path),
                            root.join(".revenant/desktop").join(&path),
                        ]
                        .into_iter()
                        .find(|path| path.is_file())
                        .unwrap_or_else(|| root.join(path))
                    };
                    relative(root, &candidate)
                });
            Some(Diagnostic {
                stage: "rust".into(),
                code: diagnostic["code"]["code"].as_str().map(String::from),
                severity: diagnostic["level"].as_str().unwrap_or("error").into(),
                message: diagnostic["message"].as_str()?.chars().take(4000).collect(),
                file,
                line: span.and_then(|span| span["line_start"].as_u64()),
                column: span.and_then(|span| span["column_start"].as_u64()),
                end_line: span.and_then(|span| span["line_end"].as_u64()),
                end_column: span.and_then(|span| span["column_end"].as_u64()),
            })
        })
        .take(MAX_RESULTS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_check_preserves_cleanup_diagnostic() {
        let root = tempfile::tempdir().unwrap();
        let directory = inspection_directory(root.path()).unwrap();
        let attempt = start_attempt(root.path(), "check:frontend").unwrap();
        let error = revenant_core::Error::new(
            "inspection_process_cleanup",
            "injected tree cleanup failure",
        )
        .into();
        fail_check_attempt(
            &directory,
            attempt,
            vec![Diagnostic::error(
                "frontend",
                "earlier_failure",
                "earlier evidence",
            )],
            &error,
        )
        .unwrap();
        let evidence: Value =
            serde_json::from_slice(&std::fs::read(directory.join("check.json")).unwrap()).unwrap();
        assert!(
            evidence["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "inspection_process_cleanup")
        );
        assert!(
            evidence["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "earlier_failure")
        );
    }

    #[test]
    fn failed_completion_fingerprint_does_not_leave_running_evidence() {
        let root = tempfile::tempdir().unwrap();
        let directory = inspection_directory(root.path()).unwrap();
        let attempt = start_attempt(root.path(), "check:sdk").unwrap();
        crate::compiled::atomic_write(
            &directory.join("check.json"),
            &serde_json::to_vec(&attempt).unwrap(),
        )
        .unwrap();
        let result = finish_check_attempt(&directory, &attempt, || {
            anyhow::bail!("injected unreadable source")
        });
        assert!(result.is_err());
        let evidence: Value =
            serde_json::from_slice(&std::fs::read(directory.join("check.json")).unwrap()).unwrap();
        assert_eq!(evidence["status"], "failed");
        assert!(evidence["finishedAt"].is_number());
        assert_eq!(
            evidence["diagnostics"][0]["code"],
            "source_fingerprint_failed"
        );
        assert!(evidence["completedSourceFingerprint"].is_null());
    }
}

fn finish_check_attempt(
    directory: &Path,
    attempt: &super::BuildAttempt,
    finish: impl FnOnce() -> Result<super::BuildAttempt>,
) -> Result<super::BuildAttempt> {
    match finish() {
        Ok(attempt) => Ok(attempt),
        Err(error) => {
            let mut attempt = attempt.clone();
            attempt.status = "failed".into();
            attempt.finished_at = Some(super::now()?);
            attempt.completed_source_fingerprint = None;
            attempt.diagnostics.push(Diagnostic::error(
                "inspection",
                "source_fingerprint_failed",
                format!("{error:#}"),
            ));
            crate::compiled::atomic_write(
                &directory.join("check.json"),
                &serde_json::to_vec(&attempt)?,
            )?;
            Err(error)
        }
    }
}

pub(super) fn run(inspector: &ProjectInspector, profile: &str) -> Result<Value> {
    ensure!(
        matches!(profile, "all" | "rust" | "frontend" | "sdk"),
        "check profile must be all, rust, frontend or sdk"
    );
    let root = inspector.root();
    let rust_paths = if matches!(profile, "all" | "rust") {
        let manifest = [
            root.join(".revenant/desktop/Cargo.toml"),
            root.join("native/Cargo.toml"),
        ]
        .into_iter()
        .find(|file| file.is_file());
        if let Some(manifest) = &manifest {
            contained_cargo_path(root, manifest)?;
        }
        let target = root.join(".revenant/target");
        contained_cargo_path(root, &target)?;
        Some((manifest, target))
    } else {
        None
    };
    let _native_preparation_lock = acquire_project_lock(root)?;
    let directory = inspection_directory(root)?;
    let lock_path = directory.join("check.lock");
    if lock_path.try_exists()? {
        ensure!(
            !std::fs::symlink_metadata(&lock_path)?
                .file_type()
                .is_symlink()
                && dunce::canonicalize(&lock_path)?.starts_with(root),
            "check lock escapes project root"
        );
    }
    let project_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    project_lock.try_lock().map_err(|error| {
        anyhow::anyhow!("Project check already running or its lock is unavailable: {error}")
    })?;
    let attempt = start_attempt(root, &format!("check:{profile}"))?;
    crate::compiled::atomic_write(
        &directory.join("check.json"),
        &serde_json::to_vec(&attempt)?,
    )?;
    let mut diagnostics = Vec::new();
    let mut steps = Vec::new();
    let execution = (|| -> Result<()> {
        inspector.cancellation.checkpoint()?;
        if matches!(profile, "all" | "sdk") {
            let checked = crate::scaffold::sdk::check(root);
            let passed = checked.is_ok();
            if let Err(error) = checked {
                diagnostics.push(Diagnostic::error(
                    "sdk",
                    "sdk_incompatible",
                    format!("{error:#}"),
                ));
            }
            steps.push(json!({"stage":"sdk","succeeded":passed}));
        }
        inspector.cancellation.checkpoint()?;
        if matches!(profile, "all" | "rust") {
            let (manifest, target) =
                rust_paths.expect("Rust profile paths validated before checks");
            if let Some(manifest) = manifest {
                match cancellable_process(
                    root,
                    "cargo",
                    &[
                        "check",
                        "--manifest-path",
                        &manifest.to_string_lossy(),
                        "--message-format=json",
                        "--target-dir",
                        &target.to_string_lossy(),
                    ],
                    &[],
                    Duration::from_secs(300),
                    &inspector.cancellation,
                ) {
                    Ok((succeeded, output, stderr)) => {
                        let mut compiler = parse_cargo_diagnostics(root, &output);
                        if !succeeded && !compiler.iter().any(|d| d.severity == "error") {
                            compiler.push(Diagnostic::error(
                                "rust",
                                "cargo_check_failed",
                                stderr.chars().take(4000).collect::<String>(),
                            ));
                        }
                        diagnostics.extend(compiler);
                        steps.push(json!({"stage":"rust","succeeded":succeeded,"manifest":relative(root,&manifest)}));
                    }
                    Err(error)
                        if super::process_cleanup_failure(&error)
                            || error
                                .downcast_ref::<revenant_core::Error>()
                                .is_some_and(|error| error.code == "request_cancelled") =>
                    {
                        return Err(error);
                    }
                    Err(error) => {
                        diagnostics.push(Diagnostic::error(
                            "rust",
                            "cargo_unavailable",
                            error.to_string(),
                        ));
                        steps.push(json!({"stage":"rust","succeeded":false}));
                    }
                }
            } else {
                diagnostics.push(Diagnostic::error("rust","missing_native_host","No generated desktop or native Cargo manifest. Run revenant dev once to prepare the native host."));
                steps.push(json!({"stage":"rust","succeeded":false}));
            }
        }
        inspector.cancellation.checkpoint()?;
        let check_frontend = matches!(profile, "all" | "frontend");
        let frontend_prepared = if check_frontend {
            match sync_frontend(inspector) {
                Ok(synced) => {
                    if synced {
                        steps.push(json!({"stage":"frontend_prepare","succeeded":true}));
                    }
                    true
                }
                Err(error)
                    if super::process_cleanup_failure(&error)
                        || error
                            .downcast_ref::<revenant_core::Error>()
                            .is_some_and(|error| error.code == "request_cancelled") =>
                {
                    return Err(error);
                }
                Err(error) => {
                    diagnostics.push(Diagnostic::error(
                        "frontend",
                        "sveltekit_sync_failed",
                        format!("{error:#}"),
                    ));
                    steps.push(json!({"stage":"frontend_prepare","succeeded":false}));
                    steps.push(json!({"stage":"frontend","succeeded":false,"skipped":"frontend preparation failed"}));
                    false
                }
            }
        } else {
            false
        };
        inspector.cancellation.checkpoint()?;
        if frontend_prepared {
            match inspector.semantic(
                json!({"action":"diagnostics","maxResults":MAX_RESULTS,"timeoutMs":20000}),
            ) {
                Ok(index) => {
                    if let Some(items) = index["diagnostics"].as_array() {
                        diagnostics.extend(items.iter().filter_map(|item| {
                            serde_json::from_value::<Diagnostic>(item.clone()).ok()
                        }));
                    }
                    if index["status"] != "complete" {
                        for issue in index["issues"].as_array().into_iter().flatten() {
                            diagnostics.push(Diagnostic::error(
                                "frontend",
                                "language_service_incomplete",
                                issue["message"]
                                    .as_str()
                                    .unwrap_or("Language inspection incomplete"),
                            ));
                        }
                    }
                    if index["truncated"] == true {
                        diagnostics.push(Diagnostic::error(
                            "frontend",
                            "diagnostics_truncated",
                            "Frontend diagnostics or source indexing reached the inspection budget",
                        ));
                    }
                    let succeeded = index["status"] == "complete"
                        && !diagnostics.iter().any(|d| {
                            matches!(d.stage.as_str(), "typescript" | "svelte" | "frontend")
                                && d.severity == "error"
                        });
                    steps.push(
                    json!({"stage":"frontend","succeeded":succeeded,"services":index["services"]}),
                );
                }
                Err(error)
                    if super::process_cleanup_failure(&error)
                        || error
                            .downcast_ref::<revenant_core::Error>()
                            .is_some_and(|error| error.code == "request_cancelled") =>
                {
                    return Err(error);
                }
                Err(error) => {
                    diagnostics.push(Diagnostic::error(
                        "frontend",
                        "language_service_failed",
                        error.to_string(),
                    ));
                    steps.push(json!({"stage":"frontend","succeeded":false}));
                }
            }
        }
        inspector.cancellation.checkpoint()?;
        Ok(())
    })();
    if let Err(error) = execution {
        fail_check_attempt(&directory, attempt, diagnostics, &error)?;
        return Err(error);
    }
    let succeeded = steps.iter().all(|step| step["succeeded"] == true)
        && !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error");
    diagnostics.truncate(MAX_RESULTS);
    let attempt = finish_check_attempt(&directory, &attempt, || {
        finish_attempt(root, attempt.clone(), succeeded, diagnostics.clone())
    })?;
    crate::compiled::atomic_write(
        &directory.join("check.json"),
        &serde_json::to_vec(&attempt)?,
    )?;
    Ok(
        json!({"profile":profile,"succeeded":succeeded,"steps":steps,"diagnostics":diagnostics,"checkAttempt":attempt}),
    )
}

fn fail_check_attempt(
    directory: &Path,
    mut attempt: super::BuildAttempt,
    mut diagnostics: Vec<Diagnostic>,
    error: &anyhow::Error,
) -> Result<()> {
    attempt.finished_at = Some(super::now()?);
    attempt.status = "failed".into();
    diagnostics.truncate(MAX_RESULTS.saturating_sub(1));
    diagnostics.push(Diagnostic::error(
        "inspection",
        error
            .downcast_ref::<revenant_core::Error>()
            .map(|error| error.code.as_str())
            .unwrap_or("inspection_failed"),
        format!("{error:#}"),
    ));
    attempt.diagnostics = diagnostics;
    crate::compiled::atomic_write(
        &directory.join("check.json"),
        &serde_json::to_vec(&attempt)?,
    )?;
    Ok(())
}
