//! Normal SDK replacement must publish source truth after regenerating locks.
use revenant::toolchain::process;
use serde_json::Value;
use std::{fs, path::Path};

fn cli(root: &Path, args: &[&str]) -> String {
    let binary = std::env::var_os("REVENANT_TEST_CLI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_BIN_EXE_revenant")).to_path_buf());
    process::run_capture(root, &binary.to_string_lossy(), args, false).unwrap()
}

#[test]
#[ignore = "compiles an isolated native consumer and installs npm dependencies"]
fn normal_upgrade_regenerates_lock_before_fingerprinting_and_restore_preserves_absence() {
    let temp = tempfile::tempdir().unwrap();
    cli(temp.path(), &["new", "sdk-freshness-proof"]);
    let root = temp.path().join("sdk-freshness-proof");
    fs::create_dir_all(root.join("native/src")).unwrap();
    for (relative, content) in [
        (
            "native/Cargo.toml",
            include_str!("fixtures/custom-native/Cargo.toml"),
        ),
        (
            "native/src/lib.rs",
            include_str!("fixtures/custom-native/src/lib.rs"),
        ),
        (
            "native/src/records.rs",
            include_str!("fixtures/custom-native/src/records.rs"),
        ),
    ] {
        fs::write(root.join(relative), content).unwrap();
    }
    let authored = root.join("native/src/lib.rs");
    let original = fs::read(&authored).unwrap();
    assert!(!root.join("native/Cargo.lock").exists());
    let output = cli(&root, &["sdk", "upgrade"]);
    let backup = output
        .lines()
        .find_map(|line| {
            line.strip_prefix("SDK upgraded and native contracts validated. Previous state: ")
        })
        .expect("upgrade must identify its recovery record");
    assert!(root.join("native/Cargo.lock").is_file());
    let snapshot: Value = serde_json::from_str(&cli(&root, &["inspect", "--json"])).unwrap();
    assert_eq!(snapshot["buildAttempt"]["status"], "succeeded");
    assert_eq!(snapshot["buildAttempt"]["sourceChangedDuringBuild"], false);
    assert_eq!(snapshot["consistency"]["sourceToFacade"], "current");
    cli(&root, &["sdk", "check"]);

    // A real authored edit remains stale; regeneration must not bless arbitrary
    // changes made after the source identity was captured.
    let mut changed = original.clone();
    changed.extend_from_slice(b"\n// authored freshness negative control\n");
    fs::write(&authored, &changed).unwrap();
    let stale: Value = serde_json::from_str(&cli(&root, &["inspect", "--json"])).unwrap();
    assert_eq!(stale["consistency"]["sourceToFacade"], "stale");
    cli(&root, &["sdk", "restore", backup]);
    assert!(!root.join("native/Cargo.lock").exists());
    assert_eq!(fs::read(authored).unwrap(), changed);
}
