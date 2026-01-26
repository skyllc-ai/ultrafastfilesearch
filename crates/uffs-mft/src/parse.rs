//! Cross-platform NTFS MFT record parsing.
//!
//! This module provides parsing functions for NTFS MFT records that work on
//! any platform. The functions operate on raw byte buffers and don't require
//! Windows APIs.
//!
//! # Key Functions
//!
//! - `apply_fixup()` - Applies multi-sector fixup (Update Sequence Array)
//! - `parse_record()` - Parses a single MFT record
//! - `parse_record_full()` - Parses with extension record support
//! - `parse_record_zero_alloc()` - Zero-allocation parsing using thread-local buffer
//!
//! # Platform Support
//!
//! All functions in this module are cross-platform and can be used to parse
//! saved MFT files on macOS, Linux, or Windows.

use core::mem::size_of;
use std::cell::RefCell;

use smallvec::SmallVec;
use tracing::{debug, info, warn};

use crate::error::{MftError, Result};
use crate::ntfs::{
    AttributeRecordHeader, AttributeType, ExtendedStandardInfo, FILE_RECORD_MAGIC,
    FileNameAttribute, FileRecordSegmentHeader, MultiSectorHeader, NameInfo, StandardInformation,
    StreamInfo, SECTOR_SIZE,
};

// Thread-local buffer for record processing to avoid per-record allocations.
// Each thread gets its own 4KB buffer (enough for any MFT record).
thread_local! {
    static RECORD_BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0u8; 4096]);
}

// ============================================================================
// Parsed Record Structures
// ============================================================================

/// Parsed data from an MFT record (full C++ parity).
///
/// This struct captures ALL information from an MFT record, including:
/// - Multiple file names (hard links)
/// - Multiple data streams (Alternate Data Streams)
/// - Extended size information (allocated, compressed)
/// - All 18 attribute flags
#[derive(Debug, Clone, Default)]
pub struct ParsedRecord {
    /// File Record Segment number.
    pub frs: u64,
    /// Primary parent directory FRS (from best name).
    pub parent_frs: u64,
    /// Primary file name (Win32 or Win32+DOS preferred).
    pub name: String,
    /// All file names (hard links). Includes primary name.
    pub names: Vec<NameInfo>,
    /// All data streams. First is the default (unnamed) stream.
    pub streams: Vec<StreamInfo>,
    /// Logical file size (from default stream).
    pub size: u64,
    /// Allocated size on disk.
    pub allocated_size: u64,
    /// Extended standard information with all flags.
    pub std_info: ExtendedStandardInfo,
    /// Whether this record is in use.
    pub in_use: bool,
    /// Whether this is a directory.
    pub is_directory: bool,
}

impl ParsedRecord {
    /// Returns the number of hard links (names).
    #[must_use]
    pub fn name_count(&self) -> u16 {
        self.names.len() as u16
    }

    /// Returns the number of data streams.
    #[must_use]
    pub fn stream_count(&self) -> u16 {
        self.streams.len() as u16
    }

    /// Returns the creation time (Unix microseconds).
    #[must_use]
    pub fn created(&self) -> i64 {
        self.std_info.created
    }

    /// Returns the modification time (Unix microseconds).
    #[must_use]
    pub fn modified(&self) -> i64 {
        self.std_info.modified
    }

    /// Returns the access time (Unix microseconds).
    #[must_use]
    pub fn accessed(&self) -> i64 {
        self.std_info.accessed
    }

    /// Returns the MFT change time (Unix microseconds).
    #[must_use]
    pub fn mft_changed(&self) -> i64 {
        self.std_info.mft_changed
    }

    /// Returns the raw flags as u16 (for backward compatibility).
    #[must_use]
    pub fn flags(&self) -> u16 {
        (self.std_info.to_raw_flags() & 0xFFFF) as u16
    }
}

