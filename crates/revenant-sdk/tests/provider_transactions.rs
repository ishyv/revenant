mod support;

use revenant_sdk::{
    BoxFuture, FileMetadata, Handle, OperationRegistration, OperationRegistry, Provider,
    ProviderContext, ProviderDescriptor, ResourceCleanup, ResourcePort, ResourceRegistry, Result,
    TaskContext, TaskManager,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::Receiver,
    },
    thread,
    time::Duration,
};
use support::{Gate, block_on, gate};

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

fn file_metadata() -> FileMetadata {
    FileMetadata {
        name: "candidate.bin".into(),
        relative_path: "candidate.bin".into(),
        size: 1_u64.into(),
        mime: "application/octet-stream".into(),
        modified: None,
    }
}

pub struct TestProvider {
    id: String,
    dependencies: Vec<String>,
    operation_gate: Arc<Mutex<Option<Gate>>>,
    prepare_gate: Mutex<Option<Gate>>,
    fail_prepare: bool,
    register_resource: bool,
    cleanup_calls: Arc<AtomicUsize>,
    file_cleanup_calls: Arc<AtomicUsize>,
    cleanup_saw_file_disposal: Arc<AtomicBool>,
}

impl TestProvider {
    fn new(id: &str, dependencies: &[&str]) -> Self {
        Self {
            id: id.into(),
            dependencies: dependencies.iter().map(|value| (*value).into()).collect(),
            operation_gate: Arc::new(Mutex::new(None)),
            prepare_gate: Mutex::new(None),
            fail_prepare: false,
            register_resource: false,
            cleanup_calls: Arc::new(AtomicUsize::new(0)),
            file_cleanup_calls: Arc::new(AtomicUsize::new(0)),
            cleanup_saw_file_disposal: Arc::new(AtomicBool::new(false)),
        }
    }

    fn with_operation_gate(mut self, gate: Gate) -> Self {
        self.operation_gate = Arc::new(Mutex::new(Some(gate)));
        self
    }

    fn with_prepare_gate(mut self, gate: Gate) -> Self {
        self.prepare_gate = Mutex::new(Some(gate));
        self
    }

    fn failing_after_registering_resource(mut self) -> Self {
        self.fail_prepare = true;
        self.register_resource = true;
        self
    }

    fn operation_id(&self) -> String {
        format!("{}.run", self.id)
    }

    fn cleanup_count(&self) -> usize {
        self.cleanup_calls.load(Ordering::SeqCst)
    }
}

#[test]
fn final_registry_owner_drop_disposes_provider_resources_and_cleanup_once() {
    let registry = OperationRegistry::new();
    let resources = registry.resources().clone();
    let mut provider = TestProvider::new("owned", &[]);
    provider.register_resource = true;
    let provider = Arc::new(provider);
    block_on(registry.install(provider.clone(), json!({}))).unwrap();
    let other_owner = registry.clone();
    drop(registry);
    assert_eq!(provider.cleanup_count(), 0);
    drop(other_owner);
    assert_eq!(provider.cleanup_count(), 1);
    assert_eq!(provider.file_cleanup_calls.load(Ordering::SeqCst), 1);
    assert_eq!(resources.provider_resource_count("owned"), 0);
}

#[test]
fn replacing_with_the_active_instance_does_not_clean_or_mutate_it() {
    let registry = OperationRegistry::new();
    let provider = Arc::new(TestProvider::new("same", &[]));
    block_on(registry.install(provider.clone(), json!({}))).unwrap();
    let error = block_on(registry.replace("same", provider.clone(), json!({}))).unwrap_err();
    assert_eq!(error.code, "provider_identity");
    assert_eq!(provider.cleanup_count(), 0);
    assert_eq!(registry.inspect()[0].generation, 1);
    drop(registry);
    assert_eq!(provider.cleanup_count(), 1);
}

impl Provider for TestProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id.clone(),
            description: format!("Test provider {}", self.id),
            dependencies: self.dependencies.clone(),
            configuration_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    fn operations(&self) -> Result<Vec<OperationRegistration>> {
        let operation_id = self.operation_id();
        let gate = self.operation_gate.clone();
        let operation = OperationRegistration::new::<u32, u32, _, _>(
            operation_id,
            "Returns its input after the optional test gate.",
            move |input, _context| {
                let gate = gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                async move {
                    if let Some(gate) = gate {
                        let _ = gate.started.send(());
                        let _ = gate.release.recv();
                    }
                    Ok(input)
                }
            },
        )?;
        Ok(vec![operation])
    }

    fn prepare(&self, context: ProviderContext, _config: Value) -> BoxFuture<'_, Result<()>> {
        let gate = self
            .prepare_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let fail_prepare = self.fail_prepare;
        let register_resource = self.register_resource;
        let file_cleanup_calls = self.file_cleanup_calls.clone();

        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.started.send(());
                let _ = gate.release.recv();
            }
            if register_resource {
                let calls = file_cleanup_calls.clone();
                let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
                    calls.fetch_add(1, Ordering::SeqCst);
                });
                context.register_file(file_metadata(), Arc::new(TestReader), Some(cleanup))?;
            }
            if fail_prepare {
                return Err(revenant_sdk::Error::new(
                    "prepare_failed",
                    "The test provider deliberately failed preparation.",
                ));
            }
            Ok(())
        })
    }

    fn cleanup(&self) {
        self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
        self.cleanup_saw_file_disposal.store(
            self.file_cleanup_calls.load(Ordering::SeqCst) == 1,
            Ordering::SeqCst,
        );
    }
}

