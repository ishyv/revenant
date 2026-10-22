//! Application metadata and CSP survive repeated host regeneration.
use revenant::{config::RevenantConfig, scaffold};
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

#[test]
fn final_executable_lookup_preserves_completed_build_and_host() {
    use revenant::{inspection, toolchain::desktop::DesktopBuild};
    let temp = TempDir::new().unwrap();
    scaffold::create_project(temp.path(), "verification-app").unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    let config = RevenantConfig::load(&root).unwrap();
    inspection::record_build_start(&root, "native:release").unwrap();
    scaffold::prepare_host(&root, &config).unwrap();
    inspection::record_build_finish(&root, true, vec![]).unwrap();
    let record = root.join(".revenant/inspection/build.json");
    let host = root.join(".revenant/desktop/src/main.rs");
    let before_record = fs::read(&record).unwrap();
    let before_host = fs::read(&host).unwrap();
    let executable = DesktopBuild::executable_path(&root, &config, true);
    assert_eq!(
        executable.parent().unwrap(),
        root.join(".revenant/target/release")
    );
    assert_eq!(fs::read(&record).unwrap(), before_record);
    assert_eq!(fs::read(&host).unwrap(), before_host);
    let attempt: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(attempt["status"], "succeeded");
}

#[test]
#[ignore = "resolves an isolated generated desktop host dependency lock"]
fn prepared_build_embeds_its_own_attempt_identity() {
    use revenant::{inspection::ProjectInspector, toolchain::desktop::DesktopBuild};
    let temp = TempDir::new().unwrap();
    scaffold::create_project(temp.path(), "attempt-identity").unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    let config = RevenantConfig::load(&root).unwrap();
    for edit in ["first", "second"] {
        fs::write(
            root.join("web/src/identity.ts"),
            format!("export const edit = '{edit}';"),
        )
        .unwrap();
        let build = DesktopBuild::new(&root, &config, false).unwrap();
        let attempt =
            ProjectInspector::open(&root).unwrap().snapshot().unwrap()["buildAttempt"].clone();
        let host = fs::read_to_string(build.host.join("src/main.rs")).unwrap();
        for expected in [
            attempt["id"].as_str().unwrap(),
            attempt["sourceFingerprint"]["digest"].as_str().unwrap(),
        ] {
            assert!(
                host.contains(expected),
                "generated host omitted current identity {expected}"
            );
        }
        drop(build);
    }
}

#[test]
fn configured_identity_and_project_icons_survive_preparation() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("app");
    scaffold::create_project(&root, "identity-app").unwrap();
    fs::create_dir(root.join("assets")).unwrap();
    for extension in ["png", "ico", "icns"] {
        fs::copy(
            root.join(format!(".revenant/desktop/icons/icon.{extension}")),
            root.join(format!("assets/图标.{extension}")),
        )
        .unwrap();
    }
    fs::write(
        root.join("revenant.toml"),
        r#"
version = 3
[project]
name = "identity-app"
identifier = "org.example.identity"
version = "1.2.3"
[desktop]
title = "Authored title"
icons = ["assets/图标.png", "assets/图标.ico", "assets/图标.icns"]
"#,
    )
    .unwrap();
    let config = RevenantConfig::load(&root).unwrap();
    scaffold::prepare_host(&root, &config).unwrap();
    let first = fs::read(root.join(".revenant/desktop/tauri.conf.json")).unwrap();
    scaffold::prepare_host(&root, &config).unwrap();
    assert_eq!(
        first,
        fs::read(root.join(".revenant/desktop/tauri.conf.json")).unwrap()
    );
    let json: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(json["version"], "1.2.3");
    assert_eq!(json["identifier"], "org.example.identity");
    assert_eq!(json["app"]["windows"][0]["title"], "Authored title");
    assert_eq!(
        json["bundle"]["windows"]["nsis"]["installerIcon"],
        "icons/application-1.ico"
    );
    assert_eq!(
        json["bundle"]["windows"]["nsis"]["uninstallerIcon"],
        "icons/application-1.ico"
    );
    for (index, extension) in ["png", "ico", "icns"].iter().enumerate() {
        let copied = format!("icons/application-{index}.{extension}");
        assert_eq!(json["bundle"]["icon"][index], copied);
        assert_eq!(
            fs::read(root.join(".revenant/desktop").join(copied)).unwrap(),
            fs::read(root.join(format!("assets/图标.{extension}"))).unwrap()
        );
    }
    let host: toml::Value =
        toml::from_str(&fs::read_to_string(root.join(".revenant/desktop/Cargo.toml")).unwrap())
            .unwrap();
    assert_eq!(host["package"]["version"].as_str(), Some("1.2.3"));
    assert_eq!(
        host["dependencies"]["revenant"]["version"].as_str(),
        Some("0.4.0")
    );
}

