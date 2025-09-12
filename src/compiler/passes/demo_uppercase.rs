//! Demo pass used to validate the pipeline end-to-end.
//!
//! Not intended for production usage. It shows how to:
//! - Inspect the `kind` to scope to markup only
//! - Replace custom syntax with standard content
//! - Emit developer diagnostics

use crate::compiler::{Diagnostic, Pass, TransformInput};
use regex::Regex;

/// A tiny demo transform that proves the pipeline works without touching real Svelte syntax.
///
/// It recognizes self-closing tags of the form:
///   <rv:upper text="hello world" />
/// and replaces them with the UPPERCASED text literal:
///   HELLO WORLD
///
/// This is intentionally simplistic and safe to run as a first pass.
pub struct DemoUppercasePass;

impl Pass for DemoUppercasePass {
    fn name(&self) -> &'static str { "demo_uppercase" }

    fn run(&self, input: &TransformInput, code: &str) -> (String, Vec<Diagnostic>) {
        if !matches!(input.kind, crate::compiler::TransformKind::Markup) {
            return (code.to_string(), vec![]);
        }

        // very small regex to capture text attr; not a general HTML parser on purpose
        let re = Regex::new(r#"<rv:upper\s+text=\"([^\"]*)\"\s*/>"#).unwrap();
        let mut diags = Vec::new();

        let result = re.replace_all(code, |caps: &regex::Captures| {
            let txt = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let upper = txt.to_uppercase();
            diags.push(Diagnostic { level: "info".into(), message: format!("demo_uppercase: replaced '{}' with '{}'", txt, &upper) });
            upper
        });

        (result.into_owned(), diags)
    }
}
