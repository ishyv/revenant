use revenant::toolchain::desktop::NativeWatcher;
use std::{fs, sync::mpsc, time::Duration};
use tempfile::TempDir;

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
    let (tx, rx) = mpsc::channel();
    let _watcher = NativeWatcher::start(&root, tx).unwrap();
    for relative in [
        "native/src/lib.rs",
        "helper/src/lib.rs",
        ".revenant/sdk/crates/helper/src/lib.rs",
    ] {
        while rx.try_recv().is_ok() {}
        fs::write(root.join(relative), "// edited source\n").unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    }
    // Let already-delivered events drain before observing the excluded tree.
    while rx.recv_timeout(Duration::from_millis(150)).is_ok() {}
    fs::write(
        root.join(".revenant/sdk/target/debug/generated.rs"),
        "// compiler output\n",
    )
    .unwrap();
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "build output triggered a source rebuild"
    );
}
