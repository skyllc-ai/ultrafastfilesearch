//! # `uffs_mft`: MFT Command-Line Tool
//!
//! Low-level tool for reading and exporting NTFS Master File Table data.
//!
//! ## Usage
//!
//! ```bash
//! # Read MFT from C: drive and export to Parquet
//! uffs_mft read --drive C --output c_drive.parquet
//!
//! # Show MFT information for a drive
//! uffs_mft info --drive C
//!
//! # List all NTFS drives
//! uffs_mft drives
//! ```
//!
//! ## Logging
//!
//! Use `-v` / `--verbose` for debug-level terminal output:
//! ```bash
//! uffs_mft -v info --drive C
//! ```
//!
//! For finer control, use environment variables:
//! - `RUST_LOG`: Terminal log level (default: `info`, or `debug` with `-v`)
//! - `RUST_LOG_FILE`: File log level (default: `info`)
//! - `UFFS_LOG_DIR`: Log directory (default: `~/bin/uffs/logs`)
//!
//! **Note**: This tool requires Administrator privileges on Windows.

// ============================================================================
// Suppress unused crate warnings
// ============================================================================
// These dependencies are used by the uffs-mft library, not this binary.
// Cargo doesn't support per-binary dependencies, so we suppress the warnings
// here.
#[cfg(not(windows))]
use core::future::Future;
use std::io::stdout;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
// Dev-dependencies (used in benchmarks and tests only)
#[cfg(test)]
use criterion as _;
// Pipelining dependencies (used in io.rs PipelinedMftReader on Windows)
#[cfg(windows)]
use crossbeam_channel as _;
// Platform-gated dependencies (used on Windows only)
#[cfg(not(windows))]
use indicatif as _;
#[cfg(windows)]
use indicatif::{ProgressBar, ProgressStyle};
#[cfg(test)]
use proptest as _;
// SmallVec for path chain building (used in index.rs PathResolver)
use smallvec as _;
#[cfg(not(windows))]
use tracing as _;
#[cfg(windows)]
use tracing::{info, warn};
use tracing_appender::non_blocking::NonBlocking;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::{EnvFilter, Layer};
#[cfg(not(windows))]
use uffs_mft as _;
#[cfg(windows)]
use uffs_mft::MftReader;
// Optional dependencies
#[cfg(feature = "zstd")]
use zstd as _;
use {bitflags as _, rayon as _, rustc_hash as _, thiserror as _, uffs_polars as _};
// Benchmark dependencies (used by bench/bench-all commands on Windows)
#[cfg(not(windows))]
use {chrono as _, hostname as _, num_cpus as _};

/// Formats a duration intelligently based on magnitude.
///
/// Output format varies by duration:
/// - Days+: `2d 3h 5m 10s`
/// - Hours+: `3h 5m 10s`
/// - Minutes+: `5 m 10 s`
/// - Seconds+: `10 s 500 ms`
/// - Milliseconds+: `500 ms 250 μs`
/// - Microseconds+: `250 μs 100 ns`
/// - Nanoseconds only: `100 ns`
fn format_duration(duration: core::time::Duration) -> String {
    let total_seconds = duration.as_secs();
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = (total_seconds / 3600) % 24;
    let days = total_seconds / 86400;

    let milliseconds = duration.subsec_millis();
    let microseconds = duration.subsec_micros() % 1_000;
    let nanoseconds = duration.subsec_nanos() % 1_000;

    if days > 0 {
        format!("{days:>2}d {hours:>2}h {minutes:>2}m {seconds:>2}s")
    } else if hours > 0 {
        format!("{hours:>2}h {minutes:>2}m {seconds:>2}s")
    } else if minutes > 0 {
        format!("{minutes:>3} m  {seconds:>3} s ")
    } else if seconds > 0 {
        format!("{seconds:>3} s  {milliseconds:>3} ms")
    } else if milliseconds > 0 {
        format!("{milliseconds:>3} ms {microseconds:>3} μs")
    } else if microseconds > 0 {
        format!("{microseconds:>3} μs {nanoseconds:>3} ns")
    } else {
        format!("{nanoseconds:>3} ns")
    }
}

/// Formats a byte count intelligently based on magnitude.
///
/// Output format varies by size:
/// - < 1 KB: `1234 B`
/// - < 1 MB: `123.45 KB`
/// - < 1 GB: `123.45 MB`
/// - < 1 TB: `123.45 GB`
/// - >= 1 TB: `123.45 TB`
#[allow(clippy::cast_precision_loss, clippy::float_arithmetic)] // Precision loss acceptable for display
fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes:>4} B")
    } else if bytes < 1024 * 1024 {
        format!("{:>7.2} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:>7.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes < 1024 * 1024 * 1024 * 1024 {
        format!("{:>7.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!(
            "{:>7.2} TB",
            bytes as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
        )
    }
}

/// Formats a number with comma separators for readability.
///
/// Examples: 1234567 → "1,234,567", 1000 → "1,000"
fn format_number_commas(num: u64) -> String {
    let num_str = num.to_string();
    let mut result = String::with_capacity(num_str.len() + num_str.len() / 3);
    for (idx, char) in num_str.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            result.push(',');
        }
        result.push(char);
    }
    result.chars().rev().collect()
}

/// Cleans up a path for user-friendly display.
///
/// On Windows, `std::fs::canonicalize` returns extended-length paths with
/// the `\\?\` prefix. This function strips that prefix for cleaner output.
fn clean_path_for_display(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    path_str
        .strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// `uffs_mft`: Low-level NTFS MFT reading tool.
#[derive(Parser)]
#[command(name = "uffs_mft")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose output.
    #[arg(short, long, global = true)]
    verbose: bool,

    /// The subcommand to execute.
    #[command(subcommand)]
    command: Commands,
}

/// Available subcommands for the `uffs_mft` CLI.
#[derive(Subcommand)]
enum Commands {
    /// Read MFT from a drive and export to Parquet
    Read {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Output file path (Parquet format)
        #[arg(short, long)]
        output: PathBuf,

        /// Read mode: auto, parallel, streaming, prefetch
        /// - auto: Select based on drive type (SSD→parallel, HDD→prefetch)
        /// - parallel: Read all chunks then parse in parallel (best for SSD)
        /// - streaming: Sequential reads with immediate parsing (lower memory)
        /// - prefetch: Double-buffered reads for I/O overlap (best for HDD)
        #[arg(short, long, default_value = "auto")]
        mode: String,

        /// Merge extension records for complete data (slower).
        /// By default, extension records (~1% of files with many hard
        /// links/ADS) are skipped for ~15-25% faster reads.
        #[arg(long)]
        full: bool,

        /// Output one row per unique FRS instead of expanding hard links.
        /// By default, hard links are expanded to separate rows (matching
        /// C++/Explorer). Use this flag for power users who want to
        /// count unique files, not paths.
        #[arg(long)]
        unique: bool,

        /// Include forensic records (deleted, corrupt, extension records).
        /// Adds `is_deleted`, `is_corrupt`, `is_extension`, `base_frs` columns.
        /// WARNING: May significantly increase output size (10-50% more rows).
        #[arg(long)]
        forensic: bool,
    },

    /// Show MFT information for a drive
    Info {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Perform deep scan (reads all MFT records for detailed statistics)
        #[arg(long)]
        deep: bool,

        /// Disable bitmap optimization (read ALL records, not just in-use
        /// ones). Use this to debug if bitmap is causing records to be
        /// skipped incorrectly.
        #[arg(long)]
        no_bitmap: bool,

        /// Output one row per unique FRS instead of expanding hard links.
        /// By default, hard links are expanded to separate rows (matching
        /// C++/Explorer). Use this flag for power users who want to
        /// count unique files, not paths.
        #[arg(long)]
        unique: bool,
    },

    /// List all available NTFS drives
    Drives,

    /// Benchmark MFT reading with detailed phase timing
    Bench {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Output results as JSON (for scripting)
        #[arg(long)]
        json: bool,

        /// Skip `DataFrame` building (measure I/O + parse only)
        #[arg(long)]
        no_df: bool,

        /// Number of runs for averaging (default: 1)
        #[arg(long, default_value = "1")]
        runs: u32,

        /// Read mode: auto, parallel, streaming, prefetch
        #[arg(short, long, default_value = "auto")]
        mode: String,

        /// Merge extension records for complete data (slower).
        /// By default, extension records (~1% of files) are skipped for faster
        /// reads.
        #[arg(long)]
        full: bool,
    },

    /// Benchmark ALL NTFS drives and save results to a file
    BenchAll {
        /// Output file path (JSON format, default:
        /// `uffs_benchmark_YYYYMMDD_HHMMSS.json`)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Skip `DataFrame` building (measure I/O + parse only)
        #[arg(long)]
        no_df: bool,

        /// Number of runs per drive for averaging (default: 1)
        #[arg(long, default_value = "1")]
        runs: u32,

        /// Merge extension records for complete data (slower).
        /// By default, extension records (~1% of files) are skipped for faster
        /// reads.
        #[arg(long)]
        full: bool,
    },

    /// Diagnose MFT bitmap to investigate record skipping
    BitmapDiag {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Show sample of individual record states
        #[arg(long)]
        samples: bool,
    },

    /// Save MFT bytes to a file for offline analysis
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft save --drive C --output mft_c.mft
    /// uffs_mft save -d C -o mft_c.mft --no-compress
    /// uffs_mft save -d C -o mft_c.raw --raw  # Compatible with other MFT tools
    /// ```
    Save {
        /// Drive letter to read MFT from (e.g., C, D, E)
        #[arg(short, long, value_name = "LETTER")]
        drive: char,

        /// Output file path for raw MFT data
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,

        /// Disable compression (default: compressed with zstd)
        #[arg(long)]
        no_compress: bool,

        /// Compression level (1-22, default 3)
        #[arg(long, default_value = "3")]
        compression_level: i32,

        /// Raw compatibility mode: output raw MFT bytes without header.
        /// This format is compatible with other MFT tools like analyzeMFT,
        /// MFT2CSV. Note: --raw implies --no-compress.
        #[arg(long)]
        raw: bool,
    },

    /// Load MFT from a saved file and export to parquet/csv
    ///
    /// Supports both UFFS-MFT format (with header) and raw NTFS format
    /// (compatible with other MFT tools). For raw NTFS files, use --drive
    /// to specify the volume letter for path resolution.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft load mft_c.mft --info-only
    /// uffs_mft load mft_c.mft --output index.parquet
    /// uffs_mft load mft_c.mft -o index.csv
    /// uffs_mft load mft_c.mft --build-index  # Debug tree metrics
    /// uffs_mft load mft_c.raw --drive C -o output.csv  # Raw NTFS format
    /// ```
    Load {
        /// Input raw MFT file path (created with 'save' command or other tools)
        #[arg(value_name = "FILE")]
        input: PathBuf,

        /// Output file path (.parquet or .csv based on extension)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,

        /// Show info about the raw MFT file only (don't export)
        #[arg(long)]
        info_only: bool,

        /// Build `MftIndex` and show tree metrics (for debugging)
        #[arg(long)]
        build_index: bool,

        /// Debug tree metrics computation (detailed hardlink handling output)
        #[arg(long)]
        debug_tree: bool,

        /// Volume letter for path resolution (e.g., C, D, E).
        /// Required for raw NTFS files that don't have this info in header.
        /// For UFFS-MFT format files, this overrides the stored volume letter.
        #[arg(short, long, value_name = "LETTER")]
        drive: Option<char>,

        /// Include forensic records (deleted, corrupt, extension records).
        /// Adds `is_deleted`, `is_corrupt`, `is_extension`, `base_frs` columns.
        /// WARNING: May significantly increase output size (10-50% more rows).
        #[arg(long)]
        forensic: bool,
    },

    /// Raw MFT read benchmark (matches C++ --benchmark-mft output exactly)
    ///
    /// Measures pure disk I/O throughput by reading the entire MFT with
    /// synchronous 1MB reads. Does NOT parse records or build `DataFrame`s.
    /// Use this to compare raw read performance between Rust and C++.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft benchmark-mft --drive C
    /// uffs_mft benchmark-mft -d S
    /// ```
    BenchmarkMft {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,
    },

    /// Full index build benchmark (matches C++ --benchmark-index output
    /// exactly)
    ///
    /// Measures the complete UFFS indexing pipeline: async I/O + parsing +
    /// `DataFrame` building. This is what users experience when indexing.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft benchmark-index --drive C
    /// uffs_mft benchmark-index -d S
    /// ```
    BenchmarkIndex {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,
    },

    /// Lean index build benchmark (no `DataFrame` overhead)
    ///
    /// Measures the UFFS indexing pipeline with lean `MftIndex` instead of
    /// Polars `DataFrame`. This should be significantly faster (~2x) because
    /// it avoids `DataFrame` building overhead.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft benchmark-index-lean --drive C
    /// uffs_mft benchmark-index-lean -d S
    /// uffs_mft benchmark-index-lean -d S --mode pipelined-parallel
    /// ```
    BenchmarkIndexLean {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Read mode: auto, parallel, streaming, prefetch, pipelined,
        /// pipelined-parallel
        /// - auto: Select based on drive type (SSD→parallel,
        ///   HDD→pipelined-parallel)
        /// - parallel: Read all chunks then parse in parallel (best for SSD)
        /// - streaming: Sequential reads with immediate parsing (lower memory)
        /// - prefetch: Double-buffered reads for I/O overlap
        /// - pipelined: I/O+CPU overlap with single-threaded parsing
        /// - pipelined-parallel: I/O+CPU overlap with multi-core parsing (best
        ///   for HDD)
        #[arg(short, long, default_value = "auto")]
        mode: String,

        /// Disable MFT bitmap optimization (read entire MFT sequentially).
        /// C++ team insight: Sequential reads may be faster than seeking to
        /// skip unused records on HDD.
        #[arg(long)]
        no_bitmap: bool,

        /// Disable placeholder creation for missing parent directories.
        /// C++ team insight: They don't add placeholders upfront - they resolve
        /// paths lazily. Disabling saves ~15% of CPU time.
        #[arg(long)]
        no_placeholders: bool,

        /// Number of concurrent I/O operations (reads in flight).
        /// Default: auto (2 for HDD, 8 for SSD, 32 for `NVMe`).
        /// Use this to experiment with I/O parallelism.
        #[arg(long)]
        concurrency: Option<usize>,

        /// I/O chunk size in KB (e.g., 1024 = 1MB, 2048 = 2MB, 4096 = 4MB).
        /// Default: auto (1MB for HDD, 2MB for SSD, 4MB for `NVMe`).
        /// Larger chunks reduce syscall overhead but increase latency per
        /// completion.
        #[arg(long)]
        io_size_kb: Option<usize>,

        /// Enable parallel parsing (M3 optimization).
        /// Uses worker threads to parse MFT records in parallel with I/O.
        /// Beneficial for `NVMe` drives where I/O is faster than parsing.
        /// Default: auto (enabled for `NVMe`, disabled for HDD/SSD).
        #[arg(long)]
        parallel_parse: bool,

        /// Number of parsing worker threads (only used with --parallel-parse).
        /// Default: number of CPU cores.
        #[arg(long)]
        parse_workers: Option<usize>,
    },

    /// Benchmark multi-volume indexing using single IOCP (M4 optimization).
    ///
    /// Reads MFTs from multiple drives simultaneously using a single I/O
    /// Completion Port. This allows the OS to optimize I/O scheduling across
    /// all drives.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft benchmark-multi-volume --drives C,D,S
    /// uffs_mft benchmark-multi-volume -d C,F
    /// ```
    BenchmarkMultiVolume {
        /// Comma-separated list of drive letters (e.g., C,D,S)
        #[arg(short, long, value_delimiter = ',')]
        drives: Vec<char>,
    },

    /// Query USN Journal information for a drive (M5 optimization).
    ///
    /// Shows the USN Journal ID, first/next USN, and other metadata.
    /// This is useful for checking if incremental updates are possible.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft usn-info --drive C
    /// ```
    UsnInfo {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,
    },

