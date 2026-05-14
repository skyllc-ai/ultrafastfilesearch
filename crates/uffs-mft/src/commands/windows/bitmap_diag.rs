// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bitmap diagnostic command handlers.
//!
//! These commands print human-readable bitmap statistics to stdout and convert
//! `usize` populations into `f64` percentages for display.  The lint
//! exemptions below capture those CLI-specific patterns; library code never
//! inherits them.
#![expect(
    clippy::print_stdout,
    reason = "intentional user-facing CLI bitmap diagnostic output"
)]
#![expect(
    clippy::float_arithmetic,
    clippy::default_numeric_fallback,
    reason = "percent / fraction calculations convert integer counters into f64 for human-readable display"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "short identifiers used for printf-style indices in CLI output"
)]
#![expect(
    clippy::too_many_lines,
    reason = "diagnostic command runs a configure -> sample -> count -> print pipeline; extracting helpers fragments the readable narrative"
)]
#![expect(
    clippy::naive_bytecount,
    reason = "small one-shot CLI bitmap summary; SIMD-accelerated `bytecount` would be overkill for an interactive diagnostic and would add a dependency"
)]

use anyhow::{Context as _, Result};
use uffs_mft::{bytes_to_mb_f64, u64_to_f64, usize_to_f64, usize_to_u64};

// ============================================================================
// Bitmap Diagnostic Command
// ============================================================================

/// Diagnose MFT bitmap to investigate why records aren't being skipped.
#[cfg(windows)]
pub(crate) async fn cmd_bitmap_diag(
    drive: uffs_mft::platform::DriveLetter,
    show_samples: bool,
) -> Result<()> {
    use uffs_mft::VolumeHandle;

    println!("═══════════════════════════════════════════════════════════════");
    println!("              MFT BITMAP DIAGNOSTIC - Drive {drive}:");
    println!("═══════════════════════════════════════════════════════════════");
    println!();

    // Open volume
    let handle =
        VolumeHandle::open(drive).with_context(|| format!("Failed to open volume {drive}:"))?;

    let volume_data = handle.volume_data();
    let record_size = volume_data.bytes_per_file_record_segment;
    let mft_size = volume_data.mft_valid_data_length;
    let total_records_from_size = mft_size / u64::from(record_size);

    println!("📊 VOLUME DATA");
    println!(
        "   MFT valid data length: {} bytes ({:.2} MB)",
        mft_size,
        bytes_to_mb_f64(mft_size)
    );
    println!("   Bytes per record: {record_size}");
    println!("   Total records (from size): {total_records_from_size}");
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
            let utilization =
                (usize_to_f64(in_use_count) / usize_to_f64(bitmap_record_count)) * 100.0;

            println!("   ✅ Bitmap retrieved successfully");
            println!("   Bitmap size: {bitmap_bytes} bytes");
            println!("   Records covered: {bitmap_record_count}");
            println!("   In-use records: {in_use_count}");
            println!("   Free records: {free_count}");
            println!("   Utilization: {utilization:.2}%");
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
                (usize_to_f64(all_ff_bytes) / usize_to_f64(bitmap_bytes)) * 100.0
            );
            println!(
                "   Bytes with no bits set (0x00): {} ({:.1}%)",
                all_00_bytes,
                (usize_to_f64(all_00_bytes) / usize_to_f64(bitmap_bytes)) * 100.0
            );
            println!(
                "   Mixed bytes: {} ({:.1}%)",
                mixed_bytes,
                (usize_to_f64(mixed_bytes) / usize_to_f64(bitmap_bytes)) * 100.0
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
                    (usize_to_f64(free_count) / usize_to_f64(bitmap_record_count)) * 100.0
                );
            }
            println!();

            // Sample first few bytes
            println!("📝 BITMAP SAMPLE (first 32 bytes)");
            let sample_bytes: Vec<_> = bitmap.as_bytes().iter().take(32).collect();
            print!("   ");
            for (i, &byte) in sample_bytes.iter().enumerate() {
                print!("{byte:02X} ");
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
                    print!("{byte:02X} ");
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
                for frs in 0..16_u64 {
                    let in_use = bitmap.is_record_in_use(frs);
                    print!("{}: {} ", frs, if in_use { "✓" } else { "✗" });
                }
                println!();

                // Check some records in the middle
                let mid = bitmap_record_count / 2;
                println!("   Checking records {}-{}:", mid, mid + 15);
                print!("   ");
                for frs in mid..(mid + 16).min(bitmap_record_count) {
                    let in_use = bitmap.is_record_in_use(usize_to_u64(frs));
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
                    let in_use = bitmap.is_record_in_use(usize_to_u64(frs));
                    print!("{}: {} ", frs, if in_use { "✓" } else { "✗" });
                }
                println!();
                println!();
            }

            // Test calculate_skip_range
            println!("📝 SKIP RANGE CALCULATION TEST");
            let test_ranges = [
                (0_u64, 1000_u64),
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
                    (u64_to_f64(skipped) / u64_to_f64(range_size)) * 100.0
                );
            }
            println!();
        }
        Err(e) => {
            println!("   ❌ Failed to retrieve bitmap: {e}");
            println!("   This means the fallback (all records valid) would be used.");
            println!();
        }
    }

    println!("═══════════════════════════════════════════════════════════════");

    Ok(())
}
