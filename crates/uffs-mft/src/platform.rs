//! Platform-specific implementations for Windows.
//!
//! This module provides Windows API wrappers for:
//! - Volume handle management
//! - NTFS volume data retrieval
//! - Privilege checking
//!
//! # Safety
//!
//! This module uses Windows FFI and requires careful handling of raw handles.

#![cfg(windows)]

use std::mem::size_of;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_FLAG_SEQUENTIAL_SCAN, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    SYNCHRONIZE,
};

/// FILE_READ_DATA access right (0x0001) - required to read data from a
/// file/volume
const FILE_READ_DATA: u32 = 0x0001;
use windows::Win32::System::Ioctl::{
    FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_GET_RETRIEVAL_POINTERS, NTFS_VOLUME_DATA_BUFFER,
    STARTING_VCN_INPUT_BUFFER,
};
use windows::core::PCWSTR;

use crate::error::{MftError, Result};
use crate::ntfs::NtfsBootSector;

// ============================================================================
// Volume Handle
// ============================================================================

/// A handle to an NTFS volume for direct disk access.
///
/// This handle is opened with backup semantics and no buffering for
/// optimal MFT reading performance.
#[derive(Debug)]
pub struct VolumeHandle {
    /// The raw Windows handle.
    handle: HANDLE,
    /// The volume letter.
    volume: char,
    /// NTFS volume data from `FSCTL_GET_NTFS_VOLUME_DATA`.
    volume_data: NtfsVolumeData,
}

// SAFETY: Windows file handles are thread-safe. The HANDLE is just a pointer
// to a kernel object that the OS manages. Multiple threads can safely read
// from the same handle (though we don't do that - each task has its own
// handle). This is required for tokio::spawn to work with MftReader.
#[allow(unsafe_code)]
unsafe impl Send for VolumeHandle {}
#[allow(unsafe_code)]
unsafe impl Sync for VolumeHandle {}

/// NTFS volume data retrieved from `FSCTL_GET_NTFS_VOLUME_DATA`.
#[derive(Debug, Clone, Copy)]
pub struct NtfsVolumeData {
    /// Volume serial number.
    pub volume_serial_number: u64,
    /// Number of sectors on the volume.
    pub number_of_sectors: u64,
    /// Total number of clusters.
    pub total_clusters: u64,
    /// Number of free clusters.
    pub free_clusters: u64,
    /// Total number of reserved clusters.
    pub total_reserved: u64,
    /// Bytes per sector.
    pub bytes_per_sector: u32,
    /// Bytes per cluster.
    pub bytes_per_cluster: u32,
    /// Bytes per file record segment.
    pub bytes_per_file_record_segment: u32,
    /// Clusters per file record segment.
    pub clusters_per_file_record_segment: u32,
    /// MFT valid data length.
    pub mft_valid_data_length: u64,
    /// MFT start LCN (Logical Cluster Number).
    pub mft_start_lcn: u64,
    /// MFT2 start LCN.
    pub mft2_start_lcn: u64,
    /// MFT zone start.
    pub mft_zone_start: u64,
    /// MFT zone end.
    pub mft_zone_end: u64,
}

impl VolumeHandle {
    /// Opens a volume for direct MFT reading.
    ///
    /// # Arguments
    ///
    /// * `volume` - The drive letter (e.g., 'C', 'D')
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The volume cannot be opened (invalid letter, access denied)
    /// - The volume is not NTFS formatted
    /// - Insufficient privileges
    ///
    /// # Safety
    ///
    /// This function opens a raw volume handle which requires Administrator
    /// privileges.
    #[allow(unsafe_code)] // Required: Windows FFI (CreateFileW)
    pub fn open(volume: char) -> Result<Self> {
        let volume = volume.to_ascii_uppercase();

        // Validate volume letter
        if !volume.is_ascii_alphabetic() {
            return Err(MftError::VolumeOpen {
                volume,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid volume letter",
                ),
            });
        }

