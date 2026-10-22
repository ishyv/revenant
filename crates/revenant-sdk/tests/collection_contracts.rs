use revenant_sdk::{Contract, DecimalU64, contract};

#[contract]
#[serde(rename_all = "camelCase")]
struct Node {
    id: DecimalU64,
    parent: Option<DecimalU64>,
    allocated_bytes: Option<DecimalU64>,
    warning: Option<String>,
}

#[contract]
struct Page {
    rows: Vec<Node>,
}

#[test]
fn optional_dto_collections_have_symmetric_contracts() {
    Node::definition().unwrap();
    let definition = Vec::<Node>::definition().unwrap();
    assert!(definition.typescript.contains("parent?:"));
    let missing = serde_json::json!([{ "id": "1" }]);
    revenant_sdk::validate_json(&definition.schema, &missing).unwrap();
    revenant_sdk::validate_json(
        &definition.schema,
        &serde_json::json!([{ "id": "1", "parent": null }]),
    )
    .unwrap_err();
    let nodes: Vec<Node> = serde_json::from_value(missing.clone()).unwrap();
    assert_eq!(serde_json::to_value(nodes).unwrap(), missing);
    std::collections::BTreeMap::<String, Node>::definition().unwrap();
    Page::definition().unwrap();
}

#[contract]
struct OptionalPair(Option<String>, u32);

#[derive(Contract)]
struct DerivedOptionalPair(Option<String>, u32);

#[contract]
enum OptionalVariant {
    Pair(Option<String>, u32),
}

#[test]
fn positional_options_preserve_null_slots_and_round_trip() {
    let expected = serde_json::json!([null, 7]);
    let value = serde_json::to_value(OptionalPair(None, 7)).unwrap();
    assert_eq!(value, expected);
    let decoded: OptionalPair = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    let definition = OptionalPair::definition().unwrap();
    revenant_sdk::validate_json(&definition.schema, &value).unwrap();
    assert!(
        definition.typescript.contains("string | null"),
        "{}",
        definition.typescript
    );

    let value = serde_json::to_value(DerivedOptionalPair(None, 7)).unwrap();
    assert_eq!(value, expected);
    let decoded: DerivedOptionalPair = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    revenant_sdk::validate_json(&DerivedOptionalPair::definition().unwrap().schema, &value)
        .unwrap();

    let value = serde_json::to_value(OptionalVariant::Pair(None, 7)).unwrap();
    assert_eq!(value, serde_json::json!({"Pair": [null, 7]}));
    let decoded: OptionalVariant = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    revenant_sdk::validate_json(&OptionalVariant::definition().unwrap().schema, &value).unwrap();
}

#[contract]
struct ExplicitOptional {
    #[serde(default, skip_serializing_if = "std::option::Option::is_none")]
    value: std::option::Option<String>,
}

#[test]
fn explicit_option_omission_is_preserved_without_duplicate_serde_attributes() {
    let value: ExplicitOptional = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(serde_json::to_value(value).unwrap(), serde_json::json!({}));
    ExplicitOptional::definition().unwrap();
}
