use revenant::inspection::{
    Diagnostic, ProjectInspector, record_build_finish, record_build_start, source_fingerprint,
};
use serde_json::json;
use std::{fs, path::Path};

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("native/src")).unwrap();
    fs::create_dir_all(dir.path().join("web/src/lib/.revenant/generation")).unwrap();
    fs::write(
        dir.path().join("revenant.toml"),
        "version = 3\n[project]\nname = 'inspection-test'\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("native/Cargo.toml"),
        "[package]\nname = 'inspection-test'\nversion = '0.1.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("native/src/lib.rs"),
        "pub fn normalize(text: String) -> String { text.trim().to_owned() }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("web/src/lib/revenant.ts"),
        "export * from './.revenant/generation/facade.ts';\n",
    )
    .unwrap();
    fs::write(dir.path().join("web/src/lib/.revenant/generation/facade.ts"), "export interface Operations { titles: { normalize: (input: { text: string }) => Promise<string> } }\n").unwrap();
    fs::write(dir.path().join("web/src/lib/.revenant/generation/revenant.contract.json"), serde_json::to_vec(&json!({
        "version":6,"protocol":5,"digest":format!("sha256:{}", "a".repeat(64)),
        "operations":[{"id":"titles.normalize","description":"Trim whitespace.","input":{"schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},"typescript":"type ContractValue = { text: string };"},"output":{"schema":{"type":"string"},"typescript":"type ContractValue = string;"},"arguments":["text"],"effect":"read_only","independent":true,"agent":true,"source":{"file":"src/lib.rs","line":1,"module":"inspection_test"}}]
    })).unwrap()).unwrap();
    dir
}

fn kit_sync_fixture(root: &Path, fail: bool) {
    let web = root.join("web");
    let kit = web.join("node_modules/@sveltejs/kit");
    fs::create_dir_all(&kit).unwrap();
    fs::write(web.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(
        web.join("tsconfig.json"),
        r#"{"extends":"$app/tsconfig","include":["src/**/*.ts"]}"#,
    )
    .unwrap();
    fs::write(
        kit.join("package.json"),
        r#"{"type":"module","bin":{"svelte-kit":"sync.mjs"}}"#,
    )
    .unwrap();
    fs::write(kit.join("sync.mjs"), if fail {
        "console.error('fixture sync failure'); process.exit(1);"
    } else {
        r#"import fs from 'node:fs';
if (process.argv[2] !== 'sync') throw new Error('unexpected command');
fs.mkdirSync('node_modules/$app', { recursive: true });
fs.writeFileSync('node_modules/$app/tsconfig.json', JSON.stringify({ compilerOptions: { strict: true, noEmit: true, allowImportingTsExtensions: true, target: 'ESNext', module: 'ESNext', moduleResolution: 'bundler' } }));
fs.writeFileSync('synced.txt', 'prepared before diagnostics');"#
    }).unwrap();
}

#[test]
fn configured_icons_outside_source_roots_affect_source_identity() {
    for extension in ["png", "PNG", "ICO", "IcNs"] {
        let dir = project();
        fs::create_dir_all(dir.path().join("assets")).unwrap();
        let config = dir.path().join("revenant.toml");
        let asset = format!("assets/app.{extension}");
        fs::write(
            &config,
            format!(
                "{}\n[desktop]\nicons = ['{asset}']\n",
                fs::read_to_string(&config).unwrap()
            ),
        )
        .unwrap();
        let icon = dir.path().join(asset);
        fs::write(&icon, b"original icon bytes").unwrap();
        let before = source_fingerprint(dir.path()).unwrap();
        fs::write(&icon, b"changed icon bytes").unwrap();
        let after = source_fingerprint(dir.path()).unwrap();
        assert!(before.complete && after.complete);
        assert_ne!(
            before.digest, after.digest,
            "configured {extension} build asset must affect source identity"
        );
    }
}

#[test]
fn embedded_shader_and_font_changes_invalidate_source_identity() {
    let dir = project();
    let assets = dir
        .path()
        .join(".revenant/sdk/crates/revenant-graphics/src");
    fs::create_dir_all(&assets).unwrap();
    for name in ["scene.wgsl", "label.ttf"] {
        let path = assets.join(name);
        fs::write(&path, b"original embedded asset").unwrap();
        let before = source_fingerprint(dir.path()).unwrap();
        fs::write(&path, b"changed embedded asset").unwrap();
        let after = source_fingerprint(dir.path()).unwrap();
        assert!(before.complete && after.complete);
        assert_ne!(
            before.digest, after.digest,
            "{name} must affect build identity"
        );
    }
}

#[test]
fn frontend_check_syncs_installed_kit_config_before_semantic_diagnostics() {
    let dir = project();
    kit_sync_fixture(dir.path(), false);
    let inspector = ProjectInspector::open(dir.path()).unwrap();
    let report = inspector.check("frontend").unwrap();
    assert_eq!(report["succeeded"], true, "{report:#}");
    assert_eq!(
        fs::read_to_string(dir.path().join("web/synced.txt")).unwrap(),
        "prepared before diagnostics"
    );
    assert_eq!(report["steps"][0]["stage"], "frontend_prepare");
    assert_eq!(report["steps"][1]["stage"], "frontend");
}

#[test]
fn frontend_check_reports_kit_sync_failure_without_misleading_type_errors() {
    let dir = project();
    kit_sync_fixture(dir.path(), true);
    let inspector = ProjectInspector::open(dir.path()).unwrap();
    let report = inspector.check("frontend").unwrap();
    assert_eq!(report["succeeded"], false);
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "sveltekit_sync_failed"
                && diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("fixture sync failure")),
        "{report:#}"
    );
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic["code"].as_str(), Some("6053" | "5097")))
    );
}

