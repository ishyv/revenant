//! Revenant Compiler CLI (`revenantc`).
//!
//! Reads a JSON `TransformInput` on stdin, applies the configured pipeline of
//! passes, and writes a `TransformOutput` on stdout. Intended to be spawned by
//! the Svelte preprocessor wrapper.

use std::io::{self, Read};

use revenant::compiler::{
    passes::{DemoUppercasePass, NoopPass},
    Pipeline, TransformInput,
};

fn main() {
    // Read JSON payload from stdin
    let mut buf = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut buf) {
        eprintln!("failed to read stdin: {}", e);
        std::process::exit(1);
    }

    let input: TransformInput = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("invalid input JSON: {}", e);
            std::process::exit(2);
        }
    };

    // Build a tiny pipeline: noop -> demo_uppercase
    let pipeline = Pipeline::new()
        .with_pass(Box::new(NoopPass))
        .with_pass(Box::new(DemoUppercasePass));

    let out = pipeline.run(&input);
    match serde_json::to_string(&out) {
        Ok(s) => println!("{}", s),
        Err(e) => {
            eprintln!("failed to encode output JSON: {}", e);
            std::process::exit(3);
        }
    }
}
