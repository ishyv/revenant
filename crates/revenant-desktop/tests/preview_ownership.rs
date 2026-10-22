mod support;

use serde_json::json;
use support::{Harness, rows};

#[tokio::test]
async fn runtime_preview_counts_survive_empty_siblings_and_reclaim_on_owner_close() {
    let harness = Harness::new(1, 0, 2, 4).await;
    let (_, initial) = harness.open(4).await;
    let ready = harness.ready(initial).await;
    let descriptor = harness
        .request(json!({"action":"media.preview","handle":rows(&ready)[0]["handle"]}))
        .await
        .unwrap();
    assert_eq!(descriptor["text"], "abc");
    assert_eq!(
        harness.runtime.runtime_inspect().unwrap()["ownership"]["previews"],
        1
    );
    for _ in 0..20 {
        let sibling = harness.runtime.create_scope(&harness.scope).unwrap();
        harness.runtime.dispose_scope(&sibling).unwrap();
        assert_eq!(
            harness.runtime.runtime_inspect().unwrap()["ownership"]["previews"],
            1
        );
    }
    harness.runtime.dispose_scope(&harness.scope).unwrap();
    let final_state = harness.runtime.runtime_inspect().unwrap();
    assert_eq!(final_state["ownership"]["scopes"], 0);
    assert_eq!(final_state["ownership"]["previews"], 0);
    assert_eq!(final_state["ownership"]["views"], 0);
    assert_eq!(final_state["ownership"]["folders"], 0);
    harness.runtime.shutdown().await.unwrap();
}
