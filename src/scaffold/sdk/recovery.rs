//! Journaled SDK replacement; only fixed framework-owned paths are moved.
use super::{check, check_sources, materialize};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MANAGED: &[&str] = &[
    ".revenant/sdk",
    ".revenant/desktop",
    "web/node_modules",
    "web/package-lock.json",
    "web/npm-shrinkwrap.json",
    "web/pnpm-lock.yaml",
    "web/yarn.lock",
    "web/bun.lock",
    "web/bun.lockb",
    "native/Cargo.lock",
    "web/src/lib/.revenant",
    "web/src/lib/revenant.ts",
];

#[derive(Serialize, Deserialize)]
struct Backup {
    format: u32,
    root: PathBuf,
    state: String,
    entries: Vec<BackupEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore: Option<PathBuf>,
}
#[derive(Serialize, Deserialize)]
struct BackupEntry {
    relative: String,
    present: bool,
}

#[derive(Serialize, Deserialize)]
struct RestoreRecord {
    selected: PathBuf,
    inverse: PathBuf,
    phase: String,
    // Each intent is persisted before its rename. A missing source and present
    // application path identifies a rename completed just before a forced exit.
    transfers: Vec<String>,
    completed: usize,
}

fn restore_journal(record: &RestoreRecord) -> Result<()> {
    crate::compiled::atomic_write(
        &record.inverse.join("restore.json"),
        &serde_json::to_vec_pretty(record)?,
    )
}

fn stamp() -> Result<String> {
    Ok(format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ))
}
fn journal(directory: &Path, backup: &Backup) -> Result<()> {
    crate::compiled::atomic_write(
        &directory.join("backup.json"),
        &serde_json::to_vec_pretty(backup)?,
    )
}
fn operation_lock(root: &Path) -> Result<fs::File> {
    let directory = root.join(".revenant");
    if exists(&directory)? {
        ensure!(
            !is_link(&fs::symlink_metadata(&directory)?),
            "SDK operation directory is a link"
        );
    }
    fs::create_dir_all(&directory)?;
    let lock = directory.join("sdk-operation.lock");
    if exists(&lock)? {
        ensure!(
            !is_link(&fs::symlink_metadata(&lock)?),
            "SDK operation lease is a link"
        );
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock)?;
    file.try_lock()
        .context("another SDK upgrade or restore is in progress")?;
    Ok(file)
}

