//! Process-leased ownership of disposable SQLite stores. Recovery never infers
//! ownership from a name or PID: both the record and a released OS lease matter.

mod journal;

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, Metadata, OpenOptions, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const RECORD: &str = "revenant-owner.json";
const LEASE: &str = "revenant-owner.lock";
const MAX_ENTRIES: u32 = 256;
const MAX_RECLAIMED: u32 = 64;
const MAX_ISSUES: usize = 16;
const STARTUP_BUDGET: Duration = Duration::from_millis(100);
const STORE_FILES: &[&str] = &[
    "store.sqlite3",
    "store.sqlite3-journal",
    "store.sqlite3-wal",
    "store.sqlite3-shm",
];

/// One store preserved by the startup recovery pass.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeStoreIssue {
    /// Native path of the store or cache directory that needs a later retry.
    pub path: PathBuf,
    /// Stable classification such as `lease_busy`, `store_io`, or `store_contents`.
    pub code: String,
    /// Explanation of why this pass retained the path.
    pub message: String,
}

/// Bounded startup recovery results for disposable native index/result stores.
///
/// The pass examines at most 256 registered stores, reclaims at most 64, and stops
/// starting work after 100 ms. Individual filesystem calls are not preemptible.
/// Diagnostics retain at most 16 issues; counts still cover every examined store.
/// A persisted journal cursor provides progress across startups. Its explicitly
/// marked metadata remains below the configured cache root. Unrelated paths and
/// unmarked legacy directories are never enumerated, adopted, or removed.
/// Interrupted compaction may replay a record; examination/retention counts
/// describe visits rather than a deduplicated census of all live stores.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeStoreRecovery {
    /// Registered store records examined, including removed/partial records.
    pub examined: u32,
    /// Marked stores reclaimed while holding their released process lease.
    pub reclaimed: u32,
    /// Marked stores with a lease still held by another handle or process.
    pub active: u32,
    /// Registered paths with partial, unsupported, or linked ownership preserved.
    pub preserved: u32,
    /// Marked stores that could not be inspected or removed; retry next startup.
    pub deferred: u32,
    /// The entry, reclamation, or elapsed-time budget ended this pass early.
    pub incomplete: bool,
    /// Bounded diagnostics for active or deferred stores.
    pub issues: Vec<NativeStoreIssue>,
}

