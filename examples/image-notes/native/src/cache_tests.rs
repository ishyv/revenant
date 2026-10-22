//! Tiny-budget regressions plus real OS lease/process-exit fixtures.
use super::*;
use std::{
    process::{Child, Command},
    sync::mpsc,
};

fn running() -> (TaskContext, revenant::TaskExecution) {
    let resources = revenant::ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let context = revenant::TaskManager::new(resources)
        .create(&scope, "cache")
        .unwrap();
    let execution = context.begin().unwrap();
    (context, execution)
}

fn cached_file(cache: &Cache, asset: &str) -> PathBuf {
    let file: String = cache
        .db
        .query_row("SELECT file FROM entries WHERE asset=?1", [asset], |row| {
            row.get(0)
        })
        .unwrap();
    cache.owned_path(&file, "jpg").unwrap()
}

#[test]
fn budget_evicts_least_recently_read_entry_and_obsolete_sizes() {
    let data = tempfile::tempdir().unwrap();
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 16, &context).unwrap();
    cache.publish("first", "1", 320, &[1; 8], &context).unwrap();
    cache
        .publish("second", "1", 320, &[2; 8], &context)
        .unwrap();
    assert_eq!(cache.read("first", "1", 320, 16).unwrap(), Some(vec![1; 8]));
    cache.publish("third", "1", 320, &[3; 8], &context).unwrap();
    assert!(cache.read("second", "1", 320, 16).unwrap().is_none());
    assert_eq!(cache.usage(&context).unwrap().bytes.get(), 16);
    cache.remove_obsolete("first", "2", &context).unwrap();
    assert!(cache.read("first", "1", 320, 16).unwrap().is_none());
    cache
        .publish("first", "2", 1440, &[4; 4], &context)
        .unwrap();
    cache.publish("first", "2", 320, &[5; 4], &context).unwrap();
    cache.remove_obsolete("first", "3", &context).unwrap();
    assert!(cache.read("first", "2", 320, 16).unwrap().is_none());
    assert!(cache.read("first", "2", 1440, 16).unwrap().is_none());
    assert_eq!(cache.usage(&context).unwrap().bytes.get(), 8);
}

#[test]
fn real_file_sizes_and_unmanaged_bytes_count_toward_the_budget() {
    let data = tempfile::tempdir().unwrap();
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 16, &context).unwrap();
    cache
        .publish("changed", "1", 320, &[1; 4], &context)
        .unwrap();
    fs::write(cached_file(&cache, "changed"), [1; 24]).unwrap();
    cache.publish("new", "1", 320, &[2; 4], &context).unwrap();
    assert!(cache.read("changed", "1", 320, 32).unwrap().is_none());
    assert_eq!(cache.usage(&context).unwrap().bytes.get(), 4);
    fs::create_dir(cache.directory.join("preserved")).unwrap();
    fs::write(cache.directory.join("preserved/original.png"), [3; 16]).unwrap();
    let warning = cache
        .publish("uncached", "1", 320, &[4; 8], &context)
        .unwrap();
    assert!(warning.unwrap().contains("unrecognized"));
    assert!(cache.read("uncached", "1", 320, 32).unwrap().is_none());
    let usage = cache.usage(&context).unwrap();
    assert_eq!(usage.unmanaged_bytes.get(), 16);
    assert!(usage.publication_blocked);
    let (_, _, usage) = cache.clear(&context).unwrap();
    assert_eq!(usage.bytes.get(), 16);
    assert!(cache.directory.join("preserved/original.png").is_file());
}

