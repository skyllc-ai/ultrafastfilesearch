// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Raw MFT benchmark command handlers.
//!
//! These commands print human-readable benchmark output to stdout, perform
//! Win32 raw I/O against `\\.\X:` volume handles, and convert byte counters
//! into `f64` for MB/s display.  The lint exemptions below capture those
//! CLI-specific patterns; library code never inherits them.
#![expect(
    clippy::print_stdout,
    reason = "intentional user-facing CLI raw-MFT benchmark output"
)]
#![expect(
    clippy::float_arithmetic,
    clippy::default_numeric_fallback,
    reason = "byte / rate calculations divide f64 helpers for human-readable MB/s display"
)]
#![expect(
    clippy::redundant_type_annotations,
    reason = "explicit types make the raw Win32 ABI plumbing self-documenting in this CLI tool"
)]
#![expect(
    clippy::too_many_lines,
    reason = "benchmark commands run a configure -> read -> compute -> print pipeline that is most readable inline"
)]
#![expect(
    clippy::items_after_statements,
    reason = "local `const BUFFER_SIZE` keeps the buffer size adjacent to its sole use site"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "slices into the local 1\u{a0}MiB buffer; sizes are clamped via `min(BUFFER_SIZE)` and `min(chunk_size)` above"
)]

use anyhow::{Context as _, Result};
use uffs_mft::{bytes_to_mb_f64, frs_to_usize, millis_to_u64, u32_as_usize, usize_to_u64};

use crate::display::char_or_dot;

