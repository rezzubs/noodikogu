#![cfg_attr(
    test,
    allow(clippy::unwrap_used, reason = "unwrap is fine in test code")
)]

mod debug_query;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

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
    /// Bulk-import scores from a CSV file into a catalogue database.
    ///
    /// Meant for bootstrapping a new catalogue. Does not handle merge
    /// conflicts, adds everything as a new score.
    Import {
        /// Path to the CSV file to import.
        csv_path: PathBuf,
        /// Path to the catalogue database to import into (created if it doesn't exist).
        #[arg(long = "database", value_name = "PATH")]
        database_path: PathBuf,
    },
}

/// Installs a stderr tracing subscriber so the `tracing::info!` events
/// already emitted by every mutating `Catalogue` method are visible.
/// Level is controlled by `RUST_LOG` (e.g. `RUST_LOG=debug`), defaulting to
/// `info` so catalogue modifications are logged out of the box.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

#[tokio::main]
async fn main() {
    init_logging();
    let cli = Cli::parse();
    match cli.command {
        Commands::DebugQuery => {
            if let Err(e) = debug_query::run() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Import {
            csv_path,
            database_path,
        } => {
            if let Err(e) = run_import_csv(&csv_path, &database_path).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Reads `csv_path`, opens (creating it if necessary) the catalogue
/// database at `database_path`, and imports every row.
async fn run_import_csv(
    csv_path: &Path,
    database_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = tokio::fs::read_to_string(csv_path).await?;
    let database_path = database_path
        .to_str()
        .ok_or("database path must be valid UTF-8")?;
    let catalogue = noodikogu::catalogue::Catalogue::open(database_path).await?;
    let imported = noodikogu::import::import_csv(&catalogue, &text).await?;
    println!("imported {imported} score(s)");
    Ok(())
}
