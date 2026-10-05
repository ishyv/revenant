use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};
use tempfile::TempDir;
/// Compile the tiny fixture once without another Cargo/native slot.
pub fn fixture() -> &'static Path {
    static FIXTURE: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let temp = TempDir::new().unwrap();
            let binary = temp.path().join(if cfg!(windows) {
                "tooling host.exe"
            } else {
                "tooling host"
            });
            let output = Command::new("rustc")
                .args(["--edition", "2024"])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tooling_host.rs"))
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "fixture compiler: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            (temp, binary)
        })
        .1
}
