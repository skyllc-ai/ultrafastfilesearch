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
//! ## Multi-Personality CLI (`BusyBox` Pattern)
//!
//! The binary name determines CLI behavior. Create symlinks for compatibility:
//! ```bash
//! ln -s uffs es           # Everything-compatible mode
//! ln -s uffs uffs-cpp     # C++ UFFS compatible mode
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

// Dependencies used in commands.rs for streaming output (Windows-only code
// paths)
use std::io::stdout;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use mimalloc::MiMalloc;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};
use {chrono as _, uffs_polars as _};

/// Use mimalloc globally - faster than system allocator for our workload:
/// many small allocations (file names, records) + large buffers (MFT,
/// `DataFrame`). Works well on Windows, macOS, and Linux without build
/// complexity.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod commands;

// ============================================================================
// Multi-Personality CLI (BusyBox Pattern)
// ============================================================================

/// CLI personality based on binary name (argv[0]).
///
/// Allows a single binary to behave differently based on how it's invoked,
/// similar to `BusyBox`. Users can create symlinks to get different CLI styles:
/// - `uffs` → Modern CLI (ripgrep/fd style)
/// - `es` → Everything-compatible mode
/// - `uffs-cpp` → C++ UFFS compatible mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)] // Variants will be used as personalities are implemented
pub enum Personality {
    /// Modern CLI: clean, ripgrep/fd style (default)
    #[default]
    Modern,
    /// Everything-compatible: matches voidtools Everything CLI
    Everything,
    /// C++ UFFS compatible: matches original C++ implementation
    CppCompat,
}

impl Personality {
    /// Detect personality from the binary name (argv[0]).
    ///
    /// Returns `Modern` for unknown binary names.
    #[must_use]
    pub fn detect() -> Self {
        let binary_name = std::env::args()
            .next()
            .and_then(|path| {
                Path::new(&path)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_lowercase())
            })
            .unwrap_or_default();

        match binary_name.as_ref() {
            "es" | "everything" => Self::Everything,
            "uffs-cpp" | "uffs_cpp" => Self::CppCompat,
            _ => Self::Modern, // "uffs" or anything else
        }
    }

    /// Get the display name for this personality.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Modern => "UFFS (Ultra Fast File Search)",
            Self::Everything => "UFFS (Everything-compatible mode)",
            Self::CppCompat => "UFFS (C++ compatible mode)",
        }
    }

    /// Check if this personality should use C++ compatible output quirks.
    #[must_use]
    pub const fn is_cpp_compat(&self) -> bool {
        matches!(self, Self::CppCompat)
    }
}

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
    #[arg(short, long, conflicts_with_all = ["drive", "drives", "mft_file"])]
    index: Option<PathBuf>,

    /// Use raw MFT file instead of live MFT (cross-platform debugging)
    ///
    /// Load a previously saved raw MFT file (from `uffs save-raw` or `uffs_mft
    /// save`). Use `--drive` to specify the volume letter for path
    /// resolution (default: X). Example: `uffs "*" --mft-file G_mft.bin
    /// --drive G`
    #[arg(long, conflicts_with_all = ["index", "drives"])]
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

    /// Tree metrics algorithm: current, cpp (C++ port)
    ///
    /// - current: Use current Rust leaf-peeling algorithm (default)
    /// - cpp: Use C++ port algorithm (100% faithful port of C++ tree algorithm)
    #[arg(long, default_value = "current")]
    tree_algo: String,

    /// MFT parsing algorithm: current, cpp (C++ port)
    ///
    /// - current: Use current Rust parsing algorithm (default)
    /// - cpp: Use C++ port algorithm (100% faithful port of C++ parsing)
    #[arg(long, default_value = "current")]
    parse_algo: String,

    /// I/O pipeline algorithm: current, cpp (C++ port)
    ///
    /// - current: Use current Rust I/O pipeline (default)
    /// - cpp: Use C++ port I/O pipeline (bitmap sync point before data reads)
    #[arg(long, default_value = "current")]
    io_algo: String,

    /// Chunk processing algorithm: current, cpp (C++ port)
    ///
    /// - current: Use current Rust chunk processing (default)
    /// - cpp: Use C++ port chunk processing (investigation target)
    #[arg(long, default_value = "current")]
    chunk_algo: String,

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
    /// Default: all columns (CPP compatible)
    #[arg(long, default_value = "all")]
    columns: String,

    /// Column separator (default: comma)
    #[arg(long, default_value = ",")]
    sep: String,

    /// Quote character for string values (default: double-quote for CPP
    /// compatibility)
    #[arg(long, default_value = "\"")]
    quotes: String,

    /// Include header row in output (default: true for CPP compatibility)
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
    #[arg(long, default_value = "auto")]
    query_mode: String,
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
    // logging This allows `uffs -v ...` to show info-level logs without
    // RUST_LOG=info
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

    // Detect CLI personality based on binary name (BusyBox pattern)
    let personality = Personality::detect();
    tracing::debug!(?personality, "CLI personality detected");

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
                    &cli.tree_algo,
                    &cli.parse_algo,
                    &cli.io_algo,
                    &cli.chunk_algo,
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
