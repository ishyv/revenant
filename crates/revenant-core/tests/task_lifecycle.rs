use revenant_core::{
    BoxFuture, FileMetadata, Handle, Progress, ResourceCleanup, ResourcePort, ResourceRegistry,
    Result, TaskManager, TaskState,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct TestReader;

impl ResourcePort for TestReader {
    fn read<'a>(
        &'a self,
        _handle: &'a Handle,
        _offset: u64,
        length: u32,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move { Ok(vec![0; length as usize]) })
    }
}

fn metadata() -> FileMetadata {
    FileMetadata {
        name: "sample.bin".into(),
        relative_path: "sample.bin".into(),
        size: 8_u64.into(),
        mime: "application/octet-stream".into(),
        modified: None,
    }
}

fn setup() -> (ResourceRegistry, TaskManager) {
    let resources = ResourceRegistry::new();
    resources.create_scope("scope", None).unwrap();
    let tasks = TaskManager::new(resources.clone());
    (resources, tasks)
}

fn terminal_json(context: &revenant_core::TaskContext) -> serde_json::Value {
    serde_json::to_value(context.snapshot()).unwrap()
}

#[test]
fn cancelling_a_queued_task_is_terminal_and_releases_its_lease() {
    let (resources, tasks) = setup();
    let cleaned = Arc::new(AtomicUsize::new(0));
    let cleaned_by_hook = cleaned.clone();
    let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
        cleaned_by_hook.fetch_add(1, Ordering::SeqCst);
    });
    let entry = resources
        .register_file_owned(
            "scope",
            metadata(),
            Arc::new(TestReader),
            Some("provider"),
            Some(cleanup),
        )
        .unwrap();
    let context = tasks.create("scope", "queued").unwrap();
    context.lease(&entry.handle).unwrap();

    resources.release("scope", &entry.handle).unwrap();
    assert_eq!(resources.provider_resource_count("provider"), 1);
    let cancelled = tasks.cancel("scope", "queued").unwrap();

    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert!(cancelled.cancel_requested);
    assert_eq!(cancelled.error.unwrap().code, "cancelled");
    assert_eq!(resources.provider_resource_count("provider"), 0);
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
}

#[test]
fn a_running_task_stays_cancelling_until_its_executor_acknowledges() {
    let (_resources, tasks) = setup();
    let context = tasks.create("scope", "running").unwrap();
    let execution = context.begin().unwrap();

    let requested = tasks.cancel("scope", "running").unwrap();
    assert_eq!(requested.state, TaskState::Cancelling);
    assert!(requested.cancel_requested);

    let acknowledgement = context.checkpoint().unwrap_err();
    assert_eq!(acknowledgement.code, "cancelled");
    assert_eq!(context.snapshot().state, TaskState::Cancelling);

    let terminal = execution.finish(Err(acknowledgement)).unwrap();
    assert_eq!(terminal.state, TaskState::Cancelled);
    assert!(terminal.cancel_requested);
    let before = terminal_json(&context);

    assert_eq!(
        context.request_cancel().unwrap().state,
        TaskState::Cancelled
    );
    assert_eq!(
        context
            .progress(Progress {
                completed: 1_u64.into(),
                total: Some(1_u64.into()),
                message: None,
            })
            .unwrap_err()
            .code,
        "task_terminal"
    );
    let begin_result = context.begin();
    assert!(matches!(begin_result, Err(error) if error.code == "task_terminal"));
    assert_eq!(before, terminal_json(&context));
}

#[test]
fn dropping_an_executor_marks_interruption_and_releases_leases() {
    let (resources, tasks) = setup();
    let cleaned = Arc::new(AtomicUsize::new(0));
    let cleaned_by_hook = cleaned.clone();
    let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
        cleaned_by_hook.fetch_add(1, Ordering::SeqCst);
    });
    let entry = resources
        .register_file_owned(
            "scope",
            metadata(),
            Arc::new(TestReader),
            Some("provider"),
            Some(cleanup),
        )
        .unwrap();
    let context = tasks.create("scope", "interrupted").unwrap();
    context.lease(&entry.handle).unwrap();
    let execution = context.begin().unwrap();
    resources.release("scope", &entry.handle).unwrap();

    drop(execution);

    let snapshot = context.snapshot();
    assert_eq!(snapshot.state, TaskState::Interrupted);
    assert_eq!(snapshot.error.unwrap().code, "interrupted");
    assert_eq!(resources.provider_resource_count("provider"), 0);
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(
        context
            .progress(Progress {
                completed: 1_u64.into(),
                total: None,
                message: None,
            })
            .unwrap_err()
            .code,
        "task_terminal"
    );
}

