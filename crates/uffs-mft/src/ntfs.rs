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

use core::mem::size_of;

// ============================================================================
// Constants
// ============================================================================

/// Magic number for FILE records ("FILE" in little-endian).
pub const FILE_RECORD_MAGIC: u32 = 0x454C_4946; // "FILE"

/// Magic number for INDX records ("INDX" in little-endian).
pub const INDX_RECORD_MAGIC: u32 = 0x5844_4E49; // "INDX"

/// Sector size (standard for NTFS).
pub const SECTOR_SIZE: usize = 512;

// ============================================================================
// Boot Sector
// ============================================================================

/// NTFS Boot Sector structure.
///
/// Located at the first sector of an NTFS volume, contains critical
/// filesystem parameters needed to locate and read the MFT.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NtfsBootSector {
    /// Jump instruction (3 bytes).
    pub jump: [u8; 3],
    /// OEM identifier ("NTFS    ").
    pub oem_id: [u8; 8],
    /// Bytes per sector (usually 512).
    pub bytes_per_sector: u16,
    /// Sectors per cluster.
    pub sectors_per_cluster: u8,
    /// Reserved sectors (unused in NTFS).
    pub reserved_sectors: u16,
    /// Padding (always 0).
    pub padding1: [u8; 3],
    /// Unused.
    pub unused1: u16,
    /// Media descriptor.
    pub media_descriptor: u8,
    /// Padding.
    pub padding2: u16,
    /// Sectors per track.
    pub sectors_per_track: u16,
    /// Number of heads.
    pub number_of_heads: u16,
    /// Hidden sectors.
    pub hidden_sectors: u32,
    /// Unused.
    pub unused2: u32,
    /// Unused.
    pub unused3: u32,
    /// Total sectors on volume.
    pub total_sectors: i64,
    /// Logical Cluster Number of `$MFT`.
    pub mft_start_lcn: i64,
    /// Logical Cluster Number of `$MFTMirr`.
    pub mft_mirror_start_lcn: i64,
    /// Clusters per File Record Segment (can be negative for byte shift).
    pub clusters_per_file_record: i8,
    /// Padding.
    pub padding3: [u8; 3],
    /// Clusters per Index Block.
    pub clusters_per_index_block: u32,
    /// Volume serial number.
    pub volume_serial_number: i64,
    /// Checksum.
    pub checksum: u32,
    /// Bootstrap code.
    pub bootstrap: [u8; 0x200 - 0x54],
}

impl NtfsBootSector {
    /// Validates that this is a valid NTFS boot sector.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check OEM ID starts with "NTFS"
        self.oem_id[0..4] == *b"NTFS"
    }

    /// Returns the cluster size in bytes.
    #[must_use]
    pub fn cluster_size(&self) -> u32 {
        u32::from(self.sectors_per_cluster) * u32::from(self.bytes_per_sector)
    }

    /// Returns the file record size in bytes.
    ///
    /// If `clusters_per_file_record` is positive, it's the number of clusters.
    /// If negative, the size is `2^(-clusters_per_file_record)` bytes.
    #[must_use]
    pub fn file_record_size(&self) -> u32 {
        if self.clusters_per_file_record >= 0 {
            #[expect(clippy::cast_sign_loss, reason = "checked positive above")]
            let clusters = self.clusters_per_file_record as u32;
            clusters * self.cluster_size()
        } else {
            #[expect(clippy::cast_sign_loss, reason = "negated negative value is positive")]
            let shift = (-self.clusters_per_file_record) as u32;
            1_u32 << shift
        }
    }

    /// Returns the byte offset of the MFT on the volume.
    #[must_use]
    pub fn mft_byte_offset(&self) -> u64 {
        #[expect(
            clippy::cast_sign_loss,
            reason = "MFT start LCN is always non-negative"
        )]
        let lcn = self.mft_start_lcn as u64;
        lcn * u64::from(self.cluster_size())
    }
}

// ============================================================================
// Multi-Sector Header (Update Sequence Array)
// ============================================================================

/// Multi-sector header present at the start of FILE and INDX records.
///
/// Contains the Update Sequence Array (USA) used for sector-level
/// integrity checking.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MultiSectorHeader {
    /// Magic number ("FILE" or "INDX").
    pub magic: u32,
    /// Offset to the Update Sequence Array.
    pub usa_offset: u16,
    /// Number of entries in the USA (including the check value).
    pub usa_count: u16,
}

impl MultiSectorHeader {
    /// Checks if this is a valid FILE record.
    #[must_use]
    pub const fn is_file_record(&self) -> bool {
        self.magic == FILE_RECORD_MAGIC
    }

    /// Checks if this is a valid INDX record.
    #[must_use]
    pub const fn is_index_record(&self) -> bool {
        self.magic == INDX_RECORD_MAGIC
    }
}

