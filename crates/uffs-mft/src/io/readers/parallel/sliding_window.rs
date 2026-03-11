//! Sliding-window IOCP reader path.

use super::*;

impl ParallelMftReader {
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
                let Some(buffer) = buffer_pool.pop() else {
                    return Err(MftError::InvalidData(
                        "I/O buffer pool exhausted while queuing overlapped reads".to_owned(),
                    ));
                };
                let mut in_flight_op = Box::pin(InFlightOp {
                    // SAFETY: `OVERLAPPED` is a plain Windows FFI struct and an
                    // all-zero value is the required initial state before offsets are set.
                    overlapped: unsafe { std::mem::zeroed() },
                    buffer,
                    op,
                });

                // Set offset in OVERLAPPED
                let offset = in_flight_op.op.disk_offset;
                // SAFETY: The pinned allocation remains in place while the I/O is in
                // flight; this only projects a mutable reference without moving it.
                let op_mut = unsafe { in_flight_op.as_mut().get_unchecked_mut() };
                op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                op_mut.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

                // Issue read
                let overlapped_ptr = &mut op_mut.overlapped as *mut _;
                let read_size = op_mut.op.size;
                // SAFETY: `overlapped_handle` is a live overlapped-capable handle,
                // the buffer slice spans `read_size` writable bytes in the pinned op,
                // and `overlapped_ptr` points to that same pinned operation.
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

            // SAFETY: `iocp.handle` is a live completion port and all out-pointers
            // reference writable stack storage for the duration of the wait.
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
                    // SAFETY: The `Pin<Box<_>>` is still pinned in this scope; we
                    // only project a mutable reference without moving the allocation.
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
                        let Some(buffer) = buffer_pool.pop() else {
                            return Err(MftError::InvalidData(
                                "I/O buffer pool exhausted while recycling overlapped reads"
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
}
