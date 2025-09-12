//! Revenant compiler core.
//!
//! This module defines a small, composable transform pipeline that operates on
//! Svelte preprocess inputs (markup/script/style). The `revenant-preprocess.js`
//! wrapper sends JSON to the `revenantc` binary that uses this module.
//!
//! The focus is on clarity and evolvability:
//! - A `Pass` trait for small, testable transforms
//! - A plain `Pipeline` that chains passes
//! - A stable JSON protocol for input/output

use serde::{Deserialize, Serialize};

pub mod passes;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// JSON payload received from the Svelte preprocessor.
pub struct TransformInput {
    pub version: u32,
    pub path: String,
    pub kind: TransformKind,
    pub source: String,
    #[serde(default)]
    pub options: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Which Svelte preprocessor hook invoked the compiler.
pub enum TransformKind {
    Markup,
    Script,
    Style,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Developer-facing diagnostics (displayed by the Node wrapper in dev).
pub struct Diagnostic {
    pub level: String,  // "info" | "warn" | "error"
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// JSON payload produced for the Svelte preprocessor.
pub struct TransformOutput {
    pub code: String,
    #[serde(default)]
    pub map: Option<serde_json::Value>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// A single transformation stage.
pub trait Pass: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, input: &TransformInput, code: &str) -> (String, Vec<Diagnostic>);
}

/// Sequential list of passes. Each pass receives the output of the previous.
pub struct Pipeline {
    passes: Vec<Box<dyn Pass>>, 
}

impl Pipeline {
    pub fn new() -> Self {
        Self { passes: Vec::new() }
    }

    /// Adds a pass to the end of the pipeline.
    pub fn with_pass(mut self, pass: Box<dyn Pass>) -> Self {
        self.passes.push(pass);
        self
    }

    /// Executes the pipeline, returning the transformed code and any diagnostics.
    pub fn run(&self, input: &TransformInput) -> TransformOutput {
        let mut code = input.source.clone();
        let mut diags: Vec<Diagnostic> = Vec::new();

        for pass in &self.passes {
            let (next_code, mut pass_diags) = pass.run(input, &code);
            code = next_code;
            diags.append(&mut pass_diags);
        }

        TransformOutput { code, map: None, diagnostics: diags }
    }
}
