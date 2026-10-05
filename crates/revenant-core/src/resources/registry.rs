//! Registry ownership is distinct from lease ownership. IDs increase monotonically and are never reused within a registry.
use super::*;
#[derive(Default)]
pub(super) struct Resources {
    pub(super) scopes: HashMap<String, Scope>,
    pub(super) resources: HashMap<String, Arc<Resource>>,
    pub(super) providers: HashMap<String, std::sync::Weak<AtomicUsize>>,
    pub(super) next_resource: u64,
}

/// Host-owned chunk access. TaskContext validates the handle and holds its resource
/// lease before calling this port. Native ports and returned futures must be Send.
pub trait ResourcePort: Send + Sync + 'static {
    /// Checks a captured resource without reading bytes, including empty files.
    /// Immutable in-memory ports may use the default. Native ports report open
    /// failures and observed metadata changes; polling can involve host I/O.
    fn validate<'a>(&'a self, _handle: &'a Handle) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    /// Reads up to length bytes starting at the byte offset from an admitted handle.
    /// The Send future may borrow self and handle until completion. Leases clamp
    /// requests at EOF; before EOF the port must return 1..=length bytes or an
    /// error. Native I/O failures should be translated to the core Error type.
    fn read<'a>(
        &'a self,
        handle: &'a Handle,
        offset: u64,
        length: u32,
    ) -> BoxFuture<'a, Result<Vec<u8>>>;
}
pub use ResourcePort as ChunkReader;

/// Thread-safe final-reference cleanup for a host-owned file resource.
/// Called exactly once after owner and all leases release it, outside registry locks.
pub trait ResourceCleanup: Send + Sync + 'static {
    /// Releases native state associated with the original handle.
    /// Cannot report a recoverable error through this port; implementations should not panic.
    fn cleanup(&self, handle: &Handle);
}
impl<F: Fn(&Handle) + Send + Sync + 'static> ResourceCleanup for F {
    fn cleanup(&self, handle: &Handle) {
        self(handle)
    }
}

/// Cloneable admission registry for native file capabilities.
/// Registry clones share owners, scope state, monotonic IDs, and provider accounting.
#[derive(Clone, Default)]
pub struct ResourceRegistry {
    pub(super) inner: Arc<Mutex<Resources>>,
}

impl ResourceRegistry {
    /// Creates an empty registry with no scopes or resources and a fresh ID sequence.
    pub fn new() -> Self {
        Self::default()
    }
    /// Registers a file owner in an active scope and returns its metadata and handle.
    /// Returns `scope_disposed` or `resource_exhausted` if admission is impossible.
    /// The port is retained until both the registry owner and captured leases drop.
    pub fn register_file(
        &self,
        scope: &str,
        metadata: FileMetadata,
        port: Arc<dyn ResourcePort>,
    ) -> Result<FileEntry> {
        self.register_file_owned(scope, metadata, port, None, None)
    }
    /// Registers a file with optional provider accounting and final-reference cleanup.
    /// Provider IDs are accounting keys, not authority. Counts include leased files
    /// after owner release. Returns `scope_disposed` or `resource_exhausted`; cleanup
    /// is installed only on success and runs after the final resource reference drops.
    pub fn register_file_owned(
        &self,
        scope: &str,
        metadata: FileMetadata,
        port: Arc<dyn ResourcePort>,
        provider: Option<&str>,
        cleanup: Option<Arc<dyn ResourceCleanup>>,
    ) -> Result<FileEntry> {
        let mut inner = lock(&self.inner);
        Self::active(&inner, scope)?;
        inner.next_resource = inner
            .next_resource
            .checked_add(1)
            .ok_or_else(|| Error::new("resource_exhausted", "Resource id space is exhausted"))?;
        let id = format!("r-{}", inner.next_resource);
        let handle = Handle {
            id,
            scope: scope.into(),
            generation: 1,
            kind: "file".into(),
        };
        let entry = metadata.clone().entry(handle.clone())?;
        inner.providers.retain(|_, count| count.strong_count() > 0);
        let count = provider.map(|id| {
            if let Some(count) = inner.providers.get(id).and_then(std::sync::Weak::upgrade) {
                count
            } else {
                let count = Arc::new(AtomicUsize::new(0));
                inner.providers.insert(id.into(), Arc::downgrade(&count));
                count
            }
        });
        if let Some(count) = &count {
            count.fetch_add(1, Ordering::AcqRel);
        }
        inner.resources.insert(
            handle.id.clone(),
            Arc::new(Resource {
                handle,
                metadata,
                port,
                cleanup,
                provider_count: count,
            }),
        );
        Ok(entry)
    }
    fn checked(inner: &Resources, scope: &str, handle: &Handle) -> Result<Arc<Resource>> {
        Self::active(inner, scope)?;
        if handle.scope != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Handles cannot be adopted between scopes",
            ));
        }
        let resource = inner
            .resources
            .get(&handle.id)
            .ok_or_else(|| Error::new("stale_handle", "Resource is missing or released"))?;
        if resource.handle != *handle {
            return Err(Error::new(
                "stale_handle",
                "Handle generation or kind does not match",
            ));
        }
        Ok(resource.clone())
    }
    /// Validates a complete handle against a live owner in an active matching scope.
    /// Returns `scope_disposed`, `scope_mismatch`, or `stale_handle`. A captured
    /// lease remains valid even when this fresh-admission check fails after release.
    pub fn validate(&self, scope: &str, handle: &Handle) -> Result<()> {
        Self::checked(&lock(&self.inner), scope, handle).map(|_| ())
    }
    /// Admits a capability by validating its scope, ID, generation, and kind.
    /// Returns the same errors as validate. The captured Arc authorizes subsequent
    /// reads independently of owner release; clones extend the resource lifetime.
    pub fn lease(&self, scope: &str, handle: &Handle) -> Result<ResourceLease> {
        Ok(ResourceLease {
            resource: Self::checked(&lock(&self.inner), scope, handle)?,
        })
    }
    /// Releases only the registry owner after full handle validation.
    /// Existing leases remain readable; cleanup waits for the final lease. Returns
    /// admission errors for stale, mismatched, or disposed handles, including repeat release.
    pub fn release(&self, scope: &str, handle: &Handle) -> Result<()> {
        let resource = {
            let mut inner = lock(&self.inner);
            Self::checked(&inner, scope, handle)?;
            inner.resources.remove(&handle.id)
        };
        drop(resource); // never run host cleanup while holding the registry lock
        Ok(())
    }
    /// Returns the number of live registered or leased resources for a provider key.
    /// Missing or fully released providers return zero; leases keep their count alive.
    pub fn provider_resource_count(&self, provider: &str) -> usize {
        lock(&self.inner)
            .providers
            .get(provider)
            .and_then(std::sync::Weak::upgrade)
            .map_or(0, |c| c.load(Ordering::Acquire))
    }
}
