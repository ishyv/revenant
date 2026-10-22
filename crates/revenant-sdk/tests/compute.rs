#![cfg(feature = "experimental-compute")]
use revenant_sdk::compute::{BinaryOp, KernelBuilder, Output};

#[test]
fn typed_builder_rejects_cross_kernel_values_and_finishes_ordinary_ir() {
    let mut builder = KernelBuilder::<u32>::new().unwrap();
    let input = builder.input();
    let three = builder.constant(3).unwrap();
    let added = builder.binary(BinaryOp::Add, input, three).unwrap();
    builder.binary(BinaryOp::Mul, added, three).unwrap();
    let other = KernelBuilder::<u32>::new().unwrap();
    assert_eq!(
        builder
            .binary(BinaryOp::Add, added, other.input())
            .unwrap_err()
            .code,
        "compute_ir"
    );
    let kernel = builder.finish(Output::Elements).unwrap();
    assert!(kernel.wgsl().contains("v1"));
    assert_eq!(kernel.dispatch(65).unwrap(), [2, 1, 1]);
}

#[test]
fn cpu_helper_uses_host_admission_and_publishes_only_owned_native_results() {
    use revenant_sdk::compute::{NumericBuffer, run_cpu};
    use revenant_sdk::{
        CancellationToken, ResourceRegistry, TaskManager, TaskServices, WorkClass, WorkJob,
        WorkService, WorkStats,
    };
    use std::sync::Arc;
    struct Inline;
    impl WorkService for Inline {
        fn execute(
            &self,
            _: &str,
            class: WorkClass,
            token: CancellationToken,
            jobs: Vec<WorkJob>,
        ) -> revenant_sdk::Result<()> {
            assert_eq!(class, WorkClass::Cpu);
            for job in jobs {
                token.checkpoint()?;
                job()?;
            }
            Ok(())
        }
        fn stats(&self) -> WorkStats {
            WorkStats::default()
        }
    }
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let manager = TaskManager::with_services(
        resources.clone(),
        Arc::new(TaskServices::new(None, Some(Arc::new(Inline)))),
    );
    let task = manager.create(&scope, "compute").unwrap();
    let _execution = task.begin().unwrap();
    let input = task.own(NumericBuffer::u32(vec![1, 2]).unwrap()).unwrap();
    let mut builder = KernelBuilder::<u32>::new().unwrap();
    let source = builder.input();
    let three = builder.constant(3).unwrap();
    builder.binary(BinaryOp::Mul, source, three).unwrap();
    let output = run_cpu(&task, &builder.finish(Output::Elements).unwrap(), &input).unwrap();
    assert_eq!(
        task.lease_resource(&output).unwrap().as_u32().unwrap(),
        &[3, 6]
    );
    assert_eq!(resources.native_resource_counts().0, 2);
    let incompatible = KernelBuilder::<i32>::new()
        .unwrap()
        .finish(Output::Elements)
        .unwrap();
    assert_eq!(
        run_cpu(&task, &incompatible, &input).unwrap_err().code,
        "compute_type"
    );
    assert_eq!(resources.native_resource_counts().0, 2);
    task.request_cancel().unwrap();
    assert_eq!(
        run_cpu(&task, &incompatible, &input).unwrap_err().code,
        "cancelled"
    );
    assert_eq!(resources.native_resource_counts().0, 2);
}
