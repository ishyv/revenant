//! Root tooling boundaries, independent of native execution and client tests.
use revenant::{config::RevenantConfig, scaffold};
use serde_json::Value;
use std::{fs, path::Path};
use tempfile::TempDir;

fn project() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("portable-app");
    scaffold::create_project(&root, "portable-app").unwrap();
    (temp, root)
}
fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap()
}

#[test]
fn builtin_scaffold_needs_only_svelte_and_version_three_config() {
    let (_temp, root) = project();
    for relative in [
        "revenant.toml",
        "GETTING_STARTED.md",
        "web/package.json",
        "web/src/routes/+page.svelte",
        "web/src/routes/+layout.svelte",
        "web/src/routes/+layout.ts",
        "web/src/app.html",
        "web/tsconfig.json",
        ".revenant/desktop/Cargo.toml",
        ".revenant/desktop/src/main.rs",
        ".revenant/desktop/build.rs",
        ".revenant/desktop/tauri.conf.json",
    ] {
        assert!(root.join(relative).is_file(), "missing {relative}");
    }
    for relative in [
        "native",
        "rust",
        "web/src/lib/wasm.ts",
        "web/src/lib/revenant.ts",
    ] {
        assert!(
            !root.join(relative).exists(),
            "unexpected uncompiled author artifact {relative}"
        );
    }
    let config = RevenantConfig::load(&root).unwrap();
    assert_eq!(config.version, 3);
    assert_eq!(config.project.name, "portable-app");
    let bootstrap = read(&root, ".revenant/desktop/src/main.rs");
    assert!(bootstrap.contains("revenant::Application::new()"));
    assert!(
        bootstrap
            .find("export_contract_if_requested(&application)")
            .unwrap()
            < bootstrap.find("tauri::Builder::default()").unwrap()
    );
    assert!(!bootstrap.contains("{{"));
    let tauri: Value =
        serde_json::from_str(&read(&root, ".revenant/desktop/tauri.conf.json")).unwrap();
    assert_eq!(tauri["build"]["frontendDist"], "../../build/web");
    assert_eq!(
        tauri["bundle"]["windows"]["webviewInstallMode"]["type"],
        "offlineInstaller"
    );
    assert!(read(&root, "web/svelte.config.js").contains("fallback: 'index.html'"));
}

#[test]
fn sdk_snapshot_resolves_without_the_cli_checkout() {
    let (_temp, root) = project();
    let sdk = root.join(".revenant/sdk");
    let workspace: toml::Value = toml::from_str(&read(&sdk, "Cargo.toml")).unwrap();
    for member in workspace["workspace"]["members"].as_array().unwrap() {
        let manifest = sdk.join(member.as_str().unwrap()).join("Cargo.toml");
        assert!(
            manifest.is_file(),
            "missing embedded member {}",
            manifest.display()
        );
        let config: toml::Value = toml::from_str(&fs::read_to_string(manifest).unwrap()).unwrap();
        assert_eq!(config["package"]["version"].as_str(), Some("0.3.0"));
    }
    for relative in [
        "crates/revenant-desktop/src/tauri_host.rs",
        "crates/revenant-desktop/src/files/README.md",
        "crates/revenant-desktop/src/results/README.md",
        "crates/revenant-sdk/src/operations/contracts.rs",
        "packages/client/src/files.ts",
        "packages/client/src/ui/CollectionView.svelte",
        "vendor/hyvui-1.0.0.tgz",
    ] {
        assert!(
            sdk.join(relative).is_file(),
            "missing embedded source {relative}"
        );
    }
    let client: Value = serde_json::from_str(&read(&sdk, "packages/client/package.json")).unwrap();
    assert_eq!(client["version"], "0.3.0");
    let host: toml::Value = toml::from_str(&read(&root, ".revenant/desktop/Cargo.toml")).unwrap();
    for name in ["revenant", "revenant-desktop"] {
        let relative = host["dependencies"][name]["path"].as_str().unwrap();
        assert!(
            root.join(".revenant/desktop")
                .join(relative)
                .join("Cargo.toml")
                .is_file()
        );
        assert!(!relative.contains(env!("CARGO_MANIFEST_DIR")));
    }
}

#[test]
fn host_refresh_preserves_author_source_sdk_edits_and_publication() {
    let (_temp, root) = project();
    let files = [
        "web/src/routes/+page.svelte",
        ".revenant/sdk/crates/revenant-sdk/src/lib.rs",
        "web/src/lib/revenant.ts",
    ];
    for relative in files {
        fs::write(root.join(relative), format!("author-owned: {relative}\n")).unwrap();
    }
    scaffold::prepare_host(&root, &RevenantConfig::load(&root).unwrap()).unwrap();
    for relative in files {
        assert_eq!(read(&root, relative), format!("author-owned: {relative}\n"));
    }
}

