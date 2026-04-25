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

use super::prelude::*;

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
        reason = "FFI: dispatches to iocp_* helpers that submit ReadFile + drain GetQueuedCompletionStatus"
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
        use alloc::collections::VecDeque;
        use core::pin::Pin;

        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            return Ok(Vec::new());
        }

        let total_bytes: u64 = chunks
            .iter()
            .map(|chunk| chunk.record_count * u64::from(record_size))
            .sum();

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

        let iocp = IoCompletionPort::new(0)?;
        iocp.associate(handle, 0)?;

        let mut all_results: Vec<ParseResult> = Vec::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        let max_chunk_size = chunks
            .iter()
            .map(|chunk| chunk.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

        // Sort chunks by disk_offset (LCN order) to minimise seek time on HDD.
        let mut sorted_chunks: Vec<ReadChunk> = chunks;
        sorted_chunks.sort_by_key(|chunk| chunk.disk_offset);
        let mut pending_chunks: VecDeque<ReadChunk> = sorted_chunks.into_iter().collect();

        // In-flight operations (pinned for OVERLAPPED pointer stability).
        let mut in_flight: Vec<Option<Pin<Box<OverlappedRead>>>> =
            (0..self.concurrency).map(|_| None).collect();

        // SAFETY: `handle` is a live overlapped-capable file handle (per
        // doc-contract), `in_flight` is freshly allocated all-`None`, and
        // `pending_chunks` is owned by the caller.
        unsafe {
            iocp_prime_initial_reads(
                handle,
                &mut pending_chunks,
                &mut in_flight,
                record_size,
                max_chunk_size + SECTOR_SIZE,
            )
        }?;

        let mut completed_count = 0_usize;
        while completed_count < num_chunks {
            Self::process_one_iocp_completion(
                handle,
                &iocp,
                record_size,
                &mut in_flight,
                &mut pending_chunks,
                &mut all_results,
                &mut bytes_read_total,
                &mut completed_count,
                merge_extensions,
                total_bytes,
                progress_callback.as_mut(),
            )?;
        }

        info!(
            records = all_results.len(),
            bytes = bytes_read_total,
            "✅ IOCP read complete"
        );

        Ok(merge_iocp_parse_results(all_results))
    }

    /// Drive one IOCP completion: wait, parse the buffer, update progress,
    /// and re-issue a read for the next pending chunk if any.
    ///
    /// Returns `Ok(())` for both successful completions and benign
    /// non-matching completions; only `Buffer`-class read-issue failures
    /// propagate as errors (the helper already logs Win32 failures).
    #[expect(
        unsafe_code,
        reason = "FFI: forwards to iocp_wait_for_completion + iocp_issue_replacement_read"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "shared mutable state across IOCP slots, pending chunks, parse results, progress, and the callback handle — bundling into a struct would obscure the per-completion data flow"
    )]
    fn process_one_iocp_completion<F>(
        handle: HANDLE,
        iocp: &IoCompletionPort,
        record_size: u32,
        in_flight: &mut [Option<core::pin::Pin<Box<OverlappedRead>>>],
        pending_chunks: &mut alloc::collections::VecDeque<ReadChunk>,
        all_results: &mut Vec<ParseResult>,
        bytes_read_total: &mut u64,
        completed_count: &mut usize,
        merge_extensions: bool,
        total_bytes: u64,
        progress_callback: Option<&mut F>,
    ) -> Result<()>
    where
        F: FnMut(u64, u64),
    {
        // SAFETY: caller's invariant: `iocp` drives every in-flight op.
        let Some((slot_idx, mut op, bytes_transferred)) =
            (unsafe { iocp_wait_for_completion(iocp, in_flight) })
        else {
            return Ok(());
        };

        // SAFETY: the `Pin<Box<_>>` is still pinned in this scope; we only
        // project a mutable reference without moving the allocation.
        let op_mut = unsafe { op.as_mut().get_unchecked_mut() };
        op_mut.bytes_read = bytes_transferred as usize;

        let results = parse_buffer_zero_copy_inner(
            op_mut.buffer.as_mut_slice(),
            op_mut.bytes_read,
            &op_mut.chunk,
            op_mut.record_size,
            merge_extensions,
        );
        all_results.extend(results);

        *bytes_read_total += u64::from(bytes_transferred);
        *completed_count += 1;

        if let Some(cb) = progress_callback {
            cb(*bytes_read_total, total_bytes);
        }

        // Recycle the just-completed op's buffer for the next chunk.
        if let Some(next_chunk) = pending_chunks.pop_front() {
            let recycled_buffer = core::mem::replace(&mut op_mut.buffer, AlignedBuffer::new(0));
            // SAFETY: same handle / IOCP invariants as the priming pass.
            match unsafe {
                iocp_issue_replacement_read(
                    handle,
                    next_chunk,
                    record_size,
                    slot_idx,
                    recycled_buffer,
                )
            } {
                Ok(new_op) => {
                    if let Some(slot) = in_flight.get_mut(slot_idx) {
                        *slot = Some(new_op);
                    }
                }
                Err(IocpReadIssueError::Win32) => {
                    // Already logged by the helper; carry on with the next
                    // completion.
                }
                Err(IocpReadIssueError::Buffer(err)) => return Err(err),
            }
        }

        Ok(())
    }
}

