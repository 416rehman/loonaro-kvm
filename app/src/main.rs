//! loonaro - KVM introspection toolkit

use clap::{Parser, Subcommand};
use loonaro_vmi::cli::VmiArgs;

mod commands;

#[derive(Parser)]
#[command(author, version, about = "KVM introspection toolkit")]
struct Cli {
    #[command(flatten)]
    vmi: VmiArgs,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// list running processes
    ListProcesses,
    /// monitor process creation
    Monitor,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::ListProcesses => commands::list_processes::run(&cli.vmi)?,
        Commands::Monitor => commands::monitor::run(&cli.vmi)?,
    };

    Ok(())
}
