//! Low-level I/O operations for MFT reading.
//!
//! This module provides efficient disk I/O for reading MFT records:
//! - Aligned buffer management for direct I/O
//! - Sector-aligned reads
//! - Multi-sector fixup (Update Sequence Array)
//! - **Fragmented MFT support** via extent mapping
//!
//! # Performance
//!
//! Uses `FILE_FLAG_NO_BUFFERING` for direct I/O, which requires:
//! - Sector-aligned file offsets
//! - Sector-aligned buffer addresses
//! - Sector-aligned read sizes
//!
//! # Fragmented MFT
//!
//! The MFT can be scattered across multiple non-contiguous extents on disk.
//! This module handles fragmentation by:
//! 1. Getting the extent map via `FSCTL_GET_RETRIEVAL_POINTERS`
//! 2. Mapping Virtual Cluster Numbers (VCN) to Logical Cluster Numbers (LCN)
//! 3. Reading from the correct physical locations

#![cfg(windows)]

use std::mem::size_of;

use tracing::{debug, info, trace, warn};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

use crate::error::{MftError, Result};
use crate::ntfs::{
    AttributeRecordHeader, AttributeType, FILE_RECORD_MAGIC, FileNameAttribute,
    FileRecordSegmentHeader, MultiSectorHeader, SECTOR_SIZE, StandardInformation,
};
use crate::platform::{MftExtent, VolumeHandle};

// ============================================================================
// Aligned Buffer
// ============================================================================

/// A buffer aligned to sector boundaries for direct I/O.
///
/// Windows `FILE_FLAG_NO_BUFFERING` requires sector-aligned buffers.
#[derive(Debug)]
pub struct AlignedBuffer {
    /// The underlying buffer (over-allocated for alignment).
    data: Vec<u8>,
    /// Offset to the aligned portion.
    offset: usize,
    /// Usable size of the aligned buffer.
    size: usize,
}

impl AlignedBuffer {
    /// Creates a new aligned buffer with the specified size.
    ///
    /// The buffer will be aligned to `SECTOR_SIZE` (512 bytes).
    #[must_use]
    pub fn new(size: usize) -> Self {
        // Allocate extra space for alignment
        let alloc_size = size + SECTOR_SIZE;
        let data = vec![0_u8; alloc_size];

        // Calculate alignment offset
        let ptr = data.as_ptr() as usize;
        let offset = (SECTOR_SIZE - (ptr % SECTOR_SIZE)) % SECTOR_SIZE;

        Self { data, offset, size }
    }

    /// Returns a mutable slice to the aligned buffer.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[self.offset..self.offset + self.size]
    }

    /// Returns an immutable slice to the aligned buffer.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.data[self.offset..self.offset + self.size]
    }

    /// Returns the size of the usable buffer.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.size
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }
}

// ============================================================================
// MFT Extent Map
// ============================================================================

/// Maps Virtual Cluster Numbers (VCN) to Logical Cluster Numbers (LCN).
///
/// The MFT can be fragmented across multiple non-contiguous extents on disk.
/// This struct provides efficient lookup to find the physical location of any
/// MFT record.
#[derive(Debug, Clone)]
pub struct MftExtentMap {
    /// Sorted list of extents (by VCN).
    extents: Vec<MftExtent>,
    /// Bytes per cluster.
    pub bytes_per_cluster: u32,
    /// Bytes per file record.
    pub bytes_per_record: u32,
}

impl MftExtentMap {
    /// Creates a new extent map from a list of extents.
    ///
    /// # Arguments
    ///
    /// * `extents` - List of MFT extents from `FSCTL_GET_RETRIEVAL_POINTERS`
    /// * `bytes_per_cluster` - Cluster size in bytes
    /// * `bytes_per_record` - File record size in bytes
    #[must_use]
    pub fn new(extents: Vec<MftExtent>, bytes_per_cluster: u32, bytes_per_record: u32) -> Self {
        Self {
            extents,
            bytes_per_cluster,
            bytes_per_record,
        }
    }

