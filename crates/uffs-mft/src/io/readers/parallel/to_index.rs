//! Sliding-window direct-to-index reader path.

use super::*;

impl ParallelMftReader {
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
    /// # I/O Overlap Architecture
    ///
    /// This function achieves true I/O-compute overlap using IOCP's sliding
    /// window:
    ///
    /// 1. **Multiple I/O in flight**: Maintains 2-8 concurrent I/O operations
    ///    (adaptive based on drive type). While one operation completes, others
    ///    are still reading from disk.
    ///
    /// 2. **Inline parsing**: When `GetQueuedCompletionStatus` returns, the
    ///    completion handler immediately applies fixup and parses records
    ///    directly into the index. Critically, this parse happens while other
    ///    I/O operations remain in flight.
    ///
    /// 3. **Immediate requeue**: After parsing completes, the buffer is
    ///    recycled and the next I/O operation is queued immediately,
    ///    maintaining the sliding window.
    ///
    /// Parse time per chunk is typically <1ms, so parsing on the IOCP
    /// completion thread is optimal—it avoids thread synchronization
    /// overhead and maintains cache locality. The overlap comes from having
    /// multiple chunks in flight, not from multi-threaded parsing.
    ///
    /// Timing instrumentation (added for profiling) logs `wait_ms`, `parse_ms`,
    /// and `overlap_pct` to quantify how much parse work was hidden behind
    /// I/O latency.
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
        use std::time::Instant;

        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        use crate::index::MftIndex;
        use crate::platform::{
            IOCP_WAIT_COMPLETION_DEADLINE, IOCP_WAIT_POLL_INTERVAL_MS, WAIT_TIMEOUT_ERROR_CODE,
            classify_wait_error_code, wait_deadline_exceeded,
        };

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
        let (estimated_records, max_frs) = if let Some(ref bm) = self.bitmap {
            (bm.count_in_use(), bm.max_frs_in_use())
        } else {
            // No bitmap: use total records as both count and max FRS
            (total_records, total_records.saturating_sub(1) as u64)
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
            max_frs,
            bytes_to_read_mb = total_bytes_to_read / (1024 * 1024),
            max_io_size_kb = max_io_size / 1024,
            direct_io = use_direct_chunk_io,
            "📊 Generated I/O operations for inline parsing"
        );

        // Pre-allocate MftIndex with C++-matching ratios to eliminate resizing during
        // parse
        let mut index = MftIndex::with_capacity_optimized(volume, estimated_records, max_frs);

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
                let Some(buffer) = buffer_pool.pop() else {
                    return Err(MftError::InvalidData(
                        "I/O buffer pool exhausted while queuing inline-parse reads".to_owned(),
                    ));
                };
                let mut in_flight_op = Box::pin(InFlightOp {
                    // SAFETY: `OVERLAPPED` is a plain Windows FFI struct and an
                    // all-zero value is the required initial state before offsets are set.
                    overlapped: unsafe { std::mem::zeroed() },
                    buffer,
                    op,
                });

                let offset = in_flight_op.op.disk_offset;
                // SAFETY: The pinned allocation remains in place while the I/O is in
                // flight; this only projects a mutable reference without moving it.
                let op_mut = unsafe { in_flight_op.as_mut().get_unchecked_mut() };
                op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                op_mut.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

                let overlapped_ptr = &mut op_mut.overlapped as *mut _;
                let read_size = op_mut.op.size;
                // SAFETY: `overlapped_handle` is a live overlapped-capable handle,
                // the buffer slice spans `read_size` writable bytes in the pinned op,
                // and `overlapped_ptr` points into that same pinned operation.
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
                        // SAFETY: `GetLastError` reads the calling thread's last-error
                        // slot and does not dereference any Rust pointers.
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
        let mut last_completion_at = Instant::now();

        // Timing instrumentation for I/O overlap analysis
        let mut total_wait_time_ns = 0u64;
        let mut total_parse_time_ns = 0u64;

        const WAIT_OPERATION: &str = "read_all_sliding_window_iocp_to_index";

