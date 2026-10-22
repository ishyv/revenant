//! Compiled descriptor metadata and schema admission. Manifest version 6 uses bridge protocol 5; operation sorting and digesting belong to the registry.
use super::OperationRegistration;
use crate::{Error, PreparedValidator, Result, TypeDefinition, WorkClass};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
/// Source definition carried into generated API hover documentation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationSource {
    /// Rust source path relative to the compiling package.
    pub file: String,
    /// One-based source line of the operation attribute.
    pub line: u32,
    /// Original Rust module path, including its crate.
    pub module: String,
}

/// Author-declared application effects, separate from scheduling and agent exposure.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "camelCase")]
pub enum OperationEffect {
    /// No effect declaration has been made; consumers must not assume purity.
    #[default]
    Unknown,
    /// The operation reads application state without intentionally changing it.
    ReadOnly,
    /// The operation intentionally changes application or external state.
    Write,
}

/// Compiled operation identity, documentation, and boundary contracts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationDescriptor {
    /// Stable dotted capability identifier used by generated bindings.
    pub id: String,
    /// Markdown documentation copied from the source function's Rustdoc.
    pub description: String,
    /// Named wire arguments in source order; empty for explicit legacy input shapes.
    pub arguments: Vec<String>,
    /// Application effects declared by the operation author.
    pub effect: OperationEffect,
    /// Host scheduling hint, independent of effects and resource authority.
    /// Interactive work must be explicitly declared read-only.
    #[serde(default)]
    pub work_class: WorkClass,
    /// Author declaration that separate batch items can execute concurrently.
    pub independent: bool,
    /// Whether the author explicitly exposes this operation to application agents.
    pub agent: bool,
    /// Serialized input shape and TypeScript declarations.
    pub input: TypeDefinition,
    /// Serialized result shape and TypeScript declarations.
    pub output: TypeDefinition,
    /// Optional compiled contract for bounded, replaceable application task updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updates: Option<TypeDefinition>,
    /// Original Rust definition, when authored through the operation macro.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<OperationSource>,
}
/// One compiled application contract shared by generation and native startup.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractManifest {
    /// Compiled contract generation version; current definitions use version 6.
    pub version: u32,
    /// Native bridge protocol version, checked before operation dispatch.
    pub protocol: u32,
    /// SHA-256 identity of the sorted operation descriptors and source documentation.
    pub digest: String,
    /// Deterministically ordered operation contracts available to this application.
    pub operations: Vec<OperationDescriptor>,
}

pub(crate) struct ContractValidators {
    pub input: Arc<PreparedValidator>,
    pub output: Arc<PreparedValidator>,
    pub updates: Option<Arc<PreparedValidator>>,
}

#[cfg(test)]
thread_local! {
    pub(crate) static PREPARATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn validate_registration(
    registration: &OperationRegistration,
) -> Result<Arc<ContractValidators>> {
    #[cfg(test)]
    PREPARATION_COUNT.with(|count| count.set(count.get() + 1));
    let descriptor = &registration.descriptor;
    if descriptor.id.is_empty() {
        return Err(Error::new("invalid_operation_id", "Operation id is empty"));
    }
    if descriptor.work_class == WorkClass::Interactive
        && descriptor.effect != OperationEffect::ReadOnly
    {
        return Err(Error::new(
            "invalid_work_class",
            "Interactive scheduling requires a read-only operation",
        ));
    }
    for definition in [&descriptor.input, &descriptor.output]
        .into_iter()
        .chain(descriptor.updates.iter())
    {
        if definition.typescript.is_empty() {
            return Err(Error::new(
                "invalid_contract",
                "TypeScript definition is empty",
            ));
        }
    }
    Ok(Arc::new(ContractValidators {
        input: Arc::new(PreparedValidator::new(descriptor.input.schema.clone())?),
        output: Arc::new(PreparedValidator::new(descriptor.output.schema.clone())?),
        updates: descriptor
            .updates
            .as_ref()
            .map(|definition| PreparedValidator::new(definition.schema.clone()).map(Arc::new))
            .transpose()?,
    }))
}
