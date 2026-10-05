//! Provider preparation scopes and generation-use guards own native lifetimes. Dropping a record disposes its scope before cleanup; use guards pin all transitive dependencies.
use super::ProviderDescriptor;
use crate::operations::RegistryData;
use crate::{
    BoxFuture, Error, FileEntry, FileMetadata, OperationRegistration, ResourceCleanup,
    ResourcePort, ResourceRegistry, Result,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
/// Preparation happens in an isolated candidate scope. Implementations must
/// dispose external acquisitions in cleanup(), including after failed preparation.
/// Configuration produces a new candidate rather than mutating an active provider.
/// Native implementations are Send + Sync and their preparation futures are Send.
/// The registry owns the installed generation until removal or final registry drop.
pub trait Provider: Send + Sync + 'static {
    /// Returns inert metadata; must not acquire resources or change provider state.
    fn descriptor(&self) -> ProviderDescriptor;
    /// Returns inert typed registrations without acquiring resources. IDs and input/
    /// output contracts must match the installed generation on replacement; descriptor
    /// and registration errors abort preparation and invoke candidate cleanup.
    fn operations(&self) -> Result<Vec<OperationRegistration>>;
    /// Acquires resources only in the supplied candidate context, using validated config.
    /// Publication follows success; returned errors or a dropped future reclaim the
    /// candidate scope and call cleanup. The default implementation acquires nothing.
    fn prepare(&self, _context: ProviderContext, _config: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    /// Releases external acquisitions after failed preparation or final removal; must not panic.
    fn cleanup(&self) {}
    /// Creates a distinct candidate for atomic reconfiguration; the active instance
    /// must remain untouched. The default returns `configuration_unsupported`.
    /// Candidate schema validation and preparation happen in the replacement transaction.
    fn reconfigured(&self, _config: Value) -> Result<Arc<dyn Provider>> {
        Err(Error::new(
            "configuration_unsupported",
            "Provider does not supply transactional configuration candidates",
        ))
    }
}
/// Isolated ownership scope supplied while a native provider is preparing.
#[derive(Clone)]
pub struct ProviderContext {
    /// Native ownership domain available during isolated provider preparation.
    pub resources: ResourceRegistry,
    /// Candidate scope; resources registered here are reclaimed if preparation fails.
    pub scope: String,
    /// Provider identity used for resource accounting and dependency checks.
    pub provider: String,
}
impl ProviderContext {
    /// Registers a file owner in the candidate scope, attributed to this provider.
    /// The port and cleanup remain retained until owner and all leases release the
    /// file. Native metadata.modified uses Unix milliseconds when present.
    /// Propagates disposed-scope or resource-ID exhaustion errors from core admission.
    pub fn register_file(
        &self,
        metadata: FileMetadata,
        reader: Arc<dyn ResourcePort>,
        cleanup: Option<Arc<dyn ResourceCleanup>>,
    ) -> Result<FileEntry> {
        self.resources.register_file_owned(
            &self.scope,
            metadata,
            reader,
            Some(&self.provider),
            cleanup,
        )
    }
}
pub(crate) struct ProviderRecord {
    pub descriptor: ProviderDescriptor,
    /// Validated configuration belonging to this installed generation.
    pub config: Value,
    /// Provider identity used for resource accounting and dependency checks.
    pub provider: Arc<dyn Provider>,
    /// Candidate scope; resources registered here are reclaimed if preparation fails.
    pub scope: String,
    pub active: Arc<AtomicUsize>,
    /// Monotonic generation; successful replacements advance it atomically.
    pub generation: u32,
    /// Native ownership domain available during isolated provider preparation.
    pub resources: ResourceRegistry,
}
impl Drop for ProviderRecord {
    fn drop(&mut self) {
        let _ = self.resources.dispose_scope(&self.scope);
        self.provider.cleanup();
    }
}
pub(crate) struct ProviderUse {
    counts: Vec<Arc<AtomicUsize>>,
}
impl ProviderUse {
    /// Pins each transitive provider generation once after the graph is validated.
    /// Reserved or missing dependencies fail before any counter is incremented;
    /// dropping the returned guard releases every count it acquired.
    pub fn acquire(data: &RegistryData, provider: Option<&str>) -> Result<Self> {
        fn collect(data: &RegistryData, id: &str, seen: &mut BTreeSet<String>) -> Result<()> {
            if !seen.insert(id.into()) {
                return Ok(());
            }
            if data.reserved.contains(id) {
                return Err(Error::new(
                    "provider_busy",
                    "Provider is in a replacement transaction",
                )
                .retryable(true));
            }
            let record = data.providers.get(id).ok_or_else(|| {
                Error::new("dependency_missing", format!("Missing provider {id}"))
            })?;
            for dependency in &record.descriptor.dependencies {
                collect(data, dependency, seen)?;
            }
            Ok(())
        }
        let mut ids = BTreeSet::new();
        if let Some(id) = provider {
            collect(data, id, &mut ids)?;
        }
        let counts = ids
            .into_iter()
            .map(|id| {
                let count = data.providers[&id].active.clone();
                count.fetch_add(1, Ordering::AcqRel);
                count
            })
            .collect();
        Ok(Self { counts })
    }
}
impl Drop for ProviderUse {
    fn drop(&mut self) {
        for count in &self.counts {
            count.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
