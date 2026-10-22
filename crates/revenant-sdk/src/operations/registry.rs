//! Registry mutation and pinned dispatch share one lock. Public manifests contain sorted descriptors; execution and provider callbacks run outside that lock.
use super::{ContractManifest, ContractValidators, OperationRegistration, validate_registration};
use crate::providers::{ProviderRecord, ProviderUse};
use crate::{Error, ResourceRegistry, Result, TaskContext};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
pub(crate) struct RegisteredOperation {
    pub registration: OperationRegistration,
    pub validators: Arc<ContractValidators>,
    pub provider: Option<String>,
}
#[derive(Default)]
pub(crate) struct RegistryData {
    /// Deterministically ordered operation contracts available to this application.
    pub operations: BTreeMap<String, RegisteredOperation>,
    pub providers: BTreeMap<String, ProviderRecord>,
    pub reserved: std::collections::BTreeSet<String>,
}
/// Owns compiled operations and transactional capability generations.
#[derive(Clone, Default)]
pub struct OperationRegistry {
    pub(crate) inner: Arc<Mutex<RegistryData>>,
    pub(crate) resources: ResourceRegistry,
}
/// Convenient name for the advanced operation registry used by the desktop host.
pub type Registry = OperationRegistry;
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl OperationRegistry {
    /// Creates an empty registry with its own resource ownership domain.
    pub fn new() -> Self {
        Self::default()
    }
    /// Creates a registry using the host's shared resource ownership domain.
    pub fn with_resources(resources: ResourceRegistry) -> Self {
        Self {
            resources,
            ..Self::default()
        }
    }
    /// Returns the ownership domain used by installed providers and operation inputs.
    pub fn resources(&self) -> &ResourceRegistry {
        &self.resources
    }
    /// Validates and publishes one operation atomically.
    /// Returns `operation_conflict` for a duplicate ID or active provider transaction,
    /// and propagates invalid schema/contract failures. The executor is retained
    /// until removed by a provider transaction or the last registration drops.
    pub fn register(&self, registration: OperationRegistration) -> Result<()> {
        let validators = validate_registration(&registration)?;
        let mut data = locked(&self.inner);
        let id = &registration.descriptor.id;
        if data.operations.contains_key(id) || !data.reserved.is_empty() {
            return Err(Error::new(
                "operation_conflict",
                "Operation already registered or provider transaction is active",
            ));
        }
        data.operations.insert(
            id.clone(),
            RegisteredOperation {
                registration,
                validators,
                provider: None,
            },
        );
        Ok(())
    }
    /// Builds the version 6/protocol 5 contract from operation IDs in sorted order.
    /// Includes source metadata and documentation in the SHA-256 digest; does not
    /// start operations or prepare providers. JSON serialization failures propagate.
    pub fn manifest(&self) -> Result<ContractManifest> {
        let operations: Vec<_> = locked(&self.inner)
            .operations
            .values()
            .map(|o| o.registration.descriptor.clone())
            .collect();
        let bytes = serde_json::to_vec(&operations)?;
        Ok(ContractManifest {
            version: revenant_core::CONTRACT_MANIFEST_VERSION,
            protocol: revenant_core::BRIDGE_PROTOCOL_VERSION,
            digest: format!("sha256:{:x}", Sha256::digest(bytes)),
            operations,
        })
    }
    /// Serializes the same sorted contract returned by manifest for build tooling.
    /// Does not execute application operations; manifest and JSON errors propagate.
    pub fn manifest_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.manifest()?)?)
    }
    pub(super) fn acquire(
        &self,
        id: &str,
    ) -> Result<(OperationRegistration, Arc<ContractValidators>, ProviderUse)> {
        let data = locked(&self.inner);
        let operation = data
            .operations
            .get(id)
            .ok_or_else(|| Error::new("operation_missing", format!("Unknown operation {id}")))?;
        let usage = ProviderUse::acquire(&data, operation.provider.as_deref())?;
        Ok((
            operation.registration.clone(),
            operation.validators.clone(),
            usage,
        ))
    }
    pub(super) async fn execute(
        registration: &OperationRegistration,
        validators: &ContractValidators,
        input: Value,
        context: TaskContext,
    ) -> Result<Value> {
        context.checkpoint()?;
        context.configure_work_class(registration.descriptor.work_class)?;
        if let Some(updates) = &validators.updates {
            context.configure_prepared_updates(updates.clone())?;
        }
        validators.input.validate(&input)?;
        registration.executor.validate_input(&input, &context)?;
        let output = registration.executor.execute(input, context).await?;
        validators.output.validate(&output)?;
        Ok(output)
    }
    /// Begins and finalizes a single-operation task while pinning its provider use.
    /// Propagates task-start, lookup, validation, execution, or finalization errors.
    /// Dropping the future interrupts the execution guard and releases pinned use.
    /// Native streaming batch hosts instead prepare one operation and own their guards.
    pub async fn dispatch(&self, id: &str, input: Value, context: TaskContext) -> Result<Value> {
        let execution = context.begin()?;
        let result = match self.acquire(id) {
            Ok((registration, validators, _usage)) => {
                Self::execute(&registration, &validators, input, context).await
            }
            Err(error) => Err(error),
        };
        execution.finish(result.clone())?;
        result
    }
}
