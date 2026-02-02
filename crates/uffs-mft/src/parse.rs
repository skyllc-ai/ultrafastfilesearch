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
//! - `parse_record_zero_alloc()` - Zero-allocation parsing using thread-local
//!   buffer
//!
//! # Platform Support

// Allow pedantic lints for low-level parsing code that works with raw NTFS structures
#![allow(
    clippy::indexing_slicing,      // Justified: bounds checked before indexing
    clippy::cast_sign_loss,         // Justified: NTFS uses signed for some fields, we validate non-negative
    clippy::cast_lossless,          // Justified: explicit casts for clarity
    clippy::min_ident_chars,        // Justified: 's' for stream is idiomatic in closures
    clippy::assigning_clones,       // Justified: clone() is clearer than clone_from() in this context
    clippy::map_unwrap_or,          // Justified: map().unwrap_or() is more readable than map_or()
    clippy::wildcard_enum_match_arm // Justified: catch-all for Skip/Extension is intentional
)]
//! All functions in this module are cross-platform and can be used to parse
//! saved MFT files on macOS, Linux, or Windows.

use core::cell::RefCell;
use core::mem::size_of;

use tracing::{debug, info, warn};

use crate::ntfs::{
    ExtendedStandardInfo, FILE_RECORD_MAGIC, MultiSectorHeader, NameInfo, ReparsePointHeader,
    SECTOR_SIZE, StreamInfo,
};

// Thread-local buffer for record processing to avoid per-record allocations.
// Each thread gets its own 4KB buffer (enough for any MFT record).
thread_local! {
    static RECORD_BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0_u8; 4096]);
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
/// - Sequence number (for detecting reused FRS)
/// - Log File Sequence Number (LSN) for journal correlation
/// - `$FILE_NAME` timestamps (often differ from `$STANDARD_INFORMATION`)
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // Forensic fields require multiple bool flags
pub struct ParsedRecord {
    /// File Record Segment number.
    pub frs: u64,
    /// Sequence number (incremented when FRS is reused).
    pub sequence_number: u16,
    /// Log File Sequence Number - correlates with `$LogFile` journal.
    pub lsn: u64,
    /// Primary parent directory FRS (from best name).
    pub parent_frs: u64,
    /// Primary file name (Win32 or Win32+DOS preferred).
    pub name: String,
    /// Primary filename namespace (0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS).
    pub namespace: u8,
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
    /// Creation time from primary `$FILE_NAME` (Unix microseconds).
    pub fn_created: i64,
    /// Modification time from primary `$FILE_NAME` (Unix microseconds).
    pub fn_modified: i64,
    /// Access time from primary `$FILE_NAME` (Unix microseconds).
    pub fn_accessed: i64,
    /// MFT change time from primary `$FILE_NAME` (Unix microseconds).
    pub fn_mft_changed: i64,
    /// Reparse tag from `$REPARSE_POINT` attribute (0 if not a reparse point).
    /// Common values: symlink (0xA000000C), junction (0xA0000003), `OneDrive`,
    /// etc.
    pub reparse_tag: u32,

    // P3 Forensic fields (populated when ParseOptions::forensic is true)
    /// True if this record is deleted (`FRH_IN_USE` flag not set).
    /// Only populated when parsing with `ParseOptions::include_deleted`.
    pub is_deleted: bool,
    /// True if this record has corrupt fixup (USA mismatch).
    /// Only populated when parsing with `ParseOptions::include_corrupt`.
    pub is_corrupt: bool,
    /// True if this is an extension record (not a base record).
    /// Only populated when parsing with `ParseOptions::include_extensions`.
    pub is_extension: bool,
    /// Base FRS for extension records (0 for base records).
    /// Only meaningful when `is_extension` is true.
    pub base_frs: u64,
}

impl ParsedRecord {
    /// Returns the number of hard links (names).
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // Names count is always < 65536
    pub fn name_count(&self) -> u16 {
        self.names.len() as u16
    }

    /// Returns the number of data streams.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // Stream count is always < 65536
    pub fn stream_count(&self) -> u16 {
        self.streams.len() as u16
    }

    /// Returns the creation time (Unix microseconds).
    #[must_use]
    pub const fn created(&self) -> i64 {
        self.std_info.created
    }

    /// Returns the modification time (Unix microseconds).
    #[must_use]
    pub const fn modified(&self) -> i64 {
        self.std_info.modified
    }

    /// Returns the access time (Unix microseconds).
    #[must_use]
    pub const fn accessed(&self) -> i64 {
        self.std_info.accessed
    }

