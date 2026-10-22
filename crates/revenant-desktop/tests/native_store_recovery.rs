//! Real process-exit fixtures: the child owns an index mid-scan or retained
//! results, and the parent verifies OS lease release and startup recovery.

use revenant_desktop::{
    DesktopOptions, DesktopRuntime,
    files::FolderIndex,
    results::{BatchOutcome, ResultStore},
};
use revenant_sdk::Application;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

struct OwnedChild(Child);
impl OwnedChild {
    fn start(root: &Path, role: &str) -> Self {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "owned_store_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("REVENANT_STORE_FIXTURE_ROOT", root)
            .env("REVENANT_STORE_FIXTURE_ROLE", role)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut child = Self(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "Store child exited before owning its store"
            );
            assert!(
                Instant::now() < deadline,
                "Store child did not reach its fixture checkpoint"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        child
    }

    fn terminate(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fixture_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("originals")).unwrap();
    std::fs::create_dir_all(root.path().join("data/settings")).unwrap();
    std::fs::write(
        root.path().join("data/settings/author-owned.json"),
        b"settings sentinel",
    )
    .unwrap();
    for ordinal in 0..1024 {
        std::fs::write(
            root.path()
                .join("originals")
                .join(format!("file-{ordinal}.txt")),
            b"original",
        )
        .unwrap();
    }
    root
}

async fn restart(root: &Path) -> DesktopRuntime {
    DesktopRuntime::new(
        Application::new(),
        DesktopOptions {
            cache_dir: Some(root.join("cache")),
            data_dir: Some(root.join("data")),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

fn store_path(root: &Path) -> PathBuf {
    std::fs::read_dir(root.join("cache"))
        .unwrap()
        .find(|entry| entry.as_ref().unwrap().file_name() != ".revenant-native-stores")
        .unwrap()
        .unwrap()
        .path()
}

#[test]
fn owned_store_child() {
    let Some(root) = std::env::var_os("REVENANT_STORE_FIXTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    match std::env::var("REVENANT_STORE_FIXTURE_ROLE")
        .unwrap()
        .as_str()
    {
        "scan" => {
            let index =
                FolderIndex::new(root.join("originals"), &root.join("cache"), true).unwrap();
            index
                .scan(&AtomicBool::new(false), |status| {
                    if status.discovered >= 128 {
                        std::fs::write(
                            root.join("ready"),
                            b"scan owns committed partial inventory",
                        )
                        .unwrap();
                        loop {
                            std::thread::park();
                        }
                    }
                })
                .unwrap();
            panic!("Scan fixture completed without stopping mid-scan");
        }
        "results" => {
            let results = ResultStore::new(&root.join("cache")).unwrap();
            results
                .append(&BatchOutcome {
                    ordinal: 0,
                    input: json!(1),
                    output: Some(json!(2)),
                    error: None,
                })
                .unwrap();
            let _retained = results.lease();
            std::fs::write(root.join("ready"), b"results owns retained outcomes").unwrap();
            loop {
                std::thread::park();
            }
        }
        _ => panic!("Unknown store fixture role"),
    }
}

#[tokio::test]
async fn forced_exit_scans_and_retained_results_are_reclaimed_on_repeated_restart() {
    for role in ["scan", "results", "scan", "results"] {
        let root = fixture_root();
        let legacy = root.path().join("cache/revenant-index-unmarked");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("original.dat"), b"legacy sentinel").unwrap();
        let mut child = OwnedChild::start(root.path(), role);
        let live = restart(root.path()).await;
        assert_eq!(live.store_recovery().active, 1);
        assert_eq!(live.store_recovery().reclaimed, 0);
        live.shutdown().await.unwrap();
        child.terminate();
        let restarted = restart(root.path()).await;
        assert_eq!(
            restarted.store_recovery().reclaimed,
            1,
            "{role}: {:?}",
            restarted.store_recovery()
        );
        assert_eq!(restarted.store_recovery().deferred, 0);
        assert_eq!(
            std::fs::read(legacy.join("original.dat")).unwrap(),
            b"legacy sentinel"
        );
        assert_eq!(
            std::fs::read(root.path().join("originals/file-0.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            std::fs::read(root.path().join("data/settings/author-owned.json")).unwrap(),
            b"settings sentinel"
        );
        restarted.shutdown().await.unwrap();
        let repeated = restart(root.path()).await;
        assert_eq!(repeated.store_recovery().reclaimed, 0);
        assert_eq!(repeated.store_recovery().deferred, 0);
        repeated.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn concurrent_instances_and_retained_leases_survive_another_startup() {
    let root = fixture_root();
    let mut child = OwnedChild::start(root.path(), "results");
    let results = ResultStore::new(&root.path().join("cache")).unwrap();
    let retained = results.lease();
    drop(results);
    let concurrent = restart(root.path()).await;
    assert_eq!(concurrent.store_recovery().active, 2);
    assert_eq!(concurrent.store_recovery().reclaimed, 0);
    concurrent.shutdown().await.unwrap();
    child.terminate();
    let after_exit = restart(root.path()).await;
    assert_eq!(after_exit.store_recovery().reclaimed, 1);
    assert_eq!(after_exit.store_recovery().active, 1);
    drop(retained);
    after_exit.shutdown().await.unwrap();
    let empty = restart(root.path()).await;
    assert_eq!(empty.store_recovery().active, 0);
    assert_eq!(empty.store_recovery().reclaimed, 0);
    assert_eq!(empty.store_recovery().deferred, 0);
    assert!(
        std::fs::read_dir(root.path().join("cache"))
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == ".revenant-native-stores")
    );
    empty.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_external_store_lease_is_reported_and_retried_after_release() {
    let root = fixture_root();
    let mut child = OwnedChild::start(root.path(), "results");
    let path = store_path(root.path());
    child.terminate();
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join("revenant-owner.lock"))
        .unwrap();
    lease.try_lock().unwrap();
    let locked = restart(root.path()).await;
    assert_eq!(locked.store_recovery().active, 1);
    assert!(
        locked
            .store_recovery()
            .issues
            .iter()
            .any(|issue| issue.code == "lease_busy")
    );
    assert!(path.join("store.sqlite3").exists());
    locked.shutdown().await.unwrap();
    drop(lease);
    let retried = restart(root.path()).await;
    assert_eq!(retried.store_recovery().reclaimed, 1);
    assert!(!path.exists());
    retried.shutdown().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn windows_sharing_violation_retains_ownership_for_a_later_restart() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = fixture_root();
    let mut child = OwnedChild::start(root.path(), "results");
    let path = store_path(root.path());
    child.terminate();
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path.join("store.sqlite3"))
        .unwrap();
    let blocked = restart(root.path()).await;
    assert_eq!(blocked.store_recovery().deferred, 1);
    assert!(path.join("revenant-owner.json").exists());
    assert!(path.join("store.sqlite3").exists());
    blocked.shutdown().await.unwrap();
    drop(locked);
    let retried = restart(root.path()).await;
    assert_eq!(retried.store_recovery().reclaimed, 1);
    assert!(!path.exists());
    retried.shutdown().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn windows_lease_file_deletion_failure_restores_the_ownership_record() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = fixture_root();
    let mut child = OwnedChild::start(root.path(), "results");
    let path = store_path(root.path());
    child.terminate();
    // Permit reading/writing and locking, but deny delete-sharing on the lease.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(path.join("revenant-owner.lock"))
        .unwrap();
    let blocked = restart(root.path()).await;
    assert_eq!(blocked.store_recovery().deferred, 1);
    assert!(path.join("revenant-owner.json").exists());
    assert!(path.join("revenant-owner.lock").exists());
    blocked.shutdown().await.unwrap();
    drop(held);
    let retried = restart(root.path()).await;
    assert_eq!(retried.store_recovery().reclaimed, 1);
    assert!(!path.exists());
    retried.shutdown().await.unwrap();
}
