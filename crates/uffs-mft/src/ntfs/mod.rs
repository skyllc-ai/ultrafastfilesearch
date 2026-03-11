//! NTFS-specific structures and parsing.
//!
//! This module provides low-level NTFS structure definitions for parsing
//! the Master File Table (MFT) directly from disk.
//!
//! # Safety
//!
//! These structures use `#[repr(C, packed)]` to match the on-disk layout.
//! Care must be taken when reading fields due to potential unaligned access.
//!
//! # Reference
//!
//! Based on the original C++ UFFS implementation and NTFS documentation.
//!
//! # Platform Support
//!
//! This module is cross-platform - NTFS structures are just byte layouts
//! and can be parsed on any platform.

mod boot_sector;
mod data_runs;
mod metadata;
mod records;
#[cfg(test)]
mod tests;

pub use self::boot_sector::NtfsBootSector;
pub use self::data_runs::{DataRun, extract_data_runs_from_attribute, parse_data_runs};
pub use self::metadata::{
    AttributeListEntry, ExtendedStandardInfo, FileNameAttribute, FileNamespace, IndexHeader,
    IndexRoot, NameInfo, ReparseMountPointBuffer, ReparsePointHeader, ReparseTag,
    STANDARD_INFO_SIZE_V12, STANDARD_INFO_SIZE_V30, StandardInformation,
    StandardInformationExtended, StreamInfo, is_internal_windows_stream,
};
pub use self::records::{
    AttributeIterator, AttributeRecordHeader, AttributeRef, AttributeType, FILE_RECORD_MAGIC,
    FileRecordFlags, FileRecordSegmentHeader, INDX_RECORD_MAGIC, MultiSectorHeader,
    NonResidentAttributeData, ResidentAttributeData, SECTOR_SIZE, apply_usa_fixup,
    fixup_file_record,
};

/// Converts a Windows FILETIME (100-nanosecond intervals since 1601-01-01)
/// to Unix timestamp in microseconds.
#[must_use]
pub const fn filetime_to_unix_micros(filetime: i64) -> i64 {
    // FILETIME epoch is 1601-01-01, Unix epoch is 1970-01-01
    // Difference is 11644473600 seconds = 116444736000000000 * 100ns
    const FILETIME_UNIX_DIFF: i64 = 116_444_736_000_000_000;

    // legacy-output parity: allow negative Unix timestamps (pre-1970 dates).
    // C++ uses FileTimeToLocalFileTime() which handles all valid FILETIME values.
    // Only clamp for filetime == 0 (unset/null timestamp).
    if filetime == 0 {
        return 0;
    }

    // Convert from 100ns to microseconds (works for both positive and negative
    // offsets)
    (filetime - FILETIME_UNIX_DIFF) / 10
}

/// Extracts the File Record Segment number from a file reference.
///
/// The lower 48 bits contain the FRS number.
#[must_use]
pub const fn file_reference_to_frs(file_reference: u64) -> u64 {
    file_reference & 0x0000_FFFF_FFFF_FFFF
}

/// Extracts the sequence number from a file reference.
///
/// The upper 16 bits contain the sequence number.
#[must_use]
pub const fn file_reference_to_sequence(file_reference: u64) -> u16 {
    (file_reference >> 48_i32) as u16
}
