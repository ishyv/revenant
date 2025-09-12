//! A do-nothing pass used as a baseline and to illustrate the `Pass` API.
//! Keeping a cheap first stage can help with debugging and future metrics.

use crate::compiler::{Diagnostic, Pass, TransformInput};

pub struct NoopPass;

impl Pass for NoopPass {
    fn name(&self) -> &'static str { "noop" }

    fn run(&self, _input: &TransformInput, code: &str) -> (String, Vec<Diagnostic>) {
        (code.to_string(), vec![])
    }
}
