//! UFFS (Ultra Fast File Search) CLI
//!
//! Fast file search from the command line.
//!
//! ## Usage
//!
//! Search is the default action (no subcommand needed):
//! ```bash
//! uffs *.txt              # Find all .txt files
//! uffs c:/pro*            # Find files starting with "pro" on C:
//! uffs --ext=rs,toml      # Find Rust files
//! ```
//!
//! ## Logging
//!
//! Use `-v` / `--verbose` for info-level terminal output:
//! ```bash
//! uffs -v *.txt
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
//! RUST_LOG=debug uffs *.txt
//!
//! # Trace mode - maximum verbosity
//! RUST_LOG=trace RUST_LOG_FILE=trace uffs *.txt
//! ```

// CLI main module uses single-call functions by design
#![expect(
    clippy::single_call_fn,
    reason = "CLI entry point functions are called once from main"
)]
#![allow(clippy::items_after_test_module)]

// Dependencies used in commands.rs for streaming output (Windows-only code
// paths)
use std::io;
use std::path::PathBuf;

use anyhow::Result;
#[cfg(test)]
use assert_cmd as _;
use chrono as _;
use clap::{Parser, Subcommand};
use mimalloc::MiMalloc;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};
use uffs_polars as _;

/// Use mimalloc globally - faster than system allocator for our workload:
/// many small allocations (file names, records) + large buffers (MFT,
/// `DataFrame`). Works well on Windows, macOS, and Linux without build
/// complexity.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod commands;

/// Operation label used for CLI-wide shutdown classification.
const CLI_OPERATION: &str = "uffs";

/// Maps spawned CLI task failures onto the approved cancellation taxonomy.
#[must_use]
fn classify_cli_task_error(
    operation: &'static str,
    error: &tokio::task::JoinError,
) -> uffs_mft::MftError {
    if error.is_cancelled() {
        return uffs_mft::MftError::Cancelled {
            operation,
            reason: error.to_string(),
        };
    }

    uffs_mft::MftError::WaitFailed {
        operation,
        reason: error.to_string(),
    }
}

/// Builds the explicit cancellation outcome for a Ctrl+C shutdown request.
#[must_use]
fn shutdown_requested_error(operation: &'static str) -> uffs_mft::MftError {
    uffs_mft::MftError::Cancelled {
        operation,
        reason: "shutdown requested by Ctrl+C".to_owned(),
    }
}

/// Builds a wait failure when the CLI cannot install a Ctrl+C listener.
#[must_use]
fn ctrl_c_listener_error(operation: &'static str, error: &io::Error) -> uffs_mft::MftError {
    uffs_mft::MftError::WaitFailed {
        operation,
        reason: format!("failed to listen for Ctrl+C: {error}"),
    }
}

