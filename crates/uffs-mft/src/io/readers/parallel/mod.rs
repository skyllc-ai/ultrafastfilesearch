// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Parallel reader implementations and strategy entrypoints.

/// Re-exports the readers-wide prelude plus parallel-only items
/// (`IoCompletionPort`, `set_overlapped_offset`, `ParallelMftReader`,
/// `ReadParseTiming`) so that parallel's child reader paths can write
/// `impl ParallelMftReader` and reference the timing struct directly.
/// The module name `prelude` is exempt from `clippy::wildcard_imports`.
#[cfg(windows)]
mod prelude {
    pub(super) use super::super::iocp::{IoCompletionPort, set_overlapped_offset};
    pub(super) use super::super::prelude::*;
    pub(super) use super::{ParallelMftReader, ReadParseTiming};
}

#[cfg(windows)]
use prelude::*;

#[cfg(windows)]
mod bulk;
#[cfg(windows)]
mod bulk_iocp;
#[cfg(windows)]
mod chunk;
#[cfg(windows)]
mod columns;
#[cfg(windows)]
mod sliding_window;
#[cfg(windows)]
mod timing;
#[cfg(windows)]
mod to_index;
#[cfg(windows)]
mod to_index_parallel;

#[cfg(test)]
mod tests;

// Chaos reader is available outside of tests for CLI integration
mod tests_chaos;
pub use tests_chaos::{ChaosMftReader, ChaosStrategy};

/// Timing breakdown for read and parse operations.
#[expect(
    clippy::struct_field_names,
    reason = "the `_ns` suffix on every field encodes the storage unit \
              (nanoseconds) and disambiguates the raw `u64` fields from \
              their `_ms` accessor counterparts (`io_ms`, `parse_ms`, ...); \
              dropping the suffix would erase the unit at the field level"
)]
pub struct ReadParseTiming {
    /// Time spent in I/O operations (reading chunks from disk).
    /// This is the cumulative time spent in `ReadFile` calls.
    pub io_ns: u64,
    /// Time spent parsing MFT records (CPU work).
    /// This is the cumulative time spent in parsing and fixup.
    pub parse_ns: u64,
    /// Time spent merging extension records.
    pub merge_ns: u64,
    /// Total wall-clock time from start to finish.
    pub wall_ns: u64,
}

impl ReadParseTiming {
    /// Returns I/O time in milliseconds.
    #[must_use]
    pub const fn io_ms(&self) -> u64 {
        self.io_ns / 1_000_000
    }

    /// Returns parse time in milliseconds.
    #[must_use]
    pub const fn parse_ms(&self) -> u64 {
        self.parse_ns / 1_000_000
    }

    /// Returns merge time in milliseconds.
    #[must_use]
    pub const fn merge_ms(&self) -> u64 {
        self.merge_ns / 1_000_000
    }

    /// Returns wall-clock time in milliseconds.
    #[must_use]
    pub const fn wall_ms(&self) -> u64 {
        self.wall_ns / 1_000_000
    }

    /// Returns the overlap ratio (sum of phases / wall time).
    /// A ratio > 1.0 indicates phases overlapped (pipelined execution).
    /// A ratio ≈ 1.0 indicates sequential execution.
    #[must_use]
    pub fn overlap_ratio(&self) -> f64 {
        if self.wall_ns == 0 {
            return 1.0;
        }
        let sum = self.io_ns + self.parse_ns + self.merge_ns;
        ratio_f64(sum, self.wall_ns)
    }
}

/// This reader implements aggressive optimizations for maximum throughput:
/// - Extent-aware reading for fragmented MFTs
/// - Bitmap-based cluster skipping
/// - Parallel record processing using Rayon
/// - Large batch I/O (4-8 MB chunks) for reduced syscall overhead
/// - Drive-type aware tuning (SSD vs HDD vs `NVMe`)
/// - Buffer reuse to minimize allocations
///
/// Windows-only: requires HANDLE for live MFT reading.
#[cfg(windows)]
#[derive(Debug)]
pub struct ParallelMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    pub(crate) chunk_size: usize,
    /// Drive type for adaptive I/O tuning.
    drive_type: crate::platform::DriveType,
    /// Progress counter (atomic for thread-safe updates).
    records_processed: Arc<AtomicU64>,
    /// Fixup failure counter (potential corruption).
    fixup_failures: Arc<AtomicU64>,
    /// Skipped records counter (not in use or invalid).
    skipped_records: Arc<AtomicU64>,
    /// M1 8.4: Reusable aligned buffer for sequential I/O.
    /// Wrapped in `RefCell` for interior mutability since `read_chunk`
    /// needs `&mut`.
    buffer: RefCell<AlignedBuffer>,
}

