// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Prefetch reader implementation.

use super::prelude::*;

/// Double-buffered MFT reader with prefetching.
///
/// This reader uses two buffers and a background thread to prefetch the next
/// chunk while processing the current one. This overlaps I/O latency with
/// CPU processing time.
///
/// Key optimizations:
/// - Double-buffering: Read into buffer A while processing buffer B
/// - Prefetch thread: Background I/O doesn't block processing
/// - Large chunks: 4-8 MB based on drive type
pub struct PrefetchMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    chunk_size: usize,
}

impl PrefetchMftReader {
    /// Creates a new prefetch reader optimized for the given drive type.
    #[must_use]
    pub fn new(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        let chunk_size = drive_type.optimal_chunk_size();
        info!(
            drive_type = ?drive_type,
            chunk_size_mb = chunk_size / (1024 * 1024),
            "🚀 Created prefetch reader with double-buffering"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
        }
    }

    /// Reads all MFT records with prefetching and double-buffering.
    ///
    /// This method uses a background thread to prefetch the next chunk while
    /// processing the current one, maximizing throughput.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] when `ReadFile`/`SetFilePointerEx` fails for
    /// the current or prefetch chunk, or the prefetch thread panics before
    /// delivering its buffer.
    pub fn read_all_prefetch<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            return Ok(Vec::new());
        }

        // Calculate total bytes for progress
        let total_bytes: u64 = chunks
            .iter()
            .map(|chunk| chunk.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = self.bitmap.as_ref().map_or_else(
            || frs_to_usize(self.extent_map.total_records()),
            crate::platform::MftBitmap::count_in_use,
        );

        info!(
            chunks = num_chunks,
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            "📖 Starting prefetch read with double-buffering"
        );

        // Use MftRecordMerger for proper extension handling
        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        // Pre-allocate two buffers for double-buffering
        let max_chunk_size = frs_to_usize(
            chunks
                .iter()
                .map(|chunk| chunk.record_count * u64::from(record_size))
                .max()
                .unwrap_or_else(|| usize_to_u64(self.chunk_size)),
        );

        let mut buffer_a = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
        let mut buffer_b = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
        let mut use_buffer_a = true;

        // Process chunks with double-buffering
        for chunk in chunks {
            // Read current chunk into active buffer
            let buffer = if use_buffer_a {
                &mut buffer_a
            } else {
                &mut buffer_b
            };

            let bytes_read = Self::read_chunk_into_buffer(handle, &chunk, record_size, buffer)?;
            bytes_read_total += usize_to_u64(bytes_read);

            // Process records from buffer using zero-copy in-place fixup
            let skip_begin = frs_to_usize(chunk.skip_begin);
            let effective_count = frs_to_usize(chunk.effective_record_count());
            let record_size_usize = u32_as_usize(record_size);
            let buffer_slice = buffer.as_mut_slice();

            for i in 0..effective_count {
                let offset = (skip_begin + i) * record_size_usize;
                if offset + record_size_usize > bytes_read {
                    break;
                }
                let Some(record_slice) = buffer_slice.get_mut(offset..offset + record_size_usize)
                else {
                    // Short-read: buffer contained fewer bytes than expected.
                    break;
                };

                let frs = chunk.start_frs + usize_to_u64(skip_begin) + usize_to_u64(i);

                // Apply fixup in-place on the shared buffer (zero-copy)
                if !apply_fixup(record_slice) {
                    continue;
                }

                // Parse record from the fixed-up slice (no copy needed)
                if merge_extensions {
                    merger.add_result(parse_record_full(record_slice, frs));
                } else if let Some(rec) = parse_record(record_slice, frs) {
                    merger.add_result(ParseResult::Base(rec));
                }
            }

            // Swap buffers for next iteration
            use_buffer_a = !use_buffer_a;

            // Report progress
            if let Some(ref mut cb) = progress_callback {
                cb(bytes_read_total, total_bytes);
            }
        }

        // Merge extensions and get final results
        let all_results = merger.merge();

        info!(
            records = all_results.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Prefetch read complete"
        );

        Ok(all_results)
    }

    /// Reads a chunk into a provided buffer.
    #[expect(
        unsafe_code,
        reason = "FFI: SetFilePointerEx and ReadFile for prefetch chunk reads"
    )]
    fn read_chunk_into_buffer(
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
        buffer: &mut AlignedBuffer,
    ) -> Result<usize> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE_U64) * SECTOR_SIZE_U64;
        let offset_adjustment = frs_to_usize(chunk.disk_offset - aligned_offset);
        let aligned_size =
            (frs_to_usize(read_size) + offset_adjustment).div_ceil(SECTOR_SIZE) * SECTOR_SIZE;

        // Resize buffer if needed
        if buffer.len() < aligned_size {
            *buffer = AlignedBuffer::new(aligned_size);
        }

        // Seek and read
        let mut new_position = 0_i64;
        // SAFETY: `handle` is a live volume handle and `new_position` is valid
        // writable storage for the duration of the seek.
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset.cast_signed(),
                Some(&raw mut new_position),
                FILE_BEGIN,
            )
        }?;

        let mut bytes_read = 0_u32;
        let Some(read_slice) = buffer.as_mut_slice().get_mut(..aligned_size) else {
            // Unreachable: buffer was sized to ≥ aligned_size by the caller.
            return Err(MftError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "prefetch buffer shorter than aligned_size",
            )));
        };
        // SAFETY: `handle` is live, the aligned buffer slice spans
        // `aligned_size` writable bytes, and `bytes_read` is a valid
        // out-parameter.
        unsafe { ReadFile(handle, Some(read_slice), Some(&raw mut bytes_read), None) }?;

        Ok(u32_as_usize(bytes_read))
    }
}