    /// Read recent USN Journal changes for a drive (M5 optimization).
    ///
    /// Reads changes from the USN Journal since a given USN. If no start USN
    /// is provided, reads from the beginning of the journal.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft usn-read --drive C
    /// uffs_mft usn-read --drive C --start-usn 12345678
    /// uffs_mft usn-read --drive C --limit 100
    /// ```
    UsnRead {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Starting USN (default: read from journal start)
        #[arg(long)]
        start_usn: Option<i64>,

        /// Maximum number of records to display (default: 50)
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// Save index to disk for incremental updates (M5 optimization).
    ///
    /// Saves the current MFT index to a binary file along with USN Journal
    /// checkpoint. This allows fast incremental updates later.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft index-save --drive C --output c_index.uffs
    /// ```
    IndexSave {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Load index from disk and show info (M5 optimization).
    ///
    /// Loads a previously saved index and displays its metadata.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft index-load --input c_index.uffs
    /// ```
    IndexLoad {
        /// Input file path
        #[arg(short, long)]
        input: PathBuf,
    },

    /// Show cache status and manage cached indices (M5 optimization).
    ///
    /// Shows the status of cached indices in the system temp directory.
    /// Use --clean to remove expired caches, --purge to remove all.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft cache-status
    /// uffs_mft cache-status --clean
    /// uffs_mft cache-status --purge
    /// ```
    CacheStatus {
        /// Remove expired cache files
        #[arg(long)]
        clean: bool,

        /// Remove ALL cache files
        #[arg(long)]
        purge: bool,
    },

    /// Get or refresh a cached index for a drive (M5 optimization).
    ///
    /// Loads the cached index if fresh, otherwise rebuilds and caches it.
    /// Uses the default TTL of 10 minutes.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft cache-get --drive C
    /// uffs_mft cache-get --drive C --force
    /// ```
    CacheGet {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Force rebuild even if cache is fresh
        #[arg(long)]
        force: bool,

        /// Custom TTL in seconds (default: 600 = 10 minutes)
        #[arg(long)]
        ttl: Option<u64>,
    },

    /// Clear cached indices (M5 optimization).
    ///
    /// Removes cached indices to force a fresh re-read on next access.
    /// Use --drive to clear a specific drive, or --all to clear everything.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft cache-clear --drive C
    /// uffs_mft cache-clear --all
    /// ```
    CacheClear {
        /// Drive letter to clear (e.g., C, D, E)
        #[arg(short, long)]
        drive: Option<char>,

        /// Clear ALL cached indices
        #[arg(long)]
        all: bool,
    },

    /// Incremental index update using USN Journal (M5 optimization).
    ///
    /// Loads a cached index and applies USN Journal changes since the last
    /// checkpoint. Much faster than a full MFT scan for typical workloads.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft index-update --drive C
    /// uffs_mft index-update --drive C --force-full
    /// ```
    IndexUpdate {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Force full MFT scan instead of incremental update
        #[arg(long)]
        force_full: bool,

        /// Custom TTL in seconds (default: 600 = 10 minutes)
        #[arg(long)]
        ttl: Option<u64>,
    },

    /// Index ALL NTFS drives in parallel (optimized lean index path).
    ///
    /// Reads MFTs from all detected NTFS drives simultaneously using the
    /// optimized `SlidingIocpInline` path with parallel parsing. Returns
    /// lean `MftIndex` structures (no `DataFrame` overhead).
    ///
    /// By default, uses cache: loads fresh indices from cache, rebuilds and
    /// saves stale/missing ones. Use `--no-cache` to force fresh reads.
    ///
    /// # Examples
    ///
    /// ```text
    /// uffs_mft index-all
    /// uffs_mft index-all --drives C,D,E
    /// uffs_mft index-all --no-cache
    /// uffs_mft index-all --ttl 300
    /// ```
    IndexAll {
        /// Comma-separated list of drive letters (default: all NTFS drives)
        #[arg(short, long, value_delimiter = ',')]
        drives: Option<Vec<char>>,

        /// Skip cache (always read fresh from disk, still saves to cache)
        #[arg(long)]
        no_cache: bool,

        /// Cache TTL in seconds (default: 600 = 10 minutes)
        #[arg(long, default_value = "600")]
        ttl: u64,
    },
}

/// Initialize logging with terminal + file support.
///
/// If `verbose` is true and `RUST_LOG` is not set, uses `debug` level for
/// terminal. Otherwise, terminal logging is controlled by `RUST_LOG` (default:
/// `info`). File logging is controlled by `RUST_LOG_FILE` (default: `info`).
/// Log directory is controlled by `UFFS_LOG_DIR` (default: `~/bin/uffs/logs`).
#[allow(clippy::single_call_fn)]
fn init_logging(verbose: bool) -> tracing_appender::non_blocking::WorkerGuard {
    use std::fs;

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

    // Create log directory if it doesn't exist
    drop(fs::create_dir_all(&log_dir));

    // Create rolling file appender (daily rotation)
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "uffs_mft_log_");
    let (non_blocking, guard): (NonBlocking, _) = NonBlocking::new(file_appender);

    // Terminal filter: -v sets debug if RUST_LOG not explicitly set
    let terminal_default = if verbose { "debug" } else { "info" };
    let terminal_filter =
        EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| terminal_default.to_owned()));

    // File filter (default: info)
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

    #[allow(clippy::expect_used)]
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global tracing subscriber");

    guard
}

#[tokio::main]
#[allow(clippy::print_stderr, clippy::exit)] // Intentional: user-facing error output
async fn main() {
    // Check for -v/--verbose flag early
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

    if let Err(err) = run().await {
        // Print clean error without stack trace
        eprintln!("Error: {err}");
        // Print cause chain if available
        for cause in err.chain().skip(1) {
            eprintln!("  Caused by: {cause}");
        }
        std::process::exit(1);
    }
}

/// Main application logic, separated from `main()` for clean error handling.
#[allow(clippy::exit, clippy::unused_async, clippy::single_call_fn)]
async fn run() -> Result<()> {
    // Parse CLI - let clap handle its own error formatting
    // Clap already provides excellent error messages with usage hints
    let cli = Cli::parse();

    // Dispatch command (platform-specific functionality handled in
    // dispatch_command)
    dispatch_command(cli.command).await
}

/// Dispatch CLI commands to their handlers.
///
/// Separated from `run()` to keep each function under the line limit.
#[cfg(windows)]
async fn dispatch_command(command: Commands) -> Result<()> {
    match command {
        Commands::Read {
            drive,
            output,
            mode,
            full,
            unique,
            forensic,
        } => cmd_read(drive, output, &mode, full, unique, forensic).await,
        Commands::Info {
            drive,
            deep,
            no_bitmap,
            unique,
        } => cmd_info(drive, deep, no_bitmap, unique).await,
        Commands::Drives => cmd_drives().await,
        Commands::Bench {
            drive,
            json,
            no_df,
            runs,
            mode,
            full,
        } => cmd_bench(drive, json, no_df, runs, &mode, full).await,
        Commands::BenchAll {
            output,
            no_df,
            runs,
            full,
        } => cmd_bench_all(output, no_df, runs, full).await,
        Commands::BitmapDiag { drive, samples } => cmd_bitmap_diag(drive, samples).await,
        Commands::Save {
            drive,
            output,
            no_compress,
            compression_level,
            raw,
        } => cmd_save(drive, &output, !no_compress, compression_level, raw).await,
        Commands::Load {
            input,
            output,
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        } => cmd_load(
            &input,
            output.as_deref(),
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        ),
        Commands::BenchmarkMft { drive } => cmd_benchmark_mft(drive).await,
        Commands::BenchmarkIndex { drive } => cmd_benchmark_index(drive).await,
        Commands::BenchmarkIndexLean {
            drive,
            mode,
            no_bitmap,
            no_placeholders,
            concurrency,
            io_size_kb,
            parallel_parse,
            parse_workers,
        } => {
            cmd_benchmark_index_lean(
                drive,
                &mode,
                no_bitmap,
                no_placeholders,
                concurrency,
                io_size_kb,
                parallel_parse,
                parse_workers,
            )
            .await
        }
        Commands::BenchmarkMultiVolume { drives } => cmd_benchmark_multi_volume(drives).await,
        Commands::UsnInfo { drive } => cmd_usn_info(drive).await,
        Commands::UsnRead {
            drive,
            start_usn,
            limit,
        } => cmd_usn_read(drive, start_usn, limit).await,
        Commands::IndexSave { drive, output } => cmd_index_save(drive, &output).await,
        Commands::IndexLoad { input } => cmd_index_load(&input).await,
        Commands::CacheStatus { clean, purge } => cmd_cache_status(clean, purge).await,
        Commands::CacheGet { drive, force, ttl } => cmd_cache_get(drive, force, ttl).await,
        Commands::CacheClear { drive, all } => cmd_cache_clear(drive, all).await,
        Commands::IndexUpdate {
            drive,
            force_full,
            ttl,
        } => cmd_index_update(drive, force_full, ttl).await,
        Commands::IndexAll {
            drives,
            no_cache,
            ttl,
        } => cmd_index_all(drives, no_cache, ttl).await,
    }
}

/// Command dispatcher for non-Windows platforms (limited functionality).
///
/// Only the `load` command works on non-Windows platforms.
#[cfg(not(windows))]
#[allow(clippy::unused_async, clippy::single_call_fn)] // Async for API parity with Windows
async fn dispatch_command(command: Commands) -> Result<()> {
    match command {
        Commands::Load {
            input,
            output,
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        } => cmd_load(
            &input,
            output.as_deref(),
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        ),
        // All other commands require Windows (direct NTFS volume access)
        Commands::Read { .. }
        | Commands::Info { .. }
        | Commands::Drives
        | Commands::Bench { .. }
        | Commands::BenchAll { .. }
        | Commands::BitmapDiag { .. }
        | Commands::Save { .. }
        | Commands::BenchmarkMft { .. }
        | Commands::BenchmarkIndex { .. }
        | Commands::BenchmarkIndexLean { .. }
        | Commands::BenchmarkMultiVolume { .. }
        | Commands::UsnInfo { .. }
        | Commands::UsnRead { .. }
        | Commands::IndexSave { .. }
        | Commands::IndexLoad { .. }
        | Commands::CacheStatus { .. }
        | Commands::CacheGet { .. }
        | Commands::CacheClear { .. }
        | Commands::IndexUpdate { .. }
        | Commands::IndexAll { .. } => {
            anyhow::bail!(
                "This command requires Windows.\n\
                 Only the 'load' command works on macOS/Linux for parsing saved MFT files."
            );
        }
    }
}

#[cfg(windows)]
async fn cmd_read(
    drive: char,
    output: PathBuf,
    mode_str: &str,
    full: bool,
    unique: bool,
    forensic: bool,
) -> Result<()> {
    use std::time::Instant;

    use tracing::debug;
    use uffs_mft::MftReadMode;

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    // Forensic mode is not yet supported for live reads
    // (requires significant I/O layer refactoring)
    if forensic {
        warn!("⚠️ Forensic mode (--forensic) is not yet supported for live reads.");
        warn!(
            "   Use 'uffs_mft save' to save the MFT, then 'uffs_mft load --forensic' to analyze."
        );
        warn!("   Proceeding with normal mode...");
    }

    // Parse read mode
    let mode: MftReadMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;

    info!(
        drive = %drive_upper,
        output = %output.display(),
        mode = %mode,
        full,
        unique,
        "📂 Starting MFT read operation{}",
        if unique { " (unique FRS mode)" } else { " (expanding hard links)" }
    );

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("valid template"),
    );
    pb.set_message("Opening volume...");

    debug!(drive = %drive_upper, "🔓 Opening volume handle");
    let open_start = Instant::now();

    let reader = MftReader::open(drive)
        .await
        .with_context(|| format!("Failed to open drive {}:", drive))?
        .with_mode(mode)
        .with_merge_extensions(full)
        .with_expand_links(!unique); // unique=true means don't expand
    // Note: forensic mode is not yet supported for live reads (see warning above)

    info!(
        drive = %drive_upper,
        elapsed_ms = open_start.elapsed().as_millis(),
        "✅ Volume opened successfully"
    );

    pb.set_message("Reading MFT records...");
    debug!("📖 Starting MFT record enumeration");
    let read_start = Instant::now();

    let mut df = reader
        .read_all()
        .await
        .with_context(|| "Failed to read MFT")?;

    let record_count = df.height();
    let read_elapsed = read_start.elapsed();
    let records_per_sec = if read_elapsed.as_secs_f64() > 0.0 {
        record_count as f64 / read_elapsed.as_secs_f64()
    } else {
        0.0
    };

    info!(
        records = record_count,
        elapsed_ms = read_elapsed.as_millis(),
        records_per_sec = format!("{:.0}", records_per_sec),
        "✅ MFT read complete"
    );

    pb.set_message("Saving to Parquet...");
    debug!(output = %output.display(), "💾 Writing Parquet file");
    let save_start = Instant::now();

    MftReader::save_parquet(&mut df, &output).with_context(|| "Failed to save Parquet")?;

    // Get file size for logging
    let file_size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let file_size_mb = file_size as f64 / (1024.0 * 1024.0);

    info!(
        output = %output.display(),
        file_size_mb = format!("{:.2}", file_size_mb),
        elapsed_ms = save_start.elapsed().as_millis(),
        "✅ Parquet file saved"
    );

    let total_elapsed = start_time.elapsed();
    info!(
        drive = %drive_upper,
        records = record_count,
        total_elapsed_ms = total_elapsed.as_millis(),
        output_size_mb = format!("{:.2}", file_size_mb),
        "🎉 MFT export complete"
    );

    pb.finish_with_message(format!(
        "✅ Exported {} records to {} ({}) in {}",
        format_number_commas(record_count as u64),
        output.display(),
        format_bytes(file_size),
        format_duration(total_elapsed)
    ));

    Ok(())
}

