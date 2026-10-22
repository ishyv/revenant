//! CPU-only measurement of the same frozen workloads used by optional native GPU acceptance.
use revenant_core::{ResourceRegistry, TaskManager, compute::fixtures};
use serde_json::json;
use std::time::Instant;

fn main() -> revenant_core::Result<()> {
    let start = Instant::now();
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None)?;
    let task = TaskManager::new(resources.clone()).create(&scope, "cpu-compute-experiment")?;
    let token = task.cancellation();
    let kernel = fixtures::resident_kernel()?;
    let input = fixtures::resident_input()?;
    let preparation_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let mut cpu = input.clone();
    for _ in 0..fixtures::RESIDENT_PASSES {
        cpu = kernel.execute_cpu(&cpu, &token)?;
    }
    let cpu_ir_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let reference = fixtures::composed_resident_cpu(&input)?;
    let cpu_composed_ms = start.elapsed().as_secs_f64() * 1000.0;
    if !cpu.conforms(&reference) {
        return Err(revenant_core::Error::new(
            "compute_conformance",
            "Resident CPU reference differs",
        ));
    }
    let start = Instant::now();
    let transfer_kernel = fixtures::transfer_kernel()?;
    let transfer_input = fixtures::transfer_input()?;
    let transfer_preparation_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let transfer = transfer_kernel.execute_cpu(&transfer_input, &token)?;
    let transfer_cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
    if !transfer
        .as_u32()
        .unwrap()
        .iter()
        .zip(transfer_input.as_u32().unwrap())
        .all(|(actual, input)| *actual == input.wrapping_add(7))
    {
        return Err(revenant_core::Error::new(
            "compute_conformance",
            "Transfer CPU reference differs",
        ));
    }
    resources.dispose_scope(&scope)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": 1, "fixture_source_digest": fixtures::source_digest(),
            "package_version": env!("CARGO_PKG_VERSION"), "target_arch": std::env::consts::ARCH,
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "resident": { "elements": input.len(), "instructions": 32, "passes": fixtures::RESIDENT_PASSES,
                "preparation_ms": preparation_ms, "cpu_ir_ms": cpu_ir_ms, "cpu_composed_reference_ms": cpu_composed_ms, "correct": true },
            "transfer_heavy": { "elements": transfer_input.len(), "instructions": 1,
                "preparation_ms": transfer_preparation_ms, "cpu_ms": transfer_cpu_ms, "correct": true },
            "decision": "CPU-only control; GPU promotion requires separate complete-cost adapter evidence",
            "limitations": ["Single-run CPU wall times", "No GPU dependencies or simulated GPU timings", "No stable automatic backend selector"]
        }))?
    );
    Ok(())
}
