//! Windows-only command handlers for the `uffs_mft` binary.
//! Exception: This module exceeds 800 lines because Windows command entry
//! points remain consolidated pending a dedicated command-module split.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};
use uffs_mft::MftReader;

use crate::display::{
    char_or_dot, clean_path_for_display, format_bytes, format_duration, format_number,
    format_number_commas, format_usn_reason, truncate_string,
};
use crate::progress::spinner;

pub async fn cmd_read(
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

    let pb = spinner("Opening volume...");

    debug!(drive = %drive_upper, "🔓 Opening volume handle");
    let open_start = Instant::now();

    let reader = MftReader::open(drive)
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

    let mut df = reader.read_all().with_context(|| "Failed to read MFT")?;

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
pub async fn cmd_info(drive: char, deep: bool, no_bitmap: bool, unique: bool) -> Result<()> {
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
            .with_context(|| format!("Failed to open drive {}:", drive))?
            .with_use_bitmap(!no_bitmap)
            .with_expand_links(!unique); // unique=true means don't expand

        let df = reader.read_all().with_context(|| "Failed to read MFT")?;

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

        // Calculate the expanded-row estimate (names × streams per record).
        // The reference output expands each record into one row per
        // (name, stream) combination.
        let expanded_row_equivalent_count = df
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
            "  Expanded rows:        {} (names × streams)",
            format_number_commas(expanded_row_equivalent_count)
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
pub async fn cmd_drives() -> Result<()> {
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
#[expect(
    unsafe_code,
    reason = "required for windows ffi call to GetVolumeInformationW"
)]
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
pub async fn cmd_bench(
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
        take_single_benchmark_result(results, "benchmark run requested one iteration")?
    } else {
        average_results(&results)?
    };

    if json {
        println!("{}", avg_result.to_json());
    } else {
        print_benchmark_result(&avg_result, runs);
    }

    Ok(())
}

#[cfg(windows)]
fn take_single_benchmark_result(
    results: Vec<uffs_mft::BenchmarkResult>,
    context: &str,
) -> Result<uffs_mft::BenchmarkResult> {
    results
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("{context}: no benchmark results were collected"))
}

