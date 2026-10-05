mod support;

use revenant_sdk::{
    Application, Contract, OperationRegistry, Result, TaskContext, contract, operation, operations,
    validate_json,
};
use serde_json::json;
use support::{block_on, fresh_task};

#[derive(Contract, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Detail {
    count: u8,
}

#[derive(Contract, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Payload {
    Text(String),
    Record(Detail),
}

#[derive(Contract, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope<T> {
    payload: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[contract]
#[derive(Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ready,
    Busy,
}

/// Adds one to the input.
///
/// Keeps **Rustdoc** in generated operation metadata.
#[operation(id = "tests.withoutContext")]
async fn without_context(input: u32) -> Result<u32> {
    Ok(input + 1)
}

/// Adds two while checking cancellation.
#[operation(id = "tests.withContext")]
async fn with_context(input: u32, context: TaskContext) -> Result<u32> {
    context.checkpoint()?;
    Ok(input + 2)
}

mod authored {
    use super::*;

    /// Original docs remain on the ordinary Rust function.
    #[operation(
        id = "tests.overriddenDocs",
        description = "Explicit operation description."
    )]
    pub async fn overridden(input: u32) -> Result<u32> {
        Ok(input * 2)
    }
}

#[test]
fn operations_composition_preserves_rust_functions_docs_and_exact_source_metadata() {
    let manifest = Application::new()
        .operations(operations![
            without_context,
            with_context,
            authored::overridden,
        ])
        .manifest()
        .unwrap();
    let expected = [
        (
            "tests.withoutContext",
            "Adds one to the input.\n\nKeeps **Rustdoc** in generated operation metadata.",
            "#[operation(id = \"tests.withoutContext\")]",
            module_path!().to_owned(),
        ),
        (
            "tests.withContext",
            "Adds two while checking cancellation.",
            "#[operation(id = \"tests.withContext\")]",
            module_path!().to_owned(),
        ),
        (
            "tests.overriddenDocs",
            "Explicit operation description.",
            "    #[operation(",
            format!("{}::authored", module_path!()),
        ),
    ];
    // Read only the compiled fixture: assertions track source moves without
    // weakening the one-based attribute-line contract to merely line > 0.
    let fixture = include_str!("compiled_contracts.rs");
    for (id, docs, attribute, module) in expected {
        let descriptor = manifest.operations.iter().find(|op| op.id == id).unwrap();
        assert_eq!(descriptor.description, docs);
        let source = descriptor.source.as_ref().unwrap();
        assert_eq!(source.file, file!());
        assert_eq!(source.module, module);
        let line = fixture.lines().position(|line| line == attribute).unwrap() + 1;
        assert_eq!(source.line as usize, line);
    }
    assert_eq!(block_on(without_context(40)).unwrap(), 41);
    let (_, _, context) = fresh_task("ordinary-rust");
    assert_eq!(block_on(with_context(40, context)).unwrap(), 42);
    assert_eq!(block_on(authored::overridden(21)).unwrap(), 42);
}

#[test]
fn compiled_struct_enum_and_generic_contracts_export_standalone_typescript_and_bounds() {
    let envelope = Envelope {
        payload: Payload::Record(Detail { count: u8::MAX }),
        note: None,
    };
    let definition = Envelope::<Payload>::definition().unwrap();
    let root_alias = format!("export type ContractValue = {};", definition.name);
    assert!(definition.typescript.starts_with(&root_alias));
    assert!(definition.typescript.contains("export type Payload"));
    assert!(definition.typescript.contains("export type Detail"));
    assert!(definition.typescript.contains("note?"));

    let value = serde_json::to_value(&envelope).unwrap();
    assert!(value.get("note").is_none());
    validate_json(&definition.schema, &value).unwrap();
    assert_eq!(
        serde_json::from_value::<Envelope<Payload>>(value.clone()).unwrap(),
        envelope
    );

    let mut above_u8 = value.clone();
    above_u8["payload"]["record"]["count"] = json!(256);
    assert_eq!(
        validate_json(&definition.schema, &above_u8)
            .unwrap_err()
            .code,
        "contract_validation"
    );

    let mut explicit_null = value;
    explicit_null["note"] = serde_json::Value::Null;
    assert_eq!(
        validate_json(&definition.schema, &explicit_null)
            .unwrap_err()
            .code,
        "contract_validation"
    );

    assert_eq!(Status::definition().unwrap().name, "Status");
}

#[test]
fn operation_macro_accepts_both_context_signatures_and_dispatches_typed_values() {
    let registry = OperationRegistry::new();
    let without = without_context_operation().unwrap();
    let with = with_context_operation().unwrap();
    assert_eq!(without.descriptor.id, "tests.withoutContext");
    assert_eq!(with.descriptor.id, "tests.withContext");
    registry.register(without).unwrap();
    registry.register(with).unwrap();

    let (_, _, first_context) = fresh_task("no-context");
    let result =
        block_on(registry.dispatch("tests.withoutContext", json!(40), first_context)).unwrap();
    assert_eq!(result, json!(41));

    let (_, _, second_context) = fresh_task("with-context");
    let result =
        block_on(registry.dispatch("tests.withContext", json!(40), second_context)).unwrap();
    assert_eq!(result, json!(42));
}
