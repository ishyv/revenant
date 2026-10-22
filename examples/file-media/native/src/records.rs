//! Ordinary records share the same typed operation model as file capabilities.
use revenant::{contract, operation};

/// A small application record. Identity and completion survive normalization.
#[contract]
pub struct Record {
    /// Stable application identity, distinct from native file handles.
    pub id: String,
    /// Human-readable title; normalized by trimming and uppercasing.
    pub title: String,
    /// Whether the record is complete.
    pub done: bool,
}

/// Trim and uppercase a record title while preserving its identity and status.
///
/// This ordinary function is discovered automatically by `revenant::app!`. File
/// checksums use the host's built-in `files.checksum` registration instead.
#[operation(id = "records.normalize", read_only, independent)]
pub fn normalize(record: Record) -> Record {
    Record {
        title: record.title.trim().to_uppercase(),
        ..record
    }
}
