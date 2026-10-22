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

/// Linker registration identity emitted by the operation attribute.
#[doc(hidden)]
pub struct OperationDiscovery {
    /// Origin package; dependencies require explicit composition.
    pub package: &'static str,
    /// Origin module path, whose first segment identifies the actual compiled crate.
    pub module: &'static str,
    /// Stable source ordering key independent of linker section ordering.
    pub source: &'static str,
    /// Deferred executor and contract construction.
    pub factory: OperationFactory,
}

/// Distributed operation registrations, filtered by the calling application's origin.
#[doc(hidden)]
#[linkme::distributed_slice]
pub static OPERATION_DISCOVERY: [OperationDiscovery];

/// Builds a default dotted identifier from the module path and camelCase function name.
#[doc(hidden)]
pub fn inferred_operation_id(module: &str, function: &str) -> String {
    let mut segments = module
        .split("::")
        .skip(1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut name = String::new();
    let mut upper = false;
    for character in function.trim_start_matches("r#").chars() {
        if character == '_' {
            upper = !name.is_empty();
        } else if upper {
            name.extend(character.to_uppercase());
            upper = false;
        } else {
            name.push(character);
        }
    }
    segments.push(name);
    segments.join(".")
}

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

    /// Adds annotated functions from the calling application crate.
    ///
    /// Used by [`crate::app!`]. Dependency operations remain explicit; a package's
    /// separate binary or test crate cannot accidentally discover its library operations.
    #[doc(hidden)]
    pub fn discover(mut self, package: &str, module: &str) -> Self {
        let crate_name = module.split("::").next();
        let mut operations: Vec<_> = OPERATION_DISCOVERY
            .iter()
            .filter(|operation| {
                operation.package == package && operation.module.split("::").next() == crate_name
            })
            .collect();
        operations.sort_by_key(|operation| operation.source);
        self.operations
            .extend(operations.into_iter().map(|operation| operation.factory));
        self
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
    pub fn into_parts(self) -> ApplicationParts {
        (self.operations, self.capabilities, self.replacements)
    }
}

/// Inert composition consumed by the host after contract validation.
#[doc(hidden)]
pub type ApplicationParts = (
    Vec<OperationFactory>,
    Vec<CapabilityDefinition>,
    BTreeMap<String, Arc<dyn Fn() -> Arc<dyn Provider> + Send + Sync>>,
);
