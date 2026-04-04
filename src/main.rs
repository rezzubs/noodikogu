mod debug_query;

use clap::{Parser, Subcommand};

/// Noodikogu score catalogue CLI.
#[derive(Debug, Parser)]
#[command(name = "noodikogu")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Interactively debug the query parser.
    DebugQuery,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::DebugQuery => {
            if let Err(e) = debug_query::run() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}