fn generation(root: &Path, fingerprint: &str) {
    fs::write(root.join("web/src/lib/.revenant/generation/generation.json"), serde_json::to_vec(&json!({"version":6,"protocol":5,"digest":format!("sha256:{}", "a".repeat(64)),"sourceFingerprint":fingerprint})).unwrap()).unwrap();
}

#[test]
fn fingerprint_is_deterministic_and_ignores_generated_and_dependency_artifacts() {
    let dir = project();
    let before = source_fingerprint(dir.path()).unwrap();
    fs::write(
        dir.path()
            .join("web/src/lib/.revenant/generation/facade.ts"),
        "changed generated artifact",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("web/node_modules/ignored")).unwrap();
    fs::write(
        dir.path().join("web/node_modules/ignored/secret.ts"),
        "ignored dependency",
    )
    .unwrap();
    assert_eq!(
        before.digest,
        source_fingerprint(dir.path()).unwrap().digest
    );
    fs::write(
        dir.path().join("native/src/lib.rs"),
        "pub fn normalize() {}\n",
    )
    .unwrap();
    assert_ne!(
        before.digest,
        source_fingerprint(dir.path()).unwrap().digest
    );
    assert!(before.complete);
}

#[test]
fn dependency_resolution_locks_are_part_of_each_source_identity() {
    let dir = project();
    for relative in [
        "native/Cargo.lock",
        ".revenant/sdk/Cargo.lock",
        ".revenant/desktop/Cargo.lock",
    ] {
        let path = dir.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let before = source_fingerprint(dir.path()).unwrap().digest;
        fs::write(
            &path,
            "version = 4\n[[package]]\nname = 'before'\nversion = '1.0.0'\n",
        )
        .unwrap();
        let created = source_fingerprint(dir.path()).unwrap().digest;
        assert_ne!(
            before, created,
            "adding {relative} must change compiled source identity"
        );
        fs::write(
            &path,
            "version = 4\n[[package]]\nname = 'after'\nversion = '1.0.0'\n",
        )
        .unwrap();
        assert_ne!(
            created,
            source_fingerprint(dir.path()).unwrap().digest,
            "editing {relative} must change compiled source identity"
        );
    }
}

#[test]
fn incomplete_build_identity_cannot_become_current_after_omitted_entries_disappear() {
    let dir = project();
    let normal = source_fingerprint(dir.path()).unwrap();
    let excess = dir.path().join("web/src/z_budget");
    fs::create_dir_all(&excess).unwrap();
    for index in 0..8300 {
        fs::write(excess.join(format!("entry{index:05}.tmp")), "").unwrap();
    }
    record_build_start(dir.path(), "native:development").unwrap();
    let incomplete = source_fingerprint(dir.path()).unwrap();
    assert!(!incomplete.complete);
    assert_eq!(
        incomplete.digest, normal.digest,
        "the omitted nonsource entries do not alter hashed source bytes"
    );
    let captured = revenant::inspection::latest_build_source_fingerprint(dir.path()).unwrap();
    assert_eq!(
        captured, None,
        "partial capture must never be published as authoritative"
    );
    fs::remove_dir_all(&excess).unwrap();
    assert!(source_fingerprint(dir.path()).unwrap().complete);
    generation(dir.path(), captured.as_deref().unwrap_or(""));
    let mut snapshot = ProjectInspector::open(dir.path())
        .unwrap()
        .snapshot()
        .unwrap();
    revenant::inspection::attach_runtime(
        &mut snapshot,
        json!({"instanceId":"partial-build","sourceFingerprint":captured,"contractDigest":format!("sha256:{}","a".repeat(64))}),
    );
    assert_eq!(snapshot["consistency"]["sourceToRuntime"], "unknown");
    assert_ne!(snapshot["consistency"]["sourceToFacade"], "current");
}

