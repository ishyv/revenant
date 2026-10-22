use revenant_sdk::operation;

revenant_sdk::app!();

#[operation(id = "duplicate.id")]
fn first() -> u32 {
    1
}

mod other_module {
    use super::*;

    #[operation(id = "duplicate.id")]
    fn second() -> u32 {
        2
    }
}

#[test]
fn linker_discovery_rejects_duplicate_ids_across_modules() {
    let error = app().manifest().unwrap_err();
    assert_eq!(error.code, "operation_conflict");
}
