//! MFT Reader - A Rust tool to read raw NTFS MFT records and export to CSV
//!
//! This tool reads the Master File Table (MFT) directly from an NTFS volume,
//! bypassing the Windows file system APIs for maximum speed.
//!
//! Usage:
//!   mft-reader.exe -d C -o output.csv
//!   mft-reader.exe --drive C --output output.csv
//!
//! Requires administrator privileges to access the raw volume.

mod csv_writer;
mod mft_reader;
mod ntfs;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::time::Instant;

/// MFT Reader - Read raw NTFS MFT records and export to CSV
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Drive letter to read (e.g., C, D, E)
    #[arg(short, long)]
    drive: char,

    /// Output CSV file path (use - for stdout)
    #[arg(short, long, default_value = "mft_records.csv")]
    output: String,

    /// Show verbose output
    #[arg(short, long, default_value_t = false)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Validate drive letter
    let drive_letter = args.drive.to_ascii_uppercase();
    if !drive_letter.is_ascii_alphabetic() {
        anyhow::bail!("Invalid drive letter: {}", args.drive);
    }

    println!("MFT Reader - NTFS Master File Table Reader");
    println!("==========================================");
    println!();

    println!("Opening volume {}:...", drive_letter);
    let start_time = Instant::now();

    // Open the MFT reader
    let reader = mft_reader::MftReader::open(drive_letter)
        .with_context(|| format!("Failed to open volume {}:", drive_letter))?;

    if args.verbose {
        println!("  Cluster size: {} bytes", reader.cluster_size);
        println!("  MFT record size: {} bytes", reader.mft_record_size);
        println!("  MFT start LCN: {}", reader.mft_start_lcn);
        println!("  MFT valid data length: {} bytes", reader.mft_valid_data_length);
        println!("  MFT extents: {}", reader.mft_extents.len());
        println!("  Total MFT records: {}", reader.record_count());
        println!();
    }

    // Read all MFT records
    println!("Reading MFT records...");
    let records = reader
        .read_all_records()
        .context("Failed to read MFT records")?;

    let read_time = start_time.elapsed();
    println!(
        "Read {} records in {:.2} seconds ({:.0} records/sec)",
        records.len(),
        read_time.as_secs_f64(),
        records.len() as f64 / read_time.as_secs_f64()
    );
    println!();

    // Write to CSV
    println!("Writing CSV output...");
    let write_start = Instant::now();

    if args.output == "-" {
        csv_writer::write_csv_stdout(&records).context("Failed to write CSV to stdout")?;
    } else {
        let output_path = PathBuf::from(&args.output);
        csv_writer::write_csv(&records, &output_path)
            .with_context(|| format!("Failed to write CSV to {}", args.output))?;
        println!("Output written to: {}", args.output);
    }

    let write_time = write_start.elapsed();
    let total_time = start_time.elapsed();

    println!();
    println!("Statistics:");
    println!("  Total records: {}", records.len());
    println!(
        "  In-use records: {}",
        records.iter().filter(|r| r.is_in_use).count()
    );
    println!(
        "  Directories: {}",
        records.iter().filter(|r| r.is_directory).count()
    );
    println!(
        "  Files: {}",
        records.iter().filter(|r| !r.is_directory && r.is_in_use).count()
    );
    println!("  Read time: {:.2}s", read_time.as_secs_f64());
    println!("  Write time: {:.2}s", write_time.as_secs_f64());
    println!("  Total time: {:.2}s", total_time.as_secs_f64());

    Ok(())
}