#[test]
fn clearing_preserves_unmarked_paths_catalog_and_notes() {
    let data = tempfile::tempdir().unwrap();
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 64, &context).unwrap();
    fs::write(data.path().join("catalog.sqlite3"), b"catalog and notes").unwrap();
    fs::write(data.path().join("original.jpg"), b"original").unwrap();
    fs::write(cache.directory.join("legacy-known-looking.jpg"), b"legacy").unwrap();
    fs::write(
        cache
            .directory
            .join(format!("{}.tmp", uuid::Uuid::new_v4())),
        b"unmarked",
    )
    .unwrap();
    cache.publish("owned", "1", 320, &[9; 8], &context).unwrap();
    let (bytes, entries, usage) = cache.clear(&context).unwrap();
    assert_eq!((bytes, entries), (8, 1));
    assert_eq!((usage.managed_bytes.get(), usage.entries), (0, 0));
    assert_eq!(usage.unmanaged_bytes.get(), 14);
    assert_eq!(
        fs::read(data.path().join("catalog.sqlite3")).unwrap(),
        b"catalog and notes"
    );
    assert_eq!(
        fs::read(data.path().join("original.jpg")).unwrap(),
        b"original"
    );
    assert!(cache.directory.join("legacy-known-looking.jpg").exists());
}

#[test]
fn invalid_ownership_paths_and_replaced_directories_are_preserved() {
    let data = tempfile::tempdir().unwrap();
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 64, &context).unwrap();
    let original = data.path().join("original.jpg");
    fs::write(&original, b"original").unwrap();
    cache.db.execute("INSERT INTO entries(file,asset,revision,bound,size,used) VALUES('../original.jpg','asset','1',320,8,1)", []).unwrap();
    assert_eq!(cache.clear(&context).err().unwrap().code, "cache_invalid");
    assert_eq!(fs::read(&original).unwrap(), b"original");
    cache.db.execute("DELETE FROM entries", []).unwrap();
    cache.publish("asset", "1", 320, &[8; 8], &context).unwrap();
    let owned = cached_file(&cache, "asset");
    fs::remove_file(&owned).unwrap();
    fs::create_dir(&owned).unwrap();
    fs::write(owned.join("unrelated"), b"preserve").unwrap();
    assert_eq!(cache.clear(&context).err().unwrap().code, "cache_invalid");
    assert!(owned.join("unrelated").exists());
}

#[test]
fn unrelated_database_is_rejected_without_changing_its_bytes() {
    let data = tempfile::tempdir().unwrap();
    fs::create_dir(data.path().join("renditions")).unwrap();
    let index = data.path().join("renditions/cache.sqlite3");
    let database = Connection::open(&index).unwrap();
    database
        .execute_batch("CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('keep');")
        .unwrap();
    drop(database);
    let original = fs::read(&index).unwrap();
    let (context, _execution) = running();
    assert_eq!(
        Cache::open_with_budget(data.path(), 64, &context)
            .err()
            .unwrap()
            .code,
        "cache_invalid"
    );
    assert_eq!(fs::read(index).unwrap(), original);
}

#[cfg(windows)]
#[test]
fn directory_junctions_are_preserved_without_counting_or_traversing_their_targets() {
    let data = tempfile::tempdir().unwrap();
    let originals = data.path().join("originals");
    fs::create_dir(&originals).unwrap();
    fs::write(originals.join("original.jpg"), [7; 1024]).unwrap();
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 64, &context).unwrap();
    let junction = cache.directory.join("preserved-junction");
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&originals)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let link_bytes = fs::symlink_metadata(&junction).unwrap().len();
    let usage = cache.usage(&context).unwrap();
    assert_eq!(usage.unmanaged_bytes.get(), link_bytes);
    cache.clear(&context).unwrap();
    assert!(junction.exists());
    assert_eq!(fs::read(originals.join("original.jpg")).unwrap(), [7; 1024]);
}