#[test]
fn source_edits_and_failed_build_preserve_facade_and_report_old_build() {
    let dir = project();
    let initial = source_fingerprint(dir.path()).unwrap();
    generation(dir.path(), &initial.digest);
    let inspector = ProjectInspector::open(dir.path()).unwrap();
    let before = inspector.snapshot().unwrap();
    assert_eq!(before["consistency"]["sourceToFacade"], "current");
    let pointer = fs::read(dir.path().join("web/src/lib/revenant.ts")).unwrap();
    fs::write(
        dir.path().join("native/src/lib.rs"),
        "compile_error!(\"intentional\");",
    )
    .unwrap();
    record_build_start(dir.path(), "native").unwrap();
    record_build_finish(
        dir.path(),
        false,
        vec![Diagnostic {
            stage: "rust".into(),
            code: Some("E0308".into()),
            severity: "error".into(),
            message: "mismatched types".into(),
            file: Some("native/src/lib.rs".into()),
            line: Some(1),
            column: Some(1),
            end_line: Some(1),
            end_column: Some(3),
        }],
    )
    .unwrap();
    let after = inspector.snapshot().unwrap();
    assert_eq!(after["consistency"]["sourceToFacade"], "stale");
    assert_eq!(after["buildAttempt"]["status"], "failed");
    assert_eq!(after["diagnostics"][0]["code"], "E0308");
    assert_eq!(
        fs::read(dir.path().join("web/src/lib/revenant.ts")).unwrap(),
        pointer
    );
}

#[test]
fn compiled_operations_explain_contract_effects_and_source_without_guessing() {
    let dir = project();
    let inspector = ProjectInspector::open(dir.path()).unwrap();
    let explanation = inspector.explain("titles.normalize").unwrap();
    assert_eq!(explanation["operation"]["agent"], true);
    assert_eq!(explanation["operation"]["independent"], true);
    assert_eq!(explanation["source"]["file"], "native/src/lib.rs");
    assert_eq!(explanation["source"]["provenance"], "compiled_manifest");
    assert_eq!(explanation["index"]["status"], "not_requested");
    assert!(inspector.explain("titles.missing").is_err());
}

#[test]
fn semantic_graph_connects_consumers_to_their_matched_generated_declarations() {
    let dir = project();
    fs::write(dir.path().join("web/src/lib/.revenant/generation/facade.ts"), "export interface Application { operations: { titles: { normalize: { run(input: { text: string }): unknown } } } }\nexport interface Operations { titles: { normalize: (input: { text: string }) => Promise<string> } }\nexport declare function useApp(): Application;\nexport declare function useOperations(): Operations;\n").unwrap();
    fs::write(dir.path().join("web/src/lib/consumer.ts"), "import { useApp, useOperations } from './.revenant/generation/facade';\nconst modern = useOperations().titles.normalize({ text: 'hello' });\nconst legacy = useApp().operations.titles.normalize.run({ text: 'hello' });\n").unwrap();
    let query = ProjectInspector::open(dir.path())
        .unwrap()
        .query(json!({"operation":"titles.normalize","maxResults":20}))
        .unwrap();
    let graph = &query["graph"];
    let bindings = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "typescript_binding")
        .collect::<Vec<_>>();
    assert_eq!(bindings.len(), 2, "{query}");
    for (consumer_line, binding_line) in [(2, 2), (3, 1)] {
        let consumer = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| {
                node["kind"] == "frontend_consumer"
                    && node["location"]["file"] == "web/src/lib/consumer.ts"
                    && node["location"]["line"] == consumer_line
            })
            .unwrap();
        let edge = graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["relation"] == "used_by" && edge["to"] == consumer["id"])
            .unwrap();
        let binding = bindings
            .iter()
            .find(|node| node["id"] == edge["from"])
            .unwrap();
        assert_eq!(binding["location"]["line"], binding_line, "{query}");
        assert_eq!(edge["provenance"], "typescript_language_service");
    }
}

