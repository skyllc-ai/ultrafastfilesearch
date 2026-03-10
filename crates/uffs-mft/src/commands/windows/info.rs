//! Volume information and discovery command handlers.

use anyhow::{Context, Result};
use tracing::info;

use super::shared::drive_type_label;
use crate::display::{format_bytes, format_duration, format_number_commas, truncate_string};

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
    let drive_type_str = drive_type_label(drive_type, "Unknown");
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
            let drive_type_str = drive_type_label(drive_type, "???");

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
