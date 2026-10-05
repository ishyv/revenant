//! Native application authoring with one compiled contract and owned capabilities.
//!
//! Start with [`Application`]. Add ordinary async functions using [`operation`]
//! and [`operations!`]; the desktop host supplies bootstrap, resources, and jobs.
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
extern crate self as revenant_sdk;
pub use revenant_core::{
    BatchSummary, BoxFuture, Contract, DecimalI64, DecimalU64, Error, FileEntry, FileMetadata,
    Handle, Outcome, Progress, ResourceCleanup, ResourceLease, ResourcePort, ResourceRegistry,
    Result, Subscription, TaskContext, TaskExecution, TaskManager, TaskSnapshot, TaskState,
    TypeDefinition, WireSafe, type_definition, validate_contract_schema, validate_json,
};
pub use revenant_macros::{Contract, contract, operation, operations};
pub use {schemars, serde, serde_json, ts_rs};

mod application;
/// Standard native metadata and checksum operations, installed by the desktop host.
pub mod builtins;
/// Advanced operation contracts, registration, and pinned dispatch interfaces.
pub mod operations;
/// Native capability configuration and transactional replacement interfaces.
pub mod providers;
pub use application::{Application, CapabilityDefinition, OperationFactory};
pub use operations::{
    ContractManifest, OperationDescriptor, OperationExecutor, OperationFuture,
    OperationRegistration, OperationRegistry, OperationSource, PreparedOperation, Registry,
};
pub use providers::{Provider, ProviderContext, ProviderDescriptor, ProviderSnapshot};