/// Pre-computed inputs for
/// [`ParallelMftReader::read_all_parallel_with_progress`].
///
/// Bundling the chunks, byte total, and capacity estimate into a single
/// value lets the orchestrator hand them off to its sub-helpers without
/// blowing past clippy's `too_many_arguments` threshold.
#[cfg(windows)]
struct ParallelReadPlan {
    /// Bitmap-aware [`ReadChunk`] schedule handed to the sequential reader.
    chunks: Vec<ReadChunk>,
    /// `bytes_per_record` from the [`MftExtentMap`], cached for downstream
    /// helpers that don't carry the map themselves.
    record_size: u32,
    /// Conservative record-count estimate used to pre-size
    /// [`MftRecordMerger`] in the merging path.
    estimated_records: usize,
    /// Total bytes the reader will deliver — fed to the progress callback
    /// as the denominator.
    total_bytes_to_read: u64,
}

#[cfg(windows)]
impl ParallelMftReader {
    /// Default chunk size for SSD (64 KB) — let OS read-ahead handle
    /// prefetching. With `FILE_FLAG_SEQUENTIAL_SCAN`, smaller buffers keep
    /// the I/O pipeline fed while the OS does aggressive read-ahead.
    pub const DEFAULT_CHUNK_SIZE_SSD: usize = 64 * 1024;

    /// Default chunk size for HDD (64 KB) — let OS read-ahead handle
    /// prefetching. With `FILE_FLAG_SEQUENTIAL_SCAN`, smaller buffers keep
    /// the I/O pipeline fed while the OS does aggressive read-ahead.
    pub const DEFAULT_CHUNK_SIZE_HDD: usize = 64 * 1024;

    /// 1 MB chunk size constant.
    pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

    /// Creates a new parallel reader using the default HDD chunk size.
    /// Assumes HDD for conservative defaults.
    #[must_use]
    pub fn new(extent_map: MftExtentMap, bitmap: Option<crate::platform::MftBitmap>) -> Self {
        let drive_type = crate::platform::DriveType::Unknown;
        let chunk_size = Self::DEFAULT_CHUNK_SIZE_HDD;
        // M1 8.4: Pre-allocate reusable buffer for chunk_size + sector alignment
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        Self {
            extent_map,
            bitmap,
            chunk_size,
            drive_type,
            records_processed: Arc::new(AtomicU64::new(0)),
            fixup_failures: Arc::new(AtomicU64::new(0)),
            skipped_records: Arc::new(AtomicU64::new(0)),
            buffer: RefCell::new(buffer),
        }
    }

    /// Creates a new parallel reader optimized for the given drive type.
    #[must_use]
    pub fn new_optimized(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        let chunk_size = drive_type.optimal_chunk_size();
        // M1 8.4: Pre-allocate reusable buffer for chunk_size + sector alignment
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        info!(
            drive_type = ?drive_type,
            chunk_size_mb = chunk_size / (1024 * 1024),
            "🚀 Creating optimized reader for drive type"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
            drive_type,
            records_processed: Arc::new(AtomicU64::new(0)),
            fixup_failures: Arc::new(AtomicU64::new(0)),
            skipped_records: Arc::new(AtomicU64::new(0)),
            buffer: RefCell::new(buffer),
        }
    }

