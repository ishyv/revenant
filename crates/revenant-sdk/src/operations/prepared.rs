//! An admitted operation pins its provider and dependency generations. Hosts own task execution guards and can reuse one operation guard across short-lived batch children.
use super::{ContractValidators, OperationDescriptor, OperationRegistration, OperationRegistry};
use crate::providers::ProviderUse;
use crate::{Result, TaskContext};
use serde_json::Value;
use std::sync::Arc;
/// A pinned compiled operation admitted by the native executor.
///
/// This guard keeps provider generations stable while queued or executing.
/// Large batches reuse it and give each item a short-lived task context.
pub struct PreparedOperation {
    registration: OperationRegistration,
    validators: Arc<ContractValidators>,
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
        context.configure_work_class(self.registration.descriptor.work_class)?;
        if let Some(updates) = &self.validators.updates {
            context.configure_prepared_updates(updates.clone())?;
        }
        self.validators.input.validate(input)?;
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
        OperationRegistry::execute(&self.registration, &self.validators, input, context).await
    }
}

impl OperationRegistry {
    /// Pins an operation and its dependency generations before queue admission.
    ///
    /// Keep the returned guard until execution ends; replacement rejects busy
    /// providers, including their transitive dependencies.
    pub fn prepare_operation(&self, id: &str) -> Result<PreparedOperation> {
        let (registration, validators, usage) = self.acquire(id)?;
        Ok(PreparedOperation {
            registration,
            validators,
            _usage: usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationRegistration, ResourceRegistry, TaskManager};
    use serde_json::json;

    #[test]
    fn installed_generation_prepares_once_and_reclaims_after_pinned_work_finishes() {
        super::super::contracts::PREPARATION_COUNT.with(|count| count.set(0));
        let registry = OperationRegistry::new();
        let mut registration =
            OperationRegistration::new::<u32, u32, _, _>("count", "count", |input, _| async move {
                Ok(input)
            })
            .unwrap()
            .with_updates::<bool>()
            .unwrap();
        registration.descriptor.input.schema = json!({"type":"integer","minimum":0,"maximum":5});
        registry.register(registration).unwrap();
        let original = registry.prepare_operation("count").unwrap();
        let input = Arc::downgrade(&original.validators.input);
        let updates = Arc::downgrade(original.validators.updates.as_ref().unwrap());
        for _ in 0..100 {
            let next = registry.prepare_operation("count").unwrap();
            assert!(Arc::ptr_eq(&original.validators, &next.validators));
            next.validators.input.validate(&json!(3)).unwrap();
        }
        super::super::contracts::PREPARATION_COUNT.with(|count| assert_eq!(count.get(), 1));
        super::super::registry::locked(&registry.inner)
            .operations
            .remove("count");
        let mut replacement =
            OperationRegistration::new::<u32, u32, _, _>("count", "new", |input, _| async move {
                Ok(input)
            })
            .unwrap();
        replacement.descriptor.input.schema = json!({"type":"integer","minimum":0,"maximum":10});
        registry.register(replacement).unwrap();
        let current = registry.prepare_operation("count").unwrap();
        assert!(!Arc::ptr_eq(&original.validators, &current.validators));
        assert!(original.validators.input.validate(&json!(8)).is_err());
        current.validators.input.validate(&json!(8)).unwrap();
        let resources = ResourceRegistry::new();
        let scope = resources.create_scope(None).unwrap();
        let context = TaskManager::new(resources)
            .create(&scope, "update")
            .unwrap();
        original.prepare_input(&json!(3), &context).unwrap();
        let execution = context.begin().unwrap();
        drop(original);
        assert!(input.upgrade().is_none());
        assert!(updates.upgrade().is_some());
        context.publish(true).unwrap();
        execution.finish(Ok(json!(3))).unwrap();
        assert!(updates.upgrade().is_none());
    }
}
