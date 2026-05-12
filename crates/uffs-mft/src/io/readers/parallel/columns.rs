// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Column-oriented parallel reader path.
//!
//! Windows-only: requires HANDLE.

#![cfg(windows)]

use super::prelude::*;

/// `(chunk, raw_bytes)` pair produced by phase 1 of
/// [`ParallelMftReader::read_all_parallel_to_columns`] for downstream
/// fold/merge passes.
type ChunkData = Vec<(ReadChunk, Vec<u8>)>;

/// Output of [`ParallelMftReader::read_chunks_with_progress`]: the
/// populated chunk vector plus the running total of bytes read.
type ChunkReadWithProgress = (ChunkData, u64);

/// Pre-computed inputs for [`ParallelMftReader::read_all_parallel_to_columns`].
///
/// Bundling the chunks, byte total, and capacity estimate into a single
/// value lets the orchestrator hand them off to its sub-helpers without
/// blowing past clippy's `too_many_arguments` threshold.
struct ColumnsReadPlan {
    /// Bitmap-aware [`ReadChunk`] schedule handed to the sequential reader.
    chunks: Vec<ReadChunk>,
    /// `bytes_per_record` from the [`MftExtentMap`], cached for downstream
    /// helpers that don't carry the map themselves.
    record_size: u32,
    /// Conservative record-count estimate used to pre-size
    /// `MftRecordMerger` and the per-chunk [`ParsedColumns`] reservation.
    estimated_records: usize,
    /// Total bytes the reader will deliver — fed to the progress callback
    /// as the denominator.
    total_bytes_to_read: u64,
}