#[test]
fn incomplete_authored_icon_sets_never_silently_use_framework_branding() {
    let temp = TempDir::new().unwrap();
    scaffold::create_project(temp.path(), "authored-icons").unwrap();
    let previous = fs::read(temp.path().join(".revenant/desktop/tauri.conf.json")).unwrap();
    for extension in ["png", "ico", "icns"] {
        fs::copy(
            temp.path()
                .join(format!(".revenant/desktop/icons/icon.{extension}")),
            temp.path().join(format!("authored.{extension}")),
        )
        .unwrap();
    }
    let config = RevenantConfig::load(temp.path()).unwrap();
    let mut incomplete = config.clone();
    incomplete.desktop.icons = Some(vec!["authored.ico".into()]);
    let error = scaffold::prepare_host(temp.path(), &incomplete).unwrap_err();
    assert!(error.to_string().contains("PNG"));
    assert_eq!(
        fs::read(temp.path().join(".revenant/desktop/tauri.conf.json")).unwrap(),
        previous
    );
    #[cfg(any(windows, target_os = "macos"))]
    {
        incomplete.desktop.icons = Some(vec!["authored.png".into()]);
        let error = scaffold::prepare_host(temp.path(), &incomplete).unwrap_err();
        #[cfg(windows)]
        assert!(error.to_string().contains("ICO"));
        #[cfg(target_os = "macos")]
        assert!(error.to_string().contains("ICNS"));
        assert_eq!(
            fs::read(temp.path().join(".revenant/desktop/tauri.conf.json")).unwrap(),
            previous
        );
    }
}

#[test]
fn packaged_policy_blocks_remote_scripts_and_eval_and_development_bounds_hmr() {
    let temp = TempDir::new().unwrap();
    scaffold::create_project(temp.path(), "secure-app").unwrap();
    let json: Value = serde_json::from_slice(
        &fs::read(temp.path().join(".revenant/desktop/tauri.conf.json")).unwrap(),
    )
    .unwrap();
    let policy = &json["app"]["security"]["csp"];
    assert_eq!(policy["script-src"], serde_json::json!(["'self'"]));
    assert!(!policy.to_string().contains("unsafe-eval"));
    assert!(!policy.to_string().contains("ws://"));
    let development = &json["app"]["security"]["devCsp"];
    assert!(
        development["connect-src"]
            .as_array()
            .unwrap()
            .contains(&Value::String("ws://127.0.0.1:5173".into()))
    );
    assert_eq!(development["script-src"], policy["script-src"]);
    fs::write(
        temp.path().join("revenant.toml"),
        r#"
version = 3
[project]
name = "secure-app"
[desktop.csp_allowances]
connect-src = ["https://api.example.test"]
"#,
    )
    .unwrap();
    scaffold::prepare_host(temp.path(), &RevenantConfig::load(temp.path()).unwrap()).unwrap();
    let json: Value = serde_json::from_slice(
        &fs::read(temp.path().join(".revenant/desktop/tauri.conf.json")).unwrap(),
    )
    .unwrap();
    assert!(
        json["app"]["security"]["csp"]["connect-src"]
            .as_array()
            .unwrap()
            .contains(&Value::String("https://api.example.test".into()))
    );
    assert_eq!(
        json["app"]["security"]["csp"]["script-src"],
        policy["script-src"]
    );
}

#[test]
fn invalid_version_icons_and_directive_injection_fail_actionably() {
    let temp = TempDir::new().unwrap();
    scaffold::create_project(temp.path(), "invalid-app").unwrap();
    for extra in [
        "[project]\nname = \"invalid-app\"\nversion = \"not-semver\"",
        "[project]\nname = \"invalid-app\"\n[desktop]\nicons = []",
        "[project]\nname = \"invalid-app\"\n[desktop.csp_allowances]\nconnect-src = [\"https://example.test; script-src *\"]",
    ] {
        fs::write(
            temp.path().join("revenant.toml"),
            format!("version = 3\n{extra}\n"),
        )
        .unwrap();
        assert!(RevenantConfig::load(temp.path()).is_err());
    }
    fs::write(temp.path().join("bad.ico"), [0, 0, 1, 0, 1, 0]).unwrap();
    fs::write(
        temp.path().join("revenant.toml"),
        "version = 3\n[project]\nname = \"invalid-app\"\n[desktop]\nicons = [\"bad.ico\"]",
    )
    .unwrap();
    assert!(
        scaffold::prepare_host(temp.path(), &RevenantConfig::load(temp.path()).unwrap())
            .unwrap_err()
            .to_string()
            .contains("invalid ico")
    );
}
