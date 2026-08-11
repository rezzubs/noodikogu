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
    /// Runs the management interface when no subcommand is given.
    #[command(subcommand)]
    command: Option<Commands>,
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

/// The level filter for both subscribers below, controlled by `RUST_LOG`
/// (e.g. `RUST_LOG=debug`). Defaults to `info` so the events every mutating
/// `Catalogue` method emits are recorded out of the box.
fn log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

/// Installs a tracing subscriber writing to stderr, for commands which print
/// their output normally.
fn init_stderr_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(std::io::stderr)
        .init();
}

/// Installs a tracing subscriber writing to `path`, creating the file and its
/// directory if needed.
///
/// Commands which take over the terminal need this: stderr is the same
/// terminal they are drawing to, so logging there would scribble over the
/// interface.
fn init_file_logging(path: &Path) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        // Colour escapes would just be noise in a file nothing renders.
        .with_ansi(false)
        .with_writer(file)
        .init();

    Ok(())
}

/// Where the terminal commands write their log, following the XDG base
/// directory specification.
fn log_path() -> PathBuf {
    let state_directory = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);

    state_directory.join("noodikogu").join("noodikogu.log")
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // The management interface and the query debugger both take over the
    // terminal, so their logs go to a file instead of landing on top of what
    // they draw.
    if matches!(cli.command, None | Some(Commands::DebugQuery)) {
        let path = log_path();
        if let Err(e) = init_file_logging(&path) {
            eprintln!("error: could not open log file {}: {e}", path.display());
            std::process::exit(1);
        }
    } else {
        init_stderr_logging();
    }

    match cli.command {
        None => noodikogu::tui::App::new().run().await,
        Some(Commands::DebugQuery) => {
            if let Err(e) = debug_query::run() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Import {
            csv_path,
            database_path,
        }) => {
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
