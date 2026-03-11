//! Shared chunk reader helper for ParallelMftReader.

use super::*;

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
    pub fn read_chunk(
        &self,
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
    ) -> Result<Vec<u8>> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
            / SECTOR_SIZE)
            * SECTOR_SIZE;

        // M1 8.4: Reuse buffer, only reallocate if needed
        let mut buffer = self.buffer.borrow_mut();
        if buffer.len() < aligned_size {
            *buffer = AlignedBuffer::new(aligned_size);
        }

        // Seek and read
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(&mut buffer.as_mut_slice()[..aligned_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        // Extract the actual data (accounting for alignment offset)
        let actual_size = (bytes_read as usize).saturating_sub(offset_adjustment);
        let data = buffer.as_slice()[offset_adjustment..offset_adjustment + actual_size].to_vec();

        Ok(data)
    }
}
