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
    ExtendedStandardInfo, FILE_RECORD_MAGIC, MultiSectorHeader, NameInfo, SECTOR_SIZE, StreamInfo,
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
#[allow(unsafe_code, clippy::single_call_fn)] // Required: ptr::read for packed NTFS struct
fn parse_standard_info_full(data: &[u8], attr_offset: usize, result: &mut ExtendedStandardInfo) {
    use core::mem::size_of;

    use crate::ntfs::{StandardInformation, filetime_to_unix_micros};

    // Get value offset (resident attribute)
    let value_offset_bytes = &data[attr_offset + 20..attr_offset + 22];
    let value_offset = u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;

    let si_offset = attr_offset + value_offset;
    if si_offset + size_of::<StandardInformation>() > data.len() {
        return;
    }

    // SAFETY: We've verified the buffer is large enough.
    let si: StandardInformation = unsafe { core::ptr::read(data[si_offset..].as_ptr().cast()) };

    result.created = filetime_to_unix_micros(si.creation_time);
    result.modified = filetime_to_unix_micros(si.modification_time);
    result.accessed = filetime_to_unix_micros(si.access_time);
    result.mft_changed = filetime_to_unix_micros(si.mft_change_time);

    // Parse all flags
    *result = ExtendedStandardInfo {
        created: result.created,
        modified: result.modified,
        accessed: result.accessed,
        mft_changed: result.mft_changed,
        ..ExtendedStandardInfo::from_attributes(si.file_attributes)
    };
}

/// Parses `$FILE_NAME` and returns a `NameInfo`.
#[allow(unsafe_code, clippy::single_call_fn)] // Required: ptr::read for packed NTFS struct
fn parse_file_name_full(data: &[u8], attr_offset: usize) -> Option<NameInfo> {
    use core::mem::size_of;

    use smallvec::SmallVec;

    use crate::ntfs::{FileNameAttribute, file_reference_to_frs};

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
    })
}

/// Parses `$DATA` attribute and returns a `StreamInfo`.
#[allow(clippy::single_call_fn)]
fn parse_data_attribute_full(
    data: &[u8],
    attr_offset: usize,
    header: &crate::ntfs::AttributeRecordHeader,
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

    let (size, allocated_size, is_sparse, is_compressed) = if header.is_non_resident != 0 {
        // Non-resident: get sizes from non-resident header
        let nr_offset = attr_offset + 16; // After common header
        if nr_offset + 48 > data.len() {
            return None;
        }

        let allocated_size =
            i64::from_le_bytes(data[nr_offset + 24..nr_offset + 32].try_into().ok()?);
        let data_size = i64::from_le_bytes(data[nr_offset + 40..nr_offset + 48].try_into().ok()?);

        // Check compression unit (at offset 16 in non-resident header)
        let compression_unit = data[nr_offset + 8];
        let is_compressed = compression_unit > 0;

        // Check sparse flag in attribute flags
        let is_sparse = (header.flags & 0x8000) != 0;

        (
            data_size.max(0) as u64,
            allocated_size.max(0) as u64,
            is_sparse,
            is_compressed,
        )
    } else {
        // Resident: get size from resident header
        let value_length_bytes = &data[attr_offset + 16..attr_offset + 20];
        let value_length = u32::from_le_bytes(value_length_bytes.try_into().ok()?);
        (value_length as u64, 0, false, false)
    };

    Some(StreamInfo {
        name: stream_name,
        size,
        allocated_size,
        is_sparse,
        is_compressed,
    })
}

// ============================================================================
// Main Parsing Functions
// ============================================================================

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
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS structs
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

    // Prepare result containers
    let mut names: Vec<NameInfo> = Vec::new();
    let mut streams: Vec<StreamInfo> = Vec::new();
    let mut std_info = ExtendedStandardInfo::default();
    let mut primary_name = String::new();
    let mut primary_parent_frs = 0_u64;
    let mut primary_namespace = 255_u8; // Invalid, will be replaced

    // Parse attributes
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        // SAFETY: We've verified offset + size_of::<AttributeRecordHeader>() <=
        // max_offset <= data.len()
        let attr_header: AttributeRecordHeader =
            unsafe { core::ptr::read(data[offset..].as_ptr().cast()) };

        // Check for end marker
        if attr_header.type_code == AttributeType::End as u32 {
            break;
        }

        // Validate attribute length
        if attr_header.length == 0 || offset + attr_header.length as usize > max_offset {
            break;
        }

        // Parse based on attribute type
        match AttributeType::from_u32(attr_header.type_code) {
            Some(AttributeType::StandardInformation) => {
                if attr_header.is_non_resident == 0 {
                    parse_standard_info_full(data, offset, &mut std_info);
                }
            }
            Some(AttributeType::FileName) => {
                if attr_header.is_non_resident == 0 {
                    if let Some(name_info) = parse_file_name_full(data, offset) {
                        // Skip DOS-only names (namespace 2)
                        if name_info.namespace != 2 {
                            // Check if this is a better primary name
                            let is_better = match name_info.namespace {
                                1 | 3 => true,                 // Win32 or Win32+DOS
                                0 => primary_namespace == 255, // POSIX only if no name yet
                                _ => false,
                            };
                            if is_better || primary_namespace == 255 {
                                primary_name = name_info.name.clone();
                                primary_parent_frs = name_info.parent_frs;
                                primary_namespace = name_info.namespace;
                            }
                            names.push(name_info);
                        }
                    }
                }
            }
            Some(AttributeType::Data) => {
                if let Some(stream_info) = parse_data_attribute_full(data, offset, &attr_header) {
                    streams.push(stream_info);
                }
            }
            _ => {}
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

    // Skip records without a $FILE_NAME attribute (matching C++ behavior).
    // C++ uses nameinfo() which returns NULL for records without filenames,
    // causing the traversal loop to skip them entirely.
    // These are typically extension records, deleted files, or corrupted records.
    if primary_name.is_empty() {
        return ParseResult::Skip;
    }

    // Calculate primary size from default stream
    let (size, allocated_size) = streams
        .iter()
        .find(|s| s.name.is_empty())
        .map(|s| (s.size, s.allocated_size))
        .unwrap_or((0, 0));

    ParseResult::Base(ParsedRecord {
        frs,
        parent_frs: primary_parent_frs,
        name: primary_name,
        names,
        streams,
        size,
        allocated_size,
        std_info,
        in_use: true,
        is_directory: header.is_directory(),
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

        // Recalculate sizes from merged streams and collect results
        let mut result = Vec::with_capacity(self.base_count);
        for record in self.base_records.iter_mut().flatten() {
            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
            }
        }

        // Collect non-None records
        for record in self.base_records.into_iter().flatten() {
            result.push(record);
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

        // Recalculate sizes from merged streams
        for record in self.base_records.iter_mut().flatten() {
            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
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
        let mut columns = ParsedColumns::with_capacity(estimated_capacity);
        for record in self.base_records.into_iter().flatten() {
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
            }]
        } else {
            record.streams.clone()
        };

        // Create one row per (name × stream) combination
        for name_info in &names {
            for stream_info in &streams {
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
