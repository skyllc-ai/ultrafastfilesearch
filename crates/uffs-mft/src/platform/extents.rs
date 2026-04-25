// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Retrieval-pointer helpers for locating MFT extents on disk.

#[cfg(windows)]
use core::mem::size_of;

#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows::Win32::System::Ioctl::{FSCTL_GET_RETRIEVAL_POINTERS, STARTING_VCN_INPUT_BUFFER};

#[cfg(windows)]
use crate::error::{MftError, Result};

/// Represents a contiguous extent of the MFT on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MftExtent {
    /// Virtual Cluster Number (offset within the file).
    pub vcn: u64,
    /// Number of clusters in this extent.
    pub cluster_count: u64,
    /// Logical Cluster Number (physical location on disk).
    /// Negative values indicate sparse/unallocated regions.
    pub lcn: i64,
}

impl MftExtent {
    /// Returns the byte offset of this extent on the volume.
    #[must_use]
    pub fn byte_offset(&self, bytes_per_cluster: u32) -> u64 {
        if self.lcn < 0 {
            0
        } else {
            self.lcn.cast_unsigned() * u64::from(bytes_per_cluster)
        }
    }

    /// Returns the size of this extent in bytes.
    #[must_use]
    pub fn byte_size(&self, bytes_per_cluster: u32) -> u64 {
        self.cluster_count * u64::from(bytes_per_cluster)
    }
}

/// Retrieves the extent map for a file using `FSCTL_GET_RETRIEVAL_POINTERS`.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "FFI: windows API (DeviceIoControl) and mem::zeroed"
)]
pub(super) fn get_retrieval_pointers(handle: HANDLE) -> Result<Vec<MftExtent>> {
    use windows::Win32::System::IO::DeviceIoControl;

    let mut extents = Vec::new();
    // SAFETY: `STARTING_VCN_INPUT_BUFFER` is a plain FFI struct whose all-zero
    // bit pattern represents `StartingVcn == 0`, the initial query window.
    let starting_vcn: STARTING_VCN_INPUT_BUFFER = unsafe { core::mem::zeroed() };

    let input_buffer_size =
        u32::try_from(size_of::<STARTING_VCN_INPUT_BUFFER>()).map_err(|err| {
            MftError::RetrievalPointers(format!(
                "STARTING_VCN_INPUT_BUFFER size {} exceeds u32::MAX ({err})",
                size_of::<STARTING_VCN_INPUT_BUFFER>()
            ))
        })?;

    let mut buffer_size = 64 * 1024;
    let mut buffer: Vec<u8> = vec![0; buffer_size];

    loop {
        let mut bytes_returned: u32 = 0;
        let output_buffer_size = u32::try_from(buffer_size).map_err(|err| {
            MftError::RetrievalPointers(format!(
                "FSCTL retrieval-pointer buffer size {buffer_size} exceeds u32::MAX ({err})"
            ))
        })?;

        // SAFETY: `handle` is an open file handle, `starting_vcn` and `buffer`
        // point to valid initialized storage for the provided lengths, and
        // `bytes_returned` is a valid out-parameter.
        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_GET_RETRIEVAL_POINTERS,
                Some(core::ptr::from_ref(&starting_vcn).cast()),
                input_buffer_size,
                Some(buffer.as_mut_ptr().cast()),
                output_buffer_size,
                Some(&raw mut bytes_returned),
                None,
            )
        };

        match result {
            Ok(()) => {
                parse_retrieval_pointers(&buffer, bytes_returned as usize, &mut extents);
                break;
            }
            Err(err) => {
                let hresult = err.code().0.cast_unsigned();
                let win32_error = if (hresult & 0xFFFF_0000) == 0x8007_0000 {
                    hresult & 0xFFFF
                } else {
                    hresult
                };

                if win32_error == 234 {
                    buffer_size *= 2;
                    buffer.resize(buffer_size, 0);
                    continue;
                }

                if win32_error == 38 {
                    break;
                }

                if extents.is_empty() {
                    return Err(MftError::RetrievalPointers(format!(
                        "FSCTL_GET_RETRIEVAL_POINTERS failed: HRESULT=0x{hresult:08X}, Win32={win32_error}"
                    )));
                }
                break;
            }
        }
    }

    Ok(extents)
}

/// Parses the `RETRIEVAL_POINTERS_BUFFER` structure.
#[cfg(windows)]
fn parse_retrieval_pointers(buffer: &[u8], size: usize, extents: &mut Vec<MftExtent>) {
    if size < size_of::<u32>() + size_of::<i64>() {
        return;
    }

    let read_le_u32 = |offset: usize| -> Option<u32> {
        let bytes = buffer.get(offset..offset + size_of::<u32>())?;
        let mut raw = [0_u8; 4];
        raw.copy_from_slice(bytes);
        Some(u32::from_le_bytes(raw))
    };
    let parse_signed64 = |offset: usize| -> Option<i64> {
        let bytes = buffer.get(offset..offset + size_of::<i64>())?;
        let mut raw = [0_u8; 8];
        raw.copy_from_slice(bytes);
        Some(i64::from_le_bytes(raw))
    };

    let Some(extent_count) = read_le_u32(0).map(|count| count as usize) else {
        return;
    };
    let Some(starting_vcn) = parse_signed64(8) else {
        return;
    };
    let mut prev_vcn = starting_vcn.cast_unsigned();

    let extent_size = 16;
    let extents_offset = 16;

    for i in 0..extent_count {
        let offset = extents_offset + i * extent_size;
        if offset + extent_size > size {
            break;
        }

        let Some(next_vcn) = parse_signed64(offset).map(i64::cast_unsigned) else {
            break;
        };
        let Some(lcn) = parse_signed64(offset + 8) else {
            break;
        };

        let cluster_count = next_vcn.saturating_sub(prev_vcn);

        extents.push(MftExtent {
            vcn: prev_vcn,
            cluster_count,
            lcn,
        });

        prev_vcn = next_vcn;
    }
}