        // Create volume path: \\.\X:
        let volume_path: Vec<u16> = format!("\\\\.\\{}:", volume)
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // Open the volume with FILE_READ_DATA for raw disk access
        // Match C++ flags: FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE
        // This is required to read the MFT bitmap from physical cluster locations
        //
        // C++ team insight: Do NOT use FILE_FLAG_NO_BUFFERING!
        // - NO_BUFFERING disables OS cache and read-ahead
        // - SEQUENTIAL_SCAN optimizes cache for sequential access
        // - These two flags work against each other
        // - Let the OS cache + read-ahead do its job
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(volume_path.as_ptr()),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0, // Access mode
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,  // Share mode
                None,                                                    // Security attributes
                OPEN_EXISTING,                                           // Creation disposition
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_SEQUENTIAL_SCAN, // Flags (no NO_BUFFERING!)
                None,                                                   // Template file
            )
        };

        let handle = match handle {
            Ok(h) => h,
            Err(err) => {
                // Check for access denied
                if err.code().0 as u32 == 0x8007_0005 {
                    return Err(MftError::InsufficientPrivileges);
                }
                return Err(MftError::VolumeOpen {
                    volume,
                    source: std::io::Error::from_raw_os_error(err.code().0 as i32),
                });
            }
        };

        // Get NTFS volume data
        let volume_data = Self::get_ntfs_volume_data(handle, volume)?;

        Ok(Self {
            handle,
            volume,
            volume_data,
        })
    }

    /// Retrieves NTFS volume data using `FSCTL_GET_NTFS_VOLUME_DATA`.
    #[allow(unsafe_code)] // Required: Windows FFI (DeviceIoControl)
    fn get_ntfs_volume_data(handle: HANDLE, volume: char) -> Result<NtfsVolumeData> {
        use windows::Win32::System::IO::DeviceIoControl;

        let mut buffer = NTFS_VOLUME_DATA_BUFFER::default();
        let mut bytes_returned: u32 = 0;

        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_GET_NTFS_VOLUME_DATA,
                None,                                          // Input buffer
                0,                                             // Input buffer size
                Some(core::ptr::from_mut(&mut buffer).cast()), // Output buffer
                size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,   // Output buffer size
                Some(&mut bytes_returned),                     // Bytes returned
                None,                                          // Overlapped
            )
        };

        if result.is_err() {
            // Volume might not be NTFS
            return Err(MftError::NotNtfs(volume));
        }

        Ok(NtfsVolumeData {
            volume_serial_number: buffer.VolumeSerialNumber as u64,
            number_of_sectors: buffer.NumberSectors as u64,
            total_clusters: buffer.TotalClusters as u64,
            free_clusters: buffer.FreeClusters as u64,
            total_reserved: buffer.TotalReserved as u64,
            bytes_per_sector: buffer.BytesPerSector,
            bytes_per_cluster: buffer.BytesPerCluster,
            bytes_per_file_record_segment: buffer.BytesPerFileRecordSegment,
            clusters_per_file_record_segment: buffer.ClustersPerFileRecordSegment,
            mft_valid_data_length: buffer.MftValidDataLength as u64,
            mft_start_lcn: buffer.MftStartLcn as u64,
            mft2_start_lcn: buffer.Mft2StartLcn as u64,
            mft_zone_start: buffer.MftZoneStart as u64,
            mft_zone_end: buffer.MftZoneEnd as u64,
        })
    }

    /// Returns the volume letter.
    #[must_use]
    pub const fn volume(&self) -> char {
        self.volume
    }

    /// Returns the NTFS volume data.
    #[must_use]
    pub const fn volume_data(&self) -> &NtfsVolumeData {
        &self.volume_data
    }

    /// Returns the raw Windows handle.
    ///
    /// # Safety
    ///
    /// The caller must not close this handle or use it after the
    /// `VolumeHandle` is dropped.
    #[must_use]
    pub const fn raw_handle(&self) -> HANDLE {
        self.handle
    }

    /// Opens a new handle to the same volume with `FILE_FLAG_OVERLAPPED`.
    ///
    /// This is required for IOCP (I/O Completion Port) operations which need
    /// overlapped I/O support. The returned handle is separate from the main
    /// handle and must be closed by the caller.
    ///
    /// # Errors
    ///
    /// Returns an error if the volume cannot be opened.
    #[allow(unsafe_code)] // Required: Windows FFI (CreateFileW)
    pub fn open_overlapped_handle(&self) -> Result<HANDLE> {
        use windows::core::PCWSTR;

        let volume_path: Vec<u16> = format!("\\\\.\\{}:", self.volume)
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // FILE_FLAG_SEQUENTIAL_SCAN enables aggressive OS read-ahead
        // Do NOT use FILE_FLAG_NO_BUFFERING - it disables OS cache and read-ahead
        // which works against SEQUENTIAL_SCAN (C++ team insight)
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(volume_path.as_ptr()),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED | FILE_FLAG_SEQUENTIAL_SCAN,
                None,
            )
        };

        match handle {
            Ok(h) => Ok(h),
            Err(err) => Err(MftError::VolumeOpen {
                volume: self.volume,
                source: std::io::Error::from_raw_os_error(err.code().0 as i32),
            }),
        }
    }

    /// Returns the byte offset of the MFT on the volume.
    #[must_use]
    pub fn mft_byte_offset(&self) -> u64 {
        self.volume_data.mft_start_lcn * u64::from(self.volume_data.bytes_per_cluster)
    }

    /// Returns the size of a file record segment in bytes.
    #[must_use]
    pub const fn file_record_size(&self) -> u32 {
        self.volume_data.bytes_per_file_record_segment
    }

    /// Returns the estimated number of MFT records.
    #[must_use]
    pub fn estimated_record_count(&self) -> u64 {
        self.volume_data.mft_valid_data_length
            / u64::from(self.volume_data.bytes_per_file_record_segment)
    }

    /// Reads the boot sector from the volume.
    ///
    /// # Errors
    ///
    /// Returns an error if the boot sector cannot be read.
    #[allow(unsafe_code)] // Required: Windows FFI and ptr::read for packed struct
    pub fn read_boot_sector(&self) -> Result<NtfsBootSector> {
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

        // Seek to the beginning of the volume
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(self.handle, 0, Some(&mut new_position), FILE_BEGIN)?;
        }

        // Read the boot sector
        let mut buffer = [0_u8; 512];
        let mut bytes_read = 0_u32;

        unsafe {
            ReadFile(self.handle, Some(&mut buffer), Some(&mut bytes_read), None)?;
        }

        if bytes_read != 512 {
            return Err(MftError::BootSectorRead(format!(
                "Expected 512 bytes, got {}",
                bytes_read
            )));
        }

        // SAFETY: NtfsBootSector is repr(C, packed) and exactly 512 bytes
        let boot_sector: NtfsBootSector = unsafe { core::ptr::read(buffer.as_ptr().cast()) };

        if !boot_sector.is_valid() {
            return Err(MftError::InvalidBootSector(
                "Invalid OEM ID (not NTFS)".to_owned(),
            ));
        }

        Ok(boot_sector)
    }

    /// Gets the MFT extents (data runs) using `FSCTL_GET_RETRIEVAL_POINTERS`.
    ///
    /// This is essential for reading fragmented MFT files. Returns a list of
    /// (VCN, cluster_count, LCN) tuples representing the physical layout.
    ///
    /// # Errors
    ///
    /// Returns an error if the MFT file cannot be opened or the extents
    /// cannot be retrieved.
    #[allow(unsafe_code)] // Required: Windows FFI (CreateFileW, DeviceIoControl, CloseHandle)
    pub fn get_mft_extents(&self) -> Result<Vec<MftExtent>> {
        // Open the $MFT file
        // Use path format "F:\$MFT" (not "\\.\F:\$MFT") to match C++ behavior
        let mft_path: Vec<u16> = format!("{}:\\$MFT", self.volume)
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        let mft_handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(mft_path.as_ptr()),
                0, // No access needed, just getting extents (matches C++)
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0), // No flags (matches C++)
                None,
            )
        };

        let mft_handle = match mft_handle {
            Ok(h) => h,
            Err(_err) => {
                // Fall back to using volume data if we can't open $MFT directly
                // This happens on some systems where direct $MFT access is restricted
                return Ok(vec![MftExtent {
                    vcn: 0,
                    cluster_count: self.volume_data.mft_valid_data_length
                        / u64::from(self.volume_data.bytes_per_cluster),
                    lcn: self.volume_data.mft_start_lcn as i64,
                }]);
            }
        };

        // Use RAII guard for handle cleanup
        let _guard = HandleGuard(mft_handle);

        // Get retrieval pointers
        get_retrieval_pointers(mft_handle)
    }

    /// Gets the MFT bitmap which indicates which records are in use.
    ///
    /// The bitmap is read from `$MFT::$BITMAP`. Each bit corresponds to one
    /// MFT record - if the bit is set, the record is in use.
    ///
    /// # Returns
    ///
    /// Returns `MftBitmap` containing the bitmap data and helper methods.
    ///
    /// # Errors
    ///
    /// Returns an error if the bitmap cannot be read.
    #[allow(unsafe_code)] // Required: Windows FFI for CreateFileW, GetFileSizeEx, ReadFile
    pub fn get_mft_bitmap(&self) -> Result<MftBitmap> {
        self.get_mft_bitmap_internal(false)
    }

    /// Gets the MFT bitmap with optional verbose diagnostic output.
    #[allow(unsafe_code)] // Required: Windows FFI for CreateFileW, GetFileSizeEx, ReadFile
    pub fn get_mft_bitmap_verbose(&self) -> Result<MftBitmap> {
        self.get_mft_bitmap_internal(true)
    }

    #[allow(unsafe_code)] // Required: Windows FFI for CreateFileW, GetFileSizeEx, ReadFile
    fn get_mft_bitmap_internal(&self, verbose: bool) -> Result<MftBitmap> {
        use windows::Win32::Storage::FileSystem::{
            FILE_BEGIN, GetFileSizeEx, ReadFile, SYNCHRONIZE, SetFilePointerEx,
        };

        // Open the $MFT::$BITMAP stream to get retrieval pointers and size
        // Use same path format as C++: "C:\$MFT::$BITMAP"
        let bitmap_path_str = format!("{}:\\$MFT::$BITMAP", self.volume);
        let bitmap_path: Vec<u16> = bitmap_path_str
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        if verbose {
            eprintln!("[BITMAP] Opening: {}", bitmap_path_str);
        }

        // Match C++ flags: FILE_READ_ATTRIBUTES | SYNCHRONIZE
        let bitmap_handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(bitmap_path.as_ptr()),
                FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_NO_BUFFERING,
                None,
            )
        };

        let bitmap_handle = match bitmap_handle {
            Ok(h) => {
                if verbose {
                    eprintln!("[BITMAP] CreateFileW succeeded: {:?}", h);
                }
                h
            }
            Err(e) => {
                if verbose {
                    eprintln!("[BITMAP] CreateFileW FAILED: {:?}", e);
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
        };

        let _guard = HandleGuard(bitmap_handle);

        // Get file size
        let mut file_size: i64 = 0;
        unsafe {
            if let Err(e) = GetFileSizeEx(bitmap_handle, &mut file_size) {
                if verbose {
                    eprintln!("[BITMAP] GetFileSizeEx FAILED: {:?}", e);
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
        }

        if verbose {
            eprintln!("[BITMAP] File size: {} bytes", file_size);
        }

        // Get retrieval pointers for the bitmap file
        let extents = match get_retrieval_pointers(bitmap_handle) {
            Ok(e) if !e.is_empty() => {
                if verbose {
                    eprintln!("[BITMAP] Got {} extents:", e.len());
                    for (i, ext) in e.iter().enumerate().take(5) {
                        eprintln!(
                            "   [{}] VCN={}, clusters={}, LCN={}",
                            i, ext.vcn, ext.cluster_count, ext.lcn
                        );
                    }
                    if e.len() > 5 {
                        eprintln!("   ... and {} more", e.len() - 5);
                    }
                }
                e
            }
            Ok(_) => {
                if verbose {
                    eprintln!("[BITMAP] get_retrieval_pointers returned empty!");
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
            Err(e) => {
                if verbose {
                    eprintln!("[BITMAP] get_retrieval_pointers FAILED: {:?}", e);
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
        };

        // Read the bitmap data from the volume at the physical cluster locations
        // With FILE_FLAG_NO_BUFFERING, reads must be sector-aligned.
        // The C++ code reads full clusters (cluster_count × cluster_size), then
        // truncates.
        let bytes_per_cluster = self.volume_data.bytes_per_cluster;

        // Calculate total cluster-aligned size to read
        let total_clusters: u64 = extents.iter().map(|e| e.cluster_count).sum();
        let aligned_size = (total_clusters * u64::from(bytes_per_cluster)) as usize;
        let mut buffer = vec![0u8; aligned_size];
        let mut buffer_offset = 0usize;

        if verbose {
            eprintln!(
                "[BITMAP] Reading from volume, bytes_per_cluster={}, file_size={}, aligned_size={}",
                bytes_per_cluster, file_size, aligned_size
            );
        }

        for (i, extent) in extents.iter().enumerate() {
            let byte_offset = extent.lcn * i64::from(bytes_per_cluster);
            // Read full clusters (always aligned)
            let extent_bytes = (extent.cluster_count * u64::from(bytes_per_cluster)) as usize;

            if extent_bytes == 0 {
                continue;
            }

            if verbose && i < 3 {
                eprintln!(
                    "[BITMAP] Extent {}: seek to offset {}, read {} bytes (full clusters)",
                    i, byte_offset, extent_bytes
                );
            }

            // Seek to the extent's physical location on the volume
            let mut new_position = 0_i64;
            unsafe {
                if let Err(e) = SetFilePointerEx(
                    self.handle,
                    byte_offset,
                    Some(&mut new_position),
                    FILE_BEGIN,
                ) {
                    if verbose {
                        eprintln!("[BITMAP] SetFilePointerEx FAILED: {:?}", e);
                    }
                    return Ok(MftBitmap::new_all_valid(
                        self.estimated_record_count() as usize
                    ));
                }
            }

            // Read the extent data from the volume (full clusters)
            let mut bytes_read: u32 = 0;
            unsafe {
                if let Err(e) = ReadFile(
                    self.handle,
                    Some(&mut buffer[buffer_offset..buffer_offset + extent_bytes]),
                    Some(&mut bytes_read),
                    None,
                ) {
                    if verbose {
                        eprintln!("[BITMAP] ReadFile FAILED: {:?}", e);
                    }
                    return Ok(MftBitmap::new_all_valid(
                        self.estimated_record_count() as usize
                    ));
                }
            }

            if verbose && i < 3 {
                eprintln!("[BITMAP] Read {} bytes from extent {}", bytes_read, i);
                if i == 0 && bytes_read > 0 {
                    let sample: Vec<String> = buffer
                        [buffer_offset..buffer_offset + 32.min(bytes_read as usize)]
                        .iter()
                        .map(|b| format!("{:02X}", b))
                        .collect();
                    eprintln!("[BITMAP] First 32 bytes: {}", sample.join(" "));
                }
            }

            buffer_offset += bytes_read as usize;
        }

        if verbose {
            eprintln!(
                "[BITMAP] Total bytes read: {}, truncating to file_size: {}",
                buffer_offset, file_size
            );
            let all_ff = buffer.iter().take(file_size as usize).all(|&b| b == 0xFF);
            let all_00 = buffer.iter().take(file_size as usize).all(|&b| b == 0x00);
            eprintln!("[BITMAP] All 0xFF: {}, All 0x00: {}", all_ff, all_00);
        }

        // Truncate to actual file size (discard padding from cluster alignment)
        buffer.truncate(file_size as usize);
        Ok(MftBitmap::from_bytes(buffer))
    }
}

// ============================================================================
// Handle Guard (RAII)
// ============================================================================

/// RAII guard for Windows handles.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    #[allow(unsafe_code)] // Required: Windows FFI for CloseHandle
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// ============================================================================
// MFT Extent
// ============================================================================

/// Represents a contiguous extent of the MFT on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MftExtent {
    /// Virtual Cluster Number (offset within the file).
    pub vcn: u64,
    /// Number of clusters in this extent.
    pub cluster_count: u64,
    /// Logical Cluster Number (physical location on disk).
    /// Negative values indicate sparse/unallocated regions.
    pub lcn: i64,
}

impl MftExtent {
    /// Returns the byte offset of this extent on the volume.
    #[must_use]
    pub fn byte_offset(&self, bytes_per_cluster: u32) -> u64 {
        if self.lcn < 0 {
            0 // Sparse extent
        } else {
            self.lcn as u64 * u64::from(bytes_per_cluster)
        }
    }

    /// Returns the size of this extent in bytes.
    #[must_use]
    pub fn byte_size(&self, bytes_per_cluster: u32) -> u64 {
        self.cluster_count * u64::from(bytes_per_cluster)
    }
}

/// Retrieves the extent map for a file using `FSCTL_GET_RETRIEVAL_POINTERS`.
#[allow(unsafe_code)] // Required: Windows FFI (DeviceIoControl) and mem::zeroed
fn get_retrieval_pointers(handle: HANDLE) -> Result<Vec<MftExtent>> {
    use windows::Win32::System::IO::DeviceIoControl;

    let mut extents = Vec::new();
    // SAFETY: STARTING_VCN_INPUT_BUFFER is a simple struct with a single i64 field.
    // Zeroing it sets StartingVcn to 0, which is what we want.
    let starting_vcn: STARTING_VCN_INPUT_BUFFER = unsafe { std::mem::zeroed() };

    // Initial buffer size - will grow if needed
    let mut buffer_size = 64 * 1024; // 64KB initial
    let mut buffer: Vec<u8> = vec![0; buffer_size];

    loop {
        let mut bytes_returned: u32 = 0;

        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_GET_RETRIEVAL_POINTERS,
                Some(core::ptr::from_ref(&starting_vcn).cast()),
                size_of::<STARTING_VCN_INPUT_BUFFER>() as u32,
                Some(buffer.as_mut_ptr().cast()),
                buffer_size as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        match result {
            Ok(()) => {
                // Parse the buffer for this VCN window.
                parse_retrieval_pointers(&buffer, bytes_returned as usize, &mut extents);
                break;
            }
            Err(err) => {
                // Extract Win32 error code from HRESULT
                // HRESULT format: 0x8007XXXX where XXXX is the Win32 error code
                // For FACILITY_WIN32 errors, the low 16 bits contain the Win32 error
                let hresult = err.code().0 as u32;
                let win32_error = if (hresult & 0xFFFF0000) == 0x80070000 {
                    // FACILITY_WIN32 HRESULT - extract low 16 bits
                    hresult & 0xFFFF
                } else {
                    // Not a Win32 HRESULT, use as-is (might be raw Win32 error)
                    hresult
                };

                // ERROR_MORE_DATA (234) - buffer too small, but the data for the
                // requested StartingVcn range is valid and complete. We must NOT
                // advance StartingVcn here; simply grow the buffer and retry so we
                // get the full RETRIEVAL_POINTERS_BUFFER for this window.
                if win32_error == 234 {
                    // Grow buffer and retry without modifying StartingVcn.
                    buffer_size *= 2;
                    buffer.resize(buffer_size, 0);
                    continue;
                }

                // ERROR_HANDLE_EOF (38) - no more extents beyond this VCN.
                if win32_error == 38 {
                    break;
                }

                // Other error - return what we have or error.
                if extents.is_empty() {
                    return Err(MftError::RetrievalPointers(format!(
                        "FSCTL_GET_RETRIEVAL_POINTERS failed: HRESULT=0x{:08X}, Win32={}",
                        hresult, win32_error
                    )));
                }
                break;
            }
        }
    }

    Ok(extents)
}

/// Parses the RETRIEVAL_POINTERS_BUFFER structure.
fn parse_retrieval_pointers(buffer: &[u8], size: usize, extents: &mut Vec<MftExtent>) {
    if size < size_of::<u32>() + size_of::<i64>() {
        return;
    }

    // RETRIEVAL_POINTERS_BUFFER layout:
    // - ExtentCount: u32
    // - StartingVcn: i64
    // - Extents[]: array of { NextVcn: i64, Lcn: i64 }

    let extent_count = u32::from_le_bytes(buffer[0..4].try_into().unwrap()) as usize;
    let mut prev_vcn = i64::from_le_bytes(buffer[8..16].try_into().unwrap()) as u64;

    let extent_size = 16; // sizeof(LARGE_INTEGER) * 2
    let extents_offset = 16; // After ExtentCount (4) + padding (4) + StartingVcn (8)

    for i in 0..extent_count {
        let offset = extents_offset + i * extent_size;
        if offset + extent_size > size {
            break;
        }

        let next_vcn = i64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap()) as u64;
        let lcn = i64::from_le_bytes(buffer[offset + 8..offset + 16].try_into().unwrap());

        let cluster_count = next_vcn.saturating_sub(prev_vcn);

        extents.push(MftExtent {
            vcn: prev_vcn,
            cluster_count,
            lcn,
        });

        prev_vcn = next_vcn;
    }
}

impl Drop for VolumeHandle {
    #[allow(unsafe_code)] // Required: Windows FFI for CloseHandle
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

// ============================================================================
// MFT Bitmap
// ============================================================================

/// Bitmap indicating which MFT records are in use.
///
/// The `$MFT::$BITMAP` stream contains one bit per MFT record.
/// If the bit is set (1), the record is in use; if clear (0), it's free.
#[derive(Debug, Clone)]
pub struct MftBitmap {
    /// Raw bitmap data.
    data: Vec<u8>,
    /// Number of records this bitmap covers.
    record_count: usize,
}

impl MftBitmap {
    /// Creates a new bitmap from raw bytes.
    #[must_use]
    pub fn from_bytes(data: Vec<u8>) -> Self {
        let record_count = data.len() * 8;
        Self { data, record_count }
    }

    /// Creates a bitmap where all records are marked as valid.
    ///
    /// Used as a fallback when the actual bitmap cannot be read.
    #[must_use]
    pub fn new_all_valid(record_count: usize) -> Self {
        let byte_count = (record_count + 7) / 8;
        Self {
            data: vec![0xFF; byte_count],
            record_count,
        }
    }

    /// Checks if a specific record is in use.
    ///
    /// # Arguments
    ///
    /// * `frs` - The File Record Segment number to check.
    ///
    /// # Returns
    ///
    /// `true` if the record is in use, `false` if it's free or out of range.
    #[must_use]
    pub fn is_record_in_use(&self, frs: u64) -> bool {
        let frs = frs as usize;
        if frs >= self.record_count {
            return false;
        }

        let byte_index = frs / 8;
        let bit_index = frs % 8;

        if byte_index >= self.data.len() {
            return false;
        }

        (self.data[byte_index] & (1 << bit_index)) != 0
    }

    /// Returns the number of records marked as in use.
    #[must_use]
    pub fn count_in_use(&self) -> usize {
        // Use popcount for efficiency
        self.data
            .iter()
            .map(|&byte| byte.count_ones() as usize)
            .sum()
    }

    /// Returns the total number of records this bitmap covers.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.record_count
    }

    /// Returns the raw bitmap data.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Returns an iterator over the FRS numbers of records that are in use.
    pub fn in_use_records(&self) -> impl Iterator<Item = u64> + '_ {
        self.data.iter().enumerate().flat_map(|(byte_idx, &byte)| {
            (0..8).filter_map(move |bit_idx| {
                if (byte & (1 << bit_idx)) != 0 {
                    Some((byte_idx * 8 + bit_idx) as u64)
                } else {
                    None
                }
            })
        })
    }

    /// Finds the first N records that are in use, starting from a given FRS.
    ///
    /// This is useful for batch processing.
    pub fn find_in_use_range(&self, start_frs: u64, count: usize) -> Vec<u64> {
        let mut result = Vec::with_capacity(count);
        let start = start_frs as usize;

        for frs in start..self.record_count {
            if result.len() >= count {
                break;
            }

            let byte_index = frs / 8;
            let bit_index = frs % 8;

            if byte_index < self.data.len() && (self.data[byte_index] & (1 << bit_index)) != 0 {
                result.push(frs as u64);
            }
        }

        result
    }

    /// Calculates skip ranges for a cluster-aligned read.
    ///
    /// Given a range of FRS numbers that would be read in a single I/O
    /// operation, this returns how many records at the beginning and end
    /// can be skipped because they are not in use.
    ///
    /// This is the key optimization from the C++ implementation - we can avoid
    /// reading entire clusters if all records in them are unused.
    ///
    /// # Arguments
    ///
    /// * `start_frs` - First FRS in the range
    /// * `end_frs` - Last FRS in the range (exclusive)
    ///
    /// # Returns
    ///
    /// `(skip_begin, skip_end)` - Number of records to skip at start and end.
    #[must_use]
    pub fn calculate_skip_range(&self, start_frs: u64, end_frs: u64) -> (u64, u64) {
        let start = start_frs as usize;
        let end = (end_frs as usize).min(self.record_count);

        if start >= end {
            return (0, 0);
        }

        // Find first in-use record from the beginning
        let mut skip_begin = 0u64;
        for frs in start..end {
            if self.is_record_in_use(frs as u64) {
                break;
            }
            skip_begin += 1;
        }

        // If all records are unused, skip the entire range
        if skip_begin == (end - start) as u64 {
            return (skip_begin, 0);
        }

        // Find first in-use record from the end
        let mut skip_end = 0u64;
        for frs in (start..end).rev() {
            if self.is_record_in_use(frs as u64) {
                break;
            }
            skip_end += 1;
        }

        (skip_begin, skip_end)
    }

    /// Checks if an entire cluster range has any in-use records.
    ///
    /// This is a fast check using byte-level operations.
    ///
    /// # Arguments
    ///
    /// * `start_frs` - First FRS in the cluster
    /// * `records_per_cluster` - Number of records per cluster
    ///
    /// # Returns
    ///
    /// `true` if any record in the cluster is in use.
    #[must_use]
    pub fn cluster_has_in_use(&self, start_frs: u64, records_per_cluster: u32) -> bool {
        let start = start_frs as usize;
        let end = (start + records_per_cluster as usize).min(self.record_count);

        // Fast path: check whole bytes when aligned
        let start_byte = start / 8;
        let end_byte = (end + 7) / 8;

        // Check if any byte in the range has any bits set
        for byte_idx in start_byte..end_byte.min(self.data.len()) {
            let byte = self.data[byte_idx];

            // Mask for partial bytes at boundaries
            let mask = if byte_idx == start_byte && start % 8 != 0 {
                // Mask out bits before start
                0xFF << (start % 8)
            } else if byte_idx == end_byte - 1 && end % 8 != 0 {
                // Mask out bits after end
                (1u8 << (end % 8)) - 1
            } else {
                0xFF
            };

            if (byte & mask) != 0 {
                return true;
            }
        }

        false
    }

    /// Returns ranges of clusters that contain in-use records.
    ///
    /// This is the key optimization for skipping entire clusters during I/O.
    ///
    /// # Arguments
    ///
    /// * `records_per_cluster` - Number of MFT records per cluster
    ///
    /// # Returns
    ///
    /// Iterator of `(start_cluster, cluster_count)` tuples for ranges with
    /// in-use records.
    pub fn in_use_cluster_ranges(
        &self,
        records_per_cluster: u32,
    ) -> impl Iterator<Item = (u64, u64)> + '_ {
        let total_clusters =
            (self.record_count + records_per_cluster as usize - 1) / records_per_cluster as usize;

        InUseClusterRangeIterator {
            bitmap: self,
            records_per_cluster,
            current_cluster: 0,
            total_clusters: total_clusters as u64,
        }
    }
}

/// Iterator over ranges of clusters containing in-use records.
struct InUseClusterRangeIterator<'a> {
    bitmap: &'a MftBitmap,
    records_per_cluster: u32,
    current_cluster: u64,
    total_clusters: u64,
}

impl Iterator for InUseClusterRangeIterator<'_> {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        // Skip clusters with no in-use records
        while self.current_cluster < self.total_clusters {
            let start_frs = self.current_cluster * u64::from(self.records_per_cluster);
            if self
                .bitmap
                .cluster_has_in_use(start_frs, self.records_per_cluster)
            {
                break;
            }
            self.current_cluster += 1;
        }

        if self.current_cluster >= self.total_clusters {
            return None;
        }

        // Found a cluster with in-use records, find the end of this range
        let range_start = self.current_cluster;
        while self.current_cluster < self.total_clusters {
            let start_frs = self.current_cluster * u64::from(self.records_per_cluster);
            if !self
                .bitmap
                .cluster_has_in_use(start_frs, self.records_per_cluster)
            {
                break;
            }
            self.current_cluster += 1;
        }

        Some((range_start, self.current_cluster - range_start))
    }
}

