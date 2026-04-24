// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Single-volume IOCP reader.
//!
//! **Module-scoped cast justification:** `as usize` / `as u32` casts convert
//! NTFS disk offsets (`u64`) and record sizes (`u32`) into `usize` / `u32`
//! respectively.  `usize` ≥ 32 bits on every supported target; NTFS disk
//! offsets are physically bounded by the volume size (≤ 2⁶⁴ bytes).
#![expect(
    clippy::cast_possible_truncation,
    reason = "NTFS disk-offset / record-size casts are lossless on supported 32/64-bit targets"
)]

#[expect(
    clippy::wildcard_imports,
    reason = "parent module's `pub(super) use` prelude \
              (HANDLE, MftError, ReadFile, rayon::prelude::*, tracing \
              macros, etc.) is designed to be consumed by submodules; \
              re-enumerating ~15 items here would duplicate the prelude \
              across every sibling reader file"
)]
use super::*;

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
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if I/O completion port setup, `ReadFile`, or
    /// `GetQueuedCompletionStatus` fails for any outstanding chunk; the error
    /// surfaces the underlying Win32 code.
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
        use core::pin::Pin;
        use std::collections::VecDeque;

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
            .map(|chunk| chunk.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = self.bitmap.as_ref().map_or_else(
            || self.extent_map.total_records() as usize,
            crate::platform::MftBitmap::count_in_use,
        );

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
            .map(|chunk| chunk.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

        // Sort chunks by disk_offset (LCN order) to minimize seek time on HDD
        let mut sorted_chunks: Vec<ReadChunk> = chunks;
        sorted_chunks.sort_by_key(|chunk| chunk.disk_offset);

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
                let aligned_size =
                    (read_size as usize + offset_adjustment).div_ceil(SECTOR_SIZE) * SECTOR_SIZE;

                // Issue overlapped read
                // SAFETY: We need get_unchecked_mut to get a mutable reference to the
                // pinned data for the OVERLAPPED pointer and buffer. The pin is maintained
                // throughout the operation lifetime.
                let overlapped_ptr = unsafe { op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
                // SAFETY: same justification as above — pinned Box, sole writer.
                let op_mut = unsafe { op.as_mut().get_unchecked_mut() };
                let Some(read_slice) = op_mut.buffer.as_mut_slice().get_mut(..aligned_size) else {
                    // Unreachable: buffer was sized to ≥ aligned_size at allocation.
                    return Err(MftError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "iocp-reader buffer shorter than aligned_size",
                    )));
                };
                // SAFETY: `handle` is a live overlapped-capable file handle, the
                // buffer slice lives inside the pinned operation for the duration of
                // the async I/O, and `overlapped_ptr` points into the same pinned op.
                let read_result = unsafe {
                    ReadFile(
                        handle,
                        Some(read_slice),
                        None, // Don't need bytes read for overlapped
                        Some(overlapped_ptr),
                    )
                };

                // Check for errors (ERROR_IO_PENDING is expected for async)
                if read_result.is_err() {
                    // SAFETY: `GetLastError` reads the calling thread's last-error
                    // slot and does not dereference any Rust pointers.
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
                core::ptr::null_mut();

            // SAFETY: `iocp.raw_handle()` is a live completion port and all
            // out-pointers reference writable stack storage for the duration of the wait.
            let wait_result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &raw mut bytes_transferred,
                    &raw mut completion_key,
                    &raw mut overlapped_ptr,
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
            for (idx, slot) in in_flight.iter_mut().enumerate() {
                if let Some(op) = slot {
                    let op_ptr = op.as_overlapped_ptr();
                    if op_ptr == overlapped_ptr {
                        completed_slot = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot
                && let Some(op_slot) = in_flight.get_mut(slot_idx)
                && let Some(mut op) = op_slot.take()
            {
                // SAFETY: The `Pin<Box<_>>` is still pinned in this scope; we
                // only project a mutable reference without moving the allocation.
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

                bytes_read_total += u64::from(bytes_transferred);
                completed_count += 1;

                // Report progress
                if let Some(ref mut cb) = progress_callback {
                    cb(bytes_read_total, total_bytes);
                }

                // Issue next read if there are more chunks
                if let Some(next_chunk) = pending_chunks.pop_front() {
                    // Reuse the buffer
                    let mut buffer = core::mem::replace(&mut op_mut.buffer, AlignedBuffer::new(0));

                    // Resize if needed
                    let next_read_size = next_chunk.record_count * u64::from(record_size);
                    let next_aligned_offset =
                        (next_chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                    let next_offset_adjustment =
                        (next_chunk.disk_offset - next_aligned_offset) as usize;
                    let next_aligned_size = (next_read_size as usize + next_offset_adjustment)
                        .div_ceil(SECTOR_SIZE)
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
                    // SAFETY: same justification as above — pinned Box, sole writer.
                    let new_op_mut = unsafe { new_op.as_mut().get_unchecked_mut() };
                    let Some(read_slice) = new_op_mut
                        .buffer
                        .as_mut_slice()
                        .get_mut(..next_aligned_size)
                    else {
                        // Unreachable: buffer was (re)sized to ≥ next_aligned_size above.
                        return Err(MftError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "iocp-reader recycled buffer shorter than next_aligned_size",
                        )));
                    };
                    // SAFETY: `handle` is a live overlapped-capable file handle,
                    // the buffer slice lives inside the pinned operation for the
                    // duration of the async I/O, and `overlapped_ptr` points into it.
                    let read_result =
                        unsafe { ReadFile(handle, Some(read_slice), None, Some(overlapped_ptr)) };

                    if read_result.is_err() {
                        // SAFETY: `GetLastError` reads the calling thread's last-error
                        // slot and does not dereference any Rust pointers.
                        let err = unsafe { GetLastError() };
                        if err != ERROR_IO_PENDING {
                            warn!(error = ?err, "Failed to issue next overlapped read");
                            continue;
                        }
                    }

                    if let Some(slot) = in_flight.get_mut(slot_idx) {
                        *slot = Some(new_op);
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
