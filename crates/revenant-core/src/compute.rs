//! Opt-in, pure numerical experiment; this IR cannot express effects or arbitrary Rust closures.
//! Integers wrap modulo 2^32. Reductions visit inputs in ascending index order.
//! Finite f32 arithmetic is compared with absolute/relative tolerance 1e-5; overflow
//! to infinity is an error, and no wide-integer accounting is converted to floats.
use crate::{CancellationToken, Error, Result};
use std::sync::Arc;

/// Frozen CPU/GPU conformance workloads and their source identity.
pub mod fixtures;

/// Maximum elements in an admitted immutable numeric buffer (four MiB).
pub const MAX_ELEMENTS: usize = 1_048_576;
/// Maximum DAG instructions per kernel; scratch is fixed and independent of buffer length.
pub const MAX_INSTRUCTIONS: usize = 32;
/// CPU/GPU finite-float conformance tolerance, applied as abs + rel * magnitude.
pub const FLOAT_TOLERANCE: f32 = 1e-5;

/// The complete supported scalar type set; 64-bit accounting stays on ordinary CPU paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericType {
    /// Unsigned wrapping 32-bit integer.
    U32,
    /// Signed two's-complement wrapping 32-bit integer.
    I32,
    /// Finite IEEE 754 binary32, with declared cross-backend tolerance.
    F32,
}
impl NumericType {
    fn wgsl(self) -> &'static str {
        match self {
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::F32 => "f32",
        }
    }
    fn zero(self) -> Scalar {
        match self {
            Self::U32 => Scalar::U32(0),
            Self::I32 => Scalar::I32(0),
            Self::F32 => Scalar::F32(0.0),
        }
    }
}

/// Explicitly typed constant; constructors validate consistency and finite float values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar {
    /// Unsigned constant.
    U32(u32),
    /// Signed constant.
    I32(i32),
    /// Finite floating-point constant.
    F32(f32),
}
impl Scalar {
    /// Scalar's numerical domain.
    pub fn numeric_type(self) -> NumericType {
        match self {
            Self::U32(_) => NumericType::U32,
            Self::I32(_) => NumericType::I32,
            Self::F32(_) => NumericType::F32,
        }
    }
    fn validate(self) -> Result<()> {
        if matches!(self, Self::F32(value) if !value.is_finite()) {
            return Err(nonfinite());
        }
        Ok(())
    }
    fn wgsl(self) -> String {
        match self {
            Self::U32(value) => format!("{value}u"),
            Self::I32(value) => format!("bitcast<i32>({}u)", value as u32),
            Self::F32(value) => format!("bitcast<f32>({}u)", value.to_bits()),
        }
    }
}

