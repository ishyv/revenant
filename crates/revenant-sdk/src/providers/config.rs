//! Configuration metadata, inspection, and dependency graph admission. Reconfiguration builds a distinct candidate; transaction publication never mutates an active instance in place.
use crate::operations::{RegistryData, locked};
use crate::{DecimalU64, Error, OperationRegistry, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::Ordering,
};
/// Declares a compiled capability's identity, dependency graph, and configuration contract.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDescriptor {
    /// Stable capability identity retained by compatible runtime replacements.
    pub id: String,
    /// Markdown explanation shown by runtime inspection and generated documentation.
    pub description: String,
    /// Capabilities this provider uses; their generations are pinned during execution.
    pub dependencies: Vec<String>,
    /// JSON Schema that initial and replacement configuration must satisfy.
    pub configuration_schema: Value,
}
/// Inspectable state of one currently installed native capability.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSnapshot {
    /// Stable capability identity retained by compatible runtime replacements.
    pub id: String,
    /// Markdown explanation shown by runtime inspection and generated documentation.
    pub description: String,
    /// Capabilities this provider uses; their generations are pinned during execution.
    pub dependencies: Vec<String>,
    /// JSON Schema that initial and replacement configuration must satisfy.
    pub configuration_schema: Value,
    /// Validated configuration belonging to this installed generation.
    pub config: Value,
    /// Monotonic generation; successful replacements advance it atomically.
    pub generation: u32,
    /// Number of executing or admitted tasks pinning this provider, as a lossless decimal.
    pub active_tasks: DecimalU64,
    /// Number of provider-owned resources, including resources retained by leases.
    pub resources: DecimalU64,
    /// Whether this compiled capability participates in supported replacement transactions.
    pub mutable: bool,
}

impl OperationRegistry {
    /// Clones installed configurations and generation metadata in provider-ID order.
    /// Task counters count admitted/executing guards; resource counters include leases
    /// after owner release. Both serialize as decimal item counts, not byte counts.
    /// Does not invoke provider callbacks or acquire new resources.
    pub fn inspect(&self) -> Vec<ProviderSnapshot> {
        locked(&self.inner)
            .providers
            .values()
            .map(|record| ProviderSnapshot {
                id: record.descriptor.id.clone(),
                description: record.descriptor.description.clone(),
                dependencies: record.descriptor.dependencies.clone(),
                configuration_schema: record.descriptor.configuration_schema.clone(),
                config: record.config.clone(),
                generation: record.generation,
                active_tasks: (record.active.load(Ordering::Acquire) as u64).into(),
                resources: (self
                    .resources
                    .provider_resource_count(&record.descriptor.id)
                    as u64)
                    .into(),
                mutable: true,
            })
            .collect()
    }
    /// Builds a distinct candidate from the installed provider, then replaces it
    /// through the normal schema, dependency, compatibility, and busy checks.
    /// Returns `provider_missing` for an unknown ID or `configuration_unsupported`
    /// when the provider supplies no candidate; candidate and transaction errors
    /// propagate. Failure leaves the installed configuration and generation unchanged.
    pub async fn configure(&self, id: &str, config: Value) -> Result<()> {
        let provider = locked(&self.inner)
            .providers
            .get(id)
            .ok_or_else(|| Error::new("provider_missing", "Unknown provider"))?
            .provider
            .clone();
        let candidate = provider.reconfigured(config.clone())?;
        self.replace(id, candidate, config).await
    }
}
pub(super) fn validate_dependencies(
    data: &RegistryData,
    candidate: &ProviderDescriptor,
) -> Result<()> {
    fn visit(
        id: &str,
        graph: &BTreeMap<String, Vec<String>>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<()> {
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.into()) {
            return Err(Error::new(
                "dependency_cycle",
                format!("Dependency cycle includes {id}"),
            ));
        }
        let dependencies = graph
            .get(id)
            .ok_or_else(|| Error::new("dependency_missing", format!("Missing dependency {id}")))?;
        for dependency in dependencies {
            visit(dependency, graph, visiting, visited)?;
        }
        visiting.remove(id);
        visited.insert(id.into());
        Ok(())
    }
    let mut graph: BTreeMap<_, _> = data
        .providers
        .iter()
        .map(|(id, p)| (id.clone(), p.descriptor.dependencies.clone()))
        .collect();
    graph.insert(candidate.id.clone(), candidate.dependencies.clone());
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in graph.keys() {
        visit(id, &graph, &mut visiting, &mut visited)?;
    }
    Ok(())
}