fn setup() -> (OperationRegistry, TaskManager) {
    let resources = ResourceRegistry::new();
    resources.create_scope("operation-scope", None).unwrap();
    (
        OperationRegistry::with_resources(resources.clone()),
        TaskManager::new(resources),
    )
}

fn install(registry: &OperationRegistry, provider: Arc<TestProvider>) {
    block_on(registry.install(provider, json!({}))).unwrap();
}

fn task(tasks: &TaskManager, id: &str) -> TaskContext {
    tasks.create("operation-scope", id).unwrap()
}

fn active_count(registry: &OperationRegistry, id: &str) -> u64 {
    registry
        .inspect()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap()
        .active_tasks
        .get()
}

fn wait_for_gate(started: Receiver<()>) {
    started
        .recv_timeout(Duration::from_secs(3))
        .expect("operation or preparation should reach its gate");
}

#[test]
fn failed_prepare_disposes_candidate_resources_before_provider_cleanup() {
    let (registry, _tasks) = setup();
    let candidate =
        Arc::new(TestProvider::new("candidate", &[]).failing_after_registering_resource());
    let file_cleanup_calls = candidate.file_cleanup_calls.clone();
    let cleanup_saw_file_disposal = candidate.cleanup_saw_file_disposal.clone();

    let error = block_on(registry.install(candidate.clone(), json!({}))).unwrap_err();

    assert_eq!(error.code, "prepare_failed");
    assert_eq!(candidate.cleanup_count(), 1);
    assert_eq!(file_cleanup_calls.load(Ordering::SeqCst), 1);
    assert!(cleanup_saw_file_disposal.load(Ordering::SeqCst));
    assert_eq!(registry.resources().provider_resource_count("candidate"), 0);
    assert!(registry.inspect().is_empty());
    assert!(registry.manifest().unwrap().operations.is_empty());
}

#[test]
fn active_dependent_tasks_make_replacing_their_provider_busy() {
    let (registry, tasks) = setup();
    install(&registry, Arc::new(TestProvider::new("a", &[])));
    let (operation_gate, started, release) = gate();
    let dependent = Arc::new(TestProvider::new("b", &["a"]).with_operation_gate(operation_gate));
    install(&registry, dependent);

    let context = task(&tasks, "dependent-call");
    let registry_for_worker = registry.clone();
    let worker =
        thread::spawn(move || block_on(registry_for_worker.dispatch("b.run", json!(7), context)));
    wait_for_gate(started);
    assert_eq!(active_count(&registry, "a"), 1);
    assert_eq!(active_count(&registry, "b"), 1);

    let candidate = Arc::new(TestProvider::new("a", &[]));
    let error = block_on(registry.replace("a", candidate.clone(), json!({}))).unwrap_err();
    assert_eq!(error.code, "provider_busy");
    assert_eq!(candidate.cleanup_count(), 1);
    assert_eq!(
        registry
            .inspect()
            .into_iter()
            .find(|snapshot| snapshot.id == "a")
            .unwrap()
            .generation,
        1
    );

    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap().unwrap(), json!(7));
    assert_eq!(active_count(&registry, "a"), 0);
    assert_eq!(active_count(&registry, "b"), 0);
}

#[test]
fn an_unrelated_active_provider_does_not_block_replacement() {
    let (registry, tasks) = setup();
    install(&registry, Arc::new(TestProvider::new("a", &[])));
    let (operation_gate, started, release) = gate();
    let unrelated =
        Arc::new(TestProvider::new("unrelated", &[]).with_operation_gate(operation_gate));
    install(&registry, unrelated);

    let context = task(&tasks, "unrelated-call");
    let registry_for_worker = registry.clone();
    let worker = thread::spawn(move || {
        block_on(registry_for_worker.dispatch("unrelated.run", json!(9), context))
    });
    wait_for_gate(started);
    assert_eq!(active_count(&registry, "unrelated"), 1);

    block_on(registry.replace("a", Arc::new(TestProvider::new("a", &[])), json!({}))).unwrap();
    assert_eq!(active_count(&registry, "unrelated"), 1);
    assert_eq!(
        registry
            .inspect()
            .into_iter()
            .find(|snapshot| snapshot.id == "a")
            .unwrap()
            .generation,
        2
    );

    release.send(()).unwrap();
    assert_eq!(worker.join().unwrap().unwrap(), json!(9));
    assert_eq!(active_count(&registry, "unrelated"), 0);
}