/// Applies the Update Sequence Array (USA) fixup to a record buffer.
///
/// NTFS uses the USA for sector-level integrity checking. The last 2 bytes
/// of each 512-byte sector are replaced with a check value, and the original
/// values are stored in the USA at the start of the record.
///
/// This function restores the original sector-end values and validates
/// that the check values match.
///
/// # Arguments
///
/// * `buffer` - Mutable buffer containing the record data
/// * `usa_offset` - Offset to the USA within the buffer
/// * `usa_count` - Number of USA entries (including the check value)
///
/// # Returns
///
/// `true` if all check values matched and fixup was successful,
/// `false` if any check value was incorrect (data corruption).
///
/// # Panics
///
/// This function does not panic - it performs bounds checking and returns
/// `false` if the buffer is too small.
#[must_use]
#[expect(clippy::indexing_slicing, reason = "bounds checked before each access")]
pub fn apply_usa_fixup(buffer: &mut [u8], usa_offset: u16, usa_count: u16) -> bool {
    let usa_offset_usize = usize::from(usa_offset);

    // Need at least the check value
    if usa_count < 1 || usa_offset_usize + 2 > buffer.len() {
        return false;
    }

    // Read the check value (first entry in USA)
    let check_value = u16::from_le_bytes([buffer[usa_offset_usize], buffer[usa_offset_usize + 1]]);

    let mut result = true;

    // Process each sector (starting from sector 1, since sector 0 contains the
    // header)
    for sector_idx in 1..usa_count {
        let sector_idx_usize = usize::from(sector_idx);

        // Offset of the last 2 bytes of this sector
        let sector_end_offset = sector_idx_usize * SECTOR_SIZE - 2;

        // Offset of the replacement value in the USA
        let usa_entry_offset = usa_offset_usize + sector_idx_usize * 2;

        // Check bounds
        if sector_end_offset + 2 > buffer.len() || usa_entry_offset + 2 > buffer.len() {
            break;
        }

        // Verify the check value matches
        let current_value =
            u16::from_le_bytes([buffer[sector_end_offset], buffer[sector_end_offset + 1]]);
        if current_value != check_value {
            result = false;
        }

        // Restore the original value from the USA
        buffer[sector_end_offset] = buffer[usa_entry_offset];
        buffer[sector_end_offset + 1] = buffer[usa_entry_offset + 1];
    }

    result
}

/// Applies USA fixup to a file record buffer in-place.
///
/// This is a convenience wrapper around `apply_usa_fixup` that reads
/// the USA offset and count from the record header.
///
/// # Returns
///
/// `true` if fixup was successful, `false` if the record is invalid
/// or corrupted.
#[must_use]
#[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
pub fn fixup_file_record(buffer: &mut [u8]) -> bool {
    if buffer.len() < size_of::<MultiSectorHeader>() {
        return false;
    }

    // SAFETY: We've verified the buffer is large enough for the header.
    let header: MultiSectorHeader = unsafe { core::ptr::read(buffer.as_ptr().cast()) };

    // Verify magic
    if !header.is_file_record() {
        return false;
    }

    apply_usa_fixup(buffer, header.usa_offset, header.usa_count)
}

// ============================================================================
// Attribute Type Codes
// ============================================================================

/// NTFS attribute type codes.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeType {
    /// Standard information (timestamps, flags).
    StandardInformation = 0x10,
    /// Attribute list (for records spanning multiple segments).
    AttributeList = 0x20,
    /// File name.
    FileName = 0x30,
    /// Object ID.
    ObjectId = 0x40,
    /// Security descriptor.
    SecurityDescriptor = 0x50,
    /// Volume name.
    VolumeName = 0x60,
    /// Volume information.
    VolumeInformation = 0x70,
    /// File data.
    Data = 0x80,
    /// Index root.
    IndexRoot = 0x90,
    /// Index allocation.
    IndexAllocation = 0xA0,
    /// Bitmap.
    Bitmap = 0xB0,
    /// Reparse point.
    ReparsePoint = 0xC0,
    /// EA information.
    EaInformation = 0xD0,
    /// Extended attributes.
    Ea = 0xE0,
    /// Property set.
    PropertySet = 0xF0,
    /// Logged utility stream.
    LoggedUtilityStream = 0x100,
    /// End marker.
    End = 0xFFFF_FFFF,
}

impl AttributeType {
    /// Creates an `AttributeType` from a raw u32 value.
    #[must_use]
    pub const fn from_u32(value: u32) -> Option<Self> {
        match value {
            0x10 => Some(Self::StandardInformation),
            0x20 => Some(Self::AttributeList),
            0x30 => Some(Self::FileName),
            0x40 => Some(Self::ObjectId),
            0x50 => Some(Self::SecurityDescriptor),
            0x60 => Some(Self::VolumeName),
            0x70 => Some(Self::VolumeInformation),
            0x80 => Some(Self::Data),
            0x90 => Some(Self::IndexRoot),
            0xA0 => Some(Self::IndexAllocation),
            0xB0 => Some(Self::Bitmap),
            0xC0 => Some(Self::ReparsePoint),
            0xD0 => Some(Self::EaInformation),
            0xE0 => Some(Self::Ea),
            0xF0 => Some(Self::PropertySet),
            0x100 => Some(Self::LoggedUtilityStream),
            0xFFFF_FFFF => Some(Self::End),
            _ => None,
        }
    }
}

// ============================================================================
// Attribute Record Header
// ============================================================================

/// Common header for all attribute records.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AttributeRecordHeader {
    /// Attribute type code.
    pub type_code: u32,
    /// Total length of this attribute record.
    pub length: u32,
    /// Non-zero if attribute is non-resident.
    pub is_non_resident: u8,
    /// Length of the attribute name (in characters).
    pub name_length: u8,
    /// Offset to the attribute name.
    pub name_offset: u16,
    /// Attribute flags (compressed, encrypted, sparse).
    pub flags: u16,
    /// Attribute instance number.
    pub instance: u16,
}

/// Resident attribute data (follows `AttributeRecordHeader`).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ResidentAttributeData {
    /// Length of the attribute value.
    pub value_length: u32,
    /// Offset to the attribute value (from start of attribute record).
    pub value_offset: u16,
    /// Resident flags.
    pub flags: u16,
}

/// Non-resident attribute data (follows `AttributeRecordHeader`).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NonResidentAttributeData {
    /// Lowest VCN covered by this attribute record.
    pub lowest_vcn: i64,
    /// Highest VCN covered by this attribute record.
    pub highest_vcn: i64,
    /// Offset to the mapping pairs (data runs).
    pub mapping_pairs_offset: u16,
    /// Compression unit size (log2).
    pub compression_unit: u8,
    /// Reserved.
    pub reserved: [u8; 5],
    /// Allocated size of the attribute.
    pub allocated_size: i64,
    /// Actual data size.
    pub data_size: i64,
    /// Initialized data size.
    pub initialized_size: i64,
    // CompressedSize follows only if compressed
}

