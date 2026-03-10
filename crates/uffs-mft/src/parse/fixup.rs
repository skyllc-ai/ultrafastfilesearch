//! Multi-sector fixup application for raw MFT record buffers.

use core::mem::size_of;

use crate::ntfs::{FILE_RECORD_MAGIC, MultiSectorHeader, SECTOR_SIZE};

/// Applies the multi-sector fixup (Update Sequence Array) to a record.
///
/// NTFS uses a fixup mechanism to detect torn writes. The last two bytes
/// of each sector are replaced with a check value, and the original bytes
/// are stored in the Update Sequence Array.
///
/// # Arguments
///
/// * `data` - The record data (must be mutable)
///
/// # Returns
///
/// `true` if the fixup was successful, `false` if the record is corrupted.
#[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
pub fn apply_fixup(data: &mut [u8]) -> bool {
    if data.len() < size_of::<MultiSectorHeader>() {
        return false;
    }

    // SAFETY: We've verified the buffer is large enough.
    let header: MultiSectorHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

    // Validate magic number
    if header.magic != FILE_RECORD_MAGIC {
        return false;
    }

    let usa_offset = header.usa_offset as usize;
    let usa_count = header.usa_count as usize;

    // USA must have at least 2 entries (check value + at least one sector)
    if usa_count < 2 {
        return false;
    }

    // Validate USA offset
    if usa_offset + usa_count * 2 > data.len() {
        return false;
    }

    // Get the check value (first entry in USA)
    let check_value = u16::from_le_bytes([data[usa_offset], data[usa_offset + 1]]);

    // Apply fixup to each sector
    for idx in 1..usa_count {
        let sector_end = idx * SECTOR_SIZE - 2;

        if sector_end + 2 > data.len() {
            break;
        }

        // Verify the check value
        let current_value = u16::from_le_bytes([data[sector_end], data[sector_end + 1]]);
        if current_value != check_value {
            return false;
        }

        // Replace with the original value from USA
        let usa_entry_offset = usa_offset + idx * 2;
        data[sector_end] = data[usa_entry_offset];
        data[sector_end + 1] = data[usa_entry_offset + 1];
    }

    true
}
