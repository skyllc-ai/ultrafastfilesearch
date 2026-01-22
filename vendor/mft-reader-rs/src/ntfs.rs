//! NTFS on-disk structures
//!
//! These structures match the NTFS on-disk format exactly.
//! All structures use repr(C, packed) to ensure correct memory layout.

/// NTFS Boot Sector - Located at sector 0 of the volume
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NtfsBootSector {
    pub jump: [u8; 3],
    pub oem: [u8; 8],
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub padding1: [u8; 3],
    pub unused1: u16,
    pub media_descriptor: u8,
    pub padding2: u16,
    pub sectors_per_track: u16,
    pub number_of_heads: u16,
    pub hidden_sectors: u32,
    pub unused2: u32,
    pub unused3: u32,
    pub total_sectors: i64,
    pub mft_start_lcn: i64,
    pub mft2_start_lcn: i64,
    pub clusters_per_file_record_segment: i8,
    pub padding3: [u8; 3],
    pub clusters_per_index_block: u32,
    pub volume_serial_number: i64,
    pub checksum: u32,
    pub bootstrap: [u8; 426],
}

impl NtfsBootSector {
    /// Calculate the file record size in bytes
    pub fn file_record_size(&self) -> u32 {
        if self.clusters_per_file_record_segment >= 0 {
            self.clusters_per_file_record_segment as u32
                * self.sectors_per_cluster as u32
                * self.bytes_per_sector as u32
        } else {
            1u32 << (-self.clusters_per_file_record_segment as u32)
        }
    }

    /// Calculate the cluster size in bytes
    pub fn cluster_size(&self) -> u32 {
        self.sectors_per_cluster as u32 * self.bytes_per_sector as u32
    }
}

/// Multi-Sector Header - Used for USA (Update Sequence Array) protection
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MultiSectorHeader {
    pub magic: u32,
    pub usa_offset: u16,
    pub usa_count: u16,
}

impl MultiSectorHeader {
    /// Magic number for FILE records
    pub const FILE_MAGIC: u32 = 0x454C4946; // 'FILE'
    /// Magic number for corrupted records
    pub const BAAD_MAGIC: u32 = 0x44414142; // 'BAAD'

    /// Apply USA unfixup to restore original sector end bytes
    /// Returns true if the record is valid, false if corrupted
    pub unsafe fn unfixup(&mut self, buffer: *mut u8, max_size: usize) -> bool {
        let usa_offset = self.usa_offset as usize;
        let usa_count = self.usa_count as usize;

        if usa_offset + usa_count * 2 > max_size {
            return false;
        }

        let usa = buffer.add(usa_offset) as *const u16;
        let usa0 = *usa;
        let mut result = true;

        for i in 1..usa_count {
            let offset = i * 512 - 2;
            if offset < max_size {
                let check = buffer.add(offset) as *mut u16;
                result &= *check == usa0;
                *check = *usa.add(i);
            } else {
                break;
            }
        }

        result
    }
}

/// Attribute Type Codes
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeTypeCode {
    StandardInformation = 0x10,
    AttributeList = 0x20,
    FileName = 0x30,
    ObjectId = 0x40,
    SecurityDescriptor = 0x50,
    VolumeName = 0x60,
    VolumeInformation = 0x70,
    Data = 0x80,
    IndexRoot = 0x90,
    IndexAllocation = 0xA0,
    Bitmap = 0xB0,
    ReparsePoint = 0xC0,
    EaInformation = 0xD0,
    Ea = 0xE0,
    PropertySet = 0xF0,
    LoggedUtilityStream = 0x100,
    End = 0xFFFFFFFF,
}