// ============================================================================
// File Record Segment Header
// ============================================================================

/// Flags for file record segments.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRecordFlags {
    /// Record is in use.
    InUse = 0x0001,
    /// Record is a directory.
    Directory = 0x0002,
}

/// File Record Segment Header (MFT entry header).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FileRecordSegmentHeader {
    /// Multi-sector header with magic and USA info.
    pub multi_sector_header: MultiSectorHeader,
    /// Log file sequence number.
    pub log_file_sequence_number: u64,
    /// Sequence number (incremented on reuse).
    pub sequence_number: u16,
    /// Hard link count.
    pub link_count: u16,
    /// Offset to the first attribute.
    pub first_attribute_offset: u16,
    /// Flags (in use, directory).
    pub flags: u16,
    /// Bytes actually used in this record.
    pub bytes_in_use: u32,
    /// Bytes allocated for this record.
    pub bytes_allocated: u32,
    /// Base file record segment (for extension records).
    pub base_file_record_segment: u64,
    /// Next attribute instance number.
    pub next_attribute_number: u16,
    /// Reserved/padding.
    pub reserved: u16,
    /// Segment number (lower 32 bits).
    pub segment_number_lower: u32,
}

impl FileRecordSegmentHeader {
    /// Returns true if this record is in use.
    #[must_use]
    pub const fn is_in_use(&self) -> bool {
        (self.flags & 0x0001) != 0
    }

    /// Returns true if this record is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        (self.flags & 0x0002) != 0
    }

    /// Returns true if this is a base record (not an extension).
    #[must_use]
    pub const fn is_base_record(&self) -> bool {
        self.base_file_record_segment == 0
    }
}

// ============================================================================
// Standard Information Attribute (0x10)
// ============================================================================

/// Standard Information attribute content (NTFS 1.2 - 36 bytes).
///
/// Contains timestamps and basic file attributes.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct StandardInformation {
    /// File creation time (FILETIME).
    pub creation_time: i64,
    /// Last modification time (FILETIME).
    pub modification_time: i64,
    /// Last MFT change time (FILETIME).
    pub mft_change_time: i64,
    /// Last access time (FILETIME).
    pub access_time: i64,
    /// File attributes (same as DOS attributes).
    pub file_attributes: u32,
    // Extended fields follow in NTFS 3.0+
}

/// Standard Information attribute content (NTFS 3.0+ - 72 bytes).
///
/// Contains timestamps, file attributes, and extended fields for
/// security, quota, and USN journal tracking.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct StandardInformationExtended {
    /// File creation time (FILETIME).
    pub creation_time: i64,
    /// Last modification time (FILETIME).
    pub modification_time: i64,
    /// Last MFT change time (FILETIME).
    pub mft_change_time: i64,
    /// Last access time (FILETIME).
    pub access_time: i64,
    /// File attributes (same as DOS attributes).
    pub file_attributes: u32,
    // NTFS 3.0+ extended fields (36 bytes additional)
    /// Maximum number of versions (usually 0).
    pub max_versions: u32,
    /// Version number (usually 0).
    pub version_number: u32,
    /// Class ID (usually 0).
    pub class_id: u32,
    /// Owner ID for quota tracking.
    pub owner_id: u32,
    /// Security ID - index into $Secure file.
    pub security_id: u32,
    /// Quota charged (bytes charged to user's quota).
    pub quota_charged: u64,
    /// Update Sequence Number - correlates with USN journal.
    pub usn: u64,
}

/// Size of NTFS 1.2 `$STANDARD_INFORMATION` (36 bytes).
pub const STANDARD_INFO_SIZE_V12: usize = 36;

/// Size of NTFS 3.0+ `$STANDARD_INFORMATION` (72 bytes).
pub const STANDARD_INFO_SIZE_V30: usize = 72;

// ============================================================================
// File Name Attribute (0x30)
// ============================================================================

/// File name namespace flags.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileNamespace {
    /// POSIX (case-sensitive, allows most characters).
    Posix = 0,
    /// Win32 (case-insensitive, standard Windows names).
    Win32 = 1,
    /// DOS 8.3 format.
    Dos = 2,
    /// Win32 and DOS (name is valid for both).
    Win32AndDos = 3,
}

/// File Name attribute content.
///
/// Contains the file name and parent directory reference.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FileNameAttribute {
    /// Parent directory file reference.
    pub parent_directory: u64,
    /// File creation time.
    pub creation_time: i64,
    /// Last modification time.
    pub modification_time: i64,
    /// Last MFT change time.
    pub mft_change_time: i64,
    /// Last access time.
    pub access_time: i64,
    /// Allocated size.
    pub allocated_size: i64,
    /// Real size (data size).
    pub data_size: i64,
    /// File attributes.
    pub file_attributes: u32,
    /// Packed EA size / reparse tag.
    pub packed_ea_size: u16,
    /// Reserved.
    pub reserved: u16,
    /// File name length in characters.
    pub file_name_length: u8,
    /// File name namespace.
    pub file_name_namespace: u8,
    // File name (UTF-16LE) follows immediately
}

impl FileNameAttribute {
    /// Returns the parent directory FRS (lower 48 bits).
    #[must_use]
    pub const fn parent_frs(&self) -> u64 {
        self.parent_directory & 0x0000_FFFF_FFFF_FFFF
    }

    /// Returns the parent directory sequence number (upper 16 bits).
    #[must_use]
    pub const fn parent_sequence(&self) -> u16 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "extracting upper 16 bits into u16"
        )]
        let seq = (self.parent_directory >> 48_i32) as u16;
        seq
    }
}

