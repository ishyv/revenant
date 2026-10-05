mod support;

use revenant_sdk::{
    BoxFuture, Contract, Error, FileMetadata, Handle, OperationRegistration, OperationRegistry,
    ResourceCleanup, ResourcePort, Result, TaskState, WireSafe,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::{block_on, fresh_task};

struct Reader;
impl ResourcePort for Reader {
    fn read<'a>(
        &'a self,
        _handle: &'a Handle,
        _offset: u64,
        length: u32,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move { Ok(vec![7; length as usize]) })
    }
}

#[derive(Contract)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadInput {
    file: Handle,
}

#[test]
fn prepared_queued_input_survives_owner_release_until_execution_finishes() {
    let (resources, _tasks, context) = fresh_task("queued-read");
    let cleanup_count = Arc::new(AtomicUsize::new(0));
    let cleaned = cleanup_count.clone();
    let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
        cleaned.fetch_add(1, Ordering::SeqCst);
    });
    let entry = resources
        .register_file_owned(
            context.scope(),
            FileMetadata {
                size: 4_u64.into(),
                ..FileMetadata::default()
            },
            Arc::new(Reader),
            Some("reader"),
            Some(cleanup),
        )
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let executed = calls.clone();
    let registry = OperationRegistry::with_resources(resources.clone());
    registry
        .register(
            OperationRegistration::new::<ReadInput, Vec<u8>, _, _>(
                "tests.read",
                "Read an admitted input file.",
                move |input, context| {
                    executed.fetch_add(1, Ordering::SeqCst);
                    async move { context.read(&input.file, 0, 4).await }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let prepared = registry.prepare_operation("tests.read").unwrap();
    let input = json!({"file": entry.handle});
    prepared.prepare_input(&input, &context).unwrap();
    assert_eq!(context.snapshot().state, TaskState::Queued);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    resources.release(context.scope(), &entry.handle).unwrap();
    assert!(matches!(resources.lease(context.scope(), &entry.handle),
        Err(error) if error.code == "stale_handle"));
    assert_eq!(resources.provider_resource_count("reader"), 1);

    // Revalidation during queued admission and execution must reuse the task's
    // original capability, rather than attempting a new owner-based admission.
    prepared.prepare_input(&input, &context).unwrap();
    let execution = context.begin().unwrap();
    let output = block_on(prepared.execute(input, context.clone())).unwrap();
    assert_eq!(output, json!([7, 7, 7, 7]));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        execution.finish(Ok(output)).unwrap().state,
        TaskState::Succeeded
    );
    assert_eq!(resources.provider_resource_count("reader"), 0);
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
}

#[test]
fn cancelling_prepared_queued_input_releases_capture_without_running_user_code() {
    let (resources, tasks, context) = fresh_task("queued-cancel");
    let cleanup_count = Arc::new(AtomicUsize::new(0));
    let cleaned = cleanup_count.clone();
    let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
        cleaned.fetch_add(1, Ordering::SeqCst);
    });
    let entry = resources
        .register_file_owned(
            context.scope(),
            FileMetadata {
                size: 1_u64.into(),
                ..FileMetadata::default()
            },
            Arc::new(Reader),
            Some("reader"),
            Some(cleanup),
        )
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let executed = calls.clone();
    let registry = OperationRegistry::with_resources(resources.clone());
    registry
        .register(
            OperationRegistration::new::<ReadInput, u32, _, _>(
                "tests.queued",
                "Does not execute during preparation.",
                move |_input, _context| {
                    executed.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(1) }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let prepared = registry.prepare_operation("tests.queued").unwrap();
    let input = json!({"file": entry.handle});
    prepared.prepare_input(&input, &context).unwrap();
    resources.release(context.scope(), &entry.handle).unwrap();
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        tasks.cancel(context.scope(), context.id()).unwrap().state,
        TaskState::Cancelled
    );
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
    assert_eq!(resources.provider_resource_count("reader"), 0);
    assert_eq!(
        prepared.prepare_input(&input, &context).unwrap_err().code,
        "task_terminal"
    );
    assert_eq!(
        block_on(prepared.execute(input, context)).unwrap_err().code,
        "task_terminal"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

// Structurally identical JSON does not make this display value a capability.
#[derive(Contract)]
#[serde(deny_unknown_fields)]
struct Lookalike {
    id: String,
    scope: String,
    generation: u32,
    kind: String,
}

#[derive(Contract)]
#[serde(rename_all = "snake_case")]
enum Nested {
    Direct(Handle),
    Named { file: Box<Handle> },
    Empty,
}

#[derive(Contract)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Capabilities {
    nested: Vec<Nested>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    optional: Option<Handle>,
    mapped: BTreeMap<String, Handle>,
    display: Lookalike,
}

fn handle(id: &str) -> Handle {
    Handle {
        id: id.into(),
        scope: "unowned".into(),
        generation: 1,
        kind: "file".into(),
    }
}

fn value() -> Capabilities {
    Capabilities {
        nested: vec![
            Nested::Direct(handle("first")),
            Nested::Named {
                file: Box::new(handle("second")),
            },
            Nested::Empty,
        ],
        optional: Some(handle("third")),
        mapped: BTreeMap::from([("key".into(), handle("fourth"))]),
        display: Lookalike {
            id: "display".into(),
            scope: "unowned".into(),
            generation: 1,
            kind: "file".into(),
        },
    }
}

#[test]
fn typed_input_and_output_visitors_traverse_nested_capabilities_only() {
    let registry = OperationRegistry::new();
    registry
        .register(
            OperationRegistration::new::<Capabilities, Capabilities, _, _>(
                "tests.capabilities",
                "Typed capability traversal.",
                |input, _context| async move { Ok(input) },
            )
            .unwrap(),
        )
        .unwrap();
    let prepared = registry.prepare_operation("tests.capabilities").unwrap();
    let mut wire = serde_json::to_value(value()).unwrap();
    let expected = vec![
        handle("first"),
        handle("second"),
        handle("third"),
        handle("fourth"),
    ];
    assert_eq!(prepared.input_handles(&wire).unwrap(), expected);
    assert_eq!(prepared.output_handles(&wire).unwrap(), expected);
    // Enumeration does not acquire these unowned handles. Optional omission and
    // enum unit variants must not fabricate extra resource identities.
    wire.as_object_mut().unwrap().remove("optional");
    let without_optional = vec![handle("first"), handle("second"), handle("fourth")];
    assert_eq!(prepared.input_handles(&wire).unwrap(), without_optional);
    assert_eq!(prepared.output_handles(&wire).unwrap(), without_optional);
    wire["nested"][0]["direct"]["generation"] = json!("bad");
    assert!(prepared.input_handles(&wire).is_err());
    assert!(prepared.output_handles(&wire).is_err());
}

#[test]
fn derived_visitor_stops_at_first_error() {
    let mut visited = Vec::new();
    let error = value()
        .visit_handles(&mut |handle| {
            visited.push(handle.id.clone());
            if handle.id == "second" {
                Err(Error::new("visitor_rejected", "Stop here"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert_eq!(error.code, "visitor_rejected");
    assert_eq!(visited, ["first", "second"]);
}