    /// Creates a simple extent map for a contiguous MFT.
    ///
    /// This is a fallback when extent information is not available.
    #[must_use]
    pub fn contiguous(
        mft_start_lcn: u64,
        mft_size_bytes: u64,
        bytes_per_cluster: u32,
        bytes_per_record: u32,
    ) -> Self {
        let cluster_count =
            (mft_size_bytes + u64::from(bytes_per_cluster) - 1) / u64::from(bytes_per_cluster);

        Self {
            extents: vec![MftExtent {
                vcn: 0,
                cluster_count,
                lcn: mft_start_lcn as i64,
            }],
            bytes_per_cluster,
            bytes_per_record,
        }
    }

    /// Returns the physical byte offset for a given File Record Segment number.
    ///
    /// # Arguments
    ///
    /// * `frs` - The File Record Segment number
    ///
    /// # Returns
    ///
    /// `Some(offset)` if the FRS is within the mapped extents,
    /// `None` if the FRS is outside the MFT or in a sparse region.
    #[must_use]
    pub fn physical_offset(&self, frs: u64) -> Option<u64> {
        // Calculate the byte offset within the MFT (virtual offset)
        let virtual_byte_offset = frs * u64::from(self.bytes_per_record);

        // Calculate the VCN containing this record
        let vcn = virtual_byte_offset / u64::from(self.bytes_per_cluster);

        // Find the extent containing this VCN
        let extent = self.find_extent(vcn)?;

        // Check for sparse extent
        if extent.lcn < 0 {
            return None;
        }

        // Calculate offset within the extent
        let vcn_offset = vcn - extent.vcn;
        let cluster_byte_offset = vcn_offset * u64::from(self.bytes_per_cluster);

        // Calculate offset within the cluster
        let offset_in_cluster = virtual_byte_offset % u64::from(self.bytes_per_cluster);

        // Physical offset = LCN * bytes_per_cluster + offset within extent + offset in
        // cluster
        let physical = (extent.lcn as u64) * u64::from(self.bytes_per_cluster)
            + cluster_byte_offset
            + offset_in_cluster;

        Some(physical)
    }