// ============================================================================
// Reparse Point Attribute (0xC0)
// ============================================================================

/// Reparse point type flags.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReparseTag {
    /// Mount point (junction).
    MountPoint = 0xA000_0003,
    /// Symbolic link.
    SymbolicLink = 0xA000_000C,
    /// WOF compressed file.
    WofCompressed = 0x8000_0017,
    /// Windows Container Image.
    WindowsContainerImage = 0x8000_0018,
    /// Global reparse.
    GlobalReparse = 0x8000_0019,
    /// App execution link.
    AppExecLink = 0x8000_001B,
    /// OneDrive/Cloud.
    Cloud = 0x9000_001A,
    /// GVFS.
    Gvfs = 0x9000_001C,
    /// Linux symbolic link (WSL).
    LinuxSymbolicLink = 0xA000_001D,
}

/// Reparse point header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ReparsePointHeader {
    /// Reparse tag.
    pub reparse_tag: u32,
    /// Data length (excluding header).
    pub data_length: u16,
    /// Reserved.
    pub reserved: u16,
}

/// Mount point / symbolic link reparse data buffer.
///
/// Used for parsing junction points and symbolic links.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ReparseMountPointBuffer {
    /// Offset of the substitute name in `PathBuffer` (in bytes).
    pub substitute_name_offset: u16,
    /// Length of the substitute name (in bytes).
    pub substitute_name_length: u16,
    /// Offset of the print name in `PathBuffer` (in bytes).
    pub print_name_offset: u16,
    /// Length of the print name (in bytes).
    pub print_name_length: u16,
    // PathBuffer follows (variable length, UTF-16LE)
}

// ============================================================================
// Attribute List (0x20)
// ============================================================================

/// Attribute List entry.
///
/// Used when a file has too many attributes to fit in a single MFT record.
/// The attribute list points to the locations of all attributes across
/// multiple MFT records.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AttributeListEntry {
    /// Attribute type code.
    pub attribute_type: u32,
    /// Length of this entry.
    pub length: u16,
    /// Length of the attribute name (in characters).
    pub name_length: u8,
    /// Offset to the attribute name.
    pub name_offset: u8,
    /// Starting VCN (for non-resident attributes).
    pub start_vcn: u64,
    /// File reference of the MFT record containing this attribute.
    pub file_reference: u64,
    /// Attribute instance number.
    pub attribute_id: u16,
    // Attribute name follows (if name_length > 0)
}

impl AttributeListEntry {
    /// Returns the FRS of the record containing this attribute.
    #[must_use]
    pub const fn target_frs(&self) -> u64 {
        self.file_reference & 0x0000_FFFF_FFFF_FFFF
    }

    /// Returns the sequence number of the target record.
    #[must_use]
    pub const fn target_sequence(&self) -> u16 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "extracting upper 16 bits into u16"
        )]
        let seq = (self.file_reference >> 48_i32) as u16;
        seq
    }
}

// ============================================================================
// Index Structures (for directories)
// ============================================================================

/// Index header (common to `INDEX_ROOT` and `INDEX_ALLOCATION`).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IndexHeader {
    /// Offset to the first index entry (from start of this header).
    pub first_entry_offset: u32,
    /// Offset to the first free byte (total size of entries).
    pub first_free_byte: u32,
    /// Allocated size of the index entries.
    pub bytes_available: u32,
    /// Flags: 0x01 = has `INDEX_ALLOCATION` (large index).
    pub flags: u8,
    /// Reserved.
    pub reserved: [u8; 3],
}

impl IndexHeader {
    /// Returns true if this index has an `INDEX_ALLOCATION` attribute.
    #[must_use]
    pub const fn has_index_allocation(&self) -> bool {
        (self.flags & 0x01) != 0
    }
}

/// Index root attribute content (0x90).
///
/// Contains the B-tree root for directory indexes.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IndexRoot {
    /// Type of the indexed attribute (usually 0x30 for `$FILE_NAME`).
    pub indexed_attribute_type: u32,
    /// Collation rule.
    pub collation_rule: u32,
    /// Size of each index block (in bytes).
    pub bytes_per_index_block: u32,
    /// Clusters per index block.
    pub clusters_per_index_block: u8,
    /// Padding.
    pub padding: [u8; 3],
    /// Index header.
    pub header: IndexHeader,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Converts a Windows FILETIME (100-nanosecond intervals since 1601-01-01)
/// to Unix timestamp in microseconds.
#[must_use]
pub const fn filetime_to_unix_micros(filetime: i64) -> i64 {
    // FILETIME epoch is 1601-01-01, Unix epoch is 1970-01-01
    // Difference is 11644473600 seconds = 116444736000000000 * 100ns
    const FILETIME_UNIX_DIFF: i64 = 116_444_736_000_000_000;

    // C++ parity: allow negative Unix timestamps (pre-1970 dates).
    // C++ uses FileTimeToLocalFileTime() which handles all valid FILETIME values.
    // Only clamp for filetime == 0 (unset/null timestamp).
    if filetime == 0 {
        return 0;
    }

    // Convert from 100ns to microseconds (works for both positive and negative offsets)
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "extracting upper 16 bits into u16"
    )]
    let seq = (file_reference >> 48_i32) as u16;
    seq
}

// ============================================================================
// Attribute Iterator
// ============================================================================

/// Iterator over attributes in a file record.
///
/// Provides safe iteration over the attribute chain in an MFT record.
pub struct AttributeIterator<'a> {
    /// The record buffer.
    data: &'a [u8],
    /// Current offset within the buffer.
    offset: usize,
    /// Maximum valid offset (`bytes_in_use` from header).
    max_offset: usize,
}

