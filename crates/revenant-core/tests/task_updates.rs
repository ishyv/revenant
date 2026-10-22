use revenant_core::{ResourceRegistry, TaskContext, TaskManager};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn task() -> (TaskManager, TaskContext) {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let manager = TaskManager::new(resources);
    let context = manager.create(&scope, "scan").unwrap();
    (manager, context)
}
fn schema() -> serde_json::Value {
    json!({"type":"object","properties":{"count":{"type":"integer","minimum":0,"maximum":4294967295_u64}},"required":["count"],"additionalProperties":false})
}

#[test]
fn latest_update_is_revisioned_retained_for_late_subscribers_and_separate_from_result() {
    let (_, context) = task();
    context.configure_updates(schema()).unwrap();
    let execution = context.begin().unwrap();
    context.publish(json!({"count":1})).unwrap();
    context.publish(json!({"count":2})).unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let capture = observed.clone();
    let subscription = context.subscribe(Arc::new(move |snapshot: revenant_core::TaskSnapshot| {
        capture.lock().unwrap().push(snapshot);
    }));
    let snapshot = context.snapshot();
    assert_eq!(snapshot.update.as_ref().unwrap().value, json!({"count":2}));
    assert_eq!(
        snapshot.update.as_ref().unwrap().revision,
        snapshot.revision
    );
    assert_eq!(observed.lock().unwrap().len(), 1);
    assert_eq!(snapshot.progress.completed.get(), 0);
    let terminal = execution.finish(Ok(json!("done"))).unwrap();
    assert!(terminal.revision > snapshot.revision);
    assert_eq!(terminal.result, Some(json!("done")));
    assert_eq!(terminal.update.unwrap().value, json!({"count":2}));
    assert_eq!(
        context.publish(json!({"count":3})).unwrap_err().code,
        "task_terminal"
    );
    drop(subscription);
}

#[test]
fn invalid_or_oversized_updates_do_not_replace_the_latest_value() {
    let (_, context) = task();
    context.configure_updates(schema()).unwrap();
    let _execution = context.begin().unwrap();
    context.publish(json!({"count":1})).unwrap();
    let previous = serde_json::to_value(context.snapshot()).unwrap();
    assert_eq!(
        context
            .publish(json!({"count":"invalid"}))
            .unwrap_err()
            .code,
        "invalid_task_update"
    );
    assert_eq!(
        context.publish("x".repeat(65_536)).unwrap_err().code,
        "task_update_limit"
    );
    assert_eq!(serde_json::to_value(context.snapshot()).unwrap(), previous);
}

#[test]
fn updates_require_declared_contract_running_executor_and_active_scope() {
    let (manager, context) = task();
    assert_eq!(
        context.publish(json!({})).unwrap_err().code,
        "task_not_running"
    );
    let _execution = context.begin().unwrap();
    assert_eq!(
        context.publish(json!({})).unwrap_err().code,
        "task_updates_undeclared"
    );
    context.configure_updates(schema()).unwrap();
    context.publish(json!({"count":1})).unwrap();
    assert_eq!(
        context.configure_updates(json!(true)).unwrap_err().code,
        "task_update_contract_conflict"
    );
    manager.dispose_scope(context.scope()).unwrap();
    assert_eq!(
        context.publish(json!({"count":2})).unwrap_err().code,
        "cancelled"
    );
    assert_eq!(context.snapshot().update.unwrap().value, json!({"count":1}));
}

#[test]
fn encoded_limit_counts_utf8_bytes_and_unsubscription_stops_lookup() {
    let (_, context) = task();
    context.configure_updates(json!({"type":"string"})).unwrap();
    let _execution = context.begin().unwrap();
    context.publish("x".repeat(65_534)).unwrap(); // Two JSON quotes fill exactly 64 KiB.
    assert_eq!(
        context.publish("é".repeat(32_768)).unwrap_err().code,
        "task_update_limit"
    );
    let observed = Arc::new(Mutex::new(0));
    let capture = observed.clone();
    let subscription = context.subscribe(Arc::new(move |_: revenant_core::TaskSnapshot| {
        *capture.lock().unwrap() += 1;
    }));
    assert_eq!(*observed.lock().unwrap(), 1);
    drop(subscription);
    context.publish("last").unwrap();
    assert_eq!(*observed.lock().unwrap(), 1);
    assert_eq!(context.snapshot().update.unwrap().value, json!("last"));
}
