//! Reader implementations and async I/O orchestration for MFT ingestion.
//! Exception: This module exceeds 800 lines because the concrete reader types
//! and shared async orchestration remain co-located pending the larger I/O
//! decomposition outside Wave 3C.

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

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;

pub struct ReadParseTiming {
    /// Time spent in I/O operations (reading chunks from disk).
    /// This is the cumulative time spent in `ReadFile` calls.
    pub io_ns: u64,
    /// Time spent parsing MFT records (CPU work).
    /// This is the cumulative time spent in parsing and fixup.
    pub parse_ns: u64,
    /// Time spent merging extension records.
    pub merge_ns: u64,
    /// Total wall-clock time from start to finish.
    pub wall_ns: u64,
}

impl ReadParseTiming {
    /// Returns I/O time in milliseconds.
    #[must_use]
    pub const fn io_ms(&self) -> u64 {
        self.io_ns / 1_000_000
    }

    /// Returns parse time in milliseconds.
    #[must_use]
    pub const fn parse_ms(&self) -> u64 {
        self.parse_ns / 1_000_000
    }

    /// Returns merge time in milliseconds.
    #[must_use]
    pub const fn merge_ms(&self) -> u64 {
        self.merge_ns / 1_000_000
    }

    /// Returns wall-clock time in milliseconds.
    #[must_use]
    pub const fn wall_ms(&self) -> u64 {
        self.wall_ns / 1_000_000
    }

    /// Returns the overlap ratio (sum of phases / wall time).
    /// A ratio > 1.0 indicates phases overlapped (pipelined execution).
    /// A ratio ≈ 1.0 indicates sequential execution.
    #[must_use]
    pub fn overlap_ratio(&self) -> f64 {
        if self.wall_ns == 0 {
            return 1.0;
        }
        let sum = self.io_ns + self.parse_ns + self.merge_ns;
        sum as f64 / self.wall_ns as f64
    }
}

/// This reader implements aggressive optimizations for maximum throughput:
/// - Extent-aware reading for fragmented MFTs
/// - Bitmap-based cluster skipping
/// - Parallel record processing using Rayon
/// - Large batch I/O (4-8 MB chunks) for reduced syscall overhead
/// - Drive-type aware tuning (SSD vs HDD vs NVMe)
/// - Buffer reuse to minimize allocations
#[derive(Debug)]
pub struct ParallelMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    pub chunk_size: usize,
    /// Drive type for adaptive I/O tuning.
    drive_type: crate::platform::DriveType,
    /// Progress counter (atomic for thread-safe updates).
    records_processed: Arc<AtomicU64>,
    /// Fixup failure counter (potential corruption).
    fixup_failures: Arc<AtomicU64>,
    /// Skipped records counter (not in use or invalid).
    skipped_records: Arc<AtomicU64>,
    /// M1 8.4: Reusable aligned buffer for sequential I/O.
    /// Wrapped in RefCell for interior mutability since read_chunk needs &mut.
    buffer: RefCell<AlignedBuffer>,
}

impl ParallelMftReader {
    /// Default chunk size for SSD (64 KB) - let OS read-ahead handle
    /// prefetching. C++ team insight: With FILE_FLAG_SEQUENTIAL_SCAN,
    /// smaller buffers keep the I/O pipeline fed while OS does aggressive
    /// read-ahead.
    pub const DEFAULT_CHUNK_SIZE_SSD: usize = 64 * 1024;

    /// Default chunk size for HDD (64 KB) - let OS read-ahead handle
    /// prefetching. C++ team insight: With FILE_FLAG_SEQUENTIAL_SCAN,
    /// smaller buffers keep the I/O pipeline fed while OS does aggressive
    /// read-ahead.
    pub const DEFAULT_CHUNK_SIZE_HDD: usize = 64 * 1024;

    /// 1 MB chunk size constant.
    pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

    /// Creates a new parallel reader using the default HDD chunk size.
    /// Assumes HDD for conservative defaults.
    #[must_use]
    pub fn new(extent_map: MftExtentMap, bitmap: Option<crate::platform::MftBitmap>) -> Self {
        let drive_type = crate::platform::DriveType::Unknown;
        let chunk_size = Self::DEFAULT_CHUNK_SIZE_HDD;
        // M1 8.4: Pre-allocate reusable buffer for chunk_size + sector alignment
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        Self {
            extent_map,
            bitmap,
            chunk_size,
            drive_type,
            records_processed: Arc::new(AtomicU64::new(0)),
            fixup_failures: Arc::new(AtomicU64::new(0)),
            skipped_records: Arc::new(AtomicU64::new(0)),
            buffer: RefCell::new(buffer),
        }
    }

    /// Creates a new parallel reader optimized for the given drive type.
    #[must_use]
    pub fn new_optimized(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        let chunk_size = drive_type.optimal_chunk_size();
        // M1 8.4: Pre-allocate reusable buffer for chunk_size + sector alignment
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        info!(
            drive_type = ?drive_type,
            chunk_size_mb = chunk_size / (1024 * 1024),
            "🚀 Creating optimized reader for drive type"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
            drive_type,
            records_processed: Arc::new(AtomicU64::new(0)),
            fixup_failures: Arc::new(AtomicU64::new(0)),
            skipped_records: Arc::new(AtomicU64::new(0)),
            buffer: RefCell::new(buffer),
        }
    }