#[test]
fn usage_prunes_unavailable_catalog_entries_and_cancellation_releases_waiters() {
    let data = tempfile::tempdir().unwrap();
    let catalog = Catalog::open_at(data.path()).unwrap();
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 64, &context).unwrap();
    cache
        .publish("removed-asset", "1", 320, &[1; 8], &context)
        .unwrap();
    let usage = cache.maintain(&catalog, &context).unwrap();
    assert_eq!((usage.bytes.get(), usage.entries), (0, 0));
    let (waiting, _waiting_execution) = running();
    let worker_context = waiting.clone();
    let directory = data.path().to_owned();
    let (sent, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        sent.send(
            Cache::open_with_budget(&directory, 64, &worker_context)
                .err()
                .unwrap()
                .code,
        )
        .unwrap();
    });
    waiting.request_cancel().unwrap();
    assert_eq!(
        received.recv_timeout(Duration::from_secs(2)).unwrap(),
        "cancelled"
    );
    worker.join().unwrap();
    assert_eq!(cache.clear(&context).unwrap().1, 0);
    assert_eq!(cache.clear(&context).unwrap().1, 0);
}

#[cfg(windows)]
#[test]
fn locked_rendition_reports_failure_and_retries_without_touching_unrelated_files() {
    let data = tempfile::tempdir().unwrap();
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 64, &context).unwrap();
    cache.publish("asset", "1", 320, &[1; 8], &context).unwrap();
    let file = cached_file(&cache, "asset");
    let mut locker = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cache::tests::subprocess_cache_worker",
            "--nocapture",
        ])
        .env("LUMA_CACHE_TEST_DATA", data.path())
        .env("LUMA_CACHE_TEST_ACTION", "lockfile")
        .env("LUMA_CACHE_TEST_TAG", "locked")
        .env("LUMA_CACHE_TEST_FILE", &file)
        .spawn()
        .unwrap();
    wait_for(&data.path().join("locked.ready"));
    assert_eq!(cache.clear(&context).err().unwrap().code, "cache_io");
    fs::write(data.path().join("locked.release"), b"release").unwrap();
    assert!(locker.wait().unwrap().success());
    assert!(file.is_file());
    assert_eq!(cache.clear(&context).unwrap().0, 8);
    assert!(!file.exists());
}

fn child(data: &Path, action: &str, tag: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cache::tests::subprocess_cache_worker",
            "--nocapture",
        ])
        .env("LUMA_CACHE_TEST_DATA", data)
        .env("LUMA_CACHE_TEST_ACTION", action)
        .env("LUMA_CACHE_TEST_TAG", tag)
        .spawn()
        .unwrap()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "cache fixture did not signal {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn concurrent_processes_share_the_budget_and_forced_exit_releases_ownership() {
    let data = tempfile::tempdir().unwrap();
    let mut first = child(data.path(), "publish", "first");
    let mut second = child(data.path(), "publish", "second");
    assert!(first.wait().unwrap().success());
    assert!(second.wait().unwrap().success());
    let (context, _execution) = running();
    let cache = Cache::open_with_budget(data.path(), 128, &context).unwrap();
    let usage = cache.usage(&context).unwrap();
    assert_eq!((usage.bytes.get(), usage.entries), (128, 2));
    drop(cache);
    fs::write(data.path().join("renditions/unmarked.tmp"), b"unmarked").unwrap();
    let mut interrupted = child(data.path(), "interrupt", "interrupted");
    wait_for(&data.path().join("interrupted.ready"));
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    for _ in 0..3 {
        let cache = Cache::open_with_budget(data.path(), 128, &context).unwrap();
        let pending: u32 = cache
            .db
            .query_row("SELECT count(*) FROM writes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pending, 0);
        assert!(
            !cache
                .directory
                .join("00000000-0000-4000-8000-000000000001.tmp")
                .exists()
        );
        assert!(
            !cache
                .directory
                .join("00000000-0000-4000-8000-000000000001.jpg")
                .exists()
        );
        assert!(cache.directory.join("unmarked.tmp").exists());
    }
    let mut interrupted = child(data.path(), "sqliteinterrupt", "database-interrupted");
    wait_for(&data.path().join("database-interrupted.ready"));
    assert!(
        data.path()
            .join("renditions/cache.sqlite3-journal")
            .is_file()
    );
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    let cache = Cache::open_with_budget(data.path(), 128, &context).unwrap();
    let rows: u32 = cache
        .db
        .query_row("SELECT count(*) FROM crash_test", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0, "the abandoned SQLite transaction must roll back");
    assert!(cache.usage(&context).unwrap().managed_bytes.get() > 0);
}