#[cfg(windows)]
async fn cmd_info(drive: char, deep: bool, no_bitmap: bool, unique: bool) -> Result<()> {
    use std::time::Instant;

    use tracing::debug;
    use uffs_mft::platform::{VolumeHandle, detect_drive_type};

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();
    info!(
        drive = %drive_upper,
        deep,
        no_bitmap,
        unique,
        "📊 Retrieving MFT information{}{}{}",
        if deep { " (deep scan)" } else { "" },
        if no_bitmap { " (bitmap disabled)" } else { "" },
        if unique { " (unique FRS mode)" } else { "" }
    );

    debug!(drive = %drive_upper, "🔓 Opening volume handle");
    let handle = VolumeHandle::open(drive).with_context(|| format!("Failed to open {}:", drive))?;

    // Detect drive type for display
    let drive_type = detect_drive_type(drive_upper);
    let drive_type_str = match drive_type {
        uffs_mft::DriveType::Nvme => "NVMe",
        uffs_mft::DriveType::Ssd => "SSD",
        uffs_mft::DriveType::Hdd => "HDD",
        uffs_mft::DriveType::Unknown => "Unknown",
    };
    debug!(drive = %drive_upper, drive_type = drive_type_str, "🚀 Drive type detected");

    let vol_data = handle.volume_data();

    // Calculate derived metrics
    let record_count =
        vol_data.mft_valid_data_length / vol_data.bytes_per_file_record_segment as u64;
    let mft_size_mb = vol_data.mft_valid_data_length as f64 / (1024.0 * 1024.0);
    let volume_size_bytes = vol_data.total_clusters * vol_data.bytes_per_cluster as u64;
    let volume_size_gb = volume_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let free_space_bytes = vol_data.free_clusters * vol_data.bytes_per_cluster as u64;
    let used_space_bytes = volume_size_bytes.saturating_sub(free_space_bytes);
    let free_percentage = if volume_size_bytes > 0 {
        (free_space_bytes as f64 / volume_size_bytes as f64) * 100.0
    } else {
        0.0
    };
    let mft_percentage = (vol_data.mft_valid_data_length as f64 / volume_size_bytes as f64) * 100.0;

    // Log detailed metrics
    info!(
        drive = %drive_upper,
        bytes_per_sector = vol_data.bytes_per_sector,
        bytes_per_cluster = vol_data.bytes_per_cluster,
        bytes_per_record = vol_data.bytes_per_file_record_segment,
        "📐 Volume geometry"
    );

    info!(
        drive = %drive_upper,
        total_clusters = vol_data.total_clusters,
        volume_size_gb = format!("{:.2}", volume_size_gb),
        "💾 Volume capacity"
    );

    info!(
        drive = %drive_upper,
        mft_start_lcn = vol_data.mft_start_lcn,
        mft_valid_length = vol_data.mft_valid_data_length,
        mft_size_mb = format!("{:.2}", mft_size_mb),
        estimated_records = record_count,
        mft_percentage = format!("{:.3}%", mft_percentage),
        "📁 MFT metrics"
    );

    // Fragmentation analysis
    let mut extent_count = 1;
    let mut is_fragmented = false;
    if let Ok(extents) = handle.get_mft_extents() {
        extent_count = extents.len();
        is_fragmented = extent_count > 1;

        if is_fragmented {
            info!(
                drive = %drive_upper,
                extent_count,
                "⚠️  MFT is fragmented across multiple extents"
            );
            debug!("MFT extent details:");
            for (i, ext) in extents.iter().enumerate() {
                debug!(
                    extent = i,
                    vcn = ext.vcn,
                    lcn = ext.lcn,
                    clusters = ext.cluster_count,
                    "  Extent {}: VCN {} → LCN {}, {} clusters",
                    i,
                    ext.vcn,
                    ext.lcn,
                    ext.cluster_count
                );
            }
        } else {
            info!(
                drive = %drive_upper,
                "✅ MFT is contiguous (single extent)"
            );
        }
    }

    // Bitmap analysis
    let mut in_use_records = 0u64;
    let mut free_records = 0u64;
    let mut utilization = 0.0f64;
    if let Ok(bitmap) = handle.get_mft_bitmap() {
        in_use_records = bitmap.count_in_use() as u64;
        free_records = record_count.saturating_sub(in_use_records);
        utilization = (in_use_records as f64 / record_count as f64) * 100.0;

        info!(
            drive = %drive_upper,
            in_use_records,
            free_records,
            utilization = format!("{:.1}%", utilization),
            "📈 MFT utilization"
        );
    }

    // Health assessment (based on metadata only - no full scan)
    let mut warnings = Vec::new();
    if is_fragmented && extent_count > 10 {
        warnings.push(format!(
            "MFT is heavily fragmented ({} extents)",
            extent_count
        ));
    }
    if utilization > 95.0 {
        warnings.push(format!(
            "MFT utilization is very high ({:.1}%)",
            utilization
        ));
    }

    let elapsed = start_time.elapsed();

    // Print human-readable summary
    println!("═══════════════════════════════════════════════════════════════");
    if deep {
        println!("                    MFT ANALYSIS REPORT");
    } else {
        println!("                    MFT INFO (Lightweight)");
    }
    println!(
        "                    Drive: {}: ({})",
        drive_upper, drive_type_str
    );
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("📐 VOLUME GEOMETRY");
    println!("  Drive type:           {}", drive_type_str);
    println!(
        "  Bytes per sector:     {}",
        format_number_commas(vol_data.bytes_per_sector.into())
    );
    println!(
        "  Bytes per cluster:    {}",
        format_number_commas(vol_data.bytes_per_cluster.into())
    );
    println!(
        "  Bytes per MFT record: {}",
        format_number_commas(vol_data.bytes_per_file_record_segment.into())
    );
    println!(
        "  Total clusters:       {}",
        format_number_commas(vol_data.total_clusters)
    );
    println!("  Volume size:         {}", format_bytes(volume_size_bytes));
    println!("  Used space:          {}", format_bytes(used_space_bytes));
    println!(
        "  Free space:          {} ({:.1}%)",
        format_bytes(free_space_bytes),
        free_percentage
    );
    println!();
    println!("📁 MFT STRUCTURE");
    println!(
        "  MFT start LCN:        {}",
        format_number_commas(vol_data.mft_start_lcn)
    );
    println!(
        "  MFT size:            {}",
        format_bytes(vol_data.mft_valid_data_length)
    );
    println!("  MFT % of volume:      {:.3}%", mft_percentage);
    println!(
        "  Total records:        {}",
        format_number_commas(record_count)
    );
    println!(
        "  In-use records:       {}",
        format_number_commas(in_use_records)
    );
    println!(
        "  Free records:         {}",
        format_number_commas(free_records)
    );
    println!("  Utilization:          {:.1}%", utilization);
    println!(
        "  Fragmentation:        {} extent(s) {}",
        extent_count,
        if is_fragmented { "⚠️" } else { "✅" }
    );
    println!();

    if !warnings.is_empty() {
        println!("⚠️  HEALTH WARNINGS");
        for warning in &warnings {
            println!("  • {}", warning);
        }
        println!();
    } else {
        println!("✅ HEALTH STATUS: Good (based on metadata)");
        println!();
    }

    // Deep scan: read all MFT records for detailed statistics
    if deep {
        println!(
            "📊 DEEP SCAN: Reading all MFT records{}{}...",
            if no_bitmap { " (bitmap disabled)" } else { "" },
            if unique {
                " (unique FRS mode)"
            } else {
                " (expanding hard links)"
            }
        );
        println!();

        let reader = MftReader::open(drive)
            .await
            .with_context(|| format!("Failed to open drive {}:", drive))?
            .with_use_bitmap(!no_bitmap)
            .with_expand_links(!unique); // unique=true means don't expand

        let df = reader
            .read_all()
            .await
            .with_context(|| "Failed to read MFT")?;

        let total_parsed = df.height();

        // Extract statistics from the DataFrame
        let dir_count = df
            .column("is_directory")
            .ok()
            .and_then(|c| c.bool().ok())
            .map(|b| b.sum().unwrap_or(0) as u64)
            .unwrap_or(0);
        let file_count = total_parsed as u64 - dir_count;

        // Helper closure to count bool columns
        let count_bool = |name: &str| -> u64 {
            df.column(name)
                .ok()
                .and_then(|c| c.bool().ok())
                .map(|b| b.sum().unwrap_or(0) as u64)
                .unwrap_or(0)
        };

        let hidden_count = count_bool("is_hidden");
        let system_count = count_bool("is_system");
        let compressed_count = count_bool("is_compressed");
        let encrypted_count = count_bool("is_encrypted");
        let sparse_count = count_bool("is_sparse");
        let reparse_count = count_bool("is_reparse");
        let readonly_count = count_bool("is_readonly");
        let archive_count = count_bool("is_archive");

        // Count multi-stream and multi-name files, and total names/streams
        let (multi_stream_count, total_stream_count) = df
            .column("stream_count")
            .ok()
            .and_then(|c| c.u16().ok())
            .map(|s| {
                let mut multi = 0u64;
                let mut total = 0u64;
                for v in s.iter().flatten() {
                    total += v as u64;
                    if v > 1 {
                        multi += 1;
                    }
                }
                (multi, total)
            })
            .unwrap_or((0, 0));
        let (multi_name_count, total_name_count) = df
            .column("name_count")
            .ok()
            .and_then(|c| c.u16().ok())
            .map(|s| {
                let mut multi = 0u64;
                let mut total = 0u64;
                for v in s.iter().flatten() {
                    total += v as u64;
                    if v > 1 {
                        multi += 1;
                    }
                }
                (multi, total)
            })
            .unwrap_or((0, 0));

        // Calculate C++ equivalent count (names × streams per record)
        // This is what C++ outputs: one row per (name, stream) combination
        let cpp_equivalent_count = df
            .column("name_count")
            .ok()
            .and_then(|c| c.u16().ok())
            .and_then(|names| {
                df.column("stream_count")
                    .ok()
                    .and_then(|c| c.u16().ok())
                    .map(|streams| {
                        names
                            .iter()
                            .zip(streams.iter())
                            .filter_map(|(n, s)| match (n, s) {
                                (Some(n), Some(s)) => Some(n as u64 * s as u64),
                                _ => None,
                            })
                            .sum::<u64>()
                    })
            })
            .unwrap_or(0);

        // Calculate total sizes
        let total_file_size: u64 = df
            .column("size")
            .ok()
            .and_then(|c| c.u64().ok())
            .map(|s| s.iter().flatten().sum::<u64>())
            .unwrap_or(0);
        let total_allocated_size: u64 = df
            .column("allocated_size")
            .ok()
            .and_then(|c| c.u64().ok())
            .map(|s| s.iter().flatten().sum::<u64>())
            .unwrap_or(0);

        let slack_space = total_allocated_size.saturating_sub(total_file_size);
        let slack_percentage = if total_allocated_size > 0 {
            (slack_space as f64 / total_allocated_size as f64) * 100.0
        } else {
            0.0
        };

        println!("📊 FILE SYSTEM STATISTICS");
        println!(
            "  Parsed records:       {}",
            format_number_commas(total_parsed as u64)
        );
        println!(
            "  Directories:          {}",
            format_number_commas(dir_count)
        );
        println!(
            "  Files:                {}",
            format_number_commas(file_count)
        );
        println!();
        println!("🏷️  ATTRIBUTE FLAGS");
        println!(
            "  Hidden:               {}",
            format_number_commas(hidden_count)
        );
        println!(
            "  System:               {}",
            format_number_commas(system_count)
        );
        println!(
            "  Read-only:            {}",
            format_number_commas(readonly_count)
        );
        println!(
            "  Archive:              {}",
            format_number_commas(archive_count)
        );
        println!(
            "  Compressed:           {}",
            format_number_commas(compressed_count)
        );
        println!(
            "  Encrypted:            {}",
            format_number_commas(encrypted_count)
        );
        println!(
            "  Sparse:               {}",
            format_number_commas(sparse_count)
        );
        println!(
            "  Reparse points:       {}",
            format_number_commas(reparse_count)
        );
        println!();
        println!("🔗 EXTENDED ATTRIBUTES");
        println!(
            "  Files with ADS:       {} (Alternate Data Streams)",
            format_number_commas(multi_stream_count)
        );
        println!(
            "  Files with hardlinks: {}",
            format_number_commas(multi_name_count)
        );
        println!(
            "  Total names (links):  {}",
            format_number_commas(total_name_count)
        );
        println!(
            "  Total streams:        {}",
            format_number_commas(total_stream_count)
        );
        println!(
            "  C++ equivalent:       {} (names × streams)",
            format_number_commas(cpp_equivalent_count)
        );
        println!();
        println!("💾 STORAGE ANALYSIS");
        println!("  Total file size:     {}", format_bytes(total_file_size));
        println!(
            "  Total allocated:     {}",
            format_bytes(total_allocated_size)
        );
        println!(
            "  Slack space:         {} ({:.1}%)",
            format_bytes(slack_space),
            slack_percentage
        );
        println!();

        // =====================================================================
        // WINDOWS COMPARISON SECTION
        // Count files/folders the way Windows defrag does:
        // - Exclude hidden files
        // - Exclude system files
        // - Exclude NTFS metadata (names starting with $)
        // =====================================================================

        // Get column references for filtering
        let is_hidden_col = df.column("is_hidden").ok().and_then(|c| c.bool().ok());
        let is_system_col = df.column("is_system").ok().and_then(|c| c.bool().ok());
        let name_col = df.column("name").ok().and_then(|c| c.str().ok());
        let is_dir_col = df.column("is_directory").ok().and_then(|c| c.bool().ok());

        if let (Some(hidden), Some(system), Some(names), Some(is_dir)) =
            (is_hidden_col, is_system_col, name_col, is_dir_col)
        {
            // Count user-visible entries (not hidden, not system, not $ metadata)
            let mut win_dirs: u64 = 0;
            let mut win_files: u64 = 0;

            for i in 0..df.height() {
                let is_hidden = hidden.get(i).unwrap_or(false);
                let is_system = system.get(i).unwrap_or(false);
                let name = names.get(i).unwrap_or("");
                let is_directory = is_dir.get(i).unwrap_or(false);

                // Skip hidden, system, and NTFS metadata files
                if is_hidden || is_system || name.starts_with('$') {
                    continue;
                }

                if is_directory {
                    win_dirs += 1;
                } else {
                    win_files += 1;
                }
            }

            let win_total = win_dirs + win_files;

            println!("🪟 WINDOWS COMPARISON");
            println!("  (Excludes hidden, system, and NTFS metadata files)");
            println!("  Folders:              {}", format_number_commas(win_dirs));
            println!(
                "  Files:                {}",
                format_number_commas(win_files)
            );
            println!(
                "  Total movable:        {}",
                format_number_commas(win_total)
            );
            println!();
        }

        let deep_elapsed = start_time.elapsed();
        println!(
            "⏱️  Deep scan completed in {}",
            format_duration(deep_elapsed)
        );
    } else {
        println!("💡 TIP: Use --deep for detailed file statistics (dirs, files, attributes).");
        println!();
        println!("⏱️  Completed in {}", format_duration(elapsed));
    }

    println!("═══════════════════════════════════════════════════════════════");

    Ok(())
}

#[cfg(windows)]
async fn cmd_drives() -> Result<()> {
    use tracing::debug;
    use uffs_mft::platform::{VolumeHandle, detect_drive_type, detect_ntfs_drives};

    info!("🔍 Detecting NTFS drives...");

    let drives = detect_ntfs_drives();

    if drives.is_empty() {
        info!("❌ No NTFS drives found");
        println!("No NTFS drives found.");
    } else {
        info!(
            count = drives.len(),
            "✅ Found {} NTFS drive(s)",
            drives.len()
        );

        // Collect drive info
        struct DriveInfo {
            letter: char,
            label: String,
            drive_type: String,
            total_size: u64,
            free_space: u64,
            used_space: u64,
            used_pct: f64,
            mft_size: u64,
            mft_records: u64,
        }

        let mut drive_infos: Vec<DriveInfo> = Vec::new();

        for drive in &drives {
            // Detect drive type
            let drive_type = detect_drive_type(*drive);
            let drive_type_str = match drive_type {
                uffs_mft::DriveType::Nvme => "NVMe",
                uffs_mft::DriveType::Ssd => "SSD",
                uffs_mft::DriveType::Hdd => "HDD",
                uffs_mft::DriveType::Unknown => "???",
            };

            // Get volume label
            let label = get_volume_label(*drive).unwrap_or_default();

            // Try to get volume info for each drive
            if let Ok(handle) = VolumeHandle::open(*drive) {
                let vol_data = handle.volume_data();
                let total_size = vol_data.total_clusters as u64 * vol_data.bytes_per_cluster as u64;
                let free_space = vol_data.free_clusters as u64 * vol_data.bytes_per_cluster as u64;
                let used_space = total_size.saturating_sub(free_space);
                let used_pct = if total_size > 0 {
                    (used_space as f64 / total_size as f64) * 100.0
                } else {
                    0.0
                };
                let mft_size = vol_data.mft_valid_data_length;
                let mft_records = mft_size / vol_data.bytes_per_file_record_segment as u64;

                debug!(
                    drive = %drive,
                    label = %label,
                    drive_type = drive_type_str,
                    total_size,
                    free_space,
                    mft_records,
                    "📁 Drive details"
                );

                drive_infos.push(DriveInfo {
                    letter: *drive,
                    label,
                    drive_type: drive_type_str.to_string(),
                    total_size,
                    free_space,
                    used_space,
                    used_pct,
                    mft_size,
                    mft_records,
                });
            }
        }

        // Print table header
        println!();
        println!(
            "═══════════════════════════════════════════════════════════════════════════════════════════════════"
        );
        println!("                                    NTFS DRIVES SUMMARY");
        println!(
            "═══════════════════════════════════════════════════════════════════════════════════════════════════"
        );
        println!();
        println!(
            "{:<6} {:<16} {:<5} {:>10} {:>10} {:>10} {:>7} {:>10} {:>12}",
            "Drive", "Label", "Type", "Size", "Used", "Free", "Used%", "MFT Size", "MFT Records"
        );
        println!(
            "{:-<6} {:-<16} {:-<5} {:->10} {:->10} {:->10} {:->7} {:->10} {:->12}",
            "", "", "", "", "", "", "", "", ""
        );

        // Print each drive
        for info in &drive_infos {
            println!(
                "{:<6} {:<16} {:<5} {:>10} {:>10} {:>10} {:>6.1}% {:>10} {:>12}",
                format!("{}:", info.letter),
                truncate_string(&info.label, 16),
                info.drive_type,
                format_bytes(info.total_size),
                format_bytes(info.used_space),
                format_bytes(info.free_space),
                info.used_pct,
                format_bytes(info.mft_size),
                format_number_commas(info.mft_records),
            );
        }

        // Print totals
        let total_size: u64 = drive_infos.iter().map(|d| d.total_size).sum();
        let total_used: u64 = drive_infos.iter().map(|d| d.used_space).sum();
        let total_free: u64 = drive_infos.iter().map(|d| d.free_space).sum();
        let total_mft: u64 = drive_infos.iter().map(|d| d.mft_size).sum();
        let total_records: u64 = drive_infos.iter().map(|d| d.mft_records).sum();
        let total_used_pct = if total_size > 0 {
            (total_used as f64 / total_size as f64) * 100.0
        } else {
            0.0
        };

        println!(
            "{:-<6} {:-<16} {:-<5} {:->10} {:->10} {:->10} {:->7} {:->10} {:->12}",
            "", "", "", "", "", "", "", "", ""
        );
        println!(
            "{:<6} {:<16} {:<5} {:>10} {:>10} {:>10} {:>6.1}% {:>10} {:>12}",
            "TOTAL",
            format!("({} drives)", drive_infos.len()),
            "",
            format_bytes(total_size),
            format_bytes(total_used),
            format_bytes(total_free),
            total_used_pct,
            format_bytes(total_mft),
            format_number_commas(total_records),
        );
        println!();
    }

    Ok(())
}

/// Gets the volume label for a drive letter.
#[cfg(windows)]
#[allow(unsafe_code)] // Required: Windows FFI (GetVolumeInformationW)
fn get_volume_label(drive: char) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    use windows::core::PCWSTR;

    let root_path: Vec<u16> = format!("{}:\\", drive)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut volume_name_buf = [0u16; 261];

    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR::from_raw(root_path.as_ptr()),
            Some(&mut volume_name_buf),
            None,
            None,
            None,
            None,
        )
    };

    if result.is_ok() {
        let len = volume_name_buf.iter().position(|&c| c == 0).unwrap_or(0);
        let label = OsString::from_wide(&volume_name_buf[..len]);
        Some(label.to_string_lossy().to_string())
    } else {
        None
    }
}

/// Truncates a string to a maximum length, adding "..." if truncated.
#[cfg(windows)]
fn truncate_string(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_owned()
    } else if max_len <= 3 {
        text.chars().take(max_len).collect()
    } else {
        // Use char boundary-safe truncation
        let truncate_at = max_len - 3;
        let safe_end = text
            .char_indices()
            .take_while(|(idx, _)| *idx < truncate_at)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        format!("{}...", &text[..safe_end])
    }
}

// ============================================================================
// Benchmark Command
// ============================================================================

