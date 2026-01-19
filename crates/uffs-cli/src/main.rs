//! UFFS (Ultra Fast File Search) CLI
//!
//! Fast file search from the command line.
//!
//! ## Logging
//!
//! Use `-v` / `--verbose` for info-level terminal output:
//! ```bash
//! uffs -v search *.txt
//! ```
//!
//! For finer control, use environment variables:
//! - `RUST_LOG`: Terminal log level (default: `error`, or `info` with `-v`)
//! - `RUST_LOG_FILE`: File log level (default: `info`)
//! - `UFFS_LOG_DIR`: Log directory (default: `~/bin/uffs/logs`)
//!
//! Examples:
//! ```bash
//! # Debug mode - verbose terminal output
//! RUST_LOG=debug uffs search *.txt
//!
//! # Trace mode - maximum verbosity
//! RUST_LOG=trace RUST_LOG_FILE=trace uffs search *.txt
//! ```

// Dependencies used in commands.rs for streaming output (Windows-only code
// paths)
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
use {chrono as _, uffs_polars as _};

#[cfg(target_os = "windows")]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod commands;

/// Parse a drive letter from various formats for CPP compatibility.
///
/// Accepts:
/// - Single letter: `C`, `c`
/// - With colon: `C:`, `c:`
///
/// Returns uppercase drive letter.
fn parse_drive_letter(input: &str) -> Result<char, String> {
    let trimmed = input.trim();
    // Strip trailing colon if present (CPP compatibility: "C:" -> "C")
    let letter_str = trimmed.strip_suffix(':').unwrap_or(trimmed);

    if letter_str.len() != 1 {
        return Err(format!(
            "Invalid drive letter '{input}': expected single letter like 'C' or 'C:'"
        ));
    }

    let ch = letter_str
        .chars()
        .next()
        .ok_or_else(|| format!("Invalid drive letter '{input}'"))?;

    if !ch.is_ascii_alphabetic() {
        return Err(format!("Invalid drive letter '{input}': must be A-Z"));
    }

    Ok(ch.to_ascii_uppercase())
}

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

    /// Drive letter to search (e.g., C or C:). Overrides drive in pattern.
    #[arg(short, long, conflicts_with = "drives", value_parser = parse_drive_letter)]
    drive: Option<char>,

    /// Multiple drive letters to search concurrently (e.g., C,D,E or C:,D:,E:)
    #[arg(long, value_delimiter = ',', conflicts_with = "drive", value_parser = parse_drive_letter)]
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

        /// Drive letter to search (e.g., C or C:). Overrides drive in pattern.
        #[arg(short, long, conflicts_with = "drives", value_parser = parse_drive_letter)]
        drive: Option<char>,

        /// Multiple drive letters to search concurrently (e.g., C,D,E or
        /// C:,D:,E:)
        #[arg(long, value_delimiter = ',', conflicts_with = "drive", value_parser = parse_drive_letter)]
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

        /// Maximum number of results (0 = unlimited)
        #[arg(short = 'n', long, default_value = "0")]
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

    /// Build an index from drive MFT(s)
    ///
    /// By default, indexes ALL available NTFS drives. Use --drive or --drives
    /// to limit to specific drives.
    ///
    /// If no extension is provided, defaults to `.parquet`.
    ///
    /// Examples:
    ///   uffs index index.parquet           # Index ALL drives
    ///   uffs index -d C index.parquet      # Index only C: drive
    ///   uffs index --drives C,D,E out.parquet  # Index C:, D:, E:
    ///   uffs index myindex                 # Creates myindex.parquet
    Index {
        /// Output file path (extension defaults to .parquet)
        output: PathBuf,

        /// Drive letter to index (limits to single drive)
        #[arg(short, long, conflicts_with = "drives", value_parser = parse_drive_letter)]
        drive: Option<char>,

        /// Multiple drive letters to index (e.g., C,D,E)
        #[arg(long, value_delimiter = ',', conflicts_with = "drive", value_parser = parse_drive_letter)]
        drives: Option<Vec<char>>,
    },

    /// Show information about an index file
    Info {
        /// Index file path
        path: PathBuf,
    },

    /// Show statistics about files in an index
    Stats {
        /// Index file path
        path: PathBuf,

        /// Show top N largest files
        #[arg(long, default_value = "10")]
        top: u32,
    },
}

/// Initialize logging with terminal + file support.
///
/// If `verbose` is true and `RUST_LOG` is not set, uses `info` level for
/// terminal. Otherwise, terminal logging is controlled by `RUST_LOG` (default:
/// `error`). File logging is controlled by `RUST_LOG_FILE` (default: `info`).
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
fn init_logging(verbose: bool) -> tracing_appender::non_blocking::WorkerGuard {
    use std::fs;

    use tracing_appender::non_blocking::NonBlocking;
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::registry::Registry;

    // Get log directory (default: ~/bin/uffs/logs)
    let log_dir = std::env::var("UFFS_LOG_DIR").map_or_else(
        |_| {
            dirs_next::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("bin")
                .join("uffs")
                .join("logs")
        },
        PathBuf::from,
    );

    // Create log directory if it doesn't exist (ignore errors - logging will fail
    // gracefully)
    drop(fs::create_dir_all(&log_dir));

    // Create rolling file appender (daily rotation)
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "uffs_log_");
    let (non_blocking, guard): (NonBlocking, _) = NonBlocking::new(file_appender);

    // Terminal filter: -v sets info if RUST_LOG not explicitly set
    let terminal_default = if verbose { "info" } else { "error" };
    let terminal_filter =
        EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| terminal_default.to_owned()));

    // File filter (default: info - more verbose for debugging)
    let file_filter =
        EnvFilter::new(std::env::var("RUST_LOG_FILE").unwrap_or_else(|_| "info".to_owned()));

    // Timer format
    let timer = UtcTime::rfc_3339();

    // Terminal layer (with ANSI colors, file/line info, thread IDs)
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(stdout)
        .with_timer(timer.clone())
        .with_ansi(true)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .with_filter(terminal_filter);

    // File layer (no ANSI colors, but with full context)
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_timer(timer)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .with_filter(file_filter);

    // Combine layers
    let subscriber = Registry::default().with(terminal_layer).with(file_layer);

    // This should only be called once at program startup
    #[allow(clippy::expect_used)]
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global tracing subscriber - was init_logging called twice?");

    guard
}

/// Run the CLI and return a result.
///
/// This is separated from `main()` to allow custom error handling that
/// doesn't show backtraces for user-facing errors like "file not found".
// Intentional separation for error handling - not a candidate for inlining.
#[allow(clippy::too_many_lines, clippy::single_call_fn)]
async fn run() -> Result<()> {
    // Check for -v/--verbose flag early to set log level before initializing
    // logging This allows `uffs -v search ...` to show info-level logs without
    // RUST_LOG=info
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

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
            output,
            drive,
            drives,
        }) => {
            commands::index(output, drive, drives).await?;
        }
        Some(Commands::Info { path }) => {
            commands::info(&path)?;
        }
        Some(Commands::Stats { path, top }) => {
            commands::stats(&path, top)?;
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

#[tokio::main]
#[allow(clippy::print_stderr)] // Intentional: user-facing error output
async fn main() {
    if let Err(err) = run().await {
        // Print error without backtrace for clean user-facing output
        // Use anyhow's chain() to iterate through the error chain
        for (idx, cause) in err.chain().enumerate() {
            if idx == 0 {
                eprintln!("Error: {cause}");
            } else {
                eprintln!("  Caused by: {cause}");
            }
        }

        std::process::exit(1);
    }
}
