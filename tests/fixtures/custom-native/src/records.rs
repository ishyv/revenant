use revenant::{contract, operation};

/// A documented record crossing the native application boundary.
#[contract]
pub struct Record {
    /// Stable record identity.
    pub id: String,
    /// Human-readable title to normalize.
    pub title: String,
}

/// Normalize a title for the portable CLI proof.
#[operation(id = "records.normalize", read_only, independent)]
pub fn normalize(record: Record) -> Record {
    Record {
        title: record.title.trim().to_uppercase(),
        ..record
    }
}
