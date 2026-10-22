//! Opt-in native compute conformance and complete-cost evidence.
#![cfg(feature = "experimental-compute")]
use revenant_core::{
    ResourceRegistry, TaskManager,
    compute::{
        BinaryOp, FLOAT_TOLERANCE, Instruction, Kernel, NumericBuffer, NumericType, Operand,
        Output, Scalar, fixtures,
    },
};
use revenant_graphics::compute::{ComputeTimings, GpuKernel};
use serde_json::json;
use std::time::Instant;

fn token() -> revenant_core::CancellationToken {
    let resources = ResourceRegistry::new();
    let scope = resources.create_scope(None).unwrap();
    TaskManager::new(resources)
        .create(&scope, "compute-fixture")
        .unwrap()
        .cancellation()
}

fn kernel(kind: NumericType, op: BinaryOp, constant: Scalar, output: Output) -> Kernel {
    Kernel::new(
        kind,
        vec![Instruction {
            op,
            left: Operand::Input,
            right: Operand::Constant(constant),
        }],
        output,
    )
    .unwrap()
}

fn same(expected: &NumericBuffer, actual: &NumericBuffer) {
    assert_eq!(expected.numeric_type(), actual.numeric_type());
    assert_eq!(expected.len(), actual.len());
    if let Some(values) = expected.as_f32() {
        for (&a, &b) in values.iter().zip(actual.as_f32().unwrap()) {
            assert!(
                (a - b).abs() <= FLOAT_TOLERANCE * (1. + a.abs().max(b.abs())),
                "float mismatch {a} vs {b}"
            );
        }
    } else {
        assert_eq!(expected.to_le_bytes(), actual.to_le_bytes());
    }
}

#[test]
#[ignore = "Requires a native DX12/Vulkan adapter; compares checked CPU and actual GPU results"]
fn gpu_matches_cpu_for_elements_ordered_reductions_and_wrapping() {
    let token = token();
    for output in [Output::Elements, Output::Sum, Output::Min, Output::Max] {
        for (input, ir) in [
            (
                NumericBuffer::u32(vec![0, u32::MAX, 123, 7, 9]).unwrap(),
                kernel(NumericType::U32, BinaryOp::Add, Scalar::U32(3), output),
            ),
            (
                NumericBuffer::i32(vec![i32::MAX, i32::MIN, -7, 0, 13]).unwrap(),
                kernel(NumericType::I32, BinaryOp::Mul, Scalar::I32(3), output),
            ),
            (
                NumericBuffer::f32(vec![0., -1.5, 2.25, 10., 0.125]).unwrap(),
                kernel(NumericType::F32, BinaryOp::Add, Scalar::F32(0.25), output),
            ),
        ] {
            let expected = ir.execute_cpu(&input, &token).unwrap();
            let mut gpu = pollster::block_on(GpuKernel::prepare(&ir)).unwrap();
            let actual = gpu.run(&input, 1, Some(&token)).unwrap();
            same(&expected, &actual.output);
        }
    }
    let overflow = kernel(
        NumericType::F32,
        BinaryOp::Mul,
        Scalar::F32(2.),
        Output::Elements,
    );
    let input = NumericBuffer::f32(vec![f32::MAX]).unwrap();
    let mut gpu = pollster::block_on(GpuKernel::prepare(&overflow)).unwrap();
    assert!(overflow.execute_cpu(&input, &token).is_err());
    assert!(gpu.run(&input, 1, Some(&token)).is_err());
    let hidden = Kernel::new(
        NumericType::F32,
        vec![
            Instruction {
                op: BinaryOp::Mul,
                left: Operand::Input,
                right: Operand::Constant(Scalar::F32(2.)),
            },
            Instruction {
                op: BinaryOp::Min,
                left: Operand::Value(0),
                right: Operand::Constant(Scalar::F32(1.)),
            },
        ],
        Output::Elements,
    )
    .unwrap();
    let mut gpu = pollster::block_on(GpuKernel::prepare(&hidden)).unwrap();
    assert!(hidden.execute_cpu(&input, &token).is_err());
    assert!(
        gpu.run(&input, 1, Some(&token)).is_err(),
        "Later min must not hide intermediate overflow"
    );
}

fn timings(t: ComputeTimings) -> serde_json::Value {
    json!({"preparation_ms":t.preparation_ms,"upload_allocate_enqueue_ms":t.upload_ms,
        "dispatch_encode_submit_ms":t.dispatch_submit_ms,"completion_map_wait_ms":t.synchronization_ms,
        "readback_copy_decode_ms":t.readback_ms,"total_wall_ms":t.total_ms})
}

