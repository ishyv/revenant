//! Setup pipeline for scaffolding and preparing the Svelte project.
//!
//! Provides a sequence of named steps with a uniform signature so the CLI can
//! run them all or select one by name.

use std::io;

// use glob::glob; // no file transforms for now

use crate::consts;
use crate::data::AppContext;
use crate::fs_util;
use crate::package_manager::verify_and_install_packages;
use crate::rusty_utils::IoResultDialog;
use crate::{named_function_vec, svelte, cmd};

/// Common type for any step closure or function.
pub type Step<'program> = Box<dyn (Fn(&AppContext) -> io::Result<()>) + Send + Sync + 'program>;

/// Builds the setup pipeline as the direct output of `named_function_vec!`.
pub fn build_setup_pipeline<'program>(
    app_context: &'program AppContext,
) -> (Vec<String>, Vec<Step<'program>>) {
    named_function_vec![
        // create-root — ensures output root exists, clears Svelte dir to avoid prompts
        create_root: |ctx: &AppContext| -> io::Result<()> {
            if !ctx.root_path.exists() {
                std::fs::create_dir_all(&ctx.root_path)
                    .dialog("Failed to create root directory while setting up")?;
            }
            if app_context.svelte_path.exists() {
                fs_util::force_delete_dir(&app_context.svelte_path)
                    .dialog("Failed to clear existing Svelte project while setting up")?;
            }
            Ok(())
        },

        // verify-packages — checks required external tools
        verify_packages: |ctx: &AppContext| -> io::Result<()> {
            verify_and_install_packages(ctx);
            Ok(())
        },

        // setup-svelte — scaffold project, install deps, initial build
        setup_svelte: |_: &AppContext| -> io::Result<()> {
            // Ensure `sv` is available (lightweight wrapper)
            cmd!("npm install -g sv");

            // Scaffold project
            cmd!(&format!(
                "npx sv create {} --template minimal --types ts --no-add-ons",
                app_context.svelte_path.to_string_lossy()
            ));

            // Read defaults.json
            let defaults = {
                let data = std::fs::read_to_string(consts::NPM_DEFAULTS)
                    .dialog("Failed to read defaults.json while loading standard dependencies")?;
                serde_json::from_str::<serde_json::Value>(&data)
                    .expect("Failed to parse defaults.json while loading standard dependencies")
            };

            // Install baseline deps
            cmd!(&format!("cd {} && npm install", app_context.svelte_path.to_string_lossy()));

            // Install additional deps from defaults
            if let Some(deps) = defaults.get("dependencies").and_then(|v| v.as_array()) {
                let deps: Vec<&str> = deps.iter().filter_map(|v| v.as_str()).collect();
                if !deps.is_empty() {
                    let install_cmd = format!("npm install {}", deps.join(" "));
                    cmd!(&format!("cd {} && {}", app_context.svelte_path.to_string_lossy(), install_cmd));
                }
            }

            // Copy template files into project
            fs_util::copy_dir(consts::DEFAULT_SVELTE_PROJECT, &app_context.svelte_path)?;

            // Initial build
            cmd!(&format!("cd {} && npm run build", app_context.svelte_path.to_string_lossy()));

            Ok(())
        },

        // inject-globals — expose a TS global `Revevant`
        inject_globals: |ctx: &AppContext| -> io::Result<()> {
            svelte::inject_globals(ctx)
        },

        // compiler step removed: no in-place transformations until syntax is formalized
    ]
}
