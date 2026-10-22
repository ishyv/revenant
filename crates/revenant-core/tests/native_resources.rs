use revenant_core::{Resource, ResourceRegistry, TaskManager, WireSafe};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Tracked(Arc<AtomicUsize>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn setup() -> (ResourceRegistry, TaskManager, String, String) {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let other = resources.create_scope(None).unwrap();
    let tasks = TaskManager::new(resources.clone());
    (resources, tasks, scope, other)
}

#[test]
fn native_accounting_tracks_released_values_until_every_lease_drops_without_history() {
    let (resources, _, scope, _other) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    assert_eq!(resources.native_resource_counts(), (0, 0));
    for _ in 0..20 {
        let resource = resources
            .register_native(&scope, Tracked(drops.clone()))
            .unwrap();
        let erased = resources
            .lease_native_handle(&scope, resource.handle())
            .unwrap();
        let typed = erased.typed::<Tracked>().unwrap();
        let clone = typed.clone();
        assert_eq!(resources.native_resource_counts(), (1, 1));
        resources.release(&scope, resource.handle()).unwrap();
        assert_eq!(resources.native_resource_counts(), (0, 1));
        drop(erased);
        drop(typed);
        assert_eq!(resources.native_resource_counts(), (0, 1));
        drop(clone);
        assert_eq!(resources.native_resource_counts(), (0, 0));
    }
    assert_eq!(drops.load(Ordering::SeqCst), 20);
    assert!(resources.register_native("missing", 0_u32).is_err());
    assert_eq!(resources.native_resource_counts(), (0, 0));
}

#[test]
fn task_retains_native_capability_after_owner_release_until_executor_exit() {
    let (resources, tasks, scope, _other) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = resources
        .register_native(&scope, Tracked(drops.clone()))
        .unwrap();
    let context = tasks.create(&scope, "scan").unwrap();
    let execution = context.begin().unwrap();
    context.admit_handle(resource.handle()).unwrap();
    resources.release(&scope, resource.handle()).unwrap();
    drop(context.lease_resource(&resource).unwrap());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    execution.finish(Ok(serde_json::Value::Null)).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn native_admission_rejects_wrong_scope_type_kind_generation_and_released_owner() {
    let (resources, _, scope, other) = setup();
    let resource = resources.register_native(&scope, 17_u32).unwrap();
    assert_eq!(
        resources
            .lease_native(&other, &resource)
            .err()
            .unwrap()
            .code,
        "scope_mismatch"
    );
    let different_type = Resource::<String>::from_handle(resource.handle().clone());
    assert_eq!(
        resources
            .lease_native(&scope, &different_type)
            .err()
            .unwrap()
            .code,
        "wrong_type"
    );
    for field in ["kind", "generation"] {
        let mut handle = resource.handle().clone();
        if field == "kind" {
            handle.kind = "file".into();
        } else {
            handle.generation += 1;
        }
        let forged = Resource::<u32>::from_handle(handle);
        assert_eq!(
            resources.lease_native(&scope, &forged).err().unwrap().code,
            "stale_handle"
        );
    }
    assert_eq!(
        resources
            .lease(&scope, resource.handle())
            .err()
            .unwrap()
            .code,
        "wrong_kind"
    );
    resources.release(&scope, resource.handle()).unwrap();
    assert_eq!(
        resources
            .lease_native(&scope, &resource)
            .err()
            .unwrap()
            .code,
        "stale_handle"
    );
}

#[test]
fn queued_cancellation_releases_native_capture_exactly_once() {
    let (resources, tasks, scope, _other) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = resources
        .register_native(&scope, Tracked(drops.clone()))
        .unwrap();
    let context = tasks.create(&scope, "queued").unwrap();
    context.admit_handle(resource.handle()).unwrap();
    resources.release(&scope, resource.handle()).unwrap();
    context.request_cancel().unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(context.lease_resource(&resource).is_err());
    drop(context);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn scope_disposal_revokes_new_admission_but_retains_existing_native_lease() {
    let (resources, _, scope, _other) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = resources
        .register_native(&scope, Tracked(drops.clone()))
        .unwrap();
    let lease = resources.lease_native(&scope, &resource).unwrap();
    resources.dispose_scope(&scope).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(
        resources
            .lease_native(&scope, &resource)
            .err()
            .unwrap()
            .code,
        "scope_disposed"
    );
    drop(lease);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn scope_disposal_cancels_running_task_without_dropping_its_active_native_lease() {
    let (resources, tasks, scope, _other) = setup();
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = resources
        .register_native(&scope, Tracked(drops.clone()))
        .unwrap();
    let context = tasks.create(&scope, "running-disposal").unwrap();
    let execution = context.begin().unwrap();
    let lease = context.lease_resource(&resource).unwrap();
    tasks.dispose_scope(&scope).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(context.checkpoint().err().unwrap().code, "cancelled");
    assert_eq!(
        context.lease_resource(&resource).err().unwrap().code,
        "cancelled"
    );
    execution
        .finish(Err(revenant_core::Error::cancelled()))
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(lease);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn native_drop_can_reenter_registry_after_release_and_scope_disposal() {
    struct Reentrant {
        scope: String,
        resources: ResourceRegistry,
        observed: Arc<AtomicUsize>,
    }
    impl Drop for Reentrant {
        fn drop(&mut self) {
            if self.resources.is_scope_active(&self.scope) {
                self.observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    let (resources, _, scope, _other) = setup();
    let observed = Arc::new(AtomicUsize::new(0));
    let resource = resources
        .register_native(
            &scope,
            Reentrant {
                scope: scope.clone(),
                resources: resources.clone(),
                observed: observed.clone(),
            },
        )
        .unwrap();
    resources.release(&scope, resource.handle()).unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    resources
        .register_native(
            &scope,
            Reentrant {
                scope: scope.clone(),
                resources: resources.clone(),
                observed: observed.clone(),
            },
        )
        .unwrap();
    resources.dispose_scope(&scope).unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[test]
fn task_created_owner_remains_scoped_after_success_and_visits_wire_identity() {
    let (resources, tasks, scope, _other) = setup();
    let context = tasks.create(&scope, "create").unwrap();
    let execution = context.begin().unwrap();
    let resource = context.own(23_u32).unwrap();
    let json = serde_json::to_value(&resource).unwrap();
    let decoded: Resource<u32> = serde_json::from_value(json.clone()).unwrap();
    let mut identities = Vec::new();
    decoded
        .visit_handles(&mut |handle| {
            identities.push(handle.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(identities, [resource.handle().clone()]);
    let definition = <Resource<u32> as revenant_core::Contract>::definition().unwrap();
    revenant_core::validate_json(&definition.schema, &json).unwrap();
    assert!(definition.typescript.contains("Handle"));
    execution.finish(Ok(json)).unwrap();
    assert_eq!(*resources.lease_native(&scope, &resource).unwrap(), 23);
    resources.dispose_scope(&scope).unwrap();
}

#[cfg(windows)]
#[test]
fn native_folder_rejects_junction_before_resolving_its_target() {
    let root = std::env::temp_dir().join(format!(
        "revenant-native-folder-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let target = root.join("target");
    let junction = root.join("junction");
    std::fs::create_dir_all(&target).unwrap();
    let result = std::process::Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    let admitted = revenant_core::NativeFolder::open(&junction);
    std::fs::remove_dir(&junction).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        admitted
            .err()
            .expect("direct native admission must reject junctions")
            .code,
        "invalid_folder"
    );
}
