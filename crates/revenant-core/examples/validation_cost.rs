//! Isolates schema preparation from validation. This is not an IPC benchmark.
//! Run: cargo run -p revenant-core --example validation_cost --release
use serde_json::json;
use std::{hint::black_box, time::Instant};

fn main() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "additionalProperties": false,
        "required": ["revision", "summary", "sceneRevision"],
        "properties": {
            "revision": {"type": "string", "pattern": "^(0|[1-9][0-9]*)$"},
            "sceneRevision": {"type": "string", "pattern": "^(0|[1-9][0-9]*)$"},
            "summary": {"type": "object", "additionalProperties": false,
                "required": ["generation", "discovered", "status"],
                "properties": {
                    "generation": {"type": "string", "pattern": "^(0|[1-9][0-9]*)$"},
                    "discovered": {"type": "string", "pattern": "^(0|[1-9][0-9]*)$"},
                    "status": {"type": "string", "enum": ["scanning", "complete", "cancelled", "partial"]}
                }
            }
        }
    });
    let value = json!({"revision": "7", "sceneRevision": "8",
        "summary": {"generation": "3", "discovered": "500000", "status": "scanning"}});
    let synthetic = measure(schema, value, "synthetic small typed-update-shaped schema");
    let entry = revenant_core::FileEntry {
        handle: revenant_core::Handle {
            id: "entry-1".into(),
            scope: "scope-fixture".into(),
            generation: 1,
            kind: "file".into(),
        },
        name: "fixture.bin".into(),
        relative_path: "fixtures/fixture.bin".into(),
        size: u64::MAX.into(),
        mime: "application/octet-stream".into(),
        modified: Some(i64::MIN.into()),
    };
    let actual = measure(
        <revenant_core::FileEntry as revenant_core::Contract>::definition()
            .unwrap()
            .schema,
        serde_json::to_value(entry).unwrap(),
        "actual published framework FileEntry contract",
    );
    println!(
        "{}",
        json!({"scope":"native validation only; no IPC or Strata speedup claim",
        "packageVersion":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "debugAssertions":cfg!(debug_assertions),"workloads":[synthetic,actual]})
    );
}
fn measure(
    schema: serde_json::Value,
    value: serde_json::Value,
    workload: &str,
) -> serde_json::Value {
    let iterations = 10_000u32;
    revenant_core::validate_json(&schema, &value).unwrap();
    let cached = revenant_core::PreparedValidator::new(schema.clone()).unwrap();
    assert!(cached.validate(&json!({"revision": 7})).is_err());
    let mut rounds = Vec::new();
    for round in 0..5 {
        let run = |prepare_each_time| {
            let start = Instant::now();
            for _ in 0..iterations {
                if prepare_each_time {
                    revenant_core::validate_json(black_box(&schema), black_box(&value)).unwrap();
                } else {
                    cached.validate(black_box(&value)).unwrap();
                }
            }
            start.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(iterations)
        };
        let (current, prepared) = if round % 2 == 0 {
            (run(true), run(false))
        } else {
            let prepared = run(false);
            (run(true), prepared)
        };
        rounds.push(json!({"round": round, "current_us_per_value": current,
            "prepared_us_per_value": prepared, "ratio": current / prepared}));
    }
    json!({"workload": workload, "iterations_per_round": iterations,
        "schema_bytes": serde_json::to_vec(&schema).unwrap().len(),
        "value_bytes": serde_json::to_vec(&value).unwrap().len(), "rounds": rounds})
}
