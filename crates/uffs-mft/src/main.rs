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

#[cfg(windows)]
use anyhow::Context;
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
// Dev-dependencies (used in benchmarks only)
#[cfg(test)]
use criterion as _;
// Platform-gated dependencies (used on Windows only)
#[cfg(not(windows))]
use indicatif as _;
#[cfg(windows)]
use indicatif::{ProgressBar, ProgressStyle};
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
use {bitflags as _, rayon as _, thiserror as _, uffs_polars as _};
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
#[cfg(windows)]
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
#[cfg(windows)]
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
#[cfg(windows)]
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
    },

    /// Show MFT information for a drive
    Info {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Perform deep scan (reads all MFT records for detailed statistics)
        #[arg(long)]
        deep: bool,
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
    Save {
        /// Drive letter to read MFT from (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Output file path for raw MFT data
        #[arg(short, long)]
        output: PathBuf,

        /// Disable compression (default: compressed with zstd)
        #[arg(long)]
        no_compress: bool,

        /// Compression level (1-22, default 3)
        #[arg(long, default_value = "3")]
        compression_level: i32,
    },

    /// Load MFT from a saved file and export to parquet/csv
    Load {
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
async fn main() -> Result<()> {
    // Check for -v/--verbose flag early
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

    // Parse CLI with custom error handling to show help on errors
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // For help/version requests, just print and exit normally
            if err.kind() == clap::error::ErrorKind::DisplayHelp
                || err.kind() == clap::error::ErrorKind::DisplayVersion
            {
                err.exit();
            }
            // For actual errors, print the error AND the help using clap's
            // built-in mechanisms (writes to stderr via std::io::Write)
            if err.print().is_err() {
                // If printing fails, we still want to exit with error
            }
            let mut cmd = Cli::command();
            if cmd.print_help().is_err() {
                // If printing fails, we still want to exit with error
            }
            std::process::exit(1);
        }
    };

    // Platform check - this tool only works on Windows
    #[cfg(not(windows))]
    {
        // Reference cli.verbose to avoid unused variable warning
        if cli.verbose {
            // Verbose mode requested but not available on non-Windows
        }
        anyhow::bail!(
            "uffs_mft only works on Windows.\n\
             It requires direct access to the NTFS Master File Table via Windows APIs."
        );
    }

    #[cfg(windows)]
    {
        match cli.command {
            Commands::Read {
                drive,
                output,
                mode,
                full,
            } => cmd_read(drive, output, &mode, full).await,
            Commands::Info { drive, deep } => cmd_info(drive, deep).await,
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
            } => cmd_save(drive, &output, !no_compress, compression_level).await,
            Commands::Load {
                input,
                output,
                info_only,
            } => cmd_load(&input, output.as_deref(), info_only).await,
        }
    }
}

