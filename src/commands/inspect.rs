use crate::inspection::{ProjectInspector, backend::DevBackend};
use anyhow::Result;
use revenant_agent::Backend;
use serde_json::json;
use std::path::Path;

pub fn run(project: Option<&Path>, json_output: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let inspector = ProjectInspector::open(project.unwrap_or(&cwd))?;
    let backend = DevBackend::new(inspector);
    let runtime = tokio::runtime::Runtime::new()?;
    let snapshot = runtime.block_on(async {
        let result = backend.call("project.inspect", json!({})).await;
        let cleanup = backend.close().await;
        result.and_then(|value| cleanup.map(|_| value))
    })?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
    } else {
        println!(
            "{} ({})",
            snapshot["project"]["name"].as_str().unwrap_or("Revenant"),
            snapshot["project"]["root"].as_str().unwrap_or("")
        );
        println!(
            "Source / facade: {}",
            snapshot["consistency"]["sourceToFacade"]
                .as_str()
                .unwrap_or("unknown")
        );
        println!(
            "Contract: {}",
            snapshot["facade"]["digest"]
                .as_str()
                .unwrap_or("not compiled")
        );
        println!(
            "Latest build: {}",
            snapshot["buildAttempt"]["status"]
                .as_str()
                .unwrap_or("not recorded")
        );
        println!(
            "Development runtime: {}",
            snapshot["runtime"]["status"]
                .as_str()
                .unwrap_or("unavailable")
        );
        println!(
            "Operations: {}",
            snapshot["operations"].as_array().map_or(0, Vec::len)
        );
    }
    Ok(())
}
