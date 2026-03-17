//! Parallel reader timing path.
//!
//! Windows-only: requires HANDLE.

#![cfg(windows)]

use super::*;

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

        // Generate optimized read chunks
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let num_chunks = chunks.len();
        info!(num_chunks, "Generated read chunks");

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };

        let record_size = self.extent_map.bytes_per_record;
        let records_processed = Arc::clone(&self.records_processed);

        // =========================================================================
        // Phase 1: I/O - Read all chunks (sequential I/O for handle safety)
        // =========================================================================
        let io_start = Instant::now();
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    chunk_data.push((chunk, data));
                }
                Err(e) => {
                    warn!(error = ?e, "Failed to read chunk");
                }
            }
        }
        let io_ns = io_start.elapsed().as_nanos() as u64;

        info!(
            chunks_read = chunk_data.len(),
            io_ms = io_ns / 1_000_000,
            "I/O phase complete"
        );

        // =========================================================================
        // Phase 2: Parse - Parallel parsing with Rayon
        // =========================================================================
        let parse_start = Instant::now();

        let (parse_results, merge_ns, records) = if merge_extensions {
            // Per-thread accumulator for fold/reduce pattern
            #[derive(Default)]
            struct ChunkStats {
                results: Vec<ParseResult>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter_mut()
                .fold(ChunkStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    acc.results.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        let record_slice = &mut data[offset..offset + record_size];
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
                .reduce(ChunkStats::default, |mut a, b| {
                    a.results.extend(b.results);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let parse_results = combined.results;
            let parse_ns = parse_start.elapsed().as_nanos() as u64;

            info!(
                parse_results = parse_results.len(),
                parse_ms = parse_ns / 1_000_000,
                "Parse phase complete"
            );

            // Phase 3: Merge extension records
            let merge_start = Instant::now();
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in parse_results {
                merger.add_result(result);
            }
            let records = merger.merge();
            let merge_ns = merge_start.elapsed().as_nanos() as u64;

            info!(
                records = records.len(),
                merge_ms = merge_ns / 1_000_000,
                "Merge phase complete"
            );

            (parse_ns, merge_ns, records)
        } else {
            // Legacy parsing (skips extension records)
            #[derive(Default)]
            struct LegacyStats {
                records: Vec<ParsedRecord>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter_mut()
                .fold(LegacyStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    acc.records.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        let record_slice = &mut data[offset..offset + record_size];
                        if !apply_fixup(record_slice) {
                            acc.skipped += 1;
                            acc.processed += 1;
                            continue;
                        }

                        match parse_record_full(record_slice, frs) {
                            ParseResult::Base(parsed) => acc.records.push(parsed),
                            _ => acc.skipped += 1,
                        }
                        acc.processed += 1;
                    }
                    acc
                })
                .reduce(LegacyStats::default, |mut a, b| {
                    a.records.extend(b.records);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let parse_ns = parse_start.elapsed().as_nanos() as u64;

            info!(
                records = combined.records.len(),
                parse_ms = parse_ns / 1_000_000,
                "Parse phase complete (no merge needed)"
            );

            (parse_ns, 0, combined.records)
        };

        let wall_ns = wall_start.elapsed().as_nanos() as u64;

        let timing = ReadParseTiming {
            io_ns,
            parse_ns: parse_results,
            merge_ns,
            wall_ns,
        };

        info!(
            io_ms = timing.io_ms(),
            parse_ms = timing.parse_ms(),
            merge_ms = timing.merge_ms(),
            wall_ms = timing.wall_ms(),
            overlap_ratio = format!("{:.2}", timing.overlap_ratio()),
            "Timing breakdown complete"
        );

        Ok((records, timing))
    }
}
