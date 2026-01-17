//! UFFS (Ultra Fast File Search) CLI
//!
//! Fast file search from the command line.
//!
//! ## Logging
//!
//! Logging is controlled by environment variables:
//! - `RUST_LOG`: Terminal log level (default: `error`)
//! - `RUST_LOG_FILE`: File log level (default: `info`)
//! - `UFFS_LOG_DIR`: Log directory (default: `~/bin/rust`)
//!
//! Examples:
//! ```bash
//! # Debug mode - verbose terminal output
//! RUST_LOG=debug uffs search *.txt
//!
//! # Trace mode - maximum verbosity
//! RUST_LOG=trace RUST_LOG_FILE=trace uffs search *.txt
//! ```

use std::io::stdout;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
// Memory allocator optimization for Windows - mimalloc reduces fragmentation
// and improves allocation performance for large datasets
#[cfg(target_os = "windows")]
use mimalloc::MiMalloc;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};

#[cfg(target_os = "windows")]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod commands;

/// UFFS - Ultra Fast File Search using direct MFT reading
#[derive(Parser)]
#[command(name = "uffs")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
#[command(args_conflicts_with_subcommands = true)]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Subcommand to execute (search, index, info, stats, save-raw, load-raw).
    #[command(subcommand)]
    command: Option<Commands>,

    /// Search pattern (glob, regex with `>`, or literal) - default action
    ///
    /// When no subcommand is specified, uffs performs a search.
    /// Examples:
    ///   `uffs *.txt`           - All .txt files
    ///   `uffs c:/pro*`         - Files starting with "pro" on C:
    ///   `uffs ">.*\.log$"`     - REGEX for .log files
    #[arg(value_name = "PATTERN")]
    pattern: Option<String>,

    /// Drive letter to search (e.g., C). Overrides drive in pattern.
    #[arg(short, long, conflicts_with = "drives")]
    drive: Option<char>,

    /// Multiple drive letters to search concurrently (e.g., C,D,E)
    #[arg(long, value_delimiter = ',', conflicts_with = "drive")]
    drives: Option<Vec<char>>,

    /// Use pre-built index file instead of live MFT
    #[arg(short, long, conflicts_with_all = ["drive", "drives"])]
    index: Option<PathBuf>,

    /// Show only files (exclude directories)
    #[arg(long)]
    files_only: bool,

    /// Show only directories
    #[arg(long)]
    dirs_only: bool,

    /// Minimum file size in bytes
    #[arg(long)]
    min_size: Option<u64>,

    /// Maximum file size in bytes
    #[arg(long)]
    max_size: Option<u64>,

    /// Maximum number of results
    #[arg(short = 'n', long, default_value = "100")]
    limit: u32,

    /// Output format: table, json, csv
    #[arg(short, long, default_value = "table")]
    format: String,

    /// Case-sensitive matching (default: off)
    #[arg(long, default_value = "false")]
    case: bool,

    /// Filter by file extension(s)
    #[arg(long)]
    ext: Option<String>,

    /// Output destination: console or filename
    #[arg(long, default_value = "console")]
    out: String,

    /// Columns to output (comma-separated or "all")
    #[arg(long, default_value = "all")]
    columns: String,

    /// Column separator (default: comma)
    #[arg(long, default_value = ",")]
    sep: String,

    /// Quote character for string values
    #[arg(long, default_value = "\"")]
    quotes: String,

    /// Include header row in output
    #[arg(long, default_value = "true")]
    header: bool,

    /// Representation for active/true boolean attributes
    #[arg(long, default_value = "1")]
    pos: String,

    /// Representation for inactive/false boolean attributes
    #[arg(long, default_value = "0")]
    neg: String,
}

