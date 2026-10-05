//! An admitted operation pins its provider and dependency generations. Hosts own task execution guards and can reuse one operation guard across short-lived batch children.
use super::{OperationDescriptor, OperationRegistration, OperationRegistry};
use crate::providers::ProviderUse;
use crate::{Result, TaskContext, validate_json};
use serde_json::Value;
/// A pinned compiled operation admitted by the native executor.
///
/// This guard keeps provider generations stable while queued or executing.
/// Large batches reuse it and give each item a short-lived task context.
pub struct PreparedOperation {
    registration: OperationRegistration,
    _usage: ProviderUse,
}
impl PreparedOperation {
    /// Borrows the contract pinned by this guard; it remains valid for the guard lifetime.
    pub fn descriptor(&self) -> &OperationDescriptor {
        &self.registration.descriptor
    }

    /// Validates the pinned input schema and admits every referenced handle.
    /// Captures are stored in the supplied task context before queueing; repeated
    /// validation reuses those leases after owner release. Propagates cancellation,
    /// schema validation, deserialization, and resource-admission errors.
    pub fn prepare_input(&self, input: &Value, context: &TaskContext) -> Result<()> {
        context.checkpoint()?;
        validate_json(&self.registration.descriptor.input.schema, input)?;
        self.registration.executor.validate_input(input, context)
    }
    /// Enumerates typed input capabilities without acquiring or executing them.
    /// Deserialization and contract visitor errors propagate; JSON lookalikes do not count.
    pub fn input_handles(&self, input: &Value) -> Result<Vec<crate::Handle>> {
        self.registration.executor.input_handles(input)
    }
    /// Enumerates typed output capabilities for independent native result leases.
    /// Deserialization and contract visitor errors propagate; the host validates ownership.
    pub fn output_handles(&self, output: &Value) -> Result<Vec<crate::Handle>> {
        self.registration.executor.output_handles(output)
    }

    /// Runs within an executor already started by its host.
    ///
    /// The caller owns terminal state reporting; output is checked against the
    /// same compiled contract used by TypeScript generation. The input is validated
    /// again against the pinned schema and cached capabilities. Errors propagate;
    /// this method neither begins nor finishes the task. Keep this guard alive
    /// until execution ends so provider replacement cannot change its generation.
    pub async fn execute(&self, input: Value, context: TaskContext) -> Result<Value> {
        OperationRegistry::execute(&self.registration, input, context).await
    }
}

impl OperationRegistry {
    /// Pins an operation and its dependency generations before queue admission.
    ///
    /// Keep the returned guard until execution ends; replacement rejects busy
    /// providers, including their transitive dependencies.
    pub fn prepare_operation(&self, id: &str) -> Result<PreparedOperation> {
        let (registration, usage) = self.acquire(id)?;
        Ok(PreparedOperation {
            registration,
            _usage: usage,
        })
    }
}