/// Parse a drive letter from common CLI input formats.
///
/// Accepts:
/// - Single letter: `C`, `c`
/// - With colon: `C:`, `c:`
///
/// Returns uppercase drive letter.
fn parse_drive_letter(input: &str) -> Result<char, String> {
    let trimmed = input.trim();
    // Strip trailing colon if present (`C:` -> `C`).
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
#[command(
    author,
    version,
    about = "Command-line interface for UFFS (Ultra Fast File Search)",
    long_about = "Fast NTFS search via direct Master File Table reads.\n\nSearch is the default action: pass a pattern with no subcommand to search a live volume, a saved index, or a raw MFT file. Use subcommands for index creation and offline inspection.",
    after_help = "Examples:\n  uffs '*.txt'\n  uffs '>.*\\.log$' --drive C\n  uffs '*' --mft-file G_mft.bin --drive G\n  uffs index -d C index.parquet"
)]
#[command(propagate_version = true)]
#[command(args_conflicts_with_subcommands = true)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI args struct mirrors many boolean flags from clap"
)]
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
    #[arg(value_name = "PATTERN", verbatim_doc_comment)]
    pattern: Option<String>,

    /// Drive letter to search (e.g., C or C:). Overrides drive in pattern.
    #[arg(short, long, conflicts_with = "drives", value_parser = parse_drive_letter)]
    drive: Option<char>,

    /// Multiple drive letters to search concurrently (e.g., C,D,E or C:,D:,E:)
    #[arg(long, value_delimiter = ',', conflicts_with = "drive", value_parser = parse_drive_letter)]
    drives: Option<Vec<char>>,

    /// Use pre-built index file instead of live MFT
    #[arg(short, long, conflicts_with_all = ["drive", "drives", "mft_file"])]
    index: Option<PathBuf>,

    /// Use raw MFT file instead of live MFT (cross-platform debugging)
    ///
    /// Load a previously saved raw MFT file (from `uffs save-raw` or `uffs_mft
    /// save`). Use `--drive` to specify the volume letter for path
    /// resolution (default: X). Example: `uffs "*" --mft-file G_mft.bin
    /// --drive G`
    #[arg(long, conflicts_with_all = ["index", "drives"], verbatim_doc_comment)]
    mft_file: Option<PathBuf>,

    /// Show only files (exclude directories)
    #[arg(long)]
    files_only: bool,

    /// Show only directories
    #[arg(long)]
    dirs_only: bool,

    /// Hide system files (files starting with $)
    #[arg(long)]
    hide_system: bool,

    /// Show detailed timing breakdown for performance profiling
    #[arg(long)]
    profile: bool,

    /// Debug tree metrics computation (prints detailed hardlink handling info)
    #[arg(long, hide = true)]
    debug_tree: bool,

    /// Benchmark mode: skip output, only measure MFT reading and filtering
    /// Use this for profiling without stdout I/O overhead
    #[arg(long)]
    benchmark: bool,

    /// Disable MFT bitmap optimization (read ALL records)
    /// Use this for debugging if records appear to be missing
    #[arg(long)]
    no_bitmap: bool,

    /// Bypass cache and read MFT fresh (default: use cache)
    #[arg(long)]
    no_cache: bool,

    /// Minimum file size in bytes
    #[arg(long)]
    min_size: Option<u64>,

    /// Maximum file size in bytes
    #[arg(long)]
    max_size: Option<u64>,

    /// Maximum number of results (0 = unlimited)
    #[arg(short = 'n', long, default_value = "0")]
    limit: u32,

    /// Output format: table, json, csv, custom
    #[arg(short, long, default_value = "custom")]
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
    /// Default: all columns.
    #[arg(long, default_value = "all")]
    columns: String,

    /// Column separator (default: comma)
    #[arg(long, default_value = ",")]
    sep: String,

    /// Quote character for string values (default: double quote)
    #[arg(long, default_value = "\"")]
    quotes: String,

    /// Include header row in output.
    #[arg(long, default_value = "true")]
    header: bool,

    /// Representation for active/true boolean attributes
    #[arg(long, default_value = "1")]
    pos: String,

    /// Representation for inactive/false boolean attributes
    #[arg(long, default_value = "0")]
    neg: String,

    /// Query execution mode: auto, index, dataframe
    ///
    /// - auto: Automatically choose best path (default)
    /// - index: Force fast `MftIndex` path (simple queries only)
    /// - dataframe: Force Polars `DataFrame` path (full features)
    #[arg(long, default_value = "auto", verbatim_doc_comment)]
    query_mode: String,

    /// Override timezone offset for timestamp display (hours from UTC).
    ///
    /// By default, timestamps are displayed in the current local timezone.
    /// Use this to force a specific offset, e.g. for reproducible parity
    /// testing when the reference was generated in a different DST period.
    ///
    /// Examples: -8 (PST), -7 (PDT), 0 (UTC), 1 (CET), 9 (JST)
    #[arg(long, allow_hyphen_values = true)]
    tz_offset: Option<i32>,
}