// ============================================================================
// Privilege Checking
// ============================================================================

/// Checks if the current process has Administrator privileges.
///
/// MFT reading requires Administrator privileges or `SE_BACKUP_PRIVILEGE`.
#[must_use]
#[allow(unsafe_code)] // Required: Windows FFI (OpenProcessToken, GetTokenInformation)
pub fn is_elevated() -> bool {
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: All Windows API calls are properly checked for errors.
    unsafe {
        let mut token_handle = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut return_length = 0_u32;

        let result = GetTokenInformation(
            token_handle,
            TokenElevation,
            Some(core::ptr::from_mut(&mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut return_length,
        );

        let _ = CloseHandle(token_handle);

        result.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// Returns the path to the volume root (e.g., "C:\").
#[must_use]
pub fn volume_root_path(volume: char) -> PathBuf {
    PathBuf::from(format!("{}:\\", volume.to_ascii_uppercase()))
}

/// Infer drive letter from a file path.
///
/// Extracts the drive letter from absolute paths (e.g., `C:\foo\bar.parquet` →
/// `'C'`). For relative paths, falls back to the current working directory's
/// drive.
///
/// # Arguments
///
/// * `path` - Any file path (absolute or relative)
///
/// # Returns
///
/// - `Some(char)` - Uppercase drive letter if found
/// - `None` - If path has no drive prefix and current directory cannot be
///   determined
///
/// # Example
///
/// ```rust,ignore
/// use std::path::Path;
/// use uffs_mft::infer_drive_from_path;
///
/// // Absolute path
/// assert_eq!(infer_drive_from_path(Path::new("C:\\data\\index.parquet")), Some('C'));
///
/// // Relative path (uses current directory's drive)
/// // If cwd is D:\work, returns Some('D')
/// let drive = infer_drive_from_path(Path::new("index.parquet"));
/// ```
#[must_use]
pub fn infer_drive_from_path(path: &Path) -> Option<char> {
    use std::path::{Component, Prefix};

    // Try to get drive from the path itself (absolute paths like C:\foo)
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        match prefix.kind() {
            Prefix::Disk(drive_byte) | Prefix::VerbatimDisk(drive_byte) => {
                return Some((drive_byte as char).to_ascii_uppercase());
            }
            _ => {} // UNC paths, device paths, etc. - no drive letter
        }
    }

    // For relative paths, get drive from current working directory
    std::env::current_dir().ok().and_then(|cwd| {
        if let Some(Component::Prefix(prefix)) = cwd.components().next() {
            match prefix.kind() {
                Prefix::Disk(drive_byte) | Prefix::VerbatimDisk(drive_byte) => {
                    Some((drive_byte as char).to_ascii_uppercase())
                }
                _ => None,
            }
        } else {
            None
        }
    })
}

/// Detects all available NTFS drives on the system.
///
/// This function iterates through all possible drive letters (A-Z) and
/// checks which ones are valid NTFS volumes that can be read.
///
/// # Returns
///
/// A vector of drive letters (uppercase) for all available NTFS drives.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_mft::platform::detect_ntfs_drives;
///
/// let drives = detect_ntfs_drives();
/// println!("Found NTFS drives: {:?}", drives);
/// // Output: Found NTFS drives: ['C', 'D', 'E']
/// ```
#[must_use]
#[allow(unsafe_code)] // Required: Windows FFI (GetLogicalDrives)
pub fn detect_ntfs_drives() -> Vec<char> {
    use windows::Win32::Storage::FileSystem::GetLogicalDrives;

    let mut ntfs_drives = Vec::new();

    // SAFETY: GetLogicalDrives is a simple Windows API call with no side effects.
    let drive_mask = unsafe { GetLogicalDrives() };

    if drive_mask == 0 {
        return ntfs_drives;
    }

    // Check each drive letter A-Z
    for i in 0..26_u32 {
        if (drive_mask & (1 << i)) != 0 {
            let drive_letter = char::from(b'A' + i as u8);

            // Try to open the volume to check if it's NTFS
            if is_ntfs_volume(drive_letter) {
                ntfs_drives.push(drive_letter);
            }
        }
    }

    ntfs_drives
}

/// Checks if a drive is an NTFS volume.
///
/// Uses `GetVolumeInformationW` to check the filesystem name, which doesn't
/// require elevated privileges.
#[allow(unsafe_code)] // Required: Windows FFI (GetDriveTypeW, GetVolumeInformationW)
fn is_ntfs_volume(drive_letter: char) -> bool {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW};

    // First check if it's a fixed or removable drive (skip network, CD-ROM, etc.)
    let root_path: Vec<u16> = format!("{}:\\", drive_letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let drive_type = unsafe { GetDriveTypeW(PCWSTR(root_path.as_ptr())) };

    // DRIVE_FIXED = 3, DRIVE_REMOVABLE = 2
    // Skip DRIVE_UNKNOWN (0), DRIVE_NO_ROOT_DIR (1), DRIVE_REMOTE (4), DRIVE_CDROM
    // (5), DRIVE_RAMDISK (6)
    if drive_type != 2 && drive_type != 3 {
        return false;
    }

    // Get filesystem name using GetVolumeInformationW (no admin required)
    let mut fs_name_buffer: [u16; 32] = [0; 32];

    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR(root_path.as_ptr()),
            None,                      // Volume name buffer (not needed)
            None,                      // Volume serial number (not needed)
            None,                      // Max component length (not needed)
            None,                      // File system flags (not needed)
            Some(&mut fs_name_buffer), // File system name buffer
        )
    };

    if result.is_err() {
        return false;
    }

    // Convert filesystem name to string and check if it's NTFS
    let fs_name = String::from_utf16_lossy(&fs_name_buffer);
    let fs_name = fs_name.trim_end_matches('\0');

    fs_name == "NTFS"
}

/// Checks if a volume is read-only.
///
/// Uses `GetVolumeInformationW` to check the `FILE_READ_ONLY_VOLUME` flag.
/// This is useful for incremental updates - if a volume is read-only,
/// nothing can have changed, so we can skip USN journal checks.
///
/// # Arguments
///
/// * `drive_letter` - The drive letter to check (e.g., 'C', 'F')
///
/// # Returns
///
/// `true` if the volume is read-only, `false` otherwise (or on error).
#[cfg(windows)]
#[must_use]
#[allow(unsafe_code)] // Required: Windows FFI (GetVolumeInformationW)
pub fn is_volume_read_only(drive_letter: char) -> bool {
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

    // FILE_READ_ONLY_VOLUME = 0x00080000
    const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

    let root_path: Vec<u16> = format!("{}:\\", drive_letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut fs_flags: u32 = 0;

    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR(root_path.as_ptr()),
            None,                // Volume name buffer (not needed)
            None,                // Volume serial number (not needed)
            None,                // Max component length (not needed)
            Some(&mut fs_flags), // File system flags
            None,                // File system name buffer (not needed)
        )
    };

    if result.is_err() {
        return false;
    }

    (fs_flags & FILE_READ_ONLY_VOLUME) != 0
}

