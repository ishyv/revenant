//! Frozen opt-in experimental workloads, shared by CPU-only and native GPU measurements.
use super::*;
use sha2::{Digest, Sha256};

/// Resident numerical workload cardinality.
pub const RESIDENT_ELEMENTS: usize = 1_048_576;
/// Number of identical resident transform passes, reading back only the final output.
pub const RESIDENT_PASSES: usize = 16;
/// Small workload cardinality; upload and readback overhead are deliberately visible.
pub const TRANSFER_ELEMENTS: usize = 128;

/// Thirty-two alternating wrapping multiply-three/add-seven instructions.
pub fn resident_kernel() -> Result<Kernel> {
    let instructions = (0..32)
        .map(|index| Instruction {
            op: if index % 2 == 0 {
                BinaryOp::Mul
            } else {
                BinaryOp::Add
            },
            left: if index == 0 {
                Operand::Input
            } else {
                Operand::Value(index - 1)
            },
            right: Operand::Constant(Scalar::U32(if index % 2 == 0 { 3 } else { 7 })),
        })
        .collect();
    Kernel::new(NumericType::U32, instructions, Output::Elements)
}
/// Deterministic ascending unsigned input, independent of external files or datasets.
pub fn resident_input() -> Result<NumericBuffer> {
    NumericBuffer::u32((0..RESIDENT_ELEMENTS).map(|index| index as u32).collect())
}
/// One wrapping unsigned add-seven instruction for the transfer-heavy case.
pub fn transfer_kernel() -> Result<Kernel> {
    Kernel::new(
        NumericType::U32,
        vec![Instruction {
            op: BinaryOp::Add,
            left: Operand::Input,
            right: Operand::Constant(Scalar::U32(7)),
        }],
        Output::Elements,
    )
}
/// Deterministic small unsigned buffer.
pub fn transfer_input() -> Result<NumericBuffer> {
    NumericBuffer::u32((0..TRANSFER_ELEMENTS).map(|index| index as u32).collect())
}
/// Exact affine composition baseline; the interpreter cannot manufacture a GPU performance win.
pub fn composed_resident_cpu(input: &NumericBuffer) -> Result<NumericBuffer> {
    let values = input
        .as_u32()
        .ok_or_else(|| Error::new("compute_type", "Resident reference requires u32"))?;
    let mut multiplier = 1u32;
    let mut offset = 0u32;
    for _ in 0..RESIDENT_PASSES {
        for _ in 0..16 {
            multiplier = multiplier.wrapping_mul(3);
            offset = offset.wrapping_mul(3).wrapping_add(7);
        }
    }
    NumericBuffer::u32(
        values
            .iter()
            .map(|value| value.wrapping_mul(multiplier).wrapping_add(offset))
            .collect(),
    )
}
/// SHA256 of compiled IR and workload sources, rather than a mutable workspace Git identity.
pub fn source_digest() -> String {
    let mut hash = Sha256::new();
    hash.update(include_bytes!("../compute.rs"));
    hash.update(include_bytes!("fixtures.rs"));
    format!("sha256:{:x}", hash.finalize())
}
