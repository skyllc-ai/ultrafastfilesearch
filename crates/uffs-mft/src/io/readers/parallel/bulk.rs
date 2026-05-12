// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bulk parallel reader path.
//!
//! Windows-only: requires HANDLE.

#![cfg(windows)]

use super::prelude::*;

impl ParallelMftReader {
    /// Reads all MFT records using bulk I/O (read all, then parse).
    ///
    /// This method pre-allocates a single buffer for the entire MFT and reads
    /// each extent directly into it, eliminating per-chunk allocations and
    /// copies. This uses the "tsunami" pattern for maximum I/O
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
        clippy::needless_pass_by_value,
        reason = "the by-value `Option<F>` signature lets callers pass capturing \
                  closures (`Some(move |..| {..})`) without manually managing \
                  the closure's lifetime; switching to `Option<&dyn Fn(..)>` \
                  would force every call site to introduce a separate let-binding"
    )]
    /// Bulk read using IOCP - queues ALL reads at once, lets Windows optimize
    /// disk scheduling. All I/O is submitted
    /// operations, then wait for completions.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if any bulk `SetFilePointerEx`/`ReadFile`
    /// invocation fails; the Windows error code is forwarded.
    pub fn read_all_bulk<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        let record_size = u32_as_usize(self.extent_map.bytes_per_record);
        let total_records = frs_to_usize(self.extent_map.total_records());
        let total_bytes = total_records * record_size;

        info!(
            total_records,
            total_bytes_mb = total_bytes / (1024 * 1024),
            "🚀 Starting bulk MFT read (queue all, then parse)"
        );

        // Phase 1: Allocate single buffer for entire MFT
        let alloc_start = std::time::Instant::now();
        let mut mft_buffer = AlignedBuffer::new(total_bytes);
        info!(
            alloc_ms = alloc_start.elapsed().as_millis(),
            "📦 Allocated MFT buffer"
        );

        // Phase 2: Plan reads (generate chunks, sort by LCN, log savings).
        let (sorted_chunks, bytes_to_read) = plan_bulk_chunks(
            &self.extent_map,
            self.bitmap.as_ref(),
            self.chunk_size,
            record_size,
            total_bytes,
        );

        // Phase 3: Synchronous but optimized: read in LCN order with skip
        // optimization (true IOCP would require an overlapped handle).
        let read_start = std::time::Instant::now();
        let bytes_read_total = bulk_read_chunks(
            handle,
            &sorted_chunks,
            record_size,
            mft_buffer.as_mut_slice(),
            bytes_to_read,
            progress_callback.as_ref(),
        )?;

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
        let estimated_records = bitmap_ref
            .as_ref()
            .map_or(total_records, |bm| bm.count_in_use());

        // Use par_chunks_mut to give each thread its own mutable slice
        let records_per_chunk = 4096_usize;
        let bytes_per_chunk = records_per_chunk * record_size;

        if merge_extensions {
            Ok(Self::parse_bulk_with_merge(
                buffer_slice,
                bitmap_ref,
                record_size,
                records_per_chunk,
                bytes_per_chunk,
                estimated_records,
                parse_start,
            ))
        } else {
            Ok(Self::parse_bulk_fast(
                buffer_slice,
                bitmap_ref,
                record_size,
                records_per_chunk,
                bytes_per_chunk,
                estimated_records,
                parse_start,
            ))
        }
    }

    /// Bulk parse with extension merging.  Splits `buffer_slice` into rayon
    /// par-chunks, parses each via `parse_record_full`, and feeds results
    /// through `MftRecordMerger` for full hard-link/ADS support.
    fn parse_bulk_with_merge(
        buffer_slice: &mut [u8],
        bitmap_ref: Option<&crate::platform::MftBitmap>,
        record_size: usize,
        records_per_chunk: usize,
        bytes_per_chunk: usize,
        estimated_records: usize,
        parse_start: std::time::Instant,
    ) -> Vec<ParsedRecord> {
        use rayon::prelude::*;

        let results: Vec<(Vec<ParseResult>, u64, u64)> = buffer_slice
            .par_chunks_mut(bytes_per_chunk)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let mut results = Vec::new();
                let mut skipped = 0_u64;
                let mut processed = 0_u64;

                let start_frs = chunk_idx * records_per_chunk;
                let records_in_chunk = chunk.len() / record_size;

                for i in 0..records_in_chunk {
                    let frs = usize_to_u64(start_frs + i);

                    if let Some(bm) = bitmap_ref
                        && !bm.is_record_in_use(frs)
                    {
                        skipped += 1;
                        processed += 1;
                        continue;
                    }

                    let offset = i * record_size;
                    let Some(record_slice) = chunk.get_mut(offset..offset + record_size) else {
                        break;
                    };

                    if !apply_fixup(record_slice) {
                        skipped += 1;
                        processed += 1;
                        continue;
                    }

                    let result = parse_record_full(record_slice, frs);
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

        let mut total_skipped = 0_u64;
        let mut total_processed = 0_u64;
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

        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        for result in all_results {
            merger.add_result(result);
        }
        merger.merge()
    }

    /// Bulk parse without extension merging — produces only base records and
    /// counts skipped extension records as `skipped`.
    fn parse_bulk_fast(
        buffer_slice: &mut [u8],
        bitmap_ref: Option<&crate::platform::MftBitmap>,
        record_size: usize,
        records_per_chunk: usize,
        bytes_per_chunk: usize,
        estimated_records: usize,
        parse_start: std::time::Instant,
    ) -> Vec<ParsedRecord> {
        use rayon::prelude::*;

        let results: Vec<(Vec<ParsedRecord>, u64, u64)> = buffer_slice
            .par_chunks_mut(bytes_per_chunk)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let mut records = Vec::new();
                let mut skipped = 0_u64;
                let mut processed = 0_u64;

                let start_frs = chunk_idx * records_per_chunk;
                let records_in_chunk = chunk.len() / record_size;

                for i in 0..records_in_chunk {
                    let frs = usize_to_u64(start_frs + i);

                    if let Some(bm) = bitmap_ref
                        && !bm.is_record_in_use(frs)
                    {
                        skipped += 1;
                        processed += 1;
                        continue;
                    }

                    let offset = i * record_size;
                    let Some(record_slice) = chunk.get_mut(offset..offset + record_size) else {
                        break;
                    };

                    if !apply_fixup(record_slice) {
                        skipped += 1;
                        processed += 1;
                        continue;
                    }

                    if let Some(record) = parse_record(record_slice, frs) {
                        records.push(record);
                    } else {
                        skipped += 1;
                    }
                    processed += 1;
                }
                (records, skipped, processed)
            })
            .collect();

        let mut total_skipped = 0_u64;
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

        all_records
    }
}

