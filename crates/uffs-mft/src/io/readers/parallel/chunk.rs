// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shared chunk reader helper for `ParallelMftReader`.
//!
//! Windows-only: requires HANDLE.

#![cfg(windows)]

use super::prelude::*;

impl ParallelMftReader {
    /// Reads a single chunk from disk.
    ///
    /// M1 8.4: Uses reusable aligned buffer to minimize allocations.
    /// The buffer is resized only if the chunk is larger than the current
    /// buffer.
    #[expect(
        unsafe_code,
        reason = "FFI: SetFilePointerEx and ReadFile for chunk-based MFT access"
    )]
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if `SetFilePointerEx` or `ReadFile` fails, or
    /// if the volume read returns fewer bytes than the requested chunk size.
    pub fn read_chunk(
        &self,
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
    ) -> Result<Vec<u8>> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE_U64) * SECTOR_SIZE_U64;
        let offset_adjustment = frs_to_usize(chunk.disk_offset - aligned_offset);
        let aligned_size =
            (frs_to_usize(read_size) + offset_adjustment).div_ceil(SECTOR_SIZE) * SECTOR_SIZE;

        // M1 8.4: Reuse buffer, only reallocate if needed
        let mut buffer = self.buffer.borrow_mut();
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
            // Unreachable: buffer was sized to ≥ aligned_size upstream.
            return Err(MftError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "chunk buffer shorter than aligned_size",
            )));
        };
        // SAFETY: `handle` is live, the aligned buffer slice spans
        // `aligned_size` writable bytes, and `bytes_read` is a valid
        // out-parameter.
        unsafe { ReadFile(handle, Some(read_slice), Some(&raw mut bytes_read), None) }?;

        // Extract the actual data (accounting for alignment offset)
        let actual_size = u32_as_usize(bytes_read).saturating_sub(offset_adjustment);
        let data = buffer
            .as_slice()
            .get(offset_adjustment..offset_adjustment + actual_size)
            .ok_or_else(|| {
                MftError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "chunk read produced fewer bytes than expected",
                ))
            })?
            .to_vec();

        Ok(data)
    }
}
