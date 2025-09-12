//! Revenant library crate.
//!
//! Currently exposes the compiler pipeline used by the `revenantc` binary.
//! Keeping the pipeline in a library lets us test passes in isolation and
//! reuse transforms in other binaries/tools if needed.

pub mod compiler;
