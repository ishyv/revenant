//! Compiled descriptor metadata and schema admission. Manifest version 3 uses bridge protocol 2; operation sorting and digesting belong to the registry.
use super::OperationRegistration;
use crate::{Error, Result, TypeDefinition, validate_json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// Compiled operation identity, documentation, and boundary contracts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationDescriptor {
    /// Stable dotted capability identifier used by generated bindings.
    pub id: String,
    /// Markdown documentation copied from the source function's Rustdoc.
    pub description: String,
    /// Serialized input shape and TypeScript declarations.
    pub input: TypeDefinition,
    /// Serialized result shape and TypeScript declarations.
    pub output: TypeDefinition,
    /// Original Rust definition, when authored through the operation macro.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<OperationSource>,
}
/// One compiled application contract shared by generation and native startup.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractManifest {
    /// Project contract generation version; desktop definitions use version 3.
    pub version: u32,
    /// Native bridge protocol version, checked before operation dispatch.
    pub protocol: u32,
    /// SHA-256 identity of the sorted operation descriptors and source documentation.
    pub digest: String,
    /// Deterministically ordered operation contracts available to this application.
    pub operations: Vec<OperationDescriptor>,
}

pub(crate) fn validate_registration(registration: &OperationRegistration) -> Result<()> {
    let descriptor = &registration.descriptor;
    if descriptor.id.is_empty() {
        return Err(Error::new("invalid_operation_id", "Operation id is empty"));
    }
    for definition in [&descriptor.input, &descriptor.output] {
        crate::validate_contract_schema(&definition.schema)?;
        json_schema_valid(&definition.schema)?;
        if definition.typescript.is_empty() {
            return Err(Error::new(
                "invalid_contract",
                "TypeScript definition is empty",
            ));
        }
    }
    Ok(())
}
fn json_schema_valid(schema: &Value) -> Result<()> {
    // Validate against the schema's declared dialect via the shared validator.
    // An arbitrary value may fail validation; only invalid_schema is a setup error.
    match validate_json(schema, &Value::Null) {
        Err(error) if error.code == "invalid_schema" => Err(error),
        _ => Ok(()),
    }
}
