use revenant::{Result, TaskContext, contract, operation};

/// A documented record crossing the native application boundary.
#[contract]
pub struct Record {
    /// Stable record identity.
    pub id: String,
    /// Human-readable title to normalize.
    pub title: String,
}

/// Normalize a title for the portable CLI proof.
#[operation(id = "records.normalize")]
pub async fn normalize(input: Record, context: TaskContext) -> Result<Record> {
    context.checkpoint()?;
    Ok(Record {
        title: input.title.trim().to_uppercase(),
        ..input
    })
}
