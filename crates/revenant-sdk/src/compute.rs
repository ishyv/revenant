//! Explicit typed IR authoring; ordinary Rust closures are never converted into shaders.
//!
//! ```
//! use revenant_sdk::compute::{BinaryOp, KernelBuilder, Output};
//! let mut builder = KernelBuilder::<u32>::new().unwrap();
//! let input = builder.input();
//! let two = builder.constant(2).unwrap();
//! builder.binary(BinaryOp::Mul, input, two).unwrap();
//! let kernel = builder.finish(Output::Elements).unwrap();
//! assert_eq!(kernel.dispatch(65).unwrap(), [2, 1, 1]);
//! ```
//! Unsupported scalar domains receive ordinary source-local type diagnostics:
//! ```compile_fail
//! let accounting = revenant_sdk::compute::KernelBuilder::<u64>::new();
//! ```
//! ```compile_fail
//! let builder = revenant_sdk::compute::KernelBuilder::<u32>::new().unwrap();
//! builder.constant("filesystem path");
//! ```
use crate::{Error, Resource, Result, TaskContext, WorkClass};
pub use revenant_core::compute::{
    BinaryOp, BufferLayout, FLOAT_TOLERANCE, Instruction, Kernel, MAX_ELEMENTS, MAX_INSTRUCTIONS,
    NumericBuffer, NumericType, Operand, Output, Scalar,
};
use std::{
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

mod sealed {
    pub trait Sealed {}
    impl Sealed for u32 {}
    impl Sealed for i32 {}
    impl Sealed for f32 {}
}
/// Only u32, i32 and f32 can cross this explicitly numerical authoring boundary.
pub trait ComputeElement: sealed::Sealed + Copy {
    /// Compiled scalar domain.
    const KIND: NumericType;
    /// Encode one typed constant without converting its numerical domain.
    fn scalar(self) -> Scalar;
}
impl ComputeElement for u32 {
    const KIND: NumericType = NumericType::U32;
    fn scalar(self) -> Scalar {
        Scalar::U32(self)
    }
}
impl ComputeElement for i32 {
    const KIND: NumericType = NumericType::I32;
    fn scalar(self) -> Scalar {
        Scalar::I32(self)
    }
}
impl ComputeElement for f32 {
    const KIND: NumericType = NumericType::F32;
    fn scalar(self) -> Scalar {
        Scalar::F32(self)
    }
}

/// A typed expression admitted by one particular builder; it cannot express native effects.
#[derive(Clone, Copy, Debug)]
pub struct Expression<T> {
    operand: Operand,
    owner: u64,
    marker: PhantomData<T>,
}
/// Source-facing authoring of a bounded typed numerical DAG.
pub struct KernelBuilder<T: ComputeElement> {
    instructions: Vec<Instruction>,
    identity: u64,
    marker: PhantomData<T>,
}
impl<T: ComputeElement> KernelBuilder<T> {
    /// Start a distinct builder with a non-reused expression identity.
    pub fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let identity = NEXT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::new("compute_ir", "Kernel builder identity exhausted"))?;
        Ok(Self {
            instructions: Vec::new(),
            identity,
            marker: PhantomData,
        })
    }
    fn expression(&self, operand: Operand) -> Expression<T> {
        Expression {
            operand,
            owner: self.identity,
            marker: PhantomData,
        }
    }
    /// Current lane's source value in this builder's scalar domain.
    pub fn input(&self) -> Expression<T> {
        self.expression(Operand::Input)
    }
    /// A scalar constant with compile-time domain checks and runtime finite-float validation.
    pub fn constant(&self, value: T) -> Result<Expression<T>> {
        let scalar = value.scalar();
        if matches!(scalar, Scalar::F32(value) if !value.is_finite()) {
            return Err(Error::new(
                "compute_nonfinite",
                "Kernel constants must be finite",
            ));
        }
        Ok(self.expression(Operand::Constant(scalar)))
    }
    /// Append a pure binary instruction; foreign-builder values and excessive DAGs fail.
    pub fn binary(
        &mut self,
        op: BinaryOp,
        left: Expression<T>,
        right: Expression<T>,
    ) -> Result<Expression<T>> {
        if left.owner != self.identity || right.owner != self.identity {
            return Err(Error::new(
                "compute_ir",
                "Expressions must belong to this kernel builder",
            ));
        }
        if self.instructions.len() >= MAX_INSTRUCTIONS {
            return Err(Error::new(
                "compute_ir",
                "Kernel exceeds thirty-two instructions",
            ));
        }
        let index = self.instructions.len();
        self.instructions.push(Instruction {
            op,
            left: left.operand,
            right: right.operand,
        });
        Ok(self.expression(Operand::Value(index)))
    }
    /// Compile the final instruction (or input for an empty DAG) with an explicit output shape.
    pub fn finish(self, output: Output) -> Result<Kernel> {
        Kernel::new(T::KIND, self.instructions, output)
    }
}
/// Run through bounded host CPU admission, retain the input lease, then publish an owned result.
/// CPU-only builds have no graphics dependency; unavailable work services remain an explicit error.
pub fn run_cpu(
    context: &TaskContext,
    kernel: &Kernel,
    input: &Resource<NumericBuffer>,
) -> Result<Resource<NumericBuffer>> {
    let input = context.lease_resource(input)?;
    let kernel = kernel.clone();
    let mut results =
        context
            .work()?
            .map_ordered(WorkClass::Cpu, [input], move |input, token| {
                kernel.execute_cpu(&input, &token)
            })?;
    context.checkpoint()?;
    context.own(results.pop().ok_or_else(|| {
        Error::new(
            "compute_incomplete",
            "CPU work returned no numerical result",
        )
    })?)
}
