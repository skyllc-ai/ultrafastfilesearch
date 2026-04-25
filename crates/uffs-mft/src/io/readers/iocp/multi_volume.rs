// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Multi-volume IOCP reader.
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
    pub io_queue: alloc::collections::VecDeque<MultiVolumeIoOp>,
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
/// - `NVMe` drives get high concurrency while HDDs get low concurrency
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
    pub const fn new(volumes: Vec<VolumeState>) -> Self {
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
    #[expect(
        clippy::cognitive_complexity,
        reason = "multi-volume IOCP orchestration: per-volume state machines, completion-key dispatch, and rebalancing in-flight slots have to share one event loop to keep IOCP fairness; extracting helpers would either inline the same control flow or hide IO-completion invariants"
    )]
    pub fn read_all_volumes(&mut self) -> Result<Vec<crate::index::MftIndex>> {
        use core::pin::Pin;

        use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError, HANDLE};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        // In-flight operation tracking per volume
        struct InFlightOp {
            overlapped: windows::Win32::System::IO::OVERLAPPED,
            buffer: AlignedBuffer,
            op: MultiVolumeIoOp,
        }

        let record_size = self
            .volumes
            .first()
            .map_or(1024_usize, |vol| vol.extent_map.bytes_per_record as usize);

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

        // Create buffer pools and in-flight tracking per volume
        let mut buffer_pools: Vec<Vec<AlignedBuffer>> = self
            .volumes
            .iter()
            .map(|volume| {
                (0..volume.max_concurrency)
                    .map(|_| AlignedBuffer::new(volume.io_chunk_size))
                    .collect()
            })
            .collect();

        let mut in_flight: Vec<Vec<Option<Pin<Box<InFlightOp>>>>> = self
            .volumes
            .iter()
            .map(|volume| (0..volume.max_concurrency).map(|_| None).collect())
            .collect();

        // Issue initial reads for all volumes
        let mut total_pending = 0_usize;

        for (vol_idx, vol) in self.volumes.iter_mut().enumerate() {
            let initial_count = core::cmp::min(vol.max_concurrency, vol.io_queue.len());

            for slot_idx in 0..initial_count {
                if let Some(op) = vol.io_queue.pop_front() {
                    let buffer = buffer_pools
                        .get_mut(vol_idx)
                        .and_then(Vec::pop)
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

                    let overlapped_ptr = core::ptr::addr_of_mut!(in_flight_op.overlapped);
                    let buffer_ptr = in_flight_op.buffer.as_mut_slice().as_mut_ptr();

                    // SAFETY: `buffer_ptr` is the start of the owned aligned buffer in
                    // `in_flight_op`, valid for `op.size` writable bytes for the IOCP read.
                    let read_slice =
                        unsafe { core::slice::from_raw_parts_mut(buffer_ptr, op.size) };
                    // SAFETY: `vol.handle` is a live overlapped handle, `read_slice` is a
                    // valid mutable slice of `op.size` bytes, and the `OVERLAPPED` pointer
                    // remains valid while the pinned op is in flight.
                    let read_result = unsafe {
                        ReadFile(vol.handle, Some(read_slice), None, Some(overlapped_ptr))
                    };

                    if read_result.is_err() {
                        // SAFETY: `GetLastError` reads the calling thread's last-error
                        // slot and does not dereference any Rust pointers.
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

                    if let Some(vol_slots) = in_flight.get_mut(vol_idx)
                        && let Some(slot) = vol_slots.get_mut(slot_idx)
                    {
                        *slot = Some(in_flight_op);
                    }
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
        let mut bytes_read_total = 0_u64;

        while total_pending > 0 {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                core::ptr::null_mut();

            // SAFETY: `iocp.raw_handle()` is live and all out-pointers reference
            // writable stack storage for the duration of the wait.
            let wait_result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &raw mut bytes_transferred,
                    &raw mut completion_key,
                    &raw mut overlapped_ptr,
                    u32::MAX,
                )
            };

            if wait_result.is_err() || overlapped_ptr.is_null() {
                // SAFETY: `GetLastError` reads the calling thread's last-error
                // slot and does not dereference any Rust pointers.
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
            let Some(vol_slots) = in_flight.get_mut(vol_idx) else {
                warn!(vol_idx, "Completion key out of range for in_flight table");
                continue;
            };
            let mut completed_slot = None;
            for (slot_idx, slot) in vol_slots.iter_mut().enumerate() {
                if let Some(op) = slot {
                    let op_ptr = core::ptr::addr_of!(op.overlapped);
                    if op_ptr.cast::<windows::Win32::System::IO::OVERLAPPED>()
                        == overlapped_ptr.cast_const()
                    {
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
            let Some(completed_pinned) = vol_slots.get_mut(slot_idx).and_then(Option::take) else {
                return Err(MftError::InvalidData(
                    "completed IOCP operation missing from in-flight slot".to_owned(),
                ));
            };
            let completed_op = Pin::into_inner(completed_pinned);
            let Some(vol) = self.volumes.get_mut(vol_idx) else {
                warn!(vol_idx, "Volume index out of range after completion");
                continue;
            };
            vol.pending_ops -= 1;
            vol.completed_io_ops += 1;
            total_pending -= 1;
            bytes_read_total += u64::from(bytes_transferred);

            // Parse the completed buffer using unified pipeline
            let Some(buffer_slice) = completed_op
                .buffer
                .as_slice()
                .get(..bytes_transferred as usize)
            else {
                // Unreachable: bytes_transferred ≤ allocated buffer size.
                return Err(MftError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "multi-volume completion reported more bytes than buffer size",
                )));
            };
            let records_in_buffer = bytes_transferred as usize / record_size;
            let start_frs = completed_op.op.start_frs;

            for (current_frs, record_idx) in (start_frs..).zip(0..records_in_buffer) {
                let record_start = record_idx * record_size;
                let record_end = record_start + record_size;
                let Some(record_data) = buffer_slice.get(record_start..record_end) else {
                    break;
                };
                let result = parse_record_full(record_data, current_frs);
                vol.merger.add_result(result);
            }

            // Return buffer to pool
            if let Some(pool) = buffer_pools.get_mut(vol_idx) {
                pool.push(completed_op.buffer);
            }

            // Issue next read for this volume if available
            if let Some(next_op) = vol.io_queue.pop_front() {
                let buffer = buffer_pools
                    .get_mut(vol_idx)
                    .and_then(Vec::pop)
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

                let next_overlapped_ptr = core::ptr::addr_of_mut!(new_in_flight.overlapped);
                let buffer_ptr = new_in_flight.buffer.as_mut_slice().as_mut_ptr();

                // SAFETY: `buffer_ptr` is the start of the owned aligned buffer in
                // `new_in_flight`, valid for `next_op.size` writable bytes for the IOCP read.
                let read_slice =
                    unsafe { core::slice::from_raw_parts_mut(buffer_ptr, next_op.size) };
                // SAFETY: `vol.handle` is a live overlapped handle, `read_slice` is a
                // valid mutable slice of `next_op.size` bytes, and the `OVERLAPPED` pointer
                // remains valid while the pinned op is in flight.
                let read_result = unsafe {
                    ReadFile(
                        vol.handle,
                        Some(read_slice),
                        None,
                        Some(next_overlapped_ptr),
                    )
                };

                if read_result.is_err() {
                    // SAFETY: `GetLastError` reads the calling thread's last-error
                    // slot and does not dereference any Rust pointers.
                    let err = unsafe { GetLastError() };
                    if err != ERROR_IO_PENDING {
                        warn!(
                            volume = %vol.drive_letter,
                            error = ?err,
                            "Failed to issue next read"
                        );
                        // Unpin to recover the buffer
                        let failed_op = Pin::into_inner(new_in_flight);
                        if let Some(pool) = buffer_pools.get_mut(vol_idx) {
                            pool.push(failed_op.buffer);
                        }
                        continue;
                    }
                }

                if let Some(next_vol_slots) = in_flight.get_mut(vol_idx)
                    && let Some(slot) = next_vol_slots.get_mut(slot_idx)
                {
                    *slot = Some(new_in_flight);
                }
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
            .map(|volume| {
                let parsed_records = volume.merger.merge();
                crate::index::MftIndex::from_parsed_records(volume.drive_letter, parsed_records)
            })
            .collect())
    }
}

/// Helper function to prepare volume state for multi-volume reading.
#[cfg(windows)]
#[must_use]
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
    sorted_chunks.sort_by_key(|chunk| chunk.disk_offset);

    let mut io_queue = alloc::collections::VecDeque::new();

    for chunk in &sorted_chunks {
        let skip_begin_bytes = chunk.skip_begin as usize * record_size;
        let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;
        if effective_records == 0 {
            continue;
        }

        let chunk_bytes = effective_records as usize * record_size;
        let mut offset_within_chunk = 0_usize;
        let mut frs_offset = 0_u64;

        while offset_within_chunk < chunk_bytes {
            let io_size = core::cmp::min(io_chunk_size, chunk_bytes - offset_within_chunk);
            let disk_offset =
                chunk.disk_offset + skip_begin_bytes as u64 + offset_within_chunk as u64;

            io_queue.push_back(MultiVolumeIoOp {
                disk_offset,
                size: io_size,
                start_frs: chunk.start_frs + chunk.skip_begin + frs_offset,
            });

            offset_within_chunk += io_size;
            frs_offset += (io_size / record_size) as u64;
        }
    }

    let total_io_ops = io_queue.len();
    let _estimated_records = bitmap
        .as_ref()
        .map_or(total_records, crate::platform::MftBitmap::count_in_use);

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
