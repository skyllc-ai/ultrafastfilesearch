// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Pipelined reader implementation.

use super::zero_copy::parse_buffer_zero_copy_inner;
use super::*;

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
                parse_buffer_zero_copy_inner(
                    read_buffer.buffer.as_mut_slice(),
                    read_buffer.bytes_read,
                    &read_buffer.chunk,
                    read_buffer.record_size,
                    merge_extensions,
                )
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
    // SAFETY: `handle` is a live volume handle and `new_pos` is valid writable
    // storage for the duration of the seek.
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
    // SAFETY: `handle` is live, the aligned buffer slice spans `aligned_size`
    // writable bytes, and `bytes_read` is a valid out-parameter.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipelined_reader_creation() {
        // Expected chunk sizes must track `DriveType::optimal_chunk_size`
        // in `crates/uffs-mft/src/platform/system.rs`.  The previous
        // hardcoded `64 * 1024` was stale from an early prototype; the
        // test never caught the drift because `mod pipelined` is
        // `#[cfg(windows)]`-gated in `readers/mod.rs` and had never run
        // in CI before `preview-artifacts.yml`'s `smoke-windows` job
        // (first ran 2026-04-24, PR #52 run 24873800282).  See tracking
        // issue #54 and `docs/architecture/dev-flow-implementation-plan.md`
        // §10.5 bug #6 for the full diagnostic.
        use crate::platform::DriveType;

        let extent_map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

        let reader = PipelinedMftReader::new(extent_map.clone(), None, DriveType::Ssd);
        assert_eq!(reader.chunk_size, DriveType::Ssd.optimal_chunk_size());
        assert_eq!(reader.pipeline_depth, 3);

        let reader = PipelinedMftReader::new(extent_map.clone(), None, DriveType::Hdd);
        assert_eq!(reader.chunk_size, DriveType::Hdd.optimal_chunk_size());
        assert_eq!(reader.pipeline_depth, 3);

        let reader = PipelinedMftReader::new(extent_map, None, DriveType::Unknown);
        assert_eq!(reader.chunk_size, DriveType::Unknown.optimal_chunk_size());
    }
}