#[test]
fn preparation_reserves_dependents_and_new_dependencies_but_keeps_unrelated_dispatch_live() {
    let (registry, tasks) = setup();
    install(&registry, Arc::new(TestProvider::new("dependency", &[])));
    install(&registry, Arc::new(TestProvider::new("a", &[])));
    install(&registry, Arc::new(TestProvider::new("dependent", &["a"])));
    install(&registry, Arc::new(TestProvider::new("unrelated", &[])));

    let (prepare_gate, started, release) = gate();
    let candidate =
        Arc::new(TestProvider::new("a", &["dependency"]).with_prepare_gate(prepare_gate));
    let registry_for_worker = registry.clone();
    let replacement =
        thread::spawn(move || block_on(registry_for_worker.replace("a", candidate, json!({}))));
    wait_for_gate(started);

    let dependent_error = block_on(registry.dispatch(
        "dependent.run",
        json!(1),
        task(&tasks, "reserved-dependent"),
    ))
    .unwrap_err();
    assert_eq!(dependent_error.code, "provider_busy");

    let dependency_error = block_on(registry.dispatch(
        "dependency.run",
        json!(2),
        task(&tasks, "reserved-new-dependency"),
    ))
    .unwrap_err();
    assert_eq!(dependency_error.code, "provider_busy");

    let unrelated_result = block_on(registry.dispatch(
        "unrelated.run",
        json!(3),
        task(&tasks, "unreserved-provider"),
    ))
    .unwrap();
    assert_eq!(unrelated_result, json!(3));

    release.send(()).unwrap();
    replacement.join().unwrap().unwrap();
    assert_eq!(
        registry
            .inspect()
            .into_iter()
            .find(|snapshot| snapshot.id == "a")
            .unwrap()
            .generation,
        2
    );
}

#[test]
fn dependency_cycles_roll_back_the_candidate_and_keep_the_active_provider() {
    let (registry, tasks) = setup();
    install(&registry, Arc::new(TestProvider::new("a", &[])));
    install(&registry, Arc::new(TestProvider::new("b", &["a"])));
    let candidate = Arc::new(TestProvider::new("a", &["b"]));

    let error = block_on(registry.replace("a", candidate.clone(), json!({}))).unwrap_err();

    assert_eq!(error.code, "dependency_cycle");
    assert_eq!(candidate.cleanup_count(), 1);
    let active = registry
        .inspect()
        .into_iter()
        .find(|snapshot| snapshot.id == "a")
        .unwrap();
    assert_eq!(active.generation, 1);
    assert_eq!(active.dependencies, Vec::<String>::new());

    let result =
        block_on(registry.dispatch("a.run", json!(11), task(&tasks, "old-provider-still-works")))
            .unwrap();
    assert_eq!(result, json!(11));
}

#[test]
fn prepared_queued_operation_pins_transitive_dependencies_until_guard_drop() {
    let (registry, tasks) = setup();
    install(&registry, Arc::new(TestProvider::new("a", &[])));
    install(&registry, Arc::new(TestProvider::new("b", &["a"])));
    install(&registry, Arc::new(TestProvider::new("c", &["b"])));
    let prepared = registry.prepare_operation("c.run").unwrap();
    let context = task(&tasks, "queued-generation");
    prepared.prepare_input(&json!(13), &context).unwrap();
    assert_eq!(context.snapshot().state, revenant_sdk::TaskState::Queued);
    assert_eq!(prepared.descriptor().id, "c.run");
    for id in ["a", "b", "c"] {
        assert_eq!(active_count(&registry, id), 1);
    }

    let candidate = Arc::new(TestProvider::new("a", &[]));
    let error = block_on(registry.replace("a", candidate.clone(), json!({}))).unwrap_err();
    assert_eq!(error.code, "provider_busy");
    assert_eq!(candidate.cleanup_count(), 1);
    let execution = context.begin().unwrap();
    let output = block_on(prepared.execute(json!(13), context)).unwrap();
    assert_eq!(output, json!(13));
    execution.finish(Ok(output)).unwrap();
    // The host retains this guard across the batch, including gaps between
    // children; finishing one task must not silently unpin its generation.
    for id in ["a", "b", "c"] {
        assert_eq!(active_count(&registry, id), 1);
    }
    drop(prepared);
    for id in ["a", "b", "c"] {
        assert_eq!(active_count(&registry, id), 0);
    }
    block_on(registry.replace("a", Arc::new(TestProvider::new("a", &[])), json!({}))).unwrap();
    assert_eq!(
        registry
            .inspect()
            .into_iter()
            .find(|p| p.id == "a")
            .unwrap()
            .generation,
        2
    );
}