    /// Finds the extent containing a given VCN.
    fn find_extent(&self, vcn: u64) -> Option<&MftExtent> {
        // Binary search for the extent
        let idx = self
            .extents
            .binary_search_by(|extent| {
                if vcn < extent.vcn {
                    std::cmp::Ordering::Greater
                } else if vcn >= extent.vcn + extent.cluster_count {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;

        Some(&self.extents[idx])
    }

    /// Returns the number of extents in the map.
    #[must_use]
    pub fn extent_count(&self) -> usize {
        self.extents.len()
    }

    /// Returns true if the MFT is fragmented (more than one extent).
    #[must_use]
    pub fn is_fragmented(&self) -> bool {
        self.extents.len() > 1
    }

    /// Returns an iterator over the extents.
    pub fn extents(&self) -> impl Iterator<Item = &MftExtent> {
        self.extents.iter()
    }

    /// Returns the total size of the MFT in bytes.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.extents
            .iter()
            .map(|e| e.cluster_count * u64::from(self.bytes_per_cluster))
            .sum()
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.total_size() / u64::from(self.bytes_per_record)
    }
}

// ============================================================================
// MFT Record Reader
// ============================================================================

/// Reads MFT records from a volume, handling fragmented MFTs.
#[derive(Debug)]
pub struct MftRecordReader {
    /// Size of each file record in bytes.
    record_size: u32,
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Aligned buffer for reading records.
    buffer: AlignedBuffer,
}

impl MftRecordReader {
    /// Creates a new MFT record reader.
    ///
    /// # Arguments
    ///
    /// * `volume` - The volume handle to read from
    ///
    /// # Note
    ///
    /// This constructor creates a simple contiguous extent map.
    /// For fragmented MFT support, use `new_with_extents()`.
    #[must_use]
    pub fn new(volume: &VolumeHandle) -> Self {
        let record_size = volume.file_record_size();
        let volume_data = volume.volume_data();

        // Create a simple contiguous extent map
        let extent_map = MftExtentMap::contiguous(
            volume_data.mft_start_lcn,
            volume_data.mft_valid_data_length,
            volume_data.bytes_per_cluster,
            record_size,
        );

        // Allocate buffer for one record (rounded up to sector boundary)
        let buffer_size = ((record_size as usize + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;
        let buffer = AlignedBuffer::new(buffer_size);

        Self {
            record_size,
            extent_map,
            buffer,
        }
    }

    /// Creates a new MFT record reader with explicit extent mapping.
    ///
    /// This constructor should be used when the MFT is fragmented.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The extent map for the MFT
    #[must_use]
    pub fn new_with_extents(extent_map: MftExtentMap) -> Self {
        let record_size = extent_map.bytes_per_record;

        // Allocate buffer for one record (rounded up to sector boundary)
        let buffer_size = ((record_size as usize + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;
        let buffer = AlignedBuffer::new(buffer_size);

        Self {
            record_size,
            extent_map,
            buffer,
        }
    }

    /// Returns the extent map.
    #[must_use]
    pub fn extent_map(&self) -> &MftExtentMap {
        &self.extent_map
    }

    /// Returns true if the MFT is fragmented.
    #[must_use]
    pub fn is_fragmented(&self) -> bool {
        self.extent_map.is_fragmented()
    }

    /// Reads a single MFT record by its File Record Segment number.
    ///
    /// This method handles fragmented MFTs by using the extent map to
    /// translate the FRS to a physical disk location.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `frs` - The File Record Segment number to read
    ///
    /// # Errors
    ///
    /// Returns an error if the record cannot be read or is invalid.
    #[allow(unsafe_code)] // Required: Windows FFI (SetFilePointerEx, ReadFile)
    pub fn read_record(&mut self, handle: HANDLE, frs: u64) -> Result<&[u8]> {
        // Use extent map to get the physical offset (handles fragmentation)
        let record_offset =
            self.extent_map
                .physical_offset(frs)
                .ok_or_else(|| MftError::RecordRead {
                    frs,
                    reason: "FRS outside MFT extents or in sparse region".to_owned(),
                })?;

        // Align to sector boundary
        let aligned_offset = (record_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_within_sector = (record_offset - aligned_offset) as usize;

        // Seek to the aligned offset
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        // Read the record
        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(self.buffer.as_mut_slice()),
                Some(&mut bytes_read),
                None,
            )?;
        }

        if (bytes_read as usize) < self.record_size as usize + offset_within_sector {
            return Err(MftError::RecordRead {
                frs,
                reason: format!(
                    "Short read: expected {} bytes, got {}",
                    self.record_size, bytes_read
                ),
            });
        }

        // Return the record data (accounting for sector alignment offset)
        Ok(&self.buffer.as_slice()
            [offset_within_sector..offset_within_sector + self.record_size as usize])
    }

    /// Returns the record size in bytes.
    #[must_use]
    pub const fn record_size(&self) -> u32 {
        self.record_size
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.extent_map.total_records()
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
// Record Parsing
// ============================================================================

use crate::ntfs::{ExtendedStandardInfo, NameInfo, StreamInfo};

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
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS structs
pub fn parse_record_full(data: &[u8], frs: u64) -> ParseResult {
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
        crate::ntfs::file_reference_to_frs(header.base_file_record_segment)
    } else {
        frs
    };

    // Prepare result containers
    let mut names: Vec<NameInfo> = Vec::new();
    let mut streams: Vec<StreamInfo> = Vec::new();
    let mut std_info = ExtendedStandardInfo::default();
    let mut primary_name = String::new();
    let mut primary_parent_frs = 0u64;
    let mut primary_namespace = 255u8; // Invalid, will be replaced

    // Parse attributes
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    // SAFETY: We've verified the buffer is large enough.
    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
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

    // For base records, require at least one name
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
pub fn parse_record(data: &[u8], frs: u64) -> Option<ParsedRecord> {
    match parse_record_full(data, frs) {
        ParseResult::Base(record) => Some(record),
        _ => None,
    }
}
/// Parses `$STANDARD_INFORMATION` into `ExtendedStandardInfo`.
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS struct
fn parse_standard_info_full(data: &[u8], attr_offset: usize, result: &mut ExtendedStandardInfo) {
    use crate::ntfs::filetime_to_unix_micros;

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
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS struct
fn parse_file_name_full(data: &[u8], attr_offset: usize) -> Option<NameInfo> {
    use crate::ntfs::file_reference_to_frs;

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
    let name_u16: Vec<u16> = name_bytes
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
fn parse_data_attribute_full(
    data: &[u8],
    attr_offset: usize,
    header: &AttributeRecordHeader,
) -> Option<StreamInfo> {
    // Extract stream name from attribute header
    let stream_name = if header.name_length > 0 {
        let name_offset = attr_offset + header.name_offset as usize;
        let name_len = header.name_length as usize;
        if name_offset + name_len * 2 > data.len() {
            return None;
        }
        let name_bytes = &data[name_offset..name_offset + name_len * 2];
        let name_u16: Vec<u16> = name_bytes
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
// MFT Record Merger
// ============================================================================

use std::collections::HashMap;

/// Merges extension record attributes into base records.
///
/// This implements the C++ behavior where attributes from extension
/// records are merged into their base records.
pub struct MftRecordMerger {
    /// Base records indexed by FRS.
    base_records: HashMap<u64, ParsedRecord>,
    /// Pending extension attributes.
    extensions: Vec<ExtensionAttributes>,
}

impl MftRecordMerger {
    /// Creates a new merger.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_records: HashMap::new(),
            extensions: Vec::new(),
        }
    }

    /// Creates a new merger with estimated capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            base_records: HashMap::with_capacity(capacity),
            extensions: Vec::with_capacity(capacity / 100), // Extensions are rare
        }
    }

    /// Adds a parse result to the merger.
    pub fn add_result(&mut self, result: ParseResult) {
        match result {
            ParseResult::Base(record) => {
                self.base_records.insert(record.frs, record);
            }
            ParseResult::Extension(ext) => {
                self.extensions.push(ext);
            }
            ParseResult::Skip => {}
        }
    }

    /// Merges all extensions into their base records and returns the result.
    #[must_use]
    pub fn merge(mut self) -> Vec<ParsedRecord> {
        // Merge all extensions into their base records
        for ext in self.extensions {
            if let Some(base) = self.base_records.get_mut(&ext.base_frs) {
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

        // Recalculate sizes from merged streams
        for record in self.base_records.values_mut() {
            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
            }
        }

        self.base_records.into_values().collect()
    }

    /// Returns the number of base records.
    #[must_use]
    pub fn base_count(&self) -> usize {
        self.base_records.len()
    }

    /// Returns the number of pending extensions.
    #[must_use]
    pub fn extension_count(&self) -> usize {
        self.extensions.len()
    }
}

impl Default for MftRecordMerger {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Batch MFT Reader
// ============================================================================

/// Batch reader for efficient MFT reading.
///
/// Reads multiple records per I/O operation by reading entire clusters
/// or extent chunks at once.
#[derive(Debug)]
pub struct BatchMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Size of each file record in bytes.
    record_size: u32,
    /// Bytes per cluster.
    bytes_per_cluster: u32,
    /// Read block size (multiple of cluster size).
    read_block_size: usize,
    /// Aligned buffer for batch reads.
    buffer: AlignedBuffer,
}

impl BatchMftReader {
    /// Default read block size (1 MB).
    pub const DEFAULT_BLOCK_SIZE: usize = 1024 * 1024;

    /// Creates a new batch reader.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The MFT extent map
    /// * `bytes_per_cluster` - Cluster size in bytes
    #[must_use]
    pub fn new(extent_map: MftExtentMap, bytes_per_cluster: u32) -> Self {
        Self::with_block_size(extent_map, bytes_per_cluster, Self::DEFAULT_BLOCK_SIZE)
    }

    /// Creates a new batch reader with a custom block size.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The MFT extent map
    /// * `bytes_per_cluster` - Cluster size in bytes
    /// * `block_size` - Read block size (will be rounded to cluster boundary)
    #[must_use]
    pub fn with_block_size(
        extent_map: MftExtentMap,
        bytes_per_cluster: u32,
        block_size: usize,
    ) -> Self {
        let record_size = extent_map.bytes_per_record;

        // Round block size to cluster boundary
        let cluster_size = bytes_per_cluster as usize;
        let read_block_size = ((block_size + cluster_size - 1) / cluster_size) * cluster_size;

        let buffer = AlignedBuffer::new(read_block_size);

        Self {
            extent_map,
            record_size,
            bytes_per_cluster,
            read_block_size,
            buffer,
        }
    }

    /// Returns the number of records that fit in one read block.
    #[must_use]
    pub fn records_per_block(&self) -> usize {
        self.read_block_size / self.record_size as usize
    }

    /// Returns the extent map.
    #[must_use]
    pub fn extent_map(&self) -> &MftExtentMap {
        &self.extent_map
    }

    /// Reads a batch of records starting from a given FRS.
    ///
    /// This reads up to `records_per_block()` records in a single I/O
    /// operation.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `start_frs` - The first FRS to read
    ///
    /// # Returns
    ///
    /// A tuple of (buffer slice, first FRS in buffer, number of records read).
    #[allow(unsafe_code)] // Required: Windows FFI (SetFilePointerEx, ReadFile)
    pub fn read_batch(&mut self, handle: HANDLE, start_frs: u64) -> Result<(&[u8], u64, usize)> {
        // Get physical offset for the starting FRS
        let start_offset =
            self.extent_map
                .physical_offset(start_frs)
                .ok_or_else(|| MftError::RecordRead {
                    frs: start_frs,
                    reason: "FRS outside MFT extents".to_owned(),
                })?;

        // Align to cluster boundary for optimal I/O
        let cluster_size = u64::from(self.bytes_per_cluster);
        let aligned_offset = (start_offset / cluster_size) * cluster_size;

        // Calculate how many records we can read
        let total_records = self.extent_map.total_records();
        let max_records = (total_records - start_frs) as usize;
        let records_to_read = max_records.min(self.records_per_block());
        let bytes_to_read = records_to_read * self.record_size as usize;

        // Seek to the aligned offset
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        // Read the batch
        let read_size = bytes_to_read.min(self.buffer.len());
        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(&mut self.buffer.as_mut_slice()[..read_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        // Calculate offset within buffer for the first record
        let offset_in_buffer = (start_offset - aligned_offset) as usize;
        let usable_bytes = (bytes_read as usize).saturating_sub(offset_in_buffer);
        let records_read = usable_bytes / self.record_size as usize;

        Ok((
            &self.buffer.as_slice()
                [offset_in_buffer..offset_in_buffer + records_read * self.record_size as usize],
            start_frs,
            records_read,
        ))
    }

    /// Extracts a single record from a batch buffer.
    ///
    /// # Arguments
    ///
    /// * `batch_buffer` - The buffer returned by `read_batch()`
    /// * `index` - The index of the record within the batch (0-based)
    ///
    /// # Returns
    ///
    /// The record data slice, or `None` if the index is out of bounds.
    #[must_use]
    pub fn extract_record<'a>(&self, batch_buffer: &'a [u8], index: usize) -> Option<&'a [u8]> {
        let record_size = self.record_size as usize;
        let start = index * record_size;
        let end = start + record_size;

        if end <= batch_buffer.len() {
            Some(&batch_buffer[start..end])
        } else {
            None
        }
    }
}

// ============================================================================
// Parallel MFT Reader
// ============================================================================

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;

/// A read chunk representing a contiguous range of MFT records.
#[derive(Debug, Clone)]
pub struct ReadChunk {
    /// Physical byte offset on disk.
    pub disk_offset: u64,
    /// First FRS in this chunk.
    pub start_frs: u64,
    /// Number of records in this chunk.
    pub record_count: u64,
    /// Number of records to skip at the beginning (all unused).
    pub skip_begin: u64,
    /// Number of records to skip at the end (all unused).
    pub skip_end: u64,
}

impl ReadChunk {
    /// Returns the effective first FRS (after skipping unused records).
    #[must_use]
    pub fn effective_start_frs(&self) -> u64 {
        self.start_frs + self.skip_begin
    }

    /// Returns the effective record count (excluding skipped records).
    #[must_use]
    pub fn effective_record_count(&self) -> u64 {
        self.record_count
            .saturating_sub(self.skip_begin + self.skip_end)
    }

    /// Returns the byte size to read (after accounting for skips).
    #[must_use]
    pub fn read_size(&self, record_size: u32) -> u64 {
        self.effective_record_count() * u64::from(record_size)
    }
}

/// Generates optimized read chunks for the MFT.
///
/// This function implements the C++ optimization of:
/// 1. Splitting the MFT into chunks based on extents
/// 2. Using the bitmap to skip clusters with no in-use records
/// 3. Calculating skip_begin/skip_end for each chunk
///
/// # Arguments
///
/// * `extent_map` - The MFT extent map
/// * `bitmap` - Optional bitmap for skip optimization
/// * `chunk_size` - Target chunk size in bytes (default 1MB)
///
/// # Returns
///
/// Vector of read chunks optimized for I/O.
pub fn generate_read_chunks(
    extent_map: &MftExtentMap,
    bitmap: Option<&crate::platform::MftBitmap>,
    chunk_size: usize,
) -> Vec<ReadChunk> {
    let mut chunks = Vec::new();
    let record_size = extent_map.bytes_per_record;
    let cluster_size = extent_map.bytes_per_cluster;
    let records_per_cluster = cluster_size / record_size;

    // Process each extent
    for extent in extent_map.extents() {
        if extent.lcn < 0 {
            continue; // Skip sparse extents
        }

        let extent_start_frs = extent.vcn * u64::from(records_per_cluster);
        let extent_records = extent.cluster_count * u64::from(records_per_cluster);
        let extent_disk_offset = (extent.lcn as u64) * u64::from(cluster_size);

        // Split extent into chunks
        let records_per_chunk = (chunk_size / record_size as usize) as u64;
        let mut chunk_start = 0u64;

        while chunk_start < extent_records {
            let chunk_records = (extent_records - chunk_start).min(records_per_chunk);
            let chunk_frs_start = extent_start_frs + chunk_start;
            let chunk_frs_end = chunk_frs_start + chunk_records;

            // Calculate skip ranges using bitmap
            let (skip_begin, skip_end) = if let Some(bm) = bitmap {
                bm.calculate_skip_range(chunk_frs_start, chunk_frs_end)
            } else {
                (0, 0)
            };

            // Only add chunk if it has any in-use records
            if skip_begin + skip_end < chunk_records {
                chunks.push(ReadChunk {
                    disk_offset: extent_disk_offset + chunk_start * u64::from(record_size),
                    start_frs: chunk_frs_start,
                    record_count: chunk_records,
                    skip_begin,
                    skip_end,
                });
            }

            chunk_start += chunk_records;
        }
    }

    chunks
}

/// High-performance parallel MFT reader.
///
/// This reader implements the same optimizations as the C++ version:
/// - Extent-aware reading for fragmented MFTs
/// - Bitmap-based cluster skipping
/// - Parallel record processing using Rayon
/// - Batch I/O for reduced syscall overhead
#[derive(Debug)]
pub struct ParallelMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    chunk_size: usize,
    /// Progress counter (atomic for thread-safe updates).
    records_processed: Arc<AtomicU64>,
}

impl ParallelMftReader {
    /// Default chunk size (1 MB).
    pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

    /// Creates a new parallel reader.
    #[must_use]
    pub fn new(extent_map: MftExtentMap, bitmap: Option<crate::platform::MftBitmap>) -> Self {
        Self {
            extent_map,
            bitmap,
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            records_processed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Sets the chunk size for I/O operations.
    #[must_use]
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    /// Returns the number of records processed so far.
    #[must_use]
    pub fn records_processed(&self) -> u64 {
        self.records_processed.load(Ordering::Relaxed)
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.extent_map.total_records()
    }

    /// Reads and parses all MFT records in parallel.
    ///
    /// This is the main entry point for high-performance MFT reading.
    /// Uses the legacy `parse_record` which skips extension records.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    ///
    /// # Returns
    ///
    /// Vector of parsed records.
    pub fn read_all_parallel(&self, handle: HANDLE) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, false, None)
    }

    /// Reads and parses all MFT records in parallel with full C++ parity.
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the C++ implementation behavior.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    pub fn read_all_parallel_with_merge(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
    ) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, merge_extensions, None)
    }

    /// Reads and parses all MFT records in parallel with progress callback.
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the C++ implementation behavior.
    /// The progress callback is called during the I/O phase with (bytes_read,
    /// total_bytes).
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    /// * `progress_callback` - Optional callback called with (bytes_read,
    ///   total_bytes)
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    pub fn read_all_parallel_with_progress<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        info!(
            chunk_size = self.chunk_size,
            merge_extensions, "Starting parallel MFT read"
        );

        // Generate optimized read chunks
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let num_chunks = chunks.len();
        info!(num_chunks, "Generated read chunks");

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };
        info!(estimated_records, "Estimated record count");

        // Process chunks in parallel
        // Note: We read sequentially but parse in parallel for thread safety with
        // HANDLE
        let record_size = self.extent_map.bytes_per_record;
        let records_processed = Arc::clone(&self.records_processed);

        // Calculate total bytes to read for progress reporting
        let total_bytes_to_read: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Read all chunks (sequential I/O for handle safety)
        debug!("Reading all chunks into memory...");
        let mut total_bytes_read: u64 = 0;
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(chunks.len());

        for (idx, chunk) in chunks.into_iter().enumerate() {
            trace!(
                chunk_idx = idx,
                start_frs = chunk.start_frs,
                "Reading chunk"
            );
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    total_bytes_read += data.len() as u64;
                    trace!(
                        chunk_idx = idx,
                        bytes = data.len(),
                        total_bytes = total_bytes_read,
                        "Chunk read successfully"
                    );

                    // Report progress after each chunk
                    if let Some(ref cb) = progress_callback {
                        cb(total_bytes_read, total_bytes_to_read);
                    }

                    chunk_data.push((chunk, data));
                }
                Err(e) => {
                    warn!(chunk_idx = idx, error = ?e, "Failed to read chunk");
                }
            }
        }

        info!(
            chunks_read = chunk_data.len(),
            total_bytes = total_bytes_read,
            total_mb = total_bytes_read / (1024 * 1024),
            "All chunks read into memory"
        );

        if merge_extensions {
            // Full parsing with extension merging
            let parse_results: Vec<ParseResult> = chunk_data
                .par_iter()
                .flat_map(|(chunk, data)| {
                    let mut results = Vec::new();
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let record_data = &data[offset..offset + record_size];
                        let mut record_buf = record_data.to_vec();

                        // Apply fixup
                        if !apply_fixup(&mut record_buf) {
                            continue;
                        }

                        // Parse record (full)
                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;
                        results.push(parse_record_full(&record_buf, frs));

                        records_processed.fetch_add(1, Ordering::Relaxed);
                    }

                    results
                })
                .collect();

            // Merge extensions into base records
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in parse_results {
                merger.add_result(result);
            }

            Ok(merger.merge())
        } else {
            // Legacy parsing (skips extension records)
            let results: Vec<ParsedRecord> = chunk_data
                .par_iter()
                .flat_map(|(chunk, data)| {
                    let mut records = Vec::new();
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let record_data = &data[offset..offset + record_size];
                        let mut record_buf = record_data.to_vec();

                        // Apply fixup
                        if !apply_fixup(&mut record_buf) {
                            continue;
                        }

                        // Parse record
                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;
                        if let Some(parsed) = parse_record(&record_buf, frs) {
                            records.push(parsed);
                        }

                        records_processed.fetch_add(1, Ordering::Relaxed);
                    }

                    records
                })
                .collect();

            Ok(results)
        }
    }

    /// Reads a single chunk from disk.
    #[allow(unsafe_code)] // Required: Windows FFI (SetFilePointerEx, ReadFile)
    fn read_chunk(&self, handle: HANDLE, chunk: &ReadChunk, record_size: u32) -> Result<Vec<u8>> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
            / SECTOR_SIZE)
            * SECTOR_SIZE;

        // Allocate aligned buffer
        let mut buffer = AlignedBuffer::new(aligned_size);

        // Seek and read
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(buffer.as_mut_slice()),
                Some(&mut bytes_read),
                None,
            )?;
        }

        // Extract the actual data (accounting for alignment offset)
        let actual_size = (bytes_read as usize).saturating_sub(offset_adjustment);
        let data = buffer.as_slice()[offset_adjustment..offset_adjustment + actual_size].to_vec();

        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_buffer() {
        let buffer = AlignedBuffer::new(1024);
        assert_eq!(buffer.len(), 1024);

        // Check alignment
        let ptr = buffer.as_slice().as_ptr() as usize;
        assert_eq!(ptr % SECTOR_SIZE, 0);
    }

    #[test]
    fn test_aligned_buffer_write() {
        let mut buffer = AlignedBuffer::new(512);
        buffer.as_mut_slice()[0] = 0x42;
        assert_eq!(buffer.as_slice()[0], 0x42);
    }

    #[test]
    fn test_extent_map_contiguous() {
        let map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

        // First record should be at LCN 100 * 4096 = 409600
        assert_eq!(map.physical_offset(0), Some(409600));

        // Second record should be at 409600 + 1024 = 410624
        assert_eq!(map.physical_offset(1), Some(410624));

        // Record 4 should be at 409600 + 4096 = 413696 (next cluster)
        assert_eq!(map.physical_offset(4), Some(413696));
    }

    #[test]
    fn test_extent_map_fragmented() {
        // Create a fragmented extent map:
        // Extent 0: VCN 0-9, LCN 100 (10 clusters)
        // Extent 1: VCN 10-19, LCN 500 (10 clusters)
        let extents = vec![
            MftExtent {
                vcn: 0,
                cluster_count: 10,
                lcn: 100,
            },
            MftExtent {
                vcn: 10,
                cluster_count: 10,
                lcn: 500,
            },
        ];
        let map = MftExtentMap::new(extents, 4096, 1024);

        // Record 0 should be in first extent
        assert_eq!(map.physical_offset(0), Some(100 * 4096));

        // Record 40 (VCN 10) should be in second extent
        // VCN 10 = record 40 (4 records per cluster with 1024 byte records and 4096
        // byte clusters)
        assert_eq!(map.physical_offset(40), Some(500 * 4096));

        // Record 44 should be at VCN 11
        assert_eq!(map.physical_offset(44), Some(500 * 4096 + 4096));
    }
}
