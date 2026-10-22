use crate::inspection::ProjectInspector;
use anyhow::{Result, ensure};
use std::path::Path;

pub fn run(project: Option<&Path>, json_output: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let inspector = ProjectInspector::open(project.unwrap_or(&cwd))?;
    let report = inspector.check("all")?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Checks {}",
            if report["succeeded"] == true {
                "passed"
            } else {
                "failed"
            }
        );
        for diagnostic in report["diagnostics"].as_array().into_iter().flatten() {
            println!(
                "{} [{}]: {}",
                diagnostic["stage"].as_str().unwrap_or("check"),
                diagnostic["code"].as_str().unwrap_or("diagnostic"),
                diagnostic["message"].as_str().unwrap_or("")
            );
        }
    }
    ensure!(
        report["succeeded"] == true,
        "project checks failed; inspect structured diagnostics above"
    );
    Ok(())
}