#[cfg(windows)]
async fn cmd_read(drive: char, output: PathBuf, mode_str: &str, full: bool) -> Result<()> {
    use std::time::Instant;

    use tracing::debug;
    use uffs_mft::MftReadMode;

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    // Parse read mode
    let mode: MftReadMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;

    info!(
        drive = %drive_upper,
        output = %output.display(),
        mode = %mode,
        full,
        "📂 Starting MFT read operation"
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
        .with_merge_extensions(full);

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
async fn cmd_info(drive: char, deep: bool) -> Result<()> {
    use std::time::Instant;

    use tracing::debug;
    use uffs_mft::platform::{VolumeHandle, detect_drive_type};

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();
    info!(
        drive = %drive_upper,
        deep,
        "📊 Retrieving MFT information{}",
        if deep { " (deep scan)" } else { "" }
    );

    debug!(drive = %drive_upper, "🔓 Opening volume handle");
    let handle = VolumeHandle::open(drive).with_context(|| format!("Failed to open {}:", drive))?;

    // Detect drive type for display
    let drive_type = detect_drive_type(drive_upper);
    let drive_type_str = match drive_type {
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
        println!("📊 DEEP SCAN: Reading all MFT records...");
        println!();

        let reader = MftReader::open(drive)
            .await
            .with_context(|| format!("Failed to open drive {}:", drive))?;

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

        // Count multi-stream and multi-name files
        let multi_stream_count = df
            .column("stream_count")
            .ok()
            .and_then(|c| c.u16().ok())
            .map(|s| s.iter().filter(|v| v.is_some_and(|x| x > 1)).count() as u64)
            .unwrap_or(0);
        let multi_name_count = df
            .column("name_count")
            .ok()
            .and_then(|c| c.u16().ok())
            .map(|s| s.iter().filter(|v| v.is_some_and(|x| x > 1)).count() as u64)
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
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::{VolumeHandle, detect_drive_type};
    use uffs_mft::{MftReader, SaveRawOptions};

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    info!(drive = %drive_upper, "Reading raw MFT from drive");

    // Get volume info for display
    let handle =
        VolumeHandle::open(drive).with_context(|| format!("Failed to open {}:", drive))?;
    let vol_data = handle.volume_data();

    let drive_type = detect_drive_type(drive_upper);
    let drive_type_str = match drive_type {
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

    let options = SaveRawOptions {
        compress,
        compression_level,
    };

    let header = reader
        .save_raw_to_file(output, &options)
        .await
        .with_context(|| format!("Failed to save raw MFT to {}", output.display()))?;

    let elapsed = start_time.elapsed();

    // Get absolute path for display
    let abs_path = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());

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
        println!("  Compression:          none");
    }
    println!();
    println!("⏱️  Completed in {}", format_duration(elapsed));

    Ok(())
}

/// Save MFT - non-Windows stub.
#[cfg(not(windows))]
#[allow(clippy::unused_async)]
async fn cmd_save(
    _drive: char,
    _output: &Path,
    _compress: bool,
    _compression_level: i32,
) -> Result<()> {
    anyhow::bail!("Raw MFT saving is only supported on Windows");
}

/// Load MFT from a saved file and optionally export.
#[cfg(windows)]
async fn cmd_load(input: &Path, output: Option<&Path>, info_only: bool) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::{MftReader, load_raw_mft_header};

    let start_time = Instant::now();

    // Load header first
    let header = load_raw_mft_header(input)
        .with_context(|| format!("Failed to load raw MFT header from {}", input.display()))?;

    // Get absolute path and file size for display
    let abs_path = std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    let file_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);

    // Print formatted output
    println!("═══════════════════════════════════════════════════════════════");
    println!("                         MFT FILE INFO");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("📁 FILE DETAILS");
    println!("  Path:                 {}", abs_path.display());
    println!("  File size:           {}", format_bytes(file_size));
    println!("  Format version:       {}", header.version);
    println!();
    println!("📊 MFT STRUCTURE");
    println!(
        "  Total records:        {}",
        format_number_commas(header.record_count.into())
    );
    println!(
        "  Bytes per record:     {}",
        format_number_commas(header.record_size.into())
    );
    println!(
        "  Original MFT size:   {}",
        format_bytes(header.original_size)
    );
    println!();
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

    if info_only {
        // Parse the MFT to get detailed statistics
        println!();
        println!("📈 PARSING MFT FOR STATISTICS...");

        let df = MftReader::load_raw_to_dataframe(input)
            .with_context(|| format!("Failed to parse raw MFT from {}", input.display()))?;

        let total_parsed = df.height();

        // Extract statistics from the DataFrame
        let dir_count = df
            .column("is_directory")
            .ok()
            .and_then(|c| c.bool().ok())
            .map(|b| b.sum().unwrap_or(0) as u64)
            .unwrap_or(0);
        let file_count = (total_parsed as u64).saturating_sub(dir_count);

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

        // Total size calculation
        let total_size: u64 = df
            .column("size")
            .ok()
            .and_then(|c| c.u64().ok())
            .map(|s| s.iter().flatten().sum())
            .unwrap_or(0);

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
        println!("  Files:                {}", format_number_commas(file_count));
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
        println!("  Sparse:               {}", format_number_commas(sparse_count));

        println!();
        let elapsed = start_time.elapsed();
        println!("⏱️  Completed in {}", format_duration(elapsed));
        return Ok(());
    }

    // Parse and export
    let output = output.context("--output is required when not using --info-only")?;

    // Determine output format from extension
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("parquet");

    let format_name = if ext == "csv" { "CSV" } else { "Parquet" };

    println!();
    println!("📤 EXPORTING TO {}...", format_name);

    let df = MftReader::load_raw_to_dataframe(input)
        .with_context(|| format!("Failed to parse raw MFT from {}", input.display()))?;

    let parsed_count = df.height();

    match ext {
        "csv" => {
            use uffs_polars::CsvWriter;
            use uffs_polars::SerWriter;
            use std::fs::File;

            let file = File::create(output)?;
            let mut df = df;
            CsvWriter::new(file).finish(&mut df)?;
        }
        _ => {
            let mut df = df;
            MftReader::save_parquet(&mut df, output)?;
        }
    }

    // Get absolute path and file size after creation
    let output_abs = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let output_size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);

    println!();
    println!("📁 OUTPUT FILE");
    println!("  Path:                 {}", output_abs.display());
    println!("  Format:               {}", format_name);
    println!("  File size:           {}", format_bytes(output_size));
    println!(
        "  Records exported:     {}",
        format_number_commas(parsed_count as u64)
    );

    let elapsed = start_time.elapsed();
    println!();
    println!("⏱️  Completed in {}", format_duration(elapsed));

    Ok(())
}

/// Load MFT - non-Windows stub.
#[cfg(not(windows))]
#[allow(clippy::unused_async)]
async fn cmd_load(_input: &Path, _output: Option<&Path>, _info_only: bool) -> Result<()> {
    anyhow::bail!("Raw MFT loading is only supported on Windows");
}
