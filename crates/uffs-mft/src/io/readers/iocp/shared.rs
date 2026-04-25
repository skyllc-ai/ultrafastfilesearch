// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shared IOCP primitives.
//!
//! **Module-scoped cast justification:** `as u32` cast here converts the
//! u64 overlapped offset low bits to u32 per Win32 OVERLAPPED struct layout;
//! the high 32 bits are stored separately in `OffsetHigh`.
#![expect(
    clippy::cast_possible_truncation,
    reason = "Win32 OVERLAPPED low-32 / high-32 offset split is the documented struct layout"
)]

use super::super::prelude::{AlignedBuffer, HANDLE, MftError, ReadChunk, Result};

/// I/O Completion Port wrapper for Windows async I/O.
///
/// This provides IOCP-based overlapped I/O for maximum I/O parallelism,
/// mirroring the legacy implementation's approach of having multiple reads
/// in flight simultaneously.
pub struct IoCompletionPort {
    /// The IOCP handle.
    handle: HANDLE,
}

impl IoCompletionPort {
    /// Creates a new I/O Completion Port.
    ///
    /// # Errors
    /// Returns an error if IOCP creation fails.
    #[expect(
        unsafe_code,
        reason = "FFI: CreateIoCompletionPort to create IOCP handle"
    )]
    pub fn new(concurrency: u32) -> Result<Self> {
        use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows::Win32::System::IO::CreateIoCompletionPort;

        // SAFETY: This creates a new completion port with no associated file
        // handle yet; the call takes no borrowed pointers.
        let handle = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, concurrency) };

        handle.map_or_else(
            |err| {
                Err(MftError::Io(std::io::Error::other(format!(
                    "Failed to create IOCP: {err}"
                ))))
            },
            |iocp_handle| {
                Ok(Self {
                    handle: iocp_handle,
                })
            },
        )
    }

    /// Associates a file handle with this IOCP.
    ///
    /// # Errors
    /// Returns an error if association fails.
    #[expect(
        unsafe_code,
        reason = "FFI: CreateIoCompletionPort to associate file handle with IOCP"
    )]
    pub fn associate(&self, file_handle: HANDLE, key: usize) -> Result<()> {
        use windows::Win32::System::IO::CreateIoCompletionPort;

        // SAFETY: `self.handle` is a live IOCP handle and `file_handle` is an
        // already-open file handle being associated with it.
        let result = unsafe { CreateIoCompletionPort(file_handle, Some(self.handle), key, 0) };

        match result {
            Ok(_) => Ok(()),
            Err(err) => Err(MftError::Io(std::io::Error::other(format!(
                "Failed to associate handle with IOCP: {err}"
            )))),
        }
    }

    /// Gets the raw IOCP handle.
    #[must_use]
    pub const fn raw_handle(&self) -> HANDLE {
        self.handle
    }
}

impl Drop for IoCompletionPort {
    #[expect(
        unsafe_code,
        reason = "FFI: CloseHandle to release IOCP handle on drop"
    )]
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        if !self.handle.is_invalid() {
            // SAFETY: `self.handle` was created by `CreateIoCompletionPort` and is
            // closed exactly once during drop after `is_invalid()` checked validity.
            unsafe { CloseHandle(self.handle) }.ok();
        }
    }
}

/// Represents an in-flight overlapped read operation.
///
/// This structure is pinned in memory because the OVERLAPPED pointer
/// is passed to Windows and must remain valid until completion.
#[repr(C)]
pub struct OverlappedRead {
    /// The Windows OVERLAPPED structure (must be first field for pointer
    /// casting).
    pub overlapped: windows::Win32::System::IO::OVERLAPPED,
    /// The aligned buffer for read data.
    pub buffer: AlignedBuffer,
    /// The chunk being read.
    pub chunk: ReadChunk,
    /// Record size for parsing.
    pub record_size: u32,
    /// Bytes actually read (set on completion).
    pub bytes_read: usize,
    /// Index in the buffer pool (for returning).
    pub pool_index: usize,
}

impl OverlappedRead {
    /// Creates a new overlapped read operation.
    #[must_use]
    pub fn new(
        buffer: AlignedBuffer,
        chunk: ReadChunk,
        record_size: u32,
        pool_index: usize,
    ) -> Self {
        Self {
            overlapped: windows::Win32::System::IO::OVERLAPPED::default(),
            buffer,
            chunk,
            record_size,
            bytes_read: 0,
            pool_index,
        }
    }

    /// Sets the file offset for the overlapped read.
    pub const fn set_offset(&mut self, offset: u64) {
        self.overlapped.Anonymous.Anonymous.Offset = offset as u32;
        self.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32_u32) as u32;
    }

    /// Gets a mutable pointer to the OVERLAPPED structure.
    ///
    /// The returned pointer is valid as long as `self` is pinned and alive.
    /// Creating the raw pointer is safe; dereferencing it requires `unsafe`.
    pub const fn as_overlapped_ptr(&mut self) -> *mut windows::Win32::System::IO::OVERLAPPED {
        &raw mut self.overlapped
    }
}