/// Load MFT from a saved file and optionally export.
///
/// Works on all platforms - parses NTFS structures from saved file.
/// Supports both UFFS-MFT format and raw NTFS format.
#[expect(
    clippy::single_call_fn,
    reason = "logical separation of load command implementation"
)]
#[expect(
    unsafe_code,
    reason = "FFI: SetFilePointerEx, ReadFile for raw volume I/O"
)]
pub(crate) async fn cmd_benchmark_mft(drive: uffs_mft::platform::DriveLetter) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::io::AlignedBuffer;
    use uffs_mft::platform::VolumeHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

    // =========================================================================
    // Open volume and get metadata
    // =========================================================================
    let handle =
        VolumeHandle::open(drive).with_context(|| format!("Failed to open volume {drive}:"))?;

    let vol_data = handle.volume_data();

    // Get MFT extents
    let extents = handle
        .get_mft_extents()
        .with_context(|| format!("Failed to get MFT extents for {drive}:"))?;

    // Calculate MFT metrics
    let mft_size = vol_data.mft_valid_data_length;
    let record_size = vol_data.bytes_per_file_record_segment;
    let record_count = mft_size / u64::from(record_size);
    let mft_size_mb = mft_size / (1024 * 1024);

    // =========================================================================
    // Print Volume Information (matches the reference benchmark layout)
    // =========================================================================
    println!("=== MFT Read Benchmark Tool ===");
    println!("Drive: {drive}:");
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
    // Print MFT Information (matches the reference benchmark layout)
    // =========================================================================
    println!("MFT Information:");
    println!("  Extents: {}", extents.len());
    println!("  MFT Size: {mft_size} bytes ({mft_size_mb} MB)");
    println!("  Record Size: {record_size} bytes");
    println!("  Record Count: {record_count}");
    println!("  Total Bytes to Read: {mft_size}");
    println!();
    println!("Starting MFT read benchmark...");
    println!();

    // =========================================================================
    // Benchmark: Read MFT with 1MB synchronous reads
    // =========================================================================
    const BUFFER_SIZE: usize = 1024 * 1024; // 1 MB buffer (matches the reference benchmark layout)
    let sector_size = u32_as_usize(vol_data.bytes_per_sector);
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

        // Calculate byte offset and size for this extent.
        //
        // NTFS exposes `LcnPosition` (extent LCN) as `i64` on the FFI
        // side even though valid extents are non-negative.  The
        // `extent.lcn < 0` guard above filters sparse extents, so
        // `i64::cast_unsigned` is the documented exact-bit-pattern
        // reinterpret (Rust 1.87 stable) without a `cast_sign_loss`
        // expect.
        let lcn_u64 = extent.lcn.cast_unsigned();
        let extent_byte_offset = lcn_u64 * u64::from(bytes_per_cluster);
        let extent_byte_size = extent.cluster_count * u64::from(bytes_per_cluster);

        // Don't read beyond MFT valid data length
        let bytes_remaining = mft_size.saturating_sub(total_bytes_read);
        let extent_bytes_to_read = extent_byte_size.min(bytes_remaining);

        if extent_bytes_to_read == 0 {
            break;
        }

        // Seek to extent start.
        //
        // Win32 `SetFilePointerEx` takes the offset as `i64`.  NTFS
        // volume sizes never exceed `i64::MAX`, so the same bit pattern
        // represents both unsigned (NTFS) and signed (Win32) views;
        // `u64::cast_signed` documents that reinterpret without needing
        // a `cast_possible_wrap` expect.
        let signed_offset = extent_byte_offset.cast_signed();
        // SAFETY: `raw_handle` is a live volume handle owned by `vol_data`'s
        // `VolumeHandle` and `signed_offset` is bounded by the MFT extent
        // returned by Windows; the cast to `i64` is safe because volume sizes
        // never exceed `i64::MAX`.
        let seek_result = unsafe { SetFilePointerEx(raw_handle, signed_offset, None, FILE_BEGIN) };
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
            let chunk_size = frs_to_usize(extent_bytes_to_read - extent_offset).min(BUFFER_SIZE);
            // Round up to sector boundary for FILE_FLAG_NO_BUFFERING
            let aligned_chunk_size = chunk_size.div_ceil(sector_size) * sector_size;

            let buf_slice = buffer.as_mut_slice();
            let mut bytes_read: u32 = 0;

            // SAFETY: `raw_handle` is a live volume handle, `buf_slice` is a
            // sector-aligned writable region of the owned `AlignedBuffer`, and
            // `bytes_read` is a valid out-parameter for the call duration.
            let read_result = unsafe {
                ReadFile(
                    raw_handle,
                    Some(&mut buf_slice[..aligned_chunk_size]),
                    Some(&raw mut bytes_read),
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
            let actual_bytes = u32_as_usize(bytes_read).min(chunk_size);
            if actual_bytes >= 4 {
                last_4_bytes.copy_from_slice(&buf_slice[actual_bytes - 4..actual_bytes]);
            }

            total_bytes_read += usize_to_u64(actual_bytes);
            extent_offset += u64::from(bytes_read);

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
    let elapsed_ms = millis_to_u64(elapsed.as_millis());
    let elapsed_secs = elapsed.as_secs_f64();

    // Calculate throughput
    let read_speed_mb_s = if elapsed_secs > 0.0 {
        bytes_to_mb_f64(total_bytes_read) / elapsed_secs
    } else {
        0.0
    };

    let total_mb = total_bytes_read / (1024 * 1024);

    // =========================================================================
    // Print benchmark results using the historical layout
    // =========================================================================
    println!("=== Benchmark Results ===");
    println!("Total bytes read: {total_bytes_read} ({total_mb} MB)");
    println!("Total records: {record_count}");
    println!("Time elapsed: {elapsed_ms} ms ({elapsed_secs:.3} seconds)");
    println!("Read speed: {read_speed_mb_s:.2} MB/s");
    println!();

    // =========================================================================
    // Print proof of complete read using the historical layout
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
    println!("First 4 bytes (hex): {first_hex}  (ASCII: {first_ascii})");

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
    println!("Last 4 bytes (hex):  {last_hex}  (ASCII: {last_ascii})");
    println!();
    println!("Note: First 4 bytes should be 'FILE' (46 49 4C 45) - the MFT record signature.");

    Ok(())
}
