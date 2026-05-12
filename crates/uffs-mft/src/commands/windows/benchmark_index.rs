// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Index benchmark command handlers.
//!
//! These commands print human-readable benchmark output to stdout, build
//! throughput rates from `u64` counters into `f64` for MB/s and rec/s, and
//! occasionally use `Debug` formatting for opaque enum values like
//! [`MftReadMode`].  The lint exemptions below capture those domain-specific
//! patterns rather than disabling the lints globally.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "intentional user-facing CLI benchmark output: stdout for primary results, stderr for per-volume failure diagnostics"
)]
#![expect(
    clippy::use_debug,
    reason = "Debug formatting is the canonical display for opaque diagnostic enums (DriveType, MftReadMode) in CLI tools"
)]
#![expect(
    clippy::float_arithmetic,
    clippy::default_numeric_fallback,
    reason = "throughput rates (MB/s, rec/s) divide f64 helpers for human-readable display"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "short identifiers (c, w, e) used for printf-style column-width and error bindings in CLI output"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "CLI prints index into known-shape benchmark snapshots; bounds are guaranteed by upstream invariants"
)]
#![expect(
    clippy::too_many_lines,
    reason = "benchmark commands are inherently linear: configure → run → format → print, kept in one place for readability"
)]

use anyhow::{Context as _, Result};
use tracing::warn;
use uffs_mft::{MftReader, bytes_to_mb_f64, f64_to_u64, millis_to_u64, u64_to_f64, usize_to_u64};

/// Converts a byte to a printable ASCII character or '.' for non-printable.
#[cfg(windows)]
pub(crate) async fn cmd_benchmark_index(drive: char) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::VolumeHandle;
    use uffs_mft::{MftReadMode, MftReader};

    let drive_upper = drive.to_ascii_uppercase();

    println!("=== Index Build Benchmark Tool ===");
    println!("Drive: {drive_upper}:");
    println!(
        "This measures the full UFFS indexing pipeline (async I/O + parsing + DataFrame building)"
    );
    println!();

    // Get volume info via VolumeHandle
    let handle = VolumeHandle::open(drive_upper)
        .with_context(|| format!("Failed to open volume {drive_upper}:"))?;
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
    println!("MFT Capacity: {mft_capacity} records");
    println!("MFT Record Size: {record_size} bytes");
    println!("MFT Total Size: {mft_size} bytes ({mft_size_mb} MB)");
    println!();

    println!("Creating index for {drive_upper}:\\ ...");
    println!("Indexing in progress...");
    println!();

    // =========================================================================
    // Run the full indexing pipeline with timing
    // =========================================================================
    let start_time = Instant::now();

    // Open reader and read MFT
    let reader = MftReader::open(drive_upper)
        .with_context(|| format!("Failed to open drive {drive_upper}:"))?
        .with_mode(MftReadMode::Auto);

    let df = reader
        .read_all()
        .with_context(|| format!("Failed to read MFT from {drive_upper}:"))?;

    let elapsed = start_time.elapsed();
    let elapsed_ms = millis_to_u64(elapsed.as_millis());
    let elapsed_secs = elapsed.as_secs_f64();

    // =========================================================================
    // Calculate statistics from DataFrame
    // =========================================================================
    let total_entries = usize_to_u64(df.height());

    // Count files vs directories using the is_directory column
    let is_dir_col = df.column("is_directory").ok().and_then(|c| c.bool().ok());

    let (files_count, dirs_count) = is_dir_col.map_or((total_entries, 0), |col| {
        let dirs = usize_to_u64(col.into_iter().filter(|v| v.unwrap_or(false)).count());
        let files = total_entries.saturating_sub(dirs);
        (files, dirs)
    });

    // =========================================================================
    // Print index statistics using the historical layout
    // =========================================================================
    println!("=== Index Statistics ===");
    println!("Records Processed: {mft_capacity}");
    println!("Files: {files_count}");
    println!("Directories: {dirs_count}");
    println!("Total Entries: {total_entries}");
    println!();

    // =========================================================================
    // Print benchmark results using the historical layout
    // =========================================================================
    let mft_read_speed = if elapsed_secs > 0.0 {
        bytes_to_mb_f64(mft_size) / elapsed_secs
    } else {
        0.0
    };

    let records_per_sec = if elapsed_secs > 0.0 {
        f64_to_u64(u64_to_f64(mft_capacity) / elapsed_secs)
    } else {
        0
    };

    let entries_per_sec = if elapsed_secs > 0.0 {
        f64_to_u64(u64_to_f64(total_entries) / elapsed_secs)
    } else {
        0
    };

    println!("=== Benchmark Results ===");
    println!("Time Elapsed: {elapsed_ms} ms ({elapsed_secs:.3} seconds)");
    println!("MFT Read Speed: {mft_read_speed:.2} MB/s");
    println!("Record Processing: {records_per_sec} records/sec");
    println!("File Indexing: {entries_per_sec} files+dirs/sec");
    println!();

    // =========================================================================
    // Print summary using the historical layout
    // =========================================================================
    println!("=== Summary ===");
    println!("Indexed {total_entries} items in {elapsed_secs:.3} seconds");

    Ok(())
}

