//! MFT Reader - Reads raw MFT records from an NTFS volume
//!
//! This module handles:
//! - Opening the volume with direct access
//! - Reading the boot sector
//! - Getting retrieval pointers for fragmented MFT
//! - Reading and parsing MFT records

use crate::ntfs::*;
use anyhow::{anyhow, Result};
use std::mem;

#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, ReadFile, SetFilePointerEx, FILE_BEGIN, FILE_FLAG_NO_BUFFERING,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::IO::DeviceIoControl,
        System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_GET_RETRIEVAL_POINTERS, NTFS_VOLUME_DATA_BUFFER},
    },
};

/// Represents an extent (contiguous run) of the MFT on disk
#[derive(Debug, Clone)]
pub struct MftExtent {
    /// Virtual Cluster Number (position within MFT)
    pub vcn: u64,
    /// Logical Cluster Number (position on disk)
    pub lcn: i64,
    /// Number of clusters in this extent
    pub cluster_count: u64,
}

/// Parsed MFT record information
#[derive(Debug, Clone)]
pub struct MftRecord {
    pub record_number: u64,
    pub sequence_number: u16,
    pub is_in_use: bool,
    pub is_directory: bool,
    pub parent_record_number: u64,
    pub parent_sequence_number: u16,
    pub file_name: String,
    pub file_size: i64,
    pub allocated_size: i64,
    pub creation_time: i64,
    pub modification_time: i64,
    pub access_time: i64,
    pub change_time: i64,
    pub file_attributes: u32,
    pub link_count: u16,
    pub is_base_record: bool,
}

impl Default for MftRecord {
    fn default() -> Self {
        Self {
            record_number: 0,
            sequence_number: 0,
            is_in_use: false,
            is_directory: false,
            parent_record_number: 0,
            parent_sequence_number: 0,
            file_name: String::new(),
            file_size: 0,
            allocated_size: 0,
            creation_time: 0,
            modification_time: 0,
            access_time: 0,
            change_time: 0,
            file_attributes: 0,
            link_count: 0,
            is_base_record: true,
        }
    }
}

/// MFT Reader for reading raw MFT records from an NTFS volume
pub struct MftReader {
    #[cfg(windows)]
    volume_handle: HANDLE,
    #[cfg(not(windows))]
    _phantom: std::marker::PhantomData<()>,
    pub cluster_size: u32,
    pub mft_record_size: u32,
    pub mft_start_lcn: i64,
    pub mft_valid_data_length: u64,
    pub mft_extents: Vec<MftExtent>,
}

#[cfg(windows)]
impl MftReader {
    /// Open a volume for MFT reading (requires admin privileges)
    pub fn open(drive_letter: char) -> Result<Self> {
        let volume_path: Vec<u16> = format!("\\\\.\\{}:", drive_letter)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let volume_handle = unsafe {
            CreateFileW(
                PCWSTR(volume_path.as_ptr()),
                0x80000000, // GENERIC_READ
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_NO_BUFFERING,
                None,
            )?
        };

        if volume_handle == INVALID_HANDLE_VALUE {
            return Err(anyhow!("Failed to open volume {}:", drive_letter));
        }

        let mut reader = Self {
            volume_handle,
            cluster_size: 0,
            mft_record_size: 0,
            mft_start_lcn: 0,
            mft_valid_data_length: 0,
            mft_extents: Vec::new(),
        };

        reader.read_volume_data()?;
        reader.get_mft_extents(drive_letter)?;

        Ok(reader)
    }