#[test]
fn clear_waits_for_an_active_writer_in_another_process() {
    let data = tempfile::tempdir().unwrap();
    let mut writer = child(data.path(), "hold", "writer");
    wait_for(&data.path().join("writer.ready"));
    let lease = File::options()
        .read(true)
        .write(true)
        .open(data.path().join("renditions/cache.lease"))
        .unwrap();
    assert!(matches!(
        lease.try_lock(),
        Err(fs::TryLockError::WouldBlock)
    ));
    let (completed, received) = mpsc::channel();
    let path = data.path().to_owned();
    let clearing = thread::spawn(move || {
        let (context, _execution) = running();
        let cache = Cache::open_with_budget(&path, 128, &context).unwrap();
        let result = cache.clear(&context).unwrap();
        completed.send(result.2.entries).unwrap();
    });
    assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
    assert!(
        data.path()
            .join("renditions/00000000-0000-4000-8000-000000000001.tmp")
            .exists()
    );
    fs::write(data.path().join("writer.release"), b"release").unwrap();
    assert!(writer.wait().unwrap().success());
    assert_eq!(received.recv_timeout(Duration::from_secs(10)).unwrap(), 0);
    clearing.join().unwrap();
}

/// Runs in an isolated test process, never as a real application operation.
#[test]
fn subprocess_cache_worker() {
    let Some(data) = std::env::var_os("LUMA_CACHE_TEST_DATA") else {
        return;
    };
    let data = PathBuf::from(data);
    let action = std::env::var("LUMA_CACHE_TEST_ACTION").unwrap();
    let tag = std::env::var("LUMA_CACHE_TEST_TAG").unwrap();
    let (context, _execution) = running();
    #[cfg(windows)]
    if action == "lockfile" {
        use std::os::windows::fs::OpenOptionsExt;
        let _locked = File::options()
            .read(true)
            .share_mode(0)
            .open(std::env::var_os("LUMA_CACHE_TEST_FILE").unwrap())
            .unwrap();
        fs::write(data.join(format!("{tag}.ready")), b"ready").unwrap();
        wait_for(&data.join(format!("{tag}.release")));
        return;
    }
    let cache = Cache::open_with_budget(&data, 128, &context).unwrap();
    if action == "sqliteinterrupt" {
        cache
            .db
            .execute_batch(
                "PRAGMA cache_size=1; CREATE TABLE crash_test(value BLOB); BEGIN IMMEDIATE;",
            )
            .unwrap();
        for _ in 0..100 {
            cache
                .db
                .execute("INSERT INTO crash_test VALUES(zeroblob(16384))", [])
                .unwrap();
        }
        fs::write(data.join(format!("{tag}.ready")), b"ready").unwrap();
        wait_for(&data.join(format!("{tag}.release")));
        return;
    }
    if action == "publish" {
        for index in 0..8 {
            cache
                .publish(&format!("{tag}-{index}"), "1", 320, &[9; 64], &context)
                .unwrap();
        }
        return;
    }
    let token = "00000000-0000-4000-8000-000000000001";
    let file = format!("{token}.jpg");
    let temporary = format!("{token}.tmp");
    cache
        .db
        .execute(
            "INSERT INTO writes(file,temporary) VALUES(?1,?2)",
            params![file, temporary],
        )
        .unwrap();
    let mut output = File::create_new(cache.owned_path(&temporary, "tmp").unwrap()).unwrap();
    output.write_all(b"interrupted owned write").unwrap();
    output.sync_all().unwrap();
    if action == "interrupt" {
        // Simulate the rename/index-commit interruption as well as the temp stage.
        fs::write(
            cache.owned_path(&file, "jpg").unwrap(),
            b"uncommitted owned result",
        )
        .unwrap();
    }
    fs::write(data.join(format!("{tag}.ready")), b"ready").unwrap();
    wait_for(&data.join(format!("{tag}.release")));
}