/// A single lane's input, earlier instruction, or bounded typed constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operand {
    /// Current input lane.
    Input,
    /// Index of an instruction preceding the current one.
    Value(usize),
    /// Explicit scalar constant.
    Constant(Scalar),
}
/// Pure supported operations; no division, allocation, calls or effect nodes exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// Wrapping integer or finite floating-point addition.
    Add,
    /// Wrapping integer or finite floating-point multiplication.
    Mul,
    /// Smaller of two finite values.
    Min,
    /// Larger of two finite values.
    Max,
}
/// One typed DAG instruction evaluated once per input lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instruction {
    /// Numerical operation.
    pub op: BinaryOp,
    /// First operand.
    pub left: Operand,
    /// Second operand.
    pub right: Operand,
}
/// Output shape; reductions apply after the final lane expression, in index order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// One transformed element per input.
    Elements,
    /// Left-to-right sum, with zero identity for empty input.
    Sum,
    /// Sequential minimum; empty input is rejected.
    Min,
    /// Sequential maximum; empty input is rejected.
    Max,
}
/// Immutable resident native buffer, suitable for Resource ownership and admitted leases.
#[derive(Clone, Debug)]
pub struct NumericBuffer(Buffer);
#[derive(Clone, Debug)]
enum Buffer {
    U32(Arc<[u32]>),
    I32(Arc<[i32]>),
    F32(Arc<[f32]>),
}
impl NumericBuffer {
    /// Admit at most four MiB of unsigned data.
    pub fn u32(values: Vec<u32>) -> Result<Self> {
        check_len(values.len())?;
        Ok(Self(Buffer::U32(values.into())))
    }
    /// Admit at most four MiB of signed data.
    pub fn i32(values: Vec<i32>) -> Result<Self> {
        check_len(values.len())?;
        Ok(Self(Buffer::I32(values.into())))
    }
    /// Admit at most four MiB of finite float data.
    pub fn f32(values: Vec<f32>) -> Result<Self> {
        check_len(values.len())?;
        if values.iter().any(|value| !value.is_finite()) {
            return Err(nonfinite());
        }
        Ok(Self(Buffer::F32(values.into())))
    }
    /// Element domain, without transferring native contents.
    pub fn numeric_type(&self) -> NumericType {
        match self.0 {
            Buffer::U32(_) => NumericType::U32,
            Buffer::I32(_) => NumericType::I32,
            Buffer::F32(_) => NumericType::F32,
        }
    }
    /// Number of resident numerical elements.
    pub fn len(&self) -> usize {
        match &self.0 {
            Buffer::U32(v) => v.len(),
            Buffer::I32(v) => v.len(),
            Buffer::F32(v) => v.len(),
        }
    }
    /// Whether the resident buffer has no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Compare declared backend semantics: exact integer equality or finite f32 tolerance.
    pub fn conforms(&self, expected: &Self) -> bool {
        match (&self.0, &expected.0) {
            (Buffer::U32(a), Buffer::U32(b)) => a == b,
            (Buffer::I32(a), Buffer::I32(b)) => a == b,
            (Buffer::F32(a), Buffer::F32(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b.iter()).all(|(actual, expected)| {
                        actual == expected
                            || (*actual - *expected).abs()
                                <= FLOAT_TOLERANCE * (1.0 + actual.abs().max(expected.abs()))
                    })
            }
            _ => false,
        }
    }
    /// Exact known native payload allocation, excluding Arc/container bookkeeping.
    pub fn byte_len(&self) -> usize {
        self.len() * 4
    }
    /// Borrow unsigned resident data only for that domain.
    pub fn as_u32(&self) -> Option<&[u32]> {
        if let Buffer::U32(values) = &self.0 {
            Some(values)
        } else {
            None
        }
    }
    /// Borrow signed resident data only for that domain.
    pub fn as_i32(&self) -> Option<&[i32]> {
        if let Buffer::I32(values) = &self.0 {
            Some(values)
        } else {
            None
        }
    }
    /// Borrow finite float resident data only for that domain.
    pub fn as_f32(&self) -> Option<&[f32]> {
        if let Buffer::F32(values) = &self.0 {
            Some(values)
        } else {
            None
        }
    }
    /// Encode checked four-byte little-endian elements for explicit GPU transfer.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        match &self.0 {
            Buffer::U32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Buffer::I32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Buffer::F32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        }
    }
    /// Decode only a bounded, aligned numerical readback; nonfinite float data fails.
    pub fn from_le_bytes(kind: NumericType, bytes: &[u8]) -> Result<Self> {
        if !bytes.len().is_multiple_of(4) {
            return Err(Error::new(
                "compute_layout",
                "Numeric buffer byte length must be a multiple of four",
            ));
        }
        check_len(bytes.len() / 4)?;
        let words = || bytes.as_chunks::<4>().0.iter().copied();
        match kind {
            NumericType::U32 => Self::u32(words().map(u32::from_le_bytes).collect()),
            NumericType::I32 => Self::i32(words().map(i32::from_le_bytes).collect()),
            NumericType::F32 => Self::f32(words().map(f32::from_le_bytes).collect()),
        }
    }
    fn scalar(&self, index: usize) -> Scalar {
        match &self.0 {
            Buffer::U32(v) => Scalar::U32(v[index]),
            Buffer::I32(v) => Scalar::I32(v[index]),
            Buffer::F32(v) => Scalar::F32(v[index]),
        }
    }
}
/// Checked shared CPU/WGSL storage and parameter layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferLayout {
    /// WGSL storage-array element stride.
    pub element_stride: u32,
    /// WGSL storage-array element alignment.
    pub element_alignment: u32,
    /// Uniform size: len and three explicit padding words.
    pub parameter_bytes: u32,
    /// Fixed element-wise thread group width.
    pub workgroup_size: u32,
}
/// Validated, immutable pure numerical program; clones pin one exact IR generation.
#[derive(Clone, Debug)]
pub struct Kernel {
    kind: NumericType,
    instructions: Arc<[Instruction]>,
    output: Output,
}
impl Kernel {
    /// Validate supported scalar domains and backward-only DAG references.
    pub fn new(kind: NumericType, instructions: Vec<Instruction>, output: Output) -> Result<Self> {
        if instructions.len() > MAX_INSTRUCTIONS {
            return Err(Error::new(
                "compute_ir",
                "Kernel exceeds thirty-two instructions",
            ));
        }
        for (index, instruction) in instructions.iter().enumerate() {
            for operand in [instruction.left, instruction.right] {
                match operand {
                    Operand::Value(reference) if reference >= index => {
                        return Err(Error::new(
                            "compute_ir",
                            "Kernel references an unavailable instruction",
                        ));
                    }
                    Operand::Constant(value) => {
                        value.validate()?;
                        if value.numeric_type() != kind {
                            return Err(Error::new(
                                "compute_type",
                                "Kernel constant uses a different numeric type",
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(Self {
            kind,
            instructions: instructions.into(),
            output,
        })
    }
    /// Numerical domain pinned by validation.
    pub fn numeric_type(&self) -> NumericType {
        self.kind
    }
    /// Output shape pinned by validation.
    pub fn output(&self) -> Output {
        self.output
    }
    /// Fixed generated layout shared by uploads, dispatch and readback validation.
    pub fn layout(&self) -> BufferLayout {
        BufferLayout {
            element_stride: 4,
            element_alignment: 4,
            parameter_bytes: 16,
            workgroup_size: 64,
        }
    }
    /// Checked logical output length; empty min/max reductions are undefined and rejected.
    pub fn output_len(&self, len: usize) -> Result<usize> {
        check_len(len)?;
        if len == 0 && matches!(self.output, Output::Min | Output::Max) {
            return Err(Error::new(
                "compute_empty_reduction",
                "Minimum and maximum need at least one element",
            ));
        }
        Ok(if self.output == Output::Elements {
            len
        } else {
            1
        })
    }
    /// Bounded dispatch dimensions, with a guarded last workgroup.
    pub fn dispatch(&self, len: usize) -> Result<[u32; 3]> {
        self.output_len(len)?;
        Ok([
            if self.output == Output::Elements {
                len.div_ceil(64) as u32
            } else {
                1
            },
            1,
            1,
        ])
    }
    /// Execute one bounded job on a CPU worker; checkpoints prevent cancelled publication.
    pub fn execute_cpu(
        &self,
        input: &NumericBuffer,
        token: &CancellationToken,
    ) -> Result<NumericBuffer> {
        token.checkpoint()?;
        if input.numeric_type() != self.kind {
            return Err(Error::new(
                "compute_type",
                "Input numeric type differs from the kernel",
            ));
        }
        let len = self.output_len(input.len())?;
        let mut scratch = [self.kind.zero(); MAX_INSTRUCTIONS];
        let mut accumulator = self.kind.zero();
        let mut unsigned = Vec::new();
        let mut signed = Vec::new();
        let mut floats = Vec::new();
        match self.kind {
            NumericType::U32 => unsigned.reserve_exact(len),
            NumericType::I32 => signed.reserve_exact(len),
            NumericType::F32 => floats.reserve_exact(len),
        }
        for index in 0..input.len() {
            if index.is_multiple_of(256) {
                token.checkpoint()?;
            }
            let value = self.evaluate(input.scalar(index), &mut scratch)?;
            let value = match self.output {
                Output::Elements => value,
                Output::Sum => {
                    accumulator = binary(BinaryOp::Add, accumulator, value)?;
                    accumulator
                }
                Output::Min | Output::Max => {
                    accumulator = if index == 0 {
                        value
                    } else {
                        binary(
                            if self.output == Output::Min {
                                BinaryOp::Min
                            } else {
                                BinaryOp::Max
                            },
                            accumulator,
                            value,
                        )?
                    };
                    accumulator
                }
            };
            if self.output == Output::Elements {
                push(value, &mut unsigned, &mut signed, &mut floats);
            }
        }
        if self.output != Output::Elements {
            push(accumulator, &mut unsigned, &mut signed, &mut floats);
        }
        token.checkpoint()?;
        match self.kind {
            NumericType::U32 => NumericBuffer::u32(unsigned),
            NumericType::I32 => NumericBuffer::i32(signed),
            NumericType::F32 => NumericBuffer::f32(floats),
        }
    }
    fn evaluate(&self, input: Scalar, scratch: &mut [Scalar; MAX_INSTRUCTIONS]) -> Result<Scalar> {
        for (index, instruction) in self.instructions.iter().enumerate() {
            let operand = |operand| match operand {
                Operand::Input => input,
                Operand::Value(index) => scratch[index],
                Operand::Constant(value) => value,
            };
            scratch[index] = binary(
                instruction.op,
                operand(instruction.left),
                operand(instruction.right),
            )?;
        }
        Ok(if self.instructions.is_empty() {
            input
        } else {
            scratch[self.instructions.len() - 1]
        })
    }
    /// Generate only validated WGSL with explicit storage layout, fixed entry point and bounds.
    pub fn wgsl(&self) -> String {
        let kind = self.kind.wgsl();
        let mut source = format!(
            "struct Params {{ len: u32, pad0: u32, pad1: u32, pad2: u32 }};\n@group(0) @binding(0) var<storage, read> inputs: array<{kind}>;\n@group(0) @binding(1) var<storage, read_write> outputs: array<{kind}>;\n@group(0) @binding(2) var<uniform> params: Params;\nfn evaluate(index: u32) -> {kind} {{\nlet input = inputs[index];\n"
        );
        for (index, instruction) in self.instructions.iter().enumerate() {
            let operand = |value| match value {
                Operand::Input => "input".to_owned(),
                Operand::Value(i) => format!("v{i}"),
                Operand::Constant(value) => value.wgsl(),
            };
            let left = operand(instruction.left);
            let right = operand(instruction.right);
            let expression = match instruction.op {
                BinaryOp::Add => format!("({left} + {right})"),
                BinaryOp::Mul => format!("({left} * {right})"),
                BinaryOp::Min => format!("min({left}, {right})"),
                BinaryOp::Max => format!("max({left}, {right})"),
            };
            source.push_str(&format!("let v{index}: {kind} = {expression};\n"));
            if self.kind == NumericType::F32 {
                source.push_str(&format!(
                    "if (!(abs(v{index}) <= bitcast<f32>(2139095039u))) {{ return v{index}; }}\n"
                ));
            }
        }
        source.push_str(&format!("return {};\n}}\n@compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n", if self.instructions.is_empty() { "input".into() } else { format!("v{}", self.instructions.len() - 1) }));
        if self.output == Output::Elements {
            source.push_str(
                "if (id.x >= params.len) { return; }\noutputs[id.x] = evaluate(id.x);\n}",
            );
        } else {
            source.push_str(&format!("if (id.x != 0u) {{ return; }}\nvar accumulator: {kind} = {};\nfor (var i = 0u; i < params.len; i = i + 1u) {{\nlet value = evaluate(i);\n", self.kind.zero().wgsl()));
            if self.kind == NumericType::F32 {
                source.push_str("if (!(abs(value) <= bitcast<f32>(2139095039u))) { outputs[0] = value; return; }\n");
            }
            match self.output {
                Output::Sum => source.push_str("accumulator = accumulator + value;\n"),
                Output::Min | Output::Max => source.push_str(&format!("if (i == 0u) {{ accumulator = value; }} else {{ accumulator = {}(accumulator, value); }}\n", if self.output == Output::Min { "min" } else { "max" })),
                Output::Elements => unreachable!(),
            }
            if self.kind == NumericType::F32 {
                source.push_str("if (!(abs(accumulator) <= bitcast<f32>(2139095039u))) { outputs[0] = accumulator; return; }\n");
            }
            source.push_str("}\noutputs[0] = accumulator;\n}");
        }
        source
    }
}
fn push(value: Scalar, unsigned: &mut Vec<u32>, signed: &mut Vec<i32>, floats: &mut Vec<f32>) {
    match value {
        Scalar::U32(v) => unsigned.push(v),
        Scalar::I32(v) => signed.push(v),
        Scalar::F32(v) => floats.push(v),
    }
}
fn binary(op: BinaryOp, left: Scalar, right: Scalar) -> Result<Scalar> {
    let result = match (left, right) {
        (Scalar::U32(a), Scalar::U32(b)) => Scalar::U32(match op {
            BinaryOp::Add => a.wrapping_add(b),
            BinaryOp::Mul => a.wrapping_mul(b),
            BinaryOp::Min => a.min(b),
            BinaryOp::Max => a.max(b),
        }),
        (Scalar::I32(a), Scalar::I32(b)) => Scalar::I32(match op {
            BinaryOp::Add => a.wrapping_add(b),
            BinaryOp::Mul => a.wrapping_mul(b),
            BinaryOp::Min => a.min(b),
            BinaryOp::Max => a.max(b),
        }),
        (Scalar::F32(a), Scalar::F32(b)) => Scalar::F32(match op {
            BinaryOp::Add => a + b,
            BinaryOp::Mul => a * b,
            BinaryOp::Min => a.min(b),
            BinaryOp::Max => a.max(b),
        }),
        _ => return Err(Error::new("compute_type", "Numerical operands differ")),
    };
    result.validate()?;
    Ok(result)
}
fn nonfinite() -> Error {
    Error::new(
        "compute_nonfinite",
        "Compute supports only finite f32 values",
    )
}
fn check_len(len: usize) -> Result<()> {
    if len > MAX_ELEMENTS {
        Err(Error::new(
            "compute_buffer_limit",
            "Compute buffers allow at most 1,048,576 elements",
        ))
    } else {
        Ok(())
    }
}
