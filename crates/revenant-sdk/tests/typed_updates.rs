mod support;
use revenant_sdk::{Contract, OperationRegistry, Result, TaskContext, operation};
use serde_json::json;
use support::{block_on, fresh_task};

#[derive(Contract)]
#[serde(deny_unknown_fields)]
struct ScanUpdate {
    count: u32,
}

#[operation(id = "scan.start", updates = ScanUpdate)]
async fn scan(context: TaskContext) -> Result<String> {
    context.publish(ScanUpdate { count: 1 })?;
    context.publish(ScanUpdate { count: 2 })?;
    Ok("complete".into())
}

#[test]
fn macro_exports_authoritative_update_type_and_dispatch_pins_it_before_user_work() {
    let registration = scan_operation().unwrap();
    let definition = registration.descriptor.updates.as_ref().unwrap();
    assert!(definition.typescript.contains("count: number"));
    assert_eq!(definition.schema, ScanUpdate::definition().unwrap().schema);
    let registry = OperationRegistry::new();
    registry.register(registration).unwrap();
    let (_, _, context) = fresh_task("scan");
    assert_eq!(
        block_on(registry.dispatch("scan.start", json!({}), context.clone())).unwrap(),
        json!("complete")
    );
    let snapshot = context.snapshot();
    assert_eq!(snapshot.update.unwrap().value, json!({"count":2}));
    assert_eq!(snapshot.result, Some(json!("complete")));
}

#[test]
fn prepared_operation_configures_updates_during_admission_and_retains_latest_on_cancel() {
    let registry = OperationRegistry::new();
    registry.register(scan_operation().unwrap()).unwrap();
    let prepared = registry.prepare_operation("scan.start").unwrap();
    let (_, tasks, context) = fresh_task("cancelled-scan");
    prepared.prepare_input(&json!({}), &context).unwrap();
    let execution = context.begin().unwrap();
    context.publish(ScanUpdate { count: 9 }).unwrap();
    tasks.cancel(context.scope(), "cancelled-scan").unwrap();
    assert_eq!(
        context.publish(ScanUpdate { count: 10 }).unwrap_err().code,
        "cancelled"
    );
    execution
        .finish(Err(revenant_sdk::Error::cancelled()))
        .unwrap();
    assert_eq!(context.snapshot().update.unwrap().value, json!({"count":9}));
}