#[cfg(windows)]
async fn cmd_bench(
    drive: char,
    json: bool,
    no_df: bool,
    runs: u32,
    mode_str: &str,
    full: bool,
) -> Result<()> {
    use uffs_mft::{BenchmarkResult, MftReadMode, MftReader};

    let drive_upper = drive.to_ascii_uppercase();
    let runs = runs.max(1);

    // Parse read mode
    let mode: MftReadMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;

    if !json {
        println!("🔬 Benchmarking MFT read on drive {}:", drive_upper);
        println!("   Runs: {}", runs);
        println!("   Skip DataFrame: {}", no_df);
        println!("   Mode: {}", mode);
        println!("   Full (merge extensions): {}", full);
        println!();
    }

    info!(
        drive = %drive_upper,
        runs,
        skip_df = no_df,
        mode = %mode,
        full,
        "📊 Starting benchmark"
    );

    // Open the reader once (opening is fast, we don't need to re-open for each run)
    let reader = MftReader::open(drive)
        .await
        .with_context(|| format!("Failed to open drive {}:", drive))?
        .with_mode(mode)
        .with_merge_extensions(full);

    let mut results: Vec<BenchmarkResult> = Vec::with_capacity(runs as usize);

    for run in 1..=runs {
        if !json && runs > 1 {
            println!("  Run {}/{}...", run, runs);
        }

        let (_, result) = reader
            .read_with_timing(no_df)
            .await
            .with_context(|| format!("Benchmark run {} failed", run))?;

        info!(
            run,
            total_ms = result.timings.total_ms,
            throughput_mb_s = format!("{:.1}", result.throughput_mb_s),
            "✅ Run complete"
        );

        results.push(result);

        // Small delay between runs to let system settle
        if run < runs {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    // Calculate averages if multiple runs
    let avg_result = if runs == 1 {
        results.into_iter().next().unwrap()
    } else {
        average_results(&results)
    };

    if json {
        println!("{}", avg_result.to_json());
    } else {
        print_benchmark_result(&avg_result, runs);
    }

    Ok(())
}

#[cfg(windows)]
fn average_results(results: &[uffs_mft::BenchmarkResult]) -> uffs_mft::BenchmarkResult {
    let n = results.len() as u64;
    if n == 0 {
        panic!("No results to average");
    }

    let first = &results[0];

    let avg_timings = uffs_mft::PhaseTimings {
        open_ms: results.iter().map(|r| r.timings.open_ms).sum::<u64>() / n,
        read_ms: results.iter().map(|r| r.timings.read_ms).sum::<u64>() / n,
        parse_ms: results.iter().map(|r| r.timings.parse_ms).sum::<u64>() / n,
        merge_ms: results.iter().map(|r| r.timings.merge_ms).sum::<u64>() / n,
        df_build_ms: results.iter().map(|r| r.timings.df_build_ms).sum::<u64>() / n,
        total_ms: results.iter().map(|r| r.timings.total_ms).sum::<u64>() / n,
    };

    let avg_throughput: f64 =
        results.iter().map(|r| r.throughput_mb_s).sum::<f64>() / results.len() as f64;
    let avg_records_per_sec: f64 =
        results.iter().map(|r| r.records_per_sec).sum::<f64>() / results.len() as f64;

    uffs_mft::BenchmarkResult {
        timings: avg_timings,
        characteristics: first.characteristics.clone(),
        records_parsed: first.records_parsed,
        throughput_mb_s: avg_throughput,
        records_per_sec: avg_records_per_sec,
    }
}

#[cfg(windows)]
fn print_benchmark_result(result: &uffs_mft::BenchmarkResult, runs: u32) {
    let c = &result.characteristics;
    let t = &result.timings;

    println!("═══════════════════════════════════════════════════════════════");
    println!("                    MFT BENCHMARK RESULTS");
    println!("═══════════════════════════════════════════════════════════════");
    println!();

    // Drive characteristics
    println!("📁 DRIVE CHARACTERISTICS");
    println!("   Drive:            {}:", c.drive_letter);
    println!("   Type:             {}", c.drive_type);
    println!(
        "   MFT Size:         {} MB",
        c.mft_size_bytes / (1024 * 1024)
    );
    println!(
        "   Total Records:    {}",
        format_number_commas(c.total_records)
    );
    if let Some(in_use) = c.in_use_records {
        let skip_pct = 100.0 - (in_use as f64 / c.total_records as f64 * 100.0);
        println!(
            "   In-Use Records:   {} ({:.1}% skipped)",
            format_number_commas(in_use),
            skip_pct
        );
    }
    println!("   Extents:          {} (fragmentation)", c.extent_count);
    println!("   Record Size:      {} bytes", c.bytes_per_record);
    println!(
        "   Chunk Size:       {} MB",
        c.chunk_size_bytes / (1024 * 1024)
    );
    println!("   Chunks:           {}", c.chunk_count);
    println!();

    // Phase timings
    println!(
        "⏱️  PHASE TIMINGS{}",
        if runs > 1 { " (averaged)" } else { "" }
    );
    println!("   Open:             {:>8} ms", t.open_ms);
    println!("   Read (I/O):       {:>8} ms  ← estimated", t.read_ms);
    println!("   Parse (CPU):      {:>8} ms  ← estimated", t.parse_ms);
    println!("   Merge:            {:>8} ms  ← estimated", t.merge_ms);
    println!("   DataFrame Build:  {:>8} ms", t.df_build_ms);
    println!("   ─────────────────────────────");
    println!("   TOTAL:            {:>8} ms", t.total_ms);
    println!();

    // Note about estimates
    println!("   ⚠️  Read/Parse/Merge are currently estimated (not instrumented).");
    println!("      Implement M0 instrumentation for accurate phase breakdown.");
    println!();

    // Throughput
    println!("🚀 THROUGHPUT");
    println!(
        "   Records/sec:      {}",
        format_number_commas(result.records_per_sec as u64)
    );
    println!("   MB/sec:           {:.1}", result.throughput_mb_s);
    println!(
        "   Records Parsed:   {}",
        format_number_commas(result.records_parsed as u64)
    );
    println!();

    // Bottleneck analysis hint
    println!("📊 BOTTLENECK HINT");
    if c.drive_type.contains("Hdd") {
        println!("   HDD detected: I/O is likely the bottleneck.");
        println!("   Focus on: Prefetch, overlapped I/O, chunk size tuning.");
    } else if c.drive_type.contains("Ssd") {
        println!("   SSD detected: CPU (parse/df_build) may be the bottleneck.");
        println!("   Focus on: Rayon tuning, fold/reduce, SoA layout.");
    } else {
        println!("   Unknown drive type. Measure to determine bottleneck.");
    }
    println!();

    println!("═══════════════════════════════════════════════════════════════");
}

// ============================================================================
// Benchmark All Drives Command
// ============================================================================

/// Combined benchmark report for all drives.
#[cfg(windows)]
#[derive(Debug)]
struct FullBenchmarkReport {
    /// Timestamp when benchmark started.
    timestamp: String,
    /// Hostname of the machine.
    hostname: String,
    /// Number of logical CPUs.
    cpu_count: usize,
    /// UFFS version.
    uffs_version: String,
    /// Individual drive results.
    drives: Vec<uffs_mft::BenchmarkResult>,
    /// Total time for all benchmarks.
    total_benchmark_time_ms: u64,
}

#[cfg(windows)]
impl FullBenchmarkReport {
    fn to_json(&self) -> String {
        let drives_json: Vec<String> = self.drives.iter().map(|d| d.to_json()).collect();
        format!(
            r#"{{
  "metadata": {{
    "timestamp": "{}",
    "hostname": "{}",
    "cpu_count": {},
    "uffs_version": "{}",
    "total_benchmark_time_ms": {}
  }},
  "drives": [
    {}
  ]
}}"#,
            self.timestamp,
            self.hostname,
            self.cpu_count,
            self.uffs_version,
            self.total_benchmark_time_ms,
            drives_json.join(",\n    ")
        )
    }
}

#[cfg(windows)]
async fn cmd_bench_all(output: Option<PathBuf>, no_df: bool, runs: u32, full: bool) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::detect_ntfs_drives;

    let total_start = Instant::now();
    let runs = runs.max(1);

    // Generate default output filename with timestamp
    let output_path = output.unwrap_or_else(|| {
        let now = chrono::Local::now();
        PathBuf::from(format!(
            "uffs_benchmark_{}.json",
            now.format("%Y%m%d_%H%M%S")
        ))
    });

    println!("═══════════════════════════════════════════════════════════════");
    println!("              UFFS MFT BENCHMARK - ALL DRIVES");
    println!("═══════════════════════════════════════════════════════════════");
    println!();

    // Detect all NTFS drives
    let drives = detect_ntfs_drives();
    if drives.is_empty() {
        println!("❌ No NTFS drives found.");
        return Ok(());
    }

    println!(
        "📁 Found {} NTFS drive(s): {}",
        drives.len(),
        drives
            .iter()
            .map(|d| format!("{}:", d))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("📊 Runs per drive: {}", runs);
    println!("📄 Output file: {}", output_path.display());
    println!("⏳ Skip DataFrame: {}", no_df);
    println!("🔗 Full (merge extensions): {}", full);
    println!();

    info!(
        drives = ?drives,
        runs,
        output = %output_path.display(),
        full,
        "📊 Starting full benchmark"
    );

    let mut results: Vec<uffs_mft::BenchmarkResult> = Vec::with_capacity(drives.len());

    for (idx, drive) in drives.iter().enumerate() {
        println!("─────────────────────────────────────────────────────────────────");
        println!(
            "  [{}/{}] Benchmarking drive {}:",
            idx + 1,
            drives.len(),
            drive
        );
        println!("─────────────────────────────────────────────────────────────────");

        match benchmark_single_drive(*drive, no_df, runs, full).await {
            Ok(result) => {
                // Print summary for this drive
                println!("  ✅ Drive {}:", drive);
                println!(
                    "     Records:     {}",
                    format_number_commas(result.records_parsed as u64)
                );
                println!("     Total time:  {} ms", result.timings.total_ms);
                println!("     Throughput:  {:.1} MB/s", result.throughput_mb_s);
                println!("     Type:        {}", result.characteristics.drive_type);
                println!();
                results.push(result);
            }
            Err(e) => {
                println!("  ❌ Drive {}: Failed - {}", drive, e);
                println!();
                warn!(drive = %drive, error = ?e, "Benchmark failed for drive");
            }
        }
    }

    let total_time_ms = total_start.elapsed().as_millis() as u64;

    // Build full report
    let report = FullBenchmarkReport {
        timestamp: chrono::Local::now().to_rfc3339(),
        hostname: hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string()),
        cpu_count: num_cpus::get(),
        uffs_version: env!("CARGO_PKG_VERSION").to_string(),
        drives: results,
        total_benchmark_time_ms: total_time_ms,
    };

    // Write to file
    let json = report.to_json();
    std::fs::write(&output_path, &json).with_context(|| {
        format!(
            "Failed to write benchmark results to {}",
            output_path.display()
        )
    })?;

    println!("═══════════════════════════════════════════════════════════════");
    println!("                      BENCHMARK COMPLETE");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("  📊 Drives benchmarked: {}", report.drives.len());
    println!(
        "  ⏱️  Total time:         {} ms ({:.1} sec)",
        total_time_ms,
        total_time_ms as f64 / 1000.0
    );
    println!("  📄 Results saved to:   {}", output_path.display());
    println!();
    println!("  Share this file for optimization analysis!");
    println!();

    info!(
        drives_benchmarked = report.drives.len(),
        total_time_ms,
        output = %output_path.display(),
        "✅ Full benchmark complete"
    );

    Ok(())
}