/// Available CLI subcommands.
///
/// Note: Search is NOT a subcommand - it's the default action.
/// This matches ripgrep/fd/Everything patterns where the tool name IS the
/// search.
#[derive(Subcommand)]
enum Commands {
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
#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity — logging setup is complex enough to warrant its own function"
)]
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

    // Terminal layer (to stderr to avoid corrupting CSV output, with ANSI colors,
    // file/line info, thread IDs)
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
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
    #[expect(
        clippy::expect_used,
        reason = "global subscriber set once at startup; panic is intentional if called twice"
    )]
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global tracing subscriber - was init_logging called twice?");

    guard
}

/// Run the CLI and return a result.
///
/// This is separated from `main()` to allow custom error handling that
/// doesn't show backtraces for user-facing errors like "file not found".
#[tracing::instrument(level = "info", skip_all)]
async fn run() -> Result<()> {
    // Check for -v/--verbose flag early to set log level before initializing
    // logging This allows `uffs -v ...` to show info-level logs without
    // RUST_LOG=info
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

    let cli = Cli::parse();

    // Handle subcommands or default search action
    match cli.command {
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
            // Default action: search
            if let Some(pattern) = cli.pattern {
                commands::search(
                    &pattern,
                    cli.drive,
                    cli.drives,
                    cli.index,
                    cli.mft_file,
                    cli.files_only,
                    cli.dirs_only,
                    cli.hide_system,
                    cli.profile,
                    cli.debug_tree,
                    cli.benchmark,
                    cli.no_bitmap,
                    cli.no_cache,
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
                    &cli.query_mode,
                    cli.tz_offset,
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

/// Runs the CLI while listening for Ctrl+C so shutdown reaches long-running
/// command flows started from the binary entrypoint.
#[expect(
    clippy::single_call_fn,
    reason = "entrypoint wrapper exists solely to propagate shutdown into the spawned command task"
)]
#[tracing::instrument(level = "debug", skip_all, fields(operation = CLI_OPERATION))]
async fn run_until_shutdown() -> Result<()> {
    let mut run_task = tokio::spawn(run());

    tokio::select! {
        result = &mut run_task => {
            match result {
                Ok(outcome) => outcome,
                Err(error) => Err(classify_cli_task_error(CLI_OPERATION, &error).into()),
            }
        }
        signal = tokio::signal::ctrl_c() => {
            run_task.abort();

            match signal {
                Ok(()) => Err(shutdown_requested_error(CLI_OPERATION).into()),
                Err(error) => Err(ctrl_c_listener_error(CLI_OPERATION, &error).into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::default_numeric_fallback,
        clippy::expect_used,
        clippy::manual_let_else,
        clippy::panic
    )]

    use std::path::{Path, PathBuf};

    use clap::{CommandFactory, Parser};

    use super::{
        Cli, Commands, classify_cli_task_error, ctrl_c_listener_error, parse_drive_letter,
        shutdown_requested_error,
    };

    fn render_long_help(mut command: clap::Command) -> String {
        let mut buffer = Vec::new();
        command
            .write_long_help(&mut buffer)
            .expect("CLI help should render successfully");
        String::from_utf8(buffer).expect("CLI help should be valid UTF-8")
    }

    fn parse_cli(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    #[test]
    fn test_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn test_top_level_help_includes_examples_and_default_search_guidance() {
        let help = render_long_help(Cli::command());

        assert!(help.contains("Search is the default action"));
        assert!(help.contains("uffs '*.txt'"));
        assert!(help.contains("uffs '>.*\\.log$' --drive C"));
        assert!(help.contains("uffs '*' --mft-file G_mft.bin --drive G"));
        assert!(help.contains("uffs index -d C index.parquet"));
    }

    #[test]
    fn test_parse_drive_letter_accepts_letter_colon_and_whitespace_variants() {
        assert_eq!(parse_drive_letter("c"), Ok('C'));
        assert_eq!(parse_drive_letter("C:"), Ok('C'));
        assert_eq!(parse_drive_letter(" d: "), Ok('D'));
    }

    #[test]
    fn test_parse_drive_letter_rejects_invalid_values() {
        assert!(parse_drive_letter("").is_err());
        assert!(parse_drive_letter("12").is_err());
        assert!(parse_drive_letter("1:").is_err());
        assert!(parse_drive_letter("CD").is_err());
    }

    #[test]
    fn test_default_search_parses_offline_mft_mode_and_common_options() {
        let cli = parse_cli(&[
            "uffs",
            "*.rs",
            "--mft-file",
            "raw.bin",
            "--drive",
            "g:",
            "--format",
            "json",
            "--tz-offset",
            "-8",
        ])
        .expect("default search args should parse");

        assert!(cli.command.is_none());
        assert_eq!(cli.pattern.as_deref(), Some("*.rs"));
        assert_eq!(cli.drive, Some('G'));
        assert_eq!(cli.drives, None);
        assert_eq!(cli.mft_file.as_deref(), Some(Path::new("raw.bin")));
        assert_eq!(cli.format, "json");
        assert_eq!(cli.tz_offset, Some(-8));
    }

    #[test]
    fn test_default_search_rejects_conflicting_search_sources() {
        let err = match parse_cli(&[
            "uffs",
            "*.rs",
            "--index",
            "saved.parquet",
            "--mft-file",
            "raw.bin",
        ]) {
            Ok(_) => panic!("conflicting search source flags should fail"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn test_index_subcommand_normalizes_multi_drive_input() {
        let cli = parse_cli(&["uffs", "index", "out.parquet", "--drives", "c:,d,e:"])
            .expect("index args should parse");

        match cli.command {
            Some(Commands::Index {
                output,
                drive,
                drives,
            }) => {
                assert_eq!(output, PathBuf::from("out.parquet"));
                assert_eq!(drive, None);
                assert_eq!(drives, Some(vec!['C', 'D', 'E']));
            }
            _ => panic!("expected index subcommand"),
        }
    }

    #[test]
    fn test_index_help_includes_examples_and_multi_drive_guidance() {
        let mut command = Cli::command();
        let help = render_long_help(
            command
                .find_subcommand_mut("index")
                .expect("index subcommand should exist")
                .clone(),
        );

        assert!(help.contains("By default, indexes ALL available NTFS drives"));
        assert!(help.contains("uffs index -d C index.parquet"));
        assert!(help.contains("uffs index --drives C,D,E out.parquet"));
        assert!(help.contains("Creates myindex.parquet"));
    }

    #[tokio::test]
    async fn test_classify_cli_task_error_maps_cancelled_joins() {
        let handle = tokio::spawn(async {
            core::future::pending::<()>().await;
        });
        handle.abort();

        let outcome = handle.await;
        assert!(outcome.is_err(), "aborted task unexpectedly completed");
        let Err(join_error) = outcome else {
            return;
        };

        let error = classify_cli_task_error("uffs", &join_error);

        assert!(matches!(
            error,
            uffs_mft::MftError::Cancelled {
                operation: "uffs",
                ..
            }
        ));
    }

    #[test]
    fn test_shutdown_requested_error_is_cancelled() {
        let error = shutdown_requested_error("uffs");

        assert!(matches!(
            error,
            uffs_mft::MftError::Cancelled {
                operation: "uffs",
                ..
            }
        ));
    }

    #[test]
    fn test_ctrl_c_listener_error_is_wait_failed() {
        let error = ctrl_c_listener_error("uffs", &std::io::Error::other("listener unavailable"));

        assert!(matches!(
            error,
            uffs_mft::MftError::WaitFailed {
                operation: "uffs",
                ..
            }
        ));
    }
}

#[tokio::main]
#[expect(
    clippy::print_stderr,
    reason = "intentional user-facing error output to stderr"
)]
async fn main() {
    if let Err(err) = run_until_shutdown().await {
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