#[test]
fn child_observes_parent_cancellation_before_and_after_parent_finalization() {
    let (_resources, tasks) = setup();
    let parent = tasks.create("scope", "parent").unwrap();
    let execution = parent.begin().unwrap();
    let running_child = parent.batch_child(0, 2).unwrap();
    let queued_child = parent.batch_child(1, 2).unwrap();
    let child_execution = running_child.begin().unwrap();
    parent.record_batch_outcome(true).unwrap();
    parent.request_cancel().unwrap();
    assert_eq!(running_child.checkpoint().unwrap_err().code, "cancelled");
    assert_eq!(running_child.snapshot().state, TaskState::Cancelling);

    let descriptor = serde_json::json!({"resultId": "partial", "count": "1"});
    let snapshot = execution.finish_batch(descriptor.clone(), true).unwrap();
    assert_eq!(snapshot.state, TaskState::Partial);
    assert_eq!(snapshot.error.as_ref().unwrap().code, "cancelled");
    assert_eq!(snapshot.result, Some(descriptor));
    assert_eq!(snapshot.summary.completed.get(), 1);
    assert_eq!(snapshot.summary.succeeded.get(), 1);
    assert_eq!(snapshot.summary.failed.get(), 0);
    assert!(snapshot.outcomes.is_empty());

    // A finalized cancelling parent still communicates cancellation, rather
    // than leaking its own task_terminal error into an unfinished child.
    assert_eq!(queued_child.checkpoint().unwrap_err().code, "cancelled");
    assert_eq!(queued_child.snapshot().state, TaskState::Cancelled);
    assert_eq!(running_child.checkpoint().unwrap_err().code, "cancelled");
    assert_eq!(
        child_execution
            .finish(Err(revenant_core::Error::cancelled()))
            .unwrap()
            .state,
        TaskState::Cancelled
    );
}

#[test]
fn cancelled_batch_without_success_retains_descriptor_and_failure_counters() {
    let (_resources, tasks) = setup();
    let context = tasks.create("scope", "cancelled-batch").unwrap();
    let execution = context.begin().unwrap();
    context.record_batch_outcome(false).unwrap();
    context.request_cancel().unwrap();
    let descriptor = serde_json::json!({"resultId": "errors", "count": "1"});
    let snapshot = execution.finish_batch(descriptor.clone(), true).unwrap();
    assert_eq!(snapshot.state, TaskState::Cancelled);
    assert_eq!(snapshot.result, Some(descriptor));
    assert_eq!(snapshot.error.unwrap().code, "cancelled");
    assert_eq!(snapshot.summary.completed.get(), 1);
    assert_eq!(snapshot.summary.succeeded.get(), 0);
    assert_eq!(snapshot.summary.failed.get(), 1);
    assert!(snapshot.outcomes.is_empty());
}

#[test]
fn ordinary_success_does_not_acknowledge_a_cancel_request() {
    let (_resources, tasks) = setup();
    let context = tasks.create("scope", "ignored-cancel").unwrap();
    let execution = context.begin().unwrap();
    context.request_cancel().unwrap();
    let snapshot = execution.finish(Ok(serde_json::json!(42))).unwrap();
    assert_eq!(snapshot.state, TaskState::Succeeded);
    assert!(snapshot.cancel_requested);
    assert!(snapshot.error.is_none());
    assert_eq!(snapshot.result, Some(serde_json::json!(42)));
}

#[test]
fn unacknowledged_batch_cancellation_uses_recorded_success_and_failure_counters() {
    for (outcomes, expected) in [
        (vec![], TaskState::Succeeded),
        (vec![true, true], TaskState::Succeeded),
        (vec![false, false], TaskState::Failed),
        (vec![true, false], TaskState::Partial),
    ] {
        let (_resources, tasks) = setup();
        let context = tasks.create("scope", "summary-batch").unwrap();
        let execution = context.begin().unwrap();
        for &success in &outcomes {
            context.record_batch_outcome(success).unwrap();
        }
        context.request_cancel().unwrap();
        let descriptor =
            serde_json::json!({"resultId": "summary", "count": outcomes.len().to_string()});
        let snapshot = execution.finish_batch(descriptor.clone(), false).unwrap();
        assert_eq!(snapshot.state, expected);
        assert!(snapshot.cancel_requested);
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.result, Some(descriptor));
        assert_eq!(snapshot.summary.completed.get(), outcomes.len() as u64);
        assert_eq!(
            snapshot.summary.succeeded.get(),
            outcomes.iter().filter(|&&success| success).count() as u64
        );
        assert_eq!(
            snapshot.summary.failed.get(),
            outcomes.iter().filter(|&&success| !success).count() as u64
        );
        assert!(snapshot.outcomes.is_empty());
    }
}

#[test]
fn host_interrupt_after_grace_deadline_releases_leases_and_rejects_late_finish() {
    let (resources, tasks) = setup();
    let cleaned = Arc::new(AtomicUsize::new(0));
    let cleaned_by_hook = cleaned.clone();
    let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
        cleaned_by_hook.fetch_add(1, Ordering::SeqCst);
    });
    let entry = resources
        .register_file_owned(
            "scope",
            metadata(),
            Arc::new(TestReader),
            Some("provider"),
            Some(cleanup),
        )
        .unwrap();
    let context = tasks.create("scope", "late-worker").unwrap();
    context.lease(&entry.handle).unwrap();
    resources.release("scope", &entry.handle).unwrap();
    let execution = context.begin().unwrap();
    context.record_batch_outcome(true).unwrap();
    context.request_cancel().unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 0);

    // Deadline timing belongs to the host. Core receives the explicit interrupt
    // only once that host deadline has elapsed; no wall-clock sleep is needed.
    let snapshot = context.interrupt().unwrap();
    assert_eq!(snapshot.state, TaskState::Interrupted);
    assert!(snapshot.cancel_requested);
    assert_eq!(snapshot.error.unwrap().code, "interrupted");
    assert_eq!(snapshot.summary.succeeded.get(), 1);
    assert_eq!(snapshot.summary.completed.get(), 1);
    assert_eq!(resources.provider_resource_count("provider"), 0);
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    let before = terminal_json(&context);
    assert_eq!(
        execution
            .finish(Ok(serde_json::json!("late")))
            .unwrap_err()
            .code,
        "task_terminal"
    );
    context.interrupt().unwrap();
    assert_eq!(before, terminal_json(&context));
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
}
