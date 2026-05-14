// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Index and cache command handlers.
//!
//! USN Journal verbs (`cmd_usn_info`, `cmd_usn_read`) moved to the
//! sibling `usn.rs` module in 2026-04-21.  This file now covers
//! index save/load, cache management (status/get/clear), and the
//! USN-checkpointed incremental update / all-drive index path.
//!
//! These commands print human-readable progress to stdout, build size/rate
//! summaries from `u64` counters into `f64` for KB/MB conversion, and use
//! `Debug` formatting for opaque diagnostic enums.  The lint exemptions
//! below capture those CLI-specific patterns; library code never inherits
//! them.
#![expect(
    clippy::print_stdout,
    reason = "intentional user-facing CLI status / progress output"
)]
#![expect(
    clippy::use_debug,
    reason = "Debug formatting is the canonical display for opaque diagnostic enums in CLI tools"
)]
#![expect(
    clippy::float_arithmetic,
    reason = "byte/rate calculations divide f64 helpers for human-readable MB / MB/s display"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "short identifiers (e, a) used for printf-style error / accessor bindings in CLI output"
)]
#![expect(
    clippy::too_many_lines,
    reason = "incremental index commands run a configure → execute → format → print pipeline that is most readable inline"
)]

use std::path::Path;

use anyhow::Result;
use uffs_mft::{bytes_to_mb_f64, u64_to_f64, usize_to_u64};

use crate::display::format_number;

/// Save index to disk for incremental updates.
#[cfg(windows)]
pub(crate) async fn cmd_index_save(drive: char, output: &Path) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::usn::query_usn_journal;
    use uffs_mft::{MftReader, VolumeHandle};

    println!("📦 Building and saving index for {drive}:...");
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

    let (usn_journal_id, next_usn) = query_usn_journal(drive).map_or_else(
        |_| {
            println!("⚠️  USN Journal not available, saving without checkpoint");
            (0, 0)
        },
        |info| (info.journal_id, info.next_usn),
    );

    // Save to file
    let save_start = Instant::now();
    index.save_to_file(output, volume_serial, usn_journal_id, next_usn)?;
    let save_time = save_start.elapsed();

    let file_size = std::fs::metadata(output)?.len();
    println!(
        "✅ Saved to {}: {:.1} MB in {:.3}s",
        output.display(),
        bytes_to_mb_f64(file_size),
        save_time.as_secs_f64()
    );

    if usn_journal_id != 0 {
        println!();
        println!("📍 USN Checkpoint: {next_usn} (Journal ID: 0x{usn_journal_id:016X})");
        println!("   Use this to apply incremental updates later.");
    }

    Ok(())
}

/// Load index from disk and show info.
#[cfg(windows)]
pub(crate) async fn cmd_index_load(input: &Path) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::index::MftIndex;

    println!("📂 Loading index from {}...", input.display());
    println!();

    let start = Instant::now();
    let (index, header) = MftIndex::load_from_file(input).map_err(|e| anyhow::anyhow!("{e}"))?;
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
    println!("  File Size:        {:.1} MB", bytes_to_mb_f64(file_size));
    println!("  Load Time:        {:.3}s", load_time.as_secs_f64());
    println!(
        "  Throughput:       {:.1} MB/s",
        bytes_to_mb_f64(file_size) / load_time.as_secs_f64()
    );

    // Count files vs directories
    let files = index.records.iter().filter(|r| !r.is_directory()).count();
    let dirs = index.records.iter().filter(|r| r.is_directory()).count();
    println!();
    println!("=== Content ===");
    println!("  Files:            {files}");
    println!("  Directories:      {dirs}");

    Ok(())
}

/// Show cache status and optionally clean up.
#[cfg(windows)]
pub(crate) async fn cmd_cache_status(clean: bool, purge: bool) -> Result<()> {
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
                (format!("{secs}s"), format!("✅ Fresh ({remaining}s left)"))
            }
            Some(secs) => (format!("{secs}s"), "⚠️  Expired".to_owned()),
            None => ("?".to_owned(), "❓ Unknown".to_owned()),
        };
        println!("{:<8} {:<12} {}", format!("{}:", drive), age_str, status);
    }

    Ok(())
}

