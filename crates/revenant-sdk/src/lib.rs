//! Native application authoring with one compiled contract and owned capabilities.
//!
//! Start with [`app!`] and ordinary functions using [`operation`]; the desktop host
//! supplies bootstrap, resources, and jobs. [`Application`] supports explicit composition.
//!
//! ```
//! revenant_sdk::app!();
//! #[revenant_sdk::operation(id = "titles.normalize", read_only, independent, agent)]
//! fn normalize(text: String) -> String { text.trim().to_owned() }
//! let manifest = app().manifest().unwrap();
//! let operation = manifest.operations.iter().find(|op| op.id == "titles.normalize").unwrap();
//! assert_eq!(operation.arguments, ["text"]);
//! assert_eq!(operation.input.schema["properties"]["text"]["type"], "string");
//! ```
//!
//! Arguments become a camelCase object, including a single domain value: a function
//! taking `input: Record` receives `{ input: Record }`. `Option<T>` arguments use
//! omission; boundary validation rejects explicit null. An optional final
//! [`TaskContext`] is supplied by the host and omitted from the wire contract.
//! `legacy_input` explicitly retains a single argument's previous wire shape.
//! Effects, independent batching, and agent exposure are opt-in declarations.
//!
//! Borrowed inputs cannot cross the owned execution boundary:
//! ```compile_fail
//! #[revenant_sdk::operation]
//! fn borrowed(text: &str) -> String { text.to_owned() }
//! ```
//! Large integer inputs require the lossless decimal wrappers:
//! ```compile_fail
//! #[revenant_sdk::operation]
//! fn lossy(bytes: u64) -> revenant_sdk::DecimalU64 { bytes.into() }
//! ```
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
extern crate self as revenant_sdk;
#[doc(hidden)]
pub use linkme;
pub use revenant_core::WireProjection;
pub use revenant_core::{
    ApplicationPaths, BatchSummary, BoxFuture, CancellationToken, Contract, DecimalI64, DecimalU64,
    Error, FileEntry, FileMetadata, FolderHandle, Handle, LatestJobs, LatestSubmission,
    MAX_TASK_UPDATE_BYTES, NativeFolder, NativeLease, NativeResourceLease, Outcome,
    PreparedValidator, Progress, Resource, ResourceCleanup, ResourceLease, ResourcePort,
    ResourceRegistry, Result, Subscription, TaskContext, TaskExecution, TaskManager, TaskServices,
    TaskSnapshot, TaskState, TaskUpdate, TypeDefinition, WireSafe, Work, WorkClass, WorkJob,
    WorkLimits, WorkService, WorkStats, type_definition, validate_contract_schema, validate_json,
};
pub use revenant_macros::{Contract, WireProjection, app, contract, operation, operations};
pub use {schemars, serde, serde_json, ts_rs};

mod application;
/// Standard native metadata and checksum operations, installed by the desktop host.
pub mod builtins;
/// Opt-in typed numerical authoring and bounded CPU execution helpers.
#[cfg(feature = "experimental-compute")]
pub mod compute;
/// Advanced operation contracts, registration, and pinned dispatch interfaces.
pub mod operations;
/// Native capability configuration and transactional replacement interfaces.
pub mod providers;
pub use application::{
    Application, CapabilityDefinition, OPERATION_DISCOVERY, OperationDiscovery, OperationFactory,
    inferred_operation_id,
};
pub use operations::{
    ContractManifest, OperationDescriptor, OperationEffect, OperationExecutor, OperationFuture,
    OperationRegistration, OperationRegistry, OperationSource, PreparedOperation, Registry,
};
pub use providers::{Provider, ProviderContext, ProviderDescriptor, ProviderSnapshot};