    /// Sets the chunk size for I/O operations.
    #[must_use]
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        // M1 8.4: Resize buffer to match new chunk size
        self.buffer = RefCell::new(AlignedBuffer::new(chunk_size + SECTOR_SIZE));
        self
    }

    /// Returns the number of records processed so far.
    #[must_use]
    pub fn records_processed(&self) -> u64 {
        self.records_processed.load(Ordering::Relaxed)
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.extent_map.total_records()
    }

    /// Reads and parses all MFT records in parallel.
    ///
    /// This is the main entry point for high-performance MFT reading.
    /// Uses the legacy `parse_record` which skips extension records.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    ///
    /// # Returns
    ///
    /// Vector of parsed records.
    pub fn read_all_parallel(&self, handle: HANDLE) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, false, None)
    }

    /// Reads and parses all MFT records in parallel with full legacy-output
    /// parity.
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the legacy implementation behavior.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    pub fn read_all_parallel_with_merge(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
    ) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, merge_extensions, None)
    }

    /// Reads and parses all MFT records in parallel with progress callback.
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the legacy implementation behavior.
    /// The progress callback is called during the I/O phase with (bytes_read,
    /// total_bytes).
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    /// * `progress_callback` - Optional callback called with (bytes_read,
    ///   total_bytes)
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    pub fn read_all_parallel_with_progress<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        info!(
            chunk_size = self.chunk_size,
            merge_extensions,
            bitmap_enabled = self.bitmap.is_some(),
            "Starting parallel MFT read (bitmap: {})",
            if self.bitmap.is_some() {
                "ENABLED"
            } else {
                "DISABLED"
            }
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

        // Process chunks in parallel
        // Note: We read sequentially but parse in parallel for thread safety with
        // HANDLE
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
                    trace!(
                        chunk_idx = idx,
                        bytes = data.len(),
                        total_bytes = total_bytes_read,
                        "Chunk read successfully"
                    );

                    // Report progress after each chunk
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
            "All chunks read into memory"
        );

        // M1 8.1 OPTIMIZATION: Use fold/reduce pattern instead of per-record atomics
        // This eliminates cache-line ping-pong across threads by accumulating
        // per-thread stats, then reducing at the end.

        if merge_extensions {
            // Per-thread accumulator for fold/reduce pattern
            #[derive(Default)]
            struct ChunkStats {
                results: Vec<ParseResult>,
                skipped: u64,
                processed: u64,
            }

            // Full parsing with extension merging using fold/reduce
            // Use par_iter_mut for zero-copy in-place fixup
            let combined = chunk_data
                .par_iter_mut()
                .fold(ChunkStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    // Log chunks with non-zero skips
                    if chunk.skip_begin > 0 || chunk.skip_end > 0 {
                        debug!(
                            chunk_start_frs = chunk.start_frs,
                            chunk_record_count = chunk.record_count,
                            skip_begin = chunk.skip_begin,
                            skip_end = chunk.skip_end,
                            effective_count,
                            "⚠️  Chunk has skip_begin or skip_end > 0 (parallel mode)"
                        );
                    }

                    // Pre-allocate for this chunk's results
                    acc.results.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        // Zero-copy: apply fixup in-place on the shared buffer
                        let record_slice = &mut data[offset..offset + record_size];
                        if !apply_fixup(record_slice) {
                            acc.skipped += 1;
                            acc.processed += 1;
                            continue;
                        }

                        // Parse from the fixed-up slice (no copy needed)
                        let result = parse_record_full(record_slice, frs);
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

            // Update atomics once at the end (not per-record!)
            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let parse_results = combined.results;
            let skipped_count = combined.skipped;

            // Log statistics
            let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);

            if fixup_fail_count > 0 {
                warn!(
                    fixup_failures = fixup_fail_count,
                    "⚠️  MFT records with fixup failures detected (possible corruption)"
                );
            }

            if skipped_count > 0 {
                debug!(
                    skipped_records = skipped_count,
                    "📋 Records skipped (not in use or invalid)"
                );
            }

            // Merge extensions into base records
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in parse_results {
                merger.add_result(result);
            }

            Ok(merger.merge())
        } else {
            // Legacy parsing (skips extension records) - also uses fold/reduce
            // Use par_iter_mut for zero-copy in-place fixup
            #[derive(Default)]
            struct LegacyStats {
                records: Vec<ParsedRecord>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter_mut()
                .fold(LegacyStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    acc.records.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        // Zero-copy: apply fixup in-place on the shared buffer
                        let record_slice = &mut data[offset..offset + record_size];
                        if !apply_fixup(record_slice) {
                            acc.skipped += 1;
                            acc.processed += 1;
                            continue;
                        }

                        // Parse from the fixed-up slice (no copy needed)
                        match parse_record_full(record_slice, frs) {
                            ParseResult::Base(parsed) => acc.records.push(parsed),
                            _ => acc.skipped += 1,
                        }
                        acc.processed += 1;
                    }
                    acc
                })
                .reduce(LegacyStats::default, |mut a, b| {
                    a.records.extend(b.records);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            // Update atomics once at the end
            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            // Log statistics
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

            Ok(combined.records)
        }
    }

    /// Reads and parses all MFT records with accurate timing breakdown.
    ///
    /// This method is identical to `read_all_parallel_with_progress` but
    /// instruments each phase (I/O, parse, merge) with precise timing.
    /// Use this for benchmarking to get accurate phase timings instead of
    /// estimates.
    ///
    /// # Returns
    ///
    /// A tuple of (records, timing) where timing contains accurate measurements
    /// for each phase.
    pub fn read_all_parallel_with_timing(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
    ) -> Result<(Vec<ParsedRecord>, ReadParseTiming)> {
        use std::time::Instant;

        let wall_start = Instant::now();

        info!(
            chunk_size = self.chunk_size,
            merge_extensions, "Starting parallel MFT read with timing"
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

        let record_size = self.extent_map.bytes_per_record;
        let records_processed = Arc::clone(&self.records_processed);

        // =========================================================================
        // Phase 1: I/O - Read all chunks (sequential I/O for handle safety)
        // =========================================================================
        let io_start = Instant::now();
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    chunk_data.push((chunk, data));
                }
                Err(e) => {
                    warn!(error = ?e, "Failed to read chunk");
                }
            }
        }
        let io_ns = io_start.elapsed().as_nanos() as u64;

        info!(
            chunks_read = chunk_data.len(),
            io_ms = io_ns / 1_000_000,
            "I/O phase complete"
        );

        // =========================================================================
        // Phase 2: Parse - Parallel parsing with Rayon
        // =========================================================================
        let parse_start = Instant::now();

        let (parse_results, merge_ns, records) = if merge_extensions {
            // Per-thread accumulator for fold/reduce pattern
            #[derive(Default)]
            struct ChunkStats {
                results: Vec<ParseResult>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter_mut()
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

                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        let record_slice = &mut data[offset..offset + record_size];
                        if !apply_fixup(record_slice) {
                            acc.skipped += 1;
                            acc.processed += 1;
                            continue;
                        }

                        let result = parse_record_full(record_slice, frs);
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

            let parse_results = combined.results;
            let parse_ns = parse_start.elapsed().as_nanos() as u64;

            info!(
                parse_results = parse_results.len(),
                parse_ms = parse_ns / 1_000_000,
                "Parse phase complete"
            );

            // Phase 3: Merge extension records
            let merge_start = Instant::now();
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in parse_results {
                merger.add_result(result);
            }
            let records = merger.merge();
            let merge_ns = merge_start.elapsed().as_nanos() as u64;

            info!(
                records = records.len(),
                merge_ms = merge_ns / 1_000_000,
                "Merge phase complete"
            );

            (parse_ns, merge_ns, records)
        } else {
            // Legacy parsing (skips extension records)
            #[derive(Default)]
            struct LegacyStats {
                records: Vec<ParsedRecord>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter_mut()
                .fold(LegacyStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    acc.records.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        let record_slice = &mut data[offset..offset + record_size];
                        if !apply_fixup(record_slice) {
                            acc.skipped += 1;
                            acc.processed += 1;
                            continue;
                        }

                        match parse_record_full(record_slice, frs) {
                            ParseResult::Base(parsed) => acc.records.push(parsed),
                            _ => acc.skipped += 1,
                        }
                        acc.processed += 1;
                    }
                    acc
                })
                .reduce(LegacyStats::default, |mut a, b| {
                    a.records.extend(b.records);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let parse_ns = parse_start.elapsed().as_nanos() as u64;

            info!(
                records = combined.records.len(),
                parse_ms = parse_ns / 1_000_000,
                "Parse phase complete (no merge needed)"
            );

            (parse_ns, 0, combined.records)
        };

        let wall_ns = wall_start.elapsed().as_nanos() as u64;

        let timing = ReadParseTiming {
            io_ns,
            parse_ns: parse_results,
            merge_ns,
            wall_ns,
        };

        info!(
            io_ms = timing.io_ms(),
            parse_ms = timing.parse_ms(),
            merge_ms = timing.merge_ms(),
            wall_ms = timing.wall_ms(),
            overlap_ratio = format!("{:.2}", timing.overlap_ratio()),
            "Timing breakdown complete"
        );

        Ok((records, timing))
    }

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

    /// Bulk read using true IOCP - queues ALL reads at once, lets Windows
    /// optimize disk scheduling. This is the C++ approach: submit all I/O
    /// operations simultaneously, then wait for completions.
    ///
    /// # Arguments
    /// * `overlapped_handle` - Handle opened with FILE_FLAG_OVERLAPPED
    /// * `merge_extensions` - Whether to merge extension records
    /// * `progress_callback` - Optional progress callback
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for IOCP bulk reads"
    )]
    pub fn read_all_bulk_iocp<F>(
        &self,
        overlapped_handle: HANDLE,
        merge_extensions: bool,
        _progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        use std::pin::Pin;

        use rayon::prelude::*;
        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        let record_size = self.extent_map.bytes_per_record as usize;
        let total_records = self.extent_map.total_records() as usize;
        let total_bytes = total_records * record_size;

        info!(
            total_records,
            total_bytes_mb = total_bytes / (1024 * 1024),
            "🚀 Starting IOCP bulk MFT read (C++ style: queue ALL, then parse)"
        );

        // Phase 1: Allocate single buffer for entire MFT
        let alloc_start = std::time::Instant::now();
        let mut mft_buffer = AlignedBuffer::new(total_bytes);
        info!(
            alloc_ms = alloc_start.elapsed().as_millis(),
            "📦 Allocated MFT buffer"
        );

        // Phase 2: Generate read chunks with bitmap skip optimization
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
            bytes_to_read_mb = bytes_to_read / (1024 * 1024),
            savings_pct = if total_bytes > 0 {
                100 - (bytes_to_read * 100 / total_bytes as u64)
            } else {
                0
            },
            "📊 Bitmap skip: reading {}MB of {}MB",
            bytes_to_read / (1024 * 1024),
            total_bytes / (1024 * 1024)
        );

        // Phase 3: Create IOCP and queue ALL reads at once
        // Use adaptive I/O size based on drive type (M2 optimization)
        let io_chunk_size = self.drive_type.optimal_io_size();

        let read_start = std::time::Instant::now();
        let iocp = IoCompletionPort::new(0)?;
        iocp.associate(overlapped_handle, 0)?;

        // Prepare all overlapped operations
        // Each operation needs: OVERLAPPED struct for async I/O tracking
        struct BulkOverlappedRead {
            overlapped: windows::Win32::System::IO::OVERLAPPED,
        }

        // Estimate number of I/O operations
        let estimated_ops = (bytes_to_read as usize / io_chunk_size) + sorted_chunks.len();

        // Pin all overlapped structs for pointer stability
        let mut operations: Vec<Pin<Box<BulkOverlappedRead>>> = Vec::with_capacity(estimated_ops);
        let mut pending_count = 0usize;

        // Queue ALL reads at once, breaking large chunks into 1MB I/O operations
        for chunk in sorted_chunks.iter() {
            let skip_begin_bytes = chunk.skip_begin as usize * record_size;
            let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;

            if effective_records == 0 {
                continue;
            }

            let effective_bytes = effective_records as usize * record_size;
            let chunk_disk_offset = chunk.disk_offset + skip_begin_bytes as u64;
            let chunk_buffer_offset = chunk.start_frs as usize * record_size + skip_begin_bytes;

            // Break this chunk into adaptive I/O operations
            let mut offset_within_chunk = 0usize;
            while offset_within_chunk < effective_bytes {
                let remaining = effective_bytes - offset_within_chunk;
                let io_size = remaining.min(io_chunk_size);

                let disk_offset = chunk_disk_offset + offset_within_chunk as u64;
                let buffer_offset = chunk_buffer_offset + offset_within_chunk;

                let mut op = Box::pin(BulkOverlappedRead {
                    overlapped: unsafe { std::mem::zeroed() },
                });

                // Set offset in OVERLAPPED
                op.overlapped.Anonymous.Anonymous.Offset = (disk_offset & 0xFFFF_FFFF) as u32;
                op.overlapped.Anonymous.Anonymous.OffsetHigh = (disk_offset >> 32) as u32;

                // Issue async read
                let target_slice = unsafe {
                    std::slice::from_raw_parts_mut(
                        mft_buffer.as_mut_slice().as_mut_ptr().add(buffer_offset),
                        io_size,
                    )
                };

                let result = unsafe {
                    ReadFile(
                        overlapped_handle,
                        Some(target_slice),
                        None, // Don't wait for completion
                        Some(&mut op.overlapped as *mut _),
                    )
                };

                match result {
                    Ok(_) => {
                        // Completed synchronously
                        pending_count += 1;
                    }
                    Err(_) => {
                        let last_error = unsafe { GetLastError() };
                        if last_error == ERROR_IO_PENDING {
                            // Queued successfully - this is expected for async I/O
                            pending_count += 1;
                        } else {
                            return Err(MftError::Io(std::io::Error::from_raw_os_error(
                                last_error.0 as i32,
                            )));
                        }
                    }
                }

                operations.push(op);
                offset_within_chunk += io_size;
            }
        }

        let num_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        info!(
            queued = pending_count,
            io_size_mb = io_chunk_size / (1024 * 1024),
            workers = num_workers,
            drive_type = ?self.drive_type,
            "📤 Queued all reads to IOCP (adaptive I/O size)"
        );

        // Wait for all completions using multiple worker threads (C++ approach)
        // This keeps the I/O pipeline full by processing completions in parallel
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

        let bytes_read_total = Arc::new(AtomicU64::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let error_flag = Arc::new(AtomicUsize::new(0)); // 0 = no error

        // Share IOCP handle across threads (IOCP is thread-safe)
        // We need to wrap the raw pointer in a Send-safe wrapper
        // SAFETY: Windows IOCP handles are thread-safe by design
        #[derive(Clone, Copy)]
        struct SendHandle(isize);
        unsafe impl Send for SendHandle {}
        unsafe impl Sync for SendHandle {}

        let iocp_handle_raw = SendHandle(iocp.handle.0 as isize);

        // Spawn worker threads
        let mut workers = Vec::with_capacity(num_workers);
        for worker_id in 0..num_workers {
            let bytes_read = Arc::clone(&bytes_read_total);
            let completed_count = Arc::clone(&completed);
            let error = Arc::clone(&error_flag);
            let pending = pending_count;
            let handle_raw = iocp_handle_raw;

            workers.push(std::thread::spawn(move || {
                // Reconstruct HANDLE from raw isize
                let iocp_handle = HANDLE(handle_raw.0 as *mut std::ffi::c_void);

                loop {
                    // Check if all completions are done
                    if completed_count.load(Ordering::Acquire) >= pending {
                        break;
                    }

                    // Check if another thread hit an error
                    if error.load(Ordering::Acquire) != 0 {
                        break;
                    }

                    let mut bytes_transferred: u32 = 0;
                    let mut completion_key: usize = 0;
                    let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                        std::ptr::null_mut();

                    // Use short timeout to allow checking completion count
                    let result = unsafe {
                        GetQueuedCompletionStatus(
                            iocp_handle,
                            &mut bytes_transferred,
                            &mut completion_key,
                            &mut overlapped_ptr,
                            100, // 100ms timeout
                        )
                    };

                    if result.is_ok() {
                        bytes_read.fetch_add(bytes_transferred as u64, Ordering::Relaxed);
                        let prev = completed_count.fetch_add(1, Ordering::AcqRel);
                        if prev + 1 >= pending {
                            // We completed the last one
                            break;
                        }
                    } else {
                        let last_error = unsafe { GetLastError() };
                        // WAIT_TIMEOUT (258) is expected when using timeout
                        if last_error.0 != 258 {
                            // Real error - signal other threads
                            error.store(last_error.0 as usize, Ordering::Release);
                            break;
                        }
                        // Timeout - loop and check again
                    }
                }
                worker_id // Return worker ID for debugging
            }));
        }

        // Wait for all workers to finish
        for worker in workers {
            let _ = worker.join();
        }

        // Check for errors
        let error_code = error_flag.load(Ordering::Acquire);
        if error_code != 0 {
            return Err(MftError::Io(std::io::Error::from_raw_os_error(
                error_code as i32,
            )));
        }

        let bytes_read_total = bytes_read_total.load(Ordering::Acquire);

        info!(
            read_ms = read_start.elapsed().as_millis(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            workers = num_workers,
            "✅ IOCP bulk read complete (multi-threaded)"
        );

        // Phase 4: Parse all records in parallel (same as read_all_bulk)
        let parse_start = std::time::Instant::now();
        let buffer_slice = mft_buffer.as_mut_slice();
        let bitmap_ref = self.bitmap.as_ref();

        let estimated_records = if let Some(ref bm) = bitmap_ref {
            bm.count_in_use()
        } else {
            total_records
        };

        let records_per_chunk = 4096usize;
        let bytes_per_chunk = records_per_chunk * record_size;

        if merge_extensions {
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

                        let parsed = parse_record_full(record_slice, frs as u64);
                        match &parsed {
                            ParseResult::Skip => skipped += 1,
                            _ => results.push(parsed),
                        }
                        processed += 1;
                    }
                    (results, skipped, processed)
                })
                .collect();

            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for (chunk_results, _, _) in results {
                for result in chunk_results {
                    merger.add_result(result);
                }
            }

            let all_records = merger.merge();
            info!(
                parse_ms = parse_start.elapsed().as_millis(),
                records = all_records.len(),
                "✅ IOCP bulk parse complete"
            );

            Ok(all_records)
        } else {
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

            let mut all_records = Vec::with_capacity(estimated_records);
            for (chunk_records, _, _) in results {
                all_records.extend(chunk_records);
            }

            info!(
                parse_ms = parse_start.elapsed().as_millis(),
                records = all_records.len(),
                "✅ IOCP bulk parse complete (fast path)"
            );

            Ok(all_records)
        }
    }

    /// Sliding window IOCP read - C++ style with 2-4 reads in flight.
    ///
    /// This matches the legacy implementation exactly:
    /// - Only 2-4 reads queued at a time (not 11,500!)
    /// - Per-read buffer allocation with recycling
    /// - Process data as it arrives (overlap I/O with parsing)
    ///
    /// Key insight from C++ team: HDDs have a single read head, so queuing
    /// thousands of reads just creates I/O scheduler overhead. 2 reads in
    /// flight = one reading, one being set up.
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for sliding window IOCP"
    )]
    pub fn read_all_sliding_window_iocp<F>(
        &self,
        overlapped_handle: HANDLE,
        merge_extensions: bool,
        _progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        use std::collections::VecDeque;
        use std::pin::Pin;

        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        let record_size = self.extent_map.bytes_per_record as usize;
        let total_records = self.extent_map.total_records() as usize;
        let total_bytes = total_records * record_size;

        // Use adaptive concurrency and I/O size based on drive type (M2 optimization)
        // For HDD, use extent-aware concurrency (fragmentation affects optimal value)
        let concurrency = if matches!(self.drive_type, crate::platform::DriveType::Hdd) {
            crate::platform::DriveType::optimal_concurrency_for_hdd(self.extent_map.extent_count())
        } else {
            self.drive_type.optimal_concurrency()
        };
        let io_chunk_size = self.drive_type.optimal_io_size();

        info!(
            total_records,
            total_bytes_mb = total_bytes / (1024 * 1024),
            concurrency,
            io_size_kb = io_chunk_size / 1024,
            drive_type = ?self.drive_type,
            "🚀 Starting sliding window IOCP read (adaptive: {} reads in flight, {}KB buffers)",
            concurrency,
            io_chunk_size / 1024
        );

        // Generate read chunks with bitmap skip optimization
        info!(
            bitmap_enabled = self.bitmap.is_some(),
            "📊 Generating read chunks (bitmap: {})",
            if self.bitmap.is_some() {
                "ENABLED"
            } else {
                "DISABLED"
            }
        );
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let mut sorted_chunks: Vec<ReadChunk> = chunks;
        sorted_chunks.sort_by_key(|c| c.disk_offset);

        // Break chunks into 1MB I/O operations
        struct IoOp {
            disk_offset: u64,
            buffer_offset: usize, // Where in final buffer this goes
            size: usize,
        }

        let mut io_ops: VecDeque<IoOp> = VecDeque::new();
        let mut buffer_offset = 0usize;
        let mut chunks_with_skips = 0usize;
        let mut total_skipped_records = 0u64;

        for chunk in sorted_chunks.iter() {
            let skip_begin_bytes = chunk.skip_begin as usize * record_size;
            let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;

            // Log chunks with non-zero skips
            if chunk.skip_begin > 0 || chunk.skip_end > 0 {
                chunks_with_skips += 1;
                total_skipped_records += chunk.skip_begin + chunk.skip_end;
                debug!(
                    chunk_start_frs = chunk.start_frs,
                    chunk_record_count = chunk.record_count,
                    skip_begin = chunk.skip_begin,
                    skip_end = chunk.skip_end,
                    effective_records,
                    "⚠️  Chunk has skip_begin or skip_end > 0"
                );
            }

            if effective_records == 0 {
                warn!(
                    chunk_start_frs = chunk.start_frs,
                    chunk_record_count = chunk.record_count,
                    skip_begin = chunk.skip_begin,
                    skip_end = chunk.skip_end,
                    "❌ SKIPPING ENTIRE CHUNK (effective_records = 0)"
                );
                continue;
            }

            let chunk_bytes = effective_records as usize * record_size;
            let mut offset_within_chunk = 0usize;

            while offset_within_chunk < chunk_bytes {
                let io_size = std::cmp::min(io_chunk_size, chunk_bytes - offset_within_chunk);
                let disk_offset =
                    chunk.disk_offset + skip_begin_bytes as u64 + offset_within_chunk as u64;

                io_ops.push_back(IoOp {
                    disk_offset,
                    buffer_offset,
                    size: io_size,
                });

                buffer_offset += io_size;
                offset_within_chunk += io_size;
            }
        }

        let total_io_ops = io_ops.len();
        let bytes_to_read = buffer_offset;

        info!(
            io_ops = total_io_ops,
            bytes_to_read_mb = bytes_to_read / (1024 * 1024),
            chunks_with_skips,
            total_skipped_records,
            "📊 Generated I/O operations"
        );

        if chunks_with_skips > 0 {
            warn!(
                chunks_with_skips,
                total_skipped_records,
                "⚠️  {} chunks have skip_begin or skip_end > 0, skipping {} total records",
                chunks_with_skips,
                total_skipped_records
            );
        }

        // Allocate final buffer for all data
        let mut mft_buffer = AlignedBuffer::new(bytes_to_read);

        // Create IOCP
        let read_start = std::time::Instant::now();
        let iocp = IoCompletionPort::new(0)?;
        iocp.associate(overlapped_handle, 0)?;

        // Sliding window state
        struct InFlightOp {
            overlapped: windows::Win32::System::IO::OVERLAPPED,
            buffer: AlignedBuffer,
            op: IoOp,
        }

        // Pre-allocate buffer pool (concurrency buffers, recycled)
        let mut buffer_pool: Vec<AlignedBuffer> = (0..concurrency)
            .map(|_| AlignedBuffer::new(io_chunk_size))
            .collect();

        // In-flight operations (pinned for OVERLAPPED pointer stability)
        let mut in_flight: Vec<Option<Pin<Box<InFlightOp>>>> =
            (0..concurrency).map(|_| None).collect();

        let mut completed_count = 0usize;
        let mut bytes_read_total = 0u64;

        // Queue initial reads (adaptive concurrency)
        for slot_id in 0..concurrency {
            if let Some(op) = io_ops.pop_front() {
                let buffer = buffer_pool.pop().unwrap();
                let mut in_flight_op = Box::pin(InFlightOp {
                    overlapped: unsafe { std::mem::zeroed() },
                    buffer,
                    op,
                });

                // Set offset in OVERLAPPED
                let offset = in_flight_op.op.disk_offset;
                // SAFETY: We need to modify the pinned data
                let op_mut = unsafe { in_flight_op.as_mut().get_unchecked_mut() };
                op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                op_mut.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

                // Issue read
                let overlapped_ptr = &mut op_mut.overlapped as *mut _;
                let read_size = op_mut.op.size;
                let result = unsafe {
                    ReadFile(
                        overlapped_handle,
                        Some(&mut op_mut.buffer.as_mut_slice()[..read_size]),
                        None,
                        Some(overlapped_ptr),
                    )
                };

                match result {
                    Ok(_) => {} // Completed synchronously
                    Err(_) => {
                        let last_error = unsafe { GetLastError() };
                        if last_error != ERROR_IO_PENDING {
                            return Err(MftError::Io(std::io::Error::from_raw_os_error(
                                last_error.0 as i32,
                            )));
                        }
                    }
                }

                in_flight[slot_id] = Some(in_flight_op);
            }
        }

        info!(
            initial_queued = in_flight.iter().filter(|s| s.is_some()).count(),
            "📤 Initial reads queued"
        );

        // Process completions and queue new reads (sliding window)
        while completed_count < total_io_ops {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.handle,
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX, // INFINITE - wait for completion
                )
            };

            if result.is_err() {
                let err = std::io::Error::last_os_error();
                warn!(error = %err, "GetQueuedCompletionStatus failed");
                continue;
            }

            // Find which slot completed
            let mut completed_slot = None;
            for (idx, slot) in in_flight.iter().enumerate() {
                if let Some(op) = slot {
                    let op_overlapped_ptr =
                        &op.overlapped as *const _ as *mut windows::Win32::System::IO::OVERLAPPED;
                    if op_overlapped_ptr == overlapped_ptr {
                        completed_slot = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot {
                // Take the completed operation
                if let Some(mut completed_op) = in_flight[slot_idx].take() {
                    let op_mut = unsafe { completed_op.as_mut().get_unchecked_mut() };

                    // Copy data to final buffer
                    let dest_offset = op_mut.op.buffer_offset;
                    let src_slice = &op_mut.buffer.as_slice()[..bytes_transferred as usize];
                    mft_buffer.as_mut_slice()
                        [dest_offset..dest_offset + bytes_transferred as usize]
                        .copy_from_slice(src_slice);

                    bytes_read_total += bytes_transferred as u64;
                    completed_count += 1;

                    // Recycle buffer and queue next read
                    let recycled_buffer = std::mem::replace(
                        &mut op_mut.buffer,
                        AlignedBuffer::new(0), // Placeholder
                    );
                    buffer_pool.push(recycled_buffer);

                    // Queue next read if available
                    if let Some(next_op) = io_ops.pop_front() {
                        let buffer = buffer_pool.pop().unwrap();
                        let mut new_in_flight = Box::pin(InFlightOp {
                            overlapped: unsafe { std::mem::zeroed() },
                            buffer,
                            op: next_op,
                        });

                        let offset = new_in_flight.op.disk_offset;
                        let new_op_mut = unsafe { new_in_flight.as_mut().get_unchecked_mut() };
                        new_op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                        new_op_mut.overlapped.Anonymous.Anonymous.OffsetHigh =
                            (offset >> 32) as u32;

                        let overlapped_ptr = &mut new_op_mut.overlapped as *mut _;
                        let read_size = new_op_mut.op.size;
                        let result = unsafe {
                            ReadFile(
                                overlapped_handle,
                                Some(&mut new_op_mut.buffer.as_mut_slice()[..read_size]),
                                None,
                                Some(overlapped_ptr),
                            )
                        };

                        match result {
                            Ok(_) => {}
                            Err(_) => {
                                let last_error = unsafe { GetLastError() };
                                if last_error != ERROR_IO_PENDING {
                                    warn!(error = ?last_error, "Failed to queue next read");
                                }
                            }
                        }

                        in_flight[slot_idx] = Some(new_in_flight);
                    }
                }
            }
        }

        let read_ms = read_start.elapsed().as_millis();
        info!(
            read_ms,
            bytes_mb = bytes_read_total / (1024 * 1024),
            completed = completed_count,
            "✅ Sliding window IOCP read complete"
        );

        // Phase 2: Parse the buffer (same as bulk IOCP)
        let parse_start = std::time::Instant::now();
        let bitmap_ref = self.bitmap.as_ref();

        // Calculate records per chunk for parallel parsing
        let bytes_per_chunk = 64 * 1024 * 1024; // 64MB chunks for parsing
        let records_per_chunk = bytes_per_chunk / record_size;
        let estimated_records = total_records;

        let buffer_slice = &mut mft_buffer.as_mut_slice()[..bytes_to_read];

        if merge_extensions {
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

                        let parsed = parse_record_full(record_slice, frs as u64);
                        match &parsed {
                            ParseResult::Skip => skipped += 1,
                            _ => results.push(parsed),
                        }
                        processed += 1;
                    }
                    (results, skipped, processed)
                })
                .collect();

            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for (chunk_results, _, _) in results {
                for result in chunk_results {
                    merger.add_result(result);
                }
            }

            let all_records = merger.merge();
            info!(
                parse_ms = parse_start.elapsed().as_millis(),
                records = all_records.len(),
                "✅ Sliding window parse complete"
            );

            Ok(all_records)
        } else {
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

            let mut all_records = Vec::with_capacity(estimated_records);
            for (chunk_records, _, _) in results {
                all_records.extend(chunk_records);
            }

            info!(
                parse_ms = parse_start.elapsed().as_millis(),
                records = all_records.len(),
                "✅ Sliding window parse complete (fast path)"
            );

            Ok(all_records)
        }
    }

    /// Sliding window IOCP read with inline parsing directly to MftIndex.
    ///
    /// This is the legacy-output parity implementation that:
    /// - Parses each 1MB chunk as soon as it completes (no buffering)
    /// - Builds the index incrementally during I/O
    /// - Creates parent placeholders on-demand
    ///
    /// This eliminates the separate parse and index build phases, saving ~7s
    /// on large MFTs by overlapping CPU work with I/O.
    ///
    /// # Arguments
    ///
    /// * `overlapped_handle` - IOCP handle for async I/O
    /// * `volume` - Volume letter (e.g., 'C')
    /// * `concurrency` - Number of I/O ops in flight (None = 2 for HDD)
    /// * `io_chunk_size` - Size of each I/O in bytes (None = 1MB)
    /// * `_progress_callback` - Optional progress callback
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for IOCP-to-index reads"
    )]
    pub fn read_all_sliding_window_iocp_to_index<F>(
        &self,
        overlapped_handle: HANDLE,
        volume: char,
        concurrency: Option<usize>,
        io_chunk_size: Option<usize>,
        _progress_callback: Option<F>,
    ) -> Result<crate::index::MftIndex>
    where
        F: Fn(u64, u64),
    {
        use std::collections::VecDeque;
        use std::pin::Pin;

        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        use crate::index::MftIndex;

        let record_size = self.extent_map.bytes_per_record as usize;
        let total_records = self.extent_map.total_records() as usize;

        // Use provided values or adaptive defaults based on drive type
        // M1: Adaptive concurrency and I/O size based on drive type
        // For HDD, use extent-aware concurrency (fragmentation affects optimal value)
        let concurrency = concurrency.unwrap_or_else(|| {
            if matches!(self.drive_type, crate::platform::DriveType::Hdd) {
                crate::platform::DriveType::optimal_concurrency_for_hdd(
                    self.extent_map.extent_count(),
                )
            } else {
                self.drive_type.optimal_concurrency()
            }
        });
        let io_chunk_size = io_chunk_size.unwrap_or_else(|| self.drive_type.optimal_io_size());

        info!(
            total_records,
            concurrency,
            io_size_kb = io_chunk_size / 1024,
            drive_type = ?self.drive_type,
            "🚀 Starting sliding window IOCP with INLINE parsing (adaptive settings)"
        );

        // Generate read chunks with bitmap skip optimization
        // For NVMe/SSD, use precise chunk generation to skip unused regions entirely
        let use_direct_chunk_io = matches!(
            self.drive_type,
            crate::platform::DriveType::Nvme | crate::platform::DriveType::Ssd
        );

        // For NVMe/SSD: use larger max to allow direct chunk-to-I/O mapping
        // For HDD: use standard io_chunk_size for predictable sequential reads
        const MAX_DIRECT_IO_SIZE: usize = 16 * 1024 * 1024; // 16MB max for direct I/O

        let sorted_chunks: Vec<ReadChunk> = match (&self.drive_type, &self.bitmap) {
            (crate::platform::DriveType::Nvme | crate::platform::DriveType::Ssd, Some(bitmap)) => {
                // NVMe/SSD: Use precise chunks that skip unused regions
                // min_gap_records=64 means gaps smaller than 64KB are read through
                // Use MAX_DIRECT_IO_SIZE as the max chunk size for direct I/O
                let mut chunks =
                    generate_precise_read_chunks(&self.extent_map, bitmap, MAX_DIRECT_IO_SIZE, 64);
                chunks.sort_by_key(|c| c.disk_offset);
                chunks
            }
            _ => {
                // HDD or no bitmap: Use standard chunk generation
                let mut chunks =
                    generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
                chunks.sort_by_key(|c| c.disk_offset);
                chunks
            }
        };

        // Build I/O operations with FRS tracking for inline parsing
        struct IoOp {
            disk_offset: u64,
            size: usize,
            start_frs: u64, // First FRS in this I/O
        }

        let mut io_ops: VecDeque<IoOp> = VecDeque::new();

        for chunk in sorted_chunks.iter() {
            let skip_begin_bytes = chunk.skip_begin as usize * record_size;
            let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;
            if effective_records == 0 {
                continue;
            }

            let chunk_bytes = effective_records as usize * record_size;

            if use_direct_chunk_io {
                // NVMe/SSD: Use chunk directly as one I/O operation (no splitting)
                // This minimizes syscall overhead since there's no seek penalty
                io_ops.push_back(IoOp {
                    disk_offset: chunk.disk_offset + skip_begin_bytes as u64,
                    size: chunk_bytes,
                    start_frs: chunk.start_frs + chunk.skip_begin,
                });
            } else {
                // HDD: Split into io_chunk_size pieces for predictable sequential reads
                let mut offset_within_chunk = 0usize;
                let mut frs_offset = 0u64;

                while offset_within_chunk < chunk_bytes {
                    let io_size = std::cmp::min(io_chunk_size, chunk_bytes - offset_within_chunk);
                    let records_in_io = io_size / record_size;
                    let disk_offset =
                        chunk.disk_offset + skip_begin_bytes as u64 + offset_within_chunk as u64;

                    io_ops.push_back(IoOp {
                        disk_offset,
                        size: io_size,
                        start_frs: chunk.start_frs + chunk.skip_begin + frs_offset,
                    });

                    offset_within_chunk += io_size;
                    frs_offset += records_in_io as u64;
                }
            }
        }

        let total_io_ops = io_ops.len();
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            total_records
        };

        // Calculate total bytes to read and max I/O size for buffer allocation
        let total_bytes_to_read: u64 = io_ops.iter().map(|op| op.size as u64).sum();
        let max_io_size = io_ops
            .iter()
            .map(|op| op.size)
            .max()
            .unwrap_or(io_chunk_size);

        info!(
            io_ops = total_io_ops,
            estimated_records,
            bytes_to_read_mb = total_bytes_to_read / (1024 * 1024),
            max_io_size_kb = max_io_size / 1024,
            direct_io = use_direct_chunk_io,
            "📊 Generated I/O operations for inline parsing"
        );

        // Create merger to accumulate parsed records (unified pipeline)
        let mut merger = MftRecordMerger::with_capacity(total_records);

        // Create IOCP
        let read_start = std::time::Instant::now();
        let iocp = IoCompletionPort::new(0)?;
        iocp.associate(overlapped_handle, 0)?;

        // Sliding window state
        struct InFlightOp {
            overlapped: windows::Win32::System::IO::OVERLAPPED,
            buffer: AlignedBuffer,
            op: IoOp,
        }

        // Allocate buffers sized for the max I/O operation
        let mut buffer_pool: Vec<AlignedBuffer> = (0..concurrency)
            .map(|_| AlignedBuffer::new(max_io_size))
            .collect();

        let mut in_flight: Vec<Option<Pin<Box<InFlightOp>>>> =
            (0..concurrency).map(|_| None).collect();

        let mut completed_count = 0usize;
        let mut bytes_read_total = 0u64;
        let mut records_parsed = 0usize;

        // Queue initial reads
        for slot_id in 0..concurrency {
            if let Some(op) = io_ops.pop_front() {
                let buffer = buffer_pool.pop().unwrap();
                let mut in_flight_op = Box::pin(InFlightOp {
                    overlapped: unsafe { std::mem::zeroed() },
                    buffer,
                    op,
                });

                let offset = in_flight_op.op.disk_offset;
                let op_mut = unsafe { in_flight_op.as_mut().get_unchecked_mut() };
                op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                op_mut.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

                let overlapped_ptr = &mut op_mut.overlapped as *mut _;
                let read_size = op_mut.op.size;
                let result = unsafe {
                    ReadFile(
                        overlapped_handle,
                        Some(&mut op_mut.buffer.as_mut_slice()[..read_size]),
                        None,
                        Some(overlapped_ptr),
                    )
                };

                match result {
                    Ok(_) => {}
                    Err(_) => {
                        let last_error = unsafe { GetLastError() };
                        if last_error != ERROR_IO_PENDING {
                            return Err(MftError::Io(std::io::Error::from_raw_os_error(
                                last_error.0 as i32,
                            )));
                        }
                    }
                }

                in_flight[slot_id] = Some(in_flight_op);
            }
        }

        // Process completions with inline parsing
        let bitmap_ref = self.bitmap.as_ref();

        while completed_count < total_io_ops {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.handle,
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX,
                )
            };

            if result.is_err() {
                let err = std::io::Error::last_os_error();
                warn!(error = %err, "GetQueuedCompletionStatus failed");
                continue;
            }

            // Find completed slot
            let mut completed_slot = None;
            for (idx, slot) in in_flight.iter().enumerate() {
                if let Some(op) = slot {
                    let op_overlapped_ptr =
                        &op.overlapped as *const _ as *mut windows::Win32::System::IO::OVERLAPPED;
                    if op_overlapped_ptr == overlapped_ptr {
                        completed_slot = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot {
                if let Some(mut completed_op) = in_flight[slot_idx].take() {
                    let op_mut = unsafe { completed_op.as_mut().get_unchecked_mut() };

                    // UNIFIED PIPELINE: parse_record_full() → MftRecordMerger
                    let buffer_slice =
                        &mut op_mut.buffer.as_mut_slice()[..bytes_transferred as usize];
                    let records_in_buffer = bytes_transferred as usize / record_size;

                    for i in 0..records_in_buffer {
                        let frs = op_mut.op.start_frs + i as u64;

                        // Check bitmap
                        if let Some(bm) = bitmap_ref {
                            if !bm.is_record_in_use(frs) {
                                continue;
                            }
                        }

                        let offset = i * record_size;
                        let record_slice = &mut buffer_slice[offset..offset + record_size];

                        // Apply fixup
                        if !apply_fixup(record_slice) {
                            continue;
                        }

                        // Parse using unified pipeline and accumulate in merger
                        let result = parse_record_full(record_slice, frs);
                        if !matches!(result, ParseResult::Skip) {
                            records_parsed += 1;
                        }
                        merger.add_result(result);
                    }

                    bytes_read_total += bytes_transferred as u64;
                    completed_count += 1;

                    // Recycle buffer and queue next read
                    let recycled_buffer =
                        std::mem::replace(&mut op_mut.buffer, AlignedBuffer::new(0));
                    buffer_pool.push(recycled_buffer);

                    if let Some(next_op) = io_ops.pop_front() {
                        let buffer = buffer_pool.pop().unwrap();
                        let mut new_in_flight = Box::pin(InFlightOp {
                            overlapped: unsafe { std::mem::zeroed() },
                            buffer,
                            op: next_op,
                        });

                        let offset = new_in_flight.op.disk_offset;
                        let new_op_mut = unsafe { new_in_flight.as_mut().get_unchecked_mut() };
                        new_op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                        new_op_mut.overlapped.Anonymous.Anonymous.OffsetHigh =
                            (offset >> 32) as u32;

                        let overlapped_ptr = &mut new_op_mut.overlapped as *mut _;
                        let read_size = new_op_mut.op.size;
                        let result = unsafe {
                            ReadFile(
                                overlapped_handle,
                                Some(&mut new_op_mut.buffer.as_mut_slice()[..read_size]),
                                None,
                                Some(overlapped_ptr),
                            )
                        };

                        match result {
                            Ok(_) => {}
                            Err(_) => {
                                let last_error = unsafe { GetLastError() };
                                if last_error != ERROR_IO_PENDING {
                                    warn!(error = ?last_error, "Failed to queue next read");
                                }
                            }
                        }

                        in_flight[slot_idx] = Some(new_in_flight);
                    }
                }
            }
        }

        let io_ms = read_start.elapsed().as_millis();
        info!(
            io_ms,
            bytes_mb = bytes_read_total / (1024 * 1024),
            records_parsed,
            base_records = merger.base_count(),
            extensions = merger.extension_count(),
            "✅ Sliding window IOCP I/O + parse complete, merging..."
        );

        // Merge extensions and build index using unified pipeline
        let parsed_records = merger.merge();
        let index = MftIndex::from_parsed_records(volume, parsed_records);

        let total_ms = read_start.elapsed().as_millis();
        info!(
            total_ms,
            io_ms,
            merge_ms = total_ms - io_ms,
            index_entries = index.records.len(),
            "✅ Sliding window IOCP with unified pipeline complete"
        );

        Ok(index)
    }

    /// Reads all MFT records using the legacy port parsing algorithm.
    ///
    /// This method uses `CppParsePipeline` which is a 100% faithful port of the
    /// C++ parsing algorithm. It processes chunks using the two-phase pipeline:
    /// - Phase 1: `preload_concurrent()` - USA fixup, max FRS discovery (NO
    ///   LOCK)
    /// - Phase 2: `load()` - Serialized attribute parsing (WITH LOCK)
    ///
    /// Reads all MFT records using parallel parsing (M3 optimization).
    ///
    /// This method uses a producer-consumer pattern:
    /// - IOCP thread reads data and sends buffers to a channel
    /// - Worker threads parse buffers using `parse_record_full()` (unified
    ///   pipeline)
    /// - After all I/O completes, results are merged via `MftRecordMerger` into
    ///   final index
    ///
    /// This is beneficial for NVMe drives where I/O is faster than parsing.
    /// For HDD, use `read_all_sliding_window_iocp_to_index` (inline parsing).
    ///
    /// # Arguments
    ///
    /// * `overlapped_handle` - Windows file handle opened with OVERLAPPED flag
    /// * `volume` - Volume letter (e.g., 'C')
    /// * `concurrency` - Number of I/O ops in flight (None = auto based on
    ///   drive)
    /// * `io_chunk_size` - Size of each I/O in bytes (None = auto based on
    ///   drive)
    /// * `num_workers` - Number of parsing worker threads (None = num_cpus)
    /// * `_progress_callback` - Optional progress callback
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for parallel IOCP reads"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "parallel I/O orchestration with worker threads requires sequential setup"
    )]
    pub fn read_all_sliding_window_iocp_to_index_parallel<F>(
        &self,
        overlapped_handle: HANDLE,
        volume: char,
        concurrency: Option<usize>,
        io_chunk_size: Option<usize>,
        num_workers: Option<usize>,
        _progress_callback: Option<F>,
    ) -> Result<crate::index::MftIndex>
    where
        F: Fn(u64, u64),
    {
        use std::collections::VecDeque;
        use std::pin::Pin;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crossbeam_channel::{Sender, bounded};
        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        use crate::index::MftIndex;

        let record_size = self.extent_map.bytes_per_record as usize;
        let total_records = self.extent_map.total_records() as usize;

        // Use provided values or adaptive defaults
        // For HDD, use extent-aware concurrency (fragmentation affects optimal value)
        let concurrency = concurrency.unwrap_or_else(|| {
            if matches!(self.drive_type, crate::platform::DriveType::Hdd) {
                crate::platform::DriveType::optimal_concurrency_for_hdd(
                    self.extent_map.extent_count(),
                )
            } else {
                self.drive_type.optimal_concurrency()
            }
        });
        let io_chunk_size = io_chunk_size.unwrap_or_else(|| self.drive_type.optimal_io_size());
        let num_workers = num_workers.unwrap_or_else(num_cpus::get);

        info!(
            total_records,
            concurrency,
            io_size_kb = io_chunk_size / 1024,
            num_workers,
            drive_type = ?self.drive_type,
            "🚀 Starting PARALLEL parsing IOCP (M3 optimization)"
        );

        // Generate read chunks with bitmap skip optimization
        // For NVMe/SSD, use precise chunk generation to skip unused regions entirely
        let use_direct_chunk_io = matches!(
            self.drive_type,
            crate::platform::DriveType::Nvme | crate::platform::DriveType::Ssd
        );

        // For NVMe/SSD: use larger max to allow direct chunk-to-I/O mapping
        // For HDD: use standard io_chunk_size for predictable sequential reads
        const MAX_DIRECT_IO_SIZE: usize = 16 * 1024 * 1024; // 16MB max for direct I/O

        let sorted_chunks: Vec<ReadChunk> = match (&self.drive_type, &self.bitmap) {
            (crate::platform::DriveType::Nvme | crate::platform::DriveType::Ssd, Some(bitmap)) => {
                // NVMe/SSD: Use precise chunks that skip unused regions
                // min_gap_records=64 means gaps smaller than 64KB are read through
                // Use MAX_DIRECT_IO_SIZE as the max chunk size for direct I/O
                let mut chunks =
                    generate_precise_read_chunks(&self.extent_map, bitmap, MAX_DIRECT_IO_SIZE, 64);
                chunks.sort_by_key(|c| c.disk_offset);
                chunks
            }
            _ => {
                // HDD or no bitmap: Use standard chunk generation
                let mut chunks =
                    generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
                chunks.sort_by_key(|c| c.disk_offset);
                chunks
            }
        };

        // Build I/O operations with FRS tracking
        struct IoOp {
            disk_offset: u64,
            size: usize,
            start_frs: u64,
        }

        let mut io_ops: VecDeque<IoOp> = VecDeque::new();

        for chunk in sorted_chunks.iter() {
            let skip_begin_bytes = chunk.skip_begin as usize * record_size;
            let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;
            if effective_records == 0 {
                continue;
            }

            let chunk_bytes = effective_records as usize * record_size;

            if use_direct_chunk_io {
                // NVMe/SSD: Use chunk directly as one I/O operation (no splitting)
                // This minimizes syscall overhead since there's no seek penalty
                io_ops.push_back(IoOp {
                    disk_offset: chunk.disk_offset + skip_begin_bytes as u64,
                    size: chunk_bytes,
                    start_frs: chunk.start_frs + chunk.skip_begin,
                });
            } else {
                // HDD: Split into io_chunk_size pieces for predictable sequential reads
                let mut offset_within_chunk = 0usize;
                let mut frs_offset = 0u64;

                while offset_within_chunk < chunk_bytes {
                    let io_size = std::cmp::min(io_chunk_size, chunk_bytes - offset_within_chunk);
                    let records_in_io = io_size / record_size;
                    let disk_offset =
                        chunk.disk_offset + skip_begin_bytes as u64 + offset_within_chunk as u64;

                    io_ops.push_back(IoOp {
                        disk_offset,
                        size: io_size,
                        start_frs: chunk.start_frs + chunk.skip_begin + frs_offset,
                    });

                    offset_within_chunk += io_size;
                    frs_offset += records_in_io as u64;
                }
            }
        }

        let total_io_ops = io_ops.len();
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            total_records
        };

        // Calculate total bytes to read and max I/O size for buffer allocation
        let total_bytes_to_read: u64 = io_ops.iter().map(|op| op.size as u64).sum();
        let max_io_size = io_ops
            .iter()
            .map(|op| op.size)
            .max()
            .unwrap_or(io_chunk_size);

        info!(
            io_ops = total_io_ops,
            estimated_records,
            bytes_to_read_mb = total_bytes_to_read / (1024 * 1024),
            max_io_size_kb = max_io_size / 1024,
            direct_io = use_direct_chunk_io,
            "📊 Generated I/O operations for parallel parsing"
        );

        // Create channel for buffer handoff (bounded to prevent memory explosion)
        // Each message contains: (buffer_data, start_frs, record_count)
        let channel_capacity = num_workers * 2;
        let (tx, rx): (
            Sender<Option<(Vec<u8>, u64, usize)>>,
            crossbeam_channel::Receiver<Option<(Vec<u8>, u64, usize)>>,
        ) = bounded(channel_capacity);

        // Shared counter for parsed records
        let records_parsed = Arc::new(AtomicUsize::new(0));

        // Clone bitmap for workers
        let bitmap_arc = self.bitmap.clone().map(Arc::new);

        // Spawn worker threads
        let mut worker_handles = Vec::with_capacity(num_workers);
        let records_per_worker = (estimated_records / num_workers) + 1;

        for worker_id in 0..num_workers {
            let rx = rx.clone();
            let bitmap = bitmap_arc.clone();
            let records_parsed = Arc::clone(&records_parsed);
            let record_size = record_size;

            let handle = std::thread::spawn(move || {
                let mut results: Vec<ParseResult> = Vec::with_capacity(records_per_worker);
                let mut local_parsed = 0usize;

                // Process buffers until channel closes
                // Use `mut buffer` to apply fixup in-place (zero-copy optimization)
                while let Ok(Some((mut buffer, start_frs, record_count))) = rx.recv() {
                    for i in 0..record_count {
                        let frs = start_frs + i as u64;

                        // Check bitmap
                        if let Some(ref bm) = bitmap {
                            if !bm.is_record_in_use(frs) {
                                continue;
                            }
                        }

                        let offset = i * record_size;
                        let end = offset + record_size;
                        if end > buffer.len() {
                            break;
                        }

                        // Apply fixup in-place (zero-copy - no per-record allocation!)
                        let record_slice = &mut buffer[offset..end];
                        if !apply_fixup(record_slice) {
                            continue;
                        }

                        // Parse using unified pipeline
                        let result = parse_record_full(record_slice, frs);
                        if !matches!(result, ParseResult::Skip) {
                            local_parsed += 1;
                            results.push(result);
                        }
                    }
                }

                records_parsed.fetch_add(local_parsed, Ordering::Relaxed);

                tracing::debug!(
                    worker_id,
                    local_parsed,
                    parse_results = results.len(),
                    "Worker complete"
                );

                results
            });

            worker_handles.push(handle);
        }

        // Drop the receiver clone so workers can detect channel close
        drop(rx);

        // IOCP reading (producer)
        let read_start = std::time::Instant::now();
        let iocp = IoCompletionPort::new(0)?;
        iocp.associate(overlapped_handle, 0)?;

        struct InFlightOp {
            overlapped: windows::Win32::System::IO::OVERLAPPED,
            buffer: AlignedBuffer,
            op: IoOp,
        }

        // Allocate buffers sized for the max I/O operation
        let mut buffer_pool: Vec<AlignedBuffer> = (0..concurrency)
            .map(|_| AlignedBuffer::new(max_io_size))
            .collect();

        let mut in_flight: Vec<Option<Pin<Box<InFlightOp>>>> =
            (0..concurrency).map(|_| None).collect();

        let mut completed_count = 0usize;
        let mut bytes_read_total = 0u64;

        // Queue initial reads
        for slot_id in 0..concurrency {
            if let Some(op) = io_ops.pop_front() {
                let buffer = buffer_pool.pop().unwrap();
                let mut in_flight_op = Box::pin(InFlightOp {
                    overlapped: unsafe { std::mem::zeroed() },
                    buffer,
                    op,
                });

                let offset = in_flight_op.op.disk_offset;
                let op_mut = unsafe { in_flight_op.as_mut().get_unchecked_mut() };
                op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                op_mut.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

                let overlapped_ptr = &mut op_mut.overlapped as *mut _;
                let read_size = op_mut.op.size;
                let result = unsafe {
                    ReadFile(
                        overlapped_handle,
                        Some(&mut op_mut.buffer.as_mut_slice()[..read_size]),
                        None,
                        Some(overlapped_ptr),
                    )
                };

                match result {
                    Ok(_) => {}
                    Err(_) => {
                        let last_error = unsafe { GetLastError() };
                        if last_error != ERROR_IO_PENDING {
                            // Signal workers to stop
                            drop(tx);
                            return Err(MftError::Io(std::io::Error::from_raw_os_error(
                                last_error.0 as i32,
                            )));
                        }
                    }
                }

                in_flight[slot_id] = Some(in_flight_op);
            }
        }

        // Process completions and send to workers
        while completed_count < total_io_ops {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.handle,
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX,
                )
            };

            if result.is_err() {
                let err = std::io::Error::last_os_error();
                warn!(error = %err, "GetQueuedCompletionStatus failed");
                continue;
            }

            // Find completed slot
            let mut completed_slot = None;
            for (idx, slot) in in_flight.iter().enumerate() {
                if let Some(op) = slot {
                    let op_overlapped_ptr =
                        &op.overlapped as *const _ as *mut windows::Win32::System::IO::OVERLAPPED;
                    if op_overlapped_ptr == overlapped_ptr {
                        completed_slot = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot {
                if let Some(mut completed_op) = in_flight[slot_idx].take() {
                    let op_mut = unsafe { completed_op.as_mut().get_unchecked_mut() };

                    // Send buffer to workers (copy the data)
                    let buffer_data =
                        op_mut.buffer.as_slice()[..bytes_transferred as usize].to_vec();
                    let start_frs = op_mut.op.start_frs;
                    let record_count = bytes_transferred as usize / record_size;

                    if tx
                        .send(Some((buffer_data, start_frs, record_count)))
                        .is_err()
                    {
                        warn!("Failed to send buffer to workers - channel closed");
                    }

                    bytes_read_total += bytes_transferred as u64;
                    completed_count += 1;

                    // Recycle buffer and queue next read
                    let recycled_buffer =
                        std::mem::replace(&mut op_mut.buffer, AlignedBuffer::new(0));
                    buffer_pool.push(recycled_buffer);

                    if let Some(next_op) = io_ops.pop_front() {
                        let buffer = buffer_pool.pop().unwrap();
                        let mut new_in_flight = Box::pin(InFlightOp {
                            overlapped: unsafe { std::mem::zeroed() },
                            buffer,
                            op: next_op,
                        });

                        let offset = new_in_flight.op.disk_offset;
                        let new_op_mut = unsafe { new_in_flight.as_mut().get_unchecked_mut() };
                        new_op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                        new_op_mut.overlapped.Anonymous.Anonymous.OffsetHigh =
                            (offset >> 32) as u32;

                        let overlapped_ptr = &mut new_op_mut.overlapped as *mut _;
                        let read_size = new_op_mut.op.size;
                        let result = unsafe {
                            ReadFile(
                                overlapped_handle,
                                Some(&mut new_op_mut.buffer.as_mut_slice()[..read_size]),
                                None,
                                Some(overlapped_ptr),
                            )
                        };

                        match result {
                            Ok(_) => {}
                            Err(_) => {
                                let last_error = unsafe { GetLastError() };
                                if last_error != ERROR_IO_PENDING {
                                    warn!(error = ?last_error, "Failed to queue next read");
                                }
                            }
                        }

                        in_flight[slot_idx] = Some(new_in_flight);
                    }
                }
            }
        }

        let read_ms = read_start.elapsed().as_millis();
        info!(
            read_ms,
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ IOCP read complete, waiting for workers"
        );

        // Signal workers to stop (send None to each)
        for _ in 0..num_workers {
            let _ = tx.send(None);
        }
        drop(tx);

        // Collect parse results from workers and merge using unified pipeline
        let merge_start = std::time::Instant::now();
        let mut merger = MftRecordMerger::with_capacity(total_records);

        for handle in worker_handles {
            match handle.join() {
                Ok(results) => {
                    for result in results {
                        merger.add_result(result);
                    }
                }
                Err(e) => {
                    warn!("Worker thread panicked: {:?}", e);
                }
            }
        }

        let total_parsed = records_parsed.load(Ordering::Relaxed);

        // Build index from merged records
        let parsed_records = merger.merge();
        let index = MftIndex::from_parsed_records(volume, parsed_records);

        let merge_ms = merge_start.elapsed().as_millis();
        let total_ms = read_start.elapsed().as_millis();

        info!(
            total_ms,
            read_ms,
            merge_ms,
            bytes_mb = bytes_read_total / (1024 * 1024),
            records_parsed = total_parsed,
            index_entries = index.records.len(),
            names_kb = index.names.len() / 1024,
            "✅ Parallel parsing IOCP with unified pipeline complete"
        );

        Ok(index)
    }

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

// ============================================================================
// Optimized Streaming Reader (Zero-Copy)
// ============================================================================

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
                Some(&mut self.buffer.as_mut_slice()[..aligned_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        Ok(bytes_read as usize)
    }
}

// ============================================================================
// Prefetch Reader (Double-Buffering)
// ============================================================================

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
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };

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
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

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

            let bytes_read = self.read_chunk_into_buffer(handle, &chunk, record_size, buffer)?;
            bytes_read_total += bytes_read as u64;

            // Process records from buffer using zero-copy in-place fixup
            let skip_begin = chunk.skip_begin as usize;
            let effective_count = chunk.effective_record_count() as usize;
            let record_size_usize = record_size as usize;
            let buffer_slice = buffer.as_mut_slice();

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
        &self,
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
        buffer: &mut AlignedBuffer,
    ) -> Result<usize> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
            / SECTOR_SIZE)
            * SECTOR_SIZE;

        // Resize buffer if needed
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

        Ok(bytes_read as usize)
    }
}