fn backup_directory(root: &Path) -> Result<PathBuf> {
    let path = root.join(".revenant/sdk-backups");
    if exists(&path)? {
        ensure!(
            !is_link(&fs::symlink_metadata(&path)?),
            "SDK backup directory is a link"
        );
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn checked_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(MANAGED.contains(&relative), "unowned SDK transaction path");
    let path = root.join(relative);
    let mut ancestor = path.parent();
    while let Some(parent) = ancestor {
        if parent == root {
            break;
        }
        if exists(parent)? {
            ensure!(
                !is_link(&fs::symlink_metadata(parent)?),
                "SDK transaction parent is a link: {}",
                parent.display()
            );
        }
        ancestor = parent.parent();
    }
    Ok(path)
}
fn remove_owned(root: &Path, relative: &str) -> Result<()> {
    let path = checked_path(root, relative)?;
    if !exists(&path)? {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.is_dir() && !is_link(&metadata) {
        fs::remove_dir_all(&path)?;
    } else if metadata.is_dir() {
        fs::remove_dir(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    Ok(())
}

fn begin_backup(root: &Path, directory: &Path) -> Result<Backup> {
    // Journal the full original inventory before the first rename, so a forced
    // exit between renames can be recovered without assuming anything vanished.
    let entries = MANAGED
        .iter()
        .map(|relative| {
            Ok(BackupEntry {
                relative: (*relative).into(),
                present: exists(&checked_path(root, relative)?)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir(directory)?;
    let backup = Backup {
        format: 1,
        root: root.to_path_buf(),
        state: "saving".into(),
        entries,
        restore: None,
    };
    journal(directory, &backup)?;
    Ok(backup)
}

fn move_current(root: &Path, directory: &Path, backup: &mut Backup) -> Result<()> {
    for entry in &backup.entries {
        if entry.present {
            let saved = directory.join("state").join(&entry.relative);
            fs::create_dir_all(saved.parent().context("backup parent")?)?;
            fs::rename(checked_path(root, &entry.relative)?, &saved)
                .with_context(|| format!("back up {}", entry.relative))?;
        }
    }
    backup.state = "saved".into();
    journal(directory, backup)
}

fn save_current(root: &Path, directory: &Path) -> Result<Backup> {
    let mut backup = begin_backup(root, directory)?;
    let result = move_current(root, directory, &mut backup);
    if let Err(error) = result {
        let recovery = rollback(root, directory, &backup);
        return Err(combine_recovery(error, recovery, directory));
    }
    Ok(backup)
}

fn rollback(root: &Path, directory: &Path, backup: &Backup) -> Result<()> {
    for entry in backup.entries.iter().rev() {
        let original = checked_path(root, &entry.relative)?;
        let saved = directory.join("state").join(&entry.relative);
        if entry.present && exists(&saved)? {
            remove_owned(root, &entry.relative)?;
            fs::create_dir_all(original.parent().context("rollback parent")?)?;
            fs::rename(&saved, &original)
                .with_context(|| format!("restore {}", original.display()))?;
        } else if entry.present {
            ensure!(
                exists(&original)?,
                "original and backup are both missing: {}",
                entry.relative
            );
        } else {
            remove_owned(root, &entry.relative)?;
        }
    }
    let mut restored = read_backup(directory)?;
    restored.state = "rolled-back".into();
    journal(directory, &restored)
}

fn combine_recovery(error: anyhow::Error, recovery: Result<()>, directory: &Path) -> anyhow::Error {
    match recovery {
        Ok(()) => error.context("SDK replacement failed; previous SDK, installation, lockfiles and contracts were restored"),
        Err(recovery) => anyhow::anyhow!("SDK replacement failed: {error:#}; recovery also failed: {recovery:#}. Backup retained at {}; stop development and run `revenant sdk restore {}`", directory.display(), directory.display()),
    }
}

fn read_backup(directory: &Path) -> Result<Backup> {
    Ok(serde_json::from_slice(&fs::read(
        directory.join("backup.json"),
    )?)?)
}

fn owned_backup(root: &Path, directory: &Path) -> Result<PathBuf> {
    let path = if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        root.join(directory)
    };
    ensure!(
        !is_link(&fs::symlink_metadata(&path)?),
        "SDK backup is a link"
    );
    let path = dunce::canonicalize(path)?;
    ensure!(
        path.parent() == Some(dunce::canonicalize(backup_directory(root)?)?.as_path()),
        "restore requires a direct, owned SDK backup in this application"
    );
    Ok(path)
}

fn validate_inventory(root: &Path, backup: &Backup) -> Result<()> {
    ensure!(
        backup.format == 1 && backup.root == root,
        "backup belongs to another application or format"
    );
    ensure!(
        backup.entries.len() == MANAGED.len()
            && backup
                .entries
                .iter()
                .map(|entry| entry.relative.as_str())
                .eq(MANAGED.iter().copied()),
        "backup contains an incomplete or unowned inventory"
    );
    Ok(())
}

fn begin_restore(
    root: &Path,
    selected: &Path,
    prior: &mut Backup,
) -> Result<(Backup, RestoreRecord)> {
    let inverse = backup_directory(root)?.join(stamp()?);
    let mut current = begin_backup(root, &inverse)?;
    let record = RestoreRecord {
        selected: selected.into(),
        inverse: inverse.clone(),
        phase: "saving".into(),
        transfers: Vec::new(),
        completed: 0,
    };
    // Establish both links before moving even the current installation. Either
    // backup can discover the transaction after a crash during save or transfer.
    restore_journal(&record)?;
    current.restore = Some(inverse.clone());
    journal(&inverse, &current)?;
    prior.restore = Some(inverse);
    prior.state = "restore-source".into();
    journal(selected, prior)?;
    Ok((current, record))
}

fn recover_restore(root: &Path, transaction: &Path) -> Result<RestoreRecord> {
    let transaction = owned_backup(root, transaction)?;
    let mut record: RestoreRecord =
        serde_json::from_slice(&fs::read(transaction.join("restore.json"))?)?;
    ensure!(
        owned_backup(root, &record.inverse)? == transaction
            && owned_backup(root, &record.selected)? != transaction,
        "invalid restore backup relationship"
    );
    let mut prior = read_backup(&record.selected)?;
    let mut current = read_backup(&record.inverse)?;
    validate_inventory(root, &prior)?;
    validate_inventory(root, &current)?;
    ensure!(
        [
            "saving",
            "transferring",
            "returning",
            "returned",
            "rolled-back",
            "committed"
        ]
        .contains(&record.phase.as_str())
            && record.completed <= record.transfers.len(),
        "invalid restore transaction phase"
    );
    let mut previous = None;
    for relative in &record.transfers {
        let index = MANAGED
            .iter()
            .position(|entry| entry == relative)
            .context("unowned restore transfer")?;
        ensure!(
            prior.entries[index].present && previous.is_none_or(|previous| index > previous),
            "invalid or repeated restore transfer"
        );
        previous = Some(index);
    }
    if record.phase == "committed" {
        ensure!(
            record.completed == record.transfers.len()
                && record.transfers.len()
                    == prior.entries.iter().filter(|entry| entry.present).count(),
            "committed restore has incomplete transfer progress"
        );
        // Commit is journaled before ancillary backup states. Finish a commit
        // interrupted by process exit instead of deleting either stored state.
        prior.state = "restored".into();
        prior.restore = None;
        journal(&record.selected, &prior)?;
        current.state = "complete".into();
        current.restore = None;
        journal(&record.inverse, &current)?;
        return Ok(record);
    }
    if !matches!(record.phase.as_str(), "returned" | "rolled-back") {
        record.phase = "returning".into();
        restore_journal(&record)?;
        for relative in record.transfers.iter().rev() {
            let saved = record.selected.join("state").join(relative);
            let original = checked_path(root, relative)?;
            if exists(&saved)? {
                ensure!(
                    !exists(&original)?,
                    "restore recovery found both source and application copies: {relative}; both are preserved"
                );
            } else {
                ensure!(
                    exists(&original)?,
                    "restore transfer is missing from source and application: {relative}"
                );
                fs::create_dir_all(saved.parent().context("restore return parent")?)?;
                fs::rename(original, saved)?;
            }
        }
        // The displaced installation must stay archived until every transferred
        // old path is safely back in the selected backup. Persist this boundary
        // before rollback starts so a second interruption cannot confuse copies.
        record.phase = "returned".into();
        restore_journal(&record)?;
    }
    for entry in &prior.entries {
        ensure!(
            !entry.present || exists(&record.selected.join("state").join(&entry.relative))?,
            "restore source remains incomplete: {}",
            entry.relative
        );
    }
    if record.phase != "rolled-back" {
        rollback(root, &record.inverse, &current)?;
        record.phase = "rolled-back".into();
        restore_journal(&record)?;
    }
    prior.state = "complete".into();
    prior.restore = None;
    journal(&record.selected, &prior)?;
    current = read_backup(&record.inverse)?;
    current.restore = None;
    journal(&record.inverse, &current)?;
    Ok(record)
}

fn replace(
    root: &Path,
    replacement: &Path,
    validate: impl FnOnce() -> Result<()>,
) -> Result<PathBuf> {
    let directory = backup_directory(root)?.join(stamp()?);
    let mut backup = save_current(root, &directory)?;
    let result = (|| -> Result<()> {
        fs::rename(replacement, root.join(".revenant/sdk"))?;
        validate()?;
        backup.state = "complete".into();
        journal(&directory, &backup)
    })();
    if let Err(error) = result {
        return Err(combine_recovery(
            error,
            rollback(root, &directory, &backup),
            &directory,
        ));
    }
    Ok(directory)
}

/// Back up and replace SDK/dependencies/locks/contracts, validating native output.
/// The previous state is restored on installation, compilation or export errors.
/// Handwritten frontend and native source configuration is never migrated here.
pub fn upgrade(
    root: &Path,
    config: &crate::config::RevenantConfig,
    verbose: bool,
) -> Result<PathBuf> {
    let root = dunce::canonicalize(root)?;
    let _lease = operation_lock(&root)?;
    let project_lease = crate::inspection::acquire_project_lock(&root)?;
    println!(
        "SDK upgrade backs up the current SDK, dependency installation, lockfiles and published contracts. Dependencies will be resolved afresh from the unchanged frontend package configuration."
    );
    let staging = root
        .join(".revenant")
        .join(format!("sdk-stage-{}", stamp()?));
    fs::create_dir(&staging)?;
    let result = (|| -> Result<PathBuf> {
        materialize(&staging)?;
        check_sources(&staging)?;
        replace(&root, &staging.join(".revenant/sdk"), || {
            crate::commands::build::ensure_frontend_with_project_lock(
                &root,
                config,
                verbose,
                &project_lease,
            )?;
            // The host consumes the author library as a path dependency and
            // does not produce that library's independent lockfile.
            let native = root.join("native");
            if native.join("Cargo.toml").is_file() {
                let (resolved, _, stderr) = crate::inspection::bounded_process(
                    &native,
                    "cargo",
                    &["generate-lockfile"],
                    &[],
                    std::time::Duration::from_secs(300),
                )
                .context("resolve the replacement native library lockfile")?;
                if verbose {
                    eprint!("{stderr}");
                }
                ensure!(resolved, "resolve native library lockfile failed: {stderr}");
            }
            let build = crate::toolchain::desktop::DesktopBuild::with_project_lock(
                &root,
                config,
                false,
                project_lease.try_clone()?,
            )?;
            build.build(verbose)?;
            build.publish(&root, verbose)?;
            check(&root)
        })
    })();
    let cleanup = fs::remove_dir_all(&staging);
    match (result, cleanup) {
        (Ok(backup), Ok(())) => Ok(backup),
        (Ok(backup), Err(error)) => {
            eprintln!(
                "SDK upgraded; remove abandoned staging {}: {error}. Backup: {}",
                staging.display(),
                backup.display()
            );
            Ok(backup)
        }
        (Err(error), _) => Err(error),
    }
}

/// Restore an owned backup and save the displaced state in an inverse backup.
/// Restoring consumes stored paths; the inverse permits redo without copying
/// large dependency trees. An interrupted replacement is recovered in place.
pub fn restore(root: &Path, selected: &Path) -> Result<PathBuf> {
    let root = dunce::canonicalize(root)?;
    let _lease = operation_lock(&root)?;
    let _project_lease = crate::inspection::acquire_project_lock(&root)?;
    let selected = owned_backup(&root, selected)?;
    let mut prior = read_backup(&selected)?;
    validate_inventory(&root, &prior)?;
    if let Some(transaction) = &prior.restore {
        let recovered = recover_restore(&root, transaction)?;
        if selected == recovered.selected && recovered.phase == "committed" {
            return Ok(recovered.inverse);
        }
        if selected == recovered.inverse && recovered.phase == "rolled-back" {
            return Ok(recovered.selected);
        }
        prior = read_backup(&selected)?;
    }
    ensure!(
        matches!(prior.state.as_str(), "complete" | "saved" | "saving"),
        "backup is not restorable for this application"
    );
    if prior.state != "complete" {
        rollback(&root, &selected, &prior)?;
        return Ok(selected);
    }
    for entry in &prior.entries {
        ensure!(
            !entry.present || exists(&selected.join("state").join(&entry.relative))?,
            "backup is incomplete: {}",
            entry.relative
        );
    }
    let (mut current, mut record) = begin_restore(&root, &selected, &mut prior)?;
    let result = (|| -> Result<()> {
        move_current(&root, &record.inverse, &mut current)?;
        record.phase = "transferring".into();
        restore_journal(&record)?;
        for entry in &prior.entries {
            if entry.present {
                let original = checked_path(&root, &entry.relative)?;
                fs::create_dir_all(original.parent().context("restore parent")?)?;
                record.transfers.push(entry.relative.clone());
                restore_journal(&record)?;
                fs::rename(selected.join("state").join(&entry.relative), &original)?;
                record.completed += 1;
                restore_journal(&record)?;
            }
        }
        record.phase = "committed".into();
        restore_journal(&record)?;
        recover_restore(&root, &record.inverse)?;
        Ok(())
    })();
    if let Err(error) = result {
        let recovery = recover_restore(&root, &record.inverse);
        if recovery
            .as_ref()
            .is_ok_and(|record| record.phase == "committed")
        {
            return Ok(record.inverse);
        }
        return Err(combine_recovery(
            error,
            recovery.map(|_| ()),
            &record.inverse,
        ));
    }
    Ok(record.inverse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn fixture() -> TempDir {
        let temp = TempDir::new().unwrap();
        materialize(temp.path()).unwrap();
        temp
    }
    #[test]
    fn failed_replacement_restores_dependencies_locks_contracts_and_source_edits() {
        let temp = fixture();
        let root = temp.path();
        super::super::tests::install(root);
        let preserved = [
            ".revenant/sdk/local-edit.txt",
            "web/node_modules/local-edit.txt",
            "web/package-lock.json",
            "native/Cargo.lock",
            "web/src/lib/.revenant/generation/facade.ts",
            "web/src/lib/revenant.ts",
        ];
        for path in preserved {
            fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            fs::write(root.join(path), path).unwrap();
        }
        let stage = TempDir::new_in(root).unwrap();
        materialize(stage.path()).unwrap();
        let error = replace(root, &stage.path().join(".revenant/sdk"), || {
            fs::create_dir_all(root.join("web/node_modules/new")).unwrap();
            fs::write(root.join("web/package-lock.json"), "changed").unwrap();
            anyhow::bail!("injected native validation failure")
        })
        .unwrap_err();
        assert!(error.to_string().contains("restored"));
        for path in preserved {
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), path);
        }
    }
    #[test]
    fn upgrade_rejects_active_project_build_before_staging_or_backups() {
        let temp = TempDir::new().unwrap();
        crate::scaffold::create_project(temp.path(), "locked-upgrade").unwrap();
        let root = temp.path();
        let mut config = crate::config::RevenantConfig::load(root).unwrap();
        // Keep a broken implementation bounded: never install real dependencies.
        config.toolchain.pkg_manager = "revenant-missing-fixture-package-manager".into();
        let _lease = crate::inspection::acquire_project_lock(root).unwrap();
        let error = upgrade(root, &config, false).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Project native build or check already running"),
            "{error:#}"
        );
        assert!(
            !root.join(".revenant/sdk-backups").exists(),
            "rejected upgrade must not start replacement"
        );
    }

    #[test]
    fn restore_rejects_active_project_build_before_moving_inputs() {
        let temp = fixture();
        let root = temp.path();
        fs::write(root.join(".revenant/sdk/local-edit.txt"), "old SDK").unwrap();
        let stage = TempDir::new_in(root).unwrap();
        materialize(stage.path()).unwrap();
        let backup = replace(root, &stage.path().join(".revenant/sdk"), || {
            check_sources(root)
        })
        .unwrap();
        let lease = crate::inspection::acquire_project_lock(root).unwrap();
        let result = restore(root, &backup);
        assert!(
            result.is_err(),
            "restore must reject an active project transaction"
        );
        assert!(!root.join(".revenant/sdk/local-edit.txt").exists());
        assert!(backup.join("state/.revenant/sdk/local-edit.txt").is_file());
        drop(lease);
        restore(root, &backup).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(".revenant/sdk/local-edit.txt")).unwrap(),
            "old SDK"
        );
    }

    #[test]
    fn successful_replacement_and_restore_preserve_authors_and_support_redo() {
        let temp = fixture();
        let root = temp.path();
        fs::create_dir_all(root.join("web/src/routes")).unwrap();
        fs::write(root.join("web/src/routes/+page.svelte"), "author frontend").unwrap();
        fs::write(root.join(".revenant/sdk/local-edit.txt"), "old SDK").unwrap();
        let stage = TempDir::new_in(root).unwrap();
        materialize(stage.path()).unwrap();
        let backup = replace(root, &stage.path().join(".revenant/sdk"), || {
            check_sources(root)
        })
        .unwrap();
        assert!(!root.join(".revenant/sdk/local-edit.txt").exists());
        let inverse = restore(root, &backup).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(".revenant/sdk/local-edit.txt")).unwrap(),
            "old SDK"
        );
        restore(root, &inverse).unwrap();
        assert!(!root.join(".revenant/sdk/local-edit.txt").exists());
        assert_eq!(
            fs::read_to_string(root.join("web/src/routes/+page.svelte")).unwrap(),
            "author frontend"
        );
    }
    #[test]
    fn interrupted_save_and_recovery_failure_keep_a_recoverable_journal() {
        let temp = fixture();
        let root = dunce::canonicalize(temp.path()).unwrap();
        fs::create_dir_all(root.join("web/src/lib/.revenant")).unwrap();
        fs::write(root.join("web/src/lib/.revenant/facade.ts"), "old facade").unwrap();
        let directory = root.join(".revenant/sdk-backups/interrupted");
        fs::create_dir_all(&directory).unwrap();
        let entries = MANAGED
            .iter()
            .map(|relative| BackupEntry {
                relative: (*relative).into(),
                present: exists(&root.join(relative)).unwrap(),
            })
            .collect();
        let backup = Backup {
            format: 1,
            root: root.clone(),
            state: "saving".into(),
            entries,
            restore: None,
        };
        journal(&directory, &backup).unwrap();
        fs::create_dir_all(directory.join("state/.revenant")).unwrap();
        fs::rename(
            root.join(".revenant/sdk"),
            directory.join("state/.revenant/sdk"),
        )
        .unwrap();
        fs::write(
            root.join(".revenant/sdk"),
            "injected inaccessible destination",
        )
        .unwrap();
        fs::create_dir_all(directory.join("state/web/src/lib")).unwrap();
        fs::rename(
            root.join("web/src/lib/.revenant"),
            directory.join("state/web/src/lib/.revenant"),
        )
        .unwrap();
        // A file where the original parent belongs injects an actual rollback
        // filesystem failure. It leaves the saved SDK and journal intact.
        fs::remove_dir(root.join("web/src/lib")).unwrap();
        fs::write(root.join("web/src/lib"), "blocked parent").unwrap();
        assert!(rollback(&root, &directory, &backup).is_err());
        assert!(directory.join("state/.revenant/sdk/Cargo.toml").is_file());
        fs::remove_file(root.join("web/src/lib")).unwrap();
        restore(&root, &directory).unwrap();
        check_sources(&root).unwrap();
    }

    #[test]
    fn interrupted_restore_returns_old_bytes_before_recovering_the_inverse() {
        for transferred in [false, true] {
            let temp = fixture();
            let root = dunce::canonicalize(temp.path()).unwrap();
            let retained = [
                "web/node_modules/identity.txt",
                "web/package-lock.json",
                "native/Cargo.lock",
                ".revenant/desktop/Cargo.lock",
                "web/src/lib/.revenant/generation/facade.ts",
                "web/src/lib/revenant.ts",
            ];
            fs::write(root.join(".revenant/sdk/old.txt"), "old SDK").unwrap();
            for relative in retained {
                fs::create_dir_all(root.join(relative).parent().unwrap()).unwrap();
                fs::write(root.join(relative), format!("old {relative}")).unwrap();
            }
            let stage = TempDir::new_in(&root).unwrap();
            materialize(stage.path()).unwrap();
            let selected = replace(&root, &stage.path().join(".revenant/sdk"), || {
                fs::write(root.join(".revenant/sdk/new.txt"), "new SDK").unwrap();
                for relative in retained {
                    fs::create_dir_all(root.join(relative).parent().unwrap()).unwrap();
                    fs::write(root.join(relative), format!("new {relative}")).unwrap();
                }
                check_sources(&root)
            })
            .unwrap();
            let mut prior = read_backup(&selected).unwrap();
            let (mut current, mut record) = begin_restore(&root, &selected, &mut prior).unwrap();
            move_current(&root, &record.inverse, &mut current).unwrap();
            record.phase = "transferring".into();
            record.transfers.push(".revenant/sdk".into());
            restore_journal(&record).unwrap();
            if transferred {
                fs::rename(
                    selected.join("state/.revenant/sdk"),
                    root.join(".revenant/sdk"),
                )
                .unwrap();
            }
            // Deterministically model a process exit on either side of the
            // rename, before its completed-progress write. Recover via inverse.
            assert_eq!(restore(&root, &record.inverse).unwrap(), selected);
            assert_eq!(
                fs::read_to_string(root.join(".revenant/sdk/new.txt")).unwrap(),
                "new SDK"
            );
            assert_eq!(
                fs::read_to_string(selected.join("state/.revenant/sdk/old.txt")).unwrap(),
                "old SDK"
            );
            for relative in retained {
                assert_eq!(
                    fs::read_to_string(root.join(relative)).unwrap(),
                    format!("new {relative}")
                );
                assert_eq!(
                    fs::read_to_string(selected.join("state").join(relative)).unwrap(),
                    format!("old {relative}")
                );
            }
            // Retrying the original selection still restores its complete old
            // state, while an inverse backup retains the recovered new state.
            let redo = restore(&root, &selected).unwrap();
            assert_eq!(
                fs::read_to_string(root.join(".revenant/sdk/old.txt")).unwrap(),
                "old SDK"
            );
            assert_eq!(
                fs::read_to_string(redo.join("state/.revenant/sdk/new.txt")).unwrap(),
                "new SDK"
            );
        }
    }
}