impl<'a> AttributeIterator<'a> {
    /// Creates a new attribute iterator from a file record buffer.
    ///
    /// The buffer should contain a complete file record with USA fixup
    /// already applied.
    ///
    /// # Returns
    ///
    /// `None` if the buffer is too small or doesn't contain a valid record.
    #[must_use]
    #[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
    #[expect(clippy::missing_const_for_fn, reason = "can't be const due to unsafe")]
    pub fn new(record: &'a [u8]) -> Option<Self> {
        if record.len() < size_of::<FileRecordSegmentHeader>() {
            return None;
        }

        // SAFETY: We've verified the buffer is large enough for the header.
        let header: FileRecordSegmentHeader = unsafe { core::ptr::read(record.as_ptr().cast()) };

        // Copy the packed field to avoid unaligned reference
        let multi_sector_header = header.multi_sector_header;
        if !multi_sector_header.is_file_record() {
            return None;
        }

        let first_attr = header.first_attribute_offset as usize;
        let bytes_in_use = header.bytes_in_use as usize;

        if first_attr >= record.len() || bytes_in_use > record.len() {
            return None;
        }

        Some(Self {
            data: record,
            offset: first_attr,
            max_offset: bytes_in_use,
        })
    }
}

/// A reference to an attribute within a record buffer.
#[derive(Debug, Clone, Copy)]
pub struct AttributeRef<'a> {
    /// The attribute data slice.
    pub data: &'a [u8],
    /// The attribute header.
    pub header: AttributeRecordHeader,
}

impl<'a> AttributeRef<'a> {
    /// Returns the attribute type.
    #[must_use]
    pub const fn attribute_type(&self) -> Option<AttributeType> {
        AttributeType::from_u32(self.header.type_code)
    }

    /// Returns true if this is a non-resident attribute.
    #[must_use]
    pub const fn is_non_resident(&self) -> bool {
        self.header.is_non_resident != 0
    }

    /// Returns the resident attribute value, if this is a resident attribute.
    #[must_use]
    #[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
    #[expect(clippy::indexing_slicing, reason = "bounds checked before access")]
    pub fn resident_value(&self) -> Option<&'a [u8]> {
        if self.is_non_resident() {
            return None;
        }

        let header_size = size_of::<AttributeRecordHeader>();
        if self.data.len() < header_size + size_of::<ResidentAttributeData>() {
            return None;
        }

        // SAFETY: We've verified the buffer is large enough.
        let resident: ResidentAttributeData =
            unsafe { core::ptr::read(self.data[header_size..].as_ptr().cast()) };

        let value_offset = resident.value_offset as usize;
        let value_length = resident.value_length as usize;

        if value_offset + value_length > self.data.len() {
            return None;
        }

        Some(&self.data[value_offset..value_offset + value_length])
    }

    /// Returns the non-resident attribute data, if this is a non-resident
    /// attribute.
    #[must_use]
    #[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
    #[expect(clippy::indexing_slicing, reason = "bounds checked before access")]
    pub fn non_resident_data(&self) -> Option<NonResidentAttributeData> {
        if !self.is_non_resident() {
            return None;
        }

        let header_size = size_of::<AttributeRecordHeader>();
        if self.data.len() < header_size + size_of::<NonResidentAttributeData>() {
            return None;
        }

        // SAFETY: We've verified the buffer is large enough.
        Some(unsafe { core::ptr::read(self.data[header_size..].as_ptr().cast()) })
    }

    /// Parses data runs from a non-resident attribute.
    #[must_use]
    pub fn data_runs(&self) -> Vec<DataRun> {
        if !self.is_non_resident() {
            return Vec::new();
        }

        extract_data_runs_from_attribute(self.data)
    }

    /// Returns the attribute name, if present.
    #[must_use]
    #[expect(clippy::indexing_slicing, reason = "bounds checked before access")]
    pub fn name(&self) -> Option<&'a [u16]> {
        if self.header.name_length == 0 {
            return None;
        }

        let name_offset = self.header.name_offset as usize;
        let name_length = self.header.name_length as usize;
        let name_end = name_offset + name_length * 2;

        if name_end > self.data.len() {
            return None;
        }

        // Bounds verified above
        let name_bytes = &self.data[name_offset..name_end];
        if name_bytes.len() % 2 != 0 {
            return None;
        }

        // Convert bytes to u16 slice (handling potential unaligned access)
        // Note: We build a Vec but return None since we can't return a reference to
        // local data
        for idx in 0..name_length {
            let offset = idx * 2;
            let _char = u16::from_le_bytes([name_bytes[offset], name_bytes[offset + 1]]);
        }

        // This is a bit awkward - we need to return a reference but we created a Vec
        // For now, return None and let callers use name_bytes directly
        None
    }
}

impl<'a> Iterator for AttributeIterator<'a> {
    type Item = AttributeRef<'a>;

    #[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
    #[expect(clippy::indexing_slicing, reason = "bounds checked before access")]
    fn next(&mut self) -> Option<Self::Item> {
        // Check if we've reached the end
        if self.offset + size_of::<AttributeRecordHeader>() > self.max_offset {
            return None;
        }

        // SAFETY: We've verified the buffer is large enough.
        let header: AttributeRecordHeader =
            unsafe { core::ptr::read(self.data[self.offset..].as_ptr().cast()) };

        // Check for end marker
        if header.type_code == 0xFFFF_FFFF {
            return None;
        }

        // Validate length
        let length = header.length as usize;
        if length < size_of::<AttributeRecordHeader>() || self.offset + length > self.max_offset {
            return None;
        }

        let attr_data = &self.data[self.offset..self.offset + length];
        self.offset += length;

        Some(AttributeRef {
            data: attr_data,
            header,
        })
    }
}

// ============================================================================
// Data Runs (Mapping Pairs)
// ============================================================================

