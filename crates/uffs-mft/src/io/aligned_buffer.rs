//! Sector-aligned buffer utilities for direct volume I/O.

// Sector-aligned buffer - low-level allocation
#![allow(clippy::all, clippy::nursery, clippy::pedantic)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use super::SECTOR_SIZE;

/// A buffer aligned to sector boundaries for direct I/O.
///
/// Windows `FILE_FLAG_NO_BUFFERING` requires sector-aligned buffers.
#[derive(Debug)]
pub struct AlignedBuffer {
    /// The underlying buffer (over-allocated for alignment).
    data: Vec<u8>,
    /// Offset to the aligned portion.
    offset: usize,
    /// Usable size of the aligned buffer.
    size: usize,
}

impl AlignedBuffer {
    /// Creates a new aligned buffer with the specified size.
    ///
    /// The buffer will be aligned to `SECTOR_SIZE` (512 bytes).
    #[must_use]
    pub fn new(size: usize) -> Self {
        // Allocate extra space for alignment
        let alloc_size = size + SECTOR_SIZE;
        let data = vec![0_u8; alloc_size];

        // Calculate alignment offset
        let ptr = data.as_ptr() as usize;
        let offset = (SECTOR_SIZE - (ptr % SECTOR_SIZE)) % SECTOR_SIZE;

        Self { data, offset, size }
    }

    /// Returns a mutable slice to the aligned buffer.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[self.offset..self.offset + self.size]
    }

    /// Returns an immutable slice to the aligned buffer.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.data[self.offset..self.offset + self.size]
    }

    /// Returns the size of the usable buffer.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.size
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_buffer() {
        let buffer = AlignedBuffer::new(1024);
        assert_eq!(buffer.len(), 1024);

        let ptr = buffer.as_slice().as_ptr() as usize;
        assert_eq!(ptr % SECTOR_SIZE, 0);
    }

    #[test]
    fn test_aligned_buffer_write() {
        let mut buffer = AlignedBuffer::new(512);
        buffer.as_mut_slice()[0] = 0x42;
        assert_eq!(buffer.as_slice()[0], 0x42);
    }
}
