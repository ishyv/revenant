//! Seekable registration and compaction under one stable process lease. Directory
//! enumeration cannot resume portably across restarts, so it is not our queue.

use super::{is_link, require_directory};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) const DIRECTORY: &str = ".revenant-native-stores";
const MARKER: &str = "owner.json";
const LEASE: &str = "lease.lock";
const STATE: &str = "state.json";
const NEXT_STATE: &str = "state.next.json";
const MAX_LINE: u64 = 128;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    format: u32,
    owner: String,
    kind: String,
    cache: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
enum Log {
    A,
    B,
}
impl Log {
    fn name(self) -> &'static str {
        match self {
            Self::A => "stores-a.log",
            Self::B => "stores-b.log",
        }
    }
    fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: u32,
    read: Log,
    write: Log,
    offset: u64,
}

pub(super) struct Entry {
    pub name: Option<String>,
    pub end: u64,
}

pub(super) struct Journal {
    directory: PathBuf,
    // Never unlink this file: every registration/recovery must lock its inode.
    _lease: File,
    state: State,
    reader: Option<BufReader<File>>,
}

impl Journal {
    pub fn open(cache: &Path, nonblocking: bool) -> std::io::Result<Self> {
        let cache = std::path::absolute(cache)?;
        require_directory(&cache)?;
        let directory = cache.join(DIRECTORY);
        let published = match std::fs::symlink_metadata(&directory) {
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                initialize(&cache, &directory)?
            }
            Err(error) => return Err(error),
        };
        require_directory(&directory)?;
        let lease_path = directory.join(LEASE);
        require_file(&lease_path, 0)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lease_path)?;
        acquire(&lease, nonblocking)?;
        if published {
            initialization_checkpoint(&directory, "published");
        }
        let identity = crate::files::storage::encode_path(&cache);
        require_file(&directory.join(MARKER), 8192)?;
        let owner: Owner = serde_json::from_slice(&std::fs::read(directory.join(MARKER))?)?;
        if owner.format != 1
            || owner.owner != "revenant-desktop"
            || owner.kind != "native-store-recovery"
            || owner.cache != identity
        {
            return Err(std::io::Error::other(
                "Recovery metadata does not belong to this cache",
            ));
        }
        // The marker grants ownership only over fixed files, including the
        // single interrupted state-write slot, never name-prefix matches.
        for (index, entry) in std::fs::read_dir(&directory)?.enumerate() {
            if index >= 64 {
                return Err(std::io::Error::other(
                    "Recovery metadata has too many unexpected temporaries",
                ));
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| std::io::Error::other("Unknown recovery metadata path"))?;
            let temporary = name == NEXT_STATE;
            if ![
                MARKER,
                LEASE,
                STATE,
                NEXT_STATE,
                Log::A.name(),
                Log::B.name(),
            ]
            .contains(&name)
            {
                return Err(std::io::Error::other(
                    "Recovery metadata contains an unrelated path",
                ));
            }
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if !metadata.is_file() || is_link(&metadata) {
                return Err(std::io::Error::other(
                    "Recovery metadata contains a linked or non-file path",
                ));
            }
            if temporary && metadata.len() <= 8192 {
                std::fs::remove_file(entry.path())?;
            } else if temporary {
                return Err(std::io::Error::other(
                    "Recovery state staging file exceeds its ownership bounds",
                ));
            }
        }
        require_file(&directory.join(STATE), 8192)?;
        let state: State = serde_json::from_slice(&std::fs::read(directory.join(STATE))?)?;
        if state.format != 1 {
            return Err(std::io::Error::other("Unsupported recovery journal state"));
        }
        for log in [Log::A, Log::B] {
            require_file(&directory.join(log.name()), u64::MAX)?;
        }
        if state.offset > std::fs::metadata(directory.join(state.read.name()))?.len() {
            return Err(std::io::Error::other("Recovery cursor exceeds its journal"));
        }
        if state.read == state.write && state.offset != 0 {
            return Err(std::io::Error::other(
                "Completed recovery journal has a nonzero cursor",
            ));
        }
        if state.offset != 0 {
            let mut file = File::open(directory.join(state.read.name()))?;
            file.seek(SeekFrom::Start(state.offset - 1))?;
            let mut previous = [0];
            file.read_exact(&mut previous)?;
            if previous[0] != b'\n' {
                return Err(std::io::Error::other(
                    "Recovery cursor is not on a record boundary",
                ));
            }
        }
        Ok(Self {
            directory,
            _lease: lease,
            state,
            reader: None,
        })
    }

    pub fn append(&mut self, name: &str) -> std::io::Result<()> {
        if !valid_name(name) {
            return Err(std::io::Error::other("Invalid store registration"));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.directory.join(self.state.write.name()))?;
        repair_tail(&mut file)?;
        file.seek(SeekFrom::End(0))?;
        file.write_all(name.as_bytes())?;
        file.write_all(b"\n")?;
        // Publication of the ownership marker/cursor follows this durable line.
        file.sync_all()
    }

    pub fn begin(&mut self) -> std::io::Result<()> {
        if self.state.read == self.state.write {
            let destination = self.state.read.other();
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(self.directory.join(destination.name()))?
                .sync_all()?;
            self.state.write = destination;
            self.persist()?;
        }
        let mut file = File::open(self.directory.join(self.state.read.name()))?;
        file.seek(SeekFrom::Start(self.state.offset))?;
        self.reader = Some(BufReader::new(file));
        Ok(())
    }

    pub fn next(&mut self) -> std::io::Result<Option<Entry>> {
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| std::io::Error::other("Recovery journal has not begun"))?;
        let mut bytes = Vec::with_capacity(MAX_LINE as usize);
        reader
            .by_ref()
            .take(MAX_LINE)
            .read_until(b'\n', &mut bytes)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        if bytes.last() != Some(&b'\n') {
            if bytes.len() as u64 == MAX_LINE {
                return Err(std::io::Error::other(
                    "Recovery journal contains an oversized record",
                ));
            }
            // A killed registration/compaction never publishes its marker or
            // advances its cursor before the complete line has been synced.
            return Ok(None);
        }
        bytes.pop();
        let name = String::from_utf8(bytes)
            .ok()
            .filter(|name| valid_name(name));
        Ok(Some(Entry {
            name,
            end: reader.stream_position()?,
        }))
    }

    pub fn advance(&mut self, end: u64) {
        self.state.offset = end;
    }

    pub fn finish(&mut self, complete: bool) -> std::io::Result<()> {
        if complete {
            self.state.read = self.state.write;
            self.state.offset = 0;
        }
        self.persist()
    }

    fn persist(&self) -> std::io::Result<()> {
        let staging = self.directory.join(NEXT_STATE);
        let mut file = File::create_new(&staging)?;
        serde_json::to_writer(&mut file, &self.state)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(staging, self.directory.join(STATE))
    }
}