#[test]
fn optional_library_package_and_custom_lib_path_are_detected() {
    let (_temp, root) = project();
    fs::create_dir(root.join("native")).unwrap();
    fs::write(
        root.join("native/Cargo.toml"),
        "[package]\nname='different-native-name'\nversion='0.3.0'\n[lib]\npath='operations.rs'\n",
    )
    .unwrap();
    fs::write(
        root.join("native/operations.rs"),
        "// Author-owned library\n",
    )
    .unwrap();
    scaffold::prepare_host(&root, &RevenantConfig::load(&root).unwrap()).unwrap();
    let host: toml::Value = toml::from_str(&read(&root, ".revenant/desktop/Cargo.toml")).unwrap();
    assert_eq!(
        host["dependencies"]["revenant-app"]["package"].as_str(),
        Some("different-native-name")
    );
    assert!(read(&root, ".revenant/desktop/src/main.rs").contains("revenant_app::app()"));
    let zed: Value = serde_json::from_str(&read(&root, ".zed/settings.json")).unwrap();
    let projects = zed
        .pointer("/lsp/rust-analyzer/initialization_options/linkedProjects")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(projects.contains(&Value::from("./native/Cargo.toml")));
}

#[test]
fn zed_jsonc_merge_preserves_existing_projects_settings_and_comments() {
    let (_temp, root) = project();
    let initial = r#"{
      // Keep this project note, including Unicode: café.
      "lsp": { "rust-analyzer": { "initialization_options": {
        "linkedProjects": ["./other/Cargo.toml",], "checkOnSave": false,
      }, }, },
      "file_scan_inclusions": ["fixtures/**",],
      "project_panel": { "hide_hidden": true },
      "custom": { "url": "https://example.invalid/*literal*/", "nested": [1, 2,] },
    }"#;
    fs::write(root.join(".zed/settings.json"), initial).unwrap();
    let config = RevenantConfig::load(&root).unwrap();
    scaffold::prepare_host(&root, &config).unwrap();
    let updated = read(&root, ".zed/settings.json");
    for preserved in [
        "// Keep this project note, including Unicode: café.",
        "\"checkOnSave\": false",
        "\"hide_hidden\": true",
        "\"custom\": { \"url\": \"https://example.invalid/*literal*/\", \"nested\": [1, 2,] }",
        "./other/Cargo.toml",
        "fixtures/**",
        "./.revenant/desktop/Cargo.toml",
        ".revenant/desktop/**",
    ] {
        assert!(
            updated.contains(preserved),
            "missing preserved/additive setting: {preserved}"
        );
    }
    scaffold::prepare_host(&root, &config).unwrap();
    assert_eq!(
        updated,
        read(&root, ".zed/settings.json"),
        "preparation must be idempotent"
    );
}

#[test]
fn invalid_zed_settings_are_preserved_without_blocking_preparation() {
    let (_temp, root) = project();
    for invalid in ["{ unterminated", "{ /* unterminated", "{\"lsp\":false}"] {
        fs::write(root.join(".zed/settings.json"), invalid).unwrap();
        scaffold::prepare_host(&root, &RevenantConfig::load(&root).unwrap()).unwrap();
        assert_eq!(read(&root, ".zed/settings.json"), invalid);
    }
}

#[test]
fn unsupported_config_and_old_sdk_preserve_successful_artifacts() {
    let (_temp, root) = project();
    let config = RevenantConfig::load(&root).unwrap();
    let before = read(&root, ".revenant/desktop/src/main.rs");
    fs::write(
        root.join("web/src/lib/revenant.ts"),
        "last successful generation",
    )
    .unwrap();
    for text in [
        "[project]\nname='portable-app'",
        "version=2\n[project]\nname='portable-app'",
        "version=3\n[project]\nname='portable-app'\n[server]\nport=8787",
    ] {
        fs::write(root.join("revenant.toml"), text).unwrap();
        assert!(RevenantConfig::load(&root).is_err());
        assert_eq!(
            read(&root, "web/src/lib/revenant.ts"),
            "last successful generation"
        );
        assert_eq!(read(&root, ".revenant/desktop/src/main.rs"), before);
    }
    fs::write(
        root.join(".revenant/sdk/crates/revenant-sdk/Cargo.toml"),
        "[package]\nversion='0.2.0'\n",
    )
    .unwrap();
    assert!(
        scaffold::prepare_host(&root, &config)
            .unwrap_err()
            .to_string()
            .contains("predates desktop 0.3")
    );
    assert_eq!(
        read(&root, "web/src/lib/revenant.ts"),
        "last successful generation"
    );
    assert_eq!(read(&root, ".revenant/desktop/src/main.rs"), before);
}
