// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

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
//! Based on NTFS on-disk format documentation and the Microsoft NTFS
//! specification.
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
    NonResidentAttributeData, ResidentAttributeData, SECTOR_SIZE, SECTOR_SIZE_U64, apply_usa_fixup,
    fixup_file_record,
};

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
