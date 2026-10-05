use revenant_core::{DecimalI64, DecimalU64, WireSafe, type_definition, validate_json};
use serde_json::json;

#[test]
fn decimal_integer_wrappers_preserve_signed_and_unsigned_bounds_canonically() {
    let unsigned_max = DecimalU64::from(u64::MAX);
    let encoded_unsigned = serde_json::to_string(&unsigned_max).unwrap();
    assert_eq!(encoded_unsigned, format!("\"{}\"", u64::MAX));
    assert_eq!(
        serde_json::from_str::<DecimalU64>(&encoded_unsigned).unwrap(),
        unsigned_max
    );
    assert!(serde_json::from_str::<DecimalU64>("\"18446744073709551616\"").is_err());
    assert!(serde_json::from_str::<DecimalU64>("\"01\"").is_err());
    assert!(serde_json::from_str::<DecimalU64>("1").is_err());

    for bound in [i64::MIN, i64::MAX] {
        let decimal = DecimalI64::from(bound);
        let encoded = serde_json::to_string(&decimal).unwrap();
        assert_eq!(encoded, format!("\"{bound}\""));
        assert_eq!(
            serde_json::from_str::<DecimalI64>(&encoded).unwrap(),
            decimal
        );
    }
    assert!(serde_json::from_str::<DecimalI64>("\"9223372036854775808\"").is_err());
    assert!(serde_json::from_str::<DecimalI64>("\"-9223372036854775809\"").is_err());
    assert!(serde_json::from_str::<DecimalI64>("\"-0\"").is_err());

    let definition = type_definition::<DecimalU64>().unwrap();
    validate_json(&definition.schema, &json!(u64::MAX.to_string())).unwrap();
    assert_eq!(
        validate_json(&definition.schema, &json!(u64::MAX))
            .unwrap_err()
            .code,
        "contract_validation"
    );
}

#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, ts_rs::TS)]
struct NestedWideInteger {
    value: i64,
}

impl WireSafe for NestedWideInteger {}

#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, ts_rs::TS)]
struct RootWithNestedWideInteger {
    nested: Vec<NestedWideInteger>,
}

impl WireSafe for RootWithNestedWideInteger {}

#[test]
fn contract_schema_audit_rejects_lossy_integers_inside_nested_types() {
    let error = type_definition::<RootWithNestedWideInteger>().unwrap_err();
    assert_eq!(error.code, "lossy_integer");
}