impl AttributeTypeCode {
    pub fn from_u32(value: u32) -> Option<Self> {
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
            0xFFFFFFFF => Some(Self::End),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::StandardInformation => "$STANDARD_INFORMATION",
            Self::AttributeList => "$ATTRIBUTE_LIST",
            Self::FileName => "$FILE_NAME",
            Self::ObjectId => "$OBJECT_ID",
            Self::SecurityDescriptor => "$SECURITY_DESCRIPTOR",
            Self::VolumeName => "$VOLUME_NAME",
            Self::VolumeInformation => "$VOLUME_INFORMATION",
            Self::Data => "$DATA",
            Self::IndexRoot => "$INDEX_ROOT",
            Self::IndexAllocation => "$INDEX_ALLOCATION",
            Self::Bitmap => "$BITMAP",
            Self::ReparsePoint => "$REPARSE_POINT",
            Self::EaInformation => "$EA_INFORMATION",
            Self::Ea => "$EA",
            Self::PropertySet => "$PROPERTY_SET",
            Self::LoggedUtilityStream => "$LOGGED_UTILITY_STREAM",
            Self::End => "$END",
        }
    }
}

/// File Record Header Flags
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRecordFlags {
    InUse = 0x0001,
    Directory = 0x0002,
}

/// File Record Segment Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FileRecordSegmentHeader {
    pub multi_sector_header: MultiSectorHeader,
    pub log_file_sequence_number: u64,
    pub sequence_number: u16,
    pub link_count: u16,
    pub first_attribute_offset: u16,
    pub flags: u16,
    pub bytes_in_use: u32,
    pub bytes_allocated: u32,
    pub base_file_record_segment: u64,
    pub next_attribute_number: u16,
    pub segment_number_upper: u16,
    pub segment_number_lower: u32,
}

impl FileRecordSegmentHeader {
    /// Check if this record is in use
    pub fn is_in_use(&self) -> bool {
        (self.flags & FileRecordFlags::InUse as u16) != 0
    }

    /// Check if this record is a directory
    pub fn is_directory(&self) -> bool {
        (self.flags & FileRecordFlags::Directory as u16) != 0
    }

    /// Get the full segment number (FRS index)
    pub fn segment_number(&self) -> u64 {
        ((self.segment_number_upper as u64) << 32) | (self.segment_number_lower as u64)
    }
}

/// Attribute Record Header (common part)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AttributeRecordHeader {
    pub type_code: u32,
    pub length: u32,
    pub is_non_resident: u8,
    pub name_length: u8,
    pub name_offset: u16,
    pub flags: u16,
    pub instance: u16,
}

/// Resident Attribute Data
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ResidentAttributeData {
    pub value_length: u32,
    pub value_offset: u16,
    pub flags: u16,
}

/// Non-Resident Attribute Data
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NonResidentAttributeData {
    pub lowest_vcn: i64,
    pub highest_vcn: i64,
    pub mapping_pairs_offset: u16,
    pub compression_unit: u8,
    pub reserved: [u8; 5],
    pub allocated_size: i64,
    pub data_size: i64,
    pub initialized_size: i64,
}

/// Filename Information Attribute
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FilenameInformation {
    pub parent_directory: u64,
    pub creation_time: i64,
    pub last_modification_time: i64,
    pub last_change_time: i64,
    pub last_access_time: i64,
    pub allocated_length: i64,
    pub file_size: i64,
    pub file_attributes: u32,
    pub packed_ea_size: u16,
    pub reserved: u16,
    pub file_name_length: u8,
    pub flags: u8,
    // Followed by variable-length filename (UTF-16LE)
}

/// Filename namespace flags
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilenameNamespace {
    Posix = 0x00,
    Win32 = 0x01,
    Dos = 0x02,
    Win32AndDos = 0x03,
}

impl FilenameInformation {
    /// Get the parent FRS index (lower 48 bits)
    pub fn parent_frs(&self) -> u64 {
        self.parent_directory & 0x0000_FFFF_FFFF_FFFF
    }

    /// Get the parent sequence number (upper 16 bits)
    pub fn parent_sequence(&self) -> u16 {
        ((self.parent_directory >> 48) & 0xFFFF) as u16
    }

    /// Check if this is a DOS-only name (8.3 short name)
    pub fn is_dos_only(&self) -> bool {
        self.flags == FilenameNamespace::Dos as u8
    }
}

/// Standard Information Attribute
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct StandardInformation {
    pub creation_time: i64,
    pub last_modification_time: i64,
    pub last_change_time: i64,
    pub last_access_time: i64,
    pub file_attributes: u32,
    // Additional fields exist in NTFS 3.0+
}

