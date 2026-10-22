use super::PreviewStore;
use revenant_core::{
    BoxFuture, FileMetadata, Handle, ResourceCleanup, ResourceLease, ResourcePort,
    ResourceRegistry, Result,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

struct Gate {
    entered: Semaphore,
    release: Semaphore,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }

    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}

struct Reader {
    gate: Option<Arc<Gate>>,
}

impl ResourcePort for Reader {
    fn read<'a>(
        &'a self,
        _handle: &'a Handle,
        offset: u64,
        length: u32,
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            if let Some(gate) = &self.gate {
                gate.entered.add_permits(1);
                tokio::time::timeout(Duration::from_secs(5), gate.release.acquire())
                    .await
                    .unwrap()
                    .unwrap()
                    .forget();
            }
            Ok(b"preview!"[offset as usize..offset as usize + length as usize].to_vec())
        })
    }
}

fn store(resources: &ResourceRegistry) -> Arc<PreviewStore> {
    Arc::new(PreviewStore::new(resources.clone()))
}

fn lease(
    resources: &ResourceRegistry,
    scope: &str,
    mime: &str,
    gate: Option<Arc<Gate>>,
    cleanup: Option<Arc<dyn ResourceCleanup>>,
) -> ResourceLease {
    let entry = resources
        .register_file_owned(
            scope,
            FileMetadata {
                name: "preview.txt".into(),
                relative_path: "preview.txt".into(),
                size: 8_u64.into(),
                mime: mime.into(),
                modified: None,
            },
            Arc::new(Reader { gate }),
            Some("preview-fixture"),
            cleanup,
        )
        .unwrap();
    resources.lease(scope, &entry.handle).unwrap()
}

#[test]
fn twenty_empty_scope_cycles_reclaim_all_preview_owner_records() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    for cycle in 0..20 {
        let scope = resources.create_scope(None).unwrap();
        resources.dispose_scope(&scope).unwrap();
        previews.dispose_scope(&scope).unwrap();
        previews.dispose_scope(&scope).unwrap();
        assert_eq!(resources.active_scope_count(), 0);
        assert_eq!(previews.owner_record_count().unwrap(), 0, "cycle {cycle}");
    }
}

#[tokio::test]
async fn twenty_preview_cycles_release_tokens_and_final_resource_leases() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    let cleaned = Arc::new(AtomicUsize::new(0));
    for cycle in 0..20 {
        let scope = resources.create_scope(None).unwrap();
        let count = cleaned.clone();
        let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_: &Handle| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let descriptor = previews
            .create(
                &scope,
                lease(&resources, &scope, "text/plain", None, Some(cleanup)),
            )
            .await
            .unwrap();
        assert_eq!(descriptor.text.as_deref(), Some("preview!"));
        assert_eq!(previews.owner_record_count().unwrap(), 1);
        resources.dispose_scope(&scope).unwrap();
        assert_eq!(cleaned.load(Ordering::SeqCst), cycle);
        previews.dispose_scope(&scope).unwrap();
        assert_eq!(previews.owner_record_count().unwrap(), 0);
        assert_eq!(cleaned.load(Ordering::SeqCst), cycle + 1);
        assert_eq!(resources.provider_resource_count("preview-fixture"), 0);
        assert_eq!(
            previews.read(&descriptor.id, None).await.unwrap_err().code,
            "preview_not_found"
        );
    }
}

#[tokio::test]
async fn retained_lease_cannot_create_after_authority_retires_before_preview_cleanup() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    let scope = resources.create_scope(None).unwrap();
    let retained = lease(&resources, &scope, "text/plain", None, None);
    resources.dispose_scope(&scope).unwrap();
    assert_eq!(
        previews
            .create(&scope, retained.clone())
            .await
            .unwrap_err()
            .code,
        "scope_disposed"
    );
    assert_eq!(retained.read(0, 8).await.unwrap(), b"preview!");
    assert_eq!(previews.owner_record_count().unwrap(), 0);
}

#[tokio::test]
async fn scope_from_another_registry_cannot_publish_a_preview() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    let foreign = ResourceRegistry::new();
    let scope = foreign.create_scope(None).unwrap();
    assert_eq!(
        previews
            .create(&scope, lease(&foreign, &scope, "text/plain", None, None))
            .await
            .unwrap_err()
            .code,
        "scope_disposed"
    );
    assert_eq!(previews.owner_record_count().unwrap(), 0);
}

