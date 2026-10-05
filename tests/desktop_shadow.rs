//! Real Windows image locking and compiler-artifact lifetime regression.
#![cfg(windows)]
use revenant::{
    config::RevenantConfig,
    scaffold,
    toolchain::{desktop::DesktopBuild, process::ManagedProcess},
};
use std::{
    fs,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn compile(executable: &Path, generation: &str) {
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    let output = Command::new("rustc")
        .args(["--edition", "2024", "--crate-name", "shadow_desktop"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shadow_desktop.rs"))
        .arg("-o")
        .arg(executable)
        .env("REVENANT_SHADOW_GENERATION", generation)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "compiler could not replace its artifact: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn ready(root: &Path) -> (PathBuf, SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let port = fs::read_to_string(root.join("shadow.port"))
            .ok()
            .and_then(|text| text.parse::<u16>().ok());
        let image = fs::read_to_string(root.join("shadow.image")).ok();
        if let (Some(port), Some(image)) = (port, image) {
            return (
                PathBuf::from(image),
                SocketAddr::from(([127, 0, 0, 1], port)),
            );
        }
        assert!(Instant::now() < deadline, "loaded image did not start");
        thread::sleep(Duration::from_millis(10));
    }
}
fn reset(root: &Path) {
    for name in ["shadow.port", "shadow.image", "shadow.generation"] {
        let _ = fs::remove_file(root.join(name));
    }
}
fn open(endpoint: SocketAddr) -> bool {
    TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).is_ok()
}

fn shadow_project() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("shadow-proof");
    scaffold::create_project(&root, "shadow-proof").unwrap();
    (temp, root)
}

#[test]
fn loaded_shadow_allows_recompile_and_reaps_descendants_before_directory_cleanup() {
    let (_temp, root) = shadow_project();
    exercise_shadow_lifecycle(root);
}

#[test]
fn loaded_shadow_supports_alternate_case_project_path() {
    let (_temp, root) = shadow_project();
    // Create the directory before aliasing it so its stored spelling stays distinct.
    let alias = root.with_file_name("SHADOW-PROOF");
    assert_ne!(alias, root);
    assert_eq!(
        dunce::canonicalize(&alias).unwrap(),
        dunce::canonicalize(&root).unwrap()
    );
    exercise_shadow_lifecycle(alias);
}

#[test]
fn loaded_shadow_supports_verbatim_project_path() {
    let (_temp, root) = shadow_project();
    let alias = fs::canonicalize(&root).unwrap();
    assert_ne!(alias, dunce::canonicalize(&root).unwrap());
    exercise_shadow_lifecycle(alias);
}

fn exercise_shadow_lifecycle(root: PathBuf) {
    let build = DesktopBuild::new(&root, &RevenantConfig::load(&root).unwrap(), false).unwrap();
    compile(&build.executable, "first");
    let (logs, _receiver) = mpsc::channel();
    // Negative control: Windows really locks this executable when launched directly.
    let direct = ManagedProcess::spawn(
        &root,
        &build.executable.to_string_lossy(),
        &[],
        "direct-control",
        logs.clone(),
    )
    .unwrap();
    let (_, direct_endpoint) = ready(&root);
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .open(&build.executable)
            .is_err()
    );
    drop(direct);
    assert!(
        !open(direct_endpoint),
        "negative-control descendant survived cleanup"
    );
    reset(&root);

    let mut first = build.launch(&root, logs.clone()).unwrap();
    let (first_image, first_endpoint) = ready(&root);
    assert_ne!(first_image, build.executable);
    assert_eq!(first_image.file_name(), build.executable.file_name());
    let expected_runs = root.join(".revenant/run");
    // Windows aliases can identify the same directory without matching lexically.
    // Resolve both sides, just as the launcher resolves the project root.
    let resolved_image = dunce::canonicalize(&first_image).unwrap();
    let resolved_runs = dunce::canonicalize(&expected_runs).unwrap();
    assert!(
        resolved_image.starts_with(&resolved_runs),
        "shadow image {first_image:?} (resolved {resolved_image:?}) is outside \
         run directory {expected_runs:?} (resolved {resolved_runs:?})"
    );
    let first_bytes = fs::read(&first_image).unwrap();
    // A real compiler relinks the same output while the original shadow is loaded.
    compile(&build.executable, "second");
    assert!(first.try_wait().unwrap().is_none());
    assert!(open(first_endpoint));
    assert_eq!(fs::read(&first_image).unwrap(), first_bytes);
    assert_ne!(fs::read(&build.executable).unwrap(), first_bytes);
    reset(&root);
    let mut second = build.launch(&root, logs).unwrap();
    let (second_image, second_endpoint) = ready(&root);
    assert_ne!(first_image.parent(), second_image.parent());
    assert_eq!(
        fs::read_to_string(root.join("shadow.generation")).unwrap(),
        "second"
    );
    drop(first);
    assert!(!open(first_endpoint));
    assert!(
        !first_image.parent().unwrap().exists(),
        "first shadow directory remained locked by a descendant"
    );
    assert!(second.try_wait().unwrap().is_none());
    assert!(open(second_endpoint));
    drop(second);
    assert!(!open(second_endpoint));
    assert_eq!(fs::read_dir(root.join(".revenant/run")).unwrap().count(), 0);
}

#[test]
fn failed_shadow_spawn_removes_only_its_owned_directory() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("bad-shadow");
    scaffold::create_project(&root, "bad-shadow").unwrap();
    let build = DesktopBuild::new(&root, &RevenantConfig::load(&root).unwrap(), false).unwrap();
    fs::create_dir_all(build.executable.parent().unwrap()).unwrap();
    fs::write(&build.executable, "not an executable").unwrap();
    let (logs, _rx) = mpsc::channel();
    assert!(build.launch(&root, logs).is_err());
    assert_eq!(fs::read_dir(root.join(".revenant/run")).unwrap().count(), 0);
    assert_eq!(
        fs::read_to_string(build.executable).unwrap(),
        "not an executable"
    );
}
