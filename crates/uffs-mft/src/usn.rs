//! # USN Journal Integration (M5 Optimization)
//!
//! This module provides USN (Update Sequence Number) Journal integration for
//! incremental index updates. Instead of rescanning the entire MFT, we can
//! query the USN Journal for changes since the last index build.
//!
//! ## Windows API
//!
//! - `FSCTL_QUERY_USN_JOURNAL` - Get journal info (ID, first/next USN)
//! - `FSCTL_READ_USN_JOURNAL` - Read changes since a given USN
//!
//! ## Change Types
//!
//! - `USN_REASON_FILE_CREATE` - New file created
//! - `USN_REASON_FILE_DELETE` - File deleted
//! - `USN_REASON_RENAME_NEW_NAME` - File renamed
//! - `USN_REASON_DATA_EXTEND/TRUNCATE` - File size changed
//! - `USN_REASON_BASIC_INFO_CHANGE` - Timestamps changed

use std::collections::HashMap;

/// USN Journal information returned by `FSCTL_QUERY_USN_JOURNAL`.
#[derive(Debug, Clone)]
pub struct UsnJournalInfo {
    /// Unique identifier for this journal instance
    pub journal_id: u64,
    /// First valid USN in the journal
    pub first_usn: i64,
    /// Next USN to be assigned
    pub next_usn: i64,
    /// Lowest valid USN (may differ from `first_usn`)
    pub lowest_valid_usn: i64,
    /// Maximum USN (journal size limit)
    pub max_usn: i64,
    /// Maximum size of the journal in bytes
    pub max_size: u64,
    /// Allocation delta (how much journal grows)
    pub allocation_delta: u64,
}

/// A single USN Journal change record.
#[derive(Debug, Clone)]
pub struct UsnRecord {
    /// File Reference Number (FRS)
    pub frs: u64,
    /// Parent directory FRS
    pub parent_frs: u64,
    /// USN of this record
    pub usn: i64,
    /// Reason flags (bitmask of `USN_REASON_*`)
    pub reason: u32,
    /// File attributes
    pub file_attributes: u32,
    /// Filename
    pub filename: String,
}

/// USN reason flags (from Windows SDK).
pub mod reason {
    /// Data in the default data stream was overwritten.
    pub const DATA_OVERWRITE: u32 = 0x0000_0001;
    /// Data in the default data stream was extended.
    pub const DATA_EXTEND: u32 = 0x0000_0002;
    /// Data in the default data stream was truncated.
    pub const DATA_TRUNCATION: u32 = 0x0000_0004;
    /// Data in a named data stream was overwritten.
    pub const NAMED_DATA_OVERWRITE: u32 = 0x0000_0010;
    /// Data in a named data stream was extended.
    pub const NAMED_DATA_EXTEND: u32 = 0x0000_0020;
    /// Data in a named data stream was truncated.
    pub const NAMED_DATA_TRUNCATION: u32 = 0x0000_0040;
    /// A new file or directory was created.
    pub const FILE_CREATE: u32 = 0x0000_0100;
    /// A file or directory was deleted.
    pub const FILE_DELETE: u32 = 0x0000_0200;
    /// Extended attributes were changed.
    pub const EA_CHANGE: u32 = 0x0000_0400;
    /// Security descriptor was changed.
    pub const SECURITY_CHANGE: u32 = 0x0000_0800;
    /// File or directory was renamed (old name).
    pub const RENAME_OLD_NAME: u32 = 0x0000_1000;
    /// File or directory was renamed (new name).
    pub const RENAME_NEW_NAME: u32 = 0x0000_2000;
    /// Indexable content was changed.
    pub const INDEXABLE_CHANGE: u32 = 0x0000_4000;
    /// Basic file attributes were changed.
    pub const BASIC_INFO_CHANGE: u32 = 0x0000_8000;
    /// Hard link was added or removed.
    pub const HARD_LINK_CHANGE: u32 = 0x0001_0000;
    /// Compression state was changed.
    pub const COMPRESSION_CHANGE: u32 = 0x0002_0000;
    /// Encryption state was changed.
    pub const ENCRYPTION_CHANGE: u32 = 0x0004_0000;
    /// Object ID was changed.
    pub const OBJECT_ID_CHANGE: u32 = 0x0008_0000;
    /// Reparse point was changed.
    pub const REPARSE_POINT_CHANGE: u32 = 0x0010_0000;
    /// Named data stream was added or removed.
    pub const STREAM_CHANGE: u32 = 0x0020_0000;
    /// Transacted change.
    pub const TRANSACTED_CHANGE: u32 = 0x0040_0000;
    /// Integrity state was changed.
    pub const INTEGRITY_CHANGE: u32 = 0x0080_0000;
    /// File handle was closed (final record for a change).
    pub const CLOSE: u32 = 0x8000_0000;
}

