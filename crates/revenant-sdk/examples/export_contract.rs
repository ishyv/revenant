//! Programmatic export tool, independent of Cargo's test harness.
use revenant_sdk::{
    Contract, DecimalU64, Registry, Result, TaskContext, builtins, contract, operation,
};

#[derive(Contract)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    title: String,
    status: Status,
    amount: DecimalU64,
    coordinates: (i32, i32),
}
#[derive(Contract)]
#[serde(tag = "state", content = "data", rename_all = "camelCase")]
enum Status {
    Pending,
    Complete { label: String },
    Failed(String),
}
#[derive(Contract)]
struct Envelope<T> {
    items: Vec<T>,
}
#[contract]
struct Caption(String);
#[derive(Contract)]
struct Empty;

#[operation(id = "records.describe")]
async fn describe(input: Envelope<Record>, ctx: TaskContext) -> Result<Vec<Caption>> {
    ctx.checkpoint()?;
    Ok(input
        .items
        .into_iter()
        .map(|record| Caption(record.title))
        .collect())
}
#[operation(id = "records.empty")]
async fn empty(_input: Empty, ctx: TaskContext) -> Result<Empty> {
    ctx.checkpoint()?;
    Ok(Empty)
}

fn main() -> Result<()> {
    let registry = Registry::new();
    builtins::register(&registry)?;
    registry.register(builtins::checksum_operation()?)?;
    registry.register(describe_operation()?)?;
    registry.register(empty_operation()?)?;
    println!("{}", registry.manifest_json()?);
    Ok(())
}
