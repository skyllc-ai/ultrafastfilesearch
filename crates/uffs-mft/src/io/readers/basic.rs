//! Basic record and batch readers.

use super::*;

/// Reads MFT records from a volume, handling fragmented MFTs.
#[derive(Debug)]
pub struct MftRecordReader {
    /// Size of each file record in bytes.
    record_size: u32,
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Aligned buffer for reading records.
    buffer: AlignedBuffer,
}

impl MftRecordReader {
    /// Creates a new MFT record reader.
    ///
    /// # Arguments
    ///
    /// * `volume` - The volume handle to read from
    ///
    /// # Note
    ///
    /// This constructor creates a simple contiguous extent map.
    /// For fragmented MFT support, use `new_with_extents()`.
    #[must_use]
    pub fn new(volume: &VolumeHandle) -> Self {
        let record_size = volume.file_record_size();
        let volume_data = volume.volume_data();

        // Create a simple contiguous extent map
        let extent_map = MftExtentMap::contiguous(
            volume_data.mft_start_lcn,
            volume_data.mft_valid_data_length,
            volume_data.bytes_per_cluster,
            record_size,
        );

        // Allocate buffer for one record (rounded up to sector boundary)
        let buffer_size = ((record_size as usize + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;
        let buffer = AlignedBuffer::new(buffer_size);

        Self {
            record_size,
            extent_map,
            buffer,
        }
    }

    /// Creates a new MFT record reader with explicit extent mapping.
    ///
    /// This constructor should be used when the MFT is fragmented.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The extent map for the MFT
    #[must_use]
    pub fn new_with_extents(extent_map: MftExtentMap) -> Self {
        let record_size = extent_map.bytes_per_record;

        // Allocate buffer for one record (rounded up to sector boundary)
        let buffer_size = ((record_size as usize + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;
        let buffer = AlignedBuffer::new(buffer_size);

        Self {
            record_size,
            extent_map,
            buffer,
        }
    }

    /// Returns the extent map.
    #[must_use]
    pub fn extent_map(&self) -> &MftExtentMap {
        &self.extent_map
    }

    /// Returns true if the MFT is fragmented.
    #[must_use]
    pub fn is_fragmented(&self) -> bool {
        self.extent_map.is_fragmented()
    }

    /// Reads a single MFT record by its File Record Segment number.
    ///
    /// This method handles fragmented MFTs by using the extent map to
    /// translate the FRS to a physical disk location.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `frs` - The File Record Segment number to read
    ///
    /// # Errors
    ///
    /// Returns an error if the record cannot be read or is invalid.
    #[expect(
        unsafe_code,
        reason = "FFI: SetFilePointerEx and ReadFile for MFT record access"
    )]
    pub fn read_record(&mut self, handle: HANDLE, frs: u64) -> Result<&[u8]> {
        // Use extent map to get the physical offset (handles fragmentation)
        let record_offset =
            self.extent_map
                .physical_offset(frs)
                .ok_or_else(|| MftError::RecordRead {
                    frs,
                    reason: "FRS outside MFT extents or in sparse region".to_owned(),
                })?;

        // Align to sector boundary
        let aligned_offset = (record_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_within_sector = (record_offset - aligned_offset) as usize;

        // Seek to the aligned offset
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        // Read the record
        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(self.buffer.as_mut_slice()),
                Some(&mut bytes_read),
                None,
            )?;
        }

        if (bytes_read as usize) < self.record_size as usize + offset_within_sector {
            return Err(MftError::RecordRead {
                frs,
                reason: format!(
                    "Short read: expected {} bytes, got {}",
                    self.record_size, bytes_read
                ),
            });
        }

        // Return the record data (accounting for sector alignment offset)
        Ok(&self.buffer.as_slice()
            [offset_within_sector..offset_within_sector + self.record_size as usize])
    }

    /// Returns the record size in bytes.
    #[must_use]
    pub const fn record_size(&self) -> u32 {
        self.record_size
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.extent_map.total_records()
    }
}

/// Batch reader for efficient MFT reading.
///
/// Reads multiple records per I/O operation by reading entire clusters
/// or extent chunks at once.
#[derive(Debug)]
pub struct BatchMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Size of each file record in bytes.
    record_size: u32,
    /// Bytes per cluster.
    bytes_per_cluster: u32,
    /// Read block size (multiple of cluster size).
    read_block_size: usize,
    /// Aligned buffer for batch reads.
    buffer: AlignedBuffer,
}