/// Convert the accumulated [`ParseResult`]s into the final
/// `Vec<ParsedRecord>` via [`MftRecordMerger`].  Hoisted out of
/// [`IocpMftReader::read_all_iocp`] so the orchestrator stays under the
/// per-function line cap.
fn merge_iocp_parse_results(all_results: Vec<ParseResult>) -> Vec<ParsedRecord> {
    let mut merger = MftRecordMerger::with_capacity(all_results.len());
    for result in all_results {
        merger.add_result(result);
    }
    merger.merge()
}

/// Failure mode of [`iocp_issue_replacement_read`].
///
/// `Win32` failures (`ReadFile` returned an error other than
/// `ERROR_IO_PENDING`) are already logged inside the helper, so callers
/// merely treat them as "skip this slot, wait for the next completion".
/// `Buffer` failures bubble back up as fatal because they indicate a
/// programmer mistake (the recycled buffer was not big enough to cover
/// the next chunk's aligned read).
#[derive(Debug)]
enum IocpReadIssueError {
    /// `ReadFile` returned a non-`ERROR_IO_PENDING` error; logged at warn
    /// level.
    Win32,
    /// The aligned slice could not be carved out of the buffer; surface to
    /// caller.
    Buffer(MftError),
}

/// Issue an overlapped read for `chunk` against `handle` using
/// `buffer` as backing storage; returns the freshly pinned
/// [`OverlappedRead`] ready to be parked in an `in_flight` slot.
///
/// `buffer` is resized in-place when smaller than the sector-aligned read
/// size — this is the recycle path used when an earlier completion frees
/// up a slot.  Use [`iocp_prime_initial_reads`] for the cold-start case.
///
/// # Safety
///
/// Caller must guarantee:
/// - `handle` is a live overlapped-capable file handle associated with the
///   completion port driving the surrounding event loop.
/// - The returned [`OverlappedRead`] outlives the in-flight read until
///   `GetQueuedCompletionStatus` reports its completion.
#[expect(
    unsafe_code,
    reason = "FFI: ReadFile + pinned OVERLAPPED reborrow for IOCP read issuance"
)]
unsafe fn iocp_issue_replacement_read(
    handle: HANDLE,
    chunk: ReadChunk,
    record_size: u32,
    slot_idx: usize,
    mut buffer: AlignedBuffer,
) -> core::result::Result<core::pin::Pin<Box<OverlappedRead>>, IocpReadIssueError> {
    use core::pin::Pin;

    use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
    use windows::Win32::Storage::FileSystem::ReadFile;

    let read_size = chunk.record_count * u64::from(record_size);
    let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
    let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
    let aligned_size = (read_size as usize + offset_adjustment).div_ceil(SECTOR_SIZE) * SECTOR_SIZE;

    if buffer.len() < aligned_size {
        buffer = AlignedBuffer::new(aligned_size);
    }

    let mut op: Pin<Box<OverlappedRead>> =
        Box::pin(OverlappedRead::new(buffer, chunk, record_size, slot_idx));
    op.set_offset(aligned_offset);

    // SAFETY: `op` is a pinned Box with this thread as the sole writer; the
    // overlapped raw pointer is consumed before the second mutable reborrow.
    let overlapped_ptr = unsafe { op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
    // SAFETY: same justification as above — pinned Box, sole writer.
    let op_mut = unsafe { op.as_mut().get_unchecked_mut() };
    let Some(read_slice) = op_mut.buffer.as_mut_slice().get_mut(..aligned_size) else {
        // Unreachable: buffer was (re)sized to ≥ aligned_size above.
        return Err(IocpReadIssueError::Buffer(MftError::Io(
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "iocp-reader buffer shorter than aligned_size",
            ),
        )));
    };
    // SAFETY: caller guarantees `handle` is a live overlapped handle;
    // `read_slice` covers `aligned_size` writable bytes; `overlapped_ptr`
    // points to `op`'s pinned OVERLAPPED struct.
    let read_result = unsafe { ReadFile(handle, Some(read_slice), None, Some(overlapped_ptr)) };

    if read_result.is_err() {
        // SAFETY: `GetLastError` reads the calling thread's last-error
        // slot; no preconditions.
        let err = unsafe { GetLastError() };
        if err != ERROR_IO_PENDING {
            warn!(error = ?err, "Failed to issue overlapped read");
            return Err(IocpReadIssueError::Win32);
        }
    }

    Ok(op)
}