/// Categorized change type for easier processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    /// File was created
    Created,
    /// File was deleted
    Deleted,
    /// File was renamed (new name)
    Renamed,
    /// File size changed
    SizeChanged,
    /// File metadata changed (timestamps, attributes)
    MetadataChanged,
    /// Other change (not directly relevant to index)
    Other,
}

impl UsnRecord {
    /// Categorizes this USN record into a `ChangeType`.
    #[must_use]
    pub const fn change_type(&self) -> ChangeType {
        if self.reason & reason::FILE_CREATE != 0 {
            ChangeType::Created
        } else if self.reason & reason::FILE_DELETE != 0 {
            ChangeType::Deleted
        } else if self.reason & reason::RENAME_NEW_NAME != 0 {
            ChangeType::Renamed
        } else if self.reason & (reason::DATA_EXTEND | reason::DATA_TRUNCATION) != 0 {
            ChangeType::SizeChanged
        } else if self.reason & reason::BASIC_INFO_CHANGE != 0 {
            ChangeType::MetadataChanged
        } else {
            ChangeType::Other
        }
    }

    /// Returns true if this is a "close" record (final record for a change).
    #[must_use]
    pub const fn is_close(&self) -> bool {
        self.reason & reason::CLOSE != 0
    }
}

/// Aggregated changes for a single file (consolidates multiple USN records).
// These bools represent independent change flags from USN journal records.
// Using a bitflags pattern would add complexity without benefit for this DTO.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent change flags from USN journal records"
)]
#[derive(Debug, Clone, Default)]
pub struct FileChange {
    /// File Reference Number
    pub frs: u64,
    /// Parent directory FRS (latest)
    pub parent_frs: u64,
    /// Filename (latest)
    pub filename: String,
    /// Was the file created?
    pub created: bool,
    /// Was the file deleted?
    pub deleted: bool,
    /// Was the file renamed?
    pub renamed: bool,
    /// Did the file size change?
    pub size_changed: bool,
    /// Did metadata change?
    pub metadata_changed: bool,
}

/// Aggregates multiple USN records into per-file changes.
#[must_use]
pub fn aggregate_changes(records: &[UsnRecord]) -> HashMap<u64, FileChange> {
    let mut changes: HashMap<u64, FileChange> = HashMap::new();
    for record in records {
        let entry = changes.entry(record.frs).or_insert_with(|| FileChange {
            frs: record.frs,
            ..Default::default()
        });
        entry.parent_frs = record.parent_frs;
        if !record.filename.is_empty() {
            entry.filename.clone_from(&record.filename);
        }
        match record.change_type() {
            ChangeType::Created => entry.created = true,
            ChangeType::Deleted => entry.deleted = true,
            ChangeType::Renamed => entry.renamed = true,
            ChangeType::SizeChanged => entry.size_changed = true,
            ChangeType::MetadataChanged => entry.metadata_changed = true,
            ChangeType::Other => {}
        }
    }
    changes
}

