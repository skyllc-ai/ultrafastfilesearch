//! Raw MFT benchmark command handlers.

use anyhow::{Context, Result};

use crate::display::char_or_dot;

/// Load MFT from a saved file and optionally export.
///
/// Works on all platforms - parses NTFS structures from saved file.
/// Supports both UFFS-MFT format and raw NTFS format.
#[expect(
    clippy::too_many_lines,
    reason = "cli output function with complex display logic"
)]
#[expect(clippy::print_stdout, reason = "intentional user-facing cli output")]
#[expect(
    clippy::shadow_reuse,
    reason = "shadow reuse improves readability in sequential processing"
)]
#[expect(
    clippy::min_ident_chars,
    reason = "short identifiers used for concise loop variables"
)]
#[expect(
    clippy::expect_used,
    reason = "expect used for file operations that should not fail after validation"
)]
#[expect(
    clippy::single_call_fn,
    reason = "logical separation of load command implementation"
)]
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "bool params map directly to cli flags"
)]
pub async fn cmd_benchmark_mft(drive: char) -> Result<()> {
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
    // Print Volume Information (matches the reference benchmark layout)
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
    // Print MFT Information (matches the reference benchmark layout)
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
    const BUFFER_SIZE: usize = 1024 * 1024; // 1 MB buffer (matches the reference benchmark layout)
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
    // Print benchmark results using the historical layout
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