/// Issue [`iocp_issue_replacement_read`] for every empty slot in
/// `in_flight`, draining `pending_chunks`.  Used at the start of an IOCP
/// run before the completion-port pump takes over.
///
/// # Safety
///
/// Same contract as [`iocp_issue_replacement_read`].
#[expect(
    unsafe_code,
    reason = "FFI: forwards to iocp_issue_replacement_read which submits ReadFile"
)]
unsafe fn iocp_prime_initial_reads(
    handle: HANDLE,
    pending_chunks: &mut alloc::collections::VecDeque<ReadChunk>,
    in_flight: &mut [Option<core::pin::Pin<Box<OverlappedRead>>>],
    record_size: u32,
    initial_buffer_size: usize,
) -> Result<()> {
    for (slot_idx, slot) in in_flight.iter_mut().enumerate() {
        let Some(chunk) = pending_chunks.pop_front() else {
            break;
        };
        let buffer = AlignedBuffer::new(initial_buffer_size);
        // SAFETY: caller guarantees `handle` is live and the slot is empty.
        match unsafe { iocp_issue_replacement_read(handle, chunk, record_size, slot_idx, buffer) } {
            Ok(op) => *slot = Some(op),
            Err(IocpReadIssueError::Win32) => {
                // Already logged inside the helper; carry on with the next
                // slot.
            }
            Err(IocpReadIssueError::Buffer(err)) => return Err(err),
        }
    }
    Ok(())
}

/// Block on `GetQueuedCompletionStatus` for `iocp` and pull the matching
/// pinned [`OverlappedRead`] out of `in_flight`.
///
/// Returns `Some((slot_idx, op, bytes_transferred))` for a normal
/// completion or `None` when the wait failed / the completion does not
/// match a tracked slot (caller should `continue` the event loop).
///
/// # Safety
///
/// Caller must ensure `iocp` is the live completion port that every
/// `Some(_)` entry in `in_flight` is associated with.
#[expect(
    unsafe_code,
    reason = "FFI: GetQueuedCompletionStatus expects exclusive raw out-pointers"
)]
unsafe fn iocp_wait_for_completion(
    iocp: &IoCompletionPort,
    in_flight: &mut [Option<core::pin::Pin<Box<OverlappedRead>>>],
) -> Option<(usize, core::pin::Pin<Box<OverlappedRead>>, u32)> {
    use windows::Win32::System::IO::GetQueuedCompletionStatus;

    let mut bytes_transferred: u32 = 0;
    let mut completion_key: usize = 0;
    let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED = core::ptr::null_mut();

    // SAFETY: `iocp.raw_handle()` is a valid IOCP handle (caller's
    // invariant); the four out-parameters are exclusive mutable
    // references to local variables that live for the call.
    let wait_result = unsafe {
        GetQueuedCompletionStatus(
            iocp.raw_handle(),
            &raw mut bytes_transferred,
            &raw mut completion_key,
            &raw mut overlapped_ptr,
            u32::MAX,
        )
    };

    if wait_result.is_err() {
        let err = std::io::Error::last_os_error();
        warn!(error = %err, "GetQueuedCompletionStatus failed");
        return None;
    }

    let mut completed_slot: Option<usize> = None;
    for (idx, slot) in in_flight.iter_mut().enumerate() {
        if let Some(op) = slot.as_mut()
            && op.as_mut().as_overlapped_ptr() == overlapped_ptr
        {
            completed_slot = Some(idx);
            break;
        }
    }

    let slot_idx = completed_slot?;
    let op_slot = in_flight.get_mut(slot_idx)?;
    let op = op_slot.take()?;
    Some((slot_idx, op, bytes_transferred))
}
