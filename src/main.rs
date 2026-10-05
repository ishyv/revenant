use clap::{Parser, Subcommand};
use colored::Colorize;

use revenant::commands;

#[derive(Parser)]
#[command(
    name = "revenant",
    version,
    about = "Native desktop applications with Rust and Svelte"
)]
struct Cli {
    /// Show full child process output
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new Revenant project
    New {
        /// Project name
        name: String,
    },
    /// Launch the desktop window with Vite HMR and Rust rebuilds
    Dev,
    /// Compile native contracts, build the SPA, and package a desktop application
    Build,
    /// Check and guide installation of required tools
    #[command(alias = "diagnose")]
    Setup,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::New { ref name } => commands::new::run(name, cli.verbose),
        Commands::Dev => commands::dev::run(cli.verbose),
        Commands::Build => commands::build::run(cli.verbose),
        Commands::Setup => commands::setup::run(),
    };

    if let Err(err) = result {
        eprintln!("\n  {} {}", "✗".red().bold(), err);
        for cause in err.chain().skip(1) {
            eprintln!("    {} {}", "caused by:".dimmed(), cause);
        }
        std::process::exit(1);
    }
}