/// Available CLI subcommands.
#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Search for files matching a pattern
    ///
    /// Supports multiple pattern syntaxes:
    /// - Glob: `*.txt`, `**/*.rs`, `file?.doc`
    /// - Drive prefix: `c:/pro*`, `D:\Users\*`
    /// - REGEX: `">C:\\Temp.*"` (patterns starting with `>`)
    /// - Literal: `readme` (no wildcards = substring match)
    Search {
        /// Search pattern (glob, regex with `>`, or literal)
        ///
        /// Examples:
        ///   `*.txt`           - All .txt files
        ///   `c:/pro*`         - Files starting with "pro" on C:
        ///   `**/*.rs`         - All .rs files recursively
        ///   `">.*\.log$"`     - REGEX for .log files
        pattern: String,

        /// Drive letter to search (e.g., C). Overrides drive in pattern.
        #[arg(short, long, conflicts_with = "drives")]
        drive: Option<char>,

        /// Multiple drive letters to search concurrently (e.g., C,D,E)
        #[arg(long, value_delimiter = ',', conflicts_with = "drive")]
        drives: Option<Vec<char>>,

        /// Use pre-built index file instead of live MFT
        #[arg(short, long, conflicts_with_all = ["drive", "drives"])]
        index: Option<PathBuf>,

        /// Show only files (exclude directories)
        #[arg(long)]
        files_only: bool,

        /// Show only directories
        #[arg(long)]
        dirs_only: bool,

        /// Minimum file size in bytes
        #[arg(long)]
        min_size: Option<u64>,

        /// Maximum file size in bytes
        #[arg(long)]
        max_size: Option<u64>,

        /// Maximum number of results
        #[arg(short = 'n', long, default_value = "100")]
        limit: u32,

        /// Output format: table, json, csv
        #[arg(short, long, default_value = "table")]
        format: String,

        /// Case-sensitive matching (default: off)
        #[arg(long, default_value = "false")]
        case: bool,

        /// Filter by file extension(s)
        ///
        /// Accepts comma-separated extensions or collection aliases:
        /// - Extensions: `jpg,png,gif`
        /// - Collections: `pictures`, `documents`, `videos`, `music`,
        ///   `archives`, `code`
        /// - Mixed: `pictures,mp4,pdf`
        #[arg(long)]
        ext: Option<String>,

        /// Output destination: console or filename
        ///
        /// Special values: console, con, term, terminal (all output to stdout)
        /// Otherwise treated as a file path to write results to.
        #[arg(long, default_value = "console")]
        out: String,

        /// Columns to output (comma-separated or "all")
        ///
        /// Available: path, name, pathonly, size, sizeondisk, created,
        /// modified, accessed, type, attributes, attributevalue,
        /// hidden, system, archive, readonly, compressed, encrypted,
        /// sparse, reparse, offline, notindexed, temporary, virtual,
        /// pinned, unpinned, descendants
        #[arg(long, default_value = "all")]
        columns: String,

        /// Column separator (default: comma)
        ///
        /// Special values: TAB, NEWLINE
        #[arg(long, default_value = ",")]
        sep: String,

        /// Quote character for string values
        #[arg(long, default_value = "\"")]
        quotes: String,

        /// Include header row in output
        #[arg(long, default_value = "true")]
        header: bool,

        /// Representation for active/true boolean attributes
        #[arg(long, default_value = "1")]
        pos: String,

        /// Representation for inactive/false boolean attributes
        #[arg(long, default_value = "0")]
        neg: String,
    },

    /// Build an index from a drive's MFT
    Index {
        /// Drive letter to index (e.g., C). Use --drives for multiple drives.
        #[arg(short, long, conflicts_with = "drives")]
        drive: Option<char>,

        /// Multiple drive letters to index concurrently (e.g., C,D,E)
        #[arg(long, value_delimiter = ',', conflicts_with = "drive")]
        drives: Option<Vec<char>>,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Show information about an index file
    Info {
        /// Index file path
        path: PathBuf,
    },

    /// Show statistics about files in an index
    Stats {
        /// Index file path
        #[arg(short, long)]
        index: PathBuf,

        /// Show top N largest files
        #[arg(long, default_value = "10")]
        top: u32,
    },

    /// Save raw MFT bytes to a file for offline analysis
    SaveRaw {
        /// Drive letter to read MFT from (e.g., C)
        #[arg(short, long)]
        drive: char,

        /// Output file path for raw MFT data
        #[arg(short, long)]
        output: PathBuf,

        /// Compress the output using zstd
        #[arg(short, long, default_value = "true")]
        compress: bool,

        /// Compression level (1-22, default 3)
        #[arg(long, default_value = "3")]
        compression_level: i32,
    },

    /// Load raw MFT from a saved file and export to parquet/csv
    LoadRaw {
        /// Input raw MFT file path
        input: PathBuf,

        /// Output file path (parquet or csv based on extension)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Show info about the raw MFT file only (don't parse)
        #[arg(long)]
        info_only: bool,
    },
}