impl BatchMftReader {
    /// Default read block size (1 MB).
    pub const DEFAULT_BLOCK_SIZE: usize = 1024 * 1024;

    /// Creates a new batch reader.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The MFT extent map
    /// * `bytes_per_cluster` - Cluster size in bytes
    #[must_use]
    pub fn new(extent_map: MftExtentMap, bytes_per_cluster: u32) -> Self {
        Self::with_block_size(extent_map, bytes_per_cluster, Self::DEFAULT_BLOCK_SIZE)
    }

    /// Creates a new batch reader with a custom block size.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The MFT extent map
    /// * `bytes_per_cluster` - Cluster size in bytes
    /// * `block_size` - Read block size (will be rounded to cluster boundary)
    #[must_use]
    pub fn with_block_size(
        extent_map: MftExtentMap,
        bytes_per_cluster: u32,
        block_size: usize,
    ) -> Self {
        let record_size = extent_map.bytes_per_record;

        // Round block size to cluster boundary
        let cluster_size = bytes_per_cluster as usize;
        let read_block_size = ((block_size + cluster_size - 1) / cluster_size) * cluster_size;

        let buffer = AlignedBuffer::new(read_block_size);

        Self {
            extent_map,
            record_size,
            bytes_per_cluster,
            read_block_size,
            buffer,
        }
    }

    /// Returns the number of records that fit in one read block.
    #[must_use]
    pub fn records_per_block(&self) -> usize {
        self.read_block_size / self.record_size as usize
    }

    /// Returns the extent map.
    #[must_use]
    pub fn extent_map(&self) -> &MftExtentMap {
        &self.extent_map
    }

    /// Reads a batch of records starting from a given FRS.
    ///
    /// This reads up to `records_per_block()` records in a single I/O
    /// operation.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `start_frs` - The first FRS to read
    ///
    /// # Returns
    ///
    /// A tuple of (buffer slice, first FRS in buffer, number of records read).
    #[expect(
        unsafe_code,
        reason = "FFI: SetFilePointerEx and ReadFile for batched MFT access"
    )]
    pub fn read_batch(&mut self, handle: HANDLE, start_frs: u64) -> Result<(&[u8], u64, usize)> {
        // Get physical offset for the starting FRS
        let start_offset =
            self.extent_map
                .physical_offset(start_frs)
                .ok_or_else(|| MftError::RecordRead {
                    frs: start_frs,
                    reason: "FRS outside MFT extents".to_owned(),
                })?;

        // Align to cluster boundary for optimal I/O
        let cluster_size = u64::from(self.bytes_per_cluster);
        let aligned_offset = (start_offset / cluster_size) * cluster_size;

        // Calculate how many records we can read
        let total_records = self.extent_map.total_records();
        let max_records = (total_records - start_frs) as usize;
        let records_to_read = max_records.min(self.records_per_block());
        let bytes_to_read = records_to_read * self.record_size as usize;

        // Seek to the aligned offset
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        // Read the batch
        let read_size = bytes_to_read.min(self.buffer.len());
        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(&mut self.buffer.as_mut_slice()[..read_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        // Calculate offset within buffer for the first record
        let offset_in_buffer = (start_offset - aligned_offset) as usize;
        let usable_bytes = (bytes_read as usize).saturating_sub(offset_in_buffer);
        let records_read = usable_bytes / self.record_size as usize;

        Ok((
            &self.buffer.as_slice()
                [offset_in_buffer..offset_in_buffer + records_read * self.record_size as usize],
            start_frs,
            records_read,
        ))
    }

    /// Extracts a single record from a batch buffer.
    ///
    /// # Arguments
    ///
    /// * `batch_buffer` - The buffer returned by `read_batch()`
    /// * `index` - The index of the record within the batch (0-based)
    ///
    /// # Returns
    ///
    /// The record data slice, or `None` if the index is out of bounds.
    #[must_use]
    pub fn extract_record<'a>(&self, batch_buffer: &'a [u8], index: usize) -> Option<&'a [u8]> {
        let record_size = self.record_size as usize;
        let start = index * record_size;
        let end = start + record_size;

        if end <= batch_buffer.len() {
            Some(&batch_buffer[start..end])
        } else {
            None
        }
    }
}
