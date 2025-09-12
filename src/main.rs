//! Revenant CLI: orchestrates Svelte project setup and provides a place to grow
//! additional workflows. The actual Svelte syntax transforms live in a separate
//! `revenantc` binary (see `src/bin/revenantc.rs`) to keep responsibilities clear.

mod data;
mod commands;
mod consts;
mod package_manager;
mod fs_util;
mod macros;
mod rusty_utils;

use std::{io, path::PathBuf};
use crate::{data::AppContext, package_manager::verify_and_install_packages, rusty_utils::IoResultDialog};
use clap::{ Parser, Subcommand };

/// Entrypoint for the `revenant` CLI.
#[derive(Parser)]
#[command(version = "1", about = "Revenant CLI", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Full setup: generate/prepare the Svelte project and sanity-build it.
    Full {
        /// * If true, will skip errors and continue with the next step.
        /// ! Use with caution; may leave the setup in a partial state.
        #[arg(short, long, default_value_t = true)]
        skip_errors: bool,
    },
}

// Common type for any step closure or function.
type Step<'program> = Box<dyn (Fn(&AppContext) -> io::Result<()>) + Send + Sync + 'program>;

/// Builds the setup pipeline as the direct output of `named_function_vec!`.
///
/// Returns:
/// - `Vec<String>`: step names (as produced by the macro).
/// - `Vec<Step<'program>>`: boxed callables with a uniform signature.
///
/// Notes:
/// - Closures intentionally capture `app_context` by reference so they can use
///   precomputed paths/config without threading them through every call.
/// - The value vector’s type is fixed explicitly via the `@type` arm.
/// - All original commands (including `cmd!`) are preserved.
fn build_setup_pipeline<'program>(
    app_context: &'program AppContext
) -> (Vec<String>, Vec<Step<'program>>) {
    named_function_vec![
        // "create-root" — creates the root directory if it doesn't exist,
        // then clears the Svelte project dir if present (to avoid prompts).
        create_root: |ctx: &AppContext| -> io::Result<()> {
            // Ensure root exists
            if !ctx.root_path.exists() {
                std::fs::create_dir_all(&ctx.root_path)
                    .dialog("Failed to create root directory while setting up")?;
            }

            // Clear /root/project_name if it exists to skip prompts.
            if app_context.svelte_path.exists() {
                fs_util::force_delete_dir(&app_context.svelte_path)
                    .dialog("Failed to clear existing Svelte project while setting up")?;
            }

            Ok(())
        },

        // "verify-packages" — verifies and installs required packages.
        verify_packages: |ctx: &AppContext| -> io::Result<()> {
            verify_and_install_packages(ctx);
            Ok(())
        },

        // "setup-svelte" — generate project, add defaults, copy files, initial build.
        setup_svelte: |_: &AppContext| -> io::Result<()> {
            // Generate a fresh Svelte skeleton using Vite’s stable template (no extra global tools required)
            cmd!(&format!(
                "npm create vite@latest {} -- --template svelte-ts",
                app_context.svelte_path.to_string_lossy()
            ));

            // Load standard dependencies from defaults JSON
            let defaults = {
                let data = std::fs::read_to_string(consts::NPM_DEFAULTS)
                    .dialog("Failed to read defaults.json while loading standard dependencies")?;
                
                serde_json::from_str::<serde_json::Value>(&data)
                    .expect("Failed to parse defaults.json while loading standard dependencies")
            };

            // Install any default deps listed
            if let Some(deps) = defaults.get("dependencies").and_then(|v| v.as_array()) {
                let deps: Vec<&str> = deps.iter().filter_map(|v| v.as_str()).collect();
                if !deps.is_empty() {
                    let install_cmd = format!("npm install {}", deps.join(" "));
                    cmd!(&format!(
                        "cd {} && {}",
                        app_context.svelte_path.to_string_lossy(),
                        install_cmd
                    ));
                }
            }

            // Copy default Svelte files into the new project
            fs_util::copy_dir(consts::DEFAULT_SVELTE_PROJECT, &app_context.svelte_path)?;

            // Sanity build
            cmd!(&format!("cd {} && npm run build", app_context.svelte_path.to_string_lossy()));

            Ok(())
        },
    ]
}

pub fn main() {
    let args = Args::parse();
    let app_context = AppContext::new(PathBuf::from(consts::DEFAULT_ROOT));

    // Build the setup pipeline.
    // The pipeline is a sequence of named steps, each represented by a closure or function.
    // Each step takes a reference to `AppContext` and returns `io::Result<()>`.
    // This allows flexible error handling and reporting.
    // And further expansion by adding more steps as needed.
    let setup_pipeline = build_setup_pipeline(&app_context);
    let pipeline_iter = setup_pipeline.0.iter().zip(setup_pipeline.1.iter());

    match args.command.unwrap_or(Commands::Full { skip_errors: true }) {
        Commands::Full { skip_errors } => {
            for (step_name, step_fn) in pipeline_iter {
                println!("[ * ] Running step: {}", step_name);

                if let Err(e) = step_fn(&app_context) {
                    eprintln!("[ ! ] Step '{}' failed: {}", step_name, e);
                    if !skip_errors {
                        return;
                    }
                }

                println!("[ + ] Step '{}' completed successfully.", step_name);
            }

            println!("[ + ] Full setup completed successfully.");
        }
    }
}