#[cfg(windows)]
async fn benchmark_single_drive(
    drive: char,
    no_df: bool,
    runs: u32,
    full: bool,
) -> Result<uffs_mft::BenchmarkResult> {
    use uffs_mft::MftReader;

    let reader = MftReader::open(drive)
        .await
        .with_context(|| format!("Failed to open drive {}:", drive))?
        .with_merge_extensions(full);

    let mut results: Vec<uffs_mft::BenchmarkResult> = Vec::with_capacity(runs as usize);

    for run in 1..=runs {
        if runs > 1 {
            println!("     Run {}/{}...", run, runs);
        }

        let (_, result) = reader
            .read_with_timing(no_df)
            .await
            .with_context(|| format!("Benchmark run {} failed", run))?;

        results.push(result);

        // Small delay between runs
        if run < runs {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    // Average results
    Ok(if runs == 1 {
        results.into_iter().next().unwrap()
    } else {
        average_results(&results)
    })
}

// ============================================================================
// Bitmap Diagnostic Command
// ============================================================================

/// Diagnose MFT bitmap to investigate why records aren't being skipped.
#[cfg(windows)]
async fn cmd_bitmap_diag(drive: char, show_samples: bool) -> Result<()> {
    use uffs_mft::VolumeHandle;

    let drive_upper = drive.to_ascii_uppercase();

    println!("═══════════════════════════════════════════════════════════════");
    println!(
        "              MFT BITMAP DIAGNOSTIC - Drive {}:",
        drive_upper
    );
    println!("═══════════════════════════════════════════════════════════════");
    println!();

    // Open volume
    let handle = VolumeHandle::open(drive_upper)
        .with_context(|| format!("Failed to open volume {}:", drive_upper))?;

    let volume_data = handle.volume_data();
    let record_size = volume_data.bytes_per_file_record_segment as u32;
    let mft_size = volume_data.mft_valid_data_length as u64;
    let total_records_from_size = mft_size / u64::from(record_size);

    println!("📊 VOLUME DATA");
    println!(
        "   MFT valid data length: {} bytes ({:.2} MB)",
        mft_size,
        mft_size as f64 / 1024.0 / 1024.0
    );
    println!("   Bytes per record: {}", record_size);
    println!("   Total records (from size): {}", total_records_from_size);
    println!();

    // Try to get bitmap with verbose output
    println!("📋 BITMAP RETRIEVAL (via get_mft_bitmap_verbose)");
    println!();
    match handle.get_mft_bitmap_verbose() {
        Ok(bitmap) => {
            let bitmap_bytes = bitmap.as_bytes().len();
            let bitmap_record_count = bitmap.record_count();
            let in_use_count = bitmap.count_in_use();
            let free_count = bitmap_record_count.saturating_sub(in_use_count);
            let utilization = (in_use_count as f64 / bitmap_record_count as f64) * 100.0;

            println!("   ✅ Bitmap retrieved successfully");
            println!("   Bitmap size: {} bytes", bitmap_bytes);
            println!("   Records covered: {}", bitmap_record_count);
            println!("   In-use records: {}", in_use_count);
            println!("   Free records: {}", free_count);
            println!("   Utilization: {:.2}%", utilization);
            println!();

            // Check for anomalies
            println!("🔍 ANOMALY DETECTION");

            // Check if all bits are set (0xFF bytes)
            let all_ff_bytes = bitmap.as_bytes().iter().filter(|&&b| b == 0xFF).count();
            let all_00_bytes = bitmap.as_bytes().iter().filter(|&&b| b == 0x00).count();
            let mixed_bytes = bitmap_bytes - all_ff_bytes - all_00_bytes;

            println!(
                "   Bytes with all bits set (0xFF): {} ({:.1}%)",
                all_ff_bytes,
                (all_ff_bytes as f64 / bitmap_bytes as f64) * 100.0
            );
            println!(
                "   Bytes with no bits set (0x00): {} ({:.1}%)",
                all_00_bytes,
                (all_00_bytes as f64 / bitmap_bytes as f64) * 100.0
            );
            println!(
                "   Mixed bytes: {} ({:.1}%)",
                mixed_bytes,
                (mixed_bytes as f64 / bitmap_bytes as f64) * 100.0
            );
            println!();

            if all_ff_bytes == bitmap_bytes {
                println!("   ⚠️  WARNING: ALL bytes are 0xFF!");
                println!("      This suggests the bitmap is a fallback (new_all_valid)");
                println!("      or the $MFT::$BITMAP read failed silently.");
            } else if in_use_count == bitmap_record_count {
                println!("   ⚠️  WARNING: in_use == record_count but not all 0xFF");
                println!("      This is unexpected - investigating...");
            } else if free_count > 0 {
                println!(
                    "   ✅ Bitmap shows {} free records ({:.1}% free)",
                    free_count,
                    (free_count as f64 / bitmap_record_count as f64) * 100.0
                );
            }
            println!();

            // Sample first few bytes
            println!("📝 BITMAP SAMPLE (first 32 bytes)");
            let sample_bytes: Vec<_> = bitmap.as_bytes().iter().take(32).collect();
            print!("   ");
            for (i, &byte) in sample_bytes.iter().enumerate() {
                print!("{:02X} ", byte);
                if (i + 1) % 16 == 0 {
                    println!();
                    if i < 31 {
                        print!("   ");
                    }
                }
            }
            if sample_bytes.len() % 16 != 0 {
                println!();
            }
            println!();

            // Sample last few bytes (often where free records are)
            if bitmap_bytes > 32 {
                println!("📝 BITMAP SAMPLE (last 32 bytes)");
                let last_bytes: Vec<_> = bitmap.as_bytes().iter().rev().take(32).collect();
                print!("   ");
                for (i, &byte) in last_bytes.iter().rev().enumerate() {
                    print!("{:02X} ", byte);
                    if (i + 1) % 16 == 0 {
                        println!();
                        if i < 31 {
                            print!("   ");
                        }
                    }
                }
                if last_bytes.len() % 16 != 0 {
                    println!();
                }
                println!();
            }

            // Check individual record samples
            if show_samples {
                println!("📝 INDIVIDUAL RECORD SAMPLES");
                println!("   Checking records 0-15:");
                print!("   ");
                for frs in 0..16u64 {
                    let in_use = bitmap.is_record_in_use(frs);
                    print!("{}: {} ", frs, if in_use { "✓" } else { "✗" });
                }
                println!();

                // Check some records in the middle
                let mid = bitmap_record_count / 2;
                println!("   Checking records {}-{}:", mid, mid + 15);
                print!("   ");
                for frs in mid..(mid + 16).min(bitmap_record_count) {
                    let in_use = bitmap.is_record_in_use(frs as u64);
                    print!("{}: {} ", frs, if in_use { "✓" } else { "✗" });
                }
                println!();

                // Check last records
                let last_start = bitmap_record_count.saturating_sub(16);
                println!(
                    "   Checking records {}-{}:",
                    last_start,
                    bitmap_record_count - 1
                );
                print!("   ");
                for frs in last_start..bitmap_record_count {
                    let in_use = bitmap.is_record_in_use(frs as u64);
                    print!("{}: {} ", frs, if in_use { "✓" } else { "✗" });
                }
                println!();
                println!();
            }

            // Test calculate_skip_range
            println!("📝 SKIP RANGE CALCULATION TEST");
            let test_ranges = [
                (0u64, 1000u64),
                (1000, 2000),
                (
                    total_records_from_size.saturating_sub(1000),
                    total_records_from_size,
                ),
            ];
            for (start, end) in test_ranges {
                let (skip_begin, skip_end) = bitmap.calculate_skip_range(start, end);
                let range_size = end - start;
                let skipped = skip_begin + skip_end;
                println!(
                    "   Range [{}, {}): skip_begin={}, skip_end={}, skipped={}/{} ({:.1}%)",
                    start,
                    end,
                    skip_begin,
                    skip_end,
                    skipped,
                    range_size,
                    (skipped as f64 / range_size as f64) * 100.0
                );
            }
            println!();
        }
        Err(e) => {
            println!("   ❌ Failed to retrieve bitmap: {}", e);
            println!("   This means the fallback (all records valid) would be used.");
            println!();
        }
    }

    println!("═══════════════════════════════════════════════════════════════");

    Ok(())
}

/// Bitmap diagnostic stub for non-Windows platforms.
#[cfg(not(windows))]
#[allow(dead_code)]
fn cmd_bitmap_diag(_drive: char, _show_samples: bool) -> impl Future<Output = Result<()>> {
    core::future::ready(Err(anyhow::anyhow!(
        "Bitmap diagnostic is only available on Windows"
    )))
}

// ============================================================================
// Save/Load Raw MFT Commands
// ============================================================================

/// Save MFT bytes to a file for offline analysis.
#[cfg(windows)]
async fn cmd_save(
    drive: char,
    output: &Path,
    compress: bool,
    compression_level: i32,
    raw_compat: bool,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::{VolumeHandle, detect_drive_type};
    use uffs_mft::{MftReader, SaveRawOptions};

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    info!(drive = %drive_upper, "Reading raw MFT from drive");

    // Get volume info for display
    let handle = VolumeHandle::open(drive).with_context(|| format!("Failed to open {}:", drive))?;
    let vol_data = handle.volume_data();

    let drive_type = detect_drive_type(drive_upper);
    let drive_type_str = match drive_type {
        uffs_mft::DriveType::Nvme => "NVMe",
        uffs_mft::DriveType::Ssd => "SSD",
        uffs_mft::DriveType::Hdd => "HDD",
        uffs_mft::DriveType::Unknown => "Unknown",
    };

    // Calculate metrics
    let record_count =
        vol_data.mft_valid_data_length / vol_data.bytes_per_file_record_segment as u64;

    // Fragmentation analysis
    let mut extent_count = 1;
    let is_fragmented;
    if let Ok(extents) = handle.get_mft_extents() {
        extent_count = extents.len();
        is_fragmented = extent_count > 1;
    } else {
        is_fragmented = false;
    }

    // Bitmap analysis
    let mut in_use_records = 0u64;
    let mut utilization = 0.0f64;
    if let Ok(bitmap) = handle.get_mft_bitmap() {
        in_use_records = bitmap.count_in_use() as u64;
        utilization = (in_use_records as f64 / record_count as f64) * 100.0;
    }
    let free_records = record_count.saturating_sub(in_use_records);

    // Open reader and save
    let reader = MftReader::open(drive)
        .await
        .with_context(|| format!("Failed to open drive {drive}:"))?;

    // Raw compat mode implies no compression
    let options = SaveRawOptions {
        compress: if raw_compat { false } else { compress },
        compression_level,
        volume_letter: drive_upper,
        raw_compat,
    };

    let header = reader
        .save_raw_to_file(output, &options)
        .await
        .with_context(|| format!("Failed to save raw MFT to {}", output.display()))?;

    let elapsed = start_time.elapsed();

    // Get absolute path for display
    let abs_path = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let abs_path = clean_path_for_display(&abs_path);

    // Print formatted output
    println!("═══════════════════════════════════════════════════════════════");
    println!("                         MFT SAVED");
    println!(
        "                    Drive: {}: ({})",
        drive_upper, drive_type_str
    );
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("📁 MFT STRUCTURE");
    println!(
        "  Total records:        {}",
        format_number_commas(record_count)
    );
    println!(
        "  In-use records:       {}",
        format_number_commas(in_use_records)
    );
    println!(
        "  Free records:         {}",
        format_number_commas(free_records)
    );
    println!("  Utilization:          {:.1}%", utilization);
    println!(
        "  Fragmentation:        {} extent(s) {}",
        extent_count,
        if is_fragmented { "⚠️" } else { "✅" }
    );
    println!();
    println!("💾 OUTPUT FILE");
    println!("  Path:                 {}", abs_path.display());
    println!(
        "  Original size:       {}",
        format_bytes(header.original_size)
    );
    if raw_compat {
        println!("  Format:               raw (compatible with other MFT tools)");
    } else if header.is_compressed() {
        println!(
            "  Compressed size:     {}",
            format_bytes(header.compressed_size)
        );
        #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
        let ratio = header.compressed_size as f64 / header.original_size as f64 * 100.0_f64;
        println!("  Compression ratio:    {ratio:.1}%");
        #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
        let savings = 100.0_f64 - ratio;
        println!("  Space saved:          {savings:.1}%");
    } else {
        println!("  Compression:          none");
        println!("  Volume letter:        {}:", header.volume_letter);
    }
    println!();
    println!("⏱️  Completed in {}", format_duration(elapsed));

    Ok(())
}

/// Load MFT from a saved file and optionally export.
///
/// Works on all platforms - parses NTFS structures from saved file.
/// Supports both UFFS-MFT format and raw NTFS format.
#[allow(
    clippy::too_many_lines,
    clippy::print_stdout,
    clippy::shadow_reuse,
    clippy::min_ident_chars,
    clippy::expect_used,
    clippy::single_call_fn,
    clippy::fn_params_excessive_bools
)] // CLI output function with complex display logic
fn cmd_load(
    input: &Path,
    output_path: Option<&Path>,
    info_only: bool,
    build_index: bool,
    debug_tree: bool,
    drive_override: Option<char>,
    forensic: bool,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::raw::LoadRawOptions;
    use uffs_mft::{MftReader, load_raw_mft};

    // Validate arguments upfront - don't print anything if we're going to fail
    if !info_only && !build_index && !debug_tree && output_path.is_none() {
        anyhow::bail!(
            "--output is required when not using --info-only, --build-index, or --debug-tree"
        );
    }

    let start_time = Instant::now();

    // Load header first (with volume letter override if provided)
    let load_options = LoadRawOptions {
        header_only: true,
        volume_letter: drive_override.map(|c| c.to_ascii_uppercase()),
        forensic,
    };
    let raw_data = load_raw_mft(input, &load_options)
        .with_context(|| format!("Failed to load raw MFT header from {}", input.display()))?;
    let header = raw_data.header;

    // Get absolute path and file size for display
    let abs_path = std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    let abs_path = clean_path_for_display(&abs_path);
    let file_size = std::fs::metadata(input).map_or(0, |meta| meta.len());

    // Determine format type for display
    let format_str = if header.version == 0 {
        "raw NTFS (compatible)"
    } else {
        "UFFS-MFT"
    };

    // Print formatted output
    println!("═══════════════════════════════════════════════════════════════");
    println!("                         MFT FILE INFO");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("📁 FILE DETAILS");
    println!("  Path:                 {}", abs_path.display());
    println!("  File size:           {}", format_bytes(file_size));
    if header.version == 0 {
        println!("  Format:               {format_str}");
    } else {
        println!("  Format:               {format_str} v{}", header.version);
    }
    println!("  Volume letter:        {}:", header.volume_letter);
    println!();
    println!("📊 MFT STRUCTURE");
    println!(
        "  Total records:        {}",
        format_number_commas(header.record_count)
    );
    println!(
        "  Bytes per record:     {}",
        format_number_commas(u64::from(header.record_size))
    );
    println!(
        "  Original MFT size:   {}",
        format_bytes(header.original_size)
    );
    println!();
    if header.version > 0 {
        println!("💾 COMPRESSION");
        if header.is_compressed() {
            println!(
                "  Compressed size:     {}",
                format_bytes(header.compressed_size)
            );
            #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
            let ratio = header.compressed_size as f64 / header.original_size as f64 * 100.0_f64;
            println!("  Compression ratio:    {ratio:.1}%");
            #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
            let savings = 100.0_f64 - ratio;
            println!("  Space saved:          {savings:.1}%");
        } else {
            println!("  Status:               uncompressed");
        }
    }

    // Create load options for data loading (not header-only)
    let data_load_options = LoadRawOptions {
        header_only: false,
        volume_letter: drive_override.map(|c| c.to_ascii_uppercase()),
        forensic,
    };

    // Print forensic mode warning if enabled
    if forensic {
        println!();
        println!("⚠️  FORENSIC MODE ENABLED");
        println!("  Including: deleted records, corrupt records, extension records");
        println!("  Output may contain 10-50% more rows than normal mode");
    }

    if info_only {
        // Parse the MFT to get detailed statistics
        println!();
        println!("📈 PARSING MFT FOR STATISTICS...");

        let df = MftReader::load_raw_to_dataframe_with_options(input, &data_load_options)
            .with_context(|| format!("Failed to parse raw MFT from {}", input.display()))?;

        let total_parsed = df.height();

        // Extract statistics from the DataFrame
        let dir_count = df
            .column("is_directory")
            .ok()
            .and_then(|col| col.bool().ok())
            .map_or(0, |bool_col| u64::from(bool_col.sum().unwrap_or(0)));
        let file_count = (total_parsed as u64).saturating_sub(dir_count);

        // Helper closure to count bool columns
        let count_bool = |name: &str| -> u64 {
            df.column(name)
                .ok()
                .and_then(|col| col.bool().ok())
                .map_or(0, |bool_col| u64::from(bool_col.sum().unwrap_or(0)))
        };

        let hidden_count = count_bool("is_hidden");
        let system_count = count_bool("is_system");
        let compressed_count = count_bool("is_compressed");
        let encrypted_count = count_bool("is_encrypted");
        let sparse_count = count_bool("is_sparse");

        // Total size calculation
        let total_size: u64 = df
            .column("size")
            .ok()
            .and_then(|col| col.u64().ok())
            .map_or(0, |size_col| size_col.iter().flatten().sum());

        println!();
        println!("📊 FILE STATISTICS");
        println!(
            "  Records parsed:       {}",
            format_number_commas(total_parsed as u64)
        );
        println!(
            "  Directories:          {}",
            format_number_commas(dir_count)
        );
        println!(
            "  Files:                {}",
            format_number_commas(file_count)
        );
        println!("  Total file size:     {}", format_bytes(total_size));
        println!();
        println!("🏷️  ATTRIBUTES");
        println!(
            "  Hidden:               {}",
            format_number_commas(hidden_count)
        );
        println!(
            "  System:               {}",
            format_number_commas(system_count)
        );
        println!(
            "  Compressed:           {}",
            format_number_commas(compressed_count)
        );
        println!(
            "  Encrypted:            {}",
            format_number_commas(encrypted_count)
        );
        println!(
            "  Sparse:               {}",
            format_number_commas(sparse_count)
        );

        println!();
        let elapsed = start_time.elapsed();
        println!("⏱️  Completed in {}", format_duration(elapsed));
        return Ok(());
    }

    // Build index and show tree metrics (for debugging)
    if build_index {
        println!();
        println!("🔨 BUILDING MFTINDEX...");

        let build_start = Instant::now();
        let index = MftReader::load_raw_to_index_with_options(input, &data_load_options)
            .with_context(|| format!("Failed to build index from {}", input.display()))?;
        let build_time = build_start.elapsed();

        println!();
        println!("✅ INDEX BUILT");
        println!(
            "  Records:              {}",
            format_number_commas(index.len() as u64)
        );
        println!("  Build time:          {}", format_duration(build_time));

        // Show sample tree metrics
        println!();
        println!("📊 TREE METRICS SAMPLE (first 10 directories):");
        println!();
        println!(
            "  {:<8} {:<12} {:<15} {:<15}",
            "FRS", "Descendants", "TreeSize", "TreeAllocated"
        );
        println!("  {}", "─".repeat(60));

        let mut shown = 0_i32;
        for record in &index.records {
            if record.is_directory() && shown < 10_i32 {
                println!(
                    "  {:<8} {:<12} {:<15} {:<15}",
                    record.frs,
                    record.descendants,
                    format_bytes(record.treesize),
                    format_bytes(record.tree_allocated)
                );
                shown += 1_i32;
            }
        }

        // Show root directory specifically
        if let Some(root) = index.records.iter().find(|r| r.frs == 5) {
            println!();
            println!("📁 ROOT DIRECTORY (FRS 5):");
            println!(
                "  Descendants:          {}",
                format_number_commas(root.descendants.into())
            );
            println!("  Tree size:           {}", format_bytes(root.treesize));
            println!(
                "  Tree allocated:      {}",
                format_bytes(root.tree_allocated)
            );
        }

        let elapsed = start_time.elapsed();
        println!();
        println!("⏱️  Completed in {}", format_duration(elapsed));
        return Ok(());
    }

    // Debug tree metrics computation (detailed hardlink handling)
    if debug_tree {
        use uffs_mft::MftIndex;
        use uffs_mft::parse::{
            ParseOptions, ParseResult, apply_fixup, parse_record, parse_record_forensic,
        };

        println!();
        println!("═══════════════════════════════════════════════════════════════");
        println!("                    DEBUG TREE METRICS");
        println!("═══════════════════════════════════════════════════════════════");
        println!();

        // Load raw MFT data
        let raw = load_raw_mft(input, &data_load_options)
            .with_context(|| format!("Failed to load raw MFT from {}", input.display()))?;
        println!("Raw MFT loaded: {} records", raw.header.record_count);

        // Parse all records
        let capacity = usize::try_from(raw.header.record_count).unwrap_or(0);
        let mut parsed_records = Vec::with_capacity(capacity);

        let parse_options = if forensic {
            ParseOptions::FORENSIC
        } else {
            ParseOptions::DEFAULT
        };

        let mut hardlink_count = 0_usize;
        let mut max_name_count = 0_u16;

        for (frs, record_data) in raw.iter_records() {
            let mut record_buf = record_data.to_vec();
            let fixup_ok = apply_fixup(&mut record_buf);

            if forensic {
                let result = parse_record_forensic(&record_buf, frs, &parse_options, !fixup_ok);
                if let ParseResult::Base(parsed) = result {
                    if parsed.names.len() > 1 {
                        hardlink_count += 1;
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            max_name_count = max_name_count.max(parsed.names.len() as u16);
                        }
                    }
                    parsed_records.push(parsed);
                }
            } else {
                if !fixup_ok {
                    continue;
                }
                if let Some(parsed) = parse_record(&record_buf, frs) {
                    if parsed.names.len() > 1 {
                        hardlink_count += 1;
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            max_name_count = max_name_count.max(parsed.names.len() as u16);
                        }
                    }
                    parsed_records.push(parsed);
                }
            }
        }

        println!("Parsed {} records", parsed_records.len());
        println!("Records with multiple names (hardlinks): {hardlink_count}");
        println!("Max name_count: {max_name_count}");

        // Show sample hardlinks
        println!();
        println!("=== SAMPLE HARDLINKS (first 10) ===");
        let mut shown = 0_u32;
        for parsed in &parsed_records {
            if parsed.names.len() > 1 && shown < 10_u32 {
                println!(
                    "  FRS {}: name_count={}, size={}",
                    parsed.frs,
                    parsed.names.len(),
                    parsed.size
                );
                for (idx, name) in parsed.names.iter().enumerate() {
                    println!(
                        "    [{idx}] parent_frs={}, name={}",
                        name.parent_frs, name.name
                    );
                }
                shown += 1_u32;
            }
        }

        // Build MftIndex (this computes tree metrics normally)
        println!();
        println!("Building MftIndex...");
        let mut index = MftIndex::from_parsed_records(header.volume_letter, parsed_records);

        println!(
            "Index built: {} records, {} children entries",
            index.len(),
            index.children_count()
        );

        // Now recompute tree metrics with debug output
        // (compute_tree_metrics_debug will recompute and print detailed info)
        println!();
        index.compute_tree_metrics_debug();

        let elapsed = start_time.elapsed();
        println!();
        println!("⏱️  Completed in {}", format_duration(elapsed));
        return Ok(());
    }

    // Parse and export (output is guaranteed to be Some by upfront validation)
    let output = output_path.expect("output validated at function start");

    // Determine output format from extension
    let ext = output
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("parquet");

    let format_name = if ext == "csv" { "CSV" } else { "Parquet" };

    println!();
    println!("📤 EXPORTING TO {format_name}...");
    println!("  Building MftIndex with tree metrics...");

    // Build MftIndex (includes tree metrics computation)
    let build_start = Instant::now();
    let index = MftReader::load_raw_to_index_with_options(input, &data_load_options)
        .with_context(|| format!("Failed to build index from {}", input.display()))?;
    let build_time = build_start.elapsed();

    println!(
        "  ✅ Index built in {} ({} records)",
        format_duration(build_time),
        format_number_commas(index.len() as u64)
    );

    // Convert MftIndex to DataFrame (includes tree metrics + path!)
    println!("  Converting to DataFrame with paths...");
    let df_start = Instant::now();
    let mut df = index
        .to_dataframe()
        .with_context(|| "Failed to convert index to DataFrame")?;
    let df_time = df_start.elapsed();

    println!(
        "  ✅ DataFrame created in {} ({} columns)",
        format_duration(df_time),
        df.width()
    );

    let parsed_count = df.height();

    // Export to file
    println!("  Writing {format_name} file...");
    let export_start = Instant::now();
    match ext {
        "csv" => {
            use std::fs::File;

            use uffs_polars::{CsvWriter, SerWriter};

            let file = File::create(output)?;
            CsvWriter::new(file).finish(&mut df)?;
        }
        _ => {
            MftReader::save_parquet(&mut df, output)?;
        }
    }
    let export_time = export_start.elapsed();

    println!("  ✅ Export completed in {}", format_duration(export_time));

    // Get absolute path and file size after creation
    let output_abs = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let output_abs = clean_path_for_display(&output_abs);
    let output_size = std::fs::metadata(output).map_or(0, |meta| meta.len());

    println!();
    println!("📁 OUTPUT FILE");
    println!("  Path:                 {}", output_abs.display());
    println!("  Format:               {format_name}");
    println!("  File size:           {}", format_bytes(output_size));
    println!(
        "  Records exported:     {}",
        format_number_commas(parsed_count as u64)
    );
    println!("  Columns:              {} columns including:", df.width());
    println!("                        - Core: frs, parent_frs, name, size, allocated_size");
    println!("                        - Timestamps: si_created, si_modified, fn_created, etc.");
    println!("                        - Flags: is_directory, is_readonly, is_hidden, etc.");
    if forensic {
        println!(
            "                        - Forensic: is_deleted, is_corrupt, is_extension, base_frs"
        );
    }
    println!("                        - Path: full resolved path (e.g., C:\\Users\\file.txt)");

    let elapsed = start_time.elapsed();
    println!();
    println!("⏱️  Completed in {}", format_duration(elapsed));

    Ok(())
}

