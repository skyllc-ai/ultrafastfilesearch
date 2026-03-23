//! Windows-only MFT capture implementations: streaming save and IOCP capture.
//!
//! Extracted from `persistence.rs` to keep it under the 800 LOC threshold.
//! Both functions are `impl MftReader` methods that require a live volume
//! handle.

#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
use tracing::{debug, info};

#[cfg(windows)]
use super::MftReader;
#[cfg(windows)]
use crate::error::{MftError, Result};

#[cfg(windows)]
impl MftReader {
    /// Internal streaming save implementation.
    #[expect(
        unsafe_code,
        reason = "FFI: windows ReadFile, SetFilePointerEx for raw MFT streaming"
    )]
    pub(super) fn save_raw_streaming<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        use std::thread;

        use crossbeam_channel::{Receiver, Sender, bounded};
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

        use crate::io::{AlignedBuffer, MftExtentMap, SECTOR_SIZE, generate_read_chunks};
        use crate::platform::detect_drive_type;
        use crate::raw::StreamingRawMftWriter;

        let vol_handle = self.require_handle();
        let record_size = vol_handle.file_record_size();
        let volume_data = vol_handle.volume_data();
        let extents = vol_handle.get_mft_extents().unwrap_or_else(|_| {
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });
        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);

        let drive_type = detect_drive_type(self.volume);
        let chunk_size = match drive_type {
            crate::platform::DriveType::Nvme => 8 * 1024 * 1024,
            crate::platform::DriveType::Ssd => 8 * 1024 * 1024,
            crate::platform::DriveType::Hdd | crate::platform::DriveType::Unknown => {
                4 * 1024 * 1024
            }
        };

        let chunks = generate_read_chunks(&extent_map, None, chunk_size);
        let total_chunks = chunks.len();
        info!(
            "Streaming save: {} chunks, {} MB each, drive type: {:?}",
            total_chunks,
            chunk_size / (1024 * 1024),
            drive_type
        );

        let mut writer = StreamingRawMftWriter::new(path, record_size, options)?;
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = bounded(2);
        let handle_ptr = vol_handle.raw_handle().0 as usize;
        let record_size_copy = record_size;

        let reader_handle = thread::spawn(move || -> Result<()> {
            let handle = HANDLE(handle_ptr as *mut std::ffi::c_void);
            let mut buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);

            for chunk in chunks {
                let read_size = chunk.record_count * u64::from(record_size_copy);
                let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
                let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
                    / SECTOR_SIZE)
                    * SECTOR_SIZE;

                if buffer.len() < aligned_size {
                    buffer = AlignedBuffer::new(aligned_size);
                }

                let mut new_pos: i64 = 0;
                // SAFETY: `handle` is a live raw MFT handle and `new_pos` is valid
                // writable storage for the duration of this seek.
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

                let mut bytes_read: u32 = 0;
                // SAFETY: `handle` is live, the aligned buffer slice covers
                // `aligned_size` writable bytes, and `bytes_read` is a valid
                // out-parameter for the call.
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

                let actual_size = read_size as usize;
                let data =
                    buffer.as_slice()[offset_adjustment..offset_adjustment + actual_size].to_vec();

                if tx.send(data).is_err() {
                    break;
                }
            }

            Ok(())
        });

        let mut chunks_written = 0;
        for data in rx {
            writer.write_chunk(&data)?;
            chunks_written += 1;

            if chunks_written % 100 == 0 {
                debug!(
                    "Streaming save progress: {}/{} chunks",
                    chunks_written, total_chunks
                );
            }
        }

        match reader_handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(_) => {
                return Err(MftError::Io(std::io::Error::other(
                    "Reader thread panicked",
                )));
            }
        }

        let header = writer.finish()?;
        info!(
            "Streaming save complete: {} records, {} bytes",
            header.record_count, header.original_size
        );
        Ok(header)
    }

    /// Internal IOCP capture implementation.
    #[expect(
        unsafe_code,
        reason = "FFI: ReadFile, GetQueuedCompletionStatus for overlapped IOCP capture"
    )]
    pub(super) fn save_iocp_internal<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw_iocp::IocpCaptureOptions,
    ) -> Result<crate::raw_iocp::IocpCaptureHeader> {
        use std::collections::VecDeque;
        use std::pin::Pin;

        use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, GetLastError, HANDLE};
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::GetQueuedCompletionStatus;

        use crate::io::{
            AlignedBuffer, IoCompletionPort, MftExtentMap, OverlappedRead, SECTOR_SIZE,
            generate_read_chunks,
        };
        use crate::platform::detect_drive_type;
        use crate::raw_iocp::IocpCaptureWriter;

        let vol_handle = self.require_handle();
        let record_size = vol_handle.file_record_size();
        let volume_data = vol_handle.volume_data();
        let concurrency = options.concurrency as usize;

        let extents = vol_handle.get_mft_extents().unwrap_or_else(|_| {
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);
        let drive_type = detect_drive_type(self.volume);
        let chunk_size = drive_type.optimal_chunk_size();
        let chunks = generate_read_chunks(&extent_map, None, chunk_size);
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            let writer = IocpCaptureWriter::new(record_size, options);
            return writer.write_to_file(path);
        }

        info!(
            chunks = num_chunks,
            chunk_size_mb = chunk_size / (1024 * 1024),
            concurrency,
            "🎯 Starting IOCP capture with {} concurrent reads",
            concurrency
        );

        // Create capture writer
        let mut writer = IocpCaptureWriter::new(record_size, options);

        // IOCP requires FILE_FLAG_OVERLAPPED, so we need a separate handle
        let handle: HANDLE = vol_handle.open_overlapped_handle()?;

        // Create IOCP and associate overlapped handle
        let iocp = IoCompletionPort::new(0)?;
        if let Err(e) = iocp.associate(handle, 0) {
            // SAFETY: handle was successfully opened by open_overlapped_handle
            unsafe { CloseHandle(handle) }.ok();
            return Err(e);
        }

        // Pre-allocate buffer pool and in-flight operations
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(chunk_size as u64) as usize;

        // Don't sort chunks - we want to capture IOCP completion order
        let mut pending_chunks: VecDeque<crate::io::ReadChunk> = chunks.into_iter().collect();
        let mut in_flight: Vec<Option<Pin<Box<OverlappedRead>>>> =
            (0..concurrency).map(|_| None).collect();

        // Issue initial reads
        for (slot_idx, slot) in in_flight.iter_mut().enumerate() {
            if let Some(chunk) = pending_chunks.pop_front() {
                let buffer = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
                let mut op = Box::pin(OverlappedRead::new(buffer, chunk, record_size, slot_idx));

                let aligned_offset =
                    (op.chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                op.set_offset(aligned_offset);

                let read_size = op.chunk.record_count * u64::from(record_size);
                let offset_adjustment = (op.chunk.disk_offset - aligned_offset) as usize;
                let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
                    / SECTOR_SIZE)
                    * SECTOR_SIZE;

                let overlapped_ptr = unsafe { op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
                let read_result = unsafe {
                    ReadFile(
                        handle,
                        Some(
                            &mut op.as_mut().get_unchecked_mut().buffer.as_mut_slice()
                                [..aligned_size],
                        ),
                        None,
                        Some(overlapped_ptr),
                    )
                };

                if read_result.is_err() {
                    let err = unsafe { GetLastError() };
                    if err != ERROR_IO_PENDING {
                        // SAFETY: handle was opened by open_overlapped_handle
                        unsafe { CloseHandle(handle) }.ok();
                        return Err(MftError::Io(std::io::Error::from_raw_os_error(
                            err.0 as i32,
                        )));
                    }
                }
                *slot = Some(op);
            }
        }

        // Process completions in the order they arrive
        let mut completed_chunks = 0;
        while completed_chunks < num_chunks {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let status = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX,
                )
            };

            if status.is_err() && overlapped_ptr.is_null() {
                // SAFETY: handle was opened by open_overlapped_handle
                unsafe { CloseHandle(handle) }.ok();
                return Err(MftError::Io(std::io::Error::last_os_error()));
            }

            // Find which slot completed by matching the overlapped pointer
            let mut completed_slot_idx: Option<usize> = None;
            for (idx, slot) in in_flight.iter_mut().enumerate() {
                if let Some(op) = slot {
                    let op_ptr = op.as_overlapped_ptr();
                    if op_ptr == overlapped_ptr {
                        completed_slot_idx = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot_idx {
                if let Some(op) = in_flight[slot_idx].take() {
                    // Extract chunk data in completion order
                    let chunk = &op.chunk;
                    let read_size = chunk.record_count * u64::from(record_size);
                    let aligned_offset =
                        (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                    let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;

                    let data = op.buffer.as_slice()
                        [offset_adjustment..offset_adjustment + read_size as usize]
                        .to_vec();

                    // Record in completion order
                    writer.record_chunk(chunk.start_frs, data);
                    completed_chunks += 1;

                    debug!(
                        "Captured chunk {} of {} (FRS {}..{})",
                        completed_chunks,
                        num_chunks,
                        chunk.start_frs,
                        chunk.start_frs + chunk.record_count
                    );

                    // Issue next read if available
                    if let Some(next_chunk) = pending_chunks.pop_front() {
                        let buffer = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
                        let mut new_op = Box::pin(OverlappedRead::new(
                            buffer,
                            next_chunk,
                            record_size,
                            slot_idx,
                        ));

                        let aligned_offset =
                            (new_op.chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                        new_op.set_offset(aligned_offset);

                        let read_size = new_op.chunk.record_count * u64::from(record_size);
                        let offset_adjustment =
                            (new_op.chunk.disk_offset - aligned_offset) as usize;
                        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE
                            - 1)
                            / SECTOR_SIZE)
                            * SECTOR_SIZE;

                        let overlapped_ptr =
                            unsafe { new_op.as_mut().get_unchecked_mut().as_overlapped_ptr() };
                        let read_result = unsafe {
                            ReadFile(
                                handle,
                                Some(
                                    &mut new_op.as_mut().get_unchecked_mut().buffer.as_mut_slice()
                                        [..aligned_size],
                                ),
                                None,
                                Some(overlapped_ptr),
                            )
                        };

                        if read_result.is_err() {
                            let err = unsafe { GetLastError() };
                            if err != ERROR_IO_PENDING {
                                // SAFETY: handle was opened by open_overlapped_handle
                                unsafe { CloseHandle(handle) }.ok();
                                return Err(MftError::Io(std::io::Error::from_raw_os_error(
                                    err.0 as i32,
                                )));
                            }
                        }
                        in_flight[slot_idx] = Some(new_op);
                    }
                }
            }
        }

        info!(
            "IOCP capture complete: {} chunks in completion order",
            writer.chunk_count()
        );

        // Close the overlapped handle before writing the file
        // SAFETY: handle was opened by open_overlapped_handle and is no longer needed
        unsafe { CloseHandle(handle) }.ok();

        writer.write_to_file(path)
    }
}
