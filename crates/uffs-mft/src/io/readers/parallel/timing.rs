// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Parallel reader timing path.
//!
//! Windows-only: requires HANDLE.

#![cfg(windows)]

use super::prelude::*;

/// `(chunk, raw_bytes)` pair produced by phase 1 of
/// [`ParallelMftReader::read_all_parallel_with_timing`] for downstream
/// fold/merge passes.
type TimedChunkData = Vec<(ReadChunk, Vec<u8>)>;

/// Output of [`ParallelMftReader::read_all_chunks_timed`]: the populated
/// chunk vector plus the elapsed I/O time in nanoseconds.
type TimedChunkRead = (TimedChunkData, u64);

/// Emit the final per-phase + wall-clock summary for a finished
/// [`ParallelMftReader::read_all_parallel_with_timing`] run.
///
/// Hoisted out of the orchestrator so the cognitive complexity of the
/// public method stays well below clippy's bar.  The tracing structure is
/// unchanged from the original inline call.
fn log_timing_summary(timing: &ReadParseTiming) {
    info!(
        io_ms = timing.io_ms(),
        parse_ms = timing.parse_ms(),
        merge_ms = timing.merge_ms(),
        wall_ms = timing.wall_ms(),
        overlap_ratio = format!("{:.2}", timing.overlap_ratio()),
        "Timing breakdown complete"
    );
}

impl ParallelMftReader {
    /// Reads and parses all MFT records with accurate timing breakdown.
    ///
    /// This method is identical to `read_all_parallel_with_progress` but
    /// instruments each phase (I/O, parse, merge) with precise timing.
    /// Use this for benchmarking to get accurate phase timings instead of
    /// estimates.
    ///
    /// # Returns
    ///
    /// A tuple of (records, timing) where timing contains accurate measurements
    /// for each phase.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if the I/O phase cannot read a chunk. Phase
    /// timing measurements are only populated on success.
    pub fn read_all_parallel_with_timing(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
    ) -> Result<(Vec<ParsedRecord>, ReadParseTiming)> {
        use std::time::Instant;

        let wall_start = Instant::now();

        info!(
            chunk_size = self.chunk_size,
            merge_extensions, "Starting parallel MFT read with timing"
        );

        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        info!(num_chunks = chunks.len(), "Generated read chunks");

        let estimated_records = self.bitmap.as_ref().map_or_else(
            || frs_to_usize(self.extent_map.total_records()),
            crate::platform::MftBitmap::count_in_use,
        );
        let record_size = self.extent_map.bytes_per_record;

        // Phase 1: I/O - read every chunk (sequentially, handle is not Send).
        let (mut chunk_data, io_ns) = self.read_all_chunks_timed(handle, chunks, record_size)?;

        // Phase 2 + 3: Parse + (optionally) Merge.
        let (records, parse_ns, merge_ns) = if merge_extensions {
            self.parse_with_merge_timed(&mut chunk_data, record_size, estimated_records)
        } else {
            self.parse_legacy_timed(&mut chunk_data, record_size)
        };

        let wall_ns = nanos_to_u64(wall_start.elapsed().as_nanos());
        let timing = ReadParseTiming {
            io_ns,
            parse_ns,
            merge_ns,
            wall_ns,
        };
        log_timing_summary(&timing);

        Ok((records, timing))
    }

    /// Phase 1 of [`Self::read_all_parallel_with_timing`].
    ///
    /// Sequentially reads every chunk via [`Self::read_chunk`] and surfaces
    /// the latest [`MftError::Io`] when too many chunks fail in a row
    /// (the volume is most likely write-protected at that point).
    /// Returns the populated `chunk_data` plus the elapsed I/O time in
    /// nanoseconds.
    fn read_all_chunks_timed(
        &self,
        handle: HANDLE,
        chunks: Vec<ReadChunk>,
        record_size: u32,
    ) -> Result<TimedChunkRead> {
        /// Abort threshold: if this many consecutive chunks fail, the volume
        /// is likely write-protected or otherwise inaccessible.
        const EARLY_ABORT_THRESHOLD: u32 = 10;

        use std::time::Instant;

        let num_chunks = chunks.len();
        let io_start = Instant::now();
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(num_chunks);
        let mut consecutive_failures: u32 = 0;

        for (idx, chunk) in chunks.into_iter().enumerate() {
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    consecutive_failures = 0;
                    chunk_data.push((chunk, data));
                }
                Err(err) => {
                    consecutive_failures += 1;
                    warn!(error = ?err, "Failed to read chunk");
                    if consecutive_failures >= EARLY_ABORT_THRESHOLD {
                        warn!(
                            consecutive_failures,
                            remaining_chunks = num_chunks - idx - 1,
                            "🛑 Aborting timing read: {consecutive_failures} consecutive chunk read failures"
                        );
                        // Surface the latest underlying I/O failure so callers
                        // can distinguish complete from truncated runs.
                        return Err(err);
                    }
                }
            }
        }
        let io_ns = nanos_to_u64(io_start.elapsed().as_nanos());

        info!(
            chunks_read = chunk_data.len(),
            io_ms = io_ns / 1_000_000,
            "I/O phase complete"
        );

        Ok((chunk_data, io_ns))
    }

    /// Parse + merge with timing breakdown.  Returns the final records along
    /// with `parse_ns` and `merge_ns` measurements.
    fn parse_with_merge_timed(
        &self,
        chunk_data: &mut [(ReadChunk, Vec<u8>)],
        record_size: u32,
        estimated_records: usize,
    ) -> (Vec<ParsedRecord>, u64, u64) {
        use std::time::Instant;

        #[derive(Default)]
        struct ChunkStats {
            results: Vec<ParseResult>,
            skipped: u64,
            processed: u64,
        }

        let parse_start = Instant::now();
        let combined = chunk_data
            .par_iter_mut()
            .fold(ChunkStats::default, |mut acc, (chunk, data)| {
                let record_size_bytes = u32_as_usize(record_size);
                let skip_begin = frs_to_usize(chunk.skip_begin);
                let effective_count = frs_to_usize(chunk.effective_record_count());

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

        let parse_ns = nanos_to_u64(parse_start.elapsed().as_nanos());
        info!(
            parse_results = combined.results.len(),
            parse_ms = parse_ns / 1_000_000,
            "Parse phase complete"
        );

        let merge_start = Instant::now();
        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        for result in combined.results {
            merger.add_result(result);
        }
        let records = merger.merge();
        let merge_ns = nanos_to_u64(merge_start.elapsed().as_nanos());

        info!(
            records = records.len(),
            merge_ms = merge_ns / 1_000_000,
            "Merge phase complete"
        );

        (records, parse_ns, merge_ns)
    }

    /// Parse without extension merging, with timing breakdown.  Returns the
    /// records along with `parse_ns`; `merge_ns` is always 0.
    fn parse_legacy_timed(
        &self,
        chunk_data: &mut [(ReadChunk, Vec<u8>)],
        record_size: u32,
    ) -> (Vec<ParsedRecord>, u64, u64) {
        use std::time::Instant;

        #[derive(Default)]
        struct LegacyStats {
            records: Vec<ParsedRecord>,
            skipped: u64,
            processed: u64,
        }

        let parse_start = Instant::now();
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

        let parse_ns = nanos_to_u64(parse_start.elapsed().as_nanos());
        info!(
            records = combined.records.len(),
            parse_ms = parse_ns / 1_000_000,
            "Parse phase complete (no merge needed)"
        );

        (combined.records, parse_ns, 0)
    }
}
