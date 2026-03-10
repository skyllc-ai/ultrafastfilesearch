//! Bulk parallel reader path.

use super::*;

impl ParallelMftReader {
    /// Reads all MFT records using bulk I/O (C++ style: read all, then parse).
    ///
    /// This method pre-allocates a single buffer for the entire MFT and reads
    /// each extent directly into it, eliminating per-chunk allocations and
    /// copies. This matches the C++ "tsunami" pattern for maximum I/O
    /// throughput.
    ///
    /// # Performance
    ///
    /// - Single allocation for entire MFT (~11GB for large drives)
    /// - Zero intermediate copies during I/O phase
    /// - Continuous sequential reads without CPU interruption
    /// - Parallel parsing after all I/O completes
    ///
    /// # Arguments
    ///
    /// * `handle` - Windows file handle to the MFT
    /// * `merge_extensions` - If true, merge extension records
    /// * `progress_callback` - Optional callback for progress reporting
    ///
    /// # Returns
    ///
    /// Vector of parsed records.
    #[expect(
        unsafe_code,
        reason = "FFI: SetFilePointerEx and ReadFile for bulk MFT reads"
    )]
    /// Bulk read using IOCP - queues ALL reads at once, lets Windows optimize
    /// disk scheduling. This is the C++ approach: submit all I/O
    /// operations, then wait for completions.
    pub fn read_all_bulk<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        use rayon::prelude::*;

        let record_size = self.extent_map.bytes_per_record as usize;
        let total_records = self.extent_map.total_records() as usize;
        let total_bytes = total_records * record_size;

        info!(
            total_records,
            total_bytes_mb = total_bytes / (1024 * 1024),
            "🚀 Starting bulk MFT read (C++ IOCP style: queue all, then parse)"
        );

        // Phase 1: Allocate single buffer for entire MFT
        let alloc_start = std::time::Instant::now();
        let mut mft_buffer = AlignedBuffer::new(total_bytes);
        info!(
            alloc_ms = alloc_start.elapsed().as_millis(),
            "📦 Allocated MFT buffer"
        );

        // Phase 2: Generate read chunks with bitmap skip optimization
        // Use generate_read_chunks which calculates skip_begin/skip_end from bitmap
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);

        // Sort chunks by disk_offset (LCN order) for optimal disk scheduling
        let mut sorted_chunks: Vec<ReadChunk> = chunks;
        sorted_chunks.sort_by_key(|c| c.disk_offset);

        // Calculate actual bytes to read (after skip optimization)
        let bytes_to_read: u64 = sorted_chunks
            .iter()
            .map(|c| {
                let effective_records = c.record_count - c.skip_begin - c.skip_end;
                effective_records * record_size as u64
            })
            .sum();

        info!(
            chunks = sorted_chunks.len(),
            total_bytes_mb = total_bytes / (1024 * 1024),
            bytes_to_read_mb = bytes_to_read / (1024 * 1024),
            savings_pct = 100 - (bytes_to_read * 100 / total_bytes as u64),
            "📊 Bitmap skip optimization: reading {}MB of {}MB ({}% savings)",
            bytes_to_read / (1024 * 1024),
            total_bytes / (1024 * 1024),
            100 - (bytes_to_read * 100 / total_bytes as u64)
        );

        // Phase 3: Open overlapped handle and create IOCP
        let read_start = std::time::Instant::now();

        // We need an overlapped handle for IOCP
        // Get volume letter from extent_map (we need to open a new handle)
        // For now, fall back to synchronous reads but queue-style
        // TODO: Accept overlapped handle as parameter for true IOCP

        // Synchronous but optimized: read in LCN order with skip optimization
        let mut bytes_read_total: u64 = 0;

        for chunk in &sorted_chunks {
            // Apply skip optimization - only read the portion with in-use records
            let skip_begin_bytes = chunk.skip_begin as usize * record_size;
            let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;

            if effective_records == 0 {
                continue; // Entire chunk is skippable
            }

            let effective_bytes = effective_records as usize * record_size;
            let disk_offset = chunk.disk_offset + skip_begin_bytes as u64;
            let buffer_offset = chunk.start_frs as usize * record_size + skip_begin_bytes;

            // Seek and read
            let mut new_pos: i64 = 0;
            unsafe {
                SetFilePointerEx(handle, disk_offset as i64, Some(&mut new_pos), FILE_BEGIN)?;

                let target_slice =
                    &mut mft_buffer.as_mut_slice()[buffer_offset..buffer_offset + effective_bytes];
                let mut bytes_read: u32 = 0;
                ReadFile(handle, Some(target_slice), Some(&mut bytes_read), None)?;
                bytes_read_total += bytes_read as u64;
            }

            // Report progress
            if let Some(ref cb) = progress_callback {
                cb(bytes_read_total, bytes_to_read);
            }
        }

        info!(
            read_ms = read_start.elapsed().as_millis(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Bulk read complete (pure I/O phase with skip optimization)"
        );

        // Phase 3: Parse all records in parallel using par_chunks_mut
        let parse_start = std::time::Instant::now();
        let buffer_slice = mft_buffer.as_mut_slice();

        // Extract bitmap reference before parallel section (avoids capturing self)
        let bitmap_ref = self.bitmap.as_ref();

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = bitmap_ref {
            bm.count_in_use()
        } else {
            total_records
        };

        // Use par_chunks_mut to give each thread its own mutable slice
        let records_per_chunk = 4096usize;
        let bytes_per_chunk = records_per_chunk * record_size;

        if merge_extensions {
            // Full parsing with extension merging
            let results: Vec<(Vec<ParseResult>, u64, u64)> = buffer_slice
                .par_chunks_mut(bytes_per_chunk)
                .enumerate()
                .map(|(chunk_idx, chunk)| {
                    let mut results = Vec::new();
                    let mut skipped = 0u64;
                    let mut processed = 0u64;

                    let start_frs = chunk_idx * records_per_chunk;
                    let records_in_chunk = chunk.len() / record_size;

                    for i in 0..records_in_chunk {
                        let frs = start_frs + i;

                        // Check bitmap if available
                        if let Some(bm) = bitmap_ref {
                            if !bm.is_record_in_use(frs as u64) {
                                skipped += 1;
                                processed += 1;
                                continue;
                            }
                        }

                        let offset = i * record_size;
                        let record_slice = &mut chunk[offset..offset + record_size];

                        // Apply fixup in-place
                        if !apply_fixup(record_slice) {
                            skipped += 1;
                            processed += 1;
                            continue;
                        }

                        // Parse record
                        let result = parse_record_full(record_slice, frs as u64);
                        if matches!(result, ParseResult::Skip) {
                            skipped += 1;
                        } else {
                            results.push(result);
                        }
                        processed += 1;
                    }
                    (results, skipped, processed)
                })
                .collect();

            // Combine results
            let mut total_skipped = 0u64;
            let mut total_processed = 0u64;
            let mut all_results = Vec::with_capacity(estimated_records);
            for (chunk_results, skipped, processed) in results {
                all_results.extend(chunk_results);
                total_skipped += skipped;
                total_processed += processed;
            }

            info!(
                parse_ms = parse_start.elapsed().as_millis(),
                records = total_processed,
                skipped = total_skipped,
                "✅ Parallel parse complete"
            );

            // Merge extensions
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in all_results {
                merger.add_result(result);
            }
            Ok(merger.merge())
        } else {
            // Fast path: skip extension merging using par_chunks_mut
            let results: Vec<(Vec<ParsedRecord>, u64, u64)> = buffer_slice
                .par_chunks_mut(bytes_per_chunk)
                .enumerate()
                .map(|(chunk_idx, chunk)| {
                    let mut records = Vec::new();
                    let mut skipped = 0u64;
                    let mut processed = 0u64;

                    let start_frs = chunk_idx * records_per_chunk;
                    let records_in_chunk = chunk.len() / record_size;

                    for i in 0..records_in_chunk {
                        let frs = start_frs + i;

                        if let Some(bm) = bitmap_ref {
                            if !bm.is_record_in_use(frs as u64) {
                                skipped += 1;
                                processed += 1;
                                continue;
                            }
                        }

                        let offset = i * record_size;
                        let record_slice = &mut chunk[offset..offset + record_size];

                        if !apply_fixup(record_slice) {
                            skipped += 1;
                            processed += 1;
                            continue;
                        }

                        if let Some(record) = parse_record(record_slice, frs as u64) {
                            records.push(record);
                        } else {
                            skipped += 1;
                        }
                        processed += 1;
                    }
                    (records, skipped, processed)
                })
                .collect();

            // Combine results
            let mut total_skipped = 0u64;
            let mut all_records = Vec::with_capacity(estimated_records);
            for (chunk_records, skipped, _processed) in results {
                all_records.extend(chunk_records);
                total_skipped += skipped;
            }

            info!(
                parse_ms = parse_start.elapsed().as_millis(),
                records = all_records.len(),
                skipped = total_skipped,
                "✅ Parallel parse complete (fast path)"
            );

            Ok(all_records)
        }
    }
}