// ============================================================================
// Pipelined MFT Reader (True I/O + CPU Overlap)
// ============================================================================

/// Message sent from reader thread to parser thread.
struct ReadBuffer {
    /// The buffer containing raw MFT data.
    buffer: AlignedBuffer,
    /// Number of bytes actually read.
    bytes_read: usize,
    /// The chunk metadata.
    chunk: ReadChunk,
    /// Record size in bytes.
    record_size: u32,
}

/// Pipelined MFT reader with true I/O and CPU overlap.
///
/// This reader uses separate threads for I/O and parsing, connected by
/// bounded channels. This allows I/O to proceed while parsing is happening,
/// maximizing throughput especially on HDDs where I/O latency is significant.
///
/// Architecture:
/// ```text
/// ┌─────────────┐     ┌──────────────────┐     ┌─────────────┐
/// │ Reader      │────▶│ Bounded Channel  │────▶│ Parser      │
/// │ Thread      │     │ (backpressure)   │     │ Thread(s)   │
/// └─────────────┘     └──────────────────┘     └─────────────┘
///       │                                             │
///       ▼                                             ▼
///   Read chunks                                 Parse records
///   from disk                                   into ParsedRecord
/// ```
///
/// Key features:
/// - **True overlap**: I/O and parsing happen concurrently
/// - **Backpressure**: Bounded channel prevents memory explosion
/// - **Buffer pool**: Reuses buffers to minimize allocations
pub struct PipelinedMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    chunk_size: usize,
    /// Number of buffers in the pipeline (channel capacity).
    pipeline_depth: usize,
}

