//! Bulk IOCP reader path for ParallelMftReader.

use super::*;

impl ParallelMftReader {
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
                    // SAFETY: `OVERLAPPED` is a plain Windows FFI struct and an
                    // all-zero value is the required initial state before offsets are set.
                    overlapped: unsafe { std::mem::zeroed() },
                });

                // Set offset in OVERLAPPED
                op.overlapped.Anonymous.Anonymous.Offset = (disk_offset & 0xFFFF_FFFF) as u32;
                op.overlapped.Anonymous.Anonymous.OffsetHigh = (disk_offset >> 32) as u32;

                // Issue async read
                // SAFETY: The pointer is derived from `mft_buffer`, `buffer_offset`
                // and `io_size` are computed to stay within that allocation, and the
                // slice is only handed to Windows for the duration of the async read.
                let target_slice = unsafe {
                    std::slice::from_raw_parts_mut(
                        mft_buffer.as_mut_slice().as_mut_ptr().add(buffer_offset),
                        io_size,
                    )
                };

                // SAFETY: `overlapped_handle` is a live overlapped-capable handle,
                // `target_slice` is a valid writable region, and the `OVERLAPPED`
                // pointer remains valid because `op` stays pinned in `operations`.
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
                        // SAFETY: `GetLastError` reads the calling thread's last-error
                        // slot and does not dereference any Rust pointers.
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
        #[derive(Clone, Copy)]
        struct SendHandle(isize);
        // SAFETY: `SendHandle` only copies the raw IOCP handle value; the kernel
        // object itself is thread-safe and ownership stays external to this wrapper.
        unsafe impl Send for SendHandle {}
        // SAFETY: Sharing copied IOCP handle values across threads is sound because
        // all synchronization is provided by the kernel-managed completion port.
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
                    // SAFETY: `iocp_handle` is live and all out-pointers reference
                    // writable stack storage for the duration of the wait.
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
                        // SAFETY: `GetLastError` reads the calling thread's last-error
                        // slot and does not dereference any Rust pointers.
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
}