/// A single data run (extent) from a non-resident attribute.
///
/// Data runs describe the physical layout of non-resident attribute data
/// on disk. Each run specifies a contiguous range of clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataRun {
    /// Virtual Cluster Number - offset within the attribute data.
    pub vcn: i64,
    /// Number of clusters in this run.
    pub cluster_count: u64,
    /// Logical Cluster Number - physical location on disk.
    /// A value of 0 indicates a sparse (unallocated) run.
    pub lcn: i64,
}

impl DataRun {
    /// Returns true if this is a sparse (unallocated) run.
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        self.lcn == 0
    }

    /// Returns the byte offset of this run on the volume.
    #[must_use]
    #[expect(clippy::cast_sign_loss, reason = "lcn is checked positive before cast")]
    pub fn byte_offset(&self, bytes_per_cluster: u32) -> u64 {
        if self.lcn <= 0 {
            0
        } else {
            self.lcn as u64 * u64::from(bytes_per_cluster)
        }
    }

    /// Returns the size of this run in bytes.
    #[must_use]
    pub fn byte_size(&self, bytes_per_cluster: u32) -> u64 {
        self.cluster_count * u64::from(bytes_per_cluster)
    }
}

/// Parses data runs (mapping pairs) from a non-resident attribute.
///
/// The mapping pairs are a compressed representation of the VCN-to-LCN
/// mapping for non-resident attribute data. Each entry encodes:
/// - Length of the run (number of clusters)
/// - Offset from the previous LCN (signed, delta-encoded)
///
/// # Arguments
///
/// * `data` - The raw mapping pairs data (starting at `mapping_pairs_offset`)
/// * `lowest_vcn` - The starting VCN for this attribute record
///
/// # Returns
///
/// A vector of `DataRun` entries describing the physical layout.
#[must_use]
#[expect(clippy::similar_names, reason = "vcn and lcn are standard NTFS terms")]
#[expect(
    clippy::indexing_slicing,
    reason = "bounds checked in while loop condition"
)]
pub fn parse_data_runs(data: &[u8], lowest_vcn: i64) -> Vec<DataRun> {
    let mut runs = Vec::new();
    let mut offset = 0;
    let mut current_vcn = lowest_vcn;
    let mut current_lcn: i64 = 0;

    while offset < data.len() {
        // First byte encodes the sizes of the length and offset fields
        let header = data[offset];
        if header == 0 {
            // End of data runs
            break;
        }

        let length_size = (header & 0x0F) as usize;
        let offset_size = ((header >> 4_i32) & 0x0F) as usize;

        offset += 1;

        // Validate we have enough data
        if offset + length_size + offset_size > data.len() {
            break;
        }

        // Parse the run length (unsigned)
        let run_length = parse_variable_length_unsigned(&data[offset..offset + length_size]);
        offset += length_size;

        // Parse the LCN offset (signed, delta from previous LCN)
        let lcn_delta = if offset_size > 0 {
            parse_variable_length_signed(&data[offset..offset + offset_size])
        } else {
            0 // Sparse run
        };
        offset += offset_size;

        // Update current LCN (delta-encoded)
        current_lcn += lcn_delta;

        runs.push(DataRun {
            vcn: current_vcn,
            cluster_count: run_length,
            lcn: if offset_size > 0 { current_lcn } else { 0 },
        });

        // run_length is u64, cast to i64 is safe for reasonable cluster counts
        #[expect(
            clippy::cast_possible_wrap,
            reason = "cluster counts are small enough to fit in i64"
        )]
        {
            current_vcn += run_length as i64;
        }
    }

    runs
}

/// Parses a variable-length unsigned integer (little-endian).
#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity alongside parse_variable_length_signed"
)]
fn parse_variable_length_unsigned(data: &[u8]) -> u64 {
    let mut value: u64 = 0;
    for (i, &byte) in data.iter().enumerate() {
        value |= u64::from(byte) << (i * 8);
    }
    value
}

/// Parses a variable-length signed integer (little-endian, sign-extended).
#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity alongside parse_variable_length_unsigned"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "data.is_empty() check ensures data.len() >= 1"
)]
fn parse_variable_length_signed(data: &[u8]) -> i64 {
    if data.is_empty() {
        return 0;
    }

    let mut value: i64 = 0;
    for (idx, &byte) in data.iter().enumerate() {
        value |= i64::from(byte) << (idx * 8);
    }

    // Sign-extend if the high bit of the last byte is set
    let last_byte = data[data.len() - 1];
    if last_byte & 0x80 != 0 {
        // Sign extend
        let shift = data.len() * 8;
        if shift < 64 {
            value |= !0_i64 << shift;
        }
    }

    value
}

/// Extracts data runs from a non-resident attribute record.
///
/// # Arguments
///
/// * `attr_data` - The complete attribute record data
///
/// # Returns
///
/// A vector of `DataRun` entries, or an empty vector if parsing fails.
#[must_use]
#[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
#[expect(clippy::indexing_slicing, reason = "bounds checked before access")]
pub fn extract_data_runs_from_attribute(attr_data: &[u8]) -> Vec<DataRun> {
    if attr_data.len() < size_of::<AttributeRecordHeader>() {
        return Vec::new();
    }

    // SAFETY: We've verified the buffer is large enough.
    let header: AttributeRecordHeader = unsafe { core::ptr::read(attr_data.as_ptr().cast()) };

    // Must be non-resident
    if header.is_non_resident == 0 {
        return Vec::new();
    }

    // Read non-resident data
    let nr_offset = size_of::<AttributeRecordHeader>();
    if attr_data.len() < nr_offset + size_of::<NonResidentAttributeData>() {
        return Vec::new();
    }

    // SAFETY: We've verified the buffer is large enough.
    let nr_data: NonResidentAttributeData =
        unsafe { core::ptr::read(attr_data[nr_offset..].as_ptr().cast()) };

    let mapping_pairs_offset = nr_data.mapping_pairs_offset as usize;
    if mapping_pairs_offset >= attr_data.len() {
        return Vec::new();
    }

    let mapping_pairs_data = &attr_data[mapping_pairs_offset..];
    parse_data_runs(mapping_pairs_data, nr_data.lowest_vcn)
}

