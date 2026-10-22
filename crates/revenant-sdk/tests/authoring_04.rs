mod support;

use revenant_sdk::{
    Application, BoxFuture, FileMetadata, Handle, OperationEffect, OperationRegistry, ResourcePort,
    Result, TaskContext, operation, operations,
};
use serde_json::json;
use support::{block_on, fresh_task};

revenant_sdk::app!();

mod configured {
    revenant_sdk::app!(revenant_sdk::Application::new());
}

/// Trim whitespace and repeat the title.
#[operation(id = "titles.normalize", read_only, independent, agent)]
fn normalize(text: String, repetitions: u8) -> String {
    text.trim().repeat(repetitions.into())
}

#[operation]
fn no_args() -> u32 {
    42
}

#[operation(effect = "write", agent)]
async fn with_context(value: u32, context: TaskContext) -> Result<u32> {
    context.checkpoint()?;
    Ok(value + 1)
}

#[operation]
fn optional_name(display_name: Option<String>) -> String {
    display_name.unwrap_or_else(|| "Untitled".into())
}

#[operation]
fn only_context(context: TaskContext) -> Result<u32> {
    context.checkpoint()?;
    Ok(42)
}

#[operation(id = "legacy.double", legacy_input)]
fn legacy_double(value: u32) -> u32 {
    value * 2
}

#[operation]
#[cfg_attr(all(), inline)]
fn conditional_function_attribute() -> u32 {
    7
}

#[operation(read_only, independent)]
fn echo_handle(file: Handle) -> Handle {
    file
}

#[operation]
fn unicode_argument(étiquette: String) -> String {
    étiquette
}

struct EmptyReader;
impl ResourcePort for EmptyReader {
    fn read<'a>(
        &'a self,
        _handle: &'a Handle,
        _offset: u64,
        _length: u32,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

mod nested {
    use super::*;

    #[operation]
    pub async fn sum_values(left: u32, right: u32) -> u32 {
        left + right
    }

