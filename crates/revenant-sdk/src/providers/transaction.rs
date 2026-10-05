//! Replacement reserves affected provider generations before async preparation. A candidate publishes atomically; failed or dropped transactions dispose candidate resources and release reservations.
use super::{Provider, ProviderContext, ProviderRecord, config::validate_dependencies};
use crate::operations::{RegisteredOperation, locked, validate_registration};
use crate::{Error, OperationRegistry, Result, validate_json};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
static NEXT_CANDIDATE: AtomicU64 = AtomicU64::new(1);
struct Transaction {
    registry: OperationRegistry,
    candidate: Arc<dyn Provider>,
    scope: String,
    reservations: BTreeSet<String>,
    committed: bool,
}
impl Drop for Transaction {
    fn drop(&mut self) {
        // Scope disposal precedes cleanup: leaked handles cannot survive a failed
        // or abandoned candidate. No host callback runs under the registry mutex.
        if !self.committed {
            let _ = self.registry.resources.dispose_scope(&self.scope);
            self.candidate.cleanup();
        }
        let mut data = locked(&self.registry.inner);
        for id in &self.reservations {
            data.reserved.remove(id);
        }
    }
}

impl OperationRegistry {
    /// Prepares and atomically installs a new capability with schema-validated config.
    /// Rejects reused provider instances, duplicate identities/operations, invalid
    /// contracts, missing/cyclic dependencies, or conflicting transactions. Failure
    /// and future drop reclaim candidate resources; only success publishes operations.
    pub async fn install(&self, provider: Arc<dyn Provider>, config: Value) -> Result<()> {
        self.change_provider(None, provider, config).await
    }
    /// Replaces an installed provider with a distinct compatible candidate.
    /// The ID and operation input/output contracts must remain compatible. Active
    /// task guards and leased resources in affected generations return retryable
    /// `provider_busy`; validation and preparation failures leave the old generation
    /// installed. A successful atomic publication advances its generation and cleans
    /// the old provider outside the registry mutex.
    pub async fn replace(
        &self,
        id: &str,
        provider: Arc<dyn Provider>,
        config: Value,
    ) -> Result<()> {
        self.change_provider(Some(id), provider, config).await
    }
    pub(super) async fn change_provider(
        &self,
        replacing: Option<&str>,
        candidate: Arc<dyn Provider>,
        config: Value,
    ) -> Result<()> {
        if locked(&self.inner)
            .providers
            .values()
            .any(|record| Arc::ptr_eq(&record.provider, &candidate))
        {
            return Err(Error::new(
                "provider_identity",
                "Use a distinct candidate instance; the active provider cannot prepare or clean itself",
            ));
        }
        // Own the candidate immediately so even invalid schema/dependency attempts
        // invoke its cleanup hook. Cleanup is synchronous to work on future Drop.
        let scope = format!(
            "provider-candidate-{}",
            NEXT_CANDIDATE.fetch_add(1, Ordering::Relaxed)
        );
        self.resources.create_scope(&scope, None)?;
        let mut transaction = Transaction {
            registry: self.clone(),
            candidate: candidate.clone(),
            scope: scope.clone(),
            reservations: BTreeSet::new(),
            committed: false,
        };
        let descriptor = candidate.descriptor();
        if descriptor.id.is_empty() || replacing.is_some_and(|id| id != descriptor.id) {
            return Err(Error::new(
                "provider_identity",
                "Replacement must retain its provider id",
            ));
        }
        crate::validate_contract_schema(&descriptor.configuration_schema)?;
        validate_json(&descriptor.configuration_schema, &config)?;
        let operations = candidate.operations()?;
        let mut ids = BTreeSet::new();
        for operation in &operations {
            validate_registration(operation)?;
            if !ids.insert(operation.descriptor.id.clone()) {
                return Err(Error::new(
                    "operation_conflict",
                    "Candidate has duplicate operation ids",
                ));
            }
        }
        let generation = {
            let mut data = locked(&self.inner);
            if !data.reserved.is_empty() {
                return Err(
                    Error::new("provider_busy", "Another provider transaction is active")
                        .retryable(true),
                );
            }
            match replacing {
                Some(id) if !data.providers.contains_key(id) => {
                    return Err(Error::new(
                        "provider_missing",
                        "Unknown replacement provider",
                    ));
                }
                None if data.providers.contains_key(&descriptor.id) => {
                    return Err(Error::new(
                        "provider_conflict",
                        "Provider already installed",
                    ));
                }
                _ => {}
            }
            validate_dependencies(&data, &descriptor)?;
            if let Some(id) = replacing {
                let old_operations: BTreeMap<_, _> = data
                    .operations
                    .iter()
                    .filter(|(_, operation)| operation.provider.as_deref() == Some(id))
                    .map(|(id, operation)| (id.clone(), &operation.registration.descriptor))
                    .collect();
                if old_operations.len() != operations.len()
                    || operations.iter().any(|operation| {
                        old_operations
                            .get(&operation.descriptor.id)
                            .is_none_or(|old| {
                                old.input != operation.descriptor.input
                                    || old.output != operation.descriptor.output
                            })
                    })
                {
                    return Err(Error::new(
                        "provider_contract_mismatch",
                        "Replacement must preserve operation ids and input/output contracts",
                    ));
                }
            }
            for operation in &operations {
                if data
                    .operations
                    .get(&operation.descriptor.id)
                    .is_some_and(|old| old.provider.as_deref() != replacing)
                {
                    return Err(Error::new(
                        "operation_conflict",
                        "Candidate operation id is supplied by another provider",
                    ));
                }
            }
            // Reserve the changed provider, its dependents, and the dependencies
            // needed to prepare the candidate. Unrelated providers keep running.
            let mut affected = BTreeSet::from([descriptor.id.clone()]);
            loop {
                let before = affected.len();
                for (id, provider) in &data.providers {
                    if provider
                        .descriptor
                        .dependencies
                        .iter()
                        .any(|dependency| affected.contains(dependency))
                    {
                        affected.insert(id.clone());
                    }
                }
                if before == affected.len() {
                    break;
                }
            }
            let changed = affected.clone();
            let mut pending = descriptor.dependencies.clone();
            while let Some(id) = pending.pop() {
                if affected.insert(id.clone()) {
                    pending.extend(data.providers[&id].descriptor.dependencies.iter().cloned());
                }
            }
            for id in &affected {
                if changed.contains(id)
                    && (data
                        .providers
                        .get(id)
                        .is_some_and(|p| p.active.load(Ordering::Acquire) != 0)
                        || self.resources.provider_resource_count(id) != 0)
                {
                    return Err(Error::new(
                        "provider_busy",
                        format!("Provider {id} has active tasks or resources"),
                    )
                    .retryable(true));
                }
            }
            let generation = data.providers.get(&descriptor.id).map_or(Ok(1), |old| {
                old.generation.checked_add(1).ok_or_else(|| {
                    Error::new("generation_exhausted", "Provider generation is exhausted")
                })
            })?;
            data.reserved.extend(affected.iter().cloned());
            transaction.reservations = affected;
            generation
        };
        candidate
            .prepare(
                ProviderContext {
                    resources: self.resources.clone(),
                    scope: scope.clone(),
                    provider: descriptor.id.clone(),
                },
                config.clone(),
            )
            .await?;
        let previous = {
            let mut data = locked(&self.inner);
            // Candidate preparation is the only async boundary. Every affected
            // provider stays reserved until the atomic activation is complete.
            if replacing.is_some() {
                data.operations
                    .retain(|_, operation| operation.provider.as_deref() != replacing);
            }
            for registration in operations {
                data.operations.insert(
                    registration.descriptor.id.clone(),
                    RegisteredOperation {
                        registration,
                        provider: Some(descriptor.id.clone()),
                    },
                );
            }
            data.providers.insert(
                descriptor.id.clone(),
                ProviderRecord {
                    descriptor,
                    config,
                    provider: candidate,
                    scope,
                    active: Arc::new(AtomicUsize::new(0)),
                    generation,
                    resources: self.resources.clone(),
                },
            )
        };
        transaction.committed = true;
        drop(previous);
        Ok(())
    }
}
