use revenant_core::{Error, ResourceRegistry, TaskManager};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Session(Arc<AtomicUsize>);
impl Drop for Session {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn setup() -> (ResourceRegistry, TaskManager, String) {
    let resources = ResourceRegistry::new();
    let ui = resources.create_scope(None).unwrap();
    let tasks = TaskManager::new(resources.clone());
    (resources, tasks, ui)
}

#[test]
fn repeated_failed_creation_releases_new_owners_in_a_long_lived_scope() {
    let (resources, tasks, ui) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let prior = resources
        .register_native(&ui, Session(drops.clone()))
        .unwrap();
    for attempt in 0..20 {
        let context = tasks.create(&ui, &format!("failed-{attempt}")).unwrap();
        let execution = context.begin().unwrap();
        context.lease_resource(&prior).unwrap();
        let created = context.own(Session(drops.clone())).unwrap();
        execution
            .finish(Err(Error::new("scan_failed", "failed after allocation")))
            .unwrap();
        assert_eq!(
            drops.load(Ordering::SeqCst),
            attempt + 1,
            "failed session retained until scope close"
        );
        assert_eq!(
            resources.lease_native(&ui, &created).unwrap_err().code,
            "stale_handle"
        );
        assert!(
            resources.lease_native(&ui, &prior).is_ok(),
            "borrowed owner must survive another task's failure"
        );
    }
    resources.release(&ui, prior.handle()).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 21);
}

#[test]
fn failed_creation_revokes_owner_but_does_not_destroy_an_independent_active_lease() {
    let (resources, tasks, ui) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let creator = tasks.create(&ui, "creator").unwrap();
    let execution = creator.begin().unwrap();
    let created = creator.own(Session(drops.clone())).unwrap();
    let consumer = tasks.create(&ui, "consumer").unwrap();
    let consuming = consumer.begin().unwrap();
    let independent = consumer.lease_resource(&created).unwrap();
    execution
        .finish(Err(Error::new("create_failed", "No returned resource")))
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(
        resources.lease_native(&ui, &created).unwrap_err().code,
        "stale_handle"
    );
    assert!(consumer.lease_resource(&created).is_ok());
    consuming.finish(Ok(json!(null))).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(independent);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn successful_returned_resource_keeps_its_scoped_owner() {
    let (resources, tasks, ui) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let context = tasks.create(&ui, "success").unwrap();
    let execution = context.begin().unwrap();
    let created = context.own(Session(drops.clone())).unwrap();
    execution
        .finish(Ok(serde_json::to_value(&created).unwrap()))
        .unwrap();
    assert!(resources.lease_native(&ui, &created).is_ok());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    tasks.dispose_scope(&ui).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn cancelled_interrupted_and_unstarted_created_sessions_release_their_owners() {
    let (resources, tasks, ui) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    for (index, mode) in ["cancelled", "interrupted", "dropped", "queued"]
        .iter()
        .enumerate()
    {
        let context = tasks.create(&ui, mode).unwrap();
        let execution = if *mode == "queued" {
            None
        } else {
            Some(context.begin().unwrap())
        };
        let created = context.own(Session(drops.clone())).unwrap();
        match *mode {
            "cancelled" => {
                context.request_cancel().unwrap();
                execution.unwrap().finish(Err(Error::cancelled())).unwrap();
            }
            "interrupted" => {
                context.interrupt().unwrap();
                drop(execution);
            }
            "dropped" => drop(execution),
            "queued" => {
                context.request_cancel().unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            resources.lease_native(&ui, &created).unwrap_err().code,
            "stale_handle"
        );
        assert_eq!(drops.load(Ordering::SeqCst), index + 1);
    }
}

#[test]
fn successful_partial_item_output_preserves_new_resource_owner_after_parent_failure() {
    let (resources, tasks, ui) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let context = tasks.create(&ui, "partial").unwrap();
    let execution = context.begin().unwrap();
    let created = context.own(Session(drops.clone())).unwrap();
    context
        .record_outcome(revenant_core::Outcome {
            index: 0,
            output: Some(serde_json::to_value(&created).unwrap()),
            error: None,
        })
        .unwrap();
    execution
        .finish(Err(Error::new(
            "next_item_failed",
            "Previously returned resource must stay usable",
        )))
        .unwrap();
    assert!(resources.lease_native(&ui, &created).is_ok());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    tasks.dispose_scope(&ui).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_creation_cleanup_can_reenter_task_and_resource_registry_without_locking() {
    struct Reentrant {
        scope: String,
        context: revenant_core::TaskContext,
        resources: ResourceRegistry,
        calls: Arc<AtomicUsize>,
    }
    impl Drop for Reentrant {
        fn drop(&mut self) {
            assert!(self.context.snapshot().state.is_terminal());
            let temporary = self.resources.register_native(&self.scope, 42_u32).unwrap();
            self.resources
                .release(&self.scope, temporary.handle())
                .unwrap();
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (resources, tasks, ui) = setup();
    let calls = Arc::new(AtomicUsize::new(0));
    let context = tasks.create(&ui, "reentrant-cleanup").unwrap();
    let execution = context.begin().unwrap();
    context
        .own(Reentrant {
            scope: ui.clone(),
            context: context.clone(),
            resources,
            calls: calls.clone(),
        })
        .unwrap();
    execution
        .finish(Err(Error::new(
            "create_failed",
            "Exercise destructor cleanup",
        )))
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