/// Stub for non-Windows platforms.
#[cfg(not(windows))]
#[must_use]
pub fn is_volume_read_only(_drive_letter: char) -> bool {
    false
}

// ============================================================================
// Drive Type Detection (SSD vs HDD vs NVMe)
// ============================================================================

/// Represents the type of storage device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    /// NVMe Solid State Drive (PCIe, extremely high IOPS, 64K+ queue depth).
    Nvme,
    /// SATA Solid State Drive (no seek time, high IOPS, 32 queue depth).
    Ssd,
    /// Hard Disk Drive (rotational, seek time matters).
    Hdd,
    /// Unknown drive type (assume HDD for safety).
    Unknown,
}

impl DriveType {
    /// Returns the optimal chunk size for this drive type.
    ///
    /// - NVMe: 4 MB (high bandwidth, large sequential reads are efficient)
    /// - SSD: 2 MB (SATA bandwidth, moderate chunk size)
    /// - HDD: 1 MB (matches C++ actual behavior - profiler shows 1024KB reads)
    /// - Unknown: 1 MB (conservative default)
    ///
    /// Note: C++ team initially said 64KB, but profiler shows they actually use
    /// 1MB (1024KB) reads. With 1MB reads, C++ does 8,141 reads for 11.5GB MFT.
    /// With 64KB reads, Rust would need ~180,000 reads - the syscall overhead
    /// alone adds ~20 seconds. Using 1MB matches C++ actual behavior.
    #[must_use]
    pub const fn optimal_chunk_size(&self) -> usize {
        match self {
            Self::Nvme => 4 * 1024 * 1024, // 4 MB - NVMe can handle large chunks
            Self::Ssd => 2 * 1024 * 1024,  // 2 MB - SATA SSD
            Self::Hdd => 1024 * 1024,      // 1 MB - C++ profiler shows 1024KB reads
            Self::Unknown => 1024 * 1024,  // 1 MB default
        }
    }

