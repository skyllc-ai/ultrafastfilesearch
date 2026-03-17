//! Parallel reader implementations and strategy entrypoints.

// Parallel reader module with complex timing and coordination
#![allow(clippy::all, clippy::nursery, clippy::pedantic)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

#[cfg(windows)]
pub(super) use super::iocp::IoCompletionPort;

#[cfg(windows)]
mod bulk;
#[cfg(windows)]
mod bulk_iocp;
#[cfg(windows)]
mod chunk;
#[cfg(windows)]
mod columns;
#[cfg(windows)]
mod sliding_window;
#[cfg(windows)]
mod timing;
#[cfg(windows)]
mod to_index;
#[cfg(windows)]
mod to_index_parallel;

#[cfg(test)]
mod tests;

// Chaos reader is available outside of tests for CLI integration
mod tests_chaos;
pub use tests_chaos::{ChaosMftReader, ChaosStrategy};

/// Timing breakdown for read and parse operations.
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
///
/// Windows-only: requires HANDLE for live MFT reading.
#[cfg(windows)]
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

#[cfg(windows)]
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
    /// **LEGACY MULTI-PASS PIPELINE:** This function uses
    /// `parse_record_full → MftRecordMerger → Vec<ParsedRecord>`.
    /// The hot path (`SlidingIocpInline`) uses direct-to-index parsing instead.
    /// This function is used by legacy read modes (`Parallel`, `Auto` when not
    /// inline).
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
}
