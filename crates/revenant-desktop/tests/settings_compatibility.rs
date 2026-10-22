mod support;
use serde_json::json;

#[tokio::test]
async fn settings_preserve_unknown_fields_and_array_positions_across_load_and_save() {
    let harness = support::Harness::new(0, 0, 1, 2).await;
    let defaults = json!({"theme":"dark","panel":{"width":20},"tuple":["name",1],"empty":[]});
    let persisted = json!({"theme":"light","panel":{"width":40,"obsolete":true},"legacy":7,"tuple":["updated",2,{"extra":true}],"empty":[false]});
    harness
        .request(json!({"action":"settings.save","key":"workspace","value":persisted}))
        .await
        .unwrap();
    assert_eq!(
        harness
            .request(json!({"action":"settings.load","key":"workspace","defaults":defaults}))
            .await
            .unwrap(),
        persisted
    );
    let shorter =
        json!({"theme":"dark","panel":{"width":40},"tuple":["short"],"empty":["anything"]});
    harness
        .request(json!({"action":"settings.save","key":"workspace","value":shorter}))
        .await
        .unwrap();
    assert_eq!(
        harness
            .request(json!({"action":"settings.load","key":"workspace","defaults":defaults}))
            .await
            .unwrap(),
        shorter
    );
    harness.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn settings_reject_wrong_known_types_without_replacing_persisted_values() {
    let harness = support::Harness::new(0, 0, 1, 2).await;
    let defaults = json!({"theme":"dark","tuple":["name",1]});
    harness
        .request(json!({"action":"settings.load","key":"workspace","defaults":defaults}))
        .await
        .unwrap();
    harness
        .request(json!({"action":"settings.save","key":"workspace","value":defaults}))
        .await
        .unwrap();
    for invalid in [
        json!({"theme":1,"tuple":["name",1]}),
        json!({"theme":"dark","tuple":["name","wrong"]}),
    ] {
        assert_eq!(
            harness
                .request(json!({"action":"settings.save","key":"workspace","value":invalid}))
                .await
                .unwrap_err()
                .code,
            "settings_type_mismatch"
        );
        assert_eq!(
            harness
                .request(json!({"action":"settings.load","key":"workspace","defaults":defaults}))
                .await
                .unwrap(),
            defaults
        );
    }
    harness.runtime.shutdown().await.unwrap();
}