    /// Returns the optimal number of prefetch buffers.
    ///
    /// - NVMe: 8 buffers (massive parallelism)
    /// - SSD: 4 buffers (can handle high parallelism)
    /// - HDD: 2 buffers (double-buffering is sufficient)
    #[must_use]
    pub const fn prefetch_buffers(&self) -> usize {
        match self {
            Self::Nvme => 8,
            Self::Ssd => 4,
            Self::Hdd => 2,
            Self::Unknown => 2,
        }
    }

    /// Returns the optimal I/O concurrency (queue depth) for this drive type.
    ///
    /// This is the number of async I/O operations to keep in flight
    /// simultaneously. Higher values hide latency but use more memory.
    ///
    /// - NVMe: 32 (can handle 64K+ queue depth, but 32 is practical)
    /// - SSD: 8 (SATA NCQ supports 32, but 8 is sufficient)
    /// - HDD: 2 (more causes seeks and hurts performance)
    /// - Unknown: 4 (conservative default)
    ///
    /// Based on benchmarks (2026-01-24):
    /// - HDD S: 40.3s @ 285 MB/s with concurrency=2,4,32,64 (no difference -
    ///   I/O bound)
    /// - NVMe C: 2.16s @ 2109 MB/s with concurrency=32-64 (28% faster than C++)
    /// - NVMe F: 1.34s @ 3384 MB/s with concurrency=64 (13% faster than C++)
    #[must_use]
    pub const fn optimal_concurrency(&self) -> usize {
        match self {
            Self::Nvme => 32,   // NVMe can handle 64K+ queue depth
            Self::Ssd => 8,     // SATA NCQ supports 32
            Self::Hdd => 2,     // Sequential, avoid seeks
            Self::Unknown => 4, // Conservative default
        }
    }

