mod support;

use revenant_sdk::{OperationRegistry, Resource, Result, TaskContext, operation};
use serde_json::json;
use support::{block_on, fresh_task};

// Native state deliberately implements no Serde, TS, schema, or wire traits.
struct AnalysisSession {
    title: String,
}

#[operation]
fn start_analysis(context: TaskContext) -> Result<Resource<AnalysisSession>> {
    context.own(AnalysisSession {
        title: "Native session".into(),
    })
}

#[operation(read_only)]
fn analysis_title(session: Resource<AnalysisSession>, context: TaskContext) -> Result<String> {
    Ok(context.lease_resource(&session)?.title.clone())
}

#[test]
fn ordinary_macro_operations_create_and_consume_native_state_without_serializing_it() {
    let registry = OperationRegistry::new();
    registry
        .register(start_analysis_operation().unwrap())
        .unwrap();
    registry
        .register(analysis_title_operation().unwrap())
        .unwrap();
    let (resources, tasks, create) = fresh_task("create-native");
    let scope = create.scope().to_owned();
    let handle = block_on(registry.dispatch("startAnalysis", json!({}), create)).unwrap();
    assert!(handle.get("title").is_none());
    let inspect = tasks.create(&scope, "inspect-native").unwrap();
    let title =
        block_on(registry.dispatch("analysisTitle", json!({"session":handle}), inspect)).unwrap();
    assert_eq!(title, json!("Native session"));
    resources.dispose_scope(&scope).unwrap();
}