    #[operation(id = "tests.disabled")]
    #[cfg(any())]
    fn disabled() -> u32 {
        0
    }
}

#[test]
fn named_owned_arguments_dispatch_without_a_handwritten_input_contract() {
    let registry = OperationRegistry::new();
    registry.register(normalize_operation().unwrap()).unwrap();
    let (_, _, context) = fresh_task("normalize");
    assert_eq!(
        block_on(registry.dispatch(
            "titles.normalize",
            json!({ "text": " hello ", "repetitions": 2 }),
            context,
        ))
        .unwrap(),
        json!("hellohello")
    );
    let (_, _, context) = fresh_task("invalid");
    assert_eq!(
        block_on(registry.dispatch(
            "titles.normalize",
            json!({ "text": "hello", "repetitions": 256 }),
            context,
        ))
        .unwrap_err()
        .code,
        "contract_validation"
    );
}

#[test]
fn discovery_keeps_calling_crate_operations_and_excludes_dependency_operations() {
    let manifest = app().manifest().unwrap();
    let ids: Vec<_> = manifest
        .operations
        .iter()
        .map(|op| op.id.as_str())
        .collect();
    assert!(ids.contains(&"titles.normalize"));
    assert!(ids.contains(&"noArgs"));
    assert!(ids.contains(&"withContext"));
    assert!(ids.contains(&"nested.sumValues"));
    assert!(!ids.contains(&"tests.disabled"));
    // SDK built-ins occur once because dependency registrations are not discovered.
    assert_eq!(ids.iter().filter(|id| **id == "files.checksum").count(), 1);
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(manifest.version, 6);
    assert_eq!(manifest.protocol, 5);
    assert_eq!(
        manifest.digest,
        configured::app().manifest().unwrap().digest
    );
    let title = manifest
        .operations
        .iter()
        .find(|op| op.id == "titles.normalize")
        .unwrap();
    assert_eq!(title.arguments, ["text", "repetitions"]);
    assert_eq!(title.effect, OperationEffect::ReadOnly);
    assert!(title.agent && title.independent);
    assert_eq!(serde_json::to_value(title).unwrap()["effect"], "readOnly");
    let context = manifest
        .operations
        .iter()
        .find(|op| op.id == "withContext")
        .unwrap();
    assert_eq!(context.arguments, ["value"]);
    assert_eq!(context.effect, OperationEffect::Write);
    assert!(!context.independent);
}

#[test]
fn no_argument_plain_result_and_async_context_dispatch_preserve_rust_calls() {
    let registry = OperationRegistry::new();
    for factory in operations![no_args, with_context, nested::sum_values] {
        registry.register(factory().unwrap()).unwrap();
    }
    for (id, input, expected) in [
        ("noArgs", json!({}), json!(42)),
        ("withContext", json!({"value": 41}), json!(42)),
        (
            "nested.sumValues",
            json!({"left": 20, "right": 22}),
            json!(42),
        ),
    ] {
        let (_, _, context) = fresh_task(id);
        assert_eq!(
            block_on(registry.dispatch(id, input, context)).unwrap(),
            expected
        );
    }
    assert_eq!(normalize(" hi ".into(), 1), "hi");
    assert_eq!(no_args(), 42);
    assert_eq!(block_on(nested::sum_values(20, 22)), 42);
}

#[test]
fn explicit_composition_still_rejects_duplicate_ids() {
    assert_eq!(
        Application::new()
            .operations(operations![normalize, normalize])
            .manifest()
            .unwrap_err()
            .code,
        "operation_conflict"
    );
}

#[test]
fn optional_owned_arguments_support_omission_and_camel_case_wire_names() {
    let registry = OperationRegistry::new();
    let registration = optional_name_operation().unwrap();
    assert_eq!(registration.descriptor.arguments, ["displayName"]);
    assert!(
        registration
            .descriptor
            .input
            .typescript
            .contains("displayName?")
    );
    registry.register(registration).unwrap();
    for (input, expected) in [
        (json!({}), json!("Untitled")),
        (json!({"displayName": "Book"}), json!("Book")),
    ] {
        let (_, _, context) = fresh_task("optional");
        assert_eq!(
            block_on(registry.dispatch("optionalName", input, context)).unwrap(),
            expected
        );
    }
    let (_, _, context) = fresh_task("optional-null");
    assert_eq!(
        block_on(registry.dispatch("optionalName", json!({"displayName": null}), context))
            .unwrap_err()
            .code,
        "contract_validation"
    );
}

#[test]
fn context_only_and_explicit_legacy_inputs_dispatch() {
    let registry = OperationRegistry::new();
    registry
        .register(only_context_operation().unwrap())
        .unwrap();
    let legacy = legacy_double_operation().unwrap();
    assert!(legacy.descriptor.arguments.is_empty());
    registry.register(legacy).unwrap();
    for (id, input) in [("onlyContext", json!({})), ("legacy.double", json!(21))] {
        let (_, _, context) = fresh_task(id);
        assert_eq!(
            block_on(registry.dispatch(id, input, context)).unwrap(),
            json!(42)
        );
    }
}

#[test]
fn generated_named_input_retains_handle_admission_until_execution_finishes() {
    let (resources, _tasks, context) = fresh_task("echo-handle");
    let entry = resources
        .register_file(
            context.scope(),
            FileMetadata::default(),
            std::sync::Arc::new(EmptyReader),
        )
        .unwrap();
    let registry = OperationRegistry::with_resources(resources.clone());
    registry.register(echo_handle_operation().unwrap()).unwrap();
    let prepared = registry.prepare_operation("echoHandle").unwrap();
    let input = json!({"file": entry.handle});
    assert_eq!(
        prepared.input_handles(&input).unwrap().as_slice(),
        std::slice::from_ref(&entry.handle)
    );
    prepared.prepare_input(&input, &context).unwrap();
    resources.release(context.scope(), &entry.handle).unwrap();
    let execution = context.begin().unwrap();
    let output = block_on(prepared.execute(input, context)).unwrap();
    assert_eq!(output, json!(entry.handle));
    assert_eq!(prepared.output_handles(&output).unwrap(), [entry.handle]);
    execution.finish(Ok(output)).unwrap();
}

#[test]
fn generated_argument_names_match_metadata_for_unicode_identifiers() {
    let operation = unicode_argument_operation().unwrap();
    assert_eq!(operation.descriptor.arguments, ["étiquette"]);
    assert_eq!(
        operation.descriptor.input.schema["properties"]["étiquette"]["type"],
        "string"
    );
    let registry = OperationRegistry::new();
    registry.register(operation).unwrap();
    let (_, _, context) = fresh_task("unicode");
    assert_eq!(
        block_on(registry.dispatch("unicodeArgument", json!({"étiquette": "titre"}), context))
            .unwrap(),
        json!("titre")
    );
}