// ============================================================================
// Lean Index Build Benchmark Command (no DataFrame overhead)
// ============================================================================

/// Tuning flags forwarded from the CLI into [`cmd_benchmark_index_lean`].
///
/// Grouping the six toggles into a struct keeps the public function under
/// the seven-argument cap without losing the flag-per-CLI-arg mapping.
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct BenchmarkIndexLeanOptions {
    /// `--no-bitmap` — disable bitmap-based skip optimisation.
    pub no_bitmap: bool,
    /// `--no-placeholders` — disable placeholder synthesis for skipped FRSes.
    pub no_placeholders: bool,
    /// `--concurrency N` — explicit IOCP concurrency override.
    pub concurrency: Option<usize>,
    /// `--io-size-kb N` — explicit per-read chunk size override (KB).
    pub io_size_kb: Option<usize>,
    /// `--parallel-parse` — enable Rayon-based parallel parsing.
    pub parallel_parse: bool,
    /// `--parse-workers N` — Rayon worker count when `parallel_parse` is on.
    pub parse_workers: Option<usize>,
}

/// Lean index build benchmark - uses `MftIndex` instead of `DataFrame`.
///
/// This measures the UFFS indexing pipeline without `DataFrame` building
/// overhead. Should be ~2x faster than `benchmark-index` on large drives.
#[cfg(windows)]
pub(crate) async fn cmd_benchmark_index_lean(
    drive: char,
    mode_str: &str,
    opts: BenchmarkIndexLeanOptions,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::platform::VolumeHandle;
    use uffs_mft::{MftReadMode, MftReader};

    let BenchmarkIndexLeanOptions {
        no_bitmap,
        no_placeholders,
        concurrency,
        io_size_kb,
        parallel_parse,
        parse_workers,
    } = opts;

    let drive_upper = drive.to_ascii_uppercase();

    // Parse read mode
    let mode: MftReadMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;

    // Get drive type for adaptive defaults display
    let drive_type = uffs_mft::platform::detect_drive_type(drive_upper);
    let effective_io_size_kb = io_size_kb.unwrap_or_else(|| drive_type.optimal_io_size() / 1024);

    println!("=== Lean Index Build Benchmark Tool ===");
    println!("Drive: {drive_upper}:");
    println!("Drive Type: {drive_type:?}");
    println!("Mode: {mode}");
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
        println!("Concurrency: {c} I/O ops in flight");
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
            parse_workers.map_or_else(|| "auto".to_owned(), |w| w.to_string())
        );
    } else {
        println!("Parallel Parse: disabled");
    }
    println!("This measures the UFFS indexing pipeline with lean MftIndex (no DataFrame overhead)");
    println!();

    // Get volume info via VolumeHandle
    let handle = VolumeHandle::open(drive_upper)
        .with_context(|| format!("Failed to open volume {drive_upper}:"))?;
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
    println!("MFT Capacity: {mft_capacity} records");
    println!("MFT Record Size: {record_size} bytes");
    println!("MFT Total Size: {mft_size} bytes ({mft_size_mb} MB)");
    println!();

    println!("Creating lean index for {drive_upper}:\\ ...");
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
        .with_context(|| format!("Failed to open drive {drive_upper}:"))?
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
        .with_context(|| format!("Failed to read MFT from {drive_upper}:"))?;

    let elapsed = start_time.elapsed();
    let elapsed_ms = millis_to_u64(elapsed.as_millis());
    let elapsed_secs = elapsed.as_secs_f64();

    // =========================================================================
    // Calculate statistics from MftIndex
    // =========================================================================
    let total_entries = usize_to_u64(index.records.len());

    // Count files vs directories
    let dirs_count = usize_to_u64(index.records.iter().filter(|r| r.is_directory()).count());
    let files_count = total_entries.saturating_sub(dirs_count);

    // =========================================================================
    // Print Index Statistics
    // =========================================================================
    println!("=== Index Statistics ===");
    println!("Records Processed: {mft_capacity}");
    println!("Files: {files_count}");
    println!("Directories: {dirs_count}");
    println!("Total Entries: {total_entries}");
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
        "I/O + Parse + Merge:  {io_parse_merge_ms:>6} ms  (compare to reference 'Read + Parse')"
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
        bytes_to_mb_f64(mft_size) / elapsed_secs
    } else {
        0.0
    };

    let records_per_sec = if elapsed_secs > 0.0 {
        f64_to_u64(u64_to_f64(mft_capacity) / elapsed_secs)
    } else {
        0
    };

    let entries_per_sec = if elapsed_secs > 0.0 {
        f64_to_u64(u64_to_f64(total_entries) / elapsed_secs)
    } else {
        0
    };

    println!("=== Benchmark Results ===");
    println!("Time Elapsed: {elapsed_ms} ms ({elapsed_secs:.3} seconds)");
    println!("MFT Read Speed: {mft_read_speed:.2} MB/s");
    println!("Record Processing: {records_per_sec} records/sec");
    println!("File Indexing: {entries_per_sec} files+dirs/sec");
    println!();

    // =========================================================================
    // Print reference-benchmark comparison guide
    // =========================================================================
    println!("=== Reference Benchmark Guide ===");
    println!("To compare with the reference uffs.com binary:");
    println!("  uffs.com --benchmark-mft={drive_upper}:   Raw I/O only");
    println!("  uffs.com --benchmark-index={drive_upper}: I/O + Parse + Preprocess");
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
        "Indexed {total_entries} items in {elapsed_secs:.3} seconds (lean index, mode: {mode})"
    );

    Ok(())
}