// ============================================================================
// High-Level Data Structures (for C++ parity)
// ============================================================================

/// Information about a single file name (hard link).
///
/// Each file can have multiple names via hard links. This struct captures
/// all the information about a single name entry, including the `$FILE_NAME`
/// timestamps which often differ from `$STANDARD_INFORMATION` timestamps.
#[derive(Debug, Clone, Default)]
pub struct NameInfo {
    /// The file name.
    pub name: String,
    /// Parent directory FRS.
    pub parent_frs: u64,
    /// Namespace (0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS).
    pub namespace: u8,
    /// Creation time from `$FILE_NAME` (Unix microseconds).
    pub fn_created: i64,
    /// Modification time from `$FILE_NAME` (Unix microseconds).
    pub fn_modified: i64,
    /// Access time from `$FILE_NAME` (Unix microseconds).
    pub fn_accessed: i64,
    /// MFT change time from `$FILE_NAME` (Unix microseconds).
    pub fn_mft_changed: i64,
    /// FRS of the MFT record this name was parsed from (base or extension).
    /// Used to sort merged names by encounter order to match C++ behavior.
    pub source_frs: u64,
}

/// Information about a single data stream.
///
/// NTFS files can have multiple data streams (Alternate Data Streams).
/// The default stream has an empty name.
#[derive(Debug, Clone, Default)]
pub struct StreamInfo {
    /// Stream name (empty for default stream).
    pub name: String,
    /// Logical size in bytes.
    pub size: u64,
    /// Allocated size on disk.
    pub allocated_size: u64,
    /// Whether this stream is sparse.
    pub is_sparse: bool,
    /// Whether this stream is compressed.
    pub is_compressed: bool,
    /// Whether this stream's data is resident (stored in MFT record itself).
    /// Resident streams are typically < 700 bytes and stored inline.
    pub is_resident: bool,
}

/// Extended standard information with individual flags.
///
/// Matches the C++ `StandardInfo` struct with 15+ boolean flags
/// for easier querying in Polars. Also includes NTFS 3.0+ extended
/// fields (`usn`, `security_id`, `owner_id`) for forensic analysis.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "NTFS has many boolean attribute flags"
)]
pub struct ExtendedStandardInfo {
    /// File creation time (Unix microseconds).
    pub created: i64,
    /// Last modification time (Unix microseconds).
    pub modified: i64,
    /// Last access time (Unix microseconds).
    pub accessed: i64,
    /// MFT record change time (Unix microseconds).
    pub mft_changed: i64,
    // NTFS 3.0+ extended fields (forensic value)
    /// Update Sequence Number - correlates with USN journal (`$UsnJrnl`).
    pub usn: u64,
    /// Security ID - index into `$Secure` file for ACL lookup.
    pub security_id: u32,
    /// Owner ID - for quota tracking.
    pub owner_id: u32,
    // Boolean flags
    /// Read-only flag.
    pub is_readonly: bool,
    /// Hidden flag.
    pub is_hidden: bool,
    /// System flag.
    pub is_system: bool,
    /// Archive flag.
    pub is_archive: bool,
    /// Device flag.
    pub is_device: bool,
    /// Normal flag.
    pub is_normal: bool,
    /// Temporary flag.
    pub is_temporary: bool,
    /// Sparse file flag.
    pub is_sparse: bool,
    /// Reparse point flag.
    pub is_reparse: bool,
    /// Compressed flag.
    pub is_compressed: bool,
    /// Offline flag.
    pub is_offline: bool,
    /// Not content indexed flag.
    pub is_not_content_indexed: bool,
    /// Encrypted flag.
    pub is_encrypted: bool,
    /// Integrity stream flag.
    pub is_integrity_stream: bool,
    /// Virtual flag.
    pub is_virtual: bool,
    /// No scrub data flag.
    pub is_no_scrub_data: bool,
    /// Pinned flag.
    pub is_pinned: bool,
    /// Unpinned flag.
    pub is_unpinned: bool,
}

impl ExtendedStandardInfo {
    /// Creates from raw file attributes.
    #[must_use]
    pub fn from_attributes(attrs: u32) -> Self {
        Self {
            is_readonly: (attrs & 0x0001) != 0,
            is_hidden: (attrs & 0x0002) != 0,
            is_system: (attrs & 0x0004) != 0,
            is_archive: (attrs & 0x0020) != 0,
            is_device: (attrs & 0x0040) != 0,
            is_normal: (attrs & 0x0080) != 0,
            is_temporary: (attrs & 0x0100) != 0,
            is_sparse: (attrs & 0x0200) != 0,
            is_reparse: (attrs & 0x0400) != 0,
            is_compressed: (attrs & 0x0800) != 0,
            is_offline: (attrs & 0x1000) != 0,
            is_not_content_indexed: (attrs & 0x2000) != 0,
            is_encrypted: (attrs & 0x4000) != 0,
            is_integrity_stream: (attrs & 0x8000) != 0,
            is_virtual: (attrs & 0x0001_0000) != 0,
            is_no_scrub_data: (attrs & 0x0002_0000) != 0,
            is_pinned: (attrs & 0x0008_0000) != 0,
            is_unpinned: (attrs & 0x0010_0000) != 0,
            ..Default::default()
        }
    }