/// Get or refresh a cached index for a drive.
#[cfg(windows)]
pub(crate) async fn cmd_cache_get(drive: char, force: bool, ttl: Option<u64>) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::cache::{CacheStatus, INDEX_TTL_SECONDS, check_cache_status, save_to_cache};
    use uffs_mft::usn::query_usn_journal;
    use uffs_mft::{MftReader, VolumeHandle};

    let ttl_seconds = ttl.unwrap_or(INDEX_TTL_SECONDS);
    println!("🔍 Checking cache for {drive}:...");
    println!("⏱️  TTL: {ttl_seconds} seconds");
    println!();

    // Check cache status (unless force rebuild)
    if force {
        println!("🔄 Force rebuild requested");
    } else {
        match check_cache_status(drive, ttl_seconds) {
            CacheStatus::Fresh {
                index,
                header,
                age_seconds,
            } => {
                println!("✅ Cache HIT! Index is fresh ({age_seconds} seconds old)");
                println!();
                println!("=== Cached Index ===");
                println!("  Records:     {}", index.len());
                println!("  USN:         {}", header.next_usn);
                println!("  Journal ID:  0x{:016X}", header.usn_journal_id);

                let files = index.records.iter().filter(|r| !r.is_directory()).count();
                let dirs = index.records.iter().filter(|r| r.is_directory()).count();
                println!("  Files:       {files}");
                println!("  Directories: {dirs}");
                return Ok(());
            }
            CacheStatus::Stale { age_seconds } => {
                println!(
                    "⚠️  Cache STALE (age: {}s, TTL: {}s)",
                    age_seconds.map_or_else(|| "?".to_owned(), |secs| secs.to_string()),
                    ttl_seconds
                );
            }
            CacheStatus::Missing => {
                println!("📭 Cache MISS - no cached index found");
            }
        }
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

    let (usn_journal_id, next_usn) = query_usn_journal(drive).map_or_else(
        |_| {
            println!("⚠️  USN Journal not available");
            (0, 0)
        },
        |info| (info.journal_id, info.next_usn),
    );

    // Save to cache
    let cache_path = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn)?;
    let file_size = std::fs::metadata(&cache_path)?.len();

    println!(
        "💾 Cached to: {} ({:.1} MB)",
        cache_path.display(),
        bytes_to_mb_f64(file_size)
    );

    if usn_journal_id != 0 {
        println!("📍 USN Checkpoint: {next_usn} (Journal ID: 0x{usn_journal_id:016X})");
    }

    Ok(())
}

/// Clear cached indices.
#[cfg(windows)]
pub(crate) async fn cmd_cache_clear(drive: Option<char>, all: bool) -> Result<()> {
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
            println!("✅ Cleared cache for {d}:");
            println!("   {}", path.display());
        } else {
            println!("📭 No cached index found for {d}:");
        }
    } else {
        println!("❌ Please specify --drive C or --all");
        println!();
        println!("Examples:");
        println!("  uffs-mft cache-clear --drive C");
        println!("  uffs-mft cache-clear --all");
    }

    Ok(())
}