    /// Returns the MFT change time (Unix microseconds).
    #[must_use]
    pub const fn mft_changed(&self) -> i64 {
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
// Parse Options (Forensic Mode)
// ============================================================================

/// Options for MFT record parsing.
///
/// By default, parsing skips deleted records, corrupt records, and merges
/// extension records into their base records. Forensic mode enables
/// extraction of these records for analysis.
///
/// # Performance Impact
///
/// - `include_deleted`: +10-50% more records (significant memory/CPU impact)
/// - `include_corrupt`: Minimal impact (corrupt records are rare)
/// - `include_extensions`: Minimal impact (extensions are rare, ~0.1% of
///   records)
#[derive(Debug, Clone, Copy, Default)]
pub struct ParseOptions {
    /// Include deleted records (`FRH_IN_USE` flag not set).
    /// These records may have partial/stale data but are valuable for
    /// forensics.
    pub include_deleted: bool,
    /// Include corrupt records (USA fixup failed).
    /// These records have torn writes but may still contain recoverable data.
    pub include_corrupt: bool,
    /// Include extension records as separate rows instead of merging.
    /// Useful for analyzing fragmented files with many attributes.
    pub include_extensions: bool,
}

impl ParseOptions {
    /// Default options: skip deleted, corrupt, and merge extensions.
    pub const DEFAULT: Self = Self {
        include_deleted: false,
        include_corrupt: false,
        include_extensions: false,
    };

    /// Forensic mode: include all records for analysis.
    pub const FORENSIC: Self = Self {
        include_deleted: true,
        include_corrupt: true,
        include_extensions: true,
    };

    /// Returns true if any forensic options are enabled.
    #[inline]
    #[must_use]
    pub const fn is_forensic(&self) -> bool {
        self.include_deleted || self.include_corrupt || self.include_extensions
    }
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
        sequence_number: 0,
        lsn: 0,
        reparse_tag: 0,
        parent_frs: 5, // Assume root as parent (FRS 5 is root directory)
        name: format!("<dir:{frs}>"),
        namespace: 1, // Win32 namespace
        names: Vec::new(),
        streams: Vec::new(),
        size: 0,
        allocated_size: 0,
        std_info: ExtendedStandardInfo::default(),
        fn_created: 0,
        fn_modified: 0,
        fn_accessed: 0,
        fn_mft_changed: 0,
        in_use: true,       // Mark as in-use so it's included in output
        is_directory: true, // Assume directory since it's referenced as parent
        // P3 forensic fields - placeholders are not deleted/corrupt/extension
        is_deleted: false,
        is_corrupt: false,
        is_extension: false,
        base_frs: 0,
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
/// `FxHash` is 5-10x faster than `SipHash` for integer keys.
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

    /// Maximum iterations for placeholder creation to prevent infinite loops.
    const MAX_ITERATIONS: usize = 10;

    // Iterate until no new placeholders are needed (handles recursive missing
    // parents)
    let mut total_added = 0;
    let mut iterations = 0;

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

// ============================================================================
// Attribute Parsing Helpers
// ============================================================================

/// Parses `$STANDARD_INFORMATION` into `ExtendedStandardInfo`.
///
/// Handles both NTFS 1.2 (36 bytes) and NTFS 3.0+ (72 bytes) formats.
/// For NTFS 3.0+, also extracts `usn`, `security_id`, and `owner_id`.
#[allow(unsafe_code, clippy::single_call_fn)] // Required: ptr::read for packed NTFS struct
fn parse_standard_info_full(data: &[u8], attr_offset: usize, result: &mut ExtendedStandardInfo) {
    use core::mem::size_of;

    use crate::ntfs::{
        STANDARD_INFO_SIZE_V12, STANDARD_INFO_SIZE_V30, StandardInformation,
        StandardInformationExtended, filetime_to_unix_micros,
    };

    // Get value offset and length (resident attribute)
    let value_length_bytes = &data[attr_offset + 16..attr_offset + 20];
    let value_length =
        u32::from_le_bytes(value_length_bytes.try_into().unwrap_or([0, 0, 0, 0])) as usize;
    let value_offset_bytes = &data[attr_offset + 20..attr_offset + 22];
    let value_offset = u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;

    let si_offset = attr_offset + value_offset;

    // Check if we have NTFS 3.0+ extended format (72 bytes)
    if value_length >= STANDARD_INFO_SIZE_V30
        && si_offset + size_of::<StandardInformationExtended>() <= data.len()
    {
        // SAFETY: We've verified the buffer is large enough.
        let si: StandardInformationExtended =
            unsafe { core::ptr::read(data[si_offset..].as_ptr().cast()) };

        *result = ExtendedStandardInfo {
            created: filetime_to_unix_micros(si.creation_time),
            modified: filetime_to_unix_micros(si.modification_time),
            accessed: filetime_to_unix_micros(si.access_time),
            mft_changed: filetime_to_unix_micros(si.mft_change_time),
            usn: si.usn,
            security_id: si.security_id,
            owner_id: si.owner_id,
            ..ExtendedStandardInfo::from_attributes(si.file_attributes)
        };
    } else if value_length >= STANDARD_INFO_SIZE_V12
        && si_offset + size_of::<StandardInformation>() <= data.len()
    {
        // NTFS 1.2 format (36 bytes) - no extended fields
        // SAFETY: We've verified the buffer is large enough.
        let si: StandardInformation = unsafe { core::ptr::read(data[si_offset..].as_ptr().cast()) };

        *result = ExtendedStandardInfo {
            created: filetime_to_unix_micros(si.creation_time),
            modified: filetime_to_unix_micros(si.modification_time),
            accessed: filetime_to_unix_micros(si.access_time),
            mft_changed: filetime_to_unix_micros(si.mft_change_time),
            usn: 0,
            security_id: 0,
            owner_id: 0,
            ..ExtendedStandardInfo::from_attributes(si.file_attributes)
        };
    }
    // If neither format fits, leave result as default
}

/// Parses `$FILE_NAME` and returns a `NameInfo` with timestamps.
#[allow(unsafe_code, clippy::single_call_fn)] // Required: ptr::read for packed NTFS struct
fn parse_file_name_full(data: &[u8], attr_offset: usize) -> Option<NameInfo> {
    use core::mem::size_of;

    use smallvec::SmallVec;

    use crate::ntfs::{FileNameAttribute, file_reference_to_frs, filetime_to_unix_micros};

    // Get value offset (resident attribute)
    let value_offset_bytes = &data[attr_offset + 20..attr_offset + 22];
    let value_offset = u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;

    let fn_offset = attr_offset + value_offset;
    if fn_offset + size_of::<FileNameAttribute>() > data.len() {
        return None;
    }

    // SAFETY: We've verified the buffer is large enough.
    let fn_attr: FileNameAttribute = unsafe { core::ptr::read(data[fn_offset..].as_ptr().cast()) };

    // Extract file name (UTF-16LE)
    let name_len = fn_attr.file_name_length as usize;
    let name_offset = fn_offset + size_of::<FileNameAttribute>();

    if name_offset + name_len * 2 > data.len() {
        return None;
    }

    let name_bytes = &data[name_offset..name_offset + name_len * 2];
    // Use SmallVec to avoid heap allocation for typical file names (< 128 chars)
    #[allow(clippy::missing_asserts_for_indexing)] // chunks_exact(2) guarantees chunk.len() == 2
    let name_u16: SmallVec<[u16; 128]> = name_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    let name = String::from_utf16(&name_u16).ok()?;

    Some(NameInfo {
        name,
        parent_frs: file_reference_to_frs(fn_attr.parent_directory),
        namespace: fn_attr.file_name_namespace,
        fn_created: filetime_to_unix_micros(fn_attr.creation_time),
        fn_modified: filetime_to_unix_micros(fn_attr.modification_time),
        fn_accessed: filetime_to_unix_micros(fn_attr.access_time),
        fn_mft_changed: filetime_to_unix_micros(fn_attr.mft_change_time),
    })
}

/// Parses `$DATA` attribute and returns a `StreamInfo`.
///
/// # Special handling for `$BadClus:$Bad`
/// The `$BadClus` file (FRS 8) has a `$Bad` stream that is a sparse file
/// spanning the entire volume. C++ uses `InitializedSize` instead of `DataSize`
/// for this stream to avoid reporting the full volume size.
#[allow(clippy::single_call_fn)]
fn parse_data_attribute_full(
    data: &[u8],
    attr_offset: usize,
    header: &crate::ntfs::AttributeRecordHeader,
    frs: u64,
) -> Option<StreamInfo> {
    use smallvec::SmallVec;

    // Extract stream name from attribute header
    // Use SmallVec to avoid heap allocation for typical stream names (< 64 chars)
    let stream_name = if header.name_length > 0 {
        let name_offset = attr_offset + header.name_offset as usize;
        let name_len = header.name_length as usize;
        if name_offset + name_len * 2 > data.len() {
            return None;
        }
        let name_bytes = &data[name_offset..name_offset + name_len * 2];
        #[allow(clippy::missing_asserts_for_indexing)]
        // chunks_exact(2) guarantees chunk.len() == 2
        let name_u16: SmallVec<[u16; 64]> = name_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&name_u16).unwrap_or_default()
    } else {
        String::new()
    };

    let is_resident = header.is_non_resident == 0;
    let (size, allocated_size, is_sparse, is_compressed) = if is_resident {
        // Resident: get size from resident header
        let value_length_bytes = &data[attr_offset + 16..attr_offset + 20];
        let value_length = u32::from_le_bytes(value_length_bytes.try_into().ok()?);
        (value_length as u64, 0, false, false)
    } else {
        // Non-resident: get sizes from non-resident header
        // Layout after common header (offset 16):
        //   0-7:   Starting VCN
        //   8-15:  Ending VCN
        //   16-17: Data runs offset
        //   18-19: Compression unit size
        //   20-23: Padding
        //   24-31: Allocated size
        //   32-39: Data size (actual file size)
        //   40-47: Initialized size
        let nr_offset = attr_offset + 16; // After common header
        if nr_offset + 48 > data.len() {
            return None;
        }

        let allocated_size =
            i64::from_le_bytes(data[nr_offset + 24..nr_offset + 32].try_into().ok()?);
        // Data size is at offset 32, initialized size is at offset 40
        let data_size = i64::from_le_bytes(data[nr_offset + 32..nr_offset + 40].try_into().ok()?);
        let initialized_size =
            i64::from_le_bytes(data[nr_offset + 40..nr_offset + 48].try_into().ok()?);

        // Check compression unit (at offset 16 in non-resident header)
        let compression_unit = data[nr_offset + 8];
        let is_compressed = compression_unit > 0;

        // Check sparse flag in attribute flags
        let is_sparse = (header.flags & 0x8000) != 0;

        // Special handling for $BadClus:$Bad (FRS 8, stream name "$Bad")
        // This is a sparse file spanning the entire volume. C++ uses InitializedSize
        // instead of DataSize to avoid reporting the full volume size.
        // See ntfs_index.hpp lines 701-716.
        let is_badclus_bad = frs == 8 && stream_name == "$Bad";
        let effective_size = if is_badclus_bad {
            initialized_size.max(0) as u64
        } else {
            data_size.max(0) as u64
        };
        let effective_allocated = if is_badclus_bad {
            initialized_size.max(0) as u64
        } else {
            allocated_size.max(0) as u64
        };

        (
            effective_size,
            effective_allocated,
            is_sparse,
            is_compressed,
        )
    };

    Some(StreamInfo {
        name: stream_name,
        size,
        allocated_size,
        is_sparse,
        is_compressed,
        is_resident,
    })
}

// ============================================================================
// Main Parsing Functions
// ============================================================================

/// Tracks the best primary name during parsing.
/// Win32 (1) and Win32+DOS (3) are preferred over POSIX (0).
struct PrimaryNameTracker {
    /// Primary filename.
    name: String,
    /// Parent FRS of the primary name.
    parent_frs: u64,
    /// Namespace of the primary name (255 = invalid/unset).
    namespace: u8,
    /// `$FILE_NAME` creation timestamp.
    fn_created: i64,
    /// `$FILE_NAME` modification timestamp.
    fn_modified: i64,
    /// `$FILE_NAME` access timestamp.
    fn_accessed: i64,
    /// `$FILE_NAME` MFT change timestamp.
    fn_mft_changed: i64,
}

impl PrimaryNameTracker {
    /// Sentinel value indicating no namespace has been set yet.
    const INVALID_NAMESPACE: u8 = 255;

    /// Updates the primary name if the new name is better.
    fn update(&mut self, name_info: &NameInfo) {
        let dominated = self.namespace == Self::INVALID_NAMESPACE;
        let is_better =
            matches!(name_info.namespace, 1 | 3) || (name_info.namespace == 0 && dominated);
        if is_better || dominated {
            self.name = name_info.name.clone();
            self.parent_frs = name_info.parent_frs;
            self.namespace = name_info.namespace;
            self.fn_created = name_info.fn_created;
            self.fn_modified = name_info.fn_modified;
            self.fn_accessed = name_info.fn_accessed;
            self.fn_mft_changed = name_info.fn_mft_changed;
        }
    }
}

impl Default for PrimaryNameTracker {
    fn default() -> Self {
        Self {
            name: String::new(),
            parent_frs: 0,
            namespace: Self::INVALID_NAMESPACE,
            fn_created: 0,
            fn_modified: 0,
            fn_accessed: 0,
            fn_mft_changed: 0,
        }
    }
}

/// Parses an MFT record and extracts relevant information.
///
/// This function handles both base records and extension records.
/// Extension records return `ParseResult::Extension` which must be
/// merged into the base record later.
///
/// # Arguments
///
/// * `data` - The raw record data (after fixup)
/// * `frs` - The File Record Segment number
///
/// # Returns
///
/// `ParseResult::Base` for base records, `ParseResult::Extension` for
/// extension records, or `ParseResult::Skip` if invalid/not in use.
#[must_use]
// Required: ptr::read for packed NTFS structs
// 101 lines: just over limit due to P2 reparse_tag extraction; splitting would hurt readability
#[allow(unsafe_code, clippy::cognitive_complexity, clippy::too_many_lines)]
pub fn parse_record_full(data: &[u8], frs: u64) -> ParseResult {
    use core::mem::size_of;

    use crate::ntfs::{
        AttributeRecordHeader, AttributeType, FileRecordSegmentHeader, file_reference_to_frs,
    };

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return ParseResult::Skip;
    }

    // SAFETY: We've verified the buffer is large enough for the header.
    let header: FileRecordSegmentHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

    // Check if record is in use
    if !header.is_in_use() {
        return ParseResult::Skip;
    }

    // Copy the packed field to avoid unaligned reference
    let multi_sector_header = header.multi_sector_header;
    if !multi_sector_header.is_file_record() {
        return ParseResult::Skip;
    }

    // Check if this is an extension record
    let is_extension = !header.is_base_record();
    let base_frs = if is_extension {
        file_reference_to_frs(header.base_file_record_segment)
    } else {
        frs
    };

    // Extract sequence number and LSN from header
    let sequence_number = header.sequence_number;
    let lsn = header.log_file_sequence_number;

    // Prepare result containers
    let mut names: Vec<NameInfo> = Vec::new();
    let mut streams: Vec<StreamInfo> = Vec::new();
    let mut std_info = ExtendedStandardInfo::default();
    let mut primary = PrimaryNameTracker::default();
    let mut reparse_tag: u32 = 0;
    let mut reparse_size: u64 = 0; // Size of $REPARSE_POINT attribute (for junctions/symlinks)
    let mut dir_index_size: u64 = 0; // Size of $INDEX_ROOT + $INDEX_ALLOCATION with name $I30
    let mut dir_index_allocated: u64 = 0; // Allocated size of directory index

    // Parse attributes
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        // SAFETY: We've verified offset + size_of::<AttributeRecordHeader>() <=
        // max_offset
        let attr_header: AttributeRecordHeader =
            unsafe { core::ptr::read(data[offset..].as_ptr().cast()) };

        if attr_header.type_code == AttributeType::End as u32 {
            break;
        }
        if attr_header.length == 0 || offset + attr_header.length as usize > max_offset {
            break;
        }

        match AttributeType::from_u32(attr_header.type_code) {
            Some(AttributeType::StandardInformation) if attr_header.is_non_resident == 0 => {
                parse_standard_info_full(data, offset, &mut std_info);
            }
            Some(AttributeType::FileName) if attr_header.is_non_resident == 0 => {
                if let Some(name_info) = parse_file_name_full(data, offset) {
                    if name_info.namespace != 2 {
                        // Skip DOS-only names
                        primary.update(&name_info);
                        names.push(name_info);
                    }
                }
            }
            Some(AttributeType::Data) => {
                if let Some(stream_info) =
                    parse_data_attribute_full(data, offset, &attr_header, frs)
                {
                    streams.push(stream_info);
                }
            }
            Some(AttributeType::ReparsePoint) => {
                // Parse $REPARSE_POINT to get the reparse tag and size
                // C++ handles both resident and non-resident reparse points:
                // - Resident: ah->Resident.ValueLength
                // - Non-resident: ah->NonResident.DataSize (rare, but possible)
                //
                // C++ also counts $REPARSE_POINT as a stream (line 696: ++stream_count)
                // This affects the descendants count in tree metrics.
                let (rp_size, rp_allocated, is_resident) = if attr_header.is_non_resident == 0 {
                    // Resident reparse point (common case)
                    let value_length = u32::from_le_bytes(
                        data.get(offset + 16..offset + 20)
                            .and_then(|b| b.try_into().ok())
                            .unwrap_or([0, 0, 0, 0]),
                    ) as u64;
                    reparse_size = value_length;

                    let value_offset = u16::from_le_bytes(
                        data.get(offset + 20..offset + 22)
                            .and_then(|b| b.try_into().ok())
                            .unwrap_or([0, 0]),
                    ) as usize;
                    let rp_offset = offset + value_offset;
                    if rp_offset + size_of::<ReparsePointHeader>() <= data.len() {
                        // SAFETY: We've verified the buffer is large enough
                        let rp_header: ReparsePointHeader =
                            unsafe { core::ptr::read(data[rp_offset..].as_ptr().cast()) };
                        reparse_tag = rp_header.reparse_tag;
                    }
                    (value_length, 0_u64, true)
                } else {
                    // Non-resident reparse point (rare - large reparse data)
                    // Use DataSize from non-resident header (at offset+32, NOT 40)
                    let nr_offset = offset + 16; // After common header
                    let (data_size, alloc_size) = if nr_offset + 48 <= data.len() {
                        let ds = i64::from_le_bytes(
                            data[nr_offset + 32..nr_offset + 40]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        let alloc = i64::from_le_bytes(
                            data[nr_offset + 24..nr_offset + 32]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        (ds.max(0) as u64, alloc.max(0) as u64)
                    } else {
                        (0_u64, 0_u64)
                    };
                    reparse_size = data_size;
                    // Note: Can't easily read reparse_tag from non-resident
                    // data without reading the actual data runs.
                    (data_size, alloc_size, false)
                };

                // C++ counts $REPARSE_POINT as a stream for descendants calculation
                // Add it as a special stream with name "$REPARSE" to distinguish from $DATA
                // Note: The size is already captured in reparse_size for the record's size
                // calculation, but we need the stream for stream_count.
                streams.push(StreamInfo {
                    name: String::from("$REPARSE"),
                    size: rp_size,
                    allocated_size: rp_allocated,
                    is_sparse: false,
                    is_compressed: false,
                    is_resident,
                });
            }
            Some(
                AttributeType::IndexRoot | AttributeType::IndexAllocation | AttributeType::Bitmap,
            ) => {
                // C++ includes $INDEX_ROOT, $INDEX_ALLOCATION, and $BITMAP with name $I30
                // in directory size (all merged into a single stream)
                // For non-$I30 indexes (like $SDH, $SII, $O, $Q, $R), C++ counts them as
                // streams

                // Extract attribute name
                let (is_i30, attr_name) = if attr_header.name_length > 0 {
                    let name_offset = offset + attr_header.name_offset as usize;
                    let name_len = attr_header.name_length as usize;
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        // Check for "$I30" in UTF-16LE
                        let is_i30 =
                            attr_header.name_length == 4 && name_bytes == b"$\x00I\x003\x000\x00";
                        // Decode name for non-$I30 indexes
                        let name = if is_i30 {
                            String::new()
                        } else {
                            let name_u16: smallvec::SmallVec<[u16; 64]> = name_bytes
                                .chunks_exact(2)
                                .filter_map(|chunk| {
                                    <[u8; 2]>::try_from(chunk).ok().map(u16::from_le_bytes)
                                })
                                .collect();
                            String::from_utf16(&name_u16).unwrap_or_default()
                        };
                        (is_i30, name)
                    } else {
                        (false, String::new())
                    }
                } else {
                    (false, String::new())
                };

                if is_i30 {
                    // This is a directory index attribute - accumulate into dir_index_size
                    if attr_header.is_non_resident == 0 {
                        // Resident: get size from resident header
                        let value_length = u32::from_le_bytes(
                            data.get(offset + 16..offset + 20)
                                .and_then(|b| b.try_into().ok())
                                .unwrap_or([0; 4]),
                        ) as u64;
                        dir_index_size += value_length;
                        // Resident attributes have no allocated size
                    } else {
                        // Non-resident: get sizes from non-resident header
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let allocated = i64::from_le_bytes(
                                data[nr_offset + 24..nr_offset + 32]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            let data_size = i64::from_le_bytes(
                                data[nr_offset + 32..nr_offset + 40]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            dir_index_size += data_size.max(0) as u64;
                            dir_index_allocated += allocated.max(0) as u64;
                        }
                    }
                } else {
                    // Non-$I30 index attribute - C++ counts these as streams
                    // Examples: $SDH, $SII (in $Secure), $O, $Q (in $Quota), $R (in $Reparse)
                    // Also includes unnamed $BITMAP (e.g., in $MFT)
                    let (size, allocated_size, is_resident) = if attr_header.is_non_resident == 0 {
                        let value_length = u32::from_le_bytes(
                            data.get(offset + 16..offset + 20)
                                .and_then(|b| b.try_into().ok())
                                .unwrap_or([0; 4]),
                        ) as u64;
                        (value_length, 0_u64, true)
                    } else {
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let allocated = i64::from_le_bytes(
                                data[nr_offset + 24..nr_offset + 32]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            let data_size = i64::from_le_bytes(
                                data[nr_offset + 32..nr_offset + 40]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            (data_size.max(0) as u64, allocated.max(0) as u64, false)
                        } else {
                            (0_u64, 0_u64, false)
                        }
                    };
                    // Use attribute type name if no explicit name
                    let stream_name = if attr_name.is_empty() {
                        match AttributeType::from_u32(attr_header.type_code) {
                            Some(AttributeType::Bitmap) => String::from("$BITMAP"),
                            Some(AttributeType::IndexRoot) => String::from("$INDEX_ROOT"),
                            Some(AttributeType::IndexAllocation) => {
                                String::from("$INDEX_ALLOCATION")
                            }
                            _ => String::new(),
                        }
                    } else {
                        attr_name
                    };
                    streams.push(StreamInfo {
                        name: stream_name,
                        size,
                        allocated_size,
                        is_sparse: false,
                        is_compressed: false,
                        is_resident,
                    });
                }
            }
            // C++ counts these attribute types as streams (lines 590-600 in ntfs_index.hpp):
            // - $OBJECT_ID (0x40)
            // - $VOLUME_NAME (0x60)
            // - $VOLUME_INFORMATION (0x70)
            // - $PROPERTY_SET (0xF0)
            // - $EA (0xE0)
            // - $EA_INFORMATION (0xD0)
            // - $LOGGED_UTILITY_STREAM (0x100) - falls through to default: case in C++
            // - And any other attribute type (default case)
            Some(
                AttributeType::ObjectId
                | AttributeType::VolumeName
                | AttributeType::VolumeInformation
                | AttributeType::PropertySet
                | AttributeType::Ea
                | AttributeType::EaInformation
                | AttributeType::LoggedUtilityStream,
            ) => {
                // Note: LoggedUtilityStream IS counted as a stream in C++ via the default: case
                // The commented out line 589 just means it's not an explicit case, so it falls
                // through

                // Extract attribute name (if any)
                let attr_name = if attr_header.name_length > 0 {
                    let name_offset = offset + attr_header.name_offset as usize;
                    let name_len = attr_header.name_length as usize;
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        let name_u16: smallvec::SmallVec<[u16; 64]> = name_bytes
                            .chunks_exact(2)
                            .filter_map(|chunk| {
                                <[u8; 2]>::try_from(chunk).ok().map(u16::from_le_bytes)
                            })
                            .collect();
                        String::from_utf16(&name_u16).unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                // Get size information
                let (size, allocated_size, is_resident) = if attr_header.is_non_resident == 0 {
                    let value_length = u32::from_le_bytes(
                        data.get(offset + 16..offset + 20)
                            .and_then(|b| b.try_into().ok())
                            .unwrap_or([0; 4]),
                    ) as u64;
                    (value_length, 0_u64, true)
                } else {
                    let nr_offset = offset + 16;
                    if nr_offset + 48 <= data.len() {
                        let allocated = i64::from_le_bytes(
                            data[nr_offset + 24..nr_offset + 32]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        let data_size = i64::from_le_bytes(
                            data[nr_offset + 32..nr_offset + 40]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        (data_size.max(0) as u64, allocated.max(0) as u64, false)
                    } else {
                        (0_u64, 0_u64, false)
                    }
                };

                // Create a stream name that identifies the attribute type
                // Note: LoggedUtilityStream (0x100) must have a synthetic name to survive
                // the named_streams filter in index.rs - otherwise its size is dropped
                // while still being counted, causing the 48-byte parity gap with C++.
                let stream_name = if attr_name.is_empty() {
                    match AttributeType::from_u32(attr_header.type_code) {
                        Some(AttributeType::ObjectId) => String::from("$OBJECT_ID"),
                        Some(AttributeType::VolumeName) => String::from("$VOLUME_NAME"),
                        Some(AttributeType::VolumeInformation) => {
                            String::from("$VOLUME_INFORMATION")
                        }
                        Some(AttributeType::PropertySet) => String::from("$PROPERTY_SET"),
                        Some(AttributeType::Ea) => String::from("$EA"),
                        Some(AttributeType::EaInformation) => String::from("$EA_INFORMATION"),
                        Some(AttributeType::LoggedUtilityStream) => {
                            String::from("$LOGGED_UTILITY_STREAM")
                        }
                        _ => String::new(),
                    }
                } else {
                    attr_name
                };

                streams.push(StreamInfo {
                    name: stream_name,
                    size,
                    allocated_size,
                    is_sparse: false,
                    is_compressed: false,
                    is_resident,
                });
            }
            // Skip known non-stream attributes silently
            Some(
                AttributeType::StandardInformation
                | AttributeType::FileName
                | AttributeType::AttributeList
                | AttributeType::SecurityDescriptor,
            ) => {}
            _ => {
                // Unknown attribute type - log at trace level for debugging
                // Copy fields from packed struct to avoid unaligned reference
                let type_code = attr_header.type_code;
                let name_len = attr_header.name_length;
                debug!(
                    frs,
                    attr_type_code = type_code,
                    name_length = name_len,
                    "Unknown attribute type"
                );
            }
        }
        offset += attr_header.length as usize;
    }

    // Handle extension records
    if is_extension {
        return ParseResult::Extension(ExtensionAttributes {
            base_frs,
            extension_frs: frs,
            names,
            streams,
        });
    }

    // Note: We do NOT skip records without a $FILE_NAME attribute here.
    // Some records have their $FILE_NAME attributes in extension records
    // (when the base record has an $ATTRIBUTE_LIST). These base records
    // will have their names populated during the merge step.
    // C++ handles this by processing all records in a single pass and
    // looking up the base record for each extension record.

    // Calculate primary size from default stream
    // For reparse points (junctions/symlinks), use $REPARSE_POINT size if no $DATA
    // stream
    // For directories, C++ includes $INDEX_ROOT + $INDEX_ALLOCATION size
    let is_directory = header.is_directory();

    // For directories with $I30 index, add a stream entry so it's counted in
    // total_stream_count C++ counts the merged $I30 as a stream with
    // type_name_id=0 (line 4590: info->type_name_id = type_name_id)
    // This is essential for tree metrics parity - each directory's $I30 contributes
    // +1 to descendants
    if is_directory && dir_index_size > 0 {
        // Add $I30 as the default stream (empty name) for directories
        // This matches C++ behavior where $I30 is the "default" stream for directories
        // just like $DATA is the default stream for files
        streams.push(StreamInfo {
            name: String::new(), // Empty name = default stream
            size: dir_index_size,
            allocated_size: dir_index_allocated,
            is_sparse: false,
            is_compressed: false,
            is_resident: false, // $INDEX_ALLOCATION is typically non-resident
        });
    }

    let (size, allocated_size) = if is_directory && dir_index_size > 0 {
        // Directory with index allocation - use index size (C++ parity)
        (dir_index_size, dir_index_allocated)
    } else {
        streams.iter().find(|s| s.name.is_empty()).map_or_else(
            || {
                // No default $DATA stream - use reparse_size for junctions/symlinks
                // C++ uses ah->Resident.ValueLength for reparse points
                if reparse_tag != 0 {
                    (reparse_size, 0) // Reparse point data is resident, allocated=0
                } else {
                    (0, 0)
                }
            },
            |s| (s.size, s.allocated_size),
        )
    };

    ParseResult::Base(ParsedRecord {
        frs,
        sequence_number,
        lsn,
        parent_frs: primary.parent_frs,
        name: primary.name,
        namespace: primary.namespace,
        names,
        streams,
        size,
        allocated_size,
        std_info,
        in_use: true,
        is_directory,
        fn_created: primary.fn_created,
        fn_modified: primary.fn_modified,
        fn_accessed: primary.fn_accessed,
        fn_mft_changed: primary.fn_mft_changed,
        reparse_tag,
        // P3 forensic fields (not populated in normal mode)
        is_deleted: false,
        is_corrupt: false,
        is_extension: false,
        base_frs: 0,
    })
}

/// Parses an MFT record with forensic options.
///
/// This function extends `parse_record_full` to support forensic analysis:
/// - `include_deleted`: Returns deleted records (`FRH_IN_USE` not set)
/// - `include_corrupt`: Returns records with corrupt fixup (handled by caller)
/// - `include_extensions`: Returns extension records as separate
///   `ParsedRecord`s
///
/// # Arguments
///
/// * `data` - The raw record data (after fixup, or raw if checking corrupt)
/// * `frs` - The File Record Segment number
/// * `options` - Forensic parsing options
/// * `is_corrupt` - True if fixup failed (set by caller)
///
/// # Returns
///
/// `ParseResult::Base` for all records matching options, or
/// `ParseResult::Skip`.
#[must_use]
#[allow(unsafe_code, clippy::too_many_lines, clippy::cognitive_complexity)]
pub fn parse_record_forensic(
    data: &[u8],
    frs: u64,
    options: &ParseOptions,
    is_corrupt: bool,
) -> ParseResult {
    use core::mem::size_of;

    use crate::ntfs::{
        AttributeRecordHeader, AttributeType, FileRecordSegmentHeader, file_reference_to_frs,
    };

    // Handle corrupt records
    if is_corrupt {
        if !options.include_corrupt {
            return ParseResult::Skip;
        }
        // Return a minimal record for corrupt entries
        return ParseResult::Base(ParsedRecord {
            frs,
            name: format!("<CORRUPT:{frs}>"),
            is_corrupt: true,
            ..Default::default()
        });
    }

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return ParseResult::Skip;
    }

    // SAFETY: We've verified the buffer is large enough for the header.
    let header: FileRecordSegmentHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

    // Check if record is in use
    let is_deleted = !header.is_in_use();
    if is_deleted && !options.include_deleted {
        return ParseResult::Skip;
    }

    // Copy the packed field to avoid unaligned reference
    let multi_sector_header = header.multi_sector_header;
    if !multi_sector_header.is_file_record() {
        return ParseResult::Skip;
    }

    // Check if this is an extension record
    let is_extension_record = !header.is_base_record();
    let base_frs_value = if is_extension_record {
        file_reference_to_frs(header.base_file_record_segment)
    } else {
        0
    };

    // In non-forensic mode, return Extension variant for merging
    if is_extension_record && !options.include_extensions {
        // Parse attributes for extension merging
        let mut names: Vec<NameInfo> = Vec::new();
        let mut streams: Vec<StreamInfo> = Vec::new();

        let mut offset = header.first_attribute_offset as usize;
        let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

        while offset + size_of::<AttributeRecordHeader>() <= max_offset {
            // SAFETY: Bounds checked above; AttributeRecordHeader is repr(C) and packed.
            let attr_header: AttributeRecordHeader =
                unsafe { core::ptr::read(data[offset..].as_ptr().cast()) };

            if attr_header.type_code == AttributeType::End as u32 {
                break;
            }
            if attr_header.length == 0 || offset + attr_header.length as usize > max_offset {
                break;
            }

            match AttributeType::from_u32(attr_header.type_code) {
                Some(AttributeType::FileName) if attr_header.is_non_resident == 0 => {
                    if let Some(name_info) = parse_file_name_full(data, offset) {
                        if name_info.namespace != 2 {
                            names.push(name_info);
                        }
                    }
                }
                Some(AttributeType::Data) => {
                    if let Some(stream_info) =
                        parse_data_attribute_full(data, offset, &attr_header, frs)
                    {
                        streams.push(stream_info);
                    }
                }
                _ => {}
            }
            offset += attr_header.length as usize;
        }

        return ParseResult::Extension(ExtensionAttributes {
            base_frs: base_frs_value,
            extension_frs: frs,
            names,
            streams,
        });
    }

    // Extract sequence number and LSN from header
    let sequence_number = header.sequence_number;
    let lsn = header.log_file_sequence_number;

    // Prepare result containers
    let mut names: Vec<NameInfo> = Vec::new();
    let mut streams: Vec<StreamInfo> = Vec::new();
    let mut std_info = ExtendedStandardInfo::default();
    let mut primary = PrimaryNameTracker::default();
    let mut reparse_tag: u32 = 0;
    let mut reparse_size: u64 = 0; // Size of $REPARSE_POINT attribute (for junctions/symlinks)
    let mut dir_index_size: u64 = 0; // Size of $INDEX_ROOT + $INDEX_ALLOCATION with name $I30
    let mut dir_index_allocated: u64 = 0; // Allocated size of directory index

    // Parse attributes
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        // SAFETY: Bounds checked above; AttributeRecordHeader is repr(C) and packed.
        let attr_header: AttributeRecordHeader =
            unsafe { core::ptr::read(data[offset..].as_ptr().cast()) };

        if attr_header.type_code == AttributeType::End as u32 {
            break;
        }
        if attr_header.length == 0 || offset + attr_header.length as usize > max_offset {
            break;
        }

        match AttributeType::from_u32(attr_header.type_code) {
            Some(AttributeType::StandardInformation) if attr_header.is_non_resident == 0 => {
                parse_standard_info_full(data, offset, &mut std_info);
            }
            Some(AttributeType::FileName) if attr_header.is_non_resident == 0 => {
                if let Some(name_info) = parse_file_name_full(data, offset) {
                    if name_info.namespace != 2 {
                        primary.update(&name_info);
                        names.push(name_info);
                    }
                }
            }
            Some(AttributeType::Data) => {
                if let Some(stream_info) =
                    parse_data_attribute_full(data, offset, &attr_header, frs)
                {
                    streams.push(stream_info);
                }
            }
            Some(AttributeType::ReparsePoint) => {
                // Parse $REPARSE_POINT to get the reparse tag and size
                // C++ handles both resident and non-resident reparse points:
                // - Resident: ah->Resident.ValueLength
                // - Non-resident: ah->NonResident.DataSize (rare, but possible)
                //
                // C++ also counts $REPARSE_POINT as a stream (line 696: ++stream_count)
                // This affects the descendants count in tree metrics.
                let (rp_size, rp_allocated, is_resident) = if attr_header.is_non_resident == 0 {
                    // Resident reparse point (common case)
                    let value_length = u32::from_le_bytes(
                        data.get(offset + 16..offset + 20)
                            .and_then(|b| b.try_into().ok())
                            .unwrap_or([0, 0, 0, 0]),
                    ) as u64;
                    reparse_size = value_length;

                    let value_offset = u16::from_le_bytes(
                        data.get(offset + 20..offset + 22)
                            .and_then(|b| b.try_into().ok())
                            .unwrap_or([0, 0]),
                    ) as usize;
                    let rp_offset = offset + value_offset;
                    if rp_offset + size_of::<ReparsePointHeader>() <= data.len() {
                        // SAFETY: Bounds checked above; ReparsePointHeader is repr(C).
                        let rp_header: ReparsePointHeader =
                            unsafe { core::ptr::read(data[rp_offset..].as_ptr().cast()) };
                        reparse_tag = rp_header.reparse_tag;
                    }
                    (value_length, 0_u64, true)
                } else {
                    // Non-resident reparse point (rare - large reparse data)
                    // Use DataSize from non-resident header (at offset+32, NOT 40)
                    let nr_offset = offset + 16; // After common header
                    let (data_size, alloc_size) = if nr_offset + 48 <= data.len() {
                        let ds = i64::from_le_bytes(
                            data[nr_offset + 32..nr_offset + 40]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        let alloc = i64::from_le_bytes(
                            data[nr_offset + 24..nr_offset + 32]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        (ds.max(0) as u64, alloc.max(0) as u64)
                    } else {
                        (0_u64, 0_u64)
                    };
                    reparse_size = data_size;
                    // Note: Can't easily read reparse_tag from non-resident
                    // data without reading the actual data runs.
                    (data_size, alloc_size, false)
                };

                // C++ counts $REPARSE_POINT as a stream for descendants calculation
                streams.push(StreamInfo {
                    name: String::from("$REPARSE"),
                    size: rp_size,
                    allocated_size: rp_allocated,
                    is_sparse: false,
                    is_compressed: false,
                    is_resident,
                });
            }
            Some(
                AttributeType::IndexRoot | AttributeType::IndexAllocation | AttributeType::Bitmap,
            ) => {
                // C++ includes $INDEX_ROOT, $INDEX_ALLOCATION, and $BITMAP with name $I30
                // in directory size (all merged into a single stream)
                // For non-$I30 indexes (like $SDH, $SII, $O, $Q, $R), C++ counts them as
                // streams

                // Extract attribute name
                let (is_i30, attr_name) = if attr_header.name_length > 0 {
                    let name_offset = offset + attr_header.name_offset as usize;
                    let name_len = attr_header.name_length as usize;
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        // Check for "$I30" in UTF-16LE
                        let is_i30 =
                            attr_header.name_length == 4 && name_bytes == b"$\x00I\x003\x000\x00";
                        // Decode name for non-$I30 indexes
                        let name = if is_i30 {
                            String::new()
                        } else {
                            let name_u16: smallvec::SmallVec<[u16; 64]> = name_bytes
                                .chunks_exact(2)
                                .filter_map(|chunk| {
                                    <[u8; 2]>::try_from(chunk).ok().map(u16::from_le_bytes)
                                })
                                .collect();
                            String::from_utf16(&name_u16).unwrap_or_default()
                        };
                        (is_i30, name)
                    } else {
                        (false, String::new())
                    }
                } else {
                    (false, String::new())
                };

                if is_i30 {
                    // This is a directory index attribute - accumulate into dir_index_size
                    if attr_header.is_non_resident == 0 {
                        // Resident: get size from resident header
                        let value_length = u32::from_le_bytes(
                            data.get(offset + 16..offset + 20)
                                .and_then(|b| b.try_into().ok())
                                .unwrap_or([0; 4]),
                        ) as u64;
                        dir_index_size += value_length;
                        // Resident attributes have no allocated size
                    } else {
                        // Non-resident: get sizes from non-resident header
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let allocated = i64::from_le_bytes(
                                data[nr_offset + 24..nr_offset + 32]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            let data_size = i64::from_le_bytes(
                                data[nr_offset + 32..nr_offset + 40]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            dir_index_size += data_size.max(0) as u64;
                            dir_index_allocated += allocated.max(0) as u64;
                        }
                    }
                } else {
                    // Non-$I30 index attribute - C++ counts these as streams
                    // Examples: $SDH, $SII (in $Secure), $O, $Q (in $Quota), $R (in $Reparse)
                    // Also includes unnamed $BITMAP (e.g., in $MFT)
                    let (size, allocated_size, is_resident) = if attr_header.is_non_resident == 0 {
                        let value_length = u32::from_le_bytes(
                            data.get(offset + 16..offset + 20)
                                .and_then(|b| b.try_into().ok())
                                .unwrap_or([0; 4]),
                        ) as u64;
                        (value_length, 0_u64, true)
                    } else {
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let allocated = i64::from_le_bytes(
                                data[nr_offset + 24..nr_offset + 32]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            let data_size = i64::from_le_bytes(
                                data[nr_offset + 32..nr_offset + 40]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            (data_size.max(0) as u64, allocated.max(0) as u64, false)
                        } else {
                            (0_u64, 0_u64, false)
                        }
                    };
                    // Use attribute type name if no explicit name
                    let stream_name = if attr_name.is_empty() {
                        match AttributeType::from_u32(attr_header.type_code) {
                            Some(AttributeType::Bitmap) => String::from("$BITMAP"),
                            Some(AttributeType::IndexRoot) => String::from("$INDEX_ROOT"),
                            Some(AttributeType::IndexAllocation) => {
                                String::from("$INDEX_ALLOCATION")
                            }
                            _ => String::new(),
                        }
                    } else {
                        attr_name
                    };
                    streams.push(StreamInfo {
                        name: stream_name,
                        size,
                        allocated_size,
                        is_sparse: false,
                        is_compressed: false,
                        is_resident,
                    });
                }
            }
            // C++ counts these attribute types as streams (lines 590-600 in ntfs_index.hpp):
            // - $OBJECT_ID (0x40)
            // - $VOLUME_NAME (0x60)
            // - $VOLUME_INFORMATION (0x70)
            // - $PROPERTY_SET (0xF0)
            // - $EA (0xE0)
            // - $EA_INFORMATION (0xD0)
            // - $LOGGED_UTILITY_STREAM (0x100) - falls through to default: case in C++
            Some(
                AttributeType::ObjectId
                | AttributeType::VolumeName
                | AttributeType::VolumeInformation
                | AttributeType::PropertySet
                | AttributeType::Ea
                | AttributeType::EaInformation
                | AttributeType::LoggedUtilityStream,
            ) => {
                // Extract attribute name (if any)
                let attr_name = if attr_header.name_length > 0 {
                    let name_offset = offset + attr_header.name_offset as usize;
                    let name_len = attr_header.name_length as usize;
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        let name_u16: smallvec::SmallVec<[u16; 64]> = name_bytes
                            .chunks_exact(2)
                            .filter_map(|chunk| {
                                <[u8; 2]>::try_from(chunk).ok().map(u16::from_le_bytes)
                            })
                            .collect();
                        String::from_utf16(&name_u16).unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                // Get size information
                let (size, allocated_size, is_resident) = if attr_header.is_non_resident == 0 {
                    let value_length = u32::from_le_bytes(
                        data.get(offset + 16..offset + 20)
                            .and_then(|b| b.try_into().ok())
                            .unwrap_or([0; 4]),
                    ) as u64;
                    (value_length, 0_u64, true)
                } else {
                    let nr_offset = offset + 16;
                    if nr_offset + 48 <= data.len() {
                        let allocated = i64::from_le_bytes(
                            data[nr_offset + 24..nr_offset + 32]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        let data_size = i64::from_le_bytes(
                            data[nr_offset + 32..nr_offset + 40]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        (data_size.max(0) as u64, allocated.max(0) as u64, false)
                    } else {
                        (0_u64, 0_u64, false)
                    }
                };

                // Create a stream name that identifies the attribute type
                // Note: LoggedUtilityStream (0x100) must have a synthetic name to survive
                // the named_streams filter in index.rs - otherwise its size is dropped
                // while still being counted, causing the 48-byte parity gap with C++.
                let stream_name = if attr_name.is_empty() {
                    match AttributeType::from_u32(attr_header.type_code) {
                        Some(AttributeType::ObjectId) => String::from("$OBJECT_ID"),
                        Some(AttributeType::VolumeName) => String::from("$VOLUME_NAME"),
                        Some(AttributeType::VolumeInformation) => {
                            String::from("$VOLUME_INFORMATION")
                        }
                        Some(AttributeType::PropertySet) => String::from("$PROPERTY_SET"),
                        Some(AttributeType::Ea) => String::from("$EA"),
                        Some(AttributeType::EaInformation) => String::from("$EA_INFORMATION"),
                        Some(AttributeType::LoggedUtilityStream) => {
                            String::from("$LOGGED_UTILITY_STREAM")
                        }
                        _ => String::new(),
                    }
                } else {
                    attr_name
                };

                streams.push(StreamInfo {
                    name: stream_name,
                    size,
                    allocated_size,
                    is_sparse: false,
                    is_compressed: false,
                    is_resident,
                });
            }
            _ => {}
        }
        offset += attr_header.length as usize;
    }

    // For deleted/extension records without $FILE_NAME, use FRS as name
    // Note: Normal records without $FILE_NAME may have their names in extension
    // records (when the base record has an $ATTRIBUTE_LIST). These will be
    // populated during the merge step.
    let name = if primary.name.is_empty() {
        if is_deleted {
            format!("<DELETED:{frs}>")
        } else if is_extension_record {
            format!("<EXT:{frs}→{base_frs_value}>")
        } else {
            // Normal record without name - keep as placeholder for merge step
            String::new()
        }
    } else {
        primary.name
    };

    // Calculate primary size from default stream
    // For reparse points (junctions/symlinks), use $REPARSE_POINT size if no $DATA
    // stream
    // For directories, C++ includes $INDEX_ROOT + $INDEX_ALLOCATION size
    let is_directory = header.is_directory();

    // For directories with $I30 index, add a stream entry so it's counted in
    // total_stream_count C++ counts the merged $I30 as a stream with
    // type_name_id=0 (line 4590: info->type_name_id = type_name_id)
    // This is essential for tree metrics parity - each directory's $I30 contributes
    // +1 to descendants
    if is_directory && dir_index_size > 0 {
        // Add $I30 as the default stream (empty name) for directories
        // This matches C++ behavior where $I30 is the "default" stream for directories
        // just like $DATA is the default stream for files
        streams.push(StreamInfo {
            name: String::new(), // Empty name = default stream
            size: dir_index_size,
            allocated_size: dir_index_allocated,
            is_sparse: false,
            is_compressed: false,
            is_resident: false, // $INDEX_ALLOCATION is typically non-resident
        });
    }

    let (size, allocated_size) = if is_directory && dir_index_size > 0 {
        // Directory with index allocation - use index size (C++ parity)
        (dir_index_size, dir_index_allocated)
    } else {
        streams.iter().find(|s| s.name.is_empty()).map_or_else(
            || {
                // No default $DATA stream - use reparse_size for junctions/symlinks
                // C++ uses ah->Resident.ValueLength for reparse points
                if reparse_tag != 0 {
                    (reparse_size, 0) // Reparse point data is resident, allocated=0
                } else {
                    (0, 0)
                }
            },
            |s| (s.size, s.allocated_size),
        )
    };

    ParseResult::Base(ParsedRecord {
        frs,
        sequence_number,
        lsn,
        parent_frs: primary.parent_frs,
        name,
        namespace: primary.namespace,
        names,
        streams,
        size,
        allocated_size,
        std_info,
        in_use: !is_deleted,
        is_directory,
        fn_created: primary.fn_created,
        fn_modified: primary.fn_modified,
        fn_accessed: primary.fn_accessed,
        fn_mft_changed: primary.fn_mft_changed,
        reparse_tag,
        // P3 forensic fields
        is_deleted,
        is_corrupt: false,
        is_extension: is_extension_record,
        base_frs: base_frs_value,
    })
}

/// Legacy parse function for backward compatibility.
///
/// This function skips extension records and returns `Option<ParsedRecord>`.
#[must_use]
pub fn parse_record(data: &[u8], frs: u64) -> Option<ParsedRecord> {
    match parse_record_full(data, frs) {
        ParseResult::Base(record) => Some(record),
        ParseResult::Extension(_) | ParseResult::Skip => None,
    }
}

/// Parses a record using a thread-local buffer to avoid allocation.
///
/// This function copies the record data into a thread-local buffer, applies
/// fixup, and parses it. This avoids per-record heap allocations in hot loops.
///
/// # Arguments
///
/// * `data` - The raw record data (will be copied to thread-local buffer)
/// * `frs` - The File Record Segment number
///
/// # Returns
///
/// `ParseResult::Base` for base records, `ParseResult::Extension` for
/// extension records, or `ParseResult::Skip` if invalid/not in use.
#[must_use]
pub fn parse_record_zero_alloc(data: &[u8], frs: u64) -> ParseResult {
    RECORD_BUFFER.with(|buf| {
        let mut buffer = buf.borrow_mut();

        // Ensure buffer is large enough
        if buffer.len() < data.len() {
            buffer.resize(data.len(), 0);
        }

        // Copy data into thread-local buffer
        buffer[..data.len()].copy_from_slice(data);

        // Apply fixup in place
        if !apply_fixup(&mut buffer[..data.len()]) {
            return ParseResult::Skip;
        }

        // Parse the record
        parse_record_full(&buffer[..data.len()], frs)
    })
}

/// Parses a record with forensic options using a thread-local buffer.
///
/// This is the forensic variant of `parse_record_zero_alloc` that supports
/// deleted, corrupt, and extension record extraction.
///
/// # Arguments
///
/// * `data` - The raw record data (will be copied to thread-local buffer)
/// * `frs` - The File Record Segment number
/// * `options` - Forensic parsing options
///
/// # Returns
///
/// `ParseResult::Base` for records matching options, `ParseResult::Extension`
/// for extension records (when not in forensic mode), or `ParseResult::Skip`.
#[must_use]
pub fn parse_record_zero_alloc_forensic(
    data: &[u8],
    frs: u64,
    options: &ParseOptions,
) -> ParseResult {
    RECORD_BUFFER.with(|buf| {
        let mut buffer = buf.borrow_mut();

        // Ensure buffer is large enough
        if buffer.len() < data.len() {
            buffer.resize(data.len(), 0);
        }

        // Copy data into thread-local buffer
        buffer[..data.len()].copy_from_slice(data);

        // Apply fixup in place
        let fixup_ok = apply_fixup(&mut buffer[..data.len()]);

        // In forensic mode, we may want corrupt records
        if !fixup_ok {
            return parse_record_forensic(&buffer[..data.len()], frs, options, true);
        }

        // Parse the record with forensic options
        parse_record_forensic(&buffer[..data.len()], frs, options, false)
    })
}

// ============================================================================
// MFT Record Merger (Cross-Platform)
// ============================================================================

/// Merges extension record attributes into base records.
///
/// This implements the C++ behavior where attributes from extension
/// records are merged into their base records.
///
/// # Performance Optimization (2026-01-23)
///
/// Uses `Vec<Option<ParsedRecord>>` indexed directly by FRS instead of
/// `HashMap`. This eliminates all hash computations (11.7M `SipHash` calls on
/// large MFTs). FRS numbers are sequential 0..N, making direct indexing O(1)
/// with no overhead.
///
/// Expected improvement: 20-30% overall (was 13% of CPU time in `HashMap` ops).
///
/// # Cross-Platform
///
/// This struct is cross-platform and can be used on all platforms.
/// It only depends on `ParsedRecord`, `ParseResult`, `ExtensionAttributes`,
/// and `ParsedColumns` which are all cross-platform.
pub struct MftRecordMerger {
    /// Base records indexed directly by FRS number.
    /// `base_records[frs]` = Some(record) if present, None otherwise.
    base_records: Vec<Option<ParsedRecord>>,
    /// Pending extension attributes.
    extensions: Vec<ExtensionAttributes>,
    /// Count of base records (for efficient `len()`)
    base_count: usize,
}

impl MftRecordMerger {
    /// Creates a new merger.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            base_records: Vec::new(),
            extensions: Vec::new(),
            base_count: 0,
        }
    }

    /// Creates a new merger with capacity for `max_frs` records.
    ///
    /// # Arguments
    ///
    /// * `max_frs` - The maximum FRS number expected (typically `total_records`
    ///   from MFT)
    #[must_use]
    pub fn with_capacity(max_frs: usize) -> Self {
        Self {
            // Pre-allocate for direct FRS indexing
            base_records: vec![None; max_frs + 1],
            extensions: Vec::with_capacity(max_frs / 100), // Extensions are rare
            base_count: 0,
        }
    }

    /// Adds a parse result to the merger.
    ///
    /// # Performance
    ///
    /// O(1) insertion - direct index assignment, no hashing.
    #[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)] // bounds checked; FRS fits in usize
    pub fn add_result(&mut self, result: ParseResult) {
        match result {
            ParseResult::Base(record) => {
                let frs = record.frs as usize;
                // Expand if needed (rare - only if FRS exceeds initial capacity)
                if frs >= self.base_records.len() {
                    self.base_records.resize(frs + 1, None);
                }
                if self.base_records[frs].is_none() {
                    self.base_count += 1;
                }
                self.base_records[frs] = Some(record);
            }
            ParseResult::Extension(ext) => {
                self.extensions.push(ext);
            }
            ParseResult::Skip => {}
        }
    }

    /// Merges all extensions into their base records and returns the result.
    #[must_use]
    #[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)] // bounds checked; FRS fits in usize
    pub fn merge(mut self) -> Vec<ParsedRecord> {
        // Merge all extensions into their base records
        for ext in self.extensions {
            let base_frs = ext.base_frs as usize;
            if base_frs < self.base_records.len() {
                if let Some(ref mut base) = self.base_records[base_frs] {
                    // Merge names (avoiding duplicates)
                    for name in ext.names {
                        if !base
                            .names
                            .iter()
                            .any(|n| n.name == name.name && n.parent_frs == name.parent_frs)
                        {
                            base.names.push(name);
                        }
                    }
                    // Merge streams (avoiding duplicates)
                    for stream in ext.streams {
                        if !base.streams.iter().any(|s| s.name == stream.name) {
                            base.streams.push(stream);
                        }
                    }
                }
            }
        }

        // Recalculate sizes from merged streams and fix primary name if needed
        let mut result = Vec::with_capacity(self.base_count);
        for record in self.base_records.iter_mut().flatten() {
            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
            }

            // If base record had no $FILE_NAME but extensions added names,
            // update the primary name from the first available name.
            // This handles cases where all $FILE_NAME attributes are in extension records.
            if record.name.is_empty() && !record.names.is_empty() {
                // Find the best name (prefer Win32/Win32+DOS namespace)
                let best_name = record
                    .names
                    .iter()
                    .rfind(|name| matches!(name.namespace, 1 | 3))
                    .or_else(|| record.names.first());
                if let Some(name_info) = best_name {
                    record.name = name_info.name.clone();
                    record.parent_frs = name_info.parent_frs;
                    record.namespace = name_info.namespace;
                    record.fn_created = name_info.fn_created;
                    record.fn_modified = name_info.fn_modified;
                    record.fn_accessed = name_info.fn_accessed;
                    record.fn_mft_changed = name_info.fn_mft_changed;
                }
            }
        }

        // Collect non-None records that have a name
        // Records without a name after merging have no $FILE_NAME attributes
        // (not even in extension records) and should be skipped
        for record in self.base_records.into_iter().flatten() {
            if !record.name.is_empty() {
                result.push(record);
            }
        }

        result
    }

    /// Returns the number of base records.
    #[must_use]
    pub const fn base_count(&self) -> usize {
        self.base_count
    }

    /// Returns the number of pending extensions.
    #[must_use]
    pub fn extension_count(&self) -> usize {
        self.extensions.len()
    }

    /// Merges all extensions and returns the result as `ParsedColumns` (`SoA`
    /// layout).
    ///
    /// This is more efficient than `merge()` followed by conversion because it
    /// avoids creating an intermediate `Vec<ParsedRecord>`.
    ///
    /// # Arguments
    ///
    /// * `expand_links` - If `true` (default), expand hard links to separate
    ///   rows (matching C++ behavior and user expectations). If `false`, output
    ///   one row per unique FRS (power user mode).
    #[must_use]
    pub fn merge_into_columns(self, expand_links: bool) -> ParsedColumns {
        self.merge_into_columns_internal(expand_links)
    }

    /// Internal implementation for `merge_into_columns`.
    #[allow(clippy::indexing_slicing)]
    fn merge_into_columns_internal(mut self, expand_links: bool) -> ParsedColumns {
        // Merge all extensions into their base records
        for ext in self.extensions {
            // FRS values are bounded by MFT size, always < 2^32 on real systems
            let base_frs = usize::try_from(ext.base_frs).unwrap_or(usize::MAX);
            if base_frs < self.base_records.len() {
                if let Some(ref mut base) = self.base_records[base_frs] {
                    // Merge names (avoiding duplicates)
                    for name in ext.names {
                        if !base
                            .names
                            .iter()
                            .any(|n| n.name == name.name && n.parent_frs == name.parent_frs)
                        {
                            base.names.push(name);
                        }
                    }
                    // Merge streams (avoiding duplicates)
                    for stream in ext.streams {
                        if !base.streams.iter().any(|s| s.name == stream.name) {
                            base.streams.push(stream);
                        }
                    }
                }
            }
        }

        // Recalculate sizes from merged streams and fix primary name if needed
        // BUT: Don't overwrite directory sizes - they come from
        // $INDEX_ROOT/$INDEX_ALLOCATION which are already correctly set during
        // parsing
        for record in self.base_records.iter_mut().flatten() {
            if !record.is_directory {
                if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                    record.size = default_stream.size;
                    record.allocated_size = default_stream.allocated_size;
                }
            }

            // If base record had no $FILE_NAME but extensions added names,
            // update the primary name from the first available name.
            // This handles cases where all $FILE_NAME attributes are in extension records.
            if record.name.is_empty() && !record.names.is_empty() {
                // Find the best name (prefer Win32/Win32+DOS namespace)
                let best_name = record
                    .names
                    .iter()
                    .rfind(|name| matches!(name.namespace, 1 | 3))
                    .or_else(|| record.names.first());
                if let Some(name_info) = best_name {
                    record.name = name_info.name.clone();
                    record.parent_frs = name_info.parent_frs;
                    record.namespace = name_info.namespace;
                    record.fn_created = name_info.fn_created;
                    record.fn_modified = name_info.fn_modified;
                    record.fn_accessed = name_info.fn_accessed;
                    record.fn_mft_changed = name_info.fn_mft_changed;
                }
            }
        }

        // Estimate capacity: if expanding links, we need more space
        // Use integer arithmetic to avoid float precision issues
        let estimated_capacity = if expand_links {
            // Rough estimate: assume average of 1.2 links per file (base_count * 6 / 5)
            self.base_count.saturating_mul(6) / 5
        } else {
            self.base_count
        };

        // Convert directly to ParsedColumns (single pass, no intermediate Vec)
        // Skip records with empty names - they have no $FILE_NAME attributes
        let mut columns = ParsedColumns::with_capacity(estimated_capacity);
        for record in self.base_records.into_iter().flatten() {
            // Skip records without a name after merging
            if record.name.is_empty() {
                continue;
            }
            if expand_links {
                columns.push_record_expanded(&record);
            } else {
                columns.push_record(&record);
            }
        }
        columns
    }
}

impl Default for MftRecordMerger {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// ParsedColumns - Struct-of-Arrays (SoA) Layout for DataFrame Building
// ============================================================================

/// Column-oriented storage for parsed MFT records (Struct-of-Arrays layout).
///
/// This struct stores MFT record data in column vectors rather than as an
/// array of structs. This layout is optimal for:
/// - Direct conversion to Polars `DataFrame` (no transpose needed)
/// - Cache-friendly parallel accumulation
/// - Efficient memory access patterns
///
/// # Performance
///
/// Using `SoA` layout eliminates the AoS→SoA transpose that was previously
/// done in `build_dataframe_from_records`, reducing `df_build` time by ~20%.
///
/// # Cross-Platform
///
/// This struct is cross-platform and can be used on all platforms.
#[derive(Debug, Clone, Default)]
pub struct ParsedColumns {
    // Core identifiers
    /// File Record Segment numbers.
    pub frs: Vec<u64>,
    /// Parent directory FRS values.
    pub parent_frs: Vec<u64>,
    /// File/directory names.
    pub name: Vec<String>,

    // Size information
    /// Logical file sizes in bytes.
    pub size: Vec<u64>,
    /// Allocated sizes on disk.
    pub allocated_size: Vec<u64>,

    // Timestamps (Unix microseconds)
    /// Creation timestamps.
    pub created: Vec<i64>,
    /// Modification timestamps.
    pub modified: Vec<i64>,
    /// Access timestamps.
    pub accessed: Vec<i64>,
    /// MFT change timestamps.
    pub mft_changed: Vec<i64>,

    // Record metadata
    /// Whether each record is a directory.
    pub is_directory: Vec<bool>,
    /// Number of hard links (names) per record.
    pub name_count: Vec<u16>,
    /// Number of data streams per record.
    pub stream_count: Vec<u16>,
    /// Stream name (empty for default stream, non-empty for ADS).
    pub stream_name: Vec<String>,

    // Attribute flags (all boolean columns for C++ parity)
    /// Read-only flag.
    pub is_readonly: Vec<bool>,
    /// Hidden flag.
    pub is_hidden: Vec<bool>,
    /// System flag.
    pub is_system: Vec<bool>,
    /// Archive flag.
    pub is_archive: Vec<bool>,
    /// Compressed flag.
    pub is_compressed: Vec<bool>,
    /// Encrypted flag.
    pub is_encrypted: Vec<bool>,
    /// Sparse flag.
    pub is_sparse: Vec<bool>,
    /// Reparse point flag.
    pub is_reparse: Vec<bool>,
    /// Offline flag.
    pub is_offline: Vec<bool>,
    /// Not content indexed flag.
    pub is_not_indexed: Vec<bool>,
    /// Temporary flag.
    pub is_temporary: Vec<bool>,
    /// Integrity stream flag (`ReFS`).
    pub is_integrity_stream: Vec<bool>,
    /// No scrub data flag.
    pub is_no_scrub_data: Vec<bool>,
    /// Pinned flag (`OneDrive`).
    pub is_pinned: Vec<bool>,
    /// Unpinned flag (`OneDrive`).
    pub is_unpinned: Vec<bool>,
    /// Virtual flag.
    pub is_virtual: Vec<bool>,
    /// Raw attribute flags (combined value for C++ parity).
    pub flags: Vec<u32>,
}

impl ParsedColumns {
    /// Creates a new empty `ParsedColumns`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new `ParsedColumns` with pre-allocated capacity.
    ///
    /// Use this when you know the approximate number of records to avoid
    /// reallocations during accumulation.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            frs: Vec::with_capacity(capacity),
            parent_frs: Vec::with_capacity(capacity),
            name: Vec::with_capacity(capacity),
            size: Vec::with_capacity(capacity),
            allocated_size: Vec::with_capacity(capacity),
            created: Vec::with_capacity(capacity),
            modified: Vec::with_capacity(capacity),
            accessed: Vec::with_capacity(capacity),
            mft_changed: Vec::with_capacity(capacity),
            is_directory: Vec::with_capacity(capacity),
            name_count: Vec::with_capacity(capacity),
            stream_count: Vec::with_capacity(capacity),
            stream_name: Vec::with_capacity(capacity),
            is_readonly: Vec::with_capacity(capacity),
            is_hidden: Vec::with_capacity(capacity),
            is_system: Vec::with_capacity(capacity),
            is_archive: Vec::with_capacity(capacity),
            is_compressed: Vec::with_capacity(capacity),
            is_encrypted: Vec::with_capacity(capacity),
            is_sparse: Vec::with_capacity(capacity),
            is_reparse: Vec::with_capacity(capacity),
            is_offline: Vec::with_capacity(capacity),
            is_not_indexed: Vec::with_capacity(capacity),
            is_temporary: Vec::with_capacity(capacity),
            is_integrity_stream: Vec::with_capacity(capacity),
            is_no_scrub_data: Vec::with_capacity(capacity),
            is_pinned: Vec::with_capacity(capacity),
            is_unpinned: Vec::with_capacity(capacity),
            is_virtual: Vec::with_capacity(capacity),
            flags: Vec::with_capacity(capacity),
        }
    }

    /// Returns the number of records stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frs.len()
    }

    /// Returns true if no records are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frs.is_empty()
    }

    /// Pushes a single parsed record into the columns.
    ///
    /// This is the hot path for accumulation - keep it fast!
    #[inline]
    pub fn push_record(&mut self, record: &ParsedRecord) {
        self.frs.push(record.frs);
        self.parent_frs.push(record.parent_frs);
        // C++ parity: directories have empty name
        if record.is_directory {
            self.name.push(String::new());
        } else {
            self.name.push(record.name.clone());
        }
        self.size.push(record.size);
        self.allocated_size.push(record.allocated_size);
        self.created.push(record.std_info.created);
        self.modified.push(record.std_info.modified);
        self.accessed.push(record.std_info.accessed);
        self.mft_changed.push(record.std_info.mft_changed);
        self.is_directory.push(record.is_directory);
        self.name_count.push(record.name_count());
        self.stream_count.push(record.stream_count());
        self.stream_name.push(String::new()); // Default stream (no ADS)
        self.is_readonly.push(record.std_info.is_readonly);
        self.is_hidden.push(record.std_info.is_hidden);
        self.is_system.push(record.std_info.is_system);
        self.is_archive.push(record.std_info.is_archive);
        self.is_compressed.push(record.std_info.is_compressed);
        self.is_encrypted.push(record.std_info.is_encrypted);
        self.is_sparse.push(record.std_info.is_sparse);
        self.is_reparse.push(record.std_info.is_reparse);
        self.is_offline.push(record.std_info.is_offline);
        self.is_not_indexed
            .push(record.std_info.is_not_content_indexed);
        self.is_temporary.push(record.std_info.is_temporary);
        self.is_integrity_stream
            .push(record.std_info.is_integrity_stream);
        self.is_no_scrub_data.push(record.std_info.is_no_scrub_data);
        self.is_pinned.push(record.std_info.is_pinned);
        self.is_unpinned.push(record.std_info.is_unpinned);
        self.is_virtual.push(record.std_info.is_virtual);
        self.flags.push(record.std_info.to_raw_flags());
    }

    /// Pushes a record with full expansion (names × streams).
    ///
    /// This matches C++ behavior: one row per (hard link × stream) combination.
    /// If a file has 2 hard links and 3 streams, this creates 6 rows.
    ///
    /// This is the default behavior for user-facing output, as users
    /// expect to see each hard link and ADS as separate entries.
    #[inline]
    pub fn push_record_expanded(&mut self, record: &ParsedRecord) {
        // Get names to iterate over (use primary name if names is empty)
        let names: Vec<_> = if record.names.is_empty() {
            vec![NameInfo {
                name: record.name.clone(),
                parent_frs: record.parent_frs,
                namespace: 3, // Win32+DOS
                fn_created: record.fn_created,
                fn_modified: record.fn_modified,
                fn_accessed: record.fn_accessed,
                fn_mft_changed: record.fn_mft_changed,
            }]
        } else {
            record.names.clone()
        };

        // Get streams to iterate over (use empty stream if streams is empty)
        let streams: Vec<_> = if record.streams.is_empty() {
            vec![StreamInfo {
                name: String::new(),
                size: record.size,
                allocated_size: record.allocated_size,
                is_sparse: false,
                is_compressed: false,
                is_resident: false,
            }]
        } else {
            record.streams.clone()
        };

        // Create one row per (name × stream) combination
        // Filter out internal Windows streams ($OBJECT_ID, $EA_INFORMATION, etc.)
        // to match C++ behavior (ntfs_index.hpp line 1388-1392)
        for name_info in &names {
            for stream_info in &streams {
                // Skip internal Windows streams (matches C++ match_attributes=false)
                if crate::ntfs::is_internal_windows_stream(&stream_info.name) {
                    continue;
                }
                self.frs.push(record.frs);
                self.parent_frs.push(name_info.parent_frs);
                // C++ parity: directories have empty name
                if record.is_directory {
                    self.name.push(String::new());
                } else {
                    self.name.push(name_info.name.clone());
                }
                // Use stream-specific size for ADS, file size for default stream
                let (size, alloc) = if stream_info.name.is_empty() {
                    (record.size, record.allocated_size)
                } else {
                    (stream_info.size, stream_info.allocated_size)
                };
                self.size.push(size);
                self.allocated_size.push(alloc);
                self.created.push(record.std_info.created);
                self.modified.push(record.std_info.modified);
                self.accessed.push(record.std_info.accessed);
                self.mft_changed.push(record.std_info.mft_changed);
                self.is_directory.push(record.is_directory);
                // For expanded records, counts are 1 (this row = one link + one stream)
                self.name_count.push(1);
                self.stream_count.push(1);
                self.stream_name.push(stream_info.name.clone());
                self.is_readonly.push(record.std_info.is_readonly);
                self.is_hidden.push(record.std_info.is_hidden);
                self.is_system.push(record.std_info.is_system);
                self.is_archive.push(record.std_info.is_archive);
                self.is_compressed.push(record.std_info.is_compressed);
                self.is_encrypted.push(record.std_info.is_encrypted);
                self.is_sparse.push(record.std_info.is_sparse);
                self.is_reparse.push(record.std_info.is_reparse);
                self.is_offline.push(record.std_info.is_offline);
                self.is_not_indexed
                    .push(record.std_info.is_not_content_indexed);
                self.is_temporary.push(record.std_info.is_temporary);
                self.is_integrity_stream
                    .push(record.std_info.is_integrity_stream);
                self.is_no_scrub_data.push(record.std_info.is_no_scrub_data);
                self.is_pinned.push(record.std_info.is_pinned);
                self.is_unpinned.push(record.std_info.is_unpinned);
                self.is_virtual.push(record.std_info.is_virtual);
                self.flags.push(record.std_info.to_raw_flags());
            }
        }
    }

    /// Extends this `ParsedColumns` with all records from another.
    ///
    /// Used in Rayon reduce phase to merge per-thread results.
    pub fn extend(&mut self, other: Self) {
        self.frs.extend(other.frs);
        self.parent_frs.extend(other.parent_frs);
        self.name.extend(other.name);
        self.size.extend(other.size);
        self.allocated_size.extend(other.allocated_size);
        self.created.extend(other.created);
        self.modified.extend(other.modified);
        self.accessed.extend(other.accessed);
        self.mft_changed.extend(other.mft_changed);
        self.is_directory.extend(other.is_directory);
        self.name_count.extend(other.name_count);
        self.stream_count.extend(other.stream_count);
        self.stream_name.extend(other.stream_name);
        self.is_readonly.extend(other.is_readonly);
        self.is_hidden.extend(other.is_hidden);
        self.is_system.extend(other.is_system);
        self.is_archive.extend(other.is_archive);
        self.is_compressed.extend(other.is_compressed);
        self.is_encrypted.extend(other.is_encrypted);
        self.is_sparse.extend(other.is_sparse);
        self.is_reparse.extend(other.is_reparse);
        self.is_offline.extend(other.is_offline);
        self.is_not_indexed.extend(other.is_not_indexed);
        self.is_temporary.extend(other.is_temporary);
        self.is_integrity_stream.extend(other.is_integrity_stream);
        self.is_no_scrub_data.extend(other.is_no_scrub_data);
        self.is_pinned.extend(other.is_pinned);
        self.is_unpinned.extend(other.is_unpinned);
        self.is_virtual.extend(other.is_virtual);
        self.flags.extend(other.flags);
    }

    /// Reserves capacity for additional records.
    pub fn reserve(&mut self, additional: usize) {
        self.frs.reserve(additional);
        self.parent_frs.reserve(additional);
        self.name.reserve(additional);
        self.size.reserve(additional);
        self.allocated_size.reserve(additional);
        self.created.reserve(additional);
        self.modified.reserve(additional);
        self.accessed.reserve(additional);
        self.mft_changed.reserve(additional);
        self.is_directory.reserve(additional);
        self.name_count.reserve(additional);
        self.stream_count.reserve(additional);
        self.stream_name.reserve(additional);
        self.is_readonly.reserve(additional);
        self.is_hidden.reserve(additional);
        self.is_system.reserve(additional);
        self.is_archive.reserve(additional);
        self.is_compressed.reserve(additional);
        self.is_encrypted.reserve(additional);
        self.is_sparse.reserve(additional);
        self.is_reparse.reserve(additional);
        self.is_offline.reserve(additional);
        self.is_not_indexed.reserve(additional);
        self.is_temporary.reserve(additional);
        self.is_integrity_stream.reserve(additional);
        self.is_no_scrub_data.reserve(additional);
        self.is_pinned.reserve(additional);
        self.is_unpinned.reserve(additional);
        self.is_virtual.reserve(additional);
        self.flags.reserve(additional);
    }

    /// Creates `ParsedColumns` from a vector of `ParsedRecord`.
    ///
    /// # Arguments
    ///
    /// * `records` - The parsed records to convert
    /// * `expand_links` - If `true`, expand hard links to separate rows
    ///   (matching C++ behavior). If `false`, one row per FRS.
    #[must_use]
    pub fn from_records(records: Vec<ParsedRecord>, expand_links: bool) -> Self {
        // Estimate capacity using integer arithmetic to avoid float precision issues
        let estimated_capacity = if expand_links {
            // Rough estimate: assume average of 1.2 links per file (len * 6 / 5)
            records.len().saturating_mul(6) / 5
        } else {
            records.len()
        };

        let mut columns = Self::with_capacity(estimated_capacity);
        for record in records {
            if expand_links {
                columns.push_record_expanded(&record);
            } else {
                columns.push_record(&record);
            }
        }
        columns
    }

    /// Maximum iterations for placeholder creation to prevent infinite loops.
    const MAX_PLACEHOLDER_ITERATIONS: usize = 10;

    /// Adds placeholder records for parent directories that are referenced
    /// but not present in the parsed records.
    ///
    /// This matches C++ behavior where `at()` creates placeholder records
    /// for any referenced FRS that hasn't been seen yet. Without this,
    /// path resolution fails with `<unknown:XXXXXX>` for files whose parent
    /// directories weren't parsed (e.g., marked as not-in-use in bitmap).
    ///
    /// # Performance Optimization (2026-01-23)
    ///
    /// Uses `FxHashSet` instead of `std::collections::HashSet` for faster
    /// hashing. `FxHash` is 5-10x faster than `SipHash` for integer keys.
    ///
    /// # Returns
    ///
    /// The number of placeholder records added.
    pub fn add_missing_parent_placeholders(&mut self) -> usize {
        use rustc_hash::FxHashSet;

        // Iterate until no new placeholders are needed (handles recursive missing
        // parents)
        let mut total_added = 0;
        let mut iterations = 0;

        loop {
            iterations += 1;
            if iterations > Self::MAX_PLACEHOLDER_ITERATIONS {
                warn!(
                    iterations,
                    "Max iterations reached in placeholder creation - possible cycle"
                );
                break;
            }

            // Collect all FRS values we have (FxHashSet for faster hashing)
            let known_frs: FxHashSet<u64> = self.frs.iter().copied().collect();

            // Collect all parent_frs values that are referenced
            let referenced_parents: FxHashSet<u64> = self.parent_frs.iter().copied().collect();

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
                "Creating placeholder records for missing parent directories"
            );

            // Create placeholder records
            for frs in missing_parents {
                let placeholder = create_placeholder_record(frs);
                self.push_record(&placeholder);
                total_added += 1;
            }
        }

        if total_added > 0 {
            info!(
                total_added,
                iterations, "Added placeholder records for missing parent directories"
            );
        }

        total_added
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a minimal valid MFT record header for testing.
    /// This creates a 1024-byte record with proper fixup values.
    fn create_test_record(frs: u64, in_use: bool, is_dir: bool) -> Vec<u8> {
        let mut data = vec![0_u8; 1024];

        // Magic: "FILE"
        data[0..4].copy_from_slice(&FILE_RECORD_MAGIC.to_le_bytes());

        // USA offset (0x30 is typical)
        data[4..6].copy_from_slice(&0x30_u16.to_le_bytes());

        // USA count (3 for 1024-byte record: check + 2 sectors)
        data[6..8].copy_from_slice(&3_u16.to_le_bytes());

        // LSN (Log Sequence Number)
        data[8..16].copy_from_slice(&12345_u64.to_le_bytes());

        // Sequence number
        data[16..18].copy_from_slice(&1_u16.to_le_bytes());

        // Hard link count
        data[18..20].copy_from_slice(&1_u16.to_le_bytes());

        // First attribute offset (after header, 0x38 typical)
        data[20..22].copy_from_slice(&0x38_u16.to_le_bytes());

        // Flags: 0x01 = in use, 0x02 = directory
        let flags: u16 = u16::from(in_use) | (u16::from(is_dir) << 1);
        data[22..24].copy_from_slice(&flags.to_le_bytes());

        // Used size of record
        data[24..28].copy_from_slice(&0x100_u32.to_le_bytes());

        // Allocated size of record
        data[28..32].copy_from_slice(&0x400_u32.to_le_bytes());

        // Base record reference (0 for base records)
        data[32..40].copy_from_slice(&0_u64.to_le_bytes());

        // Next attribute ID
        data[40..42].copy_from_slice(&1_u16.to_le_bytes());

        // FRS number (at offset 44 in modern NTFS) - truncate to u32 for test data
        #[allow(clippy::cast_possible_truncation)]
        let frs_u32 = frs as u32;
        data[44..48].copy_from_slice(&frs_u32.to_le_bytes());

        // USA: check value at offset 0x30
        let check_value: u16 = 0xABCD;
        data[0x30..0x32].copy_from_slice(&check_value.to_le_bytes());

        // USA entry 1 (original bytes from sector 1 end)
        data[0x32..0x34].copy_from_slice(&0x1234_u16.to_le_bytes());

        // USA entry 2 (original bytes from sector 2 end)
        data[0x34..0x36].copy_from_slice(&0x5678_u16.to_le_bytes());

        // Place check value at sector boundaries (will be replaced by fixup)
        data[510..512].copy_from_slice(&check_value.to_le_bytes());
        data[1022..1024].copy_from_slice(&check_value.to_le_bytes());

        // End marker attribute (type 0xFFFFFFFF)
        data[0x38..0x3C].copy_from_slice(&0xFFFF_FFFF_u32.to_le_bytes());

        data
    }

    #[test]
    fn test_apply_fixup_valid_record() {
        let mut data = create_test_record(5, true, false);

        // Before fixup, sector ends have check value
        assert_eq!(&data[510..512], &0xABCD_u16.to_le_bytes());
        assert_eq!(&data[1022..1024], &0xABCD_u16.to_le_bytes());

        let result = apply_fixup(&mut data);
        assert!(result, "Fixup should succeed for valid record");

        // After fixup, sector ends have original values from USA
        assert_eq!(&data[510..512], &0x1234_u16.to_le_bytes());
        assert_eq!(&data[1022..1024], &0x5678_u16.to_le_bytes());
    }

    #[test]
    fn test_apply_fixup_invalid_magic() {
        let mut data = vec![0_u8; 1024];
        data[0..4].copy_from_slice(b"BAAD"); // Invalid magic

        let result = apply_fixup(&mut data);
        assert!(!result, "Fixup should fail for invalid magic");
    }

    #[test]
    fn test_apply_fixup_buffer_too_small() {
        let mut data = vec![0_u8; 10]; // Too small for header

        let result = apply_fixup(&mut data);
        assert!(!result, "Fixup should fail for buffer too small");
    }

    #[test]
    fn test_apply_fixup_corrupted_check_value() {
        let mut data = create_test_record(5, true, false);

        // Corrupt the check value at first sector end
        data[510..512].copy_from_slice(&0xDEAD_u16.to_le_bytes());

        let result = apply_fixup(&mut data);
        assert!(!result, "Fixup should fail for corrupted check value");
    }

    #[test]
    fn test_create_placeholder_record() {
        let record = create_placeholder_record(12345);

        assert_eq!(record.frs, 12345);
        assert_eq!(record.parent_frs, 5); // Root directory
        assert_eq!(record.name, "<dir:12345>");
        assert!(record.is_directory);
        assert!(record.in_use);
        assert!(record.names.is_empty());
        assert!(record.streams.is_empty());
    }

    #[test]
    fn test_parse_result_variants() {
        // Test ParseResult enum
        let base = ParseResult::Base(create_placeholder_record(1));
        assert!(matches!(base, ParseResult::Base(_)));

        let ext = ParseResult::Extension(ExtensionAttributes {
            base_frs: 100,
            extension_frs: 101,
            names: Vec::new(),
            streams: Vec::new(),
        });
        assert!(matches!(ext, ParseResult::Extension(_)));

        let skip = ParseResult::Skip;
        assert!(matches!(skip, ParseResult::Skip));
    }

    #[test]
    fn test_parse_options_default() {
        let opts = ParseOptions::default();
        assert!(!opts.include_deleted);
        assert!(!opts.include_corrupt);
        assert!(!opts.include_extensions);
    }

    #[test]
    fn test_parse_options_forensic() {
        let opts = ParseOptions::FORENSIC;
        assert!(opts.include_deleted);
        assert!(opts.include_corrupt);
        assert!(opts.include_extensions);
        assert!(opts.is_forensic());
    }

    #[test]
    fn test_parsed_record_default() {
        let record = ParsedRecord::default();
        assert_eq!(record.frs, 0);
        assert_eq!(record.sequence_number, 0);
        assert_eq!(record.parent_frs, 0);
        assert!(record.name.is_empty());
        assert!(!record.in_use);
        assert!(!record.is_directory);
    }

    #[test]
    fn test_add_missing_parent_placeholders_empty() {
        let mut records: Vec<ParsedRecord> = Vec::new();
        let added = add_missing_parent_placeholders_to_vec(&mut records);
        assert_eq!(added, 0);
    }

    #[test]
    fn test_add_missing_parent_placeholders_no_missing() {
        let mut records = vec![
            {
                let mut r = create_placeholder_record(5);
                r.parent_frs = 5; // Root references itself
                r
            },
            {
                let mut r = create_placeholder_record(100);
                r.parent_frs = 5; // References root
                r
            },
        ];

        let added = add_missing_parent_placeholders_to_vec(&mut records);
        assert_eq!(added, 0, "No placeholders needed when all parents exist");
    }

    #[test]
    fn test_add_missing_parent_placeholders_with_missing() {
        let mut records = vec![{
            let mut r = create_placeholder_record(100);
            r.parent_frs = 50; // References non-existent parent
            r
        }];

        let added = add_missing_parent_placeholders_to_vec(&mut records);
        assert!(added >= 1, "Should add placeholder for missing parent 50");

        // Verify placeholder was added
        let has_50 = records.iter().any(|r| r.frs == 50);
        assert!(has_50, "Placeholder for FRS 50 should exist");
    }

    // ========================================================================
    // Property-Based Tests
    // ========================================================================

    mod proptest_tests {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            /// apply_fixup should never panic regardless of input
            #[test]
            fn apply_fixup_never_panics(mut data in prop::collection::vec(any::<u8>(), 0..2048)) {
                // Should return true or false, never panic
                // For random data, fixup usually fails (returns false) because
                // the data doesn't have valid MFT record structure
                let result = apply_fixup(&mut data);
                // Use black_box to prevent optimization and ensure result is used
                core::hint::black_box(result);
            }

            /// create_placeholder_record should always produce valid records
            #[test]
            fn placeholder_record_always_valid(frs in 0_u64..1_000_000) {
                let record = create_placeholder_record(frs);
                prop_assert_eq!(record.frs, frs);
                prop_assert!(record.is_directory);
                prop_assert!(record.in_use);
                prop_assert_eq!(record.parent_frs, 5); // Always root
            }

            /// ParseOptions should have consistent is_forensic behavior
            #[test]
            fn parse_options_forensic_consistency(
                include_deleted in any::<bool>(),
                include_corrupt in any::<bool>(),
                include_extensions in any::<bool>()
            ) {
                let opts = ParseOptions {
                    include_deleted,
                    include_corrupt,
                    include_extensions,
                };
                let expected = include_deleted || include_corrupt || include_extensions;
                prop_assert_eq!(opts.is_forensic(), expected);
            }

            /// parse_record should handle any buffer without panicking
            #[test]
            fn parse_record_never_panics(
                data in prop::collection::vec(any::<u8>(), 0..4096),
                frs in 0_u64..1_000_000
            ) {
                // Should return Some or None, never panic
                let result = parse_record(&data, frs);
                // Result is valid (Some or None)
                prop_assert!(result.is_some() || result.is_none());
            }

            /// parse_record_full should handle any buffer without panicking
            #[test]
            fn parse_record_full_never_panics(
                data in prop::collection::vec(any::<u8>(), 0..4096),
                frs in 0_u64..1_000_000
            ) {
                // Should return a ParseResult variant, never panic
                let result = parse_record_full(&data, frs);
                // Result is valid (one of the variants: Base, Extension, or Skip)
                prop_assert!(matches!(result, ParseResult::Base(_) | ParseResult::Extension(_) | ParseResult::Skip));
            }
        }
    }

    /// Test that extension records with `$FILE_NAME` are properly merged into
    /// base records that have no `$FILE_NAME` attribute.
    #[test]
    fn test_extension_merge_with_empty_base_name() {
        // Simulate the case where base record has no $FILE_NAME
        // and extension record has the $FILE_NAME

        let mut record_merger = MftRecordMerger::with_capacity(10);

        // Add base record with empty name
        let base = ParsedRecord {
            frs: 100,
            sequence_number: 1,
            lsn: 0,
            parent_frs: 0,       // Wrong - should be updated from extension
            name: String::new(), // Empty - should be updated from extension
            namespace: 255,      // Invalid
            names: Vec::new(),   // No names in base record
            streams: Vec::new(),
            size: 0,
            allocated_size: 0,
            std_info: ExtendedStandardInfo::default(),
            in_use: true,
            is_directory: true,
            fn_created: 0,
            fn_modified: 0,
            fn_accessed: 0,
            fn_mft_changed: 0,
            reparse_tag: 0,
            is_deleted: false,
            is_corrupt: false,
            is_extension: false,
            base_frs: 0,
        };
        record_merger.add_result(ParseResult::Base(base));

        // Add extension record with the actual name
        let ext = ExtensionAttributes {
            base_frs: 100,
            extension_frs: 200,
            names: vec![NameInfo {
                name: "test_directory".to_owned(),
                parent_frs: 5, // Root
                namespace: 1,  // Win32
                fn_created: 0,
                fn_modified: 0,
                fn_accessed: 0,
                fn_mft_changed: 0,
            }],
            streams: Vec::new(),
        };
        record_merger.add_result(ParseResult::Extension(ext));

        // Merge
        let result = record_merger.merge();

        // Check that the base record now has the name from extension
        assert_eq!(result.len(), 1, "Should have exactly 1 merged record");
        let rec = &result[0];
        assert_eq!(rec.frs, 100);
        assert_eq!(
            rec.name, "test_directory",
            "Name should be merged from extension"
        );
        assert_eq!(
            rec.parent_frs, 5,
            "parent_frs should be merged from extension"
        );
        assert_eq!(
            rec.namespace, 1,
            "namespace should be merged from extension"
        );
    }

    /// Test that extension records are merged even when processed before base
    /// record
    #[test]
    fn test_extension_before_base_merge() {
        let mut record_merger = MftRecordMerger::with_capacity(10);

        // Add extension record FIRST (before base record)
        let ext = ExtensionAttributes {
            base_frs: 100,
            extension_frs: 200,
            names: vec![NameInfo {
                name: "test_directory".to_owned(),
                parent_frs: 5,
                namespace: 1,
                fn_created: 0,
                fn_modified: 0,
                fn_accessed: 0,
                fn_mft_changed: 0,
            }],
            streams: Vec::new(),
        };
        record_merger.add_result(ParseResult::Extension(ext));

        // Add base record AFTER extension
        let base = ParsedRecord {
            frs: 100,
            sequence_number: 1,
            lsn: 0,
            parent_frs: 0,
            name: String::new(),
            namespace: 255,
            names: Vec::new(),
            streams: Vec::new(),
            size: 0,
            allocated_size: 0,
            std_info: ExtendedStandardInfo::default(),
            in_use: true,
            is_directory: true,
            fn_created: 0,
            fn_modified: 0,
            fn_accessed: 0,
            fn_mft_changed: 0,
            reparse_tag: 0,
            is_deleted: false,
            is_corrupt: false,
            is_extension: false,
            base_frs: 0,
        };
        record_merger.add_result(ParseResult::Base(base));

        // Merge
        let result = record_merger.merge();

        // Check that the base record now has the name from extension
        assert_eq!(result.len(), 1, "Should have exactly 1 merged record");
        let rec = &result[0];
        assert_eq!(rec.frs, 100);
        assert_eq!(
            rec.name, "test_directory",
            "Name should be merged from extension"
        );
        assert_eq!(
            rec.parent_frs, 5,
            "parent_frs should be merged from extension"
        );
    }
}