    /// Read NTFS volume data to get MFT location and sizes
    fn read_volume_data(&mut self) -> Result<()> {
        let mut volume_data: NTFS_VOLUME_DATA_BUFFER = unsafe { mem::zeroed() };
        let mut bytes_returned: u32 = 0;

        let result = unsafe {
            DeviceIoControl(
                self.volume_handle,
                FSCTL_GET_NTFS_VOLUME_DATA,
                None,
                0,
                Some(&mut volume_data as *mut _ as *mut _),
                mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        if result.is_err() {
            return Err(anyhow!("Failed to get NTFS volume data"));
        }

        self.cluster_size = volume_data.BytesPerCluster;
        self.mft_record_size = volume_data.BytesPerFileRecordSegment;
        self.mft_start_lcn = volume_data.MftStartLcn;
        self.mft_valid_data_length = volume_data.MftValidDataLength as u64;

        Ok(())
    }

    /// Get MFT extents (retrieval pointers) to handle fragmented MFT
    fn get_mft_extents(&mut self, drive_letter: char) -> Result<()> {
        // Open $MFT file to get its extents
        let mft_path: Vec<u16> = format!("{}:\\$MFT", drive_letter)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mft_handle = unsafe {
            CreateFileW(
                PCWSTR(mft_path.as_ptr()),
                0, // No access needed, just getting extents
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )?
        };

        if mft_handle == INVALID_HANDLE_VALUE {
            // Fall back to using boot sector info for single extent
            self.mft_extents.push(MftExtent {
                vcn: 0,
                lcn: self.mft_start_lcn,
                cluster_count: self.mft_valid_data_length / self.cluster_size as u64,
            });
            return Ok(());
        }

        // Get retrieval pointers with proper ERROR_MORE_DATA handling
        //
        // IMPORTANT: ERROR_MORE_DATA (234) means the buffer is too small.
        // The correct behavior is to:
        //   1. Increase buffer size
        //   2. Retry with the SAME StartingVcn (NOT advance it!)
        //   3. Do NOT parse partial results
        //
        // See: UltraFastFileSearch-code/file.cpp lines 1517-1522 for reference
        // See: mft-reader-rs/rust_team/MFT_INVESTIGATION_F_DRIVE.md Section 6

        #[repr(C)]
        struct StartingVcnInputBuffer {
            starting_vcn: i64,
        }

        // Retrieval pointers buffer header
        #[repr(C)]
        struct RetrievalPointersHeader {
            extent_count: u32,
            starting_vcn: i64,
        }

        // Each extent in the buffer
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct RetrievalPointersExtent {
            next_vcn: i64,
            lcn: i64,
        }

        const ERROR_MORE_DATA: u32 = 234;
        const ERROR_HANDLE_EOF: u32 = 38;

        // Start with a reasonable buffer size, will grow if needed
        let mut buffer_extent_count: usize = 64;
        let mut success = false;

        let input = StartingVcnInputBuffer { starting_vcn: 0 };

        // Loop until we have a large enough buffer
        while !success {
            // Calculate buffer size: header + extents
            let header_size = mem::size_of::<RetrievalPointersHeader>();
            let extents_size = buffer_extent_count * mem::size_of::<RetrievalPointersExtent>();
            let buffer_size = header_size + extents_size;

            // Allocate buffer
            let mut buffer: Vec<u8> = vec![0u8; buffer_size];
            let mut bytes_returned: u32 = 0;

            let result = unsafe {
                DeviceIoControl(
                    mft_handle,
                    FSCTL_GET_RETRIEVAL_POINTERS,
                    Some(&input as *const _ as *const _),
                    mem::size_of::<StartingVcnInputBuffer>() as u32,
                    Some(buffer.as_mut_ptr() as *mut _),
                    buffer_size as u32,
                    Some(&mut bytes_returned),
                    None,
                )
            };

            if result.is_ok() {
                // Success! Parse the extents
                success = true;

                let header = unsafe { &*(buffer.as_ptr() as *const RetrievalPointersHeader) };
                let extents_ptr = unsafe {
                    buffer.as_ptr().add(header_size) as *const RetrievalPointersExtent
                };

                let mut prev_vcn = header.starting_vcn as u64;
                for i in 0..header.extent_count as usize {
                    let extent = unsafe { &*extents_ptr.add(i) };
                    let cluster_count = extent.next_vcn as u64 - prev_vcn;

                    if extent.lcn >= 0 {
                        self.mft_extents.push(MftExtent {
                            vcn: prev_vcn,
                            lcn: extent.lcn,
                            cluster_count,
                        });
                    }
                    prev_vcn = extent.next_vcn as u64;
                }
            } else {
                // Check the error code
                let error_code = unsafe { windows::Win32::Foundation::GetLastError().0 };

                if error_code == ERROR_MORE_DATA {
                    // Buffer too small - double it and retry with SAME StartingVcn
                    buffer_extent_count *= 2;

                    // Safety limit to prevent infinite loop
                    if buffer_extent_count > 1_000_000 {
                        break;
                    }
                    // Continue loop to retry
                } else if error_code == ERROR_HANDLE_EOF {
                    // No more extents - we're done (shouldn't happen on first call)
                    success = true;
                } else {
                    // Other error - break out
                    break;
                }
            }
        }

        unsafe { CloseHandle(mft_handle).ok() };

        // If we didn't get any extents, fall back to single extent from volume data
        if self.mft_extents.is_empty() {
            self.mft_extents.push(MftExtent {
                vcn: 0,
                lcn: self.mft_start_lcn,
                cluster_count: self.mft_valid_data_length / self.cluster_size as u64,
            });
        }

        Ok(())
    }

    /// Read raw bytes from the volume at a specific byte offset
    fn read_at(&self, offset: i64, buffer: &mut [u8]) -> Result<usize> {
        unsafe {
            SetFilePointerEx(self.volume_handle, offset, None, FILE_BEGIN)?;
        }

        let mut bytes_read: u32 = 0;
        unsafe {
            ReadFile(
                self.volume_handle,
                Some(buffer),
                Some(&mut bytes_read),
                None,
            )?;
        }

        Ok(bytes_read as usize)
    }

    /// Calculate the total number of MFT records
    pub fn record_count(&self) -> u64 {
        self.mft_valid_data_length / self.mft_record_size as u64
    }

    /// Read all MFT records and return parsed information
    pub fn read_all_records(&self) -> Result<Vec<MftRecord>> {
        let record_count = self.record_count();
        let record_size = self.mft_record_size as usize;
        let cluster_size = self.cluster_size as usize;

        // Align buffer to cluster size for FILE_FLAG_NO_BUFFERING
        let buffer_size = ((record_size + cluster_size - 1) / cluster_size) * cluster_size;
        let mut buffer = vec![0u8; buffer_size];
        let mut records = Vec::with_capacity(record_count as usize);

        println!("Reading {} MFT records...", record_count);

        for record_num in 0..record_count {
            // Find which extent contains this record
            let record_vcn = (record_num * record_size as u64) / cluster_size as u64;
            let record_offset_in_cluster = (record_num * record_size as u64) % cluster_size as u64;

            let mut disk_offset: Option<i64> = None;
            for extent in &self.mft_extents {
                let extent_end_vcn = extent.vcn + extent.cluster_count;
                if record_vcn >= extent.vcn && record_vcn < extent_end_vcn {
                    let vcn_offset = record_vcn - extent.vcn;
                    disk_offset = Some(
                        (extent.lcn as u64 + vcn_offset) as i64 * cluster_size as i64
                            + record_offset_in_cluster as i64,
                    );
                    break;
                }
            }

            let offset = match disk_offset {
                Some(o) => o,
                None => continue, // Record not in any extent
            };

            // Align read offset to cluster boundary
            let aligned_offset = (offset / cluster_size as i64) * cluster_size as i64;
            let offset_adjustment = (offset - aligned_offset) as usize;

            if self.read_at(aligned_offset, &mut buffer).is_err() {
                continue;
            }

            // Parse the record
            if let Some(record) = self.parse_record(record_num, &mut buffer[offset_adjustment..]) {
                records.push(record);
            }

            if record_num % 100000 == 0 && record_num > 0 {
                println!("  Processed {} records...", record_num);
            }
        }

        println!("Finished reading {} records", records.len());
        Ok(records)
    }

    /// Parse a single MFT record from a buffer
    fn parse_record(&self, record_num: u64, buffer: &mut [u8]) -> Option<MftRecord> {
        let record_size = self.mft_record_size as usize;
        if buffer.len() < record_size {
            return None;
        }

        // Check magic number
        let header = unsafe { &mut *(buffer.as_mut_ptr() as *mut FileRecordSegmentHeader) };
        if header.multi_sector_header.magic != MultiSectorHeader::FILE_MAGIC {
            return None;
        }

        // Apply USA unfixup
        let unfixup_ok = unsafe {
            header
                .multi_sector_header
                .unfixup(buffer.as_mut_ptr(), record_size)
        };

        if !unfixup_ok {
            // Record is corrupted
            return None;
        }

        let mut record = MftRecord {
            record_number: record_num,
            sequence_number: header.sequence_number,
            is_in_use: header.is_in_use(),
            is_directory: header.is_directory(),
            link_count: header.link_count,
            is_base_record: header.base_file_record_segment == 0,
            ..Default::default()
        };

        // Parse attributes
        let first_attr_offset = header.first_attribute_offset as usize;
        let bytes_in_use = header.bytes_in_use as usize;

        if first_attr_offset >= record_size || first_attr_offset < mem::size_of::<FileRecordSegmentHeader>() {
            return Some(record);
        }

        let mut offset = first_attr_offset;
        while offset + mem::size_of::<AttributeRecordHeader>() <= bytes_in_use && offset < record_size {
            let attr_header = unsafe {
                &*(buffer.as_ptr().add(offset) as *const AttributeRecordHeader)
            };

            // Check for end marker
            if attr_header.type_code == 0xFFFFFFFF || attr_header.length == 0 {
                break;
            }

            // Validate attribute length
            if attr_header.length as usize > record_size - offset {
                break;
            }

            // Parse filename attribute
            if attr_header.type_code == AttributeTypeCode::FileName as u32
                && attr_header.is_non_resident == 0
            {
                self.parse_filename_attribute(buffer, offset, attr_header, &mut record);
            }

            // Parse standard information attribute
            if attr_header.type_code == AttributeTypeCode::StandardInformation as u32
                && attr_header.is_non_resident == 0
            {
                self.parse_standard_info_attribute(buffer, offset, attr_header, &mut record);
            }

            // Parse data attribute for file size
            if attr_header.type_code == AttributeTypeCode::Data as u32 {
                self.parse_data_attribute(buffer, offset, attr_header, &mut record);
            }

            offset += attr_header.length as usize;
        }

        Some(record)
    }

    /// Parse a $FILE_NAME attribute
    fn parse_filename_attribute(
        &self,
        buffer: &[u8],
        attr_offset: usize,
        attr_header: &AttributeRecordHeader,
        record: &mut MftRecord,
    ) {
        let resident_offset = attr_offset + mem::size_of::<AttributeRecordHeader>();
        if resident_offset + mem::size_of::<ResidentAttributeData>() > buffer.len() {
            return;
        }

        let resident = unsafe {
            &*(buffer.as_ptr().add(resident_offset) as *const ResidentAttributeData)
        };

        let value_offset = attr_offset + resident.value_offset as usize;
        if value_offset + mem::size_of::<FilenameInformation>() > buffer.len() {
            return;
        }

        let filename_info = unsafe {
            &*(buffer.as_ptr().add(value_offset) as *const FilenameInformation)
        };

        // Skip DOS-only names (8.3 short names) if we already have a name
        if filename_info.is_dos_only() && !record.file_name.is_empty() {
            return;
        }

        // Extract filename (UTF-16LE)
        let name_offset = value_offset + mem::size_of::<FilenameInformation>() - 2; // -2 for FileName[1]
        let name_len = filename_info.file_name_length as usize;
        let name_bytes = name_len * 2;

        if name_offset + name_bytes > buffer.len() {
            return;
        }

        let name_slice = &buffer[name_offset..name_offset + name_bytes];
        let name_u16: Vec<u16> = name_slice
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        record.file_name = String::from_utf16_lossy(&name_u16);
        record.parent_record_number = filename_info.parent_frs();
        record.parent_sequence_number = filename_info.parent_sequence();
        record.creation_time = filename_info.creation_time;
        record.modification_time = filename_info.last_modification_time;
        record.access_time = filename_info.last_access_time;
        record.change_time = filename_info.last_change_time;
        record.file_attributes = filename_info.file_attributes;

        // Use filename's file size if we don't have one from $DATA
        if record.file_size == 0 {
            record.file_size = filename_info.file_size;
            record.allocated_size = filename_info.allocated_length;
        }
    }

    /// Parse a $STANDARD_INFORMATION attribute
    fn parse_standard_info_attribute(
        &self,
        buffer: &[u8],
        attr_offset: usize,
        _attr_header: &AttributeRecordHeader,
        record: &mut MftRecord,
    ) {
        let resident_offset = attr_offset + mem::size_of::<AttributeRecordHeader>();
        if resident_offset + mem::size_of::<ResidentAttributeData>() > buffer.len() {
            return;
        }

        let resident = unsafe {
            &*(buffer.as_ptr().add(resident_offset) as *const ResidentAttributeData)
        };

        let value_offset = attr_offset + resident.value_offset as usize;
        if value_offset + mem::size_of::<StandardInformation>() > buffer.len() {
            return;
        }

        let std_info = unsafe {
            &*(buffer.as_ptr().add(value_offset) as *const StandardInformation)
        };

        // Standard info has more accurate timestamps
        record.creation_time = std_info.creation_time;
        record.modification_time = std_info.last_modification_time;
        record.access_time = std_info.last_access_time;
        record.change_time = std_info.last_change_time;
        record.file_attributes = std_info.file_attributes;
    }

    /// Parse a $DATA attribute for file size
    fn parse_data_attribute(
        &self,
        buffer: &[u8],
        attr_offset: usize,
        attr_header: &AttributeRecordHeader,
        record: &mut MftRecord,
    ) {
        // Only process unnamed $DATA attribute (the main file data)
        if attr_header.name_length != 0 {
            return;
        }

        if attr_header.is_non_resident != 0 {
            // Non-resident: get size from non-resident header
            let nonres_offset = attr_offset + mem::size_of::<AttributeRecordHeader>();
            if nonres_offset + mem::size_of::<NonResidentAttributeData>() > buffer.len() {
                return;
            }

            let nonres = unsafe {
                &*(buffer.as_ptr().add(nonres_offset) as *const NonResidentAttributeData)
            };

            record.file_size = nonres.data_size;
            record.allocated_size = nonres.allocated_size;
        } else {
            // Resident: get size from resident header
            let resident_offset = attr_offset + mem::size_of::<AttributeRecordHeader>();
            if resident_offset + mem::size_of::<ResidentAttributeData>() > buffer.len() {
                return;
            }

            let resident = unsafe {
                &*(buffer.as_ptr().add(resident_offset) as *const ResidentAttributeData)
            };

            record.file_size = resident.value_length as i64;
            record.allocated_size = resident.value_length as i64;
        }
    }
}

#[cfg(windows)]
impl Drop for MftReader {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.volume_handle).ok();
        }
    }
}

// Non-Windows stub implementation
#[cfg(not(windows))]
impl MftReader {
    pub fn open(_drive_letter: char) -> Result<Self> {
        Err(anyhow!("MFT reading is only supported on Windows"))
    }

    pub fn record_count(&self) -> u64 {
        0
    }

    pub fn read_all_records(&self) -> Result<Vec<MftRecord>> {
        Err(anyhow!("MFT reading is only supported on Windows"))
    }
}
