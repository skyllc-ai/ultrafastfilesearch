//! Parallel direct-to-index reader path.

use super::*;

impl ParallelMftReader {
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
        let num_workers = num_workers
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |p| p.get()));

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
                let Some(buffer) = buffer_pool.pop() else {
                    drop(tx);
                    return Err(MftError::InvalidData(
                        "I/O buffer pool exhausted while queuing worker reads".to_owned(),
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

            // SAFETY: `iocp.raw_handle()` is a live completion port and all out-pointers
            // reference writable stack storage for the duration of the wait.
            let result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
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
                    // SAFETY: The `Pin<Box<_>>` is still pinned in this scope; we
                    // only project a mutable reference without moving the allocation.
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
                        let Some(buffer) = buffer_pool.pop() else {
                            drop(tx);
                            return Err(MftError::InvalidData(
                                "I/O buffer pool exhausted while recycling worker reads".to_owned(),
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
        let mut index = MftIndex::from_parsed_records(volume, parsed_records);

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
}