    /// Returns the raw flags as u32.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "can't be const due to if statements"
    )]
    pub fn to_raw_flags(&self) -> u32 {
        let mut flags = 0_u32;
        if self.is_readonly {
            flags |= 0x0001;
        }
        if self.is_hidden {
            flags |= 0x0002;
        }
        if self.is_system {
            flags |= 0x0004;
        }
        if self.is_archive {
            flags |= 0x0020;
        }
        if self.is_device {
            flags |= 0x0040;
        }
        if self.is_normal {
            flags |= 0x0080;
        }
        if self.is_temporary {
            flags |= 0x0100;
        }
        if self.is_sparse {
            flags |= 0x0200;
        }
        if self.is_reparse {
            flags |= 0x0400;
        }
        if self.is_compressed {
            flags |= 0x0800;
        }
        if self.is_offline {
            flags |= 0x1000;
        }
        if self.is_not_content_indexed {
            flags |= 0x2000;
        }
        if self.is_encrypted {
            flags |= 0x4000;
        }
        if self.is_integrity_stream {
            flags |= 0x8000;
        }
        if self.is_virtual {
            flags |= 0x0001_0000;
        }
        if self.is_no_scrub_data {
            flags |= 0x0002_0000;
        }
        if self.is_pinned {
            flags |= 0x0008_0000;
        }
        if self.is_unpinned {
            flags |= 0x0010_0000;
        }
        flags
    }
}

// ============================================================================
// Size assertions
// ============================================================================

#[expect(
    clippy::missing_assert_message,
    reason = "compile-time size checks; messages not needed"
)]
const _: () = {
    // Verify struct sizes match expected on-disk sizes
    assert!(size_of::<NtfsBootSector>() == 512);
    assert!(size_of::<MultiSectorHeader>() == 8);
    assert!(size_of::<AttributeRecordHeader>() == 16);
    assert!(size_of::<ResidentAttributeData>() == 8);
    assert!(size_of::<NonResidentAttributeData>() == 48);
    assert!(size_of::<FileRecordSegmentHeader>() == 48);
    assert!(size_of::<StandardInformation>() == 36);
    assert!(size_of::<FileNameAttribute>() == 66);
    assert!(size_of::<ReparsePointHeader>() == 8);
};

// ============================================================================
// Stream Filtering
// ============================================================================

/// Checks if a stream name is an internal Windows stream that should be
/// filtered out during output expansion.
///
/// Internal Windows streams start with `$` followed by uppercase letters:
/// - `$DSC` - Directory Service Cache
/// - `$REPARSE` - Reparse point data
/// - `$EA` - Extended Attributes
/// - `$EA_INFORMATION` - Extended Attributes info
/// - `$TXF_DATA` - Transactional NTFS data
/// - `$OBJECT_ID` - Object IDs
/// - `$LOGGED_UTILITY_STREAM` - Logged utility stream
///
/// User-visible streams like `Zone.Identifier`, `com.dropbox.attrs`, etc. do
/// NOT start with `$`. Streams like `${GUID}.Metadata` (iCloud) start with `${`
/// and are user-visible.
///
/// This matches C++ behavior where non-$DATA attributes are filtered during
/// output when `match_attributes=false` (`ntfs_index.hpp` line 1388-1392):
/// ```cpp
/// bool const is_attribute = k->type_name_id &&
///     (k->type_name_id << (CHAR_BIT / 2)) != static_cast<int>(ntfs::AttributeTypeCode::AttributeData);
/// if (!match_attributes && is_attribute) { continue; }
/// ```
#[inline]
#[must_use]
pub fn is_internal_windows_stream(name: &str) -> bool {
    // Must start with '$' followed by an uppercase letter (not '{')
    // This allows `${GUID}.Metadata` style streams through
    name.strip_prefix('$').is_some_and(|rest| {
        rest.chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filetime_conversion() {
        // Test known value: 2024-01-01 00:00:00 UTC
        // FILETIME: 133485408000000000 (100ns intervals since 1601-01-01)
        let filetime: i64 = 133_485_408_000_000_000;
        let unix_micros = filetime_to_unix_micros(filetime);

        // Expected: 1704067200 seconds = 1704067200000000 microseconds
        assert_eq!(unix_micros, 1_704_067_200_000_000);
    }

    #[test]
    fn test_file_reference_extraction() {
        // File reference with FRS=12345 and sequence=7
        let file_ref: u64 = (7_u64 << 48) | 0x3039;

        assert_eq!(file_reference_to_frs(file_ref), 12345);
        assert_eq!(file_reference_to_sequence(file_ref), 7);
    }

    #[test]
    fn test_attribute_type_from_u32() {
        assert_eq!(
            AttributeType::from_u32(0x10),
            Some(AttributeType::StandardInformation)
        );
        assert_eq!(AttributeType::from_u32(0x30), Some(AttributeType::FileName));
        assert_eq!(AttributeType::from_u32(0x80), Some(AttributeType::Data));
        assert_eq!(
            AttributeType::from_u32(0xFFFF_FFFF),
            Some(AttributeType::End)
        );
        assert_eq!(AttributeType::from_u32(0x99), None);
    }

    #[test]
    fn test_file_record_flags() {
        let header = FileRecordSegmentHeader {
            multi_sector_header: MultiSectorHeader {
                magic: FILE_RECORD_MAGIC,
                usa_offset: 0,
                usa_count: 0,
            },
            log_file_sequence_number: 0,
            sequence_number: 1,
            link_count: 1,
            first_attribute_offset: 56,
            flags: 0x0003, // In use + Directory
            bytes_in_use: 0,
            bytes_allocated: 0,
            base_file_record_segment: 0,
            next_attribute_number: 0,
            reserved: 0,
            segment_number_lower: 0,
        };

        assert!(header.is_in_use());
        assert!(header.is_directory());
        assert!(header.is_base_record());
    }
}