    /// Returns the optimal I/O chunk size for this drive type.
    ///
    /// This is the size of each async read operation. Larger chunks reduce
    /// syscall overhead but increase latency per completion.
    ///
    /// - NVMe: 4 MB (high bandwidth, amortize syscall cost)
    /// - SSD: 2 MB (SATA bandwidth)
    /// - HDD: 1 MB (matches C++ behavior)
    /// - Unknown: 1 MB (conservative default)
    ///
    /// Based on benchmarks (2026-01-24):
    /// - 4 MB is optimal for NVMe (16 MB shows slight regression)
    /// - 1-2 MB is optimal for HDD (no difference observed)
    #[must_use]
    pub const fn optimal_io_size(&self) -> usize {
        self.optimal_chunk_size() // Same as chunk size
    }

    /// Returns true if this is a high-performance drive (SSD or NVMe).
    #[must_use]
    pub const fn is_high_performance(&self) -> bool {
        matches!(self, Self::Nvme | Self::Ssd)
    }

    /// Returns true if this drive benefits from parallel parsing.
    ///
    /// NVMe drives can read faster than single-threaded parsing can process,
    /// so parallel parsing is beneficial. HDDs are I/O bound, so parallel
    /// parsing doesn't help.
    #[must_use]
    pub const fn benefits_from_parallel_parsing(&self) -> bool {
        matches!(self, Self::Nvme)
    }
}

