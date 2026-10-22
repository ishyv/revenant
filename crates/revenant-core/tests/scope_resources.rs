use revenant_core::{ResourceRegistry, TaskManager};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn foreign_malformed_and_unissued_scope_ids_are_never_admitted_or_disposed() {
    let registry = ResourceRegistry::new();
    let foreign = ResourceRegistry::new().create_scope(None).unwrap();
    let issued = registry.create_scope(None).unwrap();
    let prefix = issued.rsplit_once(':').unwrap().0;
    for invalid in [
        foreign,
        String::new(),
        "named-scope".into(),
        format!("{prefix}:0"),
        format!("{prefix}:2"),
        format!("{prefix}:01"),
        format!("{prefix}:+1"),
    ] {
        assert!(!registry.is_scope_active(&invalid));
        assert_eq!(
            registry.create_scope(Some(&invalid)).unwrap_err().code,
            "scope_disposed"
        );
        assert_eq!(
            registry.dispose_scope(&invalid).unwrap_err().code,
            "scope_missing"
        );
    }
    registry.dispose_scope(&issued).unwrap();
    registry.dispose_scope(&issued).unwrap();
    let next = registry.create_scope(None).unwrap();
    assert_ne!(issued, next);
    assert!(!registry.is_scope_active(&issued));
}

struct Tracked(Arc<AtomicUsize>);

#[test]
fn reclaimed_scopes_cannot_resurrect_stale_native_handles() {
    let registry = ResourceRegistry::new();
    let retired = registry.create_scope(None).unwrap();
    let resource = registry.register_native(&retired, 17_u32).unwrap();
    let captured = registry.lease_native(&retired, &resource).unwrap();
    registry.dispose_scope(&retired).unwrap();
    for _ in 0..1_000 {
        let scope = registry.create_scope(None).unwrap();
        assert_eq!(
            registry.lease_native(&scope, &resource).unwrap_err().code,
            "scope_mismatch"
        );
        registry.dispose_scope(&scope).unwrap();
    }
    assert_eq!(*captured, 17);
    assert_eq!(
        registry.lease_native(&retired, &resource).unwrap_err().code,
        "scope_disposed"
    );
    assert_eq!(registry.native_resource_counts(), (0, 1));
    drop(captured);
    assert_eq!(registry.native_resource_counts(), (0, 0));
}
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn disposing_a_subtree_preserves_sibling_owners_and_active_leases() {
    let registry = ResourceRegistry::new();
    let root_scope = registry.create_scope(None).unwrap();
    let left_scope = registry.create_scope(Some(&root_scope)).unwrap();
    let leaf_scope = registry.create_scope(Some(&left_scope)).unwrap();
    let right_scope = registry.create_scope(Some(&root_scope)).unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let left = registry
        .register_native(&left_scope, Tracked(drops.clone()))
        .unwrap();
    let leaf = registry
        .register_native(&leaf_scope, Tracked(drops.clone()))
        .unwrap();
    let right = registry
        .register_native(&right_scope, Tracked(drops.clone()))
        .unwrap();
    let lease = registry.lease_native(&leaf_scope, &leaf).unwrap();
    registry.dispose_scope(&left_scope).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(registry.native_resource_counts(), (1, 2));
    assert!(!registry.is_scope_active(&left_scope));
    assert!(!registry.is_scope_active(&leaf_scope));
    assert!(registry.is_scope_active(&right_scope));
    assert_eq!(
        registry.lease_native(&left_scope, &left).unwrap_err().code,
        "scope_disposed"
    );
    assert!(registry.lease_native(&right_scope, &right).is_ok());
    registry.dispose_scope(&left_scope).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(lease);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    registry.dispose_scope(&root_scope).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 3);
    assert_eq!(registry.native_resource_counts(), (0, 0));
}

#[test]
fn deep_disposal_is_iterative_and_repeated_disposal_keeps_ids_forbidden() {
    let registry = ResourceRegistry::new();
    let root_scope = registry.create_scope(None).unwrap();
    let mut parent = root_scope.clone();
    for _ in 0..8192 {
        let next = registry.create_scope(Some(&parent)).unwrap();
        parent = next;
    }
    let drops = Arc::new(AtomicUsize::new(0));
    registry
        .register_native(&parent, Tracked(drops.clone()))
        .unwrap();
    registry.dispose_scope(&root_scope).unwrap();
    registry.dispose_scope(&root_scope).unwrap();
    registry.dispose_scope(&parent).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let replacement = registry.create_scope(None).unwrap();
    assert_ne!(replacement, parent);
    assert_eq!(registry.active_scope_count(), 1);
    assert_eq!(
        registry.create_scope(Some(&parent)).unwrap_err().code,
        "scope_disposed"
    );
    assert_eq!(
        registry.dispose_scope("missing").unwrap_err().code,
        "scope_missing"
    );
}

#[test]
fn subtree_disposal_cancels_only_its_running_task_and_drops_outside_registry_lock() {
    struct Reentrant(ResourceRegistry, Arc<AtomicUsize>, String);
    impl Drop for Reentrant {
        fn drop(&mut self) {
            assert!(!self.0.is_scope_active(&self.2));
            self.0.dispose_scope(&self.2).unwrap();
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    let registry = ResourceRegistry::new();
    let root_scope = registry.create_scope(None).unwrap();
    let left_scope = registry.create_scope(Some(&root_scope)).unwrap();
    let right_scope = registry.create_scope(Some(&root_scope)).unwrap();
    let tasks = TaskManager::new(registry.clone());
    let left = tasks.create(&left_scope, "left-task").unwrap();
    let right = tasks.create(&right_scope, "right-task").unwrap();
    let left_execution = left.begin().unwrap();
    let right_execution = right.begin().unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = left
        .own(Reentrant(
            registry.clone(),
            drops.clone(),
            left_scope.clone(),
        ))
        .unwrap();
    let lease = left.lease_resource(&resource).unwrap();
    tasks.dispose_scope(&left_scope).unwrap();
    assert_eq!(left.checkpoint().unwrap_err().code, "cancelled");
    right.checkpoint().unwrap();
    left_execution
        .finish(Err(revenant_core::Error::cancelled()))
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(lease);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    right_execution.finish(Ok(serde_json::Value::Null)).unwrap();
}
