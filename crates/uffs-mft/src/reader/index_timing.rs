//! Benchmark-oriented lean-index timing entrypoints.

#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use tracing::{info, warn};

#[cfg(windows)]
use super::PhaseTimings;
#[cfg(windows)]
use super::benchmark::{
    build_benchmark_result, build_drive_characteristics, estimate_combined_phase_timings,
};
use super::{BenchmarkResult, MftReader};
use crate::error::{MftError, Result};
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
    /// A tuple of (MftIndex, BenchmarkResult) with the index and timing
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
            let reader = MftReader {
                volume,
                handle,
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
        .map_err(|e| MftError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))??;

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
    /// C++ "preprocessing" in `--benchmark-index`.
    ///
    /// # Returns
    ///
    /// A tuple of (MftIndex, BenchmarkResult) with detailed timing breakdown.
    #[cfg(windows)]
    #[expect(
        clippy::too_many_lines,
        reason = "sequential I/O pipeline with per-phase timing cannot be meaningfully split"
    )]
    fn read_mft_index_with_timing_internal(
        &self,
    ) -> Result<(crate::index::MftIndex, BenchmarkResult)> {
        use crate::index::MftIndex;
        use crate::io::{MftExtentMap, ParallelMftReader, generate_read_chunks};
        use crate::platform::detect_drive_type;

        let total_start = Instant::now();

        // Phase 1: Open (already done, but measure metadata retrieval)
        let open_start = Instant::now();
        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();
        let drive_type = detect_drive_type(self.volume);
        let chunk_size = drive_type.optimal_chunk_size();

        // Get MFT extents
        let extents = self.handle.get_mft_extents().unwrap_or_else(|e| {
            warn!(error = ?e, "Failed to get MFT extents, using fallback");
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        let extent_map =
            MftExtentMap::new(extents.clone(), volume_data.bytes_per_cluster, record_size);
        let total_records = extent_map.total_records();
        let mft_size_bytes = total_records * u64::from(record_size);

        // Get bitmap
        let bitmap = if self.use_bitmap {
            self.handle.get_mft_bitmap().ok()
        } else {
            None
        };
        let in_use_records = bitmap.as_ref().map(|bm| bm.count_in_use() as u64);

        // Generate chunks to get count
        let chunks = generate_read_chunks(&extent_map, bitmap.as_ref(), chunk_size);
        let chunk_count = chunks.len();

        let open_ms = open_start.elapsed().as_millis() as u64;

        // Build characteristics
        let characteristics = build_drive_characteristics(
            self.volume,
            drive_type,
            mft_size_bytes,
            total_records,
            in_use_records,
            extents.len(),
            record_size,
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

        // Phase 2+3: Read + Parse with accurate timing
        let parallel_reader = ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);
        let handle = self.handle.raw_handle();

        // Use the new timing method for accurate phase breakdown
        let (mut parsed_records, read_parse_timing) =
            parallel_reader.read_all_parallel_with_timing(handle, self.merge_extensions)?;

        // Add placeholder records for missing parent directories
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

        // Use accurate timing from instrumented reader (not estimates!)
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

        let total_ms = total_start.elapsed().as_millis() as u64;

        // Calculate throughput
        let total_secs = total_ms as f64 / 1000.0;
        let throughput_mb_s = if total_secs > 0.0 {
            (mft_size_bytes as f64 / (1024.0 * 1024.0)) / total_secs
        } else {
            0.0
        };
        let records_per_sec = if total_secs > 0.0 {
            records_parsed as f64 / total_secs
        } else {
            0.0
        };

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
}
