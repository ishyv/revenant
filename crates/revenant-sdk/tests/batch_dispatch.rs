mod support;

use revenant_sdk::{Error, OperationRegistration, OperationRegistry, Progress, TaskState};
use serde_json::json;
use support::{block_on, fresh_task};

#[test]
fn prepared_items_keep_mixed_batch_counters_without_retaining_payloads() {
    let registry = OperationRegistry::new();
    registry
        .register(
            OperationRegistration::new::<u32, u32, _, _>(
                "tests.batch",
                "Returns an item error for the selected value.",
                |input, context| async move {
                    context.progress(Progress {
                        completed: 8_u64.into(),
                        total: Some(8_u64.into()),
                        message: Some("item bytes".into()),
                    })?;
                    if input == 2 {
                        Err(Error::new("item_rejected", "The second item was rejected"))
                    } else {
                        Ok(input * 10)
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let prepared = registry.prepare_operation("tests.batch").unwrap();
    let (_, _, context) = fresh_task("batch");
    let execution = context.begin().unwrap();

    // The native host owns iteration and persistence. This exercises compiled
    // execution and accounting, not desktop paging or result storage.
    for (index, input) in [1, 2, 3].into_iter().enumerate() {
        let child = context.batch_child(index as u32, 3).unwrap();
        prepared.prepare_input(&json!(input), &child).unwrap();
        let child_execution = child.begin().unwrap();
        let result = block_on(prepared.execute(json!(input), child.clone()));
        if input == 2 {
            assert_eq!(result.as_ref().unwrap_err().code, "item_rejected");
        } else {
            assert_eq!(result.as_ref().unwrap(), &json!(input * 10));
        }
        let success = result.is_ok();
        let child_snapshot = child_execution.finish(result).unwrap();
        assert_eq!(child_snapshot.progress.completed.get(), 8);
        assert_eq!(child_snapshot.summary.completed.get(), 0);
        context.record_batch_outcome(success).unwrap();
        context
            .progress(Progress {
                completed: (index as u64 + 1).into(),
                total: Some(3_u64.into()),
                message: None,
            })
            .unwrap();
    }
    let descriptor = json!({"resultId": "host-result", "count": "3"});
    let snapshot = execution.finish_batch(descriptor.clone(), false).unwrap();
    assert_eq!(snapshot.state, TaskState::Partial);
    assert_eq!(snapshot.summary.completed.get(), 3);
    assert_eq!(snapshot.summary.succeeded.get(), 2);
    assert_eq!(snapshot.summary.failed.get(), 1);
    assert_eq!(snapshot.progress.completed.get(), 3);
    assert_eq!(snapshot.progress.total.unwrap().get(), 3);
    assert!(snapshot.outcomes.is_empty());
    assert!(snapshot.error.is_none());
    assert_eq!(snapshot.result, Some(descriptor));
}