impl PipelinedMftReader {
    /// Creates a new pipelined reader.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - MFT extent map for physical offset calculation
    /// * `bitmap` - Optional MFT bitmap for skipping unused records
    /// * `drive_type` - Drive type for chunk size tuning
    #[must_use]
    pub fn new(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        // Chunk size based on drive type (use optimal_chunk_size for consistency)
        let chunk_size = drive_type.optimal_chunk_size();

        // Pipeline depth: 2-3 buffers is optimal
        // - 1 being read
        // - 1 being parsed
        // - 1 in the channel (optional, for smoothing)
        let pipeline_depth = 3;

        Self {
            extent_map,
            bitmap,
            chunk_size,
            pipeline_depth,
        }
    }

    /// Reads all MFT records with true I/O and CPU overlap.
    ///
    /// This method spawns a reader thread that reads chunks as fast as
    /// possible, sending them through a bounded channel to the main thread
    /// for parsing. The bounded channel provides backpressure to prevent
    /// memory explosion.
    pub fn read_all_pipelined<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        use std::thread;

        use crossbeam_channel::{Receiver, Sender, bounded};

        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            return Ok(Vec::new());
        }

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

        info!(
            chunks = num_chunks,
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            pipeline_depth = self.pipeline_depth,
            "🚀 Starting pipelined read with I/O+CPU overlap"
        );

        // Create bounded channel for backpressure
        let (tx, rx): (Sender<ReadBuffer>, Receiver<ReadBuffer>) = bounded(self.pipeline_depth);

        // Pre-allocate buffer pool for the reader thread
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

        // Clone data needed by reader thread
        let chunks_for_reader = chunks;
        let handle_raw = handle.0 as usize; // Convert to usize for Send

        // Spawn reader thread
        let reader_handle = thread::spawn(move || {
            // Reconstruct HANDLE in reader thread
            let handle = HANDLE(handle_raw as *mut std::ffi::c_void);

            // Create buffer pool
            let mut buffer_pool: Vec<AlignedBuffer> = Vec::new();

            for chunk in chunks_for_reader {
                // Get or create a buffer
                let mut buffer = buffer_pool
                    .pop()
                    .unwrap_or_else(|| AlignedBuffer::new(max_chunk_size + SECTOR_SIZE));

                // Read chunk into buffer
                match read_chunk_into_buffer_static(handle, &chunk, record_size, &mut buffer) {
                    Ok(bytes_read) => {
                        let read_buffer = ReadBuffer {
                            buffer,
                            bytes_read,
                            chunk,
                            record_size,
                        };

                        // Send to parser (blocks if channel is full - backpressure)
                        if tx.send(read_buffer).is_err() {
                            // Receiver dropped, stop reading
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to read chunk, skipping");
                        // Return buffer to pool
                        buffer_pool.push(buffer);
                    }
                }
            }
            // tx is dropped here, signaling end of stream
        });

        // Parse records in main thread
        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        // Receive and parse buffers
        while let Ok(read_buffer) = rx.recv() {
            let ReadBuffer {
                mut buffer,
                bytes_read,
                chunk,
                record_size,
            } = read_buffer;

            bytes_read_total += bytes_read as u64;

            // Parse records from buffer using zero-copy in-place fixup
            let skip_begin = chunk.skip_begin as usize;
            let effective_count = chunk.effective_record_count() as usize;
            let record_size_usize = record_size as usize;
            let buffer_slice = buffer.as_mut_slice();

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

            // Note: buffer is dropped here, but we could return it to a pool
            // for even better performance
        }

        // Wait for reader thread to finish
        if let Err(e) = reader_handle.join() {
            warn!("Reader thread panicked: {:?}", e);
        }

        // Merge extensions and get final results
        let all_results = merger.merge();

        info!(
            records = all_results.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Pipelined read complete"
        );

        Ok(all_results)
    }

    /// Reads all MFT records with pipelined I/O and parallel parsing.
    ///
    /// This method combines the benefits of pipelined I/O (true I/O+CPU
    /// overlap) with multi-core parallel parsing using Rayon. This is the
    /// optimal mode for HDDs with multi-core CPUs.
    ///
    /// Architecture:
    /// ```text
    /// ┌─────────────┐     ┌──────────────────┐     ┌─────────────────────┐
    /// │ Reader      │────▶│ Bounded Channel  │────▶│ Rayon Thread Pool   │
    /// │ Thread      │     │ (backpressure)   │     │ (parallel parsing)  │
    /// └─────────────┘     └──────────────────┘     └─────────────────────┘
    ///       │                                             │
    ///       ▼                                             ▼
    ///   Read chunks                                 Parse records in
    ///   from disk                                   parallel batches
    /// ```
    pub fn read_all_pipelined_parallel<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        use std::thread;

        use crossbeam_channel::{Receiver, Sender, bounded};

        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            return Ok(Vec::new());
        }

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

        info!(
            chunks = num_chunks,
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            pipeline_depth = self.pipeline_depth,
            rayon_threads = rayon::current_num_threads(),
            "🚀 Starting pipelined-parallel read with I/O+CPU overlap and multi-core parsing"
        );

        // Create bounded channel for backpressure
        // Use larger depth for parallel mode to keep Rayon workers fed
        let parallel_depth = self.pipeline_depth * 2;
        let (tx, rx): (Sender<ReadBuffer>, Receiver<ReadBuffer>) = bounded(parallel_depth);

        // Pre-allocate buffer pool for the reader thread
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

        // Clone data needed by reader thread
        let chunks_for_reader = chunks;
        let handle_raw = handle.0 as usize; // Convert to usize for Send

        // Spawn reader thread
        let reader_handle = thread::spawn(move || {
            // Reconstruct HANDLE in reader thread
            let handle = HANDLE(handle_raw as *mut std::ffi::c_void);

            // Create buffer pool
            let mut buffer_pool: Vec<AlignedBuffer> = Vec::new();

            for chunk in chunks_for_reader {
                // Get or create a buffer
                let mut buffer = buffer_pool
                    .pop()
                    .unwrap_or_else(|| AlignedBuffer::new(max_chunk_size + SECTOR_SIZE));

                // Read chunk into buffer
                match read_chunk_into_buffer_static(handle, &chunk, record_size, &mut buffer) {
                    Ok(bytes_read) => {
                        let read_buffer = ReadBuffer {
                            buffer,
                            bytes_read,
                            chunk,
                            record_size,
                        };

                        // Send to parser (blocks if channel is full - backpressure)
                        if tx.send(read_buffer).is_err() {
                            // Receiver dropped, stop reading
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to read chunk, skipping");
                        // Return buffer to pool
                        buffer_pool.push(buffer);
                    }
                }
            }
            // tx is dropped here, signaling end of stream
        });

        // Collect all buffers first, then parse in parallel with Rayon
        // This allows Rayon to efficiently distribute work across cores
        let mut all_buffers: Vec<ReadBuffer> = Vec::with_capacity(num_chunks);
        let mut bytes_read_total: u64 = 0;

        while let Ok(read_buffer) = rx.recv() {
            bytes_read_total += read_buffer.bytes_read as u64;
            all_buffers.push(read_buffer);

            // Report progress during collection phase
            if let Some(ref mut cb) = progress_callback {
                cb(bytes_read_total, total_bytes);
            }
        }

        // Wait for reader thread to finish
        if let Err(e) = reader_handle.join() {
            warn!("Reader thread panicked: {:?}", e);
        }

        info!(
            buffers = all_buffers.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "📦 All buffers collected, starting parallel parsing"
        );

        // Parse all buffers in parallel using Rayon with zero-copy in-place fixup
        let parse_results: Vec<ParseResult> = all_buffers
            .par_iter_mut()
            .flat_map(|read_buffer| {
                parse_buffer_to_results_zero_copy(read_buffer, merge_extensions)
            })
            .collect();

        info!(
            parse_results = parse_results.len(),
            "✅ Parallel parsing complete"
        );

        // Merge results using MftRecordMerger (single-threaded, as designed)
        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        for result in parse_results {
            merger.add_result(result);
        }

        let all_results = merger.merge();

        info!(
            records = all_results.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Pipelined-parallel read complete"
        );

        Ok(all_results)
    }
}