    /// Sets the chunk size for I/O operations.
    #[must_use]
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        // M1 8.4: Resize buffer to match new chunk size
        self.buffer = RefCell::new(AlignedBuffer::new(chunk_size + SECTOR_SIZE));
        self
    }

    /// Returns the number of records processed so far.
    #[must_use]
    pub fn records_processed(&self) -> u64 {
        self.records_processed.load(Ordering::Relaxed)
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.extent_map.total_records()
    }

    /// Reads and parses all MFT records in parallel.
    ///
    /// This is the main entry point for high-performance MFT reading.
    /// Uses the legacy `parse_record` which skips extension records.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    ///
    /// # Returns
    ///
    /// Vector of parsed records.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if any chunk read fails. Per-record fixup or
    /// parse failures are logged and skipped rather than propagated.
    pub fn read_all_parallel(&self, handle: HANDLE) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, false, None)
    }

    /// Reads and parses all MFT records in parallel with full legacy-output
    /// parity.
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the legacy implementation behavior.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if any chunk read fails. Extension-record
    /// merge failures are counted internally rather than propagated.
    pub fn read_all_parallel_with_merge(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
    ) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, merge_extensions, None)
    }

    /// Reads and parses all MFT records in parallel with progress callback.
    ///
    /// **LEGACY MULTI-PASS PIPELINE:** This function uses
    /// `parse_record_full → MftRecordMerger → Vec<ParsedRecord>`.
    /// The hot path (`SlidingIocpInline`) uses direct-to-index parsing instead.
    /// This function is used by legacy read modes (`Parallel`, `Auto` when not
    /// inline).
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the legacy implementation behavior.
    /// The progress callback is called during the I/O phase with
    /// (`bytes_read`, `total_bytes`).
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    /// * `progress_callback` - Optional callback called with (`bytes_read`,
    ///   `total_bytes`)
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if any chunk read fails; progress callback
    /// invocations do not short-circuit the read pipeline.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the by-value `Option<F>` signature lets callers pass capturing \
                  closures (`Some(move |..| {..})`) without manually managing \
                  the closure's lifetime; switching to `Option<&dyn Fn(..)>` \
                  would force every call site to introduce a separate let-binding"
    )]
    pub fn read_all_parallel_with_progress<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        info!(
            chunk_size = self.chunk_size,
            merge_extensions,
            bitmap_enabled = self.bitmap.is_some(),
            "Starting parallel MFT read (bitmap: {})",
            if self.bitmap.is_some() {
                "ENABLED"
            } else {
                "DISABLED"
            }
        );

        let plan = self.plan_parallel_read();

        debug!("Reading all chunks into memory...");
        let (mut chunk_data, total_bytes_read) = self.read_chunks_with_progress(
            handle,
            plan.chunks,
            plan.record_size,
            plan.total_bytes_to_read,
            progress_callback.as_ref(),
        )?;

        info!(
            chunks_read = chunk_data.len(),
            total_bytes = total_bytes_read,
            total_mb = total_bytes_read / (1024 * 1024),
            "All chunks read into memory"
        );

        // M1 8.1 OPTIMIZATION: Use fold/reduce pattern instead of per-record
        // atomics. This eliminates cache-line ping-pong across threads by
        // accumulating per-thread stats, then reducing at the end.
        if merge_extensions {
            Ok(self.parse_chunks_with_merge(
                &mut chunk_data,
                plan.record_size,
                plan.estimated_records,
            ))
        } else {
            Ok(self.parse_chunks_legacy(&mut chunk_data, plan.record_size))
        }
    }

    /// Build the chunk schedule and the precomputed estimates / total
    /// bytes used by [`Self::read_all_parallel_with_progress`].
    fn plan_parallel_read(&self) -> ParallelReadPlan {
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        info!(num_chunks = chunks.len(), "Generated read chunks");

        let estimated_records = self.bitmap.as_ref().map_or_else(
            || frs_to_usize(self.extent_map.total_records()),
            crate::platform::MftBitmap::count_in_use,
        );
        info!(estimated_records, "Estimated record count");

        let record_size = self.extent_map.bytes_per_record;
        let total_bytes_to_read: u64 = chunks
            .iter()
            .map(|chunk| chunk.record_count * u64::from(record_size))
            .sum();

        ParallelReadPlan {
            chunks,
            record_size,
            estimated_records,
            total_bytes_to_read,
        }
    }

    /// Parses every chunk through `parse_record_full` and merges extension
    /// records via [`MftRecordMerger`].  This is the full-fidelity legacy path.
    fn parse_chunks_with_merge(
        &self,
        chunk_data: &mut [(ReadChunk, Vec<u8>)],
        record_size: u32,
        estimated_records: usize,
    ) -> Vec<ParsedRecord> {
        #[derive(Default)]
        struct ChunkStats {
            results: Vec<ParseResult>,
            skipped: u64,
            processed: u64,
        }

        let combined = chunk_data
            .par_iter_mut()
            .fold(ChunkStats::default, |mut acc, (chunk, data)| {
                let record_size_bytes = u32_as_usize(record_size);
                let skip_begin = frs_to_usize(chunk.skip_begin);
                let effective_count = frs_to_usize(chunk.effective_record_count());

                if chunk.skip_begin > 0 || chunk.skip_end > 0 {
                    debug!(
                        chunk_start_frs = chunk.start_frs,
                        chunk_record_count = chunk.record_count,
                        skip_begin = chunk.skip_begin,
                        skip_end = chunk.skip_end,
                        effective_count,
                        "⚠️  Chunk has skip_begin or skip_end > 0 (parallel mode)"
                    );
                }

                acc.results.reserve(effective_count);

                for i in 0..effective_count {
                    let offset = (skip_begin + i) * record_size_bytes;
                    let Some(record_slice) = data.get_mut(offset..offset + record_size_bytes)
                    else {
                        break;
                    };

                    let frs = chunk.start_frs + usize_to_u64(skip_begin) + usize_to_u64(i);

                    if !apply_fixup(record_slice) {
                        acc.skipped += 1;
                        acc.processed += 1;
                        continue;
                    }

                    let result = parse_record_full(record_slice, frs);
                    if matches!(result, ParseResult::Skip) {
                        acc.skipped += 1;
                    } else {
                        acc.results.push(result);
                    }
                    acc.processed += 1;
                }
                acc
            })
            .reduce(ChunkStats::default, |mut acc, other| {
                acc.results.extend(other.results);
                acc.skipped += other.skipped;
                acc.processed += other.processed;
                acc
            });

        self.records_processed
            .fetch_add(combined.processed, Ordering::Relaxed);
        self.skipped_records
            .fetch_add(combined.skipped, Ordering::Relaxed);
        self.log_parse_stats(combined.skipped);

        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        for result in combined.results {
            merger.add_result(result);
        }
        merger.merge()
    }

    /// Parses every chunk through `parse_record_full` and returns only the
    /// base records — extension and skip results are counted as skipped.
    fn parse_chunks_legacy(
        &self,
        chunk_data: &mut [(ReadChunk, Vec<u8>)],
        record_size: u32,
    ) -> Vec<ParsedRecord> {
        #[derive(Default)]
        struct LegacyStats {
            records: Vec<ParsedRecord>,
            skipped: u64,
            processed: u64,
        }

        let combined = chunk_data
            .par_iter_mut()
            .fold(LegacyStats::default, |mut acc, (chunk, data)| {
                let record_size_bytes = u32_as_usize(record_size);
                let skip_begin = frs_to_usize(chunk.skip_begin);
                let effective_count = frs_to_usize(chunk.effective_record_count());

                acc.records.reserve(effective_count);

                for i in 0..effective_count {
                    let offset = (skip_begin + i) * record_size_bytes;
                    let Some(record_slice) = data.get_mut(offset..offset + record_size_bytes)
                    else {
                        break;
                    };

                    let frs = chunk.start_frs + usize_to_u64(skip_begin) + usize_to_u64(i);

                    if !apply_fixup(record_slice) {
                        acc.skipped += 1;
                        acc.processed += 1;
                        continue;
                    }

                    match parse_record_full(record_slice, frs) {
                        ParseResult::Base(parsed) => acc.records.push(parsed),
                        ParseResult::Extension(_) | ParseResult::Skip => acc.skipped += 1,
                    }
                    acc.processed += 1;
                }
                acc
            })
            .reduce(LegacyStats::default, |mut acc, other| {
                acc.records.extend(other.records);
                acc.skipped += other.skipped;
                acc.processed += other.processed;
                acc
            });

        self.records_processed
            .fetch_add(combined.processed, Ordering::Relaxed);
        self.skipped_records
            .fetch_add(combined.skipped, Ordering::Relaxed);
        self.log_parse_stats(combined.skipped);

        combined.records
    }

    /// Logs fixup-failure and skipped-record statistics shared by both
    /// parsing paths.
    fn log_parse_stats(&self, skipped_count: u64) {
        let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);
        if fixup_fail_count > 0 {
            warn!(
                fixup_failures = fixup_fail_count,
                "⚠️  MFT records with fixup failures detected (possible corruption)"
            );
        }

        if skipped_count > 0 {
            debug!(
                skipped_records = skipped_count,
                "📋 Records skipped (not in use or invalid)"
            );
        }
    }
}

/// Compute `numerator / denominator` as `f64` for timing ratios.
///
/// Precision loss from `u64→f64` is irrelevant for nanosecond counters
/// (sub-nanosecond precision is meaningless for wall-clock measurements).
#[expect(clippy::float_arithmetic, reason = "display-only ratio for profiling")]
fn ratio_f64(numerator: u64, denominator: u64) -> f64 {
    crate::index::u64_to_f64(numerator) / crate::index::u64_to_f64(denominator)
}