// ============================================================================
// Raw MFT Benchmark Command (matches C++ --benchmark-mft exactly)
// ============================================================================

/// Raw MFT read benchmark matching C++ `--benchmark-mft` output exactly.
///
/// This measures pure disk I/O throughput by reading the entire MFT with
/// synchronous 1MB reads. It does NOT parse records or build `DataFrame`s.
#[cfg(windows)]
#[allow(unsafe_code)] // Required: Windows FFI (ReadFile, SetFilePointerEx)
async fn cmd_benchmark_mft(drive: char) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::io::AlignedBuffer;
    use uffs_mft::platform::VolumeHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

    let drive_upper = drive.to_ascii_uppercase();

    // =========================================================================
    // Open volume and get metadata
    // =========================================================================
    let handle = VolumeHandle::open(drive_upper)
        .with_context(|| format!("Failed to open volume {}:", drive_upper))?;

    let vol_data = handle.volume_data();

    // Get MFT extents
    let extents = handle
        .get_mft_extents()
        .with_context(|| format!("Failed to get MFT extents for {}:", drive_upper))?;

    // Calculate MFT metrics
    let mft_size = vol_data.mft_valid_data_length;
    let record_size = vol_data.bytes_per_file_record_segment;
    let record_count = mft_size / u64::from(record_size);
    let mft_size_mb = mft_size / (1024 * 1024);

    // =========================================================================
    // Print Volume Information (matches C++ format exactly)
    // =========================================================================
    println!("=== MFT Read Benchmark Tool ===");
    println!("Drive: {}:", drive_upper);
    println!();
    println!("Volume Information:");
    println!("  BytesPerSector: {}", vol_data.bytes_per_sector);
    println!("  BytesPerCluster: {}", vol_data.bytes_per_cluster);
    println!(
        "  BytesPerFileRecordSegment: {}",
        vol_data.bytes_per_file_record_segment
    );
    println!("  MftValidDataLength: {}", vol_data.mft_valid_data_length);
    println!("  MftStartLcn: {}", vol_data.mft_start_lcn);
    println!();

    // =========================================================================
    // Print MFT Information (matches C++ format exactly)
    // =========================================================================
    println!("MFT Information:");
    println!("  Extents: {}", extents.len());
    println!("  MFT Size: {} bytes ({} MB)", mft_size, mft_size_mb);
    println!("  Record Size: {} bytes", record_size);
    println!("  Record Count: {}", record_count);
    println!("  Total Bytes to Read: {}", mft_size);
    println!();
    println!("Starting MFT read benchmark...");
    println!();

    // =========================================================================
    // Benchmark: Read MFT with 1MB synchronous reads
    // =========================================================================
    const BUFFER_SIZE: usize = 1024 * 1024; // 1 MB buffer (matches C++)
    let sector_size = vol_data.bytes_per_sector as usize;
    let bytes_per_cluster = vol_data.bytes_per_cluster;

    // Allocate sector-aligned buffer (AlignedBuffer uses SECTOR_SIZE internally)
    let mut buffer = AlignedBuffer::new(BUFFER_SIZE);

    // Storage for first and last 4 bytes (proof of complete read)
    let mut first_4_bytes: [u8; 4] = [0; 4];
    let mut last_4_bytes: [u8; 4] = [0; 4];
    let mut captured_first = false;

    let raw_handle: HANDLE = handle.raw_handle();
    let mut total_bytes_read: u64 = 0;

    // Start timing (only the read operations, not setup)
    let start_time = Instant::now();

    // Read each extent
    for extent in &extents {
        // Skip sparse extents
        if extent.lcn < 0 {
            continue;
        }

        // Calculate byte offset and size for this extent
        let extent_byte_offset = (extent.lcn as u64) * u64::from(bytes_per_cluster);
        let extent_byte_size = extent.cluster_count * u64::from(bytes_per_cluster);

        // Don't read beyond MFT valid data length
        let bytes_remaining = mft_size.saturating_sub(total_bytes_read);
        let extent_bytes_to_read = extent_byte_size.min(bytes_remaining);

        if extent_bytes_to_read == 0 {
            break;
        }

        // Seek to extent start
        let seek_result =
            unsafe { SetFilePointerEx(raw_handle, extent_byte_offset as i64, None, FILE_BEGIN) };
        if seek_result.is_err() {
            anyhow::bail!(
                "Failed to seek to offset {} for extent at LCN {}",
                extent_byte_offset,
                extent.lcn
            );
        }

        // Read extent in 1MB chunks
        let mut extent_offset: u64 = 0;
        while extent_offset < extent_bytes_to_read {
            let chunk_size = ((extent_bytes_to_read - extent_offset) as usize).min(BUFFER_SIZE);
            // Round up to sector boundary for FILE_FLAG_NO_BUFFERING
            let aligned_chunk_size = ((chunk_size + sector_size - 1) / sector_size) * sector_size;

            let buf_slice = buffer.as_mut_slice();
            let mut bytes_read: u32 = 0;

            let read_result = unsafe {
                ReadFile(
                    raw_handle,
                    Some(&mut buf_slice[..aligned_chunk_size]),
                    Some(&mut bytes_read),
                    None,
                )
            };

            if read_result.is_err() {
                anyhow::bail!(
                    "Failed to read from volume at offset {}",
                    extent_byte_offset + extent_offset
                );
            }

            if bytes_read == 0 {
                break; // EOF
            }

            // Capture first 4 bytes
            if !captured_first && bytes_read >= 4 {
                first_4_bytes.copy_from_slice(&buf_slice[0..4]);
                captured_first = true;
            }

            // Update last 4 bytes (always keep the most recent)
            let actual_bytes = (bytes_read as usize).min(chunk_size);
            if actual_bytes >= 4 {
                last_4_bytes.copy_from_slice(&buf_slice[actual_bytes - 4..actual_bytes]);
            }

            total_bytes_read += actual_bytes as u64;
            extent_offset += bytes_read as u64;

            // Stop if we've read enough
            if total_bytes_read >= mft_size {
                break;
            }
        }

        if total_bytes_read >= mft_size {
            break;
        }
    }

    // Stop timing
    let elapsed = start_time.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let elapsed_secs = elapsed.as_secs_f64();

    // Calculate throughput
    let read_speed_mb_s = if elapsed_secs > 0.0 {
        (total_bytes_read as f64 / (1024.0 * 1024.0)) / elapsed_secs
    } else {
        0.0
    };

    let total_mb = total_bytes_read / (1024 * 1024);

    // =========================================================================
    // Print Benchmark Results (matches C++ format exactly)
    // =========================================================================
    println!("=== Benchmark Results ===");
    println!("Total bytes read: {} ({} MB)", total_bytes_read, total_mb);
    println!("Total records: {}", record_count);
    println!(
        "Time elapsed: {} ms ({:.3} seconds)",
        elapsed_ms, elapsed_secs
    );
    println!("Read speed: {:.2} MB/s", read_speed_mb_s);
    println!();

    // =========================================================================
    // Print Proof of Complete Read (matches C++ format exactly)
    // =========================================================================
    println!("=== Proof of Complete Read ===");

    // Format first 4 bytes
    let first_hex = format!(
        "{:02X} {:02X} {:02X} {:02X}",
        first_4_bytes[0], first_4_bytes[1], first_4_bytes[2], first_4_bytes[3]
    );
    let first_ascii = format!(
        "{}{}{}{}",
        char_or_dot(first_4_bytes[0]),
        char_or_dot(first_4_bytes[1]),
        char_or_dot(first_4_bytes[2]),
        char_or_dot(first_4_bytes[3])
    );
    println!(
        "First 4 bytes (hex): {}  (ASCII: {})",
        first_hex, first_ascii
    );

    // Format last 4 bytes
    let last_hex = format!(
        "{:02X} {:02X} {:02X} {:02X}",
        last_4_bytes[0], last_4_bytes[1], last_4_bytes[2], last_4_bytes[3]
    );
    let last_ascii = format!(
        "{}{}{}{}",
        char_or_dot(last_4_bytes[0]),
        char_or_dot(last_4_bytes[1]),
        char_or_dot(last_4_bytes[2]),
        char_or_dot(last_4_bytes[3])
    );
    println!("Last 4 bytes (hex):  {}  (ASCII: {})", last_hex, last_ascii);
    println!();
    println!("Note: First 4 bytes should be 'FILE' (46 49 4C 45) - the MFT record signature.");

    Ok(())
}

/// Converts a byte to a printable ASCII character or '.' for non-printable.
#[cfg(windows)]
fn char_or_dot(byte: u8) -> char {
    if byte.is_ascii_graphic() || byte == b' ' {
        byte as char
    } else {
        '.'
    }
}

// ============================================================================
// Full Index Build Benchmark Command (matches C++ --benchmark-index exactly)
// ============================================================================

/// Full index build benchmark matching C++ `--benchmark-index` output exactly.
///
/// This measures the complete UFFS indexing pipeline: async I/O + parsing +
/// `DataFrame` building. This is what users experience when indexing.
#[cfg(windows)]
async fn cmd_benchmark_index(drive: char) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::VolumeHandle;
    use uffs_mft::{MftReadMode, MftReader};

    let drive_upper = drive.to_ascii_uppercase();

    println!("=== Index Build Benchmark Tool ===");
    println!("Drive: {}:", drive_upper);
    println!(
        "This measures the full UFFS indexing pipeline (async I/O + parsing + DataFrame building)"
    );
    println!();

    // Get volume info via VolumeHandle
    let handle = VolumeHandle::open(drive_upper)
        .with_context(|| format!("Failed to open volume {}:", drive_upper))?;
    let vol_data = handle.volume_data();
    let mft_size = vol_data.mft_valid_data_length;
    let record_size = vol_data.bytes_per_file_record_segment;
    let mft_capacity = mft_size / u64::from(record_size);
    let mft_size_mb = mft_size / (1024 * 1024);
    drop(handle); // Release handle before opening reader

    // =========================================================================
    // Print Volume Information (matches C++ format exactly)
    // =========================================================================
    println!("=== Volume Information ===");
    println!("MFT Capacity: {} records", mft_capacity);
    println!("MFT Record Size: {} bytes", record_size);
    println!("MFT Total Size: {} bytes ({} MB)", mft_size, mft_size_mb);
    println!();

    println!("Creating index for {}:\\ ...", drive_upper);
    println!("Indexing in progress...");
    println!();

    // =========================================================================
    // Run the full indexing pipeline with timing
    // =========================================================================
    let start_time = Instant::now();

    // Open reader and read MFT
    let reader = MftReader::open(drive_upper)
        .await
        .with_context(|| format!("Failed to open drive {}:", drive_upper))?
        .with_mode(MftReadMode::Auto);

    let df = reader
        .read_all()
        .await
        .with_context(|| format!("Failed to read MFT from {}:", drive_upper))?;

    let elapsed = start_time.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let elapsed_secs = elapsed.as_secs_f64();

    // =========================================================================
    // Calculate statistics from DataFrame
    // =========================================================================
    let total_entries = df.height() as u64;

    // Count files vs directories using the is_directory column
    let is_dir_col = df.column("is_directory").ok().and_then(|c| c.bool().ok());

    let (files_count, dirs_count) = if let Some(col) = is_dir_col {
        let dirs: u64 = col.into_iter().filter(|v| v.unwrap_or(false)).count() as u64;
        let files = total_entries.saturating_sub(dirs);
        (files, dirs)
    } else {
        // Fallback: assume all are files
        (total_entries, 0)
    };

    // =========================================================================
    // Print Index Statistics (matches C++ format exactly)
    // =========================================================================
    println!("=== Index Statistics ===");
    println!("Records Processed: {}", mft_capacity);
    println!("Files: {}", files_count);
    println!("Directories: {}", dirs_count);
    println!("Total Entries: {}", total_entries);
    println!();

    // =========================================================================
    // Print Benchmark Results (matches C++ format exactly)
    // =========================================================================
    let mft_read_speed = if elapsed_secs > 0.0 {
        (mft_size as f64 / (1024.0 * 1024.0)) / elapsed_secs
    } else {
        0.0
    };

    let records_per_sec = if elapsed_secs > 0.0 {
        (mft_capacity as f64 / elapsed_secs) as u64
    } else {
        0
    };

    let entries_per_sec = if elapsed_secs > 0.0 {
        (total_entries as f64 / elapsed_secs) as u64
    } else {
        0
    };

    println!("=== Benchmark Results ===");
    println!(
        "Time Elapsed: {} ms ({:.3} seconds)",
        elapsed_ms, elapsed_secs
    );
    println!("MFT Read Speed: {:.2} MB/s", mft_read_speed);
    println!("Record Processing: {} records/sec", records_per_sec);
    println!("File Indexing: {} files+dirs/sec", entries_per_sec);
    println!();

    // =========================================================================
    // Print Summary (matches C++ format exactly)
    // =========================================================================
    println!("=== Summary ===");
    println!(
        "Indexed {} items in {:.3} seconds",
        total_entries, elapsed_secs
    );

    Ok(())
}

// ============================================================================
// Lean Index Build Benchmark Command (no DataFrame overhead)
// ============================================================================