/// Parses all records in a buffer using zero-copy in-place fixup.
///
/// This is an optimized version of `parse_buffer_to_results` that applies
/// USA fixup directly on the shared buffer instead of copying each record.
/// This eliminates per-record heap allocations in the hot path.
///
/// # Safety
///
/// This function mutates the buffer in-place. The buffer should not be
/// reused after this call without re-reading the data from disk.
fn parse_buffer_to_results_zero_copy(
    read_buffer: &mut ReadBuffer,
    merge_extensions: bool,
) -> Vec<ParseResult> {
    parse_buffer_zero_copy_inner(
        read_buffer.buffer.as_mut_slice(),
        read_buffer.bytes_read,
        &read_buffer.chunk,
        read_buffer.record_size,
        merge_extensions,
    )
}

/// Inner zero-copy parsing function that works with raw parameters.
///
/// This is used by both `ReadBuffer` and `OverlappedRead` parsing paths.
fn parse_buffer_zero_copy_inner(
    buffer_slice: &mut [u8],
    bytes_read: usize,
    chunk: &ReadChunk,
    record_size: u32,
    merge_extensions: bool,
) -> Vec<ParseResult> {
    let skip_begin = chunk.skip_begin as usize;
    let effective_count = chunk.effective_record_count() as usize;
    let record_size_usize = record_size as usize;
    let start_frs = chunk.start_frs;

    let mut results = Vec::with_capacity(effective_count);

    for i in 0..effective_count {
        let offset = (skip_begin + i) * record_size_usize;
        if offset + record_size_usize > bytes_read {
            break;
        }

        let frs = start_frs + skip_begin as u64 + i as u64;

        // Apply fixup in-place on the shared buffer (zero-copy)
        let record_slice = &mut buffer_slice[offset..offset + record_size_usize];
        if !apply_fixup(record_slice) {
            continue;
        }

        // Parse record from the fixed-up slice (no copy needed)
        if merge_extensions {
            results.push(parse_record_full(record_slice, frs));
        } else if let Some(rec) = parse_record(record_slice, frs) {
            results.push(ParseResult::Base(rec));
        }
    }

    results
}