/// Attributes extracted from an extension record.
///
/// Extension records contain additional attributes for files that don't
/// fit in a single MFT record. These must be merged into the base record.
#[derive(Debug, Clone, Default)]
pub struct ExtensionAttributes {
    /// The base FRS this extension belongs to.
    pub base_frs: u64,
    /// The extension's own FRS.
    pub extension_frs: u64,
    /// File names found in this extension.
    pub names: Vec<NameInfo>,
    /// Streams found in this extension.
    pub streams: Vec<StreamInfo>,
}

/// Result of parsing an MFT record.
#[derive(Debug, Clone)]
pub enum ParseResult {
    /// A base record with all its data.
    Base(ParsedRecord),
    /// An extension record with attributes to merge.
    Extension(ExtensionAttributes),
    /// Record is not in use or invalid.
    Skip,
}




// ============================================================================
// Multi-Sector Fixup
// ============================================================================

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
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS struct
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

// ============================================================================
// Placeholder Records
// ============================================================================

/// Creates a placeholder record for a missing parent directory.
///
/// This matches C++ behavior where the `at()` method creates placeholder
/// records for any referenced FRS that hasn't been seen yet. When a file
/// references a parent directory that wasn't parsed (e.g., marked as not-in-use
/// in bitmap but still referenced), we create a placeholder to ensure path
/// resolution can complete.
///
/// # Arguments
///
/// * `frs` - The FRS number for the placeholder record
///
/// # Returns
///
/// A `ParsedRecord` with minimal information suitable for path resolution.
#[must_use]
pub fn create_placeholder_record(frs: u64) -> ParsedRecord {
    ParsedRecord {
        frs,
        parent_frs: 5, // Assume root as parent (FRS 5 is root directory)
        name: format!("<dir:{frs}>"),
        names: Vec::new(),
        streams: Vec::new(),
        size: 0,
        allocated_size: 0,
        std_info: ExtendedStandardInfo::default(),
        in_use: true,       // Mark as in-use so it's included in output
        is_directory: true, // Assume directory since it's referenced as parent
    }
}

/// Adds placeholder records for parent directories that are referenced
/// but not present in the parsed records.
///
/// This is the `Vec<ParsedRecord>` version of
/// `ParsedColumns::add_missing_parent_placeholders`.
///
/// # Performance Optimization (2026-01-23)
///
/// Uses `FxHashSet` instead of `std::collections::HashSet` for faster hashing.
/// FxHash is 5-10x faster than SipHash for integer keys.
///
/// # Arguments
///
/// * `records` - Mutable reference to the vector of parsed records
///
/// # Returns
///
/// The number of placeholder records added.
pub fn add_missing_parent_placeholders_to_vec(records: &mut Vec<ParsedRecord>) -> usize {
    use rustc_hash::FxHashSet;

    // Iterate until no new placeholders are needed (handles recursive missing
    // parents)
    let mut total_added = 0;
    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 10; // Prevent infinite loops

    loop {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            warn!(
                iterations,
                "Max iterations reached in placeholder creation - possible cycle"
            );
            break;
        }

        // Collect all FRS values we have (FxHashSet for faster hashing)
        let known_frs: FxHashSet<u64> = records.iter().map(|r| r.frs).collect();

        // Collect all parent_frs values that are referenced
        let referenced_parents: FxHashSet<u64> = records.iter().map(|r| r.parent_frs).collect();

        // Find missing parents (exclude 0 and 5 which are special root markers)
        let missing_parents: Vec<u64> = referenced_parents
            .difference(&known_frs)
            .filter(|&&frs| frs != 0 && frs != 5)
            .copied()
            .collect();

        if missing_parents.is_empty() {
            break; // No more missing parents
        }

        debug!(
            iteration = iterations,
            missing_count = missing_parents.len(),
            "Creating placeholder records for missing parent directories (Vec path)"
        );

        // Create placeholder records
        for frs in missing_parents {
            let placeholder = create_placeholder_record(frs);
            records.push(placeholder);
            total_added += 1;
        }
    }

    if total_added > 0 {
        info!(
            total_added,
            iterations, "Added placeholder records for missing parent directories (Vec path)"
        );
    }

    total_added
}
