//! Column-oriented parallel reader path.
//!
//! Windows-only: requires HANDLE.

#![cfg(windows)]

use super::*;

impl ParallelMftReader {
    /// Reads all MFT records and returns them as `ParsedColumns` (SoA layout).
    ///
    /// This is the optimized path that avoids the AoS→SoA transpose by:
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
    /// `ParsedColumns` ready for direct conversion to Polars DataFrame.
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
        info!(estimated_records, "Estimated record count");

        let record_size = self.extent_map.bytes_per_record;
        let records_processed = Arc::clone(&self.records_processed);

        // Calculate total bytes to read for progress reporting
        let total_bytes_to_read: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Read all chunks (sequential I/O for handle safety)
        debug!("Reading all chunks into memory...");
        let mut total_bytes_read: u64 = 0;
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(chunks.len());

        for (idx, chunk) in chunks.into_iter().enumerate() {
            trace!(
                chunk_idx = idx,
                start_frs = chunk.start_frs,
                "Reading chunk"
            );
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    total_bytes_read += data.len() as u64;
                    if let Some(ref cb) = progress_callback {
                        cb(total_bytes_read, total_bytes_to_read);
                    }
                    chunk_data.push((chunk, data));
                }
                Err(e) => {
                    warn!(chunk_idx = idx, error = ?e, "Failed to read chunk");
                }
            }
        }

        info!(
            chunks_read = chunk_data.len(),
            total_bytes = total_bytes_read,
            total_mb = total_bytes_read / (1024 * 1024),
            merge_extensions,
            "All chunks read into memory"
        );

        if merge_extensions {
            // FULL PATH: Parse → Merge → ParsedColumns
            // Uses HashMap-based MftRecordMerger for complete extension handling.
            // ~15-25% slower but handles files with many hard links/ADS correctly.

            #[derive(Default)]
            struct ChunkStats {
                results: Vec<ParseResult>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter()
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

                        let record_data = &data[offset..offset + record_size];
                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

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
                .reduce(ChunkStats::default, |mut a, b| {
                    a.results.extend(b.results);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);
            if fixup_fail_count > 0 {
                warn!(
                    fixup_failures = fixup_fail_count,
                    "⚠️  MFT records with fixup failures detected (possible corruption)"
                );
            }

            if combined.skipped > 0 {
                debug!(
                    skipped_records = combined.skipped,
                    "📋 Records skipped (not in use or invalid)"
                );
            }

            // Merge extensions and convert directly to ParsedColumns
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in combined.results {
                merger.add_result(result);
            }

            Ok(merger.merge_into_columns(expand_links))
        } else {
            // FAST PATH: Parse directly to ParsedColumns (no HashMap, no merge)
            // Skips extension records (~1% of files with many hard links/ADS).
            // ~15-25% faster on SSD, ideal for file search and size analysis.

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
                        let record_size = record_size as usize;
                        let skip_begin = chunk.skip_begin as usize;
                        let effective_count = chunk.effective_record_count() as usize;

                        acc.columns.reserve(effective_count);

                        for i in 0..effective_count {
                            let offset = (skip_begin + i) * record_size;
                            if offset + record_size > data.len() {
                                break;
                            }

                            let record_data = &data[offset..offset + record_size];
                            let frs = chunk.start_frs + skip_begin as u64 + i as u64;

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
                .reduce(
                    || FastStats::default(),
                    |mut a, b| {
                        a.columns.extend(b.columns);
                        a.skipped += b.skipped;
                        a.extensions_skipped += b.extensions_skipped;
                        a.processed += b.processed;
                        a
                    },
                );

            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);
            if fixup_fail_count > 0 {
                warn!(
                    fixup_failures = fixup_fail_count,
                    "⚠️  MFT records with fixup failures detected (possible corruption)"
                );
            }

            if combined.skipped > 0 || combined.extensions_skipped > 0 {
                debug!(
                    skipped_records = combined.skipped,
                    extensions_skipped = combined.extensions_skipped,
                    "📋 Records skipped (fast path)"
                );
            }

            Ok(combined.columns)
        }
    }
}
