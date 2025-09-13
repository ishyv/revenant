//! Revenant CLI entrypoint — thin wrapper that defers to the `cli` module.
//!
//! This keeps the crate root tidy and makes subcommands and orchestration
//! easier to document and test in isolation.

mod data;
mod commands;
mod consts;
mod package_manager;
mod fs_util;
mod macros;
mod rusty_utils;
mod svelte;
mod setup;
mod templates;
mod cli;

fn main() {
    cli::run();
}