/// Lean index build benchmark - uses `MftIndex` instead of DataFrame.
///
/// This measures the UFFS indexing pipeline without DataFrame building
/// overhead. Should be ~2x faster than `benchmark-index` on large drives.
#[cfg(windows)]
async fn cmd_benchmark_index_lean(
    drive: char,
    mode_str: &str,
    no_bitmap: bool,
    no_placeholders: bool,
    concurrency: Option<usize>,
    io_size_kb: Option<usize>,
    parallel_parse: bool,
    parse_workers: Option<usize>,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::VolumeHandle;
    use uffs_mft::{MftReadMode, MftReader};

    let drive_upper = drive.to_ascii_uppercase();

    // Parse read mode
    let mode: MftReadMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;

    // Get drive type for adaptive defaults display
    let drive_type = uffs_mft::platform::detect_drive_type(drive_upper);
    let effective_io_size_kb = io_size_kb.unwrap_or_else(|| drive_type.optimal_io_size() / 1024);

    println!("=== Lean Index Build Benchmark Tool ===");
    println!("Drive: {}:", drive_upper);
    println!("Drive Type: {:?}", drive_type);
    println!("Mode: {}", mode);
    println!("Bitmap: {}", if no_bitmap { "disabled" } else { "enabled" });
    println!(
        "Placeholders: {}",
        if no_placeholders {
            "disabled"
        } else {
            "enabled"
        }
    );
    // For HDD, concurrency is determined by extent count (fragmentation-aware)
    // so we can't show the exact value until after opening the volume
    if let Some(c) = concurrency {
        println!("Concurrency: {} I/O ops in flight", c);
    } else if matches!(drive_type, uffs_mft::platform::DriveType::Hdd) {
        println!("Concurrency: auto (extent-aware, determined after MFT scan)");
    } else {
        println!(
            "Concurrency: {} I/O ops in flight (auto)",
            drive_type.optimal_concurrency()
        );
    }
    println!(
        "I/O Size: {} KB ({} MB){}",
        effective_io_size_kb,
        effective_io_size_kb / 1024,
        if io_size_kb.is_none() { " (auto)" } else { "" }
    );
    // Determine effective parallel parse setting (auto-enabled for NVMe if not
    // explicitly set)
    let effective_parallel_parse = parallel_parse || drive_type.benefits_from_parallel_parsing();
    if effective_parallel_parse {
        println!(
            "Parallel Parse: {} (workers: {})",
            if parallel_parse {
                "enabled"
            } else {
                "enabled (auto)"
            },
            parse_workers.map_or_else(|| "auto".to_string(), |w| w.to_string())
        );
    } else {
        println!("Parallel Parse: disabled");
    }
    println!("This measures the UFFS indexing pipeline with lean MftIndex (no DataFrame overhead)");
    println!();

    // Get volume info via VolumeHandle
    let handle = VolumeHandle::open(drive_upper)
        .with_context(|| format!("Failed to open volume {}:", drive_upper))?;
    let vol_data = handle.volume_data();
    let mft_size = vol_data.mft_valid_data_length;
    let record_size = vol_data.bytes_per_file_record_segment;
    let mft_capacity = mft_size / u64::from(record_size);
    let mft_size_mb = mft_size / (1024 * 1024);
    drop(handle); // Release handle before opening reader

    // =========================================================================
    // Print Volume Information
    // =========================================================================
    println!("=== Volume Information ===");
    println!("MFT Capacity: {} records", mft_capacity);
    println!("MFT Record Size: {} bytes", record_size);
    println!("MFT Total Size: {} bytes ({} MB)", mft_size, mft_size_mb);
    println!();

    println!("Creating lean index for {}:\\ ...", drive_upper);
    println!("Indexing in progress...");
    println!();

    // =========================================================================
    // Run the lean indexing pipeline with timing
    // =========================================================================
    let start_time = Instant::now();

    // Open reader and read MFT into lean index
    // - no_bitmap: disable bitmap optimization to read entire MFT sequentially
    // - no_placeholders: skip placeholder creation for ~15% speedup
    // - concurrency: number of I/O ops in flight (None = auto based on drive type)
    // - io_size_kb: I/O chunk size in KB (None = auto based on drive type)
    // - parallel_parse: enable M3 parallel parsing optimization
    // - parse_workers: number of parsing worker threads
    let mut reader = MftReader::open(drive_upper)
        .await
        .with_context(|| format!("Failed to open drive {}:", drive_upper))?
        .with_mode(mode)
        .with_use_bitmap(!no_bitmap)
        .with_add_placeholders(!no_placeholders);

    // Only set concurrency/io_size if explicitly specified (otherwise use adaptive
    // defaults)
    if let Some(c) = concurrency {
        reader = reader.with_concurrency(c);
    }
    if let Some(io_kb) = io_size_kb {
        reader = reader.with_io_size(io_kb * 1024);
    }

    // Apply parallel parsing settings if specified
    if parallel_parse {
        reader = reader.with_parallel_parse(true);
    }
    if let Some(workers) = parse_workers {
        reader = reader.with_parse_workers(Some(workers));
    }

    let index = reader
        .read_all_index()
        .await
        .with_context(|| format!("Failed to read MFT from {}:", drive_upper))?;

    let elapsed = start_time.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let elapsed_secs = elapsed.as_secs_f64();

    // =========================================================================
    // Calculate statistics from MftIndex
    // =========================================================================
    let total_entries = index.records.len() as u64;

    // Count files vs directories
    let dirs_count = index.records.iter().filter(|r| r.is_directory()).count() as u64;
    let files_count = total_entries.saturating_sub(dirs_count);

    // =========================================================================
    // Print Index Statistics
    // =========================================================================
    println!("=== Index Statistics ===");
    println!("Records Processed: {}", mft_capacity);
    println!("Files: {}", files_count);
    println!("Directories: {}", dirs_count);
    println!("Total Entries: {}", total_entries);
    println!("Names Buffer: {} KB", index.names.len() / 1024);
    println!();

    // =========================================================================
    // Print Benchmark Results
    // =========================================================================
    let mft_read_speed = if elapsed_secs > 0.0 {
        (mft_size as f64 / (1024.0 * 1024.0)) / elapsed_secs
    } else {
        0.0
    };

    let records_per_sec = if elapsed_secs > 0.0 {
        (mft_capacity as f64 / elapsed_secs) as u64
    } else {
        0
    };

    let entries_per_sec = if elapsed_secs > 0.0 {
        (total_entries as f64 / elapsed_secs) as u64
    } else {
        0
    };

    println!("=== Benchmark Results ===");
    println!(
        "Time Elapsed: {} ms ({:.3} seconds)",
        elapsed_ms, elapsed_secs
    );
    println!("MFT Read Speed: {:.2} MB/s", mft_read_speed);
    println!("Record Processing: {} records/sec", records_per_sec);
    println!("File Indexing: {} files+dirs/sec", entries_per_sec);
    println!();

    // =========================================================================
    // Print Summary
    // =========================================================================
    println!("=== Summary ===");
    println!(
        "Indexed {} items in {:.3} seconds (lean index, mode: {})",
        total_entries, elapsed_secs, mode
    );

    Ok(())
}

/// Benchmark multi-volume indexing using single IOCP (M4 optimization).
#[cfg(windows)]
async fn cmd_benchmark_multi_volume(drives: Vec<char>) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::io::{MultiVolumeIocpReader, prepare_volume_state};
    use uffs_mft::platform::{MftExtent, VolumeHandle, detect_drive_type};

    if drives.is_empty() {
        anyhow::bail!("No drives specified. Use --drives C,D,S");
    }

    let drives: Vec<char> = drives.iter().map(|c| c.to_ascii_uppercase()).collect();

    println!("=== Multi-Volume IOCP Benchmark (M4 Optimization) ===");
    println!("Drives: {:?}", drives);
    println!();

    // Prepare volume states
    let mut volume_states = Vec::new();
    let start_time = Instant::now();

    for &drive in &drives {
        println!("📂 Preparing volume {}:...", drive);

        // Open volume handle
        let handle = match VolumeHandle::open(drive) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("  ❌ Failed to open {}: {}", drive, e);
                continue;
            }
        };

        let drive_type = detect_drive_type(drive);
        let record_size = handle.file_record_size();
        let volume_data = handle.volume_data();

        // Get MFT extents
        let extents = handle.get_mft_extents().unwrap_or_else(|e| {
            warn!(error = ?e, "Failed to get MFT extents, using fallback");
            vec![MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        // Create extent map
        let extent_map =
            uffs_mft::io::MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);

        // Get bitmap
        let bitmap = handle.get_mft_bitmap().ok();

        // Open overlapped handle for IOCP
        let overlapped_handle = match handle.open_overlapped_handle() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("  ❌ Failed to open overlapped handle for {}: {}", drive, e);
                continue;
            }
        };

        let total_records = extent_map.total_records();
        let mft_size = total_records * u64::from(record_size);

        println!(
            "  ✅ {}: {:?}, {} records, {:.1} MB MFT",
            drive,
            drive_type,
            total_records,
            mft_size as f64 / (1024.0 * 1024.0)
        );

        let state = prepare_volume_state(drive, overlapped_handle, extent_map, bitmap, drive_type);
        volume_states.push((state, overlapped_handle));
    }

    if volume_states.is_empty() {
        anyhow::bail!("No volumes could be opened");
    }

    println!();
    println!("🚀 Starting multi-volume IOCP read...");

    // Extract handles for cleanup and states for the reader
    let handles: Vec<_> = volume_states.iter().map(|(_, h)| *h).collect();
    let states: Vec<_> = volume_states.into_iter().map(|(s, _)| s).collect();

    let read_start = Instant::now();
    let mut reader = MultiVolumeIocpReader::new(states);
    let indices = reader.read_all_volumes()?;
    let read_elapsed = read_start.elapsed();

    // Close overlapped handles
    for handle in handles {
        #[allow(unsafe_code)]
        unsafe {
            windows::Win32::Foundation::CloseHandle(handle).ok();
        }
    }

    let total_elapsed = start_time.elapsed();

    // Print results
    println!();
    println!("=== Results ===");

    let mut total_records = 0u64;
    let mut total_files = 0u64;
    let mut total_dirs = 0u64;

    for (_idx, index) in indices.iter().enumerate() {
        let files = index.records.iter().filter(|r| !r.is_directory()).count();
        let dirs = index.records.iter().filter(|r| r.is_directory()).count();
        total_records += index.len() as u64;
        total_files += files as u64;
        total_dirs += dirs as u64;

        println!(
            "  {}: {} records ({} files, {} dirs)",
            index.volume,
            index.len(),
            files,
            dirs
        );
    }

    println!();
    println!("=== Timing ===");
    println!("Read time: {:.3}s", read_elapsed.as_secs_f64());
    println!("Total time: {:.3}s", total_elapsed.as_secs_f64());
    println!();
    println!("=== Summary ===");
    println!(
        "Indexed {} records ({} files, {} dirs) from {} volumes in {:.3}s",
        total_records,
        total_files,
        total_dirs,
        indices.len(),
        read_elapsed.as_secs_f64()
    );

    Ok(())
}

// ============================================================================
// M5: USN Journal Commands
// ============================================================================

/// Query USN Journal information for a drive.
#[cfg(windows)]
async fn cmd_usn_info(drive: char) -> Result<()> {
    use uffs_mft::usn::query_usn_journal;

    println!("🔍 Querying USN Journal for {}:...", drive);
    println!();

    match query_usn_journal(drive) {
        Ok(info) => {
            println!("=== USN Journal Info ===");
            println!("  Journal ID:       0x{:016X}", info.journal_id);
            println!("  First USN:        {}", info.first_usn);
            println!("  Next USN:         {}", info.next_usn);
            println!("  Lowest Valid USN: {}", info.lowest_valid_usn);
            println!("  Max USN:          {}", info.max_usn);
            println!(
                "  Max Size:         {:.1} MB",
                info.max_size as f64 / (1024.0 * 1024.0)
            );
            println!(
                "  Alloc Delta:      {:.1} MB",
                info.allocation_delta as f64 / (1024.0 * 1024.0)
            );
            println!();
            println!(
                "📊 Journal contains ~{} changes",
                (info.next_usn - info.first_usn) / 64
            ); // Rough estimate
        }
        Err(e) => {
            eprintln!("❌ Failed to query USN Journal: {}", e);
            eprintln!();
            eprintln!("Note: USN Journal may not be enabled on this volume.");
            eprintln!(
                "Run as Administrator to enable: fsutil usn createjournal m=1000 a=100 {}:",
                drive
            );
        }
    }

    Ok(())
}

/// Read recent USN Journal changes for a drive.
#[cfg(windows)]
async fn cmd_usn_read(drive: char, start_usn: Option<i64>, limit: usize) -> Result<()> {
    use uffs_mft::usn::{query_usn_journal, read_usn_journal};

    println!("🔍 Reading USN Journal for {}:...", drive);
    println!();

    // First query the journal to get the ID
    let info = match query_usn_journal(drive) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("❌ Failed to query USN Journal: {}", e);
            return Ok(());
        }
    };

    let start = start_usn.unwrap_or(info.first_usn);
    println!(
        "Reading from USN {} (journal ID: 0x{:016X})",
        start, info.journal_id
    );
    println!();

    match read_usn_journal(drive, info.journal_id, start) {
        Ok((records, next_usn)) => {
            println!(
                "=== USN Records ({} found, showing up to {}) ===",
                records.len(),
                limit
            );
            println!();
            println!(
                "{:<12} {:<12} {:<10} {:<40}",
                "FRS", "Parent", "Reason", "Filename"
            );
            println!("{}", "-".repeat(80));

            for record in records.iter().take(limit) {
                let reason_str = format_usn_reason(record.reason);
                println!(
                    "{:<12} {:<12} {:<10} {}",
                    record.frs, record.parent_frs, reason_str, record.filename
                );
            }

            if records.len() > limit {
                println!();
                println!("... and {} more records", records.len() - limit);
            }

            println!();
            println!("Next USN: {}", next_usn);
        }
        Err(e) => {
            eprintln!("❌ Failed to read USN Journal: {}", e);
        }
    }

    Ok(())
}

/// Format USN reason flags as a short string.
#[cfg(windows)]
fn format_usn_reason(reason: u32) -> String {
    use uffs_mft::usn::reason;

    let mut parts = Vec::new();
    if reason & reason::FILE_CREATE != 0 {
        parts.push("CREATE");
    }
    if reason & reason::FILE_DELETE != 0 {
        parts.push("DELETE");
    }
    if reason & reason::RENAME_NEW_NAME != 0 {
        parts.push("RENAME");
    }
    if reason & reason::DATA_EXTEND != 0 || reason & reason::DATA_TRUNCATION != 0 {
        parts.push("SIZE");
    }
    if reason & reason::BASIC_INFO_CHANGE != 0 {
        parts.push("META");
    }
    if reason & reason::CLOSE != 0 {
        parts.push("CLOSE");
    }

    if parts.is_empty() {
        format!("0x{:08X}", reason)
    } else {
        parts.join("+")
    }
}

/// Save index to disk for incremental updates.
#[cfg(windows)]
async fn cmd_index_save(drive: char, output: &Path) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::usn::query_usn_journal;
    use uffs_mft::{MftReader, VolumeHandle};

    println!("📦 Building and saving index for {}:...", drive);
    println!();

    let start = Instant::now();

    // Build the index
    let reader = MftReader::open(drive).await?;
    let index = reader.read_all_index().await?;

    let build_time = start.elapsed();
    println!(
        "✅ Built index: {} records in {:.3}s",
        index.len(),
        build_time.as_secs_f64()
    );

    // Get volume serial and USN info
    let handle = VolumeHandle::open(drive)?;
    let volume_data = handle.volume_data();
    let volume_serial = volume_data.volume_serial_number;

    let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
        Ok(info) => (info.journal_id, info.next_usn),
        Err(_) => {
            println!("⚠️  USN Journal not available, saving without checkpoint");
            (0, 0)
        }
    };

    // Save to file
    let save_start = Instant::now();
    index.save_to_file(output, volume_serial, usn_journal_id, next_usn)?;
    let save_time = save_start.elapsed();

    let file_size = std::fs::metadata(output)?.len();
    println!(
        "✅ Saved to {}: {:.1} MB in {:.3}s",
        output.display(),
        file_size as f64 / (1024.0 * 1024.0),
        save_time.as_secs_f64()
    );

    if usn_journal_id != 0 {
        println!();
        println!(
            "📍 USN Checkpoint: {} (Journal ID: 0x{:016X})",
            next_usn, usn_journal_id
        );
        println!("   Use this to apply incremental updates later.");
    }

    Ok(())
}