fn initialize(cache: &Path, destination: &Path) -> std::io::Result<bool> {
    // The public coordinator is either absent or complete. A creator killed at
    // any earlier step leaves only its private temporary, never a partial public
    // directory that every future registration would mistake for the journal.
    let mut staging = tempfile::Builder::new()
        .prefix(".revenant-native-stores-initializing-")
        .tempdir_in(cache)?;
    initialization_checkpoint(staging.path(), "directory");
    let lease = File::create_new(staging.path().join(LEASE))?;
    acquire(&lease, false)?;
    initialization_checkpoint(staging.path(), "lease");
    for log in [Log::A, Log::B] {
        File::create_new(staging.path().join(log.name()))?.sync_all()?;
        initialization_checkpoint(staging.path(), log.name());
    }
    write_new_json(
        &staging.path().join(STATE),
        &State {
            format: 1,
            read: Log::A,
            write: Log::A,
            offset: 0,
        },
    )?;
    initialization_checkpoint(staging.path(), "state");
    write_new_json(
        &staging.path().join(MARKER),
        &Owner {
            format: 1,
            owner: "revenant-desktop".into(),
            kind: "native-store-recovery".into(),
            cache: crate::files::storage::encode_path(cache),
        },
    )?;
    initialization_checkpoint(staging.path(), "owner");
    // Windows cannot rename a directory while a child file remains open. The
    // stage is private and never enumerated for recovery; release its lease
    // before publishing the fully initialized directory, then take the stable
    // public lease in open(). Concurrent registrations synchronize on that file.
    drop(lease);
    match rename_exclusive(staging.path(), destination) {
        Ok(()) => {
            staging.disable_cleanup(true);
            Ok(true)
        }
        // Another initializer may have won. Open and validate its complete
        // journal below; never merge with or replace a pre-existing directory.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(not(test))]
fn initialization_checkpoint(_: &Path, _: &str) {}

#[cfg(test)]
fn initialization_checkpoint(path: &Path, phase: &str) {
    if std::env::var("REVENANT_JOURNAL_FIXTURE_PHASE").as_deref() != Ok(phase) {
        return;
    }
    let root = PathBuf::from(std::env::var_os("REVENANT_JOURNAL_FIXTURE_ROOT").unwrap());
    let identity = std::env::var("REVENANT_JOURNAL_FIXTURE_ID").unwrap();
    if identity.starts_with("race-") {
        std::fs::write(
            root.join(format!("ready-{identity}")),
            path.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.join("publish").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        return;
    }
    std::fs::write(root.join("ready"), path.as_os_str().as_encoded_bytes()).unwrap();
    loop {
        std::thread::park();
    }
}

#[cfg(windows)]
fn rename_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileW(source: *const u16, destination: *const u16) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both arrays contain live, NUL-terminated UTF-16 paths. MoveFileW
    // cannot replace an existing destination, including an unrelated empty dir.
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    unsafe extern "C" {
        fn renameat2(
            old_directory: i32,
            old_path: *const std::ffi::c_char,
            new_directory: i32,
            new_path: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn renamex_np(
            old_path: *const std::ffi::c_char,
            new_path: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    // SAFETY: live CStrings cover the synchronous calls. Linux RENAME_NOREPLACE
    // (1), and Darwin RENAME_EXCL (4), atomically reject every existing target.
    #[cfg(target_os = "linux")]
    let result = unsafe { renameat2(-100, source.as_ptr(), -100, destination.as_ptr(), 1) };
    #[cfg(target_os = "macos")]
    let result = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), 4) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn rename_exclusive(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Native store initialization requires atomic no-replace directory publication",
    ))
}

fn acquire(file: &File, nonblocking: bool) -> std::io::Result<()> {
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
            Err(std::fs::TryLockError::WouldBlock) if nonblocking => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "Recovery coordinator lease is held",
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if started.elapsed() >= Duration::from_secs(2) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Native store registration timed out waiting for the coordinator lease",
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn valid_name(name: &str) -> bool {
    name.strip_prefix("revenant-index-")
        .or_else(|| name.strip_prefix("revenant-results-"))
        .is_some_and(|token| uuid::Uuid::parse_str(token).is_ok() && token.len() == 36)
}

fn repair_tail(file: &mut File) -> std::io::Result<()> {
    let length = file.metadata()?.len();
    if length == 0 {
        return Ok(());
    }
    let start = length.saturating_sub(MAX_LINE);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(MAX_LINE as usize);
    file.take(MAX_LINE).read_to_end(&mut bytes)?;
    if bytes.last() == Some(&b'\n') {
        return Ok(());
    }
    let end = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => start + index as u64 + 1,
        None if start == 0 => 0,
        None => {
            return Err(std::io::Error::other(
                "Recovery journal has an oversized partial registration",
            ));
        }
    };
    file.set_len(end)
}

fn require_file(path: &Path, max: u64) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > max {
        return Err(std::io::Error::other(
            "Recovery metadata must be a bounded regular file",
        ));
    }
    Ok(())
}

fn write_new_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let mut file = File::create_new(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    struct ChildFixture(Child);
    impl Drop for ChildFixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn fixture(root: &Path, phase: Option<&str>, identity: &str) -> ChildFixture {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "native_store::journal::tests::initialization_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("REVENANT_JOURNAL_FIXTURE_ROOT", root)
            .env("REVENANT_JOURNAL_FIXTURE_ID", identity)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(phase) = phase {
            command.env("REVENANT_JOURNAL_FIXTURE_PHASE", phase);
        }
        ChildFixture(command.spawn().unwrap())
    }

    fn wait_for_file(path: &Path, children: &mut [ChildFixture]) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !path.exists() {
            for child in children.iter_mut() {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "Initializer exited before its checkpoint"
                );
            }
            assert!(
                Instant::now() < deadline,
                "Initializer did not reach checkpoint"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn initialization_child() {
        let Some(root) = std::env::var_os("REVENANT_JOURNAL_FIXTURE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let identity = std::env::var("REVENANT_JOURNAL_FIXTURE_ID").unwrap();
        if std::env::var_os("REVENANT_JOURNAL_FIXTURE_PHASE").is_none() {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !root.join("start").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let mut journal = Journal::open(&root.join("cache"), false).unwrap();
        journal.append(&name()).unwrap();
        drop(journal);
        std::fs::write(root.join(format!("done-{identity}")), b"registered").unwrap();
    }

    #[test]
    fn forced_exit_at_each_initialization_step_never_publishes_a_partial_journal() {
        for phase in [
            "directory",
            "lease",
            "stores-a.log",
            "stores-b.log",
            "state",
            "owner",
            "published",
        ] {
            let root = tempfile::tempdir().unwrap();
            let cache = root.path().join("cache");
            std::fs::create_dir(&cache).unwrap();
            let mut children = [fixture(root.path(), Some(phase), "killed")];
            wait_for_file(&root.path().join("ready"), &mut children);
            let staged = PathBuf::from(
                String::from_utf8(std::fs::read(root.path().join("ready")).unwrap()).unwrap(),
            );
            if phase == "published" {
                assert!(cache.join(DIRECTORY).join(MARKER).exists());
                assert_eq!(
                    Journal::open(&cache, true).err().unwrap().kind(),
                    std::io::ErrorKind::WouldBlock,
                    "A published initializer must retain its lease until opening is safe"
                );
            } else {
                assert!(
                    !cache.join(DIRECTORY).exists(),
                    "{phase} exposed partial metadata"
                );
                // A second process may initialize without touching the first
                // process's private directory or its still-held OS lease.
                let mut concurrent = Journal::open(&cache, false).unwrap();
                concurrent.append(&name()).unwrap();
                assert!(staged.exists());
                if phase != "directory" {
                    let held = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(staged.join(LEASE))
                        .unwrap();
                    assert!(matches!(
                        held.try_lock(),
                        Err(std::fs::TryLockError::WouldBlock)
                    ));
                }
            }
            children[0].0.kill().unwrap();
            children[0].0.wait().unwrap();
            let mut resumed = Journal::open(&cache, false).unwrap();
            resumed.append(&name()).unwrap();
            resumed.begin().unwrap();
            assert!(resumed.next().unwrap().is_some());
            resumed.finish(false).unwrap();
            drop(resumed);
            Journal::open(&cache, false).unwrap();
        }
    }

    #[test]
    fn simultaneous_first_processes_publish_one_complete_coordinator_without_losing_registration() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        std::fs::create_dir(&cache).unwrap();
        let mut children: Vec<_> = (0..8)
            .map(|index| fixture(root.path(), Some("owner"), &format!("race-{index}")))
            .collect();
        // All eight processes own complete staged journals before any may
        // publish, so this tests actual conflicting directory renames.
        for index in 0..8 {
            wait_for_file(
                &root.path().join(format!("ready-race-{index}")),
                &mut children,
            );
        }
        assert!(!cache.join(DIRECTORY).exists());
        std::fs::write(root.path().join("publish"), b"publish together").unwrap();
        for (index, child) in children.iter_mut().enumerate() {
            assert!(
                child.0.wait().unwrap().success(),
                "Initializer {index} failed"
            );
            assert!(root.path().join(format!("done-race-{index}")).exists());
        }
        let mut journal = Journal::open(&cache, false).unwrap();
        journal.begin().unwrap();
        let mut count = 0;
        while let Some(entry) = journal.next().unwrap() {
            assert!(entry.name.is_some());
            count += 1;
        }
        assert_eq!(count, 8);
    }

    #[test]
    fn publication_cannot_replace_even_an_unmarked_empty_directory() {
        let cache = tempfile::tempdir().unwrap();
        let unrelated = cache.path().join(DIRECTORY);
        std::fs::create_dir(&unrelated).unwrap();
        let staged = tempfile::tempdir_in(cache.path()).unwrap();
        std::fs::write(staged.path().join("sentinel"), b"staging").unwrap();
        assert!(rename_exclusive(staged.path(), &unrelated).is_err());
        assert!(unrelated.is_dir());
        assert_eq!(std::fs::read_dir(&unrelated).unwrap().count(), 0);
        assert!(staged.path().join("sentinel").exists());
        assert!(Journal::open(cache.path(), false).is_err());
        assert_eq!(std::fs::read_dir(&unrelated).unwrap().count(), 0);
        std::fs::write(unrelated.join("original.dat"), b"original").unwrap();
        assert!(Journal::open(cache.path(), false).is_err());
        assert_eq!(
            std::fs::read(unrelated.join("original.dat")).unwrap(),
            b"original"
        );
    }

    fn name() -> String {
        format!("revenant-index-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn interrupted_registration_tail_is_repaired_before_the_next_append() {
        let cache = tempfile::tempdir().unwrap();
        let first = name();
        let second = name();
        let mut journal = Journal::open(cache.path(), false).unwrap();
        journal.append(&first).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(journal.directory.join(journal.state.write.name()))
            .unwrap();
        file.write_all(b"revenant-ind").unwrap();
        file.sync_all().unwrap();
        drop(file);
        drop(journal);
        let mut resumed = Journal::open(cache.path(), false).unwrap();
        resumed.append(&second).unwrap();
        resumed.begin().unwrap();
        assert_eq!(
            resumed.next().unwrap().unwrap().name.as_deref(),
            Some(first.as_str())
        );
        assert_eq!(
            resumed.next().unwrap().unwrap().name.as_deref(),
            Some(second.as_str())
        );
        assert!(resumed.next().unwrap().is_none());
    }

    #[test]
    fn interrupted_compaction_preserves_both_unpublished_and_published_cursor_progress() {
        let cache = tempfile::tempdir().unwrap();
        let first = name();
        let second = name();
        let mut journal = Journal::open(cache.path(), false).unwrap();
        journal.append(&first).unwrap();
        journal.append(&second).unwrap();
        journal.begin().unwrap();
        let entry = journal.next().unwrap().unwrap();
        journal.append(entry.name.as_ref().unwrap()).unwrap();
        // Crash after copying but before publishing advancement: first must replay.
        drop(journal);
        let mut replayed = Journal::open(cache.path(), false).unwrap();
        replayed.begin().unwrap();
        let replay = replayed.next().unwrap().unwrap();
        assert_eq!(replay.name.as_deref(), Some(first.as_str()));
        replayed.append(&first).unwrap();
        replayed.advance(replay.end);
        replayed.finish(false).unwrap();
        drop(replayed);
        let mut resumed = Journal::open(cache.path(), false).unwrap();
        resumed.begin().unwrap();
        let remaining = resumed.next().unwrap().unwrap();
        assert_eq!(remaining.name.as_deref(), Some(second.as_str()));
        resumed.append(&second).unwrap();
        resumed.advance(remaining.end);
        assert!(resumed.next().unwrap().is_none());
        resumed.finish(true).unwrap();
        drop(resumed);
        let mut completed = Journal::open(cache.path(), false).unwrap();
        completed.begin().unwrap();
        let mut names = Vec::new();
        while let Some(entry) = completed.next().unwrap() {
            names.push(entry.name.unwrap());
        }
        assert!(names.contains(&first));
        assert!(names.contains(&second));
    }

    #[test]
    fn interrupted_state_temporary_does_not_replace_the_committed_cursor() {
        let cache = tempfile::tempdir().unwrap();
        let first = name();
        let mut journal = Journal::open(cache.path(), false).unwrap();
        journal.append(&first).unwrap();
        let temporary = journal.directory.join(NEXT_STATE);
        std::fs::write(&temporary, b"{\"offset\":").unwrap();
        drop(journal);
        let mut resumed = Journal::open(cache.path(), false).unwrap();
        assert!(!temporary.exists());
        resumed.begin().unwrap();
        assert_eq!(
            resumed.next().unwrap().unwrap().name.as_deref(),
            Some(first.as_str())
        );
    }
}