#[test]
fn unsupported_check_profile_is_rejected_before_starting_processes() {
    let dir = project();
    let inspector = ProjectInspector::open(dir.path()).unwrap();
    assert!(inspector.check("cargo check; arbitrary").is_err());
    assert!(!dir.path().join(".revenant/inspection/build.json").exists());
}

#[test]
fn queries_cannot_escape_the_project_or_use_unbounded_results() {
    let dir = project();
    let inspector = ProjectInspector::open(dir.path()).unwrap();
    assert!(
        inspector
            .query(json!({"file":"../outside.rs","line":1,"column":1}))
            .is_err()
    );
    assert!(
        inspector
            .query(json!({"file":"native/src/lib.rs","line":1,"column":1,"maxResults":100000}))
            .is_err()
    );
}

#[test]
fn cargo_diagnostics_keep_the_primary_span_and_ignore_non_diagnostics() {
    let dir = project();
    let output = format!(
        "{}\n{}\n",
        json!({"reason":"compiler-artifact","target":{"name":"ignored"}}),
        json!({"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"expected String, found u32","spans":[{"file_name":"src/lib.rs","line_start":1,"column_start":4,"line_end":1,"column_end":8,"is_primary":true}]}})
    );
    let diagnostics = revenant::inspection::parse_cargo_diagnostics(dir.path(), &output);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_deref(), Some("E0308"));
    assert_eq!(diagnostics[0].file.as_deref(), Some("native/src/lib.rs"));
    assert_eq!(diagnostics[0].column, Some(4));
}

#[test]
fn edits_during_a_build_keep_the_attempts_original_source_identity() {
    let dir = project();
    let initial = source_fingerprint(dir.path()).unwrap().digest;
    record_build_start(dir.path(), "native").unwrap();
    fs::write(
        dir.path().join("native/src/lib.rs"),
        "pub fn changed() {}\n",
    )
    .unwrap();
    record_build_finish(dir.path(), true, vec![]).unwrap();
    let snapshot = ProjectInspector::open(dir.path())
        .unwrap()
        .snapshot()
        .unwrap();
    assert_eq!(
        snapshot["buildAttempt"]["sourceFingerprint"]["digest"],
        initial
    );
    assert_eq!(snapshot["buildAttempt"]["sourceChangedDuringBuild"], true);
}

#[test]
fn inspect_cli_returns_a_single_machine_snapshot_without_a_running_app() {
    let dir = project();
    let fingerprint = source_fingerprint(dir.path()).unwrap();
    generation(dir.path(), &fingerprint.digest);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_revenant"))
        .args(["inspect", "--json", "--project"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["consistency"]["sourceToFacade"], "current");
    assert_eq!(snapshot["runtime"]["status"], "unavailable");
    assert_eq!(snapshot["runtime"]["error"]["code"], "instance_missing");
}

#[test]
fn development_mcp_advertises_seven_tools_and_enforces_their_schemas() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = project();
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_revenant"))
            .args(["mcp", "--project"])
            .arg(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let output = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let send = |input: &mut std::process::ChildStdin, value: serde_json::Value| {
        writeln!(input, "{}", value).unwrap();
        input.flush().unwrap();
    };
    let receive = |id: u64| -> serde_json::Value {
        loop {
            let line = rx
                .recv_timeout(Duration::from_secs(15))
                .expect("MCP response timeout");
            let response: serde_json::Value =
                serde_json::from_str(&line).expect("stdout is protocol JSON only");
            if response["id"] == id {
                return response;
            }
        }
    };
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"inspection-test","version":"1"}}}),
    );
    assert!(receive(1)["result"]["serverInfo"].is_object());
    send(
        &mut input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    let advertised = receive(2);
    let names = advertised["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 7);
    assert!(names.contains(&"code.query".to_owned()));
    assert!(names.contains(&"runtime.inspect".to_owned()));
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"operation.explain","arguments":{"operation":"titles.normalize","includeConsumers":false}}}),
    );
    let explanation = receive(3);
    assert_eq!(
        explanation["result"]["structuredContent"]["operation"]["id"],
        "titles.normalize"
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"checks.run","arguments":{"profile":"arbitrary-command"}}}),
    );
    let invalid = receive(4);
    assert_eq!(invalid["result"]["isError"], true);
    assert!(!dir.path().join(".revenant/inspection/build.json").exists());
    drop(input);
}

