//! CLI definitions and command dispatch.
//!
//! Keeps `main.rs` minimal and documents each subcommand clearly.

use clap::{Args as ClapArgs, Parser, Subcommand};
use std::path::PathBuf;

use crate::consts;
use crate::data::AppContext;
use crate::setup::build_setup_pipeline;
use crate::svelte;

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
        /// If true, continue after a failed step.
        #[arg(short, long, default_value_t = true)]
        skip_errors: bool,
    },

    /// Run a specific setup step by name.
    Step { name: String },

    /// Launch the Svelte dev server under Revenant's control.
    Dev(DevArgs),

    /// Build the Svelte app (production).
    Build,

    /// Preview the production build with Vite's preview server.
    Preview(PreviewArgs),
}

#[derive(ClapArgs, Debug, Clone)]
pub struct DevArgs {
    /// Bind to all interfaces (Vite --host)
    #[arg(long, default_value_t = false)]
    pub host: bool,

    /// Dev server port (Vite --port)
    #[arg(long)]
    pub port: Option<u16>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct PreviewArgs {
    /// Preview server port (Vite --port)
    #[arg(long)]
    pub port: Option<u16>,
}

/// Parses CLI args and executes the selected command.
pub fn run() {
    let args = Args::parse();
    let app_context = AppContext::new(PathBuf::from(consts::DEFAULT_ROOT));

    let setup_pipeline = build_setup_pipeline(&app_context);
    let mut pipeline_iter = setup_pipeline.0.iter().zip(setup_pipeline.1.iter());

    match args.command.unwrap_or(Commands::Full { skip_errors: true }) {
        Commands::Full { skip_errors } => {
            for (step_name, step_fn) in pipeline_iter {
                println!("[ * ] Running step: {}", step_name);
                if let Err(e) = step_fn(&app_context) {
                    eprintln!("[ ! ] Step '{}' failed: {}", step_name, e);
                    if !skip_errors { return; }
                }
                println!("[ + ] Step '{}' completed successfully.", step_name);
            }
            println!("[ + ] Full setup completed successfully.");
        }

        Commands::Step { name } => {
            if let Some((_, step_fn)) = pipeline_iter.find(|(n, _)| *n == &name) {
                println!("[ * ] Running step: {}", name);
                if let Err(e) = step_fn(&app_context) {
                    eprintln!("[ ! ] Step '{}' failed: {}", name, e);
                } else {
                    println!("[ + ] Step '{}' completed successfully.", name);
                }
            } else {
                eprintln!("[ ! ] No such step: '{}'. Available steps are:", name);
                for (step_name, _) in setup_pipeline.0.iter().zip(setup_pipeline.1.iter()) {
                    eprintln!("    - {}", step_name);
                }
            }
        }

        Commands::Dev(dev_args) => {
            if let Err(e) = svelte::ensure_svelte_exists(&app_context) {
                eprintln!("[ ! ] {}", e);
                eprintln!("Run 'revenant full' first to scaffold the project.");
                return;
            }
            let mut extra: Vec<String> = Vec::new();
            if let Some(port) = dev_args.port { extra.extend(["--".into(), "--port".into(), port.to_string()]); }
            if dev_args.host { extra.extend(["--".into(), "--host".into()]); }
            if let Err(e) = svelte::run_npm_script_streaming(&app_context, "dev", &extra) {
                eprintln!("[ ! ] Dev server failed: {}", e);
            }
        }

        Commands::Build => {
            if let Err(e) = svelte::ensure_svelte_exists(&app_context) {
                eprintln!("[ ! ] {}", e);
                eprintln!("Run 'revenant full' first to scaffold the project.");
                return;
            }
            match svelte::run_npm_script_capture(&app_context, "build", &[]) {
                Ok(_) => println!("[ + ] Build completed."),
                Err(e) => eprintln!("[ ! ] Build failed: {}", e),
            }
        }

        Commands::Preview(preview_args) => {
            if let Err(e) = svelte::ensure_svelte_exists(&app_context) {
                eprintln!("[ ! ] {}", e);
                eprintln!("Run 'revenant full' first to scaffold the project.");
                return;
            }
            let mut extra: Vec<String> = Vec::new();
            if let Some(port) = preview_args.port { extra.extend(["--".into(), "--port".into(), port.to_string()]); }
            if let Err(e) = svelte::run_npm_script_streaming(&app_context, "preview", &extra) {
                eprintln!("[ ! ] Preview server failed: {}", e);
            }
        }
    }
}