#[test]
#[ignore = "Runs release native CPU/GPU numerical fixtures and writes an explicit promotion decision"]
fn resident_and_transfer_heavy_complete_cost_experiment() {
    let token = token();
    let resident = fixtures::resident_kernel().unwrap();
    let input = fixtures::resident_input().unwrap();
    let passes = fixtures::RESIDENT_PASSES;
    let start = Instant::now();
    let mut cpu = input.clone();
    for _ in 0..passes {
        cpu = resident.execute_cpu(&cpu, &token).unwrap();
    }
    let cpu_ir_ms = start.elapsed().as_secs_f64() * 1000.;
    // This exact affine kernel can be composed on CPU without allocation per pass.
    // Include that algorithmic baseline so interpreting the IR cannot manufacture a GPU win.
    let start = Instant::now();
    let composed = fixtures::composed_resident_cpu(&input).unwrap();
    let cpu_composed_ms = start.elapsed().as_secs_f64() * 1000.;
    same(&cpu, &composed);
    let mut gpu = pollster::block_on(GpuKernel::prepare(&resident)).unwrap();
    let adapter = gpu.adapter().to_owned();
    let cold = gpu.run(&input, passes, Some(&token)).unwrap();
    same(&cpu, &cold.output);
    let warm = gpu.run(&input, passes, Some(&token)).unwrap();
    same(&cpu, &warm.output);
    let small = fixtures::transfer_kernel().unwrap();
    let small_input = fixtures::transfer_input().unwrap();
    let start = Instant::now();
    let small_cpu = small.execute_cpu(&small_input, &token).unwrap();
    let small_cpu_ms = start.elapsed().as_secs_f64() * 1000.;
    let mut small_gpu = pollster::block_on(GpuKernel::prepare(&small)).unwrap();
    let small_cold = small_gpu.run(&small_input, 1, Some(&token)).unwrap();
    same(&small_cpu, &small_cold.output);
    let small_warm = small_gpu.run(&small_input, 1, Some(&token)).unwrap();
    same(&small_cpu, &small_warm.output);
    // A single restricted expression is insufficient for general public promotion;
    // the composed CPU comparison and transfer-heavy case are mandatory caveats.
    let report = json!({"schema":1,"adapter":adapter,"profile":if cfg!(debug_assertions){"debug"}else{"release"},
        "package_version":env!("CARGO_PKG_VERSION"),"fixture_source_digest":fixtures::source_digest(),
        "resident":{"elements":input.len(),"instructions":32,"passes":passes,"cpu_ir_ms":cpu_ir_ms,"cpu_composed_reference_ms":cpu_composed_ms,
            "gpu_cold":timings(cold.timings),"gpu_warm":timings(warm.timings),"known_gpu_buffers_bytes":warm.known_buffer_bytes,
            "transfer_scratch_bound_bytes":warm.transfer_scratch_bound_bytes,"correct":true},
        "transfer_heavy":{"elements":128,"instructions":1,"cpu_ms":small_cpu_ms,"gpu_cold":timings(small_cold.timings),
            "gpu_warm":timings(small_warm.timings),"known_gpu_buffers_bytes":small_warm.known_buffer_bytes,
            "transfer_scratch_bound_bytes":small_warm.transfer_scratch_bound_bytes,"correct":true},
        "source_sha256":{"core_ir":source_hash(include_bytes!("../../revenant-core/src/compute.rs")),
            "gpu_executor":source_hash(include_bytes!("../src/compute.rs")),"fixture":source_hash(include_bytes!("compute_experiment.rs"))},
        "decision":"no-go for public promotion; retain feature-gated experiment",
        "reason":"A win over a scalar IR interpreter does not establish a win over a composed/vectorized CPU algorithm; small transfers and preparation are visible. One adapter and two fixtures do not justify a production scheduling API.",
        "limitations":["Wall-stage timings, not GPU timestamps; synchronization includes upload, device execution and readback transfer",
            "Known GPU buffers exclude pipeline/device/driver heaps; standalone experimental devices are independent of scene services",
            "Only final output is read back for the resident chain; input upload remains included",
            "Exact64-bit accounting stays outside this32-bit numerical experiment",
            "No automatic stable-path fallback or public compute promotion"]});
    let output = serde_json::to_string_pretty(&report).unwrap();
    if let Some(path) = std::env::var_os("REVENANT_COMPUTE_REPORT") {
        std::fs::write(path, &output).unwrap();
    }
    println!("{output}");
}

fn source_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
