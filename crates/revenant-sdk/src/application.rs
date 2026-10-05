//! Application composition without host bootstrap or transport concerns.
//!
//! Definitions are inert: constructing an application does not open windows,
//! acquire files, or prepare providers. The desktop launcher validates the same
//! definitions used by the binding generator before acquiring native resources.

use crate::{ContractManifest, OperationRegistration, Provider, Registry, Result};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

/// Deferred metadata and executor construction emitted by [`crate::operations!`].
///
/// Factories run during validation, so errors have one reporting path and ordinary
/// application composition remains infallible.
#[doc(hidden)]
pub type OperationFactory = fn() -> Result<OperationRegistration>;

/// A native capability and its initial, schema-validated configuration.
pub struct CapabilityDefinition {
    /// Compiled implementation prepared only when the application starts.
    pub provider: Arc<dyn Provider>,
    /// Configuration checked against the provider's declared schema.
    pub config: Value,
}

/// Describes an application's custom operations and native capabilities.
///
/// Built-in files, media, and settings are supplied by the desktop host. Apps that
/// use only these capabilities need no Rust module. Custom apps expose `pub fn
/// app() -> Application`; Revenant supplies the executable and lifecycle.
///
/// ```
/// use revenant_sdk::Application;
/// let app = Application::new();
/// assert!(!app.manifest().unwrap().operations.is_empty());
/// ```
#[derive(Default)]
pub struct Application {
    operations: Vec<OperationFactory>,
    capabilities: Vec<CapabilityDefinition>,
    replacements: BTreeMap<String, Arc<dyn Fn() -> Arc<dyn Provider> + Send + Sync>>,
}

impl Application {
    /// Creates a definition with the standard desktop capabilities.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds operations using their ordinary Rust function paths.
    ///
    /// Pass `revenant::operations![module::function]`. Duplicate identifiers and
    /// incompatible contracts are rejected before the desktop window opens.
    pub fn operations(mut self, operations: Vec<OperationFactory>) -> Self {
        self.operations.extend(operations);
        self
    }

    /// Composes a compiled capability with its initial configuration.
    ///
    /// Preparation is transactional; failed preparation cleans candidate resources
    /// and does not publish a partially active capability.
    pub fn capability(mut self, provider: impl Provider, config: Value) -> Self {
        self.capabilities.push(CapabilityDefinition {
            provider: Arc::new(provider),
            config,
        });
        self
    }

    /// Makes a compiled provider factory available for runtime replacement.
    ///
    /// Replacement still checks contracts, dependencies, and resource ownership.
    /// This exposes alternatives compiled into the app, not arbitrary live Rust.
    pub fn replacement(
        mut self,
        name: impl Into<String>,
        factory: impl Fn() -> Arc<dyn Provider> + Send + Sync + 'static,
    ) -> Self {
        self.replacements.insert(name.into(), Arc::new(factory));
        self
    }

    /// Compiles the authoritative contract without starting the application.
    ///
    /// The CLI uses this result to generate TypeScript types and documentation.
    pub fn manifest(&self) -> Result<ContractManifest> {
        let registry = Registry::new();
        crate::builtins::register(&registry)?;
        registry.register(crate::builtins::checksum_operation()?)?;
        for factory in &self.operations {
            registry.register(factory()?)?;
        }
        for capability in &self.capabilities {
            for operation in capability.provider.operations()? {
                registry.register(operation)?;
            }
        }
        registry.manifest()
    }

    /// Host-only access to the inert composition, after manifest validation.
    #[doc(hidden)]
    pub fn into_parts(
        self,
    ) -> (
        Vec<OperationFactory>,
        Vec<CapabilityDefinition>,
        BTreeMap<String, Arc<dyn Fn() -> Arc<dyn Provider> + Send + Sync>>,
    ) {
        (self.operations, self.capabilities, self.replacements)
    }
}
