//! Admission validates complete handles; captured leases own resources beyond registry release. Host cleanup runs outside registry locks.
use crate::{BoxFuture, Error, FileEntry, FileMetadata, Handle, Result, lock};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

mod lease;
mod registry;
mod scopes;
use lease::Resource;
pub use lease::ResourceLease;
use registry::Resources;
pub use registry::{ChunkReader, ResourceCleanup, ResourcePort, ResourceRegistry};
use scopes::Scope;