/// Detects whether a drive is NVMe, SSD, or HDD.
///
/// Uses `IOCTL_STORAGE_QUERY_PROPERTY` with:
/// - `StorageAdapterProperty` to detect NVMe bus type
/// - `StorageDeviceSeekPenaltyProperty` to distinguish SSD from HDD
///
/// # Arguments
///
/// * `drive_letter` - The drive letter to check (e.g., 'C')
///
/// # Returns
///
/// The detected drive type, or `DriveType::Unknown` if detection fails.
#[must_use]
#[allow(unsafe_code)] // Required: Windows FFI (DeviceIoControl)
pub fn detect_drive_type(drive_letter: char) -> DriveType {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    // IOCTL_STORAGE_QUERY_PROPERTY = 0x002D1400
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

    // PropertyId values
    const STORAGE_DEVICE_PROPERTY: u32 = 0; // StorageDeviceProperty
    const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;

    // QueryType: PropertyStandardQuery = 0
    const PROPERTY_STANDARD_QUERY: u32 = 0;

    // Bus types (from STORAGE_BUS_TYPE enum)
    const BUS_TYPE_NVME: u32 = 17; // BusTypeNvme

    // STORAGE_PROPERTY_QUERY structure
    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: u32,
        query_type: u32,
        additional_parameters: [u8; 1],
    }

    // STORAGE_DEVICE_DESCRIPTOR structure (partial - we only need bus_type)
    #[repr(C)]
    struct StorageDeviceDescriptor {
        version: u32,
        size: u32,
        device_type: u8,
        device_type_modifier: u8,
        removable_media: u8,
        command_queueing: u8,
        vendor_id_offset: u32,
        product_id_offset: u32,
        product_revision_offset: u32,
        serial_number_offset: u32,
        bus_type: u32, // STORAGE_BUS_TYPE
        raw_properties_length: u32,
        raw_device_properties: [u8; 1],
    }

    // DEVICE_SEEK_PENALTY_DESCRIPTOR structure
    #[repr(C)]
    struct DeviceSeekPenaltyDescriptor {
        version: u32,
        size: u32,
        incurs_seek_penalty: u8, // BOOLEAN
    }

    // Open the physical drive
    let drive_path: Vec<u16> = format!("\\\\.\\{}:", drive_letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let handle = unsafe {
        CreateFileW(
            PCWSTR(drive_path.as_ptr()),
            0, // No access needed for query
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    };

    let handle = match handle {
        Ok(h) => h,
        Err(_) => return DriveType::Unknown,
    };

    // First, check if it's NVMe by querying the bus type
    let is_nvme = {
        let query = StoragePropertyQuery {
            property_id: STORAGE_DEVICE_PROPERTY,
            query_type: PROPERTY_STANDARD_QUERY,
            additional_parameters: [0],
        };

        // Allocate a buffer large enough for the descriptor
        let mut buffer = [0u8; 1024];
        let mut bytes_returned: u32 = 0;

        let result = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(&query as *const _ as *const std::ffi::c_void),
                size_of::<StoragePropertyQuery>() as u32,
                Some(buffer.as_mut_ptr() as *mut std::ffi::c_void),
                buffer.len() as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        if result.is_ok() && bytes_returned >= size_of::<StorageDeviceDescriptor>() as u32 {
            let descriptor = unsafe { &*(buffer.as_ptr() as *const StorageDeviceDescriptor) };
            descriptor.bus_type == BUS_TYPE_NVME
        } else {
            false
        }
    };

    // If it's NVMe, we're done
    if is_nvme {
        let _ = unsafe { CloseHandle(handle) };
        return DriveType::Nvme;
    }

    // Otherwise, check seek penalty to distinguish SSD from HDD
    let query = StoragePropertyQuery {
        property_id: STORAGE_DEVICE_SEEK_PENALTY_PROPERTY,
        query_type: PROPERTY_STANDARD_QUERY,
        additional_parameters: [0],
    };

    let mut descriptor = DeviceSeekPenaltyDescriptor {
        version: 0,
        size: 0,
        incurs_seek_penalty: 0,
    };

    let mut bytes_returned: u32 = 0;

    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const _ as *const std::ffi::c_void),
            size_of::<StoragePropertyQuery>() as u32,
            Some(&mut descriptor as *mut _ as *mut std::ffi::c_void),
            size_of::<DeviceSeekPenaltyDescriptor>() as u32,
            Some(&mut bytes_returned),
            None,
        )
    };

    // Close handle
    let _ = unsafe { CloseHandle(handle) };

    if result.is_ok() && bytes_returned >= size_of::<DeviceSeekPenaltyDescriptor>() as u32 {
        if descriptor.incurs_seek_penalty == 0 {
            DriveType::Ssd
        } else {
            DriveType::Hdd
        }
    } else {
        // Fallback: try to detect via trim support
        detect_drive_type_via_trim(drive_letter)
    }
}