impl NativeStoreRecovery {
    fn issue(&mut self, path: &Path, code: &str, message: impl ToString) {
        if self.issues.len() < MAX_ISSUES {
            self.issues.push(NativeStoreIssue {
                path: path.to_owned(),
                code: code.to_owned(),
                message: message.to_string(),
            });
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ownership {
    format: u32,
    owner: String,
    kind: String,
    directory: String,
    lease: String,
    // Diagnostic only. The OS lease, never PID reuse or process enumeration,
    // proves that every owner of this particular store has exited.
    pid: u32,
}

pub(crate) struct OwnedStore {
    path: PathBuf,
    lease: Option<File>,
}

impl OwnedStore {
    pub(crate) fn create(cache: &Path, prefix: &str) -> Result<Self> {
        let cache = std::path::absolute(cache).map_err(io_error)?;
        std::fs::create_dir_all(&cache).map_err(io_error)?;
        require_directory(&cache).map_err(io_error)?;
        let kind = match prefix {
            "revenant-index-" => "index",
            "revenant-results-" => "results",
            _ => return Err(Error::new("native_store", "Unsupported native store kind")),
        };
        let token = uuid::Uuid::new_v4().to_string();
        let name = format!("{prefix}{token}");
        let path = cache.join(&name);
        std::fs::create_dir(&path).map_err(io_error)?;
        let initialized = (|| -> std::io::Result<File> {
            let mut lease = File::create_new(path.join(LEASE))?;
            lease.try_lock().map_err(std::io::Error::from)?;
            lease.write_all(token.as_bytes())?;
            lease.sync_all()?;
            // Register under the coordinator lease before publishing ownership.
            // A killed creator cannot leave a valid owned store outside the queue.
            journal::Journal::open(&cache, false)?.append(&name)?;
            let record = Ownership {
                format: 2,
                owner: "revenant-desktop".into(),
                kind: kind.into(),
                directory: name,
                lease: token,
                pid: std::process::id(),
            };
            let bytes = serde_json::to_vec(&record)?;
            let mut file = File::create_new(path.join(RECORD))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(lease)
        })();
        match initialized {
            Ok(lease) => Ok(Self {
                path,
                lease: Some(lease),
            }),
            Err(error) => {
                // This directory has not been published to SQLite or any owner.
                let _ = std::fs::remove_dir_all(&path);
                Err(io_error(error))
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedStore {
    fn drop(&mut self) {
        if let Err(error) = remove_store(&self.path, &mut self.lease) {
            eprintln!(
                "Revenant native store cleanup retained {}: {error}",
                self.path.display()
            );
        }
    }
}

pub(crate) fn recover(cache: &Path) -> NativeStoreRecovery {
    recover_with_budget(cache, MAX_ENTRIES, MAX_RECLAIMED, STARTUP_BUDGET)
}

fn recover_with_budget(
    cache: &Path,
    entries: u32,
    stores: u32,
    budget: Duration,
) -> NativeStoreRecovery {
    let mut report = NativeStoreRecovery::default();
    let started = Instant::now();
    let cache = match std::path::absolute(cache) {
        Ok(cache) => cache,
        Err(error) => {
            report.deferred += 1;
            report.issue(cache, "store_io", error);
            return report;
        }
    };
    let mut journal = match journal::Journal::open(&cache, true) {
        Ok(journal) => journal,
        Err(error) => {
            report.deferred += 1;
            report.incomplete = true;
            report.issue(&cache.join(journal::DIRECTORY), "recovery_journal", error);
            return report;
        }
    };
    if started.elapsed() >= budget {
        report.incomplete = true;
        return report;
    }
    if let Err(error) = journal.begin() {
        report.deferred += 1;
        report.incomplete = true;
        report.issue(&cache.join(journal::DIRECTORY), "recovery_journal", error);
        return report;
    }
    let mut complete = false;
    loop {
        if report.examined >= entries || report.reclaimed >= stores || started.elapsed() >= budget {
            report.incomplete = true;
            break;
        }
        let entry = match journal.next() {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                complete = true;
                break;
            }
            Err(error) => {
                report.deferred += 1;
                report.incomplete = true;
                report.issue(&cache.join(journal::DIRECTORY), "recovery_journal", error);
                break;
            }
        };
        report.examined += 1;
        let Some(name) = entry.name else {
            report.preserved += 1;
            journal.advance(entry.end);
            continue;
        };
        let path = cache.join(&name);
        let retain = recover_one(&path, &mut report);
        if retain && let Err(error) = journal.append(&name) {
            // Cursor publication cannot outrun the copied ownership reference.
            report.deferred += 1;
            report.incomplete = true;
            report.issue(&cache.join(journal::DIRECTORY), "recovery_journal", error);
            break;
        }
        journal.advance(entry.end);
    }
    if let Err(error) = journal.finish(complete) {
        report.deferred += 1;
        report.incomplete = true;
        report.issue(&cache.join(journal::DIRECTORY), "recovery_journal", error);
    }
    report
}

// Whether the registered name must be visited on the next compaction cycle.
fn recover_one(path: &Path, report: &mut NativeStoreRecovery) -> bool {
    let record = match read_ownership(path) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let metadata = match std::fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
                Err(error) => {
                    report.deferred += 1;
                    report.issue(path, "store_io", error);
                    return true;
                }
            };
            if !metadata.is_dir() || is_link(&metadata) {
                report.preserved += 1;
                return true;
            }
            // Registration precedes marker publication. A concurrent creator
            // may still be filling its marker while holding this lease.
            if let Ok(lease) = open_lease(path) {
                match lease.try_lock() {
                    Err(TryLockError::WouldBlock) => {
                        report.active += 1;
                        report.issue(
                            path,
                            "lease_busy",
                            "A store creator still holds the lease before marker publication",
                        );
                        return true;
                    }
                    Err(TryLockError::Error(error)) => {
                        report.deferred += 1;
                        report.issue(path, "store_io", error);
                        return true;
                    }
                    Ok(()) => (),
                }
            }
            report.preserved += 1;
            return true;
        }
        Err(error) => {
            report.deferred += 1;
            report.issue(path, "store_io", error);
            return true;
        }
    };
    let mut lease = match open_lease(path) {
        Ok(lease) => lease,
        Err(error) => {
            report.deferred += 1;
            report.issue(path, "store_io", error);
            return true;
        }
    };
    match lease.try_lock() {
        Ok(()) => (),
        Err(TryLockError::WouldBlock) => {
            report.active += 1;
            report.issue(
                path,
                "lease_busy",
                "An owner or another recovery pass still holds the store lease",
            );
            return true;
        }
        Err(TryLockError::Error(error)) => {
            report.deferred += 1;
            report.issue(path, "store_io", error);
            return true;
        }
    }
    // Another recovery pass may have removed the path after we opened the
    // old inode. Re-read its marker under the lease before removing anything.
    let verified = (|| -> std::io::Result<bool> {
        let Some(current) = read_ownership(path)? else {
            return Ok(false);
        };
        let mut token = String::new();
        (&mut lease).take(128).read_to_string(&mut token)?;
        Ok(current.lease == record.lease && token == current.lease)
    })();
    match verified {
        Ok(true) => (),
        Ok(false) => {
            report.preserved += 1;
            return path.exists();
        }
        Err(error) => {
            report.deferred += 1;
            report.issue(path, "store_io", error);
            return true;
        }
    }
    let mut lease = Some(lease);
    match remove_store(path, &mut lease) {
        Ok(()) => {
            report.reclaimed += 1;
            false
        }
        Err(error) => {
            report.deferred += 1;
            report.issue(path, "store_contents", error);
            true
        }
    }
}

fn read_ownership(path: &Path) -> std::io::Result<Option<Ownership>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || is_link(&metadata) {
        return Ok(None);
    }
    let marker = path.join(RECORD);
    let metadata = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > 4096 {
        return Ok(None);
    }
    let bytes = std::fs::read(marker)?;
    let Ok(record) = serde_json::from_slice::<Ownership>(&bytes) else {
        return Ok(None);
    };
    let prefix = match record.kind.as_str() {
        "index" => "revenant-index-",
        "results" => "revenant-results-",
        _ => return Ok(None),
    };
    if record.format != 2
        || record.owner != "revenant-desktop"
        || uuid::Uuid::parse_str(&record.lease).is_err()
        || record.directory != format!("{prefix}{}", record.lease)
        || path.file_name().and_then(|name| name.to_str()) != Some(&record.directory)
    {
        return Ok(None);
    }
    Ok(Some(record))
}

fn open_lease(path: &Path) -> std::io::Result<File> {
    let path = path.join(LEASE);
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > 128 {
        return Err(std::io::Error::other(
            "Store lease is not a regular ownership file",
        ));
    }
    OpenOptions::new().read(true).write(true).open(path)
}

fn remove_store(path: &Path, lease: &mut Option<File>) -> std::io::Result<()> {
    require_directory(path)?;
    // SQLite stores are deliberately flat. Refuse all unexpected paths, even
    // under a valid ownership marker, so originals and linked data cannot enter
    // cleanup through accidental cache-directory reuse.
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let name = entry.file_name();
        let allowed = name
            .to_str()
            .is_some_and(|name| name == RECORD || name == LEASE || STORE_FILES.contains(&name));
        if !allowed || !metadata.is_file() || is_link(&metadata) {
            return Err(std::io::Error::other(
                "Owned store contains an unexpected path; retained for inspection",
            ));
        }
    }
    // Keep the record/lease until all database files have been removed. A
    // sharing violation leaves proof of ownership for the next startup retry.
    for name in STORE_FILES {
        match std::fs::remove_file(path.join(name)) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    let record = std::fs::read(path.join(RECORD))?;
    std::fs::remove_file(path.join(RECORD))?;
    if let Err(error) = std::fs::remove_file(path.join(LEASE)) {
        // For example, antivirus or another handle can deny deletion of just
        // the lease file. Restore the marker while our original lease is held.
        restore_record(path, &record).map_err(|restore| {
            std::io::Error::other(format!("{error}; ownership recovery failed: {restore}"))
        })?;
        return Err(error);
    }
    // Windows may retain a delete-pending lease until its final handle closes.
    drop(lease.take());
    if let Err(error) = std::fs::remove_dir(path) {
        // At this point only an empty store directory should remain. Retain a
        // fresh leased marker if its removal is temporarily denied so a later
        // startup can retry instead of silently turning it into legacy state.
        let restored = (|| -> std::io::Result<()> {
            require_directory(path)?;
            let ownership: Ownership = serde_json::from_slice(&record)?;
            let mut file = File::create_new(path.join(LEASE))?;
            file.try_lock().map_err(std::io::Error::from)?;
            file.write_all(ownership.lease.as_bytes())?;
            file.sync_all()?;
            *lease = Some(file);
            restore_record(path, &record)
        })();
        if let Err(restore) = restored {
            return Err(std::io::Error::other(format!(
                "{error}; ownership recovery failed: {restore}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn restore_record(path: &Path, record: &[u8]) -> std::io::Result<()> {
    let mut file = File::create_new(path.join(RECORD))?;
    file.write_all(record)?;
    file.sync_all()
}

fn require_directory(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(std::io::Error::other(
            "Native cache/store path must be a real directory",
        ));
    }
    Ok(())
}

fn is_link(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn io_error(error: std::io::Error) -> Error {
    Error::new("native_store", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abandoned(cache: &Path, prefix: &str) -> PathBuf {
        let mut owned = OwnedStore::create(cache, prefix).unwrap();
        let path = owned.path().to_owned();
        std::fs::write(path.join("store.sqlite3"), b"abandoned database fixture").unwrap();
        // Simulate the filesystem state after process termination. Integration
        // fixtures separately exercise actual OS termination and lock release.
        drop(owned.lease.take());
        std::mem::forget(owned);
        path
    }

    #[test]
    fn owner_record_and_released_lease_are_both_required() {
        let cache = tempfile::tempdir().unwrap();
        let live = OwnedStore::create(cache.path(), "revenant-index-").unwrap();
        let stale = abandoned(cache.path(), "revenant-results-");
        let legacy = cache.path().join("revenant-index-unmarked-legacy");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("store.sqlite3"), b"legacy").unwrap();
        let unrelated = cache.path().join("original.jpg");
        std::fs::write(&unrelated, b"original").unwrap();

        let report = recover(cache.path());
        assert_eq!(report.reclaimed, 1);
        assert_eq!(report.active, 1);
        assert_eq!(report.preserved, 0); // Unregistered originals/legacy are not enumerated.
        assert!(report.issues.iter().any(|issue| issue.code == "lease_busy"));
        assert!(live.path().exists());
        assert!(!stale.exists());
        assert_eq!(
            std::fs::read(legacy.join("store.sqlite3")).unwrap(),
            b"legacy"
        );
        assert_eq!(std::fs::read(unrelated).unwrap(), b"original");
        drop(live);
        assert_eq!(recover(cache.path()).reclaimed, 0);
    }

    #[test]
    fn pid_reuse_and_forged_or_partial_markers_do_not_grant_ownership() {
        let cache = tempfile::tempdir().unwrap();
        let live = OwnedStore::create(cache.path(), "revenant-index-").unwrap();
        let mut record = read_ownership(live.path()).unwrap().unwrap();
        record.pid = u32::MAX;
        std::fs::write(
            live.path().join(RECORD),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let copied = cache
            .path()
            .join("revenant-index-00000000-0000-0000-0000-000000000000");
        std::fs::create_dir(&copied).unwrap();
        std::fs::write(copied.join(RECORD), serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::write(copied.join("store.sqlite3"), b"unrelated").unwrap();
        let partial = cache.path().join("revenant-results-partial");
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join(RECORD), b"{\"format\":1").unwrap();

        let report = recover(cache.path());
        assert_eq!(report.active, 1);
        assert_eq!(report.reclaimed, 0);
        assert_eq!(report.preserved, 0);
        assert!(copied.join("store.sqlite3").exists());
        assert!(partial.exists());
    }

    #[test]
    fn unknown_children_are_preserved_and_failed_cleanup_is_retried() {
        let cache = tempfile::tempdir().unwrap();
        let stale = abandoned(cache.path(), "revenant-index-");
        let original = stale.join("original.png");
        std::fs::write(&original, b"original").unwrap();
        let report = recover(cache.path());
        assert_eq!(report.deferred, 1);
        assert_eq!(std::fs::read(&original).unwrap(), b"original");
        assert!(stale.join("store.sqlite3").exists());
        std::fs::remove_file(original).unwrap();
        assert_eq!(recover(cache.path()).reclaimed, 1);
        assert!(!stale.exists());
    }

    #[test]
    fn pass_bounds_are_visible_and_later_passes_recover_remaining_stores() {
        let cache = tempfile::tempdir().unwrap();
        for _ in 0..5 {
            abandoned(cache.path(), "revenant-results-");
        }
        let first = recover_with_budget(cache.path(), 2, 2, Duration::from_secs(30));
        assert_eq!(first.examined, 2);
        assert_eq!(first.reclaimed, 2);
        assert!(first.incomplete);
        assert_eq!(recover(cache.path()).reclaimed, 3);
        assert!(!recover(cache.path()).incomplete);
        abandoned(cache.path(), "revenant-index-");
        let expired = recover_with_budget(cache.path(), 256, 64, Duration::ZERO);
        assert_eq!(expired.examined, 0);
        assert!(expired.incomplete);
        assert_eq!(recover(cache.path()).reclaimed, 1);
    }

    #[test]
    fn a_replaced_lease_file_does_not_prove_abandonment() {
        let cache = tempfile::tempdir().unwrap();
        let stale = abandoned(cache.path(), "revenant-results-");
        std::fs::write(stale.join(LEASE), b"foreign lease").unwrap();
        let report = recover(cache.path());
        assert_eq!(report.reclaimed, 0);
        assert_eq!(report.preserved, 1);
        assert!(stale.join("store.sqlite3").exists());
    }

    #[test]
    fn permanent_cache_entries_and_live_stores_cannot_starve_later_registrations() {
        let cache = tempfile::tempdir().unwrap();
        for index in 0..300 {
            let legacy = cache.path().join(format!("aaa-legacy-{index:04}"));
            std::fs::create_dir(&legacy).unwrap();
            std::fs::write(legacy.join("original.dat"), b"unmarked original").unwrap();
        }
        let live: Vec<_> = (0..257)
            .map(|_| OwnedStore::create(cache.path(), "revenant-index-").unwrap())
            .collect();
        let first = recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30));
        assert_eq!(first.examined, 256);
        assert_eq!(first.active, 256);
        assert!(first.incomplete);
        // A second instance can register into the destination journal between
        // passes of an unfinished compaction; the source remains a finite queue.
        let stale = abandoned(cache.path(), "revenant-results-");
        let mut reclaimed = 0;
        for _ in 0..4 {
            let pass = recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30));
            assert!(pass.examined <= 256);
            reclaimed += pass.reclaimed;
            if !stale.exists() {
                break;
            }
        }
        assert_eq!(reclaimed, 1);
        assert!(
            !stale.exists(),
            "Cursor failed to reach a later abandoned store"
        );
        for owned in &live {
            assert!(owned.path().exists());
        }
        for index in 0..300 {
            assert_eq!(
                std::fs::read(
                    cache
                        .path()
                        .join(format!("aaa-legacy-{index:04}/original.dat"))
                )
                .unwrap(),
                b"unmarked original"
            );
        }
    }

    #[test]
    fn registration_before_marker_publication_survives_a_recovery_pass() {
        let cache = tempfile::tempdir().unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let name = format!("revenant-index-{token}");
        let path = cache.path().join(&name);
        std::fs::create_dir(&path).unwrap();
        let mut lease = File::create_new(path.join(LEASE)).unwrap();
        lease.try_lock().unwrap();
        lease.write_all(token.as_bytes()).unwrap();
        lease.sync_all().unwrap();
        journal::Journal::open(cache.path(), false)
            .unwrap()
            .append(&name)
            .unwrap();
        let during = recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30));
        assert_eq!(during.active, 1);
        assert!(path.exists());
        let record = Ownership {
            format: 2,
            owner: "revenant-desktop".into(),
            kind: "index".into(),
            directory: name,
            lease: token,
            pid: std::process::id(),
        };
        std::fs::write(path.join(RECORD), serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::write(path.join("store.sqlite3"), b"abandoned").unwrap();
        drop(lease);
        assert_eq!(
            recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30)).reclaimed,
            1
        );
    }

    #[test]
    fn concurrent_recovery_coordinators_report_contention_without_losing_progress() {
        let cache = tempfile::tempdir().unwrap();
        let stale = abandoned(cache.path(), "revenant-results-");
        let held = journal::Journal::open(cache.path(), false).unwrap();
        let busy = recover(cache.path());
        assert_eq!(busy.deferred, 1);
        assert!(busy.incomplete);
        assert!(stale.exists());
        drop(held);
        assert_eq!(
            recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30)).reclaimed,
            1
        );
    }

    #[test]
    fn unrelated_recovery_metadata_paths_are_preserved_instead_of_prefix_cleaned() {
        let cache = tempfile::tempdir().unwrap();
        let stale = abandoned(cache.path(), "revenant-results-");
        let original = cache
            .path()
            .join(journal::DIRECTORY)
            .join(".state-original.tmp");
        std::fs::write(&original, b"unrelated original").unwrap();
        let blocked = recover(cache.path());
        assert_eq!(blocked.deferred, 1);
        assert_eq!(std::fs::read(&original).unwrap(), b"unrelated original");
        assert!(stale.join("store.sqlite3").exists());
        std::fs::remove_file(original).unwrap();
        assert_eq!(
            recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30)).reclaimed,
            1
        );
    }

    #[test]
    fn linked_directories_and_linked_children_never_enter_cleanup() {
        let cache = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(target.path().join("original.jpg"), b"original").unwrap();
        let link = cache.path().join("revenant-index-linked");
        #[cfg(unix)]
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        #[cfg(windows)]
        {
            // Junction creation does not require symbolic-link privileges.
            let status = std::process::Command::new("cmd.exe")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(&link)
                .arg(target.path())
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "Cannot create reparse-point fixture");
        }
        // Finish the empty journal cycle before registering the later store.
        // A bounded startup can leave that cycle pending under I/O contention;
        // the next pass would finish it before visiting new registrations.
        let initial = recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30));
        assert!(!initial.incomplete, "{initial:?}");
        assert_eq!(initial.preserved, 0);
        assert!(link.exists());
        assert_eq!(
            std::fs::read(target.path().join("original.jpg")).unwrap(),
            b"original"
        );
        let stale = abandoned(cache.path(), "revenant-results-");
        #[cfg(unix)]
        std::os::unix::fs::symlink(target.path(), stale.join("unexpected")).unwrap();
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd.exe")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(stale.join("unexpected"))
                .arg(target.path())
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        }
        // This assertion checks contents classification, independent of the
        // bounded startup deadline while other native tests compete for I/O.
        assert_eq!(
            recover_with_budget(cache.path(), 256, 64, Duration::from_secs(30)).deferred,
            1
        );
        assert!(stale.join("store.sqlite3").exists());
        assert!(target.path().join("original.jpg").exists());
    }
}