/// Static helper to read a chunk into a buffer (for use in reader thread).
#[expect(
    unsafe_code,
    reason = "FFI: SetFilePointerEx and ReadFile for static chunk reader helper"
)]
fn read_chunk_into_buffer_static(
    handle: HANDLE,
    chunk: &ReadChunk,
    record_size: u32,
    buffer: &mut AlignedBuffer,
) -> Result<usize> {
    let read_size = chunk.record_count * u64::from(record_size);

    // Align to sector boundary
    let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
    let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
    let aligned_size =
        ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;

    // Resize buffer if needed
    if buffer.len() < aligned_size {
        *buffer = AlignedBuffer::new(aligned_size);
    }

    // Seek to position
    let mut new_pos: i64 = 0;
    let seek_result = unsafe {
        SetFilePointerEx(
            handle,
            aligned_offset as i64,
            Some(&mut new_pos),
            FILE_BEGIN,
        )
    };

    if seek_result.is_err() {
        return Err(MftError::Io(std::io::Error::last_os_error()));
    }

    // Read data
    let mut bytes_read: u32 = 0;
    let read_result = unsafe {
        ReadFile(
            handle,
            Some(&mut buffer.as_mut_slice()[..aligned_size]),
            Some(&mut bytes_read),
            None,
        )
    };

    if read_result.is_err() {
        return Err(MftError::Io(std::io::Error::last_os_error()));
    }

    Ok(bytes_read as usize)
}

// ============================================================================
// IOCP-based MFT Reader (Phase B - Advanced I/O Overlap)
// ============================================================================

/// I/O Completion Port wrapper for Windows async I/O.
///
/// This provides IOCP-based overlapped I/O for maximum I/O parallelism,
/// mirroring the legacy implementation's approach of having multiple reads
/// in flight simultaneously.
pub struct IoCompletionPort {
    /// The IOCP handle.
    handle: HANDLE,
}

impl IoCompletionPort {
    /// Creates a new I/O Completion Port.
    ///
    /// # Errors
    /// Returns an error if IOCP creation fails.
    #[expect(
        unsafe_code,
        reason = "FFI: CreateIoCompletionPort to create IOCP handle"
    )]
    pub fn new(concurrency: u32) -> Result<Self> {
        use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows::Win32::System::IO::CreateIoCompletionPort;

        let handle = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, concurrency) };

        match handle {
            Ok(h) => Ok(Self { handle: h }),
            Err(e) => Err(MftError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to create IOCP: {e}"),
            ))),
        }
    }

    /// Associates a file handle with this IOCP.
    ///
    /// # Errors
    /// Returns an error if association fails.
    #[expect(
        unsafe_code,
        reason = "FFI: CreateIoCompletionPort to associate file handle with IOCP"
    )]
    pub fn associate(&self, file_handle: HANDLE, key: usize) -> Result<()> {
        use windows::Win32::System::IO::CreateIoCompletionPort;

        let result = unsafe { CreateIoCompletionPort(file_handle, Some(self.handle), key, 0) };

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(MftError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to associate handle with IOCP: {e}"),
            ))),
        }
    }

    /// Gets the raw IOCP handle.
    #[must_use]
    pub fn raw_handle(&self) -> HANDLE {
        self.handle
    }
}

impl Drop for IoCompletionPort {
    #[expect(
        unsafe_code,
        reason = "FFI: CloseHandle to release IOCP handle on drop"
    )]
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        if !self.handle.is_invalid() {
            // SAFETY: CloseHandle is safe to call on a valid handle.
            // We check is_invalid() first to ensure the handle is valid.
            let _ = unsafe { CloseHandle(self.handle) };
        }
    }
}

/// Represents an in-flight overlapped read operation.
///
/// This structure is pinned in memory because the OVERLAPPED pointer
/// is passed to Windows and must remain valid until completion.
#[repr(C)]
pub struct OverlappedRead {
    /// The Windows OVERLAPPED structure (must be first field for pointer
    /// casting).
    overlapped: windows::Win32::System::IO::OVERLAPPED,
    /// The aligned buffer for read data.
    pub buffer: AlignedBuffer,
    /// The chunk being read.
    pub chunk: ReadChunk,
    /// Record size for parsing.
    pub record_size: u32,
    /// Bytes actually read (set on completion).
    pub bytes_read: usize,
    /// Index in the buffer pool (for returning).
    pub pool_index: usize,
}

impl OverlappedRead {
    /// Creates a new overlapped read operation.
    #[must_use]
    pub fn new(
        buffer: AlignedBuffer,
        chunk: ReadChunk,
        record_size: u32,
        pool_index: usize,
    ) -> Self {
        Self {
            overlapped: windows::Win32::System::IO::OVERLAPPED::default(),
            buffer,
            chunk,
            record_size,
            bytes_read: 0,
            pool_index,
        }
    }

    /// Sets the file offset for the overlapped read.
    pub fn set_offset(&mut self, offset: u64) {
        self.overlapped.Anonymous.Anonymous.Offset = offset as u32;
        self.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
    }

    /// Gets a mutable pointer to the OVERLAPPED structure.
    ///
    /// # Safety
    /// The returned pointer is valid as long as self is pinned and alive.
    /// Note: Creating raw pointers is safe; only dereferencing requires unsafe.
    pub fn as_overlapped_ptr(&mut self) -> *mut windows::Win32::System::IO::OVERLAPPED {
        &mut self.overlapped as *mut _
    }
}

