//! Advanced compiled operation APIs. Metadata, typed registration, registry ownership, and generation-pinned execution have separate responsibilities.
mod contracts;
mod prepared;
mod registration;
mod registry;

pub(crate) use contracts::validate_registration;
pub use contracts::{ContractManifest, OperationDescriptor, OperationSource};
pub use prepared::PreparedOperation;
pub use registration::{OperationExecutor, OperationFuture, OperationRegistration};
pub use registry::{OperationRegistry, Registry};
pub(crate) use registry::{RegisteredOperation, RegistryData, locked};
