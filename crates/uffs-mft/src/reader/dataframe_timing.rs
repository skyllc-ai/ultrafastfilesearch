//! Benchmark-oriented `DataFrame` timing entrypoints.

#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use tracing::{debug, info, warn};
use uffs_polars::DataFrame;

#[cfg(windows)]
use super::PhaseTimings;
#[cfg(windows)]
use super::benchmark::{
    build_benchmark_result, build_drive_characteristics, estimate_combined_phase_timings,
};
use super::{BenchmarkResult, MftReader};
use crate::error::{MftError, Result};

impl MftReader {
    /// Read MFT with detailed phase timing for benchmarking.
    ///
    /// This method measures each phase of MFT reading separately:
    /// - Open: Volume handle and metadata retrieval
    /// - Read: Disk I/O (reading chunks)
    /// - Parse: Record parsing (parallel)
    /// - Merge: Extension record merging
    /// - DataFrame build: Converting parsed records to DataFrame
    ///
    /// # Arguments
    ///
    /// * `skip_df_build` - If true, skip DataFrame building (measure I/O +
    ///   parse only)
    ///
    /// # Returns
    ///
    /// A tuple of (optional DataFrame, BenchmarkResult).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_with_timing(
        &self,
        skip_df_build: bool,
    ) -> Result<(Option<DataFrame>, BenchmarkResult)> {
        self.read_mft_with_timing_internal(skip_df_build)
    }

    /// Read MFT with timing (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub const fn read_with_timing(
        &self,
        _skip_df_build: bool,
    ) -> Result<(Option<DataFrame>, BenchmarkResult)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for MFT reading with detailed phase timing.
    ///
    /// This method measures each phase separately for benchmarking purposes.
    #[cfg(windows)]
    #[expect(
        clippy::too_many_lines,
        reason = "sequential I/O pipeline with per-phase timing cannot be meaningfully split"
    )]
    fn read_mft_with_timing_internal(
        &self,
        skip_df_build: bool,
    ) -> Result<(Option<DataFrame>, BenchmarkResult)> {
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
        let bitmap = self.handle.get_mft_bitmap().ok();
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
            "📊 Benchmark: MFT characteristics"
        );

        // Phase 2+3: Read + Parse (using SoA path for optimal df_build)
        // The ParallelMftReader reads sequentially then parses in parallel.
        // Using read_all_parallel_to_columns returns ParsedColumns (SoA layout)
        // which eliminates the AoS→SoA transpose in df_build.
        //
        // Fast path (merge_extensions=false): Skips extension records (~1% of files
        // with many hard links/ADS). ~15-25% faster, ideal for file search.
        let read_parse_start = Instant::now();
        let parallel_reader = ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);
        let handle = self.handle.raw_handle();

        let mut parsed_columns = parallel_reader.read_all_parallel_to_columns::<fn(u64, u64)>(
            handle,
            self.merge_extensions,
            self.expand_links,
            None,
        )?;

        // Add placeholder records for missing parent directories.
        // This matches the legacy output behavior where `at()` creates placeholder
        // records for any referenced FRS that hasn't been seen yet.
        // Can be disabled with `with_add_placeholders(false)` for ~15% speedup.
        if self.add_placeholders {
            let placeholders_added = parsed_columns.add_missing_parent_placeholders();
            if placeholders_added > 0 {
                debug!(
                    placeholders_added,
                    "Added placeholder records for path resolution"
                );
            }
        }

        let read_parse_ms = read_parse_start.elapsed().as_millis() as u64;
        let records_parsed = parsed_columns.len();

        // Note: Currently read and parse are interleaved in ParallelMftReader.
        // For now, we report combined time. Future: instrument inside
        // ParallelMftReader. Estimate: ~70% read, ~30% parse on HDD; ~30% read,
        // ~70% parse on SSD
        let (read_ms, parse_ms, merge_ms) =
            estimate_combined_phase_timings(drive_type, read_parse_ms);

        info!(
            records_parsed,
            read_parse_ms, "📊 Benchmark: Read + Parse complete"
        );

        // Phase 4: DataFrame build (optional)
        // Using SoA path: ParsedColumns → DataFrame (no transpose needed!)
        let (df, df_build_ms) = if skip_df_build {
            (None, 0)
        } else {
            let df_start = Instant::now();
            let df = Self::build_dataframe_from_columns(parsed_columns)?;
            let df_ms = df_start.elapsed().as_millis() as u64;
            info!(
                df_build_ms = df_ms,
                "📊 Benchmark: DataFrame build complete (SoA path)"
            );
            (Some(df), df_ms)
        };

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
            df_build_ms,
            index_build_ms: 0,  // Not applicable for DataFrame path
            tree_metrics_ms: 0, // Not applicable for DataFrame path
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
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            records_per_sec = format!("{:.0}", records_per_sec),
            "📊 Benchmark: Complete"
        );

        Ok((df, result))
    }
}
