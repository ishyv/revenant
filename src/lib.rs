//! Desktop application tooling for Revenant 0.3.
//!
//! The CLI owns the generated host under `.revenant/desktop`, its process tree,
//! and compiled-contract publication. Authors own `revenant.toml`, `web/`, and
//! optionally a `native/` Rust library. The running executable exports contracts.
pub mod commands;
pub mod compiled;
pub mod config;
pub mod errors;
pub mod scaffold;
pub mod toolchain;
