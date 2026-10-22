//! Explicit release-only cost measurement using generated framework wire contracts.
use revenant_core::{Contract, FileEntry, Handle, PreparedValidator, validate_json};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    hint::black_box,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const WARMUPS: usize = 2;
const REPEATS: usize = 5;
const ITERATIONS: usize = 1_000;

fn entry(index: u32) -> FileEntry {
    FileEntry {
        handle: Handle {
            id: format!("fixture-{index}"),
            scope: "scope-validation-fixture".into(),
            generation: 1,
            kind: "file".into(),
        },
        name: format!("fixture-{index}.bin"),
        relative_path: format!("group-{}/fixture-{index}.bin", index % 8),
        size: u64::MAX.into(),
        mime: "application/octet-stream".into(),
        modified: if index.is_multiple_of(2) {
            Some(i64::MIN.into())
        } else {
            None
        },
    }
}

fn timed(mut operation: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        operation();
    }
    start.elapsed().as_secs_f64() * 1_000_000.0 / ITERATIONS as f64
}

fn native_roundtrip<T: Serialize + DeserializeOwned>(
    model: &T,
    schema: &Value,
    prepared: Option<&PreparedValidator>,
) {
    let value = serde_json::to_value(black_box(model)).unwrap();
    if let Some(prepared) = prepared {
        prepared.validate(black_box(&value)).unwrap();
    } else {
        validate_json(black_box(schema), black_box(&value)).unwrap();
    }
    let bytes = serde_json::to_vec(&value).unwrap();
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    let decoded: T = serde_json::from_value(parsed).unwrap();
    black_box(decoded);
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * percentile).ceil() as usize).saturating_sub(1)]
}