#[test]
fn unavailable_semantics_still_preserve_compiled_graph_edges() {
    let dir = project();
    let query = ProjectInspector::open(dir.path())
        .unwrap()
        .query(json!({"operation":"titles.normalize","maxResults":10}))
        .unwrap();
    let edges = query["graph"]["edges"].as_array().unwrap();
    assert!(edges.iter().any(|edge|edge["relation"]=="declares" && edge["provenance"]=="compiled_manifest"));
    assert!(
        edges.iter().any(
            |edge| edge["relation"] == "generates" && edge["provenance"] == "compiled_manifest"
        )
    );
}

#[test]
fn a_failed_rebuild_distinguishes_source_facade_and_the_live_executable() {
    let dir = project();
    let initial = source_fingerprint(dir.path()).unwrap().digest;
    generation(dir.path(), &initial);
    fs::write(
        dir.path().join("native/src/lib.rs"),
        "compile_error!(\"failure\");\n",
    )
    .unwrap();
    record_build_start(dir.path(), "native").unwrap();
    record_build_finish(dir.path(), false, vec![]).unwrap();
    let mut snapshot = ProjectInspector::open(dir.path())
        .unwrap()
        .snapshot()
        .unwrap();
    revenant::inspection::attach_runtime(
        &mut snapshot,
        json!({"instanceId":"old-instance","contractDigest":format!("sha256:{}","a".repeat(64)),"sourceFingerprint":initial,"tasks":[]}),
    );
    assert_eq!(snapshot["runtime"]["status"], "connected");
    assert_eq!(snapshot["consistency"]["sourceToFacade"], "stale");
    assert_eq!(snapshot["consistency"]["sourceToRuntime"], "stale");
    assert_eq!(
        snapshot["consistency"]["selectedFacadeToRuntime"],
        "current"
    );
    assert_eq!(snapshot["buildAttempt"]["status"], "failed");
}

#[test]
fn runtime_graph_edges_retain_identity_when_the_running_contract_differs() {
    let mut query = json!({"operation":{"id":"titles.normalize"},"contractDigest":"sha256:new","graph":{"nodes":[],"edges":[]}});
    let page = json!({"instance":{"instanceId":"old-instance","contractDigest":"sha256:old"},"events":[{"sequence":7,"operationId":"titles.normalize","action":"started","requestId":"frontend-request","origin":"frontend","taskId":"task-1"}],"nextSequence":7,"dropped":0});
    revenant::inspection::attach_runtime_traces(&mut query, page);
    assert_eq!(query["runtimeTraces"]["contractMatchesManifest"], false);
    assert_eq!(
        query["graph"]["nodes"][0]["event"]["requestId"],
        "frontend-request"
    );
    assert_eq!(query["graph"]["edges"][0]["relation"], "observed");
    assert_eq!(query["graph"]["edges"][0]["contractMatchesManifest"], false);
}

