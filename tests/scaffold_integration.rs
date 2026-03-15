use tempfile::TempDir;

#[test]
fn scaffold_creates_expected_structure() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("test-project");

    revenant::scaffold::create_project(&project_dir, "test-project").unwrap();

    // All 10 template files must exist
    let expected_files = [
        "revenant.toml",
        "rust/Cargo.toml",
        "rust/src/lib.rs",
        "web/package.json",
        "web/svelte.config.js",
        "web/vite.config.js",
        "web/src/app.html",
        "web/src/routes/+page.svelte",
        "web/src/lib/wasm.ts",
        "GETTING_STARTED.md",
    ];

    for file in &expected_files {
        assert!(
            project_dir.join(file).is_file(),
            "expected file missing: {file}"
        );
    }

    // Verify substitution happened — no raw placeholders
    let toml_content = std::fs::read_to_string(project_dir.join("revenant.toml")).unwrap();
    assert!(toml_content.contains("test-project"));
    assert!(!toml_content.contains("{project_name}"));

    // Verify revenant.toml parses correctly
    let config: toml::Value = toml::from_str(&toml_content).unwrap();
    assert_eq!(
        config["project"]["name"].as_str().unwrap(),
        "test-project"
    );
}