fn measure<T: Contract>(model: T, description: &str) -> Value {
    let definition = T::definition().unwrap();
    let value = serde_json::to_value(&model).unwrap();
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut rounds = Vec::new();
    for round in 0..WARMUPS + REPEATS {
        let start = Instant::now();
        let prepared = PreparedValidator::new(definition.schema.clone()).unwrap();
        let preparation_us = start.elapsed().as_secs_f64() * 1_000_000.0;
        let mut prepared_calls = 0usize;
        let mut ad_hoc_calls = 0usize;
        let mut ad_hoc_us = 0.0;
        let mut prepared_us = 0.0;
        // The one immutable validator is reused throughout both timed stages.
        for use_prepared in if round.is_multiple_of(2) {
            [false, true]
        } else {
            [true, false]
        } {
            let time = timed(|| {
                if use_prepared {
                    prepared.validate(black_box(&value)).unwrap();
                    prepared_calls += 1;
                } else {
                    validate_json(black_box(&definition.schema), black_box(&value)).unwrap();
                    ad_hoc_calls += 1;
                }
            });
            if use_prepared {
                prepared_us = time;
            } else {
                ad_hoc_us = time;
            }
        }
        let to_value_us = timed(|| {
            black_box(serde_json::to_value(black_box(&model)).unwrap());
        });
        let encode_us = timed(|| {
            black_box(serde_json::to_vec(black_box(&value)).unwrap());
        });
        let parse_us = timed(|| {
            black_box(serde_json::from_slice::<Value>(black_box(&bytes)).unwrap());
        });
        let typed_decode_with_clone_us = timed(|| {
            black_box(serde_json::from_value::<T>(black_box(&value).clone()).unwrap());
        });
        let mut ad_hoc_roundtrip_us = 0.0;
        let mut prepared_roundtrip_us = 0.0;
        for use_prepared in if round.is_multiple_of(2) {
            [false, true]
        } else {
            [true, false]
        } {
            let time = timed(|| {
                native_roundtrip(
                    &model,
                    &definition.schema,
                    use_prepared.then_some(&prepared),
                );
                if use_prepared {
                    prepared_calls += 1;
                } else {
                    ad_hoc_calls += 1;
                }
            });
            if use_prepared {
                prepared_roundtrip_us = time;
            } else {
                ad_hoc_roundtrip_us = time;
            }
        }
        assert_eq!(prepared_calls, ITERATIONS * 2);
        assert_eq!(ad_hoc_calls, ITERATIONS * 2);
        // Discarding the prepared object at the end of this generation bounds its
        // lifetime. Schema compilation is outside all prepared validation loops.
        rounds.push(json!({"round":round,"warmup":round<WARMUPS,
            "preparationMicroseconds":preparation_us,"preparedConstructorCalls":1,
            "preparedValidateCalls":prepared_calls,"adHocValidateJsonCalls":ad_hoc_calls,
            "adHocSchemaCompilerEntryCallsBySource":ad_hoc_calls,
            "preparedSchemaCompilerEntryCallsBySource":1,
            "validationMicrosecondsPerValue":{"adHoc":ad_hoc_us,"prepared":prepared_us},
            "serializationMicrosecondsPerValue":{"typedToJsonValue":to_value_us,"jsonEncode":encode_us,
                "jsonParse":parse_us,"typedDecodeIncludingValueClone":typed_decode_with_clone_us},
            "nativeRoundtripMicrosecondsPerValue":{"adHoc":ad_hoc_roundtrip_us,"prepared":prepared_roundtrip_us},
            "estimatedValidationContributionPercent":{"adHoc":100.0*ad_hoc_us/ad_hoc_roundtrip_us,
                "prepared":100.0*prepared_us/prepared_roundtrip_us},
            "amortizedPreparationMicrosecondsPerPreparedCall":preparation_us/prepared_calls as f64}));
    }
    let metrics = rounds
        .iter()
        .skip(WARMUPS)
        .map(|round| {
            round["nativeRoundtripMicrosecondsPerValue"]["prepared"]
                .as_f64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    json!({"contract":description,"typeDefinitionName":definition.name,"schemaBytes":serde_json::to_vec(&definition.schema).unwrap().len(),
        "valueBytes":bytes.len(),"schemaSha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&definition.schema).unwrap())),
        "valueSha256":format!("{:x}",Sha256::digest(&bytes)),"iterationsPerStage":ITERATIONS,
        "generationsIncludingWarmups":WARMUPS+REPEATS,"preparedConstructionsPerGeneration":1,
        "preparedRoundtripSummary":{"p50Microseconds":percentile(&metrics,0.5),"p95Microseconds":percentile(&metrics,0.95)},
        "rounds":rounds})
}

#[test]
#[ignore = "explicit release measurement; writes target/prepared-validation-benchmark.json"]
fn generated_contract_preparation_validation_and_native_roundtrip_costs() {
    if cfg!(debug_assertions) {
        panic!("run this measurement with --release");
    }
    let small = measure(
        entry(0),
        "actual framework FileEntry contract, one complete record",
    );
    println!("measured FileEntry");
    let large = measure(
        (0..512).map(entry).collect::<Vec<_>>(),
        "generated Vec<FileEntry> contract, 512 complete records",
    );
    println!("measured Vec<FileEntry> with 512 records");
    let executable = std::env::current_exe().unwrap();
    let compiler = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .unwrap();
    let report = json!({"schemaVersion":1,
        "scope":"native validator and typed serialization roundtrip only; no operation, IPC or application speedup claim",
        "measuredAtUnixSeconds":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        "build":{"packageVersion":env!("CARGO_PKG_VERSION"),"release":true,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
            "executable":executable,"executableSha256":format!("{:x}",Sha256::digest(std::fs::read(&executable).unwrap())),
            "rustc":String::from_utf8(compiler.stdout).unwrap().trim(),"manifest":revenant_core::CONTRACT_MANIFEST_VERSION,
            "protocol":revenant_core::BRIDGE_PROTOCOL_VERSION,"sdkRevision":revenant_core::SDK_COMPATIBILITY_REVISION,
            "jsonschemaVersion":"0.33.0; Cargo.lock hash records resolved dependencies"},
        "compiledSourceSha256":{
            "crates/revenant-core/tests/prepared_validation_benchmark.rs":format!("{:x}",Sha256::digest(include_bytes!("prepared_validation_benchmark.rs"))),
            "crates/revenant-core/src/contract.rs":format!("{:x}",Sha256::digest(include_bytes!("../src/contract.rs"))),
            "crates/revenant-core/src/wire.rs":format!("{:x}",Sha256::digest(include_bytes!("../src/wire.rs"))),
            "Cargo.lock":format!("{:x}",Sha256::digest(include_bytes!("../../../Cargo.lock")))},
        "conditions":{"warmupGenerations":WARMUPS,"measuredGenerations":REPEATS,"iterationsPerStage":ITERATIONS,
            "order":"alternate ad hoc/prepared order each round; new validator generation per round",
            "observer":"Instant timer around each complete loop; no per-value profiler/sampler",
            "concurrentExternalActivity":"not controlled"},
        "preparationCountBasis":"explicit constructor/API invocation counts; each new/validate_json directly enters jsonschema::validator_for once by inspected source; library internal activity is not instrumented",
        "nativeRoundtripStages":["typed model -> JSON Value","one schema validation","JSON Value -> encoded bytes","bytes -> JSON Value","JSON Value -> typed model"],
        "contributionBasis":"isolated stage microseconds divided by separately measured native roundtrip; descriptive estimate, not an instrumented application trace; ratios need not sum exactly",
        "applicationEndToEndValidationContribution":Value::Null,
        "largeContractScope":"large serialized typed collection payload; same nested framework record schema, not a large unrelated schema or an actual application operation",
        "excluded":["operation lookup/admission","resource authorization/visitation","task lifecycle","bridge IPC","frontend publication","native domain work","filesystem IO","process memory"],
        "workloads":[small,large]});
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/prepared-validation-benchmark.json");
    std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("report: {}", output.display());
}
