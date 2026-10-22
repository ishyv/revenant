#![cfg(feature = "experimental-compute")]
use revenant_core::compute::{
    BinaryOp, Instruction, Kernel, MAX_ELEMENTS, MAX_INSTRUCTIONS, NumericBuffer, NumericType,
    Operand, Output, Scalar,
};
use revenant_core::{ResourceRegistry, TaskManager};

fn add(kind: NumericType, scalar: Scalar, output: Output) -> Kernel {
    Kernel::new(
        kind,
        vec![Instruction {
            op: BinaryOp::Add,
            left: Operand::Input,
            right: Operand::Constant(scalar),
        }],
        output,
    )
    .unwrap()
}

#[test]
fn integer_elements_and_reductions_use_declared_wrapping_arithmetic() {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let task = TaskManager::new(resources)
        .create(&scope, "compute")
        .unwrap();
    let kernel = add(NumericType::U32, Scalar::U32(1), Output::Elements);
    let input = NumericBuffer::u32(vec![u32::MAX, 1, 3]).unwrap();
    assert_eq!(
        kernel
            .execute_cpu(&input, &task.cancellation())
            .unwrap()
            .as_u32()
            .unwrap(),
        &[0, 2, 4]
    );
    let signed = add(NumericType::I32, Scalar::I32(1), Output::Sum);
    assert_eq!(
        signed
            .execute_cpu(
                &NumericBuffer::i32(vec![i32::MAX, -2]).unwrap(),
                &task.cancellation()
            )
            .unwrap()
            .as_i32()
            .unwrap(),
        &[i32::MAX]
    );
}

#[test]
fn float_reduction_is_left_to_right_and_rejects_nonfinite_data() {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let task = TaskManager::new(resources)
        .create(&scope, "compute")
        .unwrap();
    let kernel = Kernel::new(NumericType::F32, vec![], Output::Sum).unwrap();
    let input = NumericBuffer::f32(vec![1e20, -1e20, 3.0]).unwrap();
    assert_eq!(
        kernel
            .execute_cpu(&input, &task.cancellation())
            .unwrap()
            .as_f32()
            .unwrap(),
        &[3.0]
    );
    assert_eq!(
        NumericBuffer::f32(vec![f32::NAN]).unwrap_err().code,
        "compute_nonfinite"
    );
    let overflow = add(NumericType::F32, Scalar::F32(f32::MAX), Output::Elements);
    assert_eq!(
        overflow
            .execute_cpu(
                &NumericBuffer::f32(vec![f32::MAX]).unwrap(),
                &task.cancellation()
            )
            .unwrap_err()
            .code,
        "compute_nonfinite"
    );
}

#[test]
fn layouts_dispatch_and_roundtrips_are_bounded_and_checked() {
    let kernel = add(NumericType::U32, Scalar::U32(1), Output::Elements);
    let layout = kernel.layout();
    assert_eq!(
        (
            layout.element_stride,
            layout.element_alignment,
            layout.parameter_bytes,
            layout.workgroup_size
        ),
        (4, 4, 16, 64)
    );
    assert_eq!(kernel.dispatch(65).unwrap(), [2, 1, 1]);
    assert_eq!(kernel.dispatch(0).unwrap(), [0, 1, 1]);
    assert_eq!(
        kernel.dispatch(MAX_ELEMENTS + 1).unwrap_err().code,
        "compute_buffer_limit"
    );
    assert_eq!(
        NumericBuffer::u32(vec![0; MAX_ELEMENTS + 1])
            .unwrap_err()
            .code,
        "compute_buffer_limit"
    );
    for input in [
        NumericBuffer::u32(vec![0, u32::MAX]).unwrap(),
        NumericBuffer::i32(vec![i32::MIN, -1]).unwrap(),
        NumericBuffer::f32(vec![-0.0, 1.5]).unwrap(),
    ] {
        let output =
            NumericBuffer::from_le_bytes(input.numeric_type(), &input.to_le_bytes()).unwrap();
        assert_eq!(output.to_le_bytes(), input.to_le_bytes());
    }
    assert_eq!(
        NumericBuffer::from_le_bytes(NumericType::U32, &[0])
            .unwrap_err()
            .code,
        "compute_layout"
    );
    assert!(kernel.wgsl().contains("if (id.x >= params.len)"));
    assert!(kernel.wgsl().contains("@workgroup_size(64)"));
}

#[test]
fn malformed_ir_types_future_references_and_empty_reductions_are_rejected() {
    assert_eq!(
        Kernel::new(
            NumericType::U32,
            vec![Instruction {
                op: BinaryOp::Add,
                left: Operand::Value(0),
                right: Operand::Input
            }],
            Output::Elements
        )
        .unwrap_err()
        .code,
        "compute_ir"
    );
    assert_eq!(
        Kernel::new(
            NumericType::U32,
            vec![Instruction {
                op: BinaryOp::Add,
                left: Operand::Input,
                right: Operand::Constant(Scalar::I32(0))
            }],
            Output::Elements
        )
        .unwrap_err()
        .code,
        "compute_type"
    );
    assert_eq!(
        Kernel::new(
            NumericType::U32,
            vec![
                Instruction {
                    op: BinaryOp::Add,
                    left: Operand::Input,
                    right: Operand::Input
                };
                MAX_INSTRUCTIONS + 1
            ],
            Output::Elements
        )
        .unwrap_err()
        .code,
        "compute_ir"
    );
    assert_eq!(
        Kernel::new(NumericType::U32, vec![], Output::Min)
            .unwrap()
            .dispatch(0)
            .unwrap_err()
            .code,
        "compute_empty_reduction"
    );
}

#[test]
fn cancelled_or_disposed_owners_cannot_publish_compute_output() {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let task = TaskManager::new(resources.clone())
        .create(&scope, "compute")
        .unwrap();
    let _execution = task.begin().unwrap();
    let kernel = add(NumericType::U32, Scalar::U32(1), Output::Elements);
    let buffer = NumericBuffer::u32(vec![1]).unwrap();
    let owned = task.own(buffer.clone()).unwrap();
    let lease = task.lease_resource(&owned).unwrap();
    resources.dispose_scope(&scope).unwrap();
    assert_eq!(lease.as_u32().unwrap(), &[1]);
    assert_eq!(
        kernel
            .execute_cpu(&buffer, &task.cancellation())
            .unwrap_err()
            .code,
        "cancelled"
    );
}

#[test]
fn cancellation_during_a_large_cpu_run_discards_partial_output() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    let task = TaskManager::new(resources)
        .create(&scope, "compute")
        .unwrap();
    let _execution = task.begin().unwrap();
    let checkpoints = Arc::new(AtomicUsize::new(0));
    let counter = checkpoints.clone();
    task.set_cancellation_probe(Arc::new(move || {
        counter.fetch_add(1, Ordering::AcqRel) >= 2
    }))
    .unwrap();
    let kernel = add(NumericType::U32, Scalar::U32(1), Output::Elements);
    let input = NumericBuffer::u32(vec![1; 1024]).unwrap();
    assert_eq!(
        kernel
            .execute_cpu(&input, &task.cancellation())
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(checkpoints.load(Ordering::Acquire), 3);
    assert!(input.as_u32().unwrap().iter().all(|value| *value == 1));
}
