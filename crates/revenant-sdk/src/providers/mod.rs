//! Native provider lifecycle, configuration contracts, and atomic replacement. Provider callbacks never run under the registry mutex; generation guards prevent replacing work still in use.
mod config;
mod lifecycle;
mod transaction;

pub use config::{ProviderDescriptor, ProviderSnapshot};
pub use lifecycle::{Provider, ProviderContext};
pub(crate) use lifecycle::{ProviderRecord, ProviderUse};