        while completed_count < total_io_ops {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            // Time I/O wait (GetQueuedCompletionStatus)
            let wait_start = Instant::now();

            // SAFETY: `iocp.raw_handle()` is a live completion port and all out-pointers
            // reference writable stack storage for the duration of the wait.
            let result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    IOCP_WAIT_POLL_INTERVAL_MS,
                )
            };

            total_wait_time_ns += wait_start.elapsed().as_nanos() as u64;

            if result.is_err() {
                let last_error = unsafe { GetLastError() };
                if last_error.0 == WAIT_TIMEOUT_ERROR_CODE {
                    let stalled_for = last_completion_at.elapsed();
                    if stalled_for >= IOCP_WAIT_COMPLETION_DEADLINE {
                        return Err(wait_deadline_exceeded(
                            WAIT_OPERATION,
                            stalled_for,
                            format!(
                                "GetQueuedCompletionStatus observed no inline IOCP completions after {completed_count} of {total_io_ops} reads"
                            ),
                        ));
                    }
                    continue;
                }

                return Err(classify_wait_error_code(
                    WAIT_OPERATION,
                    last_error.0,
                    format!(
                        "GetQueuedCompletionStatus failed after {completed_count} of {total_io_ops} inline IOCP reads completed"
                    ),
                ));
            }

            if overlapped_ptr.is_null() {
                return Err(MftError::WaitFailed {
                    operation: WAIT_OPERATION,
                    reason: format!(
                        "GetQueuedCompletionStatus returned a null OVERLAPPED pointer after {completed_count} of {total_io_ops} inline IOCP reads completed"
                    ),
                });
            }

            last_completion_at = Instant::now();

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
                    // SAFETY: The `Pin<Box<_>>` is still pinned in this scope; we
                    // only project a mutable reference without moving the allocation.
                    let op_mut = unsafe { completed_op.as_mut().get_unchecked_mut() };

                    // Time parse phase (fixup + parse_record_to_index)
                    let parse_start = Instant::now();

                    // DIRECT-TO-INDEX: parse records directly into MftIndex
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

                        // Parse directly into index (single-pass, no intermediates)
                        if parse_record_to_index(record_slice, frs, &mut index) {
                            records_parsed += 1;
                        }
                    }

                    total_parse_time_ns += parse_start.elapsed().as_nanos() as u64;

                    bytes_read_total += bytes_transferred as u64;
                    completed_count += 1;

                    // Recycle buffer and queue next read
                    let recycled_buffer =
                        std::mem::replace(&mut op_mut.buffer, AlignedBuffer::new(0));
                    buffer_pool.push(recycled_buffer);

                    if let Some(next_op) = io_ops.pop_front() {
                        let Some(buffer) = buffer_pool.pop() else {
                            return Err(MftError::InvalidData(
                                "I/O buffer pool exhausted while recycling inline-parse reads"
                                    .to_owned(),
                            ));
                        };
                        let mut new_in_flight = Box::pin(InFlightOp {
                            // SAFETY: `OVERLAPPED` is a plain Windows FFI struct and an
                            // all-zero value is the required initial state before offsets are set.
                            overlapped: unsafe { std::mem::zeroed() },
                            buffer,
                            op: next_op,
                        });

                        let offset = new_in_flight.op.disk_offset;
                        // SAFETY: The pinned allocation remains in place while the I/O
                        // is in flight; this only projects a mutable reference.
                        let new_op_mut = unsafe { new_in_flight.as_mut().get_unchecked_mut() };
                        new_op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                        new_op_mut.overlapped.Anonymous.Anonymous.OffsetHigh =
                            (offset >> 32) as u32;

                        let overlapped_ptr = &mut new_op_mut.overlapped as *mut _;
                        let read_size = new_op_mut.op.size;
                        // SAFETY: `overlapped_handle` is a live overlapped-capable
                        // handle, the buffer slice spans `read_size` writable bytes in
                        // the pinned op, and `overlapped_ptr` points into that op.
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
                                // SAFETY: `GetLastError` reads the calling thread's
                                // last-error slot and does not dereference Rust pointers.
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

        let total_ms = read_start.elapsed().as_millis();
        let wait_ms = total_wait_time_ns / 1_000_000;
        let parse_ms = total_parse_time_ns / 1_000_000;

        // Calculate overlap efficiency: if wait_ms + parse_ms > total_ms,
        // then we had effective overlap (parse happened while other I/O was in flight)
        let overlap_pct = if total_ms > 0 {
            ((wait_ms + parse_ms).saturating_sub(total_ms) as f64 / total_ms as f64) * 100.0
        } else {
            0.0
        };

        info!(
            total_ms,
            wait_ms,
            parse_ms,
            overlap_pct = format!("{:.1}%", overlap_pct),
            bytes_mb = bytes_read_total / (1024 * 1024),
            records_parsed,
            index_entries = index.records.len(),
            "✅ Sliding window IOCP with direct-to-index parsing complete (I/O overlap analysis)"
        );

        Ok(index)
    }
}