#[tokio::test]
async fn closing_during_text_io_rejects_publication_with_or_without_preview_cleanup() {
    for cleanup in [false, true] {
        let resources = ResourceRegistry::new();
        let previews = store(&resources);
        let scope = resources.create_scope(None).unwrap();
        let gate = Gate::new();
        let retained = lease(&resources, &scope, "text/plain", Some(gate.clone()), None);
        let create = tokio::spawn({
            let previews = previews.clone();
            let scope = scope.clone();
            async move { previews.create(&scope, retained).await }
        });
        gate.entered().await;
        resources.dispose_scope(&scope).unwrap();
        if cleanup {
            previews.dispose_scope(&scope).unwrap();
        }
        gate.release.add_permits(1);
        let result = tokio::time::timeout(Duration::from_secs(5), create)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().code, "scope_disposed");
        assert_eq!(previews.owner_record_count().unwrap(), 0);
        assert_eq!(resources.provider_resource_count("preview-fixture"), 0);
    }
}

#[tokio::test]
async fn retired_scope_rejects_new_media_reads_before_preview_cleanup() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    let scope = resources.create_scope(None).unwrap();
    let descriptor = previews
        .create(&scope, lease(&resources, &scope, "image/png", None, None))
        .await
        .unwrap();
    resources.dispose_scope(&scope).unwrap();
    assert_eq!(previews.owner_record_count().unwrap(), 1);
    assert_eq!(
        previews.read(&descriptor.id, None).await.unwrap_err().code,
        "scope_disposed"
    );
    previews.dispose_scope(&scope).unwrap();
    assert_eq!(previews.owner_record_count().unwrap(), 0);
    assert_eq!(resources.provider_resource_count("preview-fixture"), 0);
}

#[tokio::test]
async fn admitted_media_read_finishes_after_scope_and_preview_disposal() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    let scope = resources.create_scope(None).unwrap();
    let gate = Gate::new();
    let descriptor = previews
        .create(
            &scope,
            lease(&resources, &scope, "image/png", Some(gate.clone()), None),
        )
        .await
        .unwrap();
    let read = tokio::spawn({
        let previews = previews.clone();
        let token = descriptor.id.clone();
        async move { previews.read(&token, None).await }
    });
    gate.entered().await;
    resources.dispose_scope(&scope).unwrap();
    previews.dispose_scope(&scope).unwrap();
    assert_eq!(previews.owner_record_count().unwrap(), 0);
    assert_eq!(resources.provider_resource_count("preview-fixture"), 1);
    assert_eq!(
        previews.read(&descriptor.id, None).await.unwrap_err().code,
        "preview_not_found"
    );
    gate.release.add_permits(1);
    let response = tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.body, b"preview!");
    assert_eq!(resources.provider_resource_count("preview-fixture"), 0);
}

#[test]
fn preview_cleanup_requires_scope_authority_retirement_first() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    let scope = resources.create_scope(None).unwrap();
    assert_eq!(
        previews.dispose_scope(&scope).unwrap_err().code,
        "scope_active"
    );
    assert!(resources.is_scope_active(&scope));
    resources.dispose_scope(&scope).unwrap();
    previews.dispose_scope(&scope).unwrap();
    assert_eq!(previews.owner_record_count().unwrap(), 0);
}

#[tokio::test]
async fn final_resource_cleanup_can_reenter_preview_diagnostics() {
    let resources = ResourceRegistry::new();
    let previews = store(&resources);
    let scope = resources.create_scope(None).unwrap();
    let weak = Arc::downgrade(&previews);
    let cleaned = Arc::new(AtomicUsize::new(0));
    let cleaned_callback = cleaned.clone();
    let cleanup: Arc<dyn ResourceCleanup> = Arc::new(move |_: &Handle| {
        assert_eq!(weak.upgrade().unwrap().owner_record_count().unwrap(), 0);
        cleaned_callback.fetch_add(1, Ordering::SeqCst);
    });
    previews
        .create(
            &scope,
            lease(&resources, &scope, "image/png", None, Some(cleanup)),
        )
        .await
        .unwrap();
    resources.dispose_scope(&scope).unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let cleanup_thread = std::thread::spawn(move || {
        sent.send(previews.dispose_scope(&scope)).unwrap();
    });
    received
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    cleanup_thread.join().unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
}
