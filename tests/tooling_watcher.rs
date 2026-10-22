use revenant::toolchain::desktop::NativeWatcher;
use std::{fs, sync::mpsc, time::Duration};
use tempfile::TempDir;

#[test]
fn creating_and_writing_authored_native_target_does_not_restart_the_app() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("native-target-output");
    fs::create_dir_all(root.join("native/src")).unwrap();
    fs::write(
        root.join("native/Cargo.toml"),
        "[package]\nname='test-native'\nversion='0.4.0'\n",
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let _watcher = NativeWatcher::start(&root, tx).unwrap();
    fs::create_dir_all(root.join("native/target/debug/build")).unwrap();
    for index in 0..256 {
        fs::write(
            root.join(format!("native/target/debug/build/{index}.rs")),
            "// compiler output\n",
        )
        .unwrap();
    }
    let unexpected = rx.recv_timeout(Duration::from_millis(750));
    assert!(
        unexpected.is_err(),
        "native build cache triggered rebuild: {unexpected:?}"
    );
}

#[cfg(windows)]
#[test]
fn generated_state_directory_metadata_does_not_trigger_source_rebuilds() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("generated-directory-events");
    fs::create_dir_all(root.join(".revenant")).unwrap();
    let (tx, rx) = mpsc::channel();
    let _watcher = NativeWatcher::start(&root, tx).unwrap();
    let directory = fs::OpenOptions::new()
        .write(true)
        .custom_flags(0x02000000)
        .open(root.join(".revenant"))
        .unwrap();
    directory
        .set_modified(std::time::SystemTime::now())
        .unwrap();
    let unexpected = rx.recv_timeout(Duration::from_millis(750));
    assert!(
        unexpected.is_err(),
        "generated state metadata triggered rebuild: {unexpected:?}"
    );
}

#[test]
fn watcher_tracks_native_path_dependency_and_sdk_but_ignores_build_outputs() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("desktop");
    for relative in [
        "native/src",
        "helper/src",
        ".revenant/sdk/crates/helper/src",
        ".revenant/sdk/target/debug",
    ] {
        fs::create_dir_all(root.join(relative)).unwrap();
    }
    fs::write(root.join("native/Cargo.toml"), "[package]\nname='watch-native'\nversion='0.3.0'\n[dependencies]\nhelper={path='../helper'}\n").unwrap();
    fs::write(
        root.join("helper/Cargo.toml"),
        "[package]\nname='helper'\nversion='0.3.0'\n",
    )
    .unwrap();
    fs::write(
        root.join(".revenant/sdk/Cargo.toml"),
        "[workspace]\nmembers=[]\n",
    )
    .unwrap();
    // Keep the negative fixture untouched while source edits are delivered.
    // Windows can publish directory-metadata events after file creation returns.
    let outputs = temp.path().join("output desktop");
    fs::create_dir_all(outputs.join(".revenant/sdk/target/debug")).unwrap();
    fs::write(
        outputs.join(".revenant/sdk/Cargo.toml"),
        "[workspace]\nmembers=[]\n",
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let watcher = NativeWatcher::start(&root, tx).unwrap();
    for relative in [
        "native/src/lib.rs",
        "helper/src/lib.rs",
        ".revenant/sdk/crates/helper/src/lib.rs",
    ] {
        while rx.try_recv().is_ok() {}
        fs::write(root.join(relative), "// edited source\n").unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    }
    // A fresh subscription to the same tree still sees delayed source-directory
    // metadata. Observe an independent, untouched tree for excluded outputs.
    drop(watcher);
    drop(rx);
    let (tx, rx) = mpsc::channel();
    let _watcher = NativeWatcher::start(&outputs, tx).unwrap();
    fs::write(
        outputs.join(".revenant/sdk/target/debug/generated.rs"),
        "// compiler output\n",
    )
    .unwrap();
    let unexpected = rx.recv_timeout(Duration::from_millis(500));
    assert!(
        unexpected.is_err(),
        "unexpected watcher notification: {unexpected:?}"
    );
}
