mod support;
use revenant_sdk::{
    OperationEffect, OperationRegistration, OperationRegistry, Result, TaskContext, WorkClass,
    operation,
};
use serde_json::json;
use support::{block_on, fresh_task};

#[operation(read_only, work = "interactive")]
fn interactive_read(context: TaskContext) -> Result<u32> {
    assert_eq!(context.work_class(), WorkClass::Interactive);
    let child = context.batch_child(0, 1)?;
    assert_eq!(child.work_class(), WorkClass::Interactive);
    assert_eq!(
        context
            .configure_work_class(WorkClass::Cpu)
            .unwrap_err()
            .code,
        "task_work_class_conflict"
    );
    Ok(17)
}
#[operation(work = "io")]
fn io_read(context: TaskContext) -> u32 {
    assert_eq!(context.work_class(), WorkClass::Io);
    23
}
#[test]
fn declared_work_class_is_pinned_in_dispatch_and_inherited_by_children() {
    let registry = OperationRegistry::new();
    registry
        .register(interactive_read_operation().unwrap())
        .unwrap();
    registry.register(io_read_operation().unwrap()).unwrap();
    let (_, _, context) = fresh_task("interactive");
    assert_eq!(
        block_on(registry.dispatch("interactiveRead", json!({}), context)).unwrap(),
        json!(17)
    );
    let (_, _, context) = fresh_task("io");
    assert_eq!(
        block_on(registry.dispatch("ioRead", json!({}), context)).unwrap(),
        json!(23)
    );
}
#[test]
fn advanced_registrations_cannot_reserve_interactive_slots_for_mutations() {
    let registry = OperationRegistry::new();
    let mut registration =
        OperationRegistration::new::<(), (), _, _>("mutate", "", |_, _| async { Ok(()) }).unwrap();
    registration.descriptor.effect = OperationEffect::Write;
    registration.descriptor.work_class = WorkClass::Interactive;
    assert_eq!(
        registry.register(registration).unwrap_err().code,
        "invalid_work_class"
    );
}
