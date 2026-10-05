//! Opt-in real CLI proof. Run serially after the shared Cargo slot is released.
//! This compiles author applications; native dispatch and client semantics belong
//! to their owners' suites. No GUI, HTTP backend or system shutdown is required.
use revenant::{
    commands::build,
    config::RevenantConfig,
    toolchain::{desktop::DesktopBuild, process},
};
use serde_json::Value;
use std::{fs, path::Path};
use tempfile::TempDir;

fn manifest(root: &Path, executable: &Path) -> Value {
    let json = process::run_capture(
        root,
        &executable.to_string_lossy(),
        &["--revenant-contract"],
        false,
    )
    .unwrap();
    serde_json::from_str(&json).unwrap()
}

#[test]
#[ignore = "requires released Cargo slot, platform desktop prerequisites and npm dependencies"]
fn new_app_and_custom_native_compile_outside_checkout_and_preserve_failed_build_artifacts() {
    let temp = TempDir::new().unwrap();
    let cli = std::env::var_os("REVENANT_TEST_CLI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_BIN_EXE_revenant")).to_path_buf());
    process::run_blocking(
        temp.path(),
        &cli.to_string_lossy(),
        &["new", "portable-proof"],
        true,
    )
    .unwrap();
    let root = temp.path().join("portable-proof");
    assert!(!root.starts_with(env!("CARGO_MANIFEST_DIR")));
    assert!(
        !root.join("native").exists(),
        "built-in authoring must not require Rust application files"
    );
    let config = RevenantConfig::load(&root).unwrap();
    let executable = DesktopBuild::new(&root, &config, false).unwrap().executable;
    let builtin = manifest(&root, &executable);
    assert_eq!(builtin["version"], 3);
    assert_eq!(builtin["protocol"], 2);
    let ids: Vec<_> = builtin["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| op["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"files.checksum"));
    assert!(ids.contains(&"media.readMetadata"));

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
    let generation = build::build_rust(&root, &config, false, true).unwrap();
    let compiled = manifest(&root, &executable);
    let operation = compiled["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|op| op["id"] == "records.normalize")
        .unwrap();
    assert!(
        operation["description"]
            .as_str()
            .unwrap()
            .contains("portable CLI proof")
    );
    for side in ["input", "output"] {
        let types = operation[side]["typescript"].as_str().unwrap();
        assert!(types.contains("ContractValue"));
        assert!(types.contains("title"));
        assert!(types.contains("Human-readable title"));
    }
    let facade = root
        .join("web/src/lib/.revenant")
        .join(&generation)
        .join("facade.ts");
    let facade_text = fs::read_to_string(&facade).unwrap();
    assert!(facade_text.contains("RecordsNormalize.Input.ContractValue"));
    assert!(facade_text.contains("native/src/records.rs#L"));
    process::run_blocking(&root.join("web"), "npm", &["run", "check"], true).unwrap();
    process::run_blocking(&root.join("web"), "npm", &["run", "build"], true).unwrap();
    let frontend = root.join("build/web/index.html");
    assert!(
        frontend.is_file(),
        "the portable app must produce a static SPA"
    );
    let artifacts = [root.join("web/src/lib/revenant.ts"), facade, frontend];
    let before: Vec<_> = artifacts
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect();
    let source = root.join("native/src/lib.rs");
    let original = fs::read_to_string(&source).unwrap();
    fs::write(
        &source,
        format!("{original}\ncompile_error!(\"intentional CLI artifact-preservation failure\");\n"),
    )
    .unwrap();
    let error = build::build_rust(&root, &config, false, false).unwrap_err();
    assert!(format!("{error:#}").contains("intentional CLI artifact-preservation failure"));
    for (path, bytes) in artifacts.iter().zip(&before) {
        assert_eq!(
            &fs::read(path).unwrap(),
            bytes,
            "failed native build replaced {}",
            path.display()
        );
    }
    assert!(
        fs::read_to_string(&source)
            .unwrap()
            .contains("compile_error!"),
        "failed build must preserve author source"
    );
    fs::write(source, original).unwrap();
    assert_eq!(
        build::build_rust(&root, &config, false, false).unwrap(),
        generation,
        "repair must recover the same contract"
    );
}
