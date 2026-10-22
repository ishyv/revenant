use revenant_core::{PreparedValidator, validate_json};
use serde_json::json;
use std::sync::Arc;

#[test]
fn prepared_contract_preserves_references_annotations_bounds_and_concurrent_validation() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": {"Count": {"type":"integer","minimum":0,"maximum":100}},
        "type":"object", "properties":{"count":{"$ref":"#/$defs/Count"}},
        "required":["count"], "unevaluatedProperties":false
    });
    let prepared = Arc::new(PreparedValidator::new(schema.clone()).unwrap());
    for value in [
        json!({"count":17}),
        json!({"count":101}),
        json!({"count":0,"extra":1}),
        json!({"count":"17"}),
    ] {
        assert_eq!(
            prepared.validate(&value).is_ok(),
            validate_json(&schema, &value).is_ok()
        );
    }
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let prepared = prepared.clone();
            scope.spawn(move || {
                for _ in 0..1_000 {
                    prepared.validate(&json!({"count":23})).unwrap();
                }
            });
        }
    });
    assert_eq!(Arc::strong_count(&prepared), 1);
    assert_eq!(prepared.schema(), &schema);
    assert_eq!(
        PreparedValidator::new(json!({"type":17})).unwrap_err().code,
        "invalid_schema"
    );
}