/// Plan the bulk-read chunk schedule.
///
/// Generates the bitmap-aware [`ReadChunk`] list for `extent_map`, sorts it
/// in LCN order for optimal disk scheduling, and logs the resulting
/// skip-optimisation savings.
///
/// Returns `(sorted_chunks, bytes_to_read)` where `bytes_to_read` is the
/// total post-skip byte count.
fn plan_bulk_chunks(
    extent_map: &MftExtentMap,
    bitmap: Option<&crate::platform::MftBitmap>,
    chunk_size: usize,
    record_size: usize,
    total_bytes: usize,
) -> (Vec<ReadChunk>, u64) {
    let mut sorted_chunks = generate_read_chunks(extent_map, bitmap, chunk_size);
    sorted_chunks.sort_by_key(|chunk| chunk.disk_offset);

    let record_size_u64 = usize_to_u64(record_size);
    let bytes_to_read: u64 = sorted_chunks
        .iter()
        .map(|chunk| {
            let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;
            effective_records * record_size_u64
        })
        .sum();

    let total_mb = total_bytes / (1024 * 1024);
    let read_mb = bytes_to_read / (1024 * 1024);
    let savings_pct = 100 - (bytes_to_read * 100 / usize_to_u64(total_bytes));

    info!(
        chunks = sorted_chunks.len(),
        total_bytes_mb = total_mb,
        bytes_to_read_mb = read_mb,
        savings_pct,
        "📊 Bitmap skip optimization: reading {read_mb}MB of {total_mb}MB ({savings_pct}% savings)",
    );

    (sorted_chunks, bytes_to_read)
}

/// Read each `sorted_chunks` entry into `mft_buffer` using sequential
/// `SetFilePointerEx` + `ReadFile` syscalls.  Honours per-chunk
/// `skip_begin` / `skip_end` so unused regions are skipped on disk.
/// Returns the total bytes actually read.
#[expect(
    unsafe_code,
    reason = "FFI: SetFilePointerEx and ReadFile for bulk MFT reads"
)]
fn bulk_read_chunks<F>(
    handle: HANDLE,
    sorted_chunks: &[ReadChunk],
    record_size: usize,
    mft_buffer: &mut [u8],
    bytes_to_read: u64,
    progress_callback: Option<&F>,
) -> Result<u64>
where
    F: Fn(u64, u64),
{
    let mut bytes_read_total: u64 = 0;

    for chunk in sorted_chunks {
        let skip_begin_bytes = frs_to_usize(chunk.skip_begin) * record_size;
        let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;

        if effective_records == 0 {
            continue;
        }

        let effective_bytes = frs_to_usize(effective_records) * record_size;
        let disk_offset = chunk.disk_offset + usize_to_u64(skip_begin_bytes);
        let buffer_offset = frs_to_usize(chunk.start_frs) * record_size + skip_begin_bytes;

        let mut new_pos: i64 = 0;
        // SAFETY: `handle` is live and `new_pos` is valid writable storage
        // for the seek result.
        unsafe {
            SetFilePointerEx(
                handle,
                disk_offset.cast_signed(),
                Some(&raw mut new_pos),
                FILE_BEGIN,
            )
        }?;

        let Some(target_slice) = mft_buffer.get_mut(buffer_offset..buffer_offset + effective_bytes)
        else {
            return Err(MftError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "bulk buffer shorter than buffer_offset + effective_bytes",
            )));
        };
        let mut bytes_read: u32 = 0;
        // SAFETY: `handle` is live, `target_slice` is an in-bounds writable
        // slice of `effective_bytes` bytes, and `bytes_read` is a valid
        // out-parameter.
        unsafe { ReadFile(handle, Some(target_slice), Some(&raw mut bytes_read), None) }?;
        bytes_read_total += u64::from(bytes_read);

        if let Some(cb) = progress_callback {
            cb(bytes_read_total, bytes_to_read);
        }
    }

    Ok(bytes_read_total)
}
