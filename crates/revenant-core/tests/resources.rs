use revenant_core::{
    BoxFuture, FileEntry, FileMetadata, Handle, ResourceCleanup, ResourcePort, ResourceRegistry,
    Result,
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

fn register(registry: &ResourceRegistry, scope: &str) -> FileEntry {
    registry
        .register_file(scope, metadata(), Arc::new(TestReader))
        .unwrap()
}

#[test]
fn handles_are_scope_bound_and_stale_after_release() {
    let registry = ResourceRegistry::new();
    registry.create_scope("owner", None).unwrap();
    registry.create_scope("other", None).unwrap();
    let entry = register(&registry, "owner");

    assert_eq!(
        registry.validate("other", &entry.handle).unwrap_err().code,
        "scope_mismatch"
    );

    let mut stale_generation = entry.handle.clone();
    stale_generation.generation += 1;
    assert_eq!(
        registry
            .validate("owner", &stale_generation)
            .unwrap_err()
            .code,
        "stale_handle"
    );

    registry.release("owner", &entry.handle).unwrap();
    assert_eq!(
        registry.validate("owner", &entry.handle).unwrap_err().code,
        "stale_handle"
    );
    assert_eq!(
        registry.release("owner", &entry.handle).unwrap_err().code,
        "stale_handle"
    );
}

#[test]
fn resource_cleanup_waits_for_leases_after_release_or_scope_disposal() {
    let registry = ResourceRegistry::new();
    registry.create_scope("released", None).unwrap();
    registry.create_scope("disposed", None).unwrap();

    let released_cleanup = Arc::new(AtomicUsize::new(0));
    let released_cleanup_for_callback = released_cleanup.clone();
    let released_hook: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
        released_cleanup_for_callback.fetch_add(1, Ordering::SeqCst);
    });
    let released_entry = registry
        .register_file_owned(
            "released",
            metadata(),
            Arc::new(TestReader),
            Some("provider-release"),
            Some(released_hook),
        )
        .unwrap();
    let released_lease = registry.lease("released", &released_entry.handle).unwrap();
    registry
        .release("released", &released_entry.handle)
        .unwrap();
    assert_eq!(registry.provider_resource_count("provider-release"), 1);
    assert_eq!(released_cleanup.load(Ordering::SeqCst), 0);
    drop(released_lease);
    assert_eq!(registry.provider_resource_count("provider-release"), 0);
    assert_eq!(released_cleanup.load(Ordering::SeqCst), 1);

    let disposed_cleanup = Arc::new(AtomicUsize::new(0));
    let disposed_cleanup_for_callback = disposed_cleanup.clone();
    let disposed_hook: Arc<dyn ResourceCleanup> = Arc::new(move |_handle: &Handle| {
        disposed_cleanup_for_callback.fetch_add(1, Ordering::SeqCst);
    });
    let disposed_entry = registry
        .register_file_owned(
            "disposed",
            metadata(),
            Arc::new(TestReader),
            Some("provider-dispose"),
            Some(disposed_hook),
        )
        .unwrap();
    let disposed_lease = registry.lease("disposed", &disposed_entry.handle).unwrap();

    registry.dispose_scope("disposed").unwrap();
    assert!(!registry.is_scope_active("disposed"));
    assert_eq!(
        registry
            .validate("disposed", &disposed_entry.handle)
            .unwrap_err()
            .code,
        "scope_disposed"
    );
    assert_eq!(registry.provider_resource_count("provider-dispose"), 1);
    assert_eq!(disposed_cleanup.load(Ordering::SeqCst), 0);

    drop(disposed_lease);
    assert_eq!(registry.provider_resource_count("provider-dispose"), 0);
    assert_eq!(disposed_cleanup.load(Ordering::SeqCst), 1);
}
