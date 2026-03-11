//! Streaming reader implementation.

use super::*;

/// Ultra-fast MFT reader with streaming processing.
///
/// This reader processes records as they are read, avoiding the need to
/// buffer the entire MFT in memory. Key optimizations:
/// - Reusable aligned buffer (no per-chunk allocation)
/// - Streaming processing (parse while reading)
/// - Larger I/O chunks (4-8 MB based on drive type)
#[derive(Debug)]
pub struct StreamingMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    chunk_size: usize,
    /// Reusable aligned buffer.
    buffer: AlignedBuffer,
}

impl StreamingMftReader {
    /// Creates a new streaming reader optimized for the given drive type.
    #[must_use]
    pub fn new(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        let chunk_size = drive_type.optimal_chunk_size();
        // Pre-allocate buffer for largest expected chunk
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        info!(
            drive_type = ?drive_type,
            chunk_size_mb = chunk_size / (1024 * 1024),
            "🚀 Created streaming reader"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
            buffer,
        }
    }

    /// Reads and processes all MFT records with streaming.
    ///
    /// This method reads chunks and processes them immediately, reducing
    /// memory pressure compared to buffering the entire MFT.
    pub fn read_all_streaming<F>(
        &mut self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;

        // Calculate total bytes for progress
        let total_bytes: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };

        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        info!(
            chunks = chunks.len(),
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            "📖 Starting streaming read"
        );

        for chunk in chunks {
            // Read chunk into reusable buffer
            let bytes_read = self.read_chunk_into_buffer(handle, &chunk, record_size)?;
            bytes_read_total += bytes_read as u64;

            // Process records from buffer using zero-copy in-place fixup
            let skip_begin = chunk.skip_begin as usize;
            let effective_count = chunk.effective_record_count() as usize;
            let record_size_usize = record_size as usize;
            let buffer_slice = self.buffer.as_mut_slice();

            for i in 0..effective_count {
                let offset = (skip_begin + i) * record_size_usize;
                if offset + record_size_usize > bytes_read {
                    break;
                }

                let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                // Apply fixup in-place on the shared buffer (zero-copy)
                let record_slice = &mut buffer_slice[offset..offset + record_size_usize];
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

            // Report progress
            if let Some(ref mut cb) = progress_callback {
                cb(bytes_read_total, total_bytes);
            }
        }

        // Merge extensions and get final results
        let all_results = if merge_extensions {
            merger.merge()
        } else {
            merger.merge()
        };

        info!(
            records = all_results.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Streaming read complete"
        );

        Ok(all_results)
    }

    /// Reads a chunk into the internal reusable buffer.
    #[expect(
        unsafe_code,
        reason = "FFI: SetFilePointerEx and ReadFile for streaming chunk reads"
    )]
    fn read_chunk_into_buffer(
        &mut self,
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
    ) -> Result<usize> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
            / SECTOR_SIZE)
            * SECTOR_SIZE;

        // Resize buffer if needed
        if self.buffer.len() < aligned_size {
            self.buffer = AlignedBuffer::new(aligned_size);
        }

        // Seek and read
        let mut new_position = 0_i64;
        // SAFETY: `handle` is a live volume handle and `new_position` is valid
        // writable storage for the duration of the seek.
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        let mut bytes_read = 0_u32;
        // SAFETY: `handle` is live, the aligned buffer slice spans
        // `aligned_size` writable bytes, and `bytes_read` is a valid
        // out-parameter.
        unsafe {
            ReadFile(
                handle,
                Some(&mut self.buffer.as_mut_slice()[..aligned_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        Ok(bytes_read as usize)
    }
}