/// Initialize logging with terminal + file support.
///
/// Terminal logging is controlled by `RUST_LOG` (default: `error`).
/// File logging is controlled by `RUST_LOG_FILE` (default: `info`).
/// Log directory is controlled by `UFFS_LOG_DIR` (default: `~/bin/rust`).
///
/// Returns a guard that must be kept alive for the duration of the program.
///
/// # Panics
///
/// Panics if the global tracing subscriber cannot be set (should only happen
/// if called more than once).
// Extracted for clarity and maintainability - logging setup is complex enough
// to warrant its own function even if only called once.
#[allow(clippy::single_call_fn)]
fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
    use std::fs;

    use tracing_appender::non_blocking::NonBlocking;
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::registry::Registry;

    // Get log directory (default: ~/bin/rust)
    let log_dir = std::env::var("UFFS_LOG_DIR").map_or_else(
        |_| {
            dirs_next::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("bin")
                .join("rust")
        },
        PathBuf::from,
    );

    // Create log directory if it doesn't exist (ignore errors - logging will fail
    // gracefully)
    drop(fs::create_dir_all(&log_dir));

    // Create rolling file appender (daily rotation)
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "uffs_log_");
    let (non_blocking, guard): (NonBlocking, _) = NonBlocking::new(file_appender);

    // Terminal filter (default: error - minimal output)
    let terminal_filter =
        EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| "error".to_owned()));

    // File filter (default: info - more verbose for debugging)
    let file_filter =
        EnvFilter::new(std::env::var("RUST_LOG_FILE").unwrap_or_else(|_| "info".to_owned()));

    // Timer format
    let timer = UtcTime::rfc_3339();

    // Terminal layer (with ANSI colors)
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(stdout)
        .with_timer(timer.clone())
        .with_ansi(true)
        .with_filter(terminal_filter);

    // File layer (no ANSI colors)
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_timer(timer)
        .with_ansi(false)
        .with_filter(file_filter);

    // Combine layers
    let subscriber = Registry::default().with(terminal_layer).with(file_layer);

    // This should only be called once at program startup
    #[allow(clippy::expect_used)]
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global tracing subscriber - was init_logging called twice?");

    guard
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    // Initialize logging with terminal + file support
    let _guard = init_logging();

    let cli = Cli::parse();

    // Handle default search (no subcommand) or explicit subcommand
    match cli.command {
        Some(Commands::Search {
            pattern,
            drive,
            drives,
            index,
            files_only,
            dirs_only,
            min_size,
            max_size,
            limit,
            format,
            case,
            ext,
            out,
            columns,
            sep,
            quotes,
            header,
            pos,
            neg,
        }) => {
            commands::search(
                &pattern,
                drive,
                drives,
                index,
                files_only,
                dirs_only,
                min_size,
                max_size,
                limit,
                &format,
                case,
                ext.as_deref(),
                &out,
                &columns,
                &sep,
                &quotes,
                header,
                &pos,
                &neg,
            )
            .await?;
        }
        Some(Commands::Index {
            drive,
            drives,
            output,
        }) => {
            commands::index(drive, drives, &output).await?;
        }
        Some(Commands::Info { path }) => {
            commands::info(&path)?;
        }
        Some(Commands::Stats { index, top }) => {
            commands::stats(&index, top)?;
        }
        Some(Commands::SaveRaw {
            drive,
            output,
            compress,
            compression_level,
        }) => {
            commands::save_raw(drive, &output, compress, compression_level).await?;
        }
        Some(Commands::LoadRaw {
            input,
            output,
            info_only,
        }) => {
            commands::load_raw(&input, output.as_deref(), info_only)?;
        }
        None => {
            // Default action: search with top-level arguments
            if let Some(pattern) = cli.pattern {
                commands::search(
                    &pattern,
                    cli.drive,
                    cli.drives,
                    cli.index,
                    cli.files_only,
                    cli.dirs_only,
                    cli.min_size,
                    cli.max_size,
                    cli.limit,
                    &cli.format,
                    cli.case,
                    cli.ext.as_deref(),
                    &cli.out,
                    &cli.columns,
                    &cli.sep,
                    &cli.quotes,
                    cli.header,
                    &cli.pos,
                    &cli.neg,
                )
                .await?;
            } else {
                // No pattern provided - show help
                use clap::CommandFactory;
                Cli::command().print_help()?;
            }
        }
    }

    Ok(())
}
