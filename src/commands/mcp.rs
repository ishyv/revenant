use crate::inspection::{ProjectInspector, backend::DevBackend};
use anyhow::Result;
use std::{path::Path, sync::Arc};

pub fn run(project: &Path) -> Result<()> {
    let inspector = ProjectInspector::open(project)?;
    tokio::runtime::Runtime::new()?.block_on(revenant_agent::serve_stdio(Arc::new(
        DevBackend::new(inspector),
    )))
}