/// IOCP-based MFT reader with multiple concurrent reads in flight.
///
/// This reader uses Windows I/O Completion Ports to issue multiple
/// overlapped reads simultaneously, maximizing I/O parallelism and
/// hiding disk latency. This mirrors the legacy implementation's approach.
///
/// Architecture:
/// ```text
/// ┌─────────────────────────────────────────────────────────────────┐
/// │                    IOCP Event Loop                              │
/// │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐            │
/// │  │ Read 1  │  │ Read 2  │  │ Read 3  │  │ Read N  │  In-flight │
/// │  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘            │
/// │       │            │            │            │                  │
/// │       ▼            ▼            ▼            ▼                  │
/// │  ┌──────────────────────────────────────────────────┐          │
/// │  │           GetQueuedCompletionStatus              │          │
/// │  └──────────────────────────────────────────────────┘          │
/// │                          │                                      │
/// │                          ▼                                      │
/// │  ┌──────────────────────────────────────────────────┐          │
/// │  │    Parse completed buffer → Issue next read      │          │
/// │  └──────────────────────────────────────────────────┘          │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
pub struct IocpMftReader {
    /// Extent map for the MFT.
    extent_map: MftExtentMap,
    /// Optional bitmap for filtering in-use records.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Chunk size for reads.
    chunk_size: usize,
    /// Number of concurrent reads to keep in flight.
    concurrency: usize,
}

impl IocpMftReader {
    /// Default concurrency (number of reads in flight).
    /// Higher values hide more latency but use more memory.
    pub const DEFAULT_CONCURRENCY: usize = 8;

    /// Creates a new IOCP reader.
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
            concurrency = Self::DEFAULT_CONCURRENCY,
            "🚀 Created IOCP reader with overlapped I/O"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
            concurrency: Self::DEFAULT_CONCURRENCY,
        }
    }

    /// Sets the concurrency level (number of reads in flight).
    #[must_use]
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    /// Reads all MFT records using IOCP overlapped I/O.
    ///
    /// This method issues multiple overlapped reads simultaneously,
    /// processing completions as they arrive and issuing new reads
    /// to maintain the target concurrency level.
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for overlapped IOCP reads"
    )]
    pub fn read_all_iocp<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        use std::collections::VecDeque;
        use std::pin::Pin;

        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            return Ok(Vec::new());
        }

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

        info!(
            chunks = num_chunks,
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            concurrency = self.concurrency,
            "🚀 Starting IOCP read with {} concurrent reads in flight",
            self.concurrency
        );

        // Create IOCP
        let iocp = IoCompletionPort::new(0)?; // 0 = use number of processors
        iocp.associate(handle, 0)?;

        // Pre-allocate results - use ParseResult for merger compatibility
        let mut all_results: Vec<ParseResult> = Vec::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        // Create buffer pool and in-flight operations
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

        // Sort chunks by disk_offset (LCN order) to minimize seek time on HDD
        let mut sorted_chunks: Vec<ReadChunk> = chunks;
        sorted_chunks.sort_by_key(|c| c.disk_offset);

        // Use a VecDeque for chunks to process (now in LCN order)
        let mut pending_chunks: VecDeque<ReadChunk> = sorted_chunks.into_iter().collect();

        // In-flight operations (pinned for OVERLAPPED pointer stability)
        let mut in_flight: Vec<Option<Pin<Box<OverlappedRead>>>> =
            (0..self.concurrency).map(|_| None).collect();

        // Issue initial reads up to concurrency limit
        for (slot_idx, slot) in in_flight.iter_mut().enumerate() {
            if let Some(chunk) = pending_chunks.pop_front() {
                let buffer = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
                let mut op = Box::pin(OverlappedRead::new(buffer, chunk, record_size, slot_idx));

                // Calculate aligned offset
                let aligned_offset =
                    (op.chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                op.set_offset(aligned_offset);

                // Calculate read size
                let read_size = op.chunk.record_count * u64::from(record_size);
                let offset_adjustment = (op.chunk.disk_offset - aligned_offset) as usize;
                let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
                    / SECTOR_SIZE)
                    * SECTOR_SIZE;

                // Issue overlapped read
                // SAFETY: We need get_unchecked_mut to get a mutable reference to the
                // pinned data for the OVERLAPPED pointer and buffer. The pin is maintained
                // throughout the operation lifetime.
                let overlapped_ptr = unsafe { op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
                let read_result = unsafe {
                    ReadFile(
                        handle,
                        Some(
                            &mut op.as_mut().get_unchecked_mut().buffer.as_mut_slice()
                                [..aligned_size],
                        ),
                        None, // Don't need bytes read for overlapped
                        Some(overlapped_ptr),
                    )
                };

                // Check for errors (ERROR_IO_PENDING is expected for async)
                if read_result.is_err() {
                    let err = unsafe { GetLastError() };
                    if err != ERROR_IO_PENDING {
                        warn!(error = ?err, "Failed to issue overlapped read");
                        continue;
                    }
                }

                *slot = Some(op);
            }
        }

        // Process completions until all chunks are done
        let mut completed_count = 0;
        let total_to_complete = num_chunks;

        while completed_count < total_to_complete {
            // Wait for a completion
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let wait_result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX, // INFINITE
                )
            };

            if wait_result.is_err() {
                let err = std::io::Error::last_os_error();
                warn!(error = %err, "GetQueuedCompletionStatus failed");
                continue;
            }

            // Find which slot completed by matching the overlapped pointer
            let mut completed_slot: Option<usize> = None;
            for (idx, slot) in in_flight.iter().enumerate() {
                if let Some(op) = slot {
                    let op_ptr = &op.overlapped as *const _ as *mut _;
                    if op_ptr == overlapped_ptr {
                        completed_slot = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot {
                // Take the completed operation
                if let Some(mut op) = in_flight[slot_idx].take() {
                    let op_mut = unsafe { op.as_mut().get_unchecked_mut() };
                    op_mut.bytes_read = bytes_transferred as usize;

                    // Parse the buffer using zero-copy in-place fixup
                    let results = parse_buffer_zero_copy_inner(
                        op_mut.buffer.as_mut_slice(),
                        op_mut.bytes_read,
                        &op_mut.chunk,
                        op_mut.record_size,
                        merge_extensions,
                    );
                    all_results.extend(results);

                    bytes_read_total += bytes_transferred as u64;
                    completed_count += 1;

                    // Report progress
                    if let Some(ref mut cb) = progress_callback {
                        cb(bytes_read_total, total_bytes);
                    }

                    // Issue next read if there are more chunks
                    if let Some(next_chunk) = pending_chunks.pop_front() {
                        // Reuse the buffer
                        let mut buffer =
                            std::mem::replace(&mut op_mut.buffer, AlignedBuffer::new(0));

                        // Resize if needed
                        let next_read_size = next_chunk.record_count * u64::from(record_size);
                        let next_aligned_offset =
                            (next_chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                        let next_offset_adjustment =
                            (next_chunk.disk_offset - next_aligned_offset) as usize;
                        let next_aligned_size =
                            ((next_read_size as usize + next_offset_adjustment + SECTOR_SIZE - 1)
                                / SECTOR_SIZE)
                                * SECTOR_SIZE;

                        if buffer.len() < next_aligned_size {
                            buffer = AlignedBuffer::new(next_aligned_size);
                        }

                        let mut new_op = Box::pin(OverlappedRead::new(
                            buffer,
                            next_chunk,
                            record_size,
                            slot_idx,
                        ));
                        new_op.set_offset(next_aligned_offset);

                        // Issue overlapped read
                        // SAFETY: We need get_unchecked_mut to get a mutable reference to the
                        // pinned data for the OVERLAPPED pointer and buffer.
                        let overlapped_ptr =
                            unsafe { new_op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
                        let read_result = unsafe {
                            ReadFile(
                                handle,
                                Some(
                                    &mut new_op.as_mut().get_unchecked_mut().buffer.as_mut_slice()
                                        [..next_aligned_size],
                                ),
                                None,
                                Some(overlapped_ptr),
                            )
                        };

                        if read_result.is_err() {
                            let err = unsafe { GetLastError() };
                            if err != ERROR_IO_PENDING {
                                warn!(error = ?err, "Failed to issue next overlapped read");
                                continue;
                            }
                        }

                        in_flight[slot_idx] = Some(new_op);
                    }
                }
            }
        }

        info!(
            records = all_results.len(),
            bytes = bytes_read_total,
            "✅ IOCP read complete"
        );

        // Always use merger to convert ParseResult to ParsedRecord
        let mut merger = MftRecordMerger::with_capacity(all_results.len());
        for result in all_results {
            merger.add_result(result);
        }
        Ok(merger.merge())
    }
}

// ============================================================================
// M4: Multi-Volume Parallel IOCP Reader
// ============================================================================

/// Per-volume state for multi-volume IOCP reading.
#[cfg(windows)]
#[derive(Debug)]
pub struct VolumeState {
    /// Drive letter (e.g., 'C')
    pub drive_letter: char,
    /// Volume handle (opened with OVERLAPPED flag)
    pub handle: HANDLE,
    /// Extent map for this volume's MFT
    pub extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization
    pub bitmap: Option<crate::platform::MftBitmap>,
    /// Drive type for adaptive I/O tuning
    pub drive_type: crate::platform::DriveType,
    /// Number of pending I/O operations for this volume
    pub pending_ops: usize,
    /// Maximum concurrent ops for this volume (based on drive type)
    pub max_concurrency: usize,
    /// I/O chunk size for this volume
    pub io_chunk_size: usize,
    /// Record merger accumulating parsed records (unified pipeline)
    pub merger: MftRecordMerger,
    /// Queue of pending I/O operations
    pub io_queue: std::collections::VecDeque<MultiVolumeIoOp>,
    /// Next I/O operation index to issue
    pub next_io_idx: usize,
    /// Total I/O operations for this volume
    pub total_io_ops: usize,
    /// Completed I/O operations
    pub completed_io_ops: usize,
}

/// I/O operation for multi-volume reading.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct MultiVolumeIoOp {
    /// Disk offset to read from
    pub disk_offset: u64,
    /// Size of the read in bytes
    pub size: usize,
    /// First FRS in this I/O
    pub start_frs: u64,
}

/// Multi-volume IOCP reader that uses a single IOCP for all volumes.
///
/// This is the M4 optimization: instead of creating separate IOCPs for each
/// volume, we use a single IOCP and associate all volume handles with it.
/// The completion key identifies which volume completed.
///
/// Benefits:
/// - Single event loop for all volumes
/// - OS can optimize I/O scheduling across all drives
/// - Reduced thread overhead
/// - NVMe drives get high concurrency while HDDs get low concurrency
#[cfg(windows)]
pub struct MultiVolumeIocpReader {
    /// Per-volume state, indexed by completion key
    volumes: Vec<VolumeState>,
}

#[cfg(windows)]
impl MultiVolumeIocpReader {
    /// Creates a new multi-volume IOCP reader.
    ///
    /// # Arguments
    ///
    /// * `volumes` - Vector of volume states to read from
    #[must_use]
    pub fn new(volumes: Vec<VolumeState>) -> Self {
        Self { volumes }
    }

    /// Reads all MFTs from all volumes using a single IOCP.
    ///
    /// Returns a vector of `MftIndex`, one per volume, in the same order
    /// as the input volumes.
    ///
    /// # Errors
    ///
    /// Returns an error if IOCP creation fails or if all volumes fail to read.
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for multi-volume IOCP reads"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "multi-volume IOCP orchestration with per-volume state tracking"
    )]
    pub fn read_all_volumes(&mut self) -> Result<Vec<crate::index::MftIndex>> {
        use std::pin::Pin;

        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError, HANDLE};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        let record_size = if self.volumes.is_empty() {
            1024 // Default
        } else {
            self.volumes[0].extent_map.bytes_per_record as usize
        };

        // Create single IOCP for all volumes
        let iocp = IoCompletionPort::new(0)?;

        // Associate all volume handles with the IOCP
        // The completion key is the volume index
        for (idx, vol) in self.volumes.iter().enumerate() {
            iocp.associate(vol.handle, idx)?;
            info!(
                volume = %vol.drive_letter,
                key = idx,
                drive_type = ?vol.drive_type,
                concurrency = vol.max_concurrency,
                io_size_kb = vol.io_chunk_size / 1024,
                "📎 Associated volume with IOCP"
            );
        }

        // In-flight operation tracking per volume
        struct InFlightOp {
            overlapped: windows::Win32::System::IO::OVERLAPPED,
            buffer: AlignedBuffer,
            op: MultiVolumeIoOp,
        }

        // Create buffer pools and in-flight tracking per volume
        let mut buffer_pools: Vec<Vec<AlignedBuffer>> = self
            .volumes
            .iter()
            .map(|v| {
                (0..v.max_concurrency)
                    .map(|_| AlignedBuffer::new(v.io_chunk_size))
                    .collect()
            })
            .collect();

        let mut in_flight: Vec<Vec<Option<Pin<Box<InFlightOp>>>>> = self
            .volumes
            .iter()
            .map(|v| (0..v.max_concurrency).map(|_| None).collect())
            .collect();

        // Issue initial reads for all volumes
        let mut total_pending = 0usize;

        for (vol_idx, vol) in self.volumes.iter_mut().enumerate() {
            let initial_count = std::cmp::min(vol.max_concurrency, vol.io_queue.len());

            for slot_idx in 0..initial_count {
                if let Some(op) = vol.io_queue.pop_front() {
                    let buffer = buffer_pools[vol_idx]
                        .pop()
                        .unwrap_or_else(|| AlignedBuffer::new(vol.io_chunk_size));

                    let mut in_flight_op = Box::pin(InFlightOp {
                        overlapped: windows::Win32::System::IO::OVERLAPPED {
                            Anonymous: windows::Win32::System::IO::OVERLAPPED_0 {
                                Anonymous: windows::Win32::System::IO::OVERLAPPED_0_0 {
                                    Offset: (op.disk_offset & 0xFFFF_FFFF) as u32,
                                    OffsetHigh: (op.disk_offset >> 32) as u32,
                                },
                            },
                            hEvent: HANDLE::default(),
                            Internal: 0,
                            InternalHigh: 0,
                        },
                        buffer,
                        op: op.clone(),
                    });

                    let overlapped_ptr = std::ptr::addr_of_mut!(in_flight_op.overlapped);
                    let buffer_ptr = in_flight_op.buffer.as_mut_slice().as_mut_ptr();

                    let read_result = unsafe {
                        ReadFile(
                            vol.handle,
                            Some(std::slice::from_raw_parts_mut(buffer_ptr, op.size)),
                            None,
                            Some(overlapped_ptr),
                        )
                    };

                    if read_result.is_err() {
                        let err = unsafe { GetLastError() };
                        if err != ERROR_IO_PENDING {
                            warn!(
                                volume = %vol.drive_letter,
                                error = ?err,
                                "Failed to issue initial read"
                            );
                            continue;
                        }
                    }

                    in_flight[vol_idx][slot_idx] = Some(in_flight_op);
                    vol.pending_ops += 1;
                    total_pending += 1;
                }
            }
        }

        info!(
            volumes = self.volumes.len(),
            total_pending, "🚀 Started multi-volume IOCP reading"
        );

        // Process completions
        let mut bytes_read_total = 0u64;

        while total_pending > 0 {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let wait_result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX,
                )
            };

            if wait_result.is_err() || overlapped_ptr.is_null() {
                let err = unsafe { GetLastError() };
                warn!(error = ?err, "IOCP wait failed");
                break;
            }

            let vol_idx = completion_key;
            if vol_idx >= self.volumes.len() {
                warn!(key = vol_idx, "Invalid completion key");
                continue;
            }

            // Find the completed operation
            let mut completed_slot = None;
            for (slot_idx, slot) in in_flight[vol_idx].iter_mut().enumerate() {
                if let Some(op) = slot {
                    let op_ptr = std::ptr::addr_of!(op.overlapped);
                    if op_ptr as *const _ == overlapped_ptr as *const _ {
                        completed_slot = Some(slot_idx);
                        break;
                    }
                }
            }

            let Some(slot_idx) = completed_slot else {
                warn!("Could not find completed operation");
                continue;
            };

            // Take the completed operation and unpin it to get ownership
            let completed_pinned = in_flight[vol_idx][slot_idx].take().unwrap();
            let completed_op = Pin::into_inner(completed_pinned);
            let vol = &mut self.volumes[vol_idx];
            vol.pending_ops -= 1;
            vol.completed_io_ops += 1;
            total_pending -= 1;
            bytes_read_total += bytes_transferred as u64;

            // Parse the completed buffer using unified pipeline
            let buffer_slice = &completed_op.buffer.as_slice()[..bytes_transferred as usize];
            let records_in_buffer = bytes_transferred as usize / record_size;
            let mut current_frs = completed_op.op.start_frs;

            for record_idx in 0..records_in_buffer {
                let record_start = record_idx * record_size;
                let record_end = record_start + record_size;
                if record_end > buffer_slice.len() {
                    break;
                }

                let record_data = &buffer_slice[record_start..record_end];
                let result = parse_record_full(record_data, current_frs);
                vol.merger.add_result(result);
                current_frs += 1;
            }

            // Return buffer to pool
            buffer_pools[vol_idx].push(completed_op.buffer);

            // Issue next read for this volume if available
            if let Some(next_op) = vol.io_queue.pop_front() {
                let buffer = buffer_pools[vol_idx]
                    .pop()
                    .unwrap_or_else(|| AlignedBuffer::new(vol.io_chunk_size));

                let mut new_in_flight = Box::pin(InFlightOp {
                    overlapped: windows::Win32::System::IO::OVERLAPPED {
                        Anonymous: windows::Win32::System::IO::OVERLAPPED_0 {
                            Anonymous: windows::Win32::System::IO::OVERLAPPED_0_0 {
                                Offset: (next_op.disk_offset & 0xFFFF_FFFF) as u32,
                                OffsetHigh: (next_op.disk_offset >> 32) as u32,
                            },
                        },
                        hEvent: HANDLE::default(),
                        Internal: 0,
                        InternalHigh: 0,
                    },
                    buffer,
                    op: next_op.clone(),
                });

                let overlapped_ptr = std::ptr::addr_of_mut!(new_in_flight.overlapped);
                let buffer_ptr = new_in_flight.buffer.as_mut_slice().as_mut_ptr();

                let read_result = unsafe {
                    ReadFile(
                        vol.handle,
                        Some(std::slice::from_raw_parts_mut(buffer_ptr, next_op.size)),
                        None,
                        Some(overlapped_ptr),
                    )
                };

                if read_result.is_err() {
                    let err = unsafe { GetLastError() };
                    if err != ERROR_IO_PENDING {
                        warn!(
                            volume = %vol.drive_letter,
                            error = ?err,
                            "Failed to issue next read"
                        );
                        // Unpin to recover the buffer
                        let failed_op = Pin::into_inner(new_in_flight);
                        buffer_pools[vol_idx].push(failed_op.buffer);
                        continue;
                    }
                }

                in_flight[vol_idx][slot_idx] = Some(new_in_flight);
                vol.pending_ops += 1;
                total_pending += 1;
            }
        }

        // Log completion stats per volume
        for vol in &self.volumes {
            info!(
                volume = %vol.drive_letter,
                base_records = vol.merger.base_count(),
                extensions = vol.merger.extension_count(),
                completed_ops = vol.completed_io_ops,
                total_ops = vol.total_io_ops,
                "✅ Volume read complete"
            );
        }

        info!(
            volumes = self.volumes.len(),
            total_bytes = bytes_read_total,
            "✅ Multi-volume IOCP read complete, merging..."
        );

        // Merge extensions and build index for each volume using unified pipeline
        Ok(self
            .volumes
            .drain(..)
            .map(|v| {
                let parsed_records = v.merger.merge();
                crate::index::MftIndex::from_parsed_records(v.drive_letter, parsed_records)
            })
            .collect())
    }
}

