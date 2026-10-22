//! File/media desktop example: built-in native files, previews and checksum.
//!
//! Only record normalization is application-specific. The framework owns its
//! desktop bootstrap, task executor, settings storage and contract extraction.
pub mod records;
revenant::app!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_sync_normalization_preserves_record_identity_and_status() {
        let record = records::normalize(records::Record {
            id: "draft".into(),
            title: "  hello  ".into(),
            done: true,
        });
        assert_eq!(record.id, "draft");
        assert_eq!(record.title, "HELLO");
        assert!(record.done);
        let manifest = app().manifest().unwrap();
        let operation = manifest
            .operations
            .iter()
            .find(|op| op.id == "records.normalize")
            .unwrap();
        assert_eq!(operation.arguments, ["record"]);
        assert!(operation.independent);
        assert!(!operation.agent);
        revenant::validate_json(
            &operation.input.schema,
            &revenant::serde_json::json!({"record": {"id":"draft", "title":"hello", "done":true}}),
        )
        .unwrap();
    }
}