#[test]
fn fixed_checks_preserve_the_native_build_attempt_and_publish_separate_evidence() {
    let dir = project();
    record_build_start(dir.path(), "native:development").unwrap();
    record_build_finish(dir.path(), true, vec![]).unwrap();
    let before = fs::read(dir.path().join(".revenant/inspection/build.json")).unwrap();
    let report = ProjectInspector::open(dir.path())
        .unwrap()
        .check("sdk")
        .unwrap();
    assert_eq!(report["succeeded"], false);
    assert_eq!(report["checkAttempt"]["stage"], "check:sdk");
    assert_eq!(
        fs::read(dir.path().join(".revenant/inspection/build.json")).unwrap(),
        before
    );
    let snapshot = ProjectInspector::open(dir.path())
        .unwrap()
        .snapshot()
        .unwrap();
    assert_eq!(snapshot["buildAttempt"]["status"], "succeeded");
    assert_eq!(snapshot["checkAttempt"]["status"], "failed");
    assert!(
        snapshot["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["stage"] == "sdk")
    );
}

#[test]
fn concurrent_check_processes_are_rejected_before_overwriting_evidence() {
    let dir = project();
    fs::create_dir_all(dir.path().join(".revenant/inspection")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.path().join(".revenant/inspection/check.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let error = ProjectInspector::open(dir.path())
        .unwrap()
        .check("sdk")
        .unwrap_err();
    assert!(error.to_string().contains("already running"), "{error:#}");
    assert!(!dir.path().join(".revenant/inspection/check.json").exists());
}

#[test]
fn checks_respect_the_shared_native_preparation_lock() {
    let dir = project();
    fs::create_dir_all(dir.path().join(".revenant/inspection")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.path().join(".revenant/inspection/project.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let error = ProjectInspector::open(dir.path())
        .unwrap()
        .check("sdk")
        .unwrap_err();
    assert!(error.to_string().contains("already running"), "{error:#}");
    assert!(!dir.path().join(".revenant/inspection/check.json").exists());
}

#[cfg(windows)]
#[test]
fn fixed_rust_checks_reject_external_manifest_and_target_junctions() {
    // Real Windows junctions exercise canonical containment rather than a
    // lexical '..' check. Every destination belongs to this temporary fixture.
    for relative in ["native", ".revenant/desktop", ".revenant/target"] {
        let dir = project();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("Cargo.toml"), "[invalid manifest\n").unwrap();
        fs::write(dir.path().join("native/Cargo.toml"), "[invalid manifest\n").unwrap();
        let link = dir.path().join(relative);
        if link.exists() {
            fs::remove_dir_all(&link).unwrap();
        }
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        let script = outside.path().join("create-junction.ps1");
        fs::write(&script, "param([string]$Link, [string]$Destination)\n$ErrorActionPreference = 'Stop'\nNew-Item -ItemType Junction -Path $Link -Value $Destination | Out-Null\n").unwrap();
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .arg(&link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            dunce::canonicalize(&link).unwrap(),
            dunce::canonicalize(outside.path()).unwrap()
        );
        let report = ProjectInspector::open(dir.path()).unwrap().check("rust");
        // Remove the junction itself before either owned temporary tree is
        // cleaned up; no recursive deletion can follow it into the destination.
        fs::remove_dir(&link).unwrap();
        let error = report.unwrap_err();
        assert!(
            error.to_string().contains("escapes project root"),
            "{relative}: {error:#}"
        );
        assert!(
            !dir.path().join(".revenant/inspection/check.json").exists(),
            "{relative} must be rejected before recording a check or launching Cargo"
        );
        assert!(!outside.path().join("debug").exists());
    }
}

#[test]
fn a_real_failed_cargo_check_reports_current_errors_and_preserves_the_facade() {
    let dir = project();
    let initial = source_fingerprint(dir.path()).unwrap().digest;
    generation(dir.path(), &initial);
    record_build_start(dir.path(), "native:development").unwrap();
    record_build_finish(dir.path(), true, vec![]).unwrap();
    let pointer = fs::read(dir.path().join("web/src/lib/revenant.ts")).unwrap();
    fs::write(
        dir.path().join("native/src/lib.rs"),
        "pub fn normalize() -> String { 42 }\n",
    )
    .unwrap();
    let inspector = ProjectInspector::open(dir.path()).unwrap();
    let report = inspector.check("rust").unwrap();
    assert_eq!(report["succeeded"], false);
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "E0308"
                && diagnostic["stage"] == "rust"
                && diagnostic["file"] == "native/src/lib.rs"
                && diagnostic["line"] == 1),
        "{report}"
    );
    assert_eq!(
        fs::read(dir.path().join("web/src/lib/revenant.ts")).unwrap(),
        pointer
    );
    let snapshot = inspector.snapshot().unwrap();
    assert_eq!(snapshot["buildAttempt"]["status"], "succeeded");
    assert_eq!(snapshot["checkAttempt"]["status"], "failed");
    assert_eq!(snapshot["consistency"]["sourceToFacade"], "stale");
}

#[test]
fn runtime_diagnostics_retain_failure_codes_and_correlation_without_payloads() {
    let state = json!({"instanceId":"instance-1","tasks":[{"id":"task-1","operationId":"titles.normalize","requestId":"request-1","state":"failed","errorCode":"invalid_input","arguments":{"private":"secret"}}],"cleanupFailures":[{"code":"cleanup_failed","message":"Could not release native resource","details":{"private":"secret"}}],"frontendReports":[{"kind":"error","message":"Render failed","stack":"private stack"},{"kind":"ui","route":"/editor"}]});
    let diagnostics = revenant::inspection::runtime_diagnostics(&state);
    assert!(
        diagnostics
            .iter()
            .any(|entry| entry["code"] == "invalid_input"
                && entry["requestId"] == "request-1"
                && entry["taskId"] == "task-1")
    );
    assert!(
        diagnostics
            .iter()
            .any(|entry| entry["code"] == "cleanup_failed")
    );
    assert!(
        diagnostics
            .iter()
            .any(|entry| entry["code"] == "frontend_error"
                && entry["provenance"] == "frontend_report")
    );
    assert!(
        !serde_json::to_string(&diagnostics)
            .unwrap()
            .contains("secret")
    );
}

#[test]
fn operation_catalogues_report_the_full_total_and_truncated_page() {
    let dir = project();
    let path = dir
        .path()
        .join("web/src/lib/.revenant/generation/revenant.contract.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let operation = manifest["operations"][0].clone();
    manifest["operations"] = json!(
        (0..205)
            .map(|index| {
                let mut operation = operation.clone();
                operation["id"] = json!(format!("titles.operation{index}"));
                operation
            })
            .collect::<Vec<_>>()
    );
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let snapshot = ProjectInspector::open(dir.path())
        .unwrap()
        .snapshot()
        .unwrap();
    assert_eq!(snapshot["catalogue"]["total"], 205);
    assert_eq!(snapshot["catalogue"]["returned"], 200);
    assert_eq!(snapshot["catalogue"]["truncated"], true);
    assert_eq!(snapshot["graph"]["operationCount"], 205);
}

#[test]
fn source_traversal_stops_when_the_entry_budget_is_exhausted() {
    let dir = project();
    let files = dir.path().join("native/large");
    fs::create_dir_all(&files).unwrap();
    for index in 0..4200 {
        fs::write(files.join(format!("entry{index:04}.json")), "{}").unwrap();
    }
    let fingerprint = source_fingerprint(dir.path()).unwrap();
    assert!(!fingerprint.complete);
    assert!(fingerprint.files <= 4096);
    assert!(fingerprint.visited_entries <= 8192);
    assert!(fingerprint.omitted.len() <= 100);
}

#[tokio::test]
async fn read_only_developer_queries_reconnect_after_a_native_app_restart() {
    use revenant::inspection::backend::DevBackend;
    use revenant_agent::{AgentTool, Backend, BackendFuture, BridgeServer, InstanceInfo};
    use std::sync::Arc;
    struct InspectionRuntime(&'static str);
    impl Backend for InspectionRuntime {
        fn tools(&self) -> Vec<AgentTool> {
            vec![AgentTool {
                name: "runtime.inspect".into(),
                description: "Fixture native inspection".into(),
                input_schema: json!({"type":"object","additionalProperties":false}),
                output_schema: None,
                read_only: true,
            }]
        }
        fn call<'a>(
            &'a self,
            _: &'a str,
            _: serde_json::Value,
        ) -> BackendFuture<'a, serde_json::Value> {
            Box::pin(async move {
                Ok(json!({"instanceId":self.0,"contractDigest":"sha256:fixture","tasks":[]}))
            })
        }
    }
    let dir = project();
    let server1 = BridgeServer::start(
        InstanceInfo::new(
            "inspection-reconnect",
            "sha256:fixture",
            Some(dir.path().to_owned()),
            true,
        ),
        Arc::new(|_| Ok(Arc::new(InspectionRuntime("first")))),
    )
    .await
    .unwrap();
    let first_id = server1.info().instance_id.clone();
    let backend = DevBackend::new(ProjectInspector::open(dir.path()).unwrap());
    assert_eq!(
        backend.call("runtime.inspect", json!({})).await.unwrap()["instanceId"],
        "first"
    );
    server1.stop().await.unwrap();
    let server2 = BridgeServer::start(
        InstanceInfo::new(
            "inspection-reconnect",
            "sha256:fixture",
            Some(dir.path().to_owned()),
            true,
        ),
        Arc::new(|_| Ok(Arc::new(InspectionRuntime("second")))),
    )
    .await
    .unwrap();
    assert_eq!(
        backend.call("runtime.inspect", json!({})).await.unwrap()["instanceId"],
        "second"
    );
    assert_eq!(
        backend
            .call("runtime.inspect", json!({"instanceId":first_id}))
            .await
            .unwrap_err()
            .code,
        "instance_missing"
    );
    backend.close().await.unwrap();
    server2.stop().await.unwrap();
}
