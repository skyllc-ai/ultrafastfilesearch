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
use std::io::stdout;
use std::path::PathBuf;

#[cfg(windows)]
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
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
use tracing::info;
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
    },

    /// Show MFT information for a drive
    Info {
        /// Drive letter (e.g., C, D, E)
        #[arg(short, long)]
        drive: char,

        /// Perform deep scan (reads all MFT records for detailed statistics)
        #[arg(long, default_value = "false")]
        deep: bool,
    },

    /// List all available NTFS drives
    Drives,
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

    // Platform check - this tool only works on Windows
    #[cfg(not(windows))]
    {
        // Parse CLI to show help/version even on non-Windows
        let _cli = Cli::parse();
        anyhow::bail!(
            "uffs_mft only works on Windows.\n\
             It requires direct access to the NTFS Master File Table via Windows APIs."
        );
    }

    #[cfg(windows)]
    let cli = Cli::parse();

    #[cfg(windows)]
    {
        match cli.command {
            Commands::Read { drive, output } => cmd_read(drive, output).await,
            Commands::Info { drive, deep } => cmd_info(drive, deep).await,
            Commands::Drives => cmd_drives().await,
        }
    }
}

#[cfg(windows)]
async fn cmd_read(drive: char, output: PathBuf) -> Result<()> {
    use std::time::Instant;

    use tracing::debug;

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    info!(
        drive = %drive_upper,
        output = %output.display(),
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
        .with_context(|| format!("Failed to open drive {}:", drive))?;

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
        "✅ Exported {} records to {} ({:.2} MB) in {:.1}s",
        record_count,
        output.display(),
        file_size_mb,
        total_elapsed.as_secs_f64()
    ));

    Ok(())
}

#[cfg(windows)]
async fn cmd_info(drive: char, deep: bool) -> Result<()> {
    use std::time::Instant;

    use tracing::debug;
    use uffs_mft::platform::VolumeHandle;

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

    let vol_data = handle.volume_data();

    // Calculate derived metrics
    let record_count =
        vol_data.mft_valid_data_length / vol_data.bytes_per_file_record_segment as u64;
    let mft_size_mb = vol_data.mft_valid_data_length as f64 / (1024.0 * 1024.0);
    let volume_size_bytes = vol_data.total_clusters as u64 * vol_data.bytes_per_cluster as u64;
    let volume_size_gb = volume_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
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
    println!("                    Drive: {}:", drive_upper);
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("📐 VOLUME GEOMETRY");
    println!("  Bytes per sector:     {}", vol_data.bytes_per_sector);
    println!("  Bytes per cluster:    {}", vol_data.bytes_per_cluster);
    println!(
        "  Bytes per MFT record: {}",
        vol_data.bytes_per_file_record_segment
    );
    println!("  Total clusters:       {}", vol_data.total_clusters);
    println!("  Volume size:          {:.2} GB", volume_size_gb);
    println!();
    println!("📁 MFT STRUCTURE");
    println!("  MFT start LCN:        {}", vol_data.mft_start_lcn);
    println!("  MFT size:             {:.2} MB", mft_size_mb);
    println!("  MFT % of volume:      {:.3}%", mft_percentage);
    println!("  Total records:        {}", record_count);
    println!("  In-use records:       {}", in_use_records);
    println!("  Free records:         {}", free_records);
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
        println!("  Parsed records:       {}", total_parsed);
        println!("  Directories:          {}", dir_count);
        println!("  Files:                {}", file_count);
        println!();
        println!("🏷️  ATTRIBUTE FLAGS");
        println!("  Hidden:               {}", hidden_count);
        println!("  System:               {}", system_count);
        println!("  Read-only:            {}", readonly_count);
        println!("  Archive:              {}", archive_count);
        println!("  Compressed:           {}", compressed_count);
        println!("  Encrypted:            {}", encrypted_count);
        println!("  Sparse:               {}", sparse_count);
        println!("  Reparse points:       {}", reparse_count);
        println!();
        println!("🔗 EXTENDED ATTRIBUTES");
        println!(
            "  Files with ADS:       {} (Alternate Data Streams)",
            multi_stream_count
        );
        println!("  Files with hardlinks: {}", multi_name_count);
        println!();
        println!("💾 STORAGE ANALYSIS");
        println!(
            "  Total file size:      {:.2} GB",
            total_file_size as f64 / (1024.0 * 1024.0 * 1024.0)
        );
        println!(
            "  Total allocated:      {:.2} GB",
            total_allocated_size as f64 / (1024.0 * 1024.0 * 1024.0)
        );
        println!(
            "  Slack space:          {:.2} MB ({:.1}%)",
            slack_space as f64 / (1024.0 * 1024.0),
            slack_percentage
        );
        println!();

        let deep_elapsed = start_time.elapsed();
        println!(
            "⏱️  Deep scan completed in {:.2}s",
            deep_elapsed.as_secs_f64()
        );
    } else {
        println!("💡 TIP: Use --deep for detailed file statistics (dirs, files, attributes).");
        println!();
        println!("⏱️  Completed in {:.1}ms", elapsed.as_secs_f64() * 1000.0);
    }

    println!("═══════════════════════════════════════════════════════════════");

    Ok(())
}

#[cfg(windows)]
async fn cmd_drives() -> Result<()> {
    use tracing::debug;
    use uffs_mft::platform::{VolumeHandle, detect_ntfs_drives};

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

        println!("NTFS drives:");
        for drive in &drives {
            // Try to get volume info for each drive
            if let Ok(handle) = VolumeHandle::open(*drive) {
                let vol_data = handle.volume_data();
                let volume_size_gb =
                    (vol_data.total_clusters as u64 * vol_data.bytes_per_cluster as u64) as f64
                        / (1024.0 * 1024.0 * 1024.0);
                let record_count =
                    vol_data.mft_valid_data_length / vol_data.bytes_per_file_record_segment as u64;

                debug!(
                    drive = %drive,
                    volume_size_gb = format!("{:.2}", volume_size_gb),
                    mft_records = record_count,
                    "📁 Drive details"
                );

                println!(
                    "  {}: ({:.1} GB, ~{} MFT records)",
                    drive, volume_size_gb, record_count
                );
            } else {
                println!("  {}:", drive);
            }
        }
    }

    Ok(())
}
