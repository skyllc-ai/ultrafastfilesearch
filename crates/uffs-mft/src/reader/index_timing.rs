// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Benchmark-oriented lean-index timing entrypoints.

#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use tracing::{debug, info, warn};

#[cfg(windows)]
use super::PhaseTimings;
#[cfg(windows)]
use super::benchmark::{MftMetrics, build_benchmark_result, build_drive_characteristics};
use super::{BenchmarkResult, MftReader};
use crate::error::{MftError, Result};
#[cfg(windows)]
use crate::index::{u64_to_f64, usize_to_f64};
#[cfg(windows)]
use crate::platform::VolumeHandle;

impl MftReader {
    /// Read MFT into lean index with detailed timing breakdown.
    ///
    /// This is the benchmarking version of `read_all_index()` that returns
    /// detailed timing for each phase, including tree metrics computation.
    ///
    /// # Returns
    ///
    /// A tuple of (`MftIndex`, `BenchmarkResult`) with the index and timing
    /// breakdown.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub async fn read_all_index_with_timing(
        &self,
    ) -> Result<(crate::index::MftIndex, BenchmarkResult)> {
        // Capture configuration to recreate reader in blocking thread
        let volume = self.volume;
        let mode = self.mode;
        let merge_extensions = self.merge_extensions;
        let use_bitmap = self.use_bitmap;
        let expand_links = self.expand_links;
        let add_placeholders = self.add_placeholders;
        let concurrency = self.concurrency;
        let io_size = self.io_size;
        let parallel_parse = self.parallel_parse;
        let parse_workers = self.parse_workers;
        let forensic = self.forensic;

        let result = tokio::task::spawn_blocking(move || {
            // Create a new reader in the blocking thread
            let handle = VolumeHandle::open(volume)?;
            let reader = Self {
                volume,
                source: super::MftSource::LiveVolume(Box::new(handle)),
                mode,
                merge_extensions,
                use_bitmap,
                expand_links,
                add_placeholders,
                concurrency,
                io_size,
                parallel_parse,
                parse_workers,
                forensic,
            };
            reader.read_mft_index_with_timing_internal()
        })
        .await
        .map_err(|error| MftError::from_join_error("read_all_index_with_timing", &error))??;

        Ok(result)
    }

    /// Read MFT into lean index with timing (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_all_index_with_timing(
        &self,
    ) -> Result<(crate::index::MftIndex, BenchmarkResult)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for MFT lean index reading with detailed phase
    /// timing.
    ///
    /// This method measures each phase separately for benchmarking purposes,
    /// including the tree metrics computation phase which corresponds to
    /// "preprocessing" phase in `--benchmark-index`.
    ///
    /// # Returns
    ///
    /// A tuple of (`MftIndex`, `BenchmarkResult`) with detailed timing
    /// breakdown.
    #[cfg(windows)]
    fn read_mft_index_with_timing_internal(
        &self,
    ) -> Result<(crate::index::MftIndex, BenchmarkResult)> {
        use crate::index::MftIndex;

        let total_start = Instant::now();

        let phase1 = self.benchmark_index_phase1_open()?;
        let IndexPhase1Snapshot {
            extent_map,
            bitmap,
            drive_type,
            characteristics,
            mft_size_bytes,
            open_ms,
        } = phase1;

        let phase23 = self.benchmark_index_phase23_read_parse(extent_map, bitmap, drive_type)?;
        let IndexReadParseSnapshot {
            parsed_records,
            read_ms,
            parse_ms,
            merge_ms,
            records_parsed,
        } = phase23;

        // Phase 4: Build MftIndex with timing breakdown
        let (index, index_timing) =
            MftIndex::from_parsed_records_with_timing(self.volume, parsed_records);

        info!(
            records = index.records.len(),
            names_buffer_kb = index.names.len() / 1024,
            record_insert_ms = index_timing.record_insert_ms,
            extension_index_ms = index_timing.extension_index_ms,
            sort_children_ms = index_timing.sort_children_ms,
            tree_metrics_ms = index_timing.tree_metrics_ms,
            index_total_ms = index_timing.total_ms,
            "📊 Benchmark (lean index): Index build complete"
        );

        let total_ms = u64::try_from(total_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        let (throughput_mb_s, records_per_sec) =
            compute_index_benchmark_throughput(total_ms, mft_size_bytes, records_parsed);

        let timings = PhaseTimings {
            open_ms,
            read_ms,
            parse_ms,
            merge_ms,
            df_build_ms: 0, // Not applicable for MftIndex path
            index_build_ms: index_timing.index_only_ms(),
            tree_metrics_ms: index_timing.tree_metrics_ms,
            total_ms,
        };

        let result = build_benchmark_result(
            timings,
            characteristics,
            records_parsed,
            throughput_mb_s,
            records_per_sec,
        );

        info!(
            total_ms,
            index_build_ms = index_timing.index_only_ms(),
            tree_metrics_ms = index_timing.tree_metrics_ms,
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            records_per_sec = format!("{:.0}", records_per_sec),
            "📊 Benchmark (lean index): Complete"
        );

        Ok((index, result))
    }

    /// Phase 1 of [`Self::read_mft_index_with_timing_internal`].
    ///
    /// Mirrors the layout of [`Self::benchmark_phase1_open`] used by the
    /// `DataFrame` benchmark, but honours `self.use_bitmap` so callers can
    /// disable the bitmap-skip optimisation for apples-to-apples
    /// comparisons.
    #[cfg(windows)]
    fn benchmark_index_phase1_open(&self) -> Result<IndexPhase1Snapshot> {
        use crate::io::{MftExtentMap, generate_read_chunks};
        use crate::platform::detect_drive_type;

        let open_start = Instant::now();
        let record_size = self.require_handle()?.file_record_size();
        let volume_data = self.require_handle()?.volume_data();
        let drive_type = detect_drive_type(self.volume);
        let chunk_size = drive_type.optimal_chunk_size();

        let extents = self
            .require_handle()?
            .get_mft_extents()
            .unwrap_or_else(|err| {
                warn!(error = ?err, "Failed to get MFT extents, using fallback");
                vec![crate::platform::MftExtent {
                    vcn: 0,
                    cluster_count: volume_data.mft_valid_data_length
                        / u64::from(volume_data.bytes_per_cluster),
                    lcn: crate::platform::Lcn::new(volume_data.mft_start_lcn.cast_signed()),
                }]
            });

        let extent_map =
            MftExtentMap::new(extents.clone(), volume_data.bytes_per_cluster, record_size);
        let total_records = extent_map.total_records();
        let mft_size_bytes = total_records * u64::from(record_size);

        let bitmap = if self.use_bitmap {
            self.require_handle()?.get_mft_bitmap().ok()
        } else {
            None
        };
        let in_use_records = bitmap
            .as_ref()
            .map(|bm| u64::try_from(bm.count_in_use()).unwrap_or(u64::MAX));

        let chunks = generate_read_chunks(&extent_map, bitmap.as_ref(), chunk_size);
        let chunk_count = chunks.len();

        let open_ms = u64::try_from(open_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        let characteristics = build_drive_characteristics(
            self.volume,
            drive_type,
            MftMetrics {
                size_bytes: mft_size_bytes,
                total_records,
                in_use_records,
                extent_count: extents.len(),
                bytes_per_record: record_size,
            },
            chunk_size,
            chunk_count,
        );

        info!(
            volume = %self.volume,
            drive_type = ?drive_type,
            total_records,
            mft_size_mb = mft_size_bytes / (1024 * 1024),
            extents = extents.len(),
            chunks = chunk_count,
            "📊 Benchmark (lean index): MFT characteristics"
        );

        Ok(IndexPhase1Snapshot {
            extent_map,
            bitmap,
            drive_type,
            characteristics,
            mft_size_bytes,
            open_ms,
        })
    }

    /// Phase 2 + 3 of [`Self::read_mft_index_with_timing_internal`].
    ///
    /// Drives [`crate::io::ParallelMftReader::read_all_parallel_with_timing`]
    /// (which records *accurate* read / parse / merge timings rather than
    /// the estimates used in the `DataFrame` path) and optionally adds
    /// parent-directory placeholders.
    #[cfg(windows)]
    fn benchmark_index_phase23_read_parse(
        &self,
        extent_map: crate::io::MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Result<IndexReadParseSnapshot> {
        use crate::io::ParallelMftReader;

        let parallel_reader = ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);
        let handle = self.require_handle()?.raw_handle();

        let (mut parsed_records, read_parse_timing) =
            parallel_reader.read_all_parallel_with_timing(handle, self.merge_extensions)?;

        if self.add_placeholders {
            let placeholders_added =
                crate::io::add_missing_parent_placeholders_to_vec(&mut parsed_records);
            if placeholders_added > 0 {
                debug!(
                    placeholders_added,
                    "Added placeholder records for path resolution"
                );
            }
        }

        let records_parsed = parsed_records.len();
        let read_ms = read_parse_timing.io_ms();
        let parse_ms = read_parse_timing.parse_ms();
        let merge_ms = read_parse_timing.merge_ms();

        info!(
            records_parsed,
            read_ms,
            parse_ms,
            merge_ms,
            wall_ms = read_parse_timing.wall_ms(),
            overlap_ratio = format!("{:.2}", read_parse_timing.overlap_ratio()),
            "📊 Benchmark (lean index): Read + Parse complete (accurate timing)"
        );

        Ok(IndexReadParseSnapshot {
            parsed_records,
            read_ms,
            parse_ms,
            merge_ms,
            records_parsed,
        })
    }
}

/// Output of [`MftReader::benchmark_index_phase1_open`].
///
/// Bundles the extent map, optional bitmap, detected drive type, and the
/// pre-computed characteristics so the benchmark orchestrator can move
/// them into phase 2+3 in a single binding.
#[cfg(windows)]
struct IndexPhase1Snapshot {
    /// MFT extent map handed off to [`crate::io::ParallelMftReader`].
    extent_map: crate::io::MftExtentMap,
    /// Optional bitmap for skip-optimised chunking (gated by
    /// `MftReader::use_bitmap`).
    bitmap: Option<crate::platform::MftBitmap>,
    /// Detected drive type (HDD/SSD/NVMe/Unknown).
    drive_type: crate::platform::DriveType,
    /// Drive characteristics used in the final benchmark report.
    characteristics: super::DriveCharacteristics,
    /// Total MFT byte size used for throughput calculation.
    mft_size_bytes: u64,
    /// Wall-clock cost of phase 1 in milliseconds.
    open_ms: u64,
}

/// Output of [`MftReader::benchmark_index_phase23_read_parse`].
///
/// Bundles the parsed records together with the (accurate) per-phase
/// timings reported by the instrumented reader.
#[cfg(windows)]
struct IndexReadParseSnapshot {
    /// Parsed MFT records ready for [`crate::index::MftIndex`] construction.
    parsed_records: Vec<crate::parse::ParsedRecord>,
    /// Accurate read-phase wall clock in milliseconds.
    read_ms: u64,
    /// Accurate parse-phase wall clock in milliseconds.
    parse_ms: u64,
    /// Accurate merge-phase wall clock in milliseconds.
    merge_ms: u64,
    /// Final number of parsed records.
    records_parsed: usize,
}

/// Compute throughput (MiB/s) and records-per-second for the lean-index
/// benchmark summary, returning `(0.0, 0.0)` if `total_ms` is zero.
#[cfg(windows)]
#[expect(
    clippy::float_arithmetic,
    reason = "benchmark telemetry: throughput / records-per-sec require float division"
)]
fn compute_index_benchmark_throughput(
    total_ms: u64,
    mft_size_bytes: u64,
    records_parsed: usize,
) -> (f64, f64) {
    let total_secs = u64_to_f64(total_ms) / 1000.0_f64;
    if total_secs <= 0.0_f64 {
        return (0.0_f64, 0.0_f64);
    }
    let throughput_mb_s = (u64_to_f64(mft_size_bytes) / (1024.0_f64 * 1024.0_f64)) / total_secs;
    let records_per_sec = usize_to_f64(records_parsed) / total_secs;
    (throughput_mb_s, records_per_sec)
}
