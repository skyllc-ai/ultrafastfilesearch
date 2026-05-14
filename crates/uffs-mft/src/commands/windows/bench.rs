// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Benchmark command handlers for live read benchmarking.
//!
//! These commands print human-readable benchmark output to stdout, build
//! throughput rates from `u64` / `u128` counters into `f64`, and use `Debug`
//! formatting for opaque diagnostic enums.  The lint exemptions below capture
//! those CLI-specific patterns; library code never inherits them.
#![expect(
    clippy::print_stdout,
    reason = "intentional user-facing CLI benchmark output"
)]
#![expect(
    clippy::float_arithmetic,
    clippy::default_numeric_fallback,
    reason = "throughput rates (MB/s, rec/s) divide f64 helpers for human-readable display"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "short identifiers aid readability in CLI driver code"
)]
#![expect(
    clippy::too_many_lines,
    reason = "benchmark commands run a configure -> run -> format -> print pipeline that is most readable inline"
)]

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use tracing::{info, warn};
use uffs_mft::{f64_to_u64, millis_to_u64, u32_as_usize, u64_to_f64, usize_to_f64, usize_to_u64};

use super::shared::pause_between_benchmark_runs;
use crate::display::format_number_commas;

/// Truncates a string to a maximum length, adding "..." if truncated.
#[cfg(windows)]
pub(crate) async fn cmd_bench(
    drive: uffs_mft::platform::DriveLetter,
    json: bool,
    no_df: bool,
    requested_runs: u32,
    mode_str: &str,
    full: bool,
) -> Result<()> {
    use uffs_mft::{BenchmarkResult, MftReadMode, MftReader};

    let runs = requested_runs.max(1);

    // Parse read mode
    let mode: MftReadMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;

    if !json {
        println!("🔬 Benchmarking MFT read on drive {drive}:");
        println!("   Runs: {runs}");
        println!("   Skip DataFrame: {no_df}");
        println!("   Mode: {mode}");
        println!("   Full (merge extensions): {full}");
        println!();
    }

    info!(
        drive = %drive,
        runs,
        skip_df = no_df,
        mode = %mode,
        full,
        "📊 Starting benchmark"
    );

    // Open the reader once (opening is fast, we don't need to re-open for each run)
    let reader = MftReader::open(drive)
        .with_context(|| format!("Failed to open drive {drive}:"))?
        .with_mode(mode)
        .with_merge_extensions(full);

    let mut results: Vec<BenchmarkResult> = Vec::with_capacity(u32_as_usize(runs));

    for run in 1..=runs {
        if !json && runs > 1 {
            println!("  Run {run}/{runs}...");
        }

        let (_, result) = reader
            .read_with_timing(no_df)
            .with_context(|| format!("Benchmark run {run} failed"))?;

        info!(
            run,
            total_ms = result.timings.total_ms,
            throughput_mb_s = format!("{:.1}", result.throughput_mb_s),
            "✅ Run complete"
        );

        results.push(result);

        // Small delay between runs to let system settle
        if run < runs {
            pause_between_benchmark_runs(run, runs).await;
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

/// Pop the first result of a single benchmark run, surfacing a helpful
/// `context` in the error message if the result vector is empty.
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

/// Average a slice of benchmark results into a single `BenchmarkResult`.
///
/// The result inherits the first run's `DriveCharacteristics`; per-phase
/// timings and throughputs are arithmetic means across all runs.
#[cfg(windows)]
fn average_results(results: &[uffs_mft::BenchmarkResult]) -> Result<uffs_mft::BenchmarkResult> {
    let Some(first) = results.first() else {
        anyhow::bail!("no benchmark results were collected");
    };
    let n = usize_to_u64(results.len());

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
        results.iter().map(|r| r.throughput_mb_s).sum::<f64>() / usize_to_f64(results.len());
    let avg_records_per_sec: f64 =
        results.iter().map(|r| r.records_per_sec).sum::<f64>() / usize_to_f64(results.len());

    Ok(uffs_mft::BenchmarkResult {
        timings: avg_timings,
        characteristics: first.characteristics.clone(),
        records_parsed: first.records_parsed,
        throughput_mb_s: avg_throughput,
        records_per_sec: avg_records_per_sec,
    })
}

/// Print a single (already averaged) benchmark result to stdout in the
/// canonical CLI tabular format.  `runs` is shown verbatim in the header.
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
        let skip_pct = (u64_to_f64(in_use) / u64_to_f64(c.total_records)).mul_add(-100.0, 100.0);
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
        format_number_commas(f64_to_u64(result.records_per_sec))
    );
    println!("   MB/sec:           {:.1}", result.throughput_mb_s);
    println!(
        "   Records Parsed:   {}",
        format_number_commas(usize_to_u64(result.records_parsed))
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
    /// Render this report as a UTF-8 JSON document with one entry per
    /// benchmarked drive plus the system-level fields.
    fn to_json(&self) -> String {
        let drives_json: Vec<String> = self
            .drives
            .iter()
            .map(uffs_mft::BenchmarkResult::to_json)
            .collect();
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

/// `bench-all` CLI command — run the benchmark suite on every detected
/// NTFS drive `requested_runs` times, optionally writing the resulting
/// JSON report to `output`.
#[cfg(windows)]
pub(crate) async fn cmd_bench_all(
    output: Option<PathBuf>,
    no_df: bool,
    requested_runs: u32,
    full: bool,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::detect_ntfs_drives;

    let total_start = Instant::now();
    let runs = requested_runs.max(1);

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
            .map(|d| format!("{d}:"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("📊 Runs per drive: {runs}");
    println!("📄 Output file: {}", output_path.display());
    println!("⏳ Skip DataFrame: {no_df}");
    println!("🔗 Full (merge extensions): {full}");
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
                println!("  ✅ Drive {drive}:");
                println!(
                    "     Records:     {}",
                    format_number_commas(usize_to_u64(result.records_parsed))
                );
                println!("     Total time:  {} ms", result.timings.total_ms);
                println!("     Throughput:  {:.1} MB/s", result.throughput_mb_s);
                println!("     Type:        {}", result.characteristics.drive_type);
                println!();
                results.push(result);
            }
            Err(e) => {
                println!("  ❌ Drive {drive}: Failed - {e}");
                println!();
                warn!(drive = %drive, error = ?e, "Benchmark failed for drive");
            }
        }
    }

    let total_time_ms = millis_to_u64(total_start.elapsed().as_millis());

    // Build full report
    let report = FullBenchmarkReport {
        timestamp: chrono::Local::now().to_rfc3339(),
        hostname: hostname::get().map_or_else(
            |_| "unknown".to_owned(),
            |h| h.to_string_lossy().to_string(),
        ),
        cpu_count: std::thread::available_parallelism().map_or(1, core::num::NonZero::get),
        uffs_version: env!("CARGO_PKG_VERSION").to_owned(),
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
        u64_to_f64(total_time_ms) / 1000.0
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

/// Run the benchmark suite against one drive `runs` times, optionally
/// skipping the `DataFrame` build when `no_df` is set.
#[cfg(windows)]
async fn benchmark_single_drive(
    drive: uffs_mft::platform::DriveLetter,
    no_df: bool,
    runs: u32,
    full: bool,
) -> Result<uffs_mft::BenchmarkResult> {
    use uffs_mft::MftReader;

    let reader = MftReader::open(drive)
        .with_context(|| format!("Failed to open drive {drive}:"))?
        .with_merge_extensions(full);

    let mut results: Vec<uffs_mft::BenchmarkResult> = Vec::with_capacity(u32_as_usize(runs));

    for run in 1..=runs {
        if runs > 1 {
            println!("     Run {run}/{runs}...");
        }

        let (_, result) = reader
            .read_with_timing(no_df)
            .with_context(|| format!("Benchmark run {run} failed"))?;

        results.push(result);

        // Small delay between runs
        if run < runs {
            pause_between_benchmark_runs(run, runs).await;
        }
    }

    // Average results
    Ok(if runs == 1 {
        take_single_benchmark_result(results, "single-drive benchmark requested one iteration")?
    } else {
        average_results(&results)?
    })
}
