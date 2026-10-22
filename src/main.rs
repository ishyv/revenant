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
    /// Inspect source, generated contracts and running instances
    Inspect {
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Run predefined checks and return current structured diagnostics
    Check {
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Serve development tools over MCP stdio
    Mcp {
        #[arg(long)]
        project: std::path::PathBuf,
    },
    /// Inspect, back up, replace or restore an application's SDK snapshot
    Sdk {
        #[command(subcommand)]
        command: SdkCommands,
    },
}

#[derive(Subcommand)]
enum SdkCommands {
    /// Check SDK sources and the installed client without changing files
    Check,
    /// Back up the current SDK and validate a replacement before committing it
    Upgrade,
    /// Restore an application-local backup; preserve displaced state for redo
    Restore {
        /// Backup directory returned by sdk upgrade or sdk restore
        backup: std::path::PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::New { ref name } => commands::new::run(name, cli.verbose),
        Commands::Dev => commands::dev::run(cli.verbose),
        Commands::Build => commands::build::run(cli.verbose),
        Commands::Setup => commands::setup::run(),
        Commands::Inspect { project, json } => commands::inspect::run(project.as_deref(), json),
        Commands::Check { project, json } => commands::check::run(project.as_deref(), json),
        Commands::Mcp { project } => commands::mcp::run(&project),
        Commands::Sdk {
            command: SdkCommands::Check,
        } => commands::sdk::check(),
        Commands::Sdk {
            command: SdkCommands::Upgrade,
        } => commands::sdk::upgrade(cli.verbose),
        Commands::Sdk {
            command: SdkCommands::Restore { backup },
        } => commands::sdk::restore(&backup),
    };

    if let Err(err) = result {
        eprintln!("\n  {} {}", "✗".red().bold(), err);
        for cause in err.chain().skip(1) {
            eprintln!("    {} {}", "caused by:".dimmed(), cause);
        }
        std::process::exit(1);
    }
}