/// Load index from disk and show info.
#[cfg(windows)]
async fn cmd_index_load(input: &Path) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::index::MftIndex;

    println!("📂 Loading index from {}...", input.display());
    println!();

    let start = Instant::now();
    let (index, header) = MftIndex::load_from_file(input).map_err(|e| anyhow::anyhow!("{}", e))?;
    let load_time = start.elapsed();

    let file_size = std::fs::metadata(input)?.len();

    println!("=== Index Header ===");
    println!("  Volume:           {}:", header.volume);
    println!("  Volume Serial:    0x{:016X}", header.volume_serial);
    println!("  USN Journal ID:   0x{:016X}", header.usn_journal_id);
    println!("  Next USN:         {}", header.next_usn);
    println!("  Created At:       {} (FILETIME)", header.created_at);
    println!();
    println!("=== Index Stats ===");
    println!("  Records:          {}", header.record_count);
    println!("  Names Size:       {} bytes", header.names_size);
    println!("  Links:            {}", header.links_count);
    println!("  Streams:          {}", header.streams_count);
    println!("  Children:         {}", header.children_count);
    println!();
    println!("=== Performance ===");
    println!(
        "  File Size:        {:.1} MB",
        file_size as f64 / (1024.0 * 1024.0)
    );
    println!("  Load Time:        {:.3}s", load_time.as_secs_f64());
    println!(
        "  Throughput:       {:.1} MB/s",
        (file_size as f64 / (1024.0 * 1024.0)) / load_time.as_secs_f64()
    );

    // Count files vs directories
    let files = index.records.iter().filter(|r| !r.is_directory()).count();
    let dirs = index.records.iter().filter(|r| r.is_directory()).count();
    println!();
    println!("=== Content ===");
    println!("  Files:            {}", files);
    println!("  Directories:      {}", dirs);

    Ok(())
}

/// Show cache status and optionally clean up.
#[cfg(windows)]
async fn cmd_cache_status(clean: bool, purge: bool) -> Result<()> {
    use uffs_mft::cache::{
        INDEX_TTL_SECONDS, cache_age_seconds, cache_dir, cleanup_expired_cache, list_cached_drives,
        remove_all_cached_indices,
    };

    let dir = cache_dir();
    println!("📁 Cache Directory: {}", dir.display());
    println!(
        "⏱️  TTL: {} seconds ({} minutes)",
        INDEX_TTL_SECONDS,
        INDEX_TTL_SECONDS / 60
    );
    println!();

    if purge {
        println!("🗑️  Purging ALL cached indices...");
        remove_all_cached_indices();
        println!("✅ Cache purged.");
        return Ok(());
    }

    if clean {
        println!("🧹 Cleaning expired caches...");
        cleanup_expired_cache(INDEX_TTL_SECONDS);
        println!("✅ Cleanup complete.");
        println!();
    }

    let drives = list_cached_drives();
    if drives.is_empty() {
        println!("📭 No cached indices found.");
        return Ok(());
    }

    println!("=== Cached Indices ===");
    println!("{:<8} {:<12} {:<10}", "Drive", "Age", "Status");
    println!("{}", "-".repeat(32));

    for drive in &drives {
        let age = cache_age_seconds(*drive);
        let (age_str, status) = match age {
            Some(secs) if secs < INDEX_TTL_SECONDS => {
                let remaining = INDEX_TTL_SECONDS - secs;
                (
                    format!("{}s", secs),
                    format!("✅ Fresh ({}s left)", remaining),
                )
            }
            Some(secs) => (format!("{}s", secs), "⚠️  Expired".to_string()),
            None => ("?".to_string(), "❓ Unknown".to_string()),
        };
        println!("{:<8} {:<12} {}", format!("{}:", drive), age_str, status);
    }

    Ok(())
}

/// Get or refresh a cached index for a drive.
#[cfg(windows)]
async fn cmd_cache_get(drive: char, force: bool, ttl: Option<u64>) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::cache::{CacheStatus, INDEX_TTL_SECONDS, check_cache_status, save_to_cache};
    use uffs_mft::usn::query_usn_journal;
    use uffs_mft::{MftReader, VolumeHandle};

    let ttl_seconds = ttl.unwrap_or(INDEX_TTL_SECONDS);
    println!("🔍 Checking cache for {}:...", drive);
    println!("⏱️  TTL: {} seconds", ttl_seconds);
    println!();

    // Check cache status (unless force rebuild)
    if !force {
        match check_cache_status(drive, ttl_seconds) {
            CacheStatus::Fresh {
                index,
                header,
                age_seconds,
            } => {
                println!("✅ Cache HIT! Index is fresh ({} seconds old)", age_seconds);
                println!();
                println!("=== Cached Index ===");
                println!("  Records:     {}", index.len());
                println!("  USN:         {}", header.next_usn);
                println!("  Journal ID:  0x{:016X}", header.usn_journal_id);

                let files = index.records.iter().filter(|r| !r.is_directory()).count();
                let dirs = index.records.iter().filter(|r| r.is_directory()).count();
                println!("  Files:       {}", files);
                println!("  Directories: {}", dirs);
                return Ok(());
            }
            CacheStatus::Stale { age_seconds } => {
                println!(
                    "⚠️  Cache STALE (age: {}s, TTL: {}s)",
                    age_seconds.map_or("?".to_string(), |a| a.to_string()),
                    ttl_seconds
                );
            }
            CacheStatus::Missing => {
                println!("📭 Cache MISS - no cached index found");
            }
        }
    } else {
        println!("🔄 Force rebuild requested");
    }

    println!();
    println!("🔨 Building fresh index...");

    let start = Instant::now();
    let reader = MftReader::open(drive).await?;
    let index = reader.read_all_index().await?;
    let build_time = start.elapsed();

    println!(
        "✅ Built index: {} records in {:.3}s",
        index.len(),
        build_time.as_secs_f64()
    );

    // Get volume info for caching
    let handle = VolumeHandle::open(drive)?;
    let volume_data = handle.volume_data();
    let volume_serial = volume_data.volume_serial_number;

    let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
        Ok(info) => (info.journal_id, info.next_usn),
        Err(_) => {
            println!("⚠️  USN Journal not available");
            (0, 0)
        }
    };

    // Save to cache
    let cache_path = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn)?;
    let file_size = std::fs::metadata(&cache_path)?.len();

    println!(
        "💾 Cached to: {} ({:.1} MB)",
        cache_path.display(),
        file_size as f64 / (1024.0 * 1024.0)
    );

    if usn_journal_id != 0 {
        println!(
            "📍 USN Checkpoint: {} (Journal ID: 0x{:016X})",
            next_usn, usn_journal_id
        );
    }

    Ok(())
}

/// Clear cached indices.
#[cfg(windows)]
async fn cmd_cache_clear(drive: Option<char>, all: bool) -> Result<()> {
    use uffs_mft::cache::{
        cache_dir, cache_file_path, list_cached_drives, remove_all_cached_indices,
        remove_cached_index,
    };

    if all {
        println!("🗑️  Clearing ALL cached indices...");
        let drives = list_cached_drives();
        remove_all_cached_indices();
        if drives.is_empty() {
            println!("📭 No cached indices found.");
        } else {
            println!("✅ Cleared {} cached indices: {:?}", drives.len(), drives);
        }
        println!("📁 Cache directory: {}", cache_dir().display());
    } else if let Some(d) = drive {
        let path = cache_file_path(d);
        if path.exists() {
            remove_cached_index(d);
            println!("✅ Cleared cache for {}:", d);
            println!("   {}", path.display());
        } else {
            println!("📭 No cached index found for {}:", d);
        }
    } else {
        println!("❌ Please specify --drive C or --all");
        println!();
        println!("Examples:");
        println!("  uffs_mft cache-clear --drive C");
        println!("  uffs_mft cache-clear --all");
    }

    Ok(())
}

/// Incremental index update using USN Journal.
#[cfg(windows)]
async fn cmd_index_update(drive: char, force_full: bool, ttl: Option<u64>) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::VolumeHandle;
    use uffs_mft::cache::{CacheStatus, INDEX_TTL_SECONDS, check_cache_status, save_to_cache};
    use uffs_mft::platform::is_volume_read_only;
    use uffs_mft::usn::{aggregate_changes, query_usn_journal, read_usn_journal};

    let ttl_seconds = ttl.unwrap_or(INDEX_TTL_SECONDS);
    let start = Instant::now();

    println!("🔄 Incremental index update for {}:...", drive);
    println!();

    // If force_full, skip cache and do full scan
    if force_full {
        println!("🔨 Force full scan requested...");
        return do_full_index_build(drive).await;
    }

    // Check cache status
    let cache_result = check_cache_status(drive, ttl_seconds);

    match cache_result {
        CacheStatus::Fresh {
            index,
            header,
            age_seconds,
        } => {
            println!("📦 Found cached index ({} seconds old)", age_seconds);
            println!(
                "   Records: {}, USN checkpoint: {}",
                index.len(),
                header.next_usn
            );
            println!();

            // Check if volume is read-only - if so, nothing can have changed
            if is_volume_read_only(drive) {
                println!("🔒 Volume is read-only - no changes possible");
                println!("✅ Using cached index ({} records)", index.len());
                let elapsed = start.elapsed();
                println!();
                println!("⏱️  Completed in {:.3}s", elapsed.as_secs_f64());
                return Ok(());
            }

            // Query current USN Journal
            let current_info = match query_usn_journal(drive) {
                Ok(info) => info,
                Err(e) => {
                    println!("⚠️  USN Journal not available: {}", e);
                    println!("   Falling back to full scan...");
                    return do_full_index_build(drive).await;
                }
            };

            // Check if journal ID matches (journal may have been recreated)
            if current_info.journal_id != header.usn_journal_id {
                println!(
                    "⚠️  USN Journal ID changed (was 0x{:016X}, now 0x{:016X})",
                    header.usn_journal_id, current_info.journal_id
                );
                println!("   Falling back to full scan...");
                return do_full_index_build(drive).await;
            }

            // Check if our checkpoint is still valid
            if header.next_usn < current_info.first_usn {
                println!(
                    "⚠️  USN Journal wrapped (checkpoint {} < first {})",
                    header.next_usn, current_info.first_usn
                );
                println!("   Falling back to full scan...");
                return do_full_index_build(drive).await;
            }

            // Read changes since our checkpoint
            println!("📖 Reading USN changes since {}...", header.next_usn);
            let (records, next_usn) =
                match read_usn_journal(drive, current_info.journal_id, header.next_usn) {
                    Ok(r) => r,
                    Err(e) => {
                        println!("⚠️  Failed to read USN Journal: {}", e);
                        println!("   Falling back to full scan...");
                        return do_full_index_build(drive).await;
                    }
                };

            if records.is_empty() {
                println!("✅ No changes since last update!");
                println!("   Index is up-to-date ({} records)", index.len());
                let elapsed = start.elapsed();
                println!();
                println!("⏱️  Completed in {:.3}s", elapsed.as_secs_f64());
                return Ok(());
            }

            // Aggregate changes by FRS
            let changes_map = aggregate_changes(&records);
            let changes: Vec<_> = changes_map.into_values().collect();
            println!(
                "   Found {} USN records → {} unique file changes",
                records.len(),
                changes.len()
            );

            // Apply changes to index
            println!();
            println!("🔧 Applying {} changes to index...", changes.len());

            let mut updated_index = index;
            let apply_start = Instant::now();
            let stats = updated_index.apply_usn_changes(&changes);
            let apply_time = apply_start.elapsed();

            println!(
                "   Created: {}, Deleted: {}, Modified: {}, Skipped: {}",
                stats.created, stats.deleted, stats.modified, stats.skipped
            );
            println!("   Applied in {:.3}s", apply_time.as_secs_f64());

            // Recompute tree metrics after structural changes
            println!();
            println!("🔨 Recomputing tree metrics...");
            let tree_start = Instant::now();
            updated_index.compute_tree_metrics();
            let tree_time = tree_start.elapsed();
            println!("   Computed in {:.3}s", tree_time.as_secs_f64());

            // Save updated index
            let handle = VolumeHandle::open(drive)?;
            let volume_data = handle.volume_data();
            let volume_serial = volume_data.volume_serial_number;

            let cache_path = save_to_cache(
                &updated_index,
                drive,
                volume_serial,
                current_info.journal_id,
                next_usn,
            )?;

            let elapsed = start.elapsed();
            println!();
            println!("✅ Incremental update complete!");
            println!("   Records: {}", updated_index.len());
            println!("   New USN checkpoint: {}", next_usn);
            println!("   Saved to: {}", cache_path.display());
            println!("⏱️  Total time: {:.3}s", elapsed.as_secs_f64());
        }
        CacheStatus::Stale { age_seconds } => {
            println!(
                "⚠️  Cache is stale (age: {}s, TTL: {}s)",
                age_seconds.map_or("?".to_string(), |a| a.to_string()),
                ttl_seconds
            );
            println!("   Performing full scan...");
            return do_full_index_build(drive).await;
        }
        CacheStatus::Missing => {
            println!("📭 No cached index found");
            println!("   Performing initial full scan...");
            return do_full_index_build(drive).await;
        }
    }

    Ok(())
}

/// Helper function to do a full index build and cache it.
#[cfg(windows)]
async fn do_full_index_build(drive: char) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::cache::save_to_cache;
    use uffs_mft::usn::query_usn_journal;
    use uffs_mft::{MftReader, VolumeHandle};

    let start = Instant::now();

    println!();
    println!("🔨 Building full index for {}:...", drive);

    let reader = MftReader::open(drive).await?;
    let index = reader.read_all_index().await?;
    let build_time = start.elapsed();

    println!(
        "✅ Built index: {} records in {:.3}s",
        index.len(),
        build_time.as_secs_f64()
    );

    // Get volume info
    let handle = VolumeHandle::open(drive)?;
    let volume_data = handle.volume_data();
    let volume_serial = volume_data.volume_serial_number;

    let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
        Ok(info) => (info.journal_id, info.next_usn),
        Err(_) => {
            println!("⚠️  USN Journal not available");
            (0, 0)
        }
    };

    // Save to cache
    let cache_path = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn)?;
    let file_size = std::fs::metadata(&cache_path)?.len();

    println!(
        "💾 Cached to: {} ({:.1} MB)",
        cache_path.display(),
        file_size as f64 / (1024.0 * 1024.0)
    );

    if usn_journal_id != 0 {
        println!(
            "📍 USN Checkpoint: {} (Journal ID: 0x{:016X})",
            next_usn, usn_journal_id
        );
    }

    let total_time = start.elapsed();
    println!();
    println!("⏱️  Total time: {:.3}s", total_time.as_secs_f64());

    Ok(())
}

/// Index ALL NTFS drives in parallel using the optimized lean index path.
#[cfg(windows)]
async fn cmd_index_all(drives: Option<Vec<char>>, no_cache: bool, ttl: u64) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::{MultiDriveMftReader, detect_ntfs_drives};

    let start = Instant::now();

    // Detect drives if not specified
    let drive_list: Vec<char> = match drives {
        Some(d) if !d.is_empty() => d.into_iter().map(|c| c.to_ascii_uppercase()).collect(),
        _ => {
            println!("🔍 Detecting NTFS drives...");
            detect_ntfs_drives()
        }
    };

    if drive_list.is_empty() {
        println!("❌ No NTFS drives found");
        return Ok(());
    }

    println!();
    println!("=== Index All NTFS Drives ===");
    println!(
        "Drives: {}",
        drive_list
            .iter()
            .map(|c| format!("{}:", c))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "Mode: {}",
        if no_cache {
            "fresh (no cache read)"
        } else {
            "cached"
        }
    );
    if !no_cache {
        println!("TTL: {} seconds", ttl);
    }
    println!();

    // Create multi-drive reader
    let reader = MultiDriveMftReader::new(drive_list.clone());

    // Read all indices (default: use cache)
    let indices = if no_cache {
        println!("🔨 Building fresh indices (will save to cache)...");
        reader.read_all_index_cached(0).await? // TTL=0 forces rebuild but still saves
    } else {
        println!("📦 Reading indices (with cache)...");
        reader.read_all_index_cached(ttl).await?
    };

    let read_time = start.elapsed();

    // Print summary
    println!();
    println!("=== Index Summary ===");
    println!();

    let mut total_files = 0u64;
    let mut total_dirs = 0u64;
    let mut total_entries = 0u64;

    for index in &indices {
        let files = index.file_count() as u64;
        let dirs = index.dir_count() as u64;
        total_files += files;
        total_dirs += dirs;
        total_entries += index.len() as u64;

        println!(
            "  {}:  {:>10} files  {:>8} dirs  {:>10} total",
            index.volume,
            format_number(files),
            format_number(dirs),
            format_number(index.len() as u64),
        );
    }

    println!();
    println!("─────────────────────────────────────────────────");
    println!(
        "  TOTAL: {:>10} files  {:>8} dirs  {:>10} entries",
        format_number(total_files),
        format_number(total_dirs),
        format_number(total_entries),
    );
    println!();

    // Performance stats
    let elapsed_secs = read_time.as_secs_f64();
    let entries_per_sec = total_entries as f64 / elapsed_secs;

    println!("=== Performance ===");
    println!("Time: {:.3}s", elapsed_secs);
    println!("Throughput: {:.0} entries/sec", entries_per_sec);
    println!();

    Ok(())
}

/// Format a number with thousands separators.
#[cfg(windows)]
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Index ALL NTFS drives (non-Windows stub).
#[cfg(not(windows))]
#[allow(dead_code, clippy::unused_async)]
async fn cmd_index_all(_drives: Option<Vec<char>>, _no_cache: bool, _ttl: u64) -> Result<()> {
    anyhow::bail!("index-all command is only supported on Windows")
}
