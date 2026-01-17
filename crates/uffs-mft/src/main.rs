//! # uffs_mft: MFT Command-Line Tool
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
//! **Note**: This tool requires Administrator privileges on Windows.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[cfg(windows)]
use anyhow::Context;
#[cfg(windows)]
use indicatif::{ProgressBar, ProgressStyle};
#[cfg(windows)]
use tracing::info;
#[cfg(windows)]
use uffs_mft::MftReader;

/// uffs_mft: Low-level NTFS MFT reading tool
#[derive(Parser)]
#[command(name = "uffs_mft")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

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
    },

    /// List all available NTFS drives
    Drives,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Check platform
    #[cfg(not(windows))]
    {
        bail!(
            "uffs_mft only works on Windows.\n\
             It requires direct access to the NTFS Master File Table via Windows APIs."
        );
    }

    #[cfg(windows)]
    {
        match cli.command {
            Commands::Read { drive, output } => cmd_read(drive, output).await,
            Commands::Info { drive } => cmd_info(drive).await,
            Commands::Drives => cmd_drives().await,
        }
    }
}

#[cfg(windows)]
async fn cmd_read(drive: char, output: PathBuf) -> Result<()> {
    info!("Reading MFT from {}:", drive.to_ascii_uppercase());

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("valid template"),
    );
    pb.set_message("Opening volume...");

    let reader = MftReader::open(drive)
        .await
        .with_context(|| format!("Failed to open drive {}:", drive))?;

    pb.set_message("Reading MFT records...");
    let df = reader
        .read_all()
        .await
        .with_context(|| "Failed to read MFT")?;

    pb.set_message("Saving to Parquet...");
    MftReader::save_parquet(&df, &output).with_context(|| "Failed to save Parquet")?;

    pb.finish_with_message(format!(
        "✅ Exported {} records to {}",
        df.height(),
        output.display()
    ));

    Ok(())
}

#[cfg(windows)]
async fn cmd_info(drive: char) -> Result<()> {
    use uffs_mft::platform::VolumeHandle;

    info!("MFT Info for {}:", drive.to_ascii_uppercase());

    let handle = VolumeHandle::open(drive).with_context(|| format!("Failed to open {}:", drive))?;

    let vol_data = handle
        .get_ntfs_volume_data()
        .with_context(|| "Failed to get volume data")?;

    println!("Drive: {}:", drive.to_ascii_uppercase());
    println!("  Bytes per sector:     {}", vol_data.bytes_per_sector);
    println!("  Bytes per cluster:    {}", vol_data.bytes_per_cluster);
    println!(
        "  Bytes per MFT record: {}",
        vol_data.bytes_per_file_record_segment
    );
    println!("  Total clusters:       {}", vol_data.total_clusters);
    println!("  MFT start LCN:        {}", vol_data.mft_start_lcn);
    println!("  MFT valid length:     {} bytes", vol_data.mft_valid_data_length);

    let record_count = vol_data.mft_valid_data_length / vol_data.bytes_per_file_record_segment as u64;
    println!("  Estimated records:    {record_count}");

    Ok(())
}

#[cfg(windows)]
async fn cmd_drives() -> Result<()> {
    use uffs_mft::platform::detect_ntfs_drives;

    info!("Detecting NTFS drives...");

    let drives = detect_ntfs_drives();

    if drives.is_empty() {
        println!("No NTFS drives found.");
    } else {
        println!("NTFS drives:");
        for drive in drives {
            println!("  {}:", drive);
        }
    }

    Ok(())
}