/// Incremental index update using USN Journal.
#[cfg(windows)]
pub(crate) async fn cmd_index_update(
    drive: char,
    force_full: bool,
    ttl: Option<u64>,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::VolumeHandle;
    use uffs_mft::cache::{CacheStatus, INDEX_TTL_SECONDS, check_cache_status, save_to_cache};
    use uffs_mft::platform::is_volume_read_only;
    use uffs_mft::usn::{aggregate_changes, query_usn_journal, read_usn_journal};

    let ttl_seconds = ttl.unwrap_or(INDEX_TTL_SECONDS);
    let start = Instant::now();

    println!("🔄 Incremental index update for {drive}:...");
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
            println!("📦 Found cached index ({age_seconds} seconds old)");
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
                    println!("⚠️  USN Journal not available: {e}");
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
                        println!("⚠️  Failed to read USN Journal: {e}");
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

            // Phase 1: deletes
            let (mut stats, frs_to_read) = updated_index.apply_usn_deletes(&changes);

            // Phase 2: targeted MFT reads for non-delete changes
            let handle = VolumeHandle::open(drive)?;
            if !frs_to_read.is_empty() {
                println!(
                    "   🎯 Reading {} targeted MFT records...",
                    frs_to_read.len()
                );
                match uffs_mft::usn::read_targeted_frs_records(
                    &handle,
                    &mut updated_index,
                    &frs_to_read,
                ) {
                    Ok(count) => {
                        stats.targeted_reads = count;
                    }
                    Err(e) => {
                        println!("   ⚠️ Targeted reads failed: {e}");
                    }
                }
            }
            let apply_time = apply_start.elapsed();

            println!(
                "   Targeted reads: {}, Deleted: {}, Skipped: {}",
                stats.targeted_reads, stats.deleted, stats.skipped
            );
            println!("   Applied in {:.3}s", apply_time.as_secs_f64());

            // Rebuild derived structures
            let had_changes = stats.deleted > 0 || stats.targeted_reads > 0;
            if had_changes {
                println!();
                println!("🔨 Rebuilding extension index...");
                updated_index.build_extension_index();

                println!("🔨 Recomputing tree metrics...");
                let tree_start = Instant::now();
                updated_index.compute_tree_metrics();
                let tree_time = tree_start.elapsed();
                println!("   Computed in {:.3}s", tree_time.as_secs_f64());
            }

            // Save updated index
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
            println!("   New USN checkpoint: {next_usn}");
            println!("   Saved to: {}", cache_path.display());
            println!("⏱️  Total time: {:.3}s", elapsed.as_secs_f64());
        }
        CacheStatus::Stale { age_seconds } => {
            println!(
                "⚠️  Cache is stale (age: {}s, TTL: {}s)",
                age_seconds.map_or_else(|| "?".to_owned(), |secs| secs.to_string()),
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
    println!("🔨 Building full index for {drive}:...");

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

    let (usn_journal_id, next_usn) = query_usn_journal(drive).map_or_else(
        |_| {
            println!("⚠️  USN Journal not available");
            (0, 0)
        },
        |info| (info.journal_id, info.next_usn),
    );

    // Save to cache
    let cache_path = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn)?;
    let file_size = std::fs::metadata(&cache_path)?.len();

    println!(
        "💾 Cached to: {} ({:.1} MB)",
        cache_path.display(),
        bytes_to_mb_f64(file_size)
    );

    if usn_journal_id != 0 {
        println!("📍 USN Checkpoint: {next_usn} (Journal ID: 0x{usn_journal_id:016X})");
    }

    let total_time = start.elapsed();
    println!();
    println!("⏱️  Total time: {:.3}s", total_time.as_secs_f64());

    Ok(())
}

/// Index ALL NTFS drives in parallel using the optimized lean index path.
#[cfg(windows)]
pub(crate) async fn cmd_index_all(
    drives: Option<Vec<char>>,
    no_cache: bool,
    ttl: u64,
) -> Result<()> {
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
            .map(|c| format!("{c}:"))
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
        println!("TTL: {ttl} seconds");
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

    let mut total_files = 0_u64;
    let mut total_dirs = 0_u64;
    let mut total_entries = 0_u64;

    for index in &indices {
        let files = usize_to_u64(index.file_count());
        let dirs = usize_to_u64(index.dir_count());
        total_files += files;
        total_dirs += dirs;
        total_entries += usize_to_u64(index.len());

        println!(
            "  {}:  {:>10} files  {:>8} dirs  {:>10} total",
            index.volume,
            format_number(files),
            format_number(dirs),
            format_number(usize_to_u64(index.len())),
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
    let entries_per_sec = u64_to_f64(total_entries) / elapsed_secs;

    println!("=== Performance ===");
    println!("Time: {elapsed_secs:.3}s");
    println!("Throughput: {entries_per_sec:.0} entries/sec");
    println!();

    Ok(())
}
