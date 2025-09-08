mod data;
mod commands;
mod consts;
mod package_manager;

use std::path::PathBuf;
use crate::{data::AppContext, package_manager::verify_and_install_packages};

/// Creates the necessary folders/files for the App to run.
fn setup(app_context: &AppContext) -> std::io::Result<()> {
    // If the output path doesn't exist, create it
    if !app_context.root_path.exists() {
        std::fs::create_dir_all(&app_context.root_path)?;
    }

    verify_and_install_packages(app_context);


    // * Set up Svelte

    // Clear /root/project_name if it exists to skip prompts.
    if app_context.svelte_path.exists() {
        std::fs::remove_dir_all(&app_context.svelte_path)?;
    }

    cmd!(&format!("npx sv create {} --template minimal --types ts --install npm --no-add-ons", app_context.svelte_path.to_string_lossy()));


    // Everything went fine
    Ok(())
}

fn main() {
    let app_context = AppContext::new(PathBuf::from(consts::DEFAULT_ROOT));

    if let Err(e) = setup(&app_context) {
        eprintln!("Error during setup: {}", e);
        std::process::exit(1);
    }
}
