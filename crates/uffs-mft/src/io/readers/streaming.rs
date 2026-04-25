// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Streaming reader implementation.
//!
//! **Module-scoped cast justification:** `as usize` casts here convert NTFS
//! disk offsets / read sizes (`u64`) and record sizes (`u32`) into `usize` for
//! buffer slicing.  `usize` is ≥ 32 bits on every supported target; the u64
//! values are physically bounded by the volume size (≤ 2⁶⁴ bytes).
#![expect(
    clippy::cast_possible_truncation,
    reason = "NTFS disk-offset / record-size casts are lossless on supported 32/64-bit targets"
)]

use super::prelude::*;

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
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if any streaming `ReadFile`/`SetFilePointerEx`
    /// call fails mid-enumeration.
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
            .map(|chunk| chunk.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = self.bitmap.as_ref().map_or_else(
            || self.extent_map.total_records() as usize,
            crate::platform::MftBitmap::count_in_use,
        );

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
                let Some(record_slice) = buffer_slice.get_mut(offset..offset + record_size_usize)
                else {
                    // Short-read: buffer contained fewer bytes than expected.
                    break;
                };

                let frs = chunk.start_frs + skip_begin as u64 + i as u64;

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

            // Report progress
            if let Some(ref mut cb) = progress_callback {
                cb(bytes_read_total, total_bytes);
            }
        }

        // Merge extensions and get final results.  The `merge_extensions`
        // branching already happened per-record above (the legacy path skips
        // extension records at parse time), so by here both modes collapse to
        // the same `merge()` call.
        let all_results = merger.merge();

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
        let aligned_size =
            (read_size as usize + offset_adjustment).div_ceil(SECTOR_SIZE) * SECTOR_SIZE;

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
                aligned_offset.cast_signed(),
                Some(&raw mut new_position),
                FILE_BEGIN,
            )
        }?;

        let mut bytes_read = 0_u32;
        let Some(read_slice) = self.buffer.as_mut_slice().get_mut(..aligned_size) else {
            // Unreachable: streaming buffer was sized to ≥ aligned_size upfront.
            return Err(MftError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "streaming buffer shorter than aligned_size",
            )));
        };
        // SAFETY: `handle` is live, the aligned buffer slice spans
        // `aligned_size` writable bytes, and `bytes_read` is a valid
        // out-parameter.
        unsafe { ReadFile(handle, Some(read_slice), Some(&raw mut bytes_read), None) }?;

        Ok(bytes_read as usize)
    }
}