#[cfg(windows)]
fn average_results(results: &[uffs_mft::BenchmarkResult]) -> Result<uffs_mft::BenchmarkResult> {
    let Some(first) = results.first() else {
        anyhow::bail!("no benchmark results were collected");
    };
    let n = results.len() as u64;

    let avg_timings = uffs_mft::PhaseTimings {
        open_ms: results.iter().map(|r| r.timings.open_ms).sum::<u64>() / n,
        read_ms: results.iter().map(|r| r.timings.read_ms).sum::<u64>() / n,
        parse_ms: results.iter().map(|r| r.timings.parse_ms).sum::<u64>() / n,
        merge_ms: results.iter().map(|r| r.timings.merge_ms).sum::<u64>() / n,
        df_build_ms: results.iter().map(|r| r.timings.df_build_ms).sum::<u64>() / n,
        index_build_ms: results
            .iter()
            .map(|r| r.timings.index_build_ms)
            .sum::<u64>()
            / n,
        tree_metrics_ms: results
            .iter()
            .map(|r| r.timings.tree_metrics_ms)
            .sum::<u64>()
            / n,
        total_ms: results.iter().map(|r| r.timings.total_ms).sum::<u64>() / n,
    };

    let avg_throughput: f64 =
        results.iter().map(|r| r.throughput_mb_s).sum::<f64>() / results.len() as f64;
    let avg_records_per_sec: f64 =
        results.iter().map(|r| r.records_per_sec).sum::<f64>() / results.len() as f64;

    Ok(uffs_mft::BenchmarkResult {
        timings: avg_timings,
        characteristics: first.characteristics.clone(),
        records_parsed: first.records_parsed,
        throughput_mb_s: avg_throughput,
        records_per_sec: avg_records_per_sec,
    })
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
    println!(
        "   Read (I/O):       {:>8} ms  ← estimated (DataFrame path)",
        t.read_ms
    );
    println!(
        "   Parse (CPU):      {:>8} ms  ← estimated (DataFrame path)",
        t.parse_ms
    );
    println!(
        "   Merge:            {:>8} ms  ← estimated (DataFrame path)",
        t.merge_ms
    );
    println!("   DataFrame Build:  {:>8} ms", t.df_build_ms);
    println!("   ─────────────────────────────");
    println!("   TOTAL:            {:>8} ms", t.total_ms);
    println!();

    // Note about estimates
    println!("   ⚠️  Read/Parse/Merge are estimated in DataFrame path.");
    println!("      Use `benchmark-index-lean` for accurate phase timing.");
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
pub async fn cmd_bench_all(
    output: Option<PathBuf>,
    no_df: bool,
    runs: u32,
    full: bool,
) -> Result<()> {
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
        .with_context(|| format!("Failed to open drive {}:", drive))?
        .with_merge_extensions(full);

    let mut results: Vec<uffs_mft::BenchmarkResult> = Vec::with_capacity(runs as usize);

    for run in 1..=runs {
        if runs > 1 {
            println!("     Run {}/{}...", run, runs);
        }

        let (_, result) = reader
            .read_with_timing(no_df)
            .with_context(|| format!("Benchmark run {} failed", run))?;

        results.push(result);

        // Small delay between runs
        if run < runs {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    // Average results
    Ok(if runs == 1 {
        take_single_benchmark_result(results, "single-drive benchmark requested one iteration")?
    } else {
        average_results(&results)?
    })
}

// ============================================================================
// Bitmap Diagnostic Command
// ============================================================================

/// Diagnose MFT bitmap to investigate why records aren't being skipped.
#[cfg(windows)]
pub async fn cmd_bitmap_diag(drive: char, show_samples: bool) -> Result<()> {
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

// ============================================================================
// Save/Load Raw MFT Commands
// ============================================================================

/// Save MFT bytes to a file for offline analysis.
#[cfg(windows)]
pub async fn cmd_save(
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

/// Converts a byte to a printable ASCII character or '.' for non-printable.
#[cfg(windows)]
pub async fn cmd_benchmark_index(drive: char) -> Result<()> {
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
    // Print volume information using the historical layout
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
        .with_context(|| format!("Failed to open drive {}:", drive_upper))?
        .with_mode(MftReadMode::Auto);

    let df = reader
        .read_all()
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
    // Print index statistics using the historical layout
    // =========================================================================
    println!("=== Index Statistics ===");
    println!("Records Processed: {}", mft_capacity);
    println!("Files: {}", files_count);
    println!("Directories: {}", dirs_count);
    println!("Total Entries: {}", total_entries);
    println!();

    // =========================================================================
    // Print benchmark results using the historical layout
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
    // Print summary using the historical layout
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
pub async fn cmd_benchmark_index_lean(
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

    let (index, benchmark) = reader
        .read_all_index_with_timing()
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
    // Print phase timing breakdown for reference-benchmark comparison
    // =========================================================================
    println!("=== Phase Timing Breakdown ===");
    println!("Open/Metadata:    {:>6} ms", benchmark.timings.open_ms);
    println!(
        "I/O (read):       {:>6} ms  ✓ accurate",
        benchmark.timings.read_ms
    );
    println!(
        "Parse:            {:>6} ms  ✓ accurate",
        benchmark.timings.parse_ms
    );
    println!(
        "Merge:            {:>6} ms  ✓ accurate",
        benchmark.timings.merge_ms
    );
    println!(
        "Index Build:      {:>6} ms  (record insertion + ext index + sort)",
        benchmark.timings.index_build_ms
    );
    println!(
        "Tree Metrics:     {:>6} ms  (reference 'preprocessing' equivalent)",
        benchmark.timings.tree_metrics_ms
    );
    println!("─────────────────────────────────────────");
    println!("Total:            {:>6} ms", benchmark.timings.total_ms);
    println!();

    // Show I/O + Parse + Merge subtotal for reference-benchmark comparison
    let io_parse_merge_ms =
        benchmark.timings.read_ms + benchmark.timings.parse_ms + benchmark.timings.merge_ms;
    println!("=== Reference Benchmark Comparison ===");
    println!(
        "I/O + Parse + Merge:  {:>6} ms  (compare to reference 'Read + Parse')",
        io_parse_merge_ms
    );
    println!(
        "Tree Metrics:         {:>6} ms  (compare to reference 'Preprocess')",
        benchmark.timings.tree_metrics_ms
    );
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
    // Print reference-benchmark comparison guide
    // =========================================================================
    println!("=== Reference Benchmark Guide ===");
    println!("To compare with the reference uffs.com binary:");
    println!("  uffs.com --benchmark-mft={}:   Raw I/O only", drive_upper);
    println!(
        "  uffs.com --benchmark-index={}: I/O + Parse + Preprocess",
        drive_upper
    );
    println!();
    println!("Rust equivalent phases:");
    println!(
        "  I/O + Parse + Merge = {} ms",
        benchmark.timings.read_ms + benchmark.timings.parse_ms + benchmark.timings.merge_ms
    );
    println!(
        "  Tree Metrics (Preprocess) = {} ms",
        benchmark.timings.tree_metrics_ms
    );
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

/// Benchmark tree metrics computation in isolation.
///
/// This measures ONLY the tree metrics phase (descendants, treesize,
/// tree_allocated), which corresponds to the reference "preprocessing" phase.
/// Use this for direct apples-to-apples comparison of tree algorithm
/// performance.
#[cfg(windows)]
pub async fn cmd_benchmark_tree(drive: char, iterations: usize, no_cache: bool) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::cache::{INDEX_TTL_SECONDS, load_cached_index};

    let drive_upper = drive.to_ascii_uppercase();

    println!("=== Tree Metrics Benchmark ===");
    println!("Drive: {}:", drive_upper);
    println!("Iterations: {}", iterations);
    println!("Cache: {}", if no_cache { "disabled" } else { "enabled" });
    println!();
    println!("This measures ONLY tree metrics computation (reference 'preprocessing' equivalent).");
    println!();

    // Load or build the index
    let load_start = Instant::now();
    let mut index = if no_cache {
        println!("Building fresh index from disk...");
        let reader = MftReader::open(drive_upper)
            .with_context(|| format!("Failed to open drive {}:", drive_upper))?;
        reader
            .read_all_index()
            .await
            .with_context(|| format!("Failed to read MFT from {}:", drive_upper))?
    } else {
        println!("Loading index from cache...");
        match load_cached_index(drive_upper, INDEX_TTL_SECONDS) {
            Some((cached, _header)) => cached,
            None => {
                println!("Cache miss - building fresh index...");
                let reader = MftReader::open(drive_upper)
                    .with_context(|| format!("Failed to open drive {}:", drive_upper))?;
                reader
                    .read_all_index()
                    .await
                    .with_context(|| format!("Failed to read MFT from {}:", drive_upper))?
            }
        }
    };
    let load_ms = load_start.elapsed().as_millis() as u64;
    println!("Index loaded in {} ms", load_ms);
    println!();

    // Get index stats
    let total_entries = index.records.len();
    let dirs_count = index.records.iter().filter(|r| r.is_directory()).count();
    let files_count = total_entries.saturating_sub(dirs_count);

    println!("=== Index Statistics ===");
    println!("Total Entries: {}", total_entries);
    println!("Files: {}", files_count);
    println!("Directories: {}", dirs_count);
    println!();

    // Run tree metrics computation multiple times
    println!("=== Running {} iterations ===", iterations);
    let mut times_ms: Vec<u64> = Vec::with_capacity(iterations);

    for i in 0..iterations {
        // Clear tree metrics before each run
        for record in &mut index.records {
            record.descendants = 0;
            record.treesize = 0;
            record.tree_allocated = 0;
        }

        // Time the tree metrics computation
        let tree_start = Instant::now();
        index.compute_tree_metrics();
        let tree_ms = tree_start.elapsed().as_millis() as u64;
        times_ms.push(tree_ms);

        println!("  Iteration {}: {} ms", i + 1, tree_ms);
    }

    // Calculate statistics
    let min_ms = *times_ms.iter().min().unwrap_or(&0);
    let max_ms = *times_ms.iter().max().unwrap_or(&0);
    let sum_ms: u64 = times_ms.iter().sum();
    let avg_ms = if iterations > 0 {
        sum_ms / iterations as u64
    } else {
        0
    };

    // Calculate median
    let mut sorted = times_ms.clone();
    sorted.sort_unstable();
    let median_ms = if iterations > 0 {
        if iterations % 2 == 0 {
            (sorted[iterations / 2 - 1] + sorted[iterations / 2]) / 2
        } else {
            sorted[iterations / 2]
        }
    } else {
        0
    };

    println!();
    println!("=== Tree Metrics Timing Results ===");
    println!("Min:    {:>6} ms", min_ms);
    println!("Max:    {:>6} ms", max_ms);
    println!("Avg:    {:>6} ms", avg_ms);
    println!("Median: {:>6} ms", median_ms);
    println!();

    // Calculate throughput
    let entries_per_sec = if avg_ms > 0 {
        (total_entries as u64 * 1000) / avg_ms
    } else {
        0
    };

    println!("=== Throughput ===");
    println!("Entries processed: {}", total_entries);
    println!("Throughput: {} entries/sec", entries_per_sec);
    println!();

    // Reference benchmark guide
    println!("=== Reference Benchmark Guide ===");
    println!("To compare with the reference uffs.com binary:");
    println!("  1. Run: uffs.com --benchmark-index={}:", drive_upper);
    println!("  2. Look for the 'Preprocess' phase timing");
    println!("  3. Compare with Rust 'Tree Metrics' timing above");
    println!();
    println!("Note: the reference 'Preprocess' phase includes the same tree metrics computation:");
    println!("  - descendants (recursive child count)");
    println!("  - treesize (recursive file count per stream)");
    println!("  - tree_allocated (recursive allocated size)");

    Ok(())
}

/// Benchmark multi-volume indexing using single IOCP (M4 optimization).
#[cfg(windows)]
pub async fn cmd_benchmark_multi_volume(drives: Vec<char>) -> Result<()> {
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
        #[expect(unsafe_code, reason = "required for windows ffi call to CloseHandle")]
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
pub async fn cmd_usn_info(drive: char) -> Result<()> {
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
pub async fn cmd_usn_read(drive: char, start_usn: Option<i64>, limit: usize) -> Result<()> {
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

/// Save index to disk for incremental updates.
#[cfg(windows)]
pub async fn cmd_index_save(drive: char, output: &Path) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::usn::query_usn_journal;
    use uffs_mft::{MftReader, VolumeHandle};

    println!("📦 Building and saving index for {}:...", drive);
    println!();

    let start = Instant::now();

    // Build the index
    let reader = MftReader::open(drive)?;
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
pub async fn cmd_index_load(input: &Path) -> Result<()> {
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
pub async fn cmd_cache_status(clean: bool, purge: bool) -> Result<()> {
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
pub async fn cmd_cache_get(drive: char, force: bool, ttl: Option<u64>) -> Result<()> {
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
    let reader = MftReader::open(drive)?;
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
pub async fn cmd_cache_clear(drive: Option<char>, all: bool) -> Result<()> {
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
pub async fn cmd_index_update(drive: char, force_full: bool, ttl: Option<u64>) -> Result<()> {
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

    let reader = MftReader::open(drive)?;
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
pub async fn cmd_index_all(drives: Option<Vec<char>>, no_cache: bool, ttl: u64) -> Result<()> {
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
        reader.read_all_index_cached(0).await? // TTL=0 forces rebuild but still
    // saves
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