/// Benchmark tree metrics computation in isolation.
///
/// This measures ONLY the tree metrics phase (descendants, treesize,
/// `tree_allocated`), which corresponds to the reference "preprocessing" phase.
/// Use this for direct apples-to-apples comparison of tree algorithm
/// performance.
#[cfg(windows)]
pub(crate) async fn cmd_benchmark_tree(
    drive: char,
    iterations: usize,
    no_cache: bool,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::cache::{INDEX_TTL_SECONDS, load_cached_index};

    let drive_upper = drive.to_ascii_uppercase();

    println!("=== Tree Metrics Benchmark ===");
    println!("Drive: {drive_upper}:");
    println!("Iterations: {iterations}");
    println!("Cache: {}", if no_cache { "disabled" } else { "enabled" });
    println!();
    println!("This measures ONLY tree metrics computation (reference 'preprocessing' equivalent).");
    println!();

    // Load or build the index
    let load_start = Instant::now();
    let mut index = if no_cache {
        println!("Building fresh index from disk...");
        let reader = MftReader::open(drive_upper)
            .with_context(|| format!("Failed to open drive {drive_upper}:"))?;
        reader
            .read_all_index()
            .await
            .with_context(|| format!("Failed to read MFT from {drive_upper}:"))?
    } else {
        println!("Loading index from cache...");
        if let Some((cached, _header)) = load_cached_index(drive_upper, INDEX_TTL_SECONDS) {
            cached
        } else {
            println!("Cache miss - building fresh index...");
            let reader = MftReader::open(drive_upper)
                .with_context(|| format!("Failed to open drive {drive_upper}:"))?;
            reader
                .read_all_index()
                .await
                .with_context(|| format!("Failed to read MFT from {drive_upper}:"))?
        }
    };
    let load_ms = millis_to_u64(load_start.elapsed().as_millis());
    println!("Index loaded in {load_ms} ms");
    println!();

    // Get index stats
    let total_entries = index.records.len();
    let dirs_count = index.records.iter().filter(|r| r.is_directory()).count();
    let files_count = total_entries.saturating_sub(dirs_count);

    println!("=== Index Statistics ===");
    println!("Total Entries: {total_entries}");
    println!("Files: {files_count}");
    println!("Directories: {dirs_count}");
    println!();

    // Run tree metrics computation multiple times
    println!("=== Running {iterations} iterations ===");
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
        let tree_ms = millis_to_u64(tree_start.elapsed().as_millis());
        times_ms.push(tree_ms);

        println!("  Iteration {}: {} ms", i + 1, tree_ms);
    }

    // Calculate statistics
    let min_ms = *times_ms.iter().min().unwrap_or(&0);
    let max_ms = *times_ms.iter().max().unwrap_or(&0);
    let sum_ms: u64 = times_ms.iter().sum();
    let avg_ms = if iterations > 0 {
        sum_ms / usize_to_u64(iterations)
    } else {
        0
    };

    // Calculate median
    let mut sorted = times_ms.clone();
    sorted.sort_unstable();
    let median_ms = if iterations > 0 {
        if iterations.is_multiple_of(2) {
            u64::midpoint(sorted[iterations / 2 - 1], sorted[iterations / 2])
        } else {
            sorted[iterations / 2]
        }
    } else {
        0
    };

    println!();
    println!("=== Tree Metrics Timing Results ===");
    println!("Min:    {min_ms:>6} ms");
    println!("Max:    {max_ms:>6} ms");
    println!("Avg:    {avg_ms:>6} ms");
    println!("Median: {median_ms:>6} ms");
    println!();

    // Calculate throughput
    let entries_per_sec = (usize_to_u64(total_entries) * 1000)
        .checked_div(avg_ms)
        .unwrap_or(0);

    println!("=== Throughput ===");
    println!("Entries processed: {total_entries}");
    println!("Throughput: {entries_per_sec} entries/sec");
    println!();

    // Reference benchmark guide
    println!("=== Reference Benchmark Guide ===");
    println!("To compare with the reference uffs.com binary:");
    println!("  1. Run: uffs.com --benchmark-index={drive_upper}:");
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
pub(crate) async fn cmd_benchmark_multi_volume(drives: Vec<char>) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::io::{MultiVolumeIocpReader, prepare_volume_state};
    use uffs_mft::platform::{MftExtent, VolumeHandle, detect_drive_type};

    if drives.is_empty() {
        anyhow::bail!("No drives specified. Use --drives C,D,S");
    }

    let upper_drives: Vec<char> = drives.iter().map(char::to_ascii_uppercase).collect();

    println!("=== Multi-Volume IOCP Benchmark (M4 Optimization) ===");
    println!("Drives: {upper_drives:?}");
    println!();

    // Prepare volume states
    let mut volume_states = Vec::new();
    let start_time = Instant::now();

    for &drive in &upper_drives {
        println!("📂 Preparing volume {drive}:...");

        // Open volume handle
        let handle = match VolumeHandle::open(drive) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("  ❌ Failed to open {drive}: {e}");
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
                lcn: volume_data.mft_start_lcn.cast_signed(),
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
                eprintln!("  ❌ Failed to open overlapped handle for {drive}: {e}");
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
            bytes_to_mb_f64(mft_size)
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
        {
            // SAFETY: each `handle` was returned by `open_overlapped_handle` above
            // and not used after this point; `CloseHandle` is the documented
            // counterpart for `CreateFileW`-derived volume handles.
            _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
    }

    let total_elapsed = start_time.elapsed();

    // Print results
    println!();
    println!("=== Results ===");

    let mut total_records = 0_u64;
    let mut total_files = 0_u64;
    let mut total_dirs = 0_u64;

    for index in &indices {
        let files = index.records.iter().filter(|r| !r.is_directory()).count();
        let dirs = index.records.iter().filter(|r| r.is_directory()).count();
        total_records += usize_to_u64(index.len());
        total_files += usize_to_u64(files);
        total_dirs += usize_to_u64(dirs);

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