/// Fallback detection using TRIM support (SSDs support TRIM).
#[allow(unsafe_code)]
fn detect_drive_type_via_trim(drive_letter: char) -> DriveType {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    const STORAGE_DEVICE_TRIM_PROPERTY: u32 = 8;
    const PROPERTY_STANDARD_QUERY: u32 = 0;

    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: u32,
        query_type: u32,
        additional_parameters: [u8; 1],
    }

    #[repr(C)]
    struct DeviceTrimDescriptor {
        version: u32,
        size: u32,
        trim_enabled: u8,
    }

    let drive_path: Vec<u16> = format!("\\\\.\\{}:", drive_letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let handle = unsafe {
        CreateFileW(
            PCWSTR(drive_path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    };

    let handle = match handle {
        Ok(h) => h,
        Err(_) => return DriveType::Unknown,
    };

    let query = StoragePropertyQuery {
        property_id: STORAGE_DEVICE_TRIM_PROPERTY,
        query_type: PROPERTY_STANDARD_QUERY,
        additional_parameters: [0],
    };

    let mut descriptor = DeviceTrimDescriptor {
        version: 0,
        size: 0,
        trim_enabled: 0,
    };

    let mut bytes_returned: u32 = 0;

    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const _ as *const std::ffi::c_void),
            size_of::<StoragePropertyQuery>() as u32,
            Some(&mut descriptor as *mut _ as *mut std::ffi::c_void),
            size_of::<DeviceTrimDescriptor>() as u32,
            Some(&mut bytes_returned),
            None,
        )
    };

    let _ = unsafe { CloseHandle(handle) };

    if result.is_ok() && bytes_returned >= size_of::<DeviceTrimDescriptor>() as u32 {
        if descriptor.trim_enabled != 0 {
            DriveType::Ssd
        } else {
            DriveType::Hdd
        }
    } else {
        DriveType::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_root_path() {
        assert_eq!(volume_root_path('c'), PathBuf::from("C:\\"));
        assert_eq!(volume_root_path('D'), PathBuf::from("D:\\"));
    }

    #[test]
    fn test_is_elevated() {
        // Just verify it doesn't panic
        let _ = is_elevated();
    }

    // =========================================================================
    // DriveType optimal settings tests
    // =========================================================================

    #[test]
    fn test_nvme_optimal_settings() {
        let drive_type = DriveType::Nvme;

        // NVMe should use highest concurrency and largest I/O size
        assert_eq!(drive_type.optimal_concurrency(), 32);
        assert_eq!(drive_type.optimal_io_size(), 4 * 1024 * 1024); // 4 MB
        assert_eq!(drive_type.optimal_chunk_size(), 4 * 1024 * 1024); // 4 MB
        assert_eq!(drive_type.prefetch_buffers(), 8);
        assert!(drive_type.is_high_performance());
        assert!(drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_ssd_optimal_settings() {
        let drive_type = DriveType::Ssd;

        // SSD should use moderate concurrency and I/O size
        assert_eq!(drive_type.optimal_concurrency(), 8);
        assert_eq!(drive_type.optimal_io_size(), 2 * 1024 * 1024); // 2 MB
        assert_eq!(drive_type.optimal_chunk_size(), 2 * 1024 * 1024); // 2 MB
        assert_eq!(drive_type.prefetch_buffers(), 4);
        assert!(drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_hdd_optimal_settings() {
        let drive_type = DriveType::Hdd;

        // HDD should use minimal concurrency to avoid seeks
        assert_eq!(drive_type.optimal_concurrency(), 2);
        assert_eq!(drive_type.optimal_io_size(), 1024 * 1024); // 1 MB
        assert_eq!(drive_type.optimal_chunk_size(), 1024 * 1024); // 1 MB
        assert_eq!(drive_type.prefetch_buffers(), 2);
        assert!(!drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_unknown_optimal_settings() {
        let drive_type = DriveType::Unknown;

        // Unknown should use conservative defaults (similar to HDD)
        assert_eq!(drive_type.optimal_concurrency(), 4);
        assert_eq!(drive_type.optimal_io_size(), 1024 * 1024); // 1 MB
        assert_eq!(drive_type.optimal_chunk_size(), 1024 * 1024); // 1 MB
        assert_eq!(drive_type.prefetch_buffers(), 2);
        assert!(!drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_optimal_settings_are_reasonable() {
        // Verify that optimal settings follow expected ordering:
        // NVMe > SSD > HDD for concurrency and I/O size

        let nvme = DriveType::Nvme;
        let ssd = DriveType::Ssd;
        let hdd = DriveType::Hdd;

        // Concurrency: NVMe > SSD > HDD
        assert!(nvme.optimal_concurrency() > ssd.optimal_concurrency());
        assert!(ssd.optimal_concurrency() > hdd.optimal_concurrency());

        // I/O size: NVMe > SSD > HDD
        assert!(nvme.optimal_io_size() > ssd.optimal_io_size());
        assert!(ssd.optimal_io_size() >= hdd.optimal_io_size());

        // Prefetch buffers: NVMe > SSD > HDD
        assert!(nvme.prefetch_buffers() > ssd.prefetch_buffers());
        assert!(ssd.prefetch_buffers() >= hdd.prefetch_buffers());
    }
}