impl ParallelMftReader {
    /// Reads all MFT records and returns them as `ParsedColumns` (`SoA`
    /// layout).
    ///
    /// This is the optimized path that avoids the `AoS`→`SoA` transpose by:
    /// 1. Parsing records into `ParseResult` (same as before)
    /// 2. Optionally merging extensions using `MftRecordMerger`
    /// 3. Converting directly to `ParsedColumns` (no intermediate
    ///    `Vec<ParsedRecord>`)
    ///
    /// # Performance
    ///
    /// - **Fast path** (`merge_extensions=false`): Parses directly to
    ///   `ParsedColumns`, skipping the HashMap-based merge. ~15-25% faster on
    ///   SSD. Extension records (~1% of files with many hard links or ADS) are
    ///   skipped.
    ///
    /// - **Full path** (`merge_extensions=true`): Uses `MftRecordMerger` to
    ///   merge extension attributes. Complete data for all files.
    ///
    /// # Arguments
    ///
    /// * `handle` - Windows file handle to the MFT
    /// * `merge_extensions` - If true, merge extension records (slower but
    ///   complete). If false, skip extensions for maximum speed.
    /// * `progress_callback` - Optional callback for progress reporting
    ///
    /// # Returns
    ///
    /// `ParsedColumns` ready for direct conversion to Polars `DataFrame`.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if any chunk read fails. Record-level fixup
    /// failures are counted internally and do not propagate.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the by-value `Option<F>` signature lets callers pass capturing \
                  closures (`Some(move |..| {..})`) without manually managing \
                  the closure's lifetime; switching to `Option<&dyn Fn(..)>` \
                  would force every call site to introduce a separate let-binding"
    )]
    pub fn read_all_parallel_to_columns<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        expand_links: bool,
        progress_callback: Option<F>,
    ) -> Result<ParsedColumns>
    where
        F: Fn(u64, u64),
    {
        info!(
            chunk_size = self.chunk_size,
            "Starting parallel MFT read (SoA path)"
        );

        let plan = self.plan_columns_read();

        debug!("Reading all chunks into memory...");
        let (chunk_data, total_bytes_read) = self.read_chunks_with_progress(
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
            merge_extensions,
            "All chunks read into memory"
        );

        Ok(self.dispatch_columns_parse(
            &chunk_data,
            plan.record_size,
            plan.estimated_records,
            merge_extensions,
            expand_links,
        ))
    }

    /// Build the chunk schedule and the precomputed estimates / total
    /// bytes used by [`Self::read_all_parallel_to_columns`].
    fn plan_columns_read(&self) -> ColumnsReadPlan {
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

        ColumnsReadPlan {
            chunks,
            record_size,
            estimated_records,
            total_bytes_to_read,
        }
    }

    /// Dispatch into the merging vs. fast-path parse depending on whether
    /// the caller asked for full extension merging.
    fn dispatch_columns_parse(
        &self,
        chunk_data: &[(ReadChunk, Vec<u8>)],
        record_size: u32,
        estimated_records: usize,
        merge_extensions: bool,
        expand_links: bool,
    ) -> ParsedColumns {
        if merge_extensions {
            self.parse_columns_with_merge(chunk_data, record_size, estimated_records, expand_links)
        } else {
            self.parse_columns_fast(chunk_data, record_size, estimated_records, expand_links)
        }
    }

    /// Sequentially read every chunk into memory, reporting progress
    /// through `progress_callback` and aborting if too many chunks fail in
    /// a row (the volume is most likely write-protected at that point).
    ///
    /// Shared between [`Self::read_all_parallel_to_columns`] and
    /// [`Self::read_all_parallel_with_progress`] so both `SoA` and `AoS`
    /// paths use identical I/O semantics.
    pub(super) fn read_chunks_with_progress<F>(
        &self,
        handle: HANDLE,
        chunks: Vec<ReadChunk>,
        record_size: u32,
        total_bytes_to_read: u64,
        progress_callback: Option<&F>,
    ) -> Result<ChunkReadWithProgress>
    where
        F: Fn(u64, u64),
    {
        /// Abort threshold: if this many consecutive chunks fail, the volume
        /// is likely write-protected or otherwise inaccessible.
        const EARLY_ABORT_THRESHOLD: u32 = 10;

        let num_chunks = chunks.len();
        let mut total_bytes_read: u64 = 0;
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(num_chunks);
        let mut consecutive_failures: u32 = 0;

        for (idx, chunk) in chunks.into_iter().enumerate() {
            trace!(
                chunk_idx = idx,
                start_frs = chunk.start_frs,
                "Reading chunk"
            );
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    consecutive_failures = 0;
                    total_bytes_read += usize_to_u64(data.len());
                    if let Some(cb) = progress_callback {
                        cb(total_bytes_read, total_bytes_to_read);
                    }
                    chunk_data.push((chunk, data));
                }
                Err(err) => {
                    consecutive_failures += 1;
                    warn!(chunk_idx = idx, error = ?err, "Failed to read chunk");
                    if consecutive_failures >= EARLY_ABORT_THRESHOLD {
                        warn!(
                            consecutive_failures,
                            remaining_chunks = num_chunks - idx - 1,
                            "🛑 Aborting columns read: {consecutive_failures} consecutive chunk read failures"
                        );
                        return Err(err);
                    }
                }
            }
        }

        Ok((chunk_data, total_bytes_read))
    }

    /// FULL PATH: Parse → Merge → `ParsedColumns`.  Uses `HashMap`-based
    /// `MftRecordMerger` for complete extension handling (~15-25% slower than
    /// the fast path but handles files with many hard links / ADS correctly).
    fn parse_columns_with_merge(
        &self,
        chunk_data: &[(ReadChunk, Vec<u8>)],
        record_size: u32,
        estimated_records: usize,
        expand_links: bool,
    ) -> ParsedColumns {
        #[derive(Default)]
        struct ChunkStats {
            results: Vec<ParseResult>,
            skipped: u64,
            processed: u64,
        }

        let combined = chunk_data
            .par_iter()
            .fold(ChunkStats::default, |mut acc, (chunk, data)| {
                let record_size_bytes = u32_as_usize(record_size);
                let skip_begin = frs_to_usize(chunk.skip_begin);
                let effective_count = frs_to_usize(chunk.effective_record_count());

                acc.results.reserve(effective_count);

                for i in 0..effective_count {
                    let offset = (skip_begin + i) * record_size_bytes;
                    let Some(record_data) = data.get(offset..offset + record_size_bytes) else {
                        break;
                    };

                    let frs = chunk.start_frs + usize_to_u64(skip_begin) + usize_to_u64(i);

                    let result = parse_record_zero_alloc(record_data, frs);
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
        self.log_columns_stats(combined.skipped, 0);

        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        for result in combined.results {
            merger.add_result(result);
        }
        merger.merge_into_columns(expand_links)
    }

    /// FAST PATH: Parse directly to `ParsedColumns` (no `HashMap`, no merge).
    /// Skips extension records (~1% of files with many hard links / ADS) and
    /// is ~15-25% faster on SSD — ideal for file search and size analysis.
    fn parse_columns_fast(
        &self,
        chunk_data: &[(ReadChunk, Vec<u8>)],
        record_size: u32,
        estimated_records: usize,
        expand_links: bool,
    ) -> ParsedColumns {
        #[derive(Default)]
        struct FastStats {
            columns: ParsedColumns,
            skipped: u64,
            extensions_skipped: u64,
            processed: u64,
        }

        let combined = chunk_data
            .par_iter()
            .fold(
                || FastStats {
                    columns: ParsedColumns::with_capacity(
                        estimated_records / rayon::current_num_threads(),
                    ),
                    ..Default::default()
                },
                |mut acc, (chunk, data)| {
                    let record_size_bytes = u32_as_usize(record_size);
                    let skip_begin = frs_to_usize(chunk.skip_begin);
                    let effective_count = frs_to_usize(chunk.effective_record_count());

                    acc.columns.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size_bytes;
                        let Some(record_data) = data.get(offset..offset + record_size_bytes) else {
                            break;
                        };

                        let frs = chunk.start_frs + usize_to_u64(skip_begin) + usize_to_u64(i);

                        match parse_record_zero_alloc(record_data, frs) {
                            ParseResult::Base(record) => {
                                if expand_links {
                                    acc.columns.push_record_expanded(&record);
                                } else {
                                    acc.columns.push_record(&record);
                                }
                            }
                            ParseResult::Extension(_) => {
                                acc.extensions_skipped += 1;
                            }
                            ParseResult::Skip => {
                                acc.skipped += 1;
                            }
                        }
                        acc.processed += 1;
                    }
                    acc
                },
            )
            .reduce(FastStats::default, |mut acc, other| {
                acc.columns.extend(other.columns);
                acc.skipped += other.skipped;
                acc.extensions_skipped += other.extensions_skipped;
                acc.processed += other.processed;
                acc
            });

        self.records_processed
            .fetch_add(combined.processed, Ordering::Relaxed);
        self.skipped_records
            .fetch_add(combined.skipped, Ordering::Relaxed);
        self.log_columns_stats(combined.skipped, combined.extensions_skipped);

        combined.columns
    }

    /// Logs fixup-failure and skipped-record statistics for both
    /// column-building paths.
    fn log_columns_stats(&self, skipped: u64, extensions_skipped: u64) {
        let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);
        if fixup_fail_count > 0 {
            warn!(
                fixup_failures = fixup_fail_count,
                "⚠️  MFT records with fixup failures detected (possible corruption)"
            );
        }

        if skipped > 0 || extensions_skipped > 0 {
            debug!(
                skipped_records = skipped,
                extensions_skipped, "📋 Records skipped"
            );
        }
    }
}