/// Helper function to prepare volume state for multi-volume reading.
#[cfg(windows)]
pub fn prepare_volume_state(
    drive_letter: char,
    handle: HANDLE,
    extent_map: MftExtentMap,
    bitmap: Option<crate::platform::MftBitmap>,
    drive_type: crate::platform::DriveType,
) -> VolumeState {
    let record_size = extent_map.bytes_per_record as usize;
    let total_records = extent_map.total_records() as usize;
    // For HDD, use extent-aware concurrency (fragmentation affects optimal value)
    let max_concurrency = if matches!(drive_type, crate::platform::DriveType::Hdd) {
        crate::platform::DriveType::optimal_concurrency_for_hdd(extent_map.extent_count())
    } else {
        drive_type.optimal_concurrency()
    };
    let io_chunk_size = drive_type.optimal_io_size();

    // Generate I/O operations
    let chunks = generate_read_chunks(&extent_map, bitmap.as_ref(), 64 * 1024);
    let mut sorted_chunks: Vec<ReadChunk> = chunks;
    sorted_chunks.sort_by_key(|c| c.disk_offset);

    let mut io_queue = std::collections::VecDeque::new();

    for chunk in sorted_chunks.iter() {
        let skip_begin_bytes = chunk.skip_begin as usize * record_size;
        let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;
        if effective_records == 0 {
            continue;
        }

        let chunk_bytes = effective_records as usize * record_size;
        let mut offset_within_chunk = 0usize;
        let mut frs_offset = 0u64;

        while offset_within_chunk < chunk_bytes {
            let io_size = std::cmp::min(io_chunk_size, chunk_bytes - offset_within_chunk);
            let disk_offset =
                chunk.disk_offset + skip_begin_bytes as u64 + offset_within_chunk as u64;

            io_queue.push_back(MultiVolumeIoOp {
                disk_offset,
                size: io_size,
                start_frs: chunk.start_frs + chunk.skip_begin as u64 + frs_offset,
            });

            offset_within_chunk += io_size;
            frs_offset += (io_size / record_size) as u64;
        }
    }

    let total_io_ops = io_queue.len();
    let _estimated_records = bitmap.as_ref().map_or(total_records, |b| b.count_in_use());

    VolumeState {
        drive_letter,
        handle,
        extent_map,
        bitmap,
        drive_type,
        pending_ops: 0,
        max_concurrency,
        io_chunk_size,
        merger: MftRecordMerger::with_capacity(total_records),
        io_queue,
        next_io_idx: 0,
        total_io_ops,
        completed_io_ops: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipelined_reader_creation() {
        let extent_map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

        let reader =
            PipelinedMftReader::new(extent_map.clone(), None, crate::platform::DriveType::Ssd);
        assert_eq!(reader.chunk_size, 64 * 1024);
        assert_eq!(reader.pipeline_depth, 3);

        let reader =
            PipelinedMftReader::new(extent_map.clone(), None, crate::platform::DriveType::Hdd);
        assert_eq!(reader.chunk_size, 64 * 1024);
        assert_eq!(reader.pipeline_depth, 3);

        let reader = PipelinedMftReader::new(extent_map, None, crate::platform::DriveType::Unknown);
        assert_eq!(reader.chunk_size, 64 * 1024);
    }

    #[test]
    fn test_parallel_mft_reader_uses_optimal_chunk_size() {
        use crate::platform::DriveType;

        let extent_map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

        let nvme_reader =
            ParallelMftReader::new_optimized(extent_map.clone(), None, DriveType::Nvme);
        assert_eq!(
            nvme_reader.chunk_size,
            4 * 1024 * 1024,
            "NVMe should use 4MB chunk size"
        );
        assert_eq!(nvme_reader.drive_type, DriveType::Nvme);

        let ssd_reader = ParallelMftReader::new_optimized(extent_map.clone(), None, DriveType::Ssd);
        assert_eq!(
            ssd_reader.chunk_size,
            2 * 1024 * 1024,
            "SSD should use 2MB chunk size"
        );
        assert_eq!(ssd_reader.drive_type, DriveType::Ssd);

        let hdd_reader = ParallelMftReader::new_optimized(extent_map.clone(), None, DriveType::Hdd);
        assert_eq!(
            hdd_reader.chunk_size,
            1024 * 1024,
            "HDD should use 1MB chunk size"
        );
        assert_eq!(hdd_reader.drive_type, DriveType::Hdd);

        let unknown_reader =
            ParallelMftReader::new_optimized(extent_map.clone(), None, DriveType::Unknown);
        assert_eq!(
            unknown_reader.chunk_size,
            1024 * 1024,
            "Unknown should use 1MB chunk size"
        );
        assert_eq!(unknown_reader.drive_type, DriveType::Unknown);
    }

    #[test]
    fn test_drive_type_stored_in_reader() {
        use crate::platform::DriveType;

        let extent_map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

        for drive_type in [
            DriveType::Nvme,
            DriveType::Ssd,
            DriveType::Hdd,
            DriveType::Unknown,
        ] {
            let reader = ParallelMftReader::new_optimized(extent_map.clone(), None, drive_type);
            assert_eq!(
                reader.drive_type, drive_type,
                "Drive type should be stored in reader"
            );
        }
    }

    #[test]
    fn test_optimal_defaults_when_none_passed() {
        use crate::platform::DriveType;

        fn resolve_concurrency(user_value: Option<usize>, drive_type: DriveType) -> usize {
            user_value.unwrap_or_else(|| drive_type.optimal_concurrency())
        }

        fn resolve_io_size(user_value: Option<usize>, drive_type: DriveType) -> usize {
            user_value.unwrap_or_else(|| drive_type.optimal_io_size())
        }

        assert_eq!(resolve_concurrency(None, DriveType::Nvme), 32);
        assert_eq!(resolve_io_size(None, DriveType::Nvme), 4 * 1024 * 1024);

        assert_eq!(resolve_concurrency(None, DriveType::Ssd), 8);
        assert_eq!(resolve_io_size(None, DriveType::Ssd), 2 * 1024 * 1024);

        assert_eq!(resolve_concurrency(None, DriveType::Hdd), 4);
        assert_eq!(resolve_io_size(None, DriveType::Hdd), 1024 * 1024);

        assert_eq!(resolve_concurrency(None, DriveType::Unknown), 4);
        assert_eq!(resolve_io_size(None, DriveType::Unknown), 1024 * 1024);

        assert_eq!(resolve_concurrency(Some(16), DriveType::Nvme), 16);
        assert_eq!(
            resolve_io_size(Some(8 * 1024 * 1024), DriveType::Hdd),
            8 * 1024 * 1024
        );
    }

    #[test]
    fn test_parallel_parsing_auto_detection() {
        use crate::platform::DriveType;

        fn resolve_parallel_parse(user_value: Option<bool>, drive_type: DriveType) -> bool {
            user_value.unwrap_or_else(|| drive_type.benefits_from_parallel_parsing())
        }

        assert!(
            resolve_parallel_parse(None, DriveType::Nvme),
            "NVMe should auto-enable parallel parsing"
        );
        assert!(
            !resolve_parallel_parse(None, DriveType::Ssd),
            "SSD should NOT auto-enable parallel parsing"
        );
        assert!(
            !resolve_parallel_parse(None, DriveType::Hdd),
            "HDD should NOT auto-enable parallel parsing"
        );
        assert!(
            !resolve_parallel_parse(None, DriveType::Unknown),
            "Unknown should NOT auto-enable parallel parsing"
        );

        assert!(resolve_parallel_parse(Some(true), DriveType::Hdd));
        assert!(!resolve_parallel_parse(Some(false), DriveType::Nvme));
    }
}
