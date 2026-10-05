mod common;
use revenant::compiled::BuildStage;
use serde_json::{Value, json};
use std::{fs, path::Path};
use tempfile::TempDir;

fn manifest(description: &str) -> Value {
    json!({"version":3,"protocol":2,"digest":format!("sha256:{}", "a".repeat(64)),"operations":[{
        "id":"records.normalize","description":description,
        "source":{"file":"src/records.rs","line":17,"module":"custom_native::records"},
        "input":{"schema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]},"typescript":"/** A title to normalize. */\nexport type ContractValue = Record;\nexport type Record = { /** Human-readable title. */ title: string };"},
        "output":{"schema":{"type":"string"},"typescript":"export type ContractValue = string;"}
    }]})
}
fn export(root: &Path, value: &Value) {
    fs::write(
        root.join("fixture-contract.json"),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}
fn publish(root: &Path) -> String {
    let stage = BuildStage::new(root).unwrap();
    stage.publish(common::fixture(), false).unwrap()
}
fn no_unpublished_stages(root: &Path) {
    for item in fs::read_dir(root.join("web/src/lib/.revenant")).unwrap() {
        assert!(
            !item
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        );
    }
}

#[test]
fn immutable_generation_publishes_documented_types_and_rust_source() {
    let temp = TempDir::new().unwrap();
    export(
        temp.path(),
        &manifest("Trim a title. Keep {{literal}} text and close */ safely."),
    );
    let first = publish(temp.path());
    let first_path = temp.path().join("web/src/lib/.revenant").join(&first);
    let before = fs::read(first_path.join("facade.ts")).unwrap();
    let facade = String::from_utf8(before.clone()).unwrap();
    assert!(facade.contains("export namespace RecordsNormalize"));
    // TypeScript treats a same-line `{ /**` as a trailing comment rather than
    // documentation for the following operation property.
    assert!(!facade.contains("\"records\": { /**"));
    assert!(facade.contains("Human-readable title."));
    assert!(facade.contains("Keep {{literal}} text"));
    assert!(facade.contains("close * / safely."));
    assert!(facade.contains("../../../../../native/src/records.rs#L17"));
    assert!(facade.contains("runtime.bindOperation<RecordsNormalize.Input.ContractValue, RecordsNormalize.Output.ContractValue>"));
    let contract: Value =
        serde_json::from_slice(&fs::read(first_path.join("revenant.contract.json")).unwrap())
            .unwrap();
    assert_eq!(
        contract,
        manifest("Trim a title. Keep {{literal}} text and close */ safely.")
    );
    assert_eq!(
        first,
        publish(temp.path()),
        "same exported API must reuse the immutable generation"
    );
    export(temp.path(), &manifest("A changed operation document."));
    let second = publish(temp.path());
    assert_ne!(
        first, second,
        "documentation changes are part of generation identity"
    );
    assert_eq!(fs::read(first_path.join("facade.ts")).unwrap(), before);
    assert!(
        fs::read_to_string(temp.path().join("web/src/lib/revenant.ts"))
            .unwrap()
            .contains(&second)
    );
    no_unpublished_stages(temp.path());
}

#[test]
fn failed_export_or_invalid_manifest_preserves_last_success_and_cleans_staging() {
    let temp = TempDir::new().unwrap();
    export(temp.path(), &manifest("last success"));
    let generation = publish(temp.path());
    let entry = temp.path().join("web/src/lib/revenant.ts");
    let before = fs::read(&entry).unwrap();
    fs::write(temp.path().join("fixture-export-fails"), "fail").unwrap();
    {
        let stage = BuildStage::new(temp.path()).unwrap();
        assert!(stage.publish(common::fixture(), false).is_err());
    }
    fs::remove_file(temp.path().join("fixture-export-fails")).unwrap();
    let mut cases = vec![];
    let mut wrong_protocol = manifest("invalid");
    wrong_protocol["protocol"] = json!(1);
    cases.push(wrong_protocol);
    let mut wrong_version = manifest("invalid");
    wrong_version["version"] = json!(2);
    cases.push(wrong_version);
    let mut invalid_type = manifest("invalid");
    invalid_type["operations"][0]["input"]["typescript"] = json!("");
    cases.push(invalid_type);
    let mut invalid_schema = manifest("invalid");
    invalid_schema["operations"][0]["output"]["schema"] = json!(42);
    cases.push(invalid_schema);
    let mut duplicate = manifest("invalid");
    let op = duplicate["operations"][0].clone();
    duplicate["operations"].as_array_mut().unwrap().push(op);
    cases.push(duplicate);
    let mut dangerous_group = manifest("invalid");
    dangerous_group["operations"][0]["id"] = json!("__proto__.polluted");
    cases.push(dangerous_group);
    for invalid in cases {
        export(temp.path(), &invalid);
        {
            let stage = BuildStage::new(temp.path()).unwrap();
            assert!(stage.publish(common::fixture(), false).is_err());
        }
        assert_eq!(fs::read(&entry).unwrap(), before);
        no_unpublished_stages(temp.path());
    }
    fs::write(temp.path().join("fixture-contract.json"), "not JSON").unwrap();
    {
        let stage = BuildStage::new(temp.path()).unwrap();
        assert!(stage.publish(common::fixture(), false).is_err());
    }
    assert_eq!(fs::read(entry).unwrap(), before);
    assert!(
        temp.path()
            .join("web/src/lib/.revenant")
            .join(generation)
            .join("facade.ts")
            .is_file()
    );
    no_unpublished_stages(temp.path());
}

#[test]
fn builtins_extend_capabilities_and_scopes_keep_typed_aliases() {
    let temp = TempDir::new().unwrap();
    let mut value = manifest("native checksum");
    value["operations"][0]["id"] = json!("files.checksum");
    value["operations"][0]["source"] =
        json!({"file":"src/builtins.rs","line":101,"module":"revenant_sdk::builtins"});
    export(temp.path(), &value);
    let generation = publish(temp.path());
    let facade = fs::read_to_string(
        temp.path()
            .join("web/src/lib/.revenant")
            .join(generation)
            .join("facade.ts"),
    )
    .unwrap();
    assert!(facade.contains("files: Object.assign(runtime.files"));
    assert!(facade.contains("createScope: (): Application => bindApp(createScope())"));
    assert!(facade.contains("type RuntimeApp = App<FilesChecksum.Output.ContractValue, unknown>"));
    assert!(facade.contains("useRuntimeApp<FilesChecksum.Output.ContractValue, unknown>(app)"));
    assert!(facade.contains(".revenant/sdk/crates/revenant-sdk/src/builtins.rs#L101"));
    assert!(facade.contains(
        "Operation<FilesChecksum.Input.ContractValue, FilesChecksum.Output.ContractValue>"
    ));
}