// Re-export platform-specific functions
#[cfg(windows)]
pub use windows_impl::{query_usn_journal, read_usn_journal};

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "FFI: Windows API calls (CreateFileW, DeviceIoControl, CloseHandle)"
)]
mod windows_impl {
    use core::mem::size_of;
    use core::ptr;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL};
    use windows::core::PCWSTR;

    use super::*;

    #[repr(C)]
    #[derive(Default)]
    struct UsnJournalDataV0 {
        usn_journal_id: u64,
        first_usn: i64,
        next_usn: i64,
        lowest_valid_usn: i64,
        max_usn: i64,
        maximum_size: u64,
        allocation_delta: u64,
    }

    #[repr(C)]
    struct ReadUsnJournalDataV0 {
        start_usn: i64,
        reason_mask: u32,
        return_only_on_close: u32,
        timeout: u64,
        bytes_to_wait_for: u64,
        usn_journal_id: u64,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy)]
    struct UsnRecordV2Header {
        record_length: u32,
        major_version: u16,
        minor_version: u16,
        file_reference_number: u64,
        parent_file_reference_number: u64,
        usn: i64,
        time_stamp: i64,
        reason: u32,
        source_info: u32,
        security_id: u32,
        file_attributes: u32,
        file_name_length: u16,
        file_name_offset: u16,
    }

    fn open_volume_handle(volume: char) -> Result<HANDLE, std::io::Error> {
        let path = format!("\\\\.\\{}:", volume);
        let wide: Vec<u16> = OsStr::new(&path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // USN Journal operations require GENERIC_READ access
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                GENERIC_READ.0.into(),
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        };
        match handle {
            Ok(h) if h != INVALID_HANDLE_VALUE => Ok(h),
            Ok(_) => Err(std::io::Error::last_os_error()),
            Err(err) => Err(std::io::Error::from_raw_os_error(err.code().0)),
        }
    }

    /// Queries the USN Journal for a volume.
    pub fn query_usn_journal(volume: char) -> Result<UsnJournalInfo, std::io::Error> {
        let handle = open_volume_handle(volume)?;
        let mut journal_data = UsnJournalDataV0::default();
        let mut bytes_returned: u32 = 0;
        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_QUERY_USN_JOURNAL,
                None,                                          // Input buffer
                0,                                             // Input buffer size
                Some(ptr::from_mut(&mut journal_data).cast()), // Output buffer
                size_of::<UsnJournalDataV0>() as u32,          // Output buffer size
                Some(&mut bytes_returned),                     // Bytes returned
                None,                                          // Overlapped
            )
        };
        let _ = unsafe { CloseHandle(handle) };
        if result.is_err() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(UsnJournalInfo {
            journal_id: journal_data.usn_journal_id,
            first_usn: journal_data.first_usn,
            next_usn: journal_data.next_usn,
            lowest_valid_usn: journal_data.lowest_valid_usn,
            max_usn: journal_data.max_usn,
            max_size: journal_data.maximum_size,
            allocation_delta: journal_data.allocation_delta,
        })
    }

    /// Reads USN Journal records starting from a given USN.
    pub fn read_usn_journal(
        volume: char,
        journal_id: u64,
        start_usn: i64,
    ) -> Result<(Vec<UsnRecord>, i64), std::io::Error> {
        let handle = open_volume_handle(volume)?;
        let read_data = ReadUsnJournalDataV0 {
            start_usn,
            reason_mask: 0xFFFF_FFFF,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 0,
            usn_journal_id: journal_id,
        };
        let mut buffer = vec![0u8; 64 * 1024];
        let mut bytes_returned: u32 = 0;
        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                Some(ptr::from_ref(&read_data).cast()), // Input buffer
                size_of::<ReadUsnJournalDataV0>() as u32, // Input buffer size
                Some(buffer.as_mut_ptr().cast()),       // Output buffer
                buffer.len() as u32,                    // Output buffer size
                Some(&mut bytes_returned),              // Bytes returned
                None,                                   // Overlapped
            )
        };
        let _ = unsafe { CloseHandle(handle) };
        if result.is_err() {
            return Err(std::io::Error::last_os_error());
        }
        let next_usn = i64::from_le_bytes(buffer[0..8].try_into().unwrap());
        let mut records = Vec::new();
        let mut offset = 8usize;
        while offset + size_of::<UsnRecordV2Header>() <= bytes_returned as usize {
            let header: UsnRecordV2Header =
                unsafe { ptr::read_unaligned(buffer.as_ptr().add(offset).cast()) };
            if header.record_length == 0 {
                break;
            }
            let name_start = offset + header.file_name_offset as usize;
            let name_end = name_start + header.file_name_length as usize;
            let filename = if name_end <= bytes_returned as usize {
                let name_bytes = &buffer[name_start..name_end];
                let name_u16: Vec<u16> = name_bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&name_u16)
            } else {
                String::new()
            };
            records.push(UsnRecord {
                frs: header.file_reference_number & 0x0000_FFFF_FFFF_FFFF,
                parent_frs: header.parent_file_reference_number & 0x0000_FFFF_FFFF_FFFF,
                usn: header.usn,
                reason: header.reason,
                file_attributes: header.file_attributes,
                filename,
            });
            offset += header.record_length as usize;
        }
        Ok((records, next_usn))
    }
}

/// Queries the USN Journal for a volume (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(windows))]
pub fn query_usn_journal(_volume: char) -> Result<UsnJournalInfo, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "USN Journal is only available on Windows",
    ))
}

/// Reads USN Journal records starting from a given USN (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(windows))]
pub fn read_usn_journal(
    _volume: char,
    _journal_id: u64,
    _start_usn: i64,
) -> Result<(Vec<UsnRecord>, i64), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "USN Journal is only available on Windows",
    ))
}
