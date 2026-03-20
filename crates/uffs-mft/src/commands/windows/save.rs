//! Raw MFT save command handlers.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::info;

use super::shared::drive_type_label;
use crate::display::{clean_path_for_display, format_bytes, format_duration, format_number_commas};

// ============================================================================
// Save/Load Raw MFT Commands
// ============================================================================

/// Save MFT bytes to a file for offline analysis.
#[cfg(windows)]
#[expect(clippy::too_many_arguments, reason = "CLI command with many options")]
pub async fn cmd_save(
    drive: char,
    output: &Path,
    compress: bool,
    compression_level: i32,
    raw_compat: bool,
    iocp_mode: bool,
    iocp_concurrency: usize,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::{VolumeHandle, detect_drive_type};

    // IOCP mode uses a different code path
    if iocp_mode {
        return cmd_save_iocp(drive, output, compress, compression_level, iocp_concurrency).await;
    }

    use uffs_mft::{MftReader, SaveRawOptions};

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    info!(drive = %drive_upper, "Reading raw MFT from drive");

    // Get volume info for display
    let handle = VolumeHandle::open(drive).with_context(|| format!("Failed to open {}:", drive))?;
    let vol_data = handle.volume_data();

    let drive_type = detect_drive_type(drive_upper);
    let drive_type_str = drive_type_label(drive_type, "Unknown");

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
    let reader =
        MftReader::open(drive).with_context(|| format!("Failed to open drive {drive}:"))?;

    // Raw compat mode implies no compression
    let options = SaveRawOptions {
        compress: if raw_compat { false } else { compress },
        compression_level,
        volume_letter: drive_upper,
        raw_compat,
    };

    let header = reader
        .save_raw_to_file(output, &options)
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
        #[expect(
            clippy::cast_precision_loss,
            reason = "precision loss acceptable for display percentages"
        )]
        #[expect(
            clippy::float_arithmetic,
            reason = "floating-point needed for compression ratio calculation"
        )]
        let ratio = header.compressed_size as f64 / header.original_size as f64 * 100.0_f64;
        println!("  Compression ratio:    {ratio:.1}%");
        #[expect(
            clippy::cast_precision_loss,
            reason = "precision loss acceptable for display percentages"
        )]
        #[expect(
            clippy::float_arithmetic,
            reason = "floating-point needed for savings calculation"
        )]
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

/// Save MFT in IOCP capture mode.
///
/// This mode reads MFT using IOCP and saves chunks in the order they complete,
/// capturing the non-deterministic I/O ordering for realistic testing.
#[cfg(windows)]
async fn cmd_save_iocp(
    drive: char,
    output: &Path,
    compress: bool,
    compression_level: i32,
    concurrency: usize,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::{VolumeHandle, detect_drive_type};
    use uffs_mft::{IocpCaptureOptions, MftReader};

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    info!(drive = %drive_upper, concurrency, "Reading MFT with IOCP capture mode");

    // Get volume info for display
    let handle = VolumeHandle::open(drive).with_context(|| format!("Failed to open {}:", drive))?;
    let vol_data = handle.volume_data();

    let drive_type = detect_drive_type(drive_upper);
    let drive_type_str = drive_type_label(drive_type, "Unknown");

    // Calculate metrics
    let record_count =
        vol_data.mft_valid_data_length / vol_data.bytes_per_file_record_segment as u64;

    println!("═══════════════════════════════════════════════════════════════");
    println!("                    MFT IOCP CAPTURE");
    println!(
        "                    Drive: {}: ({})",
        drive_upper, drive_type_str
    );
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("📊 MFT INFO");
    println!(
        "  Total records:        {}",
        format_number_commas(record_count)
    );
    println!("  IOCP concurrency:     {}", concurrency);
    println!();
    println!("⏳ Reading with IOCP and capturing chunk order...");

    // Create capture options
    let reserved_alloc = vol_data.reserved_allocated_bytes();
    let options = IocpCaptureOptions {
        compress,
        compression_level,
        volume_letter: drive_upper,
        concurrency: concurrency as u8,
        reserved_allocated_bytes: reserved_alloc,
    };

    // Open reader and save with IOCP capture
    let reader =
        MftReader::open(drive).with_context(|| format!("Failed to open drive {drive}:"))?;

    let header = reader
        .save_iocp_capture(output, &options)
        .with_context(|| format!("Failed to save IOCP capture to {}", output.display()))?;

    let elapsed = start_time.elapsed();

    // Get absolute path for display
    let abs_path = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let abs_path = clean_path_for_display(&abs_path);

    println!();
    println!("💾 OUTPUT FILE");
    println!("  Path:                 {}", abs_path.display());
    println!("  Format:               UFFS-IOCP (chunk order preserved)");
    println!(
        "  Chunks captured:      {}",
        format_number_commas(u64::from(header.chunk_count))
    );
    println!(
        "  Total records:        {}",
        format_number_commas(header.total_records)
    );
    println!(
        "  Data size:            {}",
        format_bytes(header.total_data_size)
    );
    if header.is_compressed() {
        println!("  Compression:          zstd (level {})", compression_level);
    } else {
        println!("  Compression:          none");
    }
    println!();
    println!("⏱️  Completed in {}", format_duration(elapsed));

    Ok(())
}
