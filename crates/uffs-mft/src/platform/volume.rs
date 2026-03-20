//! Windows NTFS volume handle and metadata access helpers.

use std::mem::size_of;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_FLAG_SEQUENTIAL_SCAN, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    SYNCHRONIZE,
};
use windows::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};
use windows::core::PCWSTR;
use zerocopy::FromBytes;

use super::bitmap::MftBitmap;
use super::extents::{MftExtent, get_retrieval_pointers};
use crate::error::{MftError, Result};
use crate::ntfs::NtfsBootSector;

/// FILE_READ_DATA access right (0x0001) - required to read data from a
/// file/volume.
const FILE_READ_DATA: u32 = 0x0001;

/// Short poll interval for Windows completion waits in MFT hot paths.
pub(crate) const IOCP_WAIT_POLL_INTERVAL_MS: u32 = 100;

/// Maximum stall time allowed between Windows completion notifications.
pub(crate) const IOCP_WAIT_COMPLETION_DEADLINE: Duration = Duration::from_secs(30);

/// Win32 error code reported when a wait interval expires.
pub(crate) const WAIT_TIMEOUT_ERROR_CODE: u32 = 258;

/// Win32 error code reported when an overlapped operation is aborted.
const ERROR_OPERATION_ABORTED_CODE: u32 = 995;

/// Classifies a Windows wait failure using the approved Wave 3A taxonomy.
#[must_use]
pub(crate) fn classify_wait_error_code(
    operation: &'static str,
    error_code: u32,
    detail: impl Into<String>,
) -> MftError {
    let detail = detail.into();

    match error_code {
        ERROR_OPERATION_ABORTED_CODE => MftError::Cancelled {
            operation,
            reason: format!("{detail} (Win32 error {error_code})"),
        },
        _ => MftError::WaitFailed {
            operation,
            reason: format!("{detail} (Win32 error {error_code})"),
        },
    }
}

/// Builds a timeout error for a Windows completion wait that exceeded its
/// deadline.
#[must_use]
pub(crate) fn wait_deadline_exceeded(
    operation: &'static str,
    waited: Duration,
    detail: impl Into<String>,
) -> MftError {
    let detail = detail.into();

    MftError::Timeout {
        operation,
        reason: format!(
            "{detail} after {} ms without observing a completion",
            waited.as_millis()
        ),
    }
}

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

// SAFETY: `VolumeHandle` owns a Windows `HANDLE` to a kernel-managed file
// object plus immutable metadata (`volume` and `volume_data`). It contains no
// Rust references or unsynchronized interior mutability, so moving ownership
// to another thread does not invalidate any aliasing assumptions. Handle
// cleanup remains centralized in `Drop`.
#[expect(
    unsafe_code,
    reason = "windows file handles are thread-safe kernel objects"
)]
unsafe impl Send for VolumeHandle {}

// SAFETY: Shared references to `VolumeHandle` only expose immutable metadata or
// copy the raw `HANDLE`. The wrapper itself performs no unsynchronized mutable
// access, and Windows file handles are designed to be used from multiple
// threads.
#[expect(
    unsafe_code,
    reason = "windows file handles are thread-safe kernel objects"
)]
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

impl NtfsVolumeData {
    /// Computes the reserved allocated bytes for the root directory.
    ///
    /// C++ formula: `(TotalReserved + MftZoneEnd - MftZoneStart) *
    /// BytesPerCluster`. This is added to the root's `tree_allocated` at
    /// depth 0 during tree metrics computation.
    #[must_use]
    pub const fn reserved_allocated_bytes(&self) -> u64 {
        let reserved_clusters =
            self.total_reserved + self.mft_zone_end.saturating_sub(self.mft_zone_start);
        reserved_clusters * self.bytes_per_cluster as u64
    }
}

impl VolumeHandle {
    /// Opens a volume for direct MFT reading.
    #[expect(unsafe_code, reason = "FFI: windows API (CreateFileW)")]
    pub fn open(volume: char) -> Result<Self> {
        let volume = volume.to_ascii_uppercase();

        if !volume.is_ascii_alphabetic() {
            return Err(MftError::VolumeOpen {
                volume,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid volume letter",
                ),
            });
        }

        let volume_path: Vec<u16> = format!("\\\\.\\{}:", volume)
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // SAFETY: `volume_path` is UTF-16 and NUL-terminated for the duration of
        // the call, optional pointers are passed as `None`, and on success the
        // returned handle is owned by this function.
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(volume_path.as_ptr()),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_SEQUENTIAL_SCAN,
                None,
            )
        };

        let handle = match handle {
            Ok(h) => h,
            Err(err) => {
                if err.code().0 as u32 == 0x8007_0005 {
                    return Err(MftError::InsufficientPrivileges);
                }
                return Err(MftError::VolumeOpen {
                    volume,
                    source: std::io::Error::from_raw_os_error(err.code().0 as i32),
                });
            }
        };

        let volume_data = Self::get_ntfs_volume_data(handle, volume)?;

        Ok(Self {
            handle,
            volume,
            volume_data,
        })
    }

    /// Retrieves NTFS volume data using `FSCTL_GET_NTFS_VOLUME_DATA`.
    #[expect(unsafe_code, reason = "FFI: windows API (DeviceIoControl)")]
    fn get_ntfs_volume_data(handle: HANDLE, volume: char) -> Result<NtfsVolumeData> {
        use windows::Win32::System::IO::DeviceIoControl;

        let mut buffer = NTFS_VOLUME_DATA_BUFFER::default();
        let mut bytes_returned: u32 = 0;

        // SAFETY: `handle` is an open volume handle, `buffer` points to valid
        // writable storage for `NTFS_VOLUME_DATA_BUFFER`, and
        // `bytes_returned` is a valid out-parameter for the duration of the
        // call.
        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_GET_NTFS_VOLUME_DATA,
                None,
                0,
                Some(core::ptr::from_mut(&mut buffer).cast()),
                size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        if result.is_err() {
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
    #[must_use]
    pub const fn raw_handle(&self) -> HANDLE {
        self.handle
    }

    /// Opens a new handle to the same volume with `FILE_FLAG_OVERLAPPED`.
    #[expect(unsafe_code, reason = "FFI: windows API (CreateFileW)")]
    pub fn open_overlapped_handle(&self) -> Result<HANDLE> {
        let volume_path: Vec<u16> = format!("\\\\.\\{}:", self.volume)
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // SAFETY: `volume_path` is UTF-16 and NUL-terminated for the duration of
        // the call, optional pointers are passed as `None`, and ownership of
        // any returned handle is transferred to the caller.
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
    #[expect(unsafe_code, reason = "FFI: windows API to read the boot sector")]
    pub fn read_boot_sector(&self) -> Result<NtfsBootSector> {
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

        let mut new_position = 0_i64;
        // SAFETY: `self.handle` is a live volume handle and `new_position`
        // points to writable stack storage for the duration of the call.
        unsafe {
            SetFilePointerEx(self.handle, 0, Some(&mut new_position), FILE_BEGIN)?;
        }

        let mut buffer = [0_u8; 512];
        let mut bytes_read = 0_u32;

        // SAFETY: `self.handle` is a live volume handle, `buffer` is a writable
        // 512-byte stack array, and `bytes_read` is a valid out-parameter.
        unsafe {
            ReadFile(self.handle, Some(&mut buffer), Some(&mut bytes_read), None)?;
        }

        if bytes_read != 512 {
            return Err(MftError::BootSectorRead(format!(
                "Expected 512 bytes, got {}",
                bytes_read
            )));
        }

        let boot_sector = match NtfsBootSector::read_from_prefix(&buffer) {
            Ok((boot_sector, _)) => boot_sector,
            Err(_) => {
                return Err(MftError::InvalidBootSector(
                    "Unable to decode NTFS boot sector layout".to_owned(),
                ));
            }
        };

        if !boot_sector.is_valid() {
            return Err(MftError::InvalidBootSector(
                "Invalid OEM ID (not NTFS)".to_owned(),
            ));
        }

        Ok(boot_sector)
    }

    /// Gets the MFT extents (data runs) using `FSCTL_GET_RETRIEVAL_POINTERS`.
    #[expect(
        unsafe_code,
        reason = "FFI: windows API (CreateFileW, DeviceIoControl, CloseHandle)"
    )]
    pub fn get_mft_extents(&self) -> Result<Vec<MftExtent>> {
        let mft_path: Vec<u16> = format!("{}:\\$MFT", self.volume)
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // SAFETY: `mft_path` is UTF-16 and NUL-terminated for the duration of
        // the call, optional pointers are `None`, and any returned handle is
        // wrapped in `HandleGuard` before use.
        let mft_handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(mft_path.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };

        let mft_handle = match mft_handle {
            Ok(h) => h,
            Err(_err) => {
                return Ok(vec![MftExtent {
                    vcn: 0,
                    cluster_count: self.volume_data.mft_valid_data_length
                        / u64::from(self.volume_data.bytes_per_cluster),
                    lcn: self.volume_data.mft_start_lcn as i64,
                }]);
            }
        };

        let _guard = HandleGuard(mft_handle);
        get_retrieval_pointers(mft_handle)
    }

    /// Gets the MFT bitmap which indicates which records are in use.
    pub fn get_mft_bitmap(&self) -> Result<MftBitmap> {
        self.get_mft_bitmap_internal(false)
    }

    /// Gets the MFT bitmap with optional verbose diagnostic output.
    pub fn get_mft_bitmap_verbose(&self) -> Result<MftBitmap> {
        self.get_mft_bitmap_internal(true)
    }

    #[expect(
        unsafe_code,
        reason = "FFI: windows API (CreateFileW, GetFileSizeEx, ReadFile)"
    )]
    fn get_mft_bitmap_internal(&self, verbose: bool) -> Result<MftBitmap> {
        use windows::Win32::Storage::FileSystem::{
            FILE_BEGIN, GetFileSizeEx, ReadFile, SYNCHRONIZE, SetFilePointerEx,
        };

        let bitmap_path_str = format!("{}:\\$MFT::$BITMAP", self.volume);
        let bitmap_path: Vec<u16> = bitmap_path_str
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        if verbose {
            tracing::info!(volume = %self.volume, bitmap_path = %bitmap_path_str, "Opening MFT bitmap");
        }

        // SAFETY: `bitmap_path` is UTF-16 and NUL-terminated for the duration of
        // the call, optional pointers are `None`, and any returned handle is
        // wrapped in `HandleGuard` before use.
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
                    tracing::info!(volume = %self.volume, handle = ?h, "CreateFileW for MFT bitmap succeeded");
                }
                h
            }
            Err(e) => {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        error = ?e,
                        "CreateFileW for MFT bitmap failed; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
        };

        let _guard = HandleGuard(bitmap_handle);

        let mut file_size: i64 = 0;
        // SAFETY: `bitmap_handle` is a live file handle and `file_size` points
        // to writable stack storage for the duration of the call.
        unsafe {
            if let Err(e) = GetFileSizeEx(bitmap_handle, &mut file_size) {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        error = ?e,
                        "GetFileSizeEx for MFT bitmap failed; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
        }

        if verbose {
            tracing::info!(volume = %self.volume, file_size, "Retrieved MFT bitmap size");
        }

        let extents = match get_retrieval_pointers(bitmap_handle) {
            Ok(e) if !e.is_empty() => {
                if verbose {
                    tracing::info!(volume = %self.volume, extent_count = e.len(), "Retrieved MFT bitmap extents");
                    for (i, ext) in e.iter().enumerate().take(5) {
                        tracing::info!(
                            volume = %self.volume,
                            extent_index = i,
                            vcn = ext.vcn,
                            cluster_count = ext.cluster_count,
                            lcn = ext.lcn,
                            "MFT bitmap extent sample"
                        );
                    }
                    if e.len() > 5 {
                        tracing::info!(
                            volume = %self.volume,
                            additional_extent_count = e.len() - 5,
                            "Additional MFT bitmap extents omitted from verbose sample"
                        );
                    }
                }
                e
            }
            Ok(_) => {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        "get_retrieval_pointers returned no MFT bitmap extents; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
            Err(e) => {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        error = ?e,
                        "get_retrieval_pointers for MFT bitmap failed; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(
                    self.estimated_record_count() as usize
                ));
            }
        };

        let bytes_per_cluster = self.volume_data.bytes_per_cluster;
        let total_clusters: u64 = extents.iter().map(|e| e.cluster_count).sum();
        let aligned_size = (total_clusters * u64::from(bytes_per_cluster)) as usize;
        let mut buffer = vec![0_u8; aligned_size];
        let mut buffer_offset = 0_usize;

        if verbose {
            tracing::info!(
                volume = %self.volume,
                bytes_per_cluster,
                file_size,
                aligned_size,
                "Reading MFT bitmap extents from volume"
            );
        }

        for (i, extent) in extents.iter().enumerate() {
            let byte_offset = extent.lcn * i64::from(bytes_per_cluster);
            let extent_bytes = (extent.cluster_count * u64::from(bytes_per_cluster)) as usize;

            if extent_bytes == 0 {
                continue;
            }

            if verbose && i < 3 {
                tracing::info!(
                    volume = %self.volume,
                    extent_index = i,
                    byte_offset,
                    extent_bytes,
                    "Reading MFT bitmap extent"
                );
            }

            let mut new_position = 0_i64;
            // SAFETY: `self.handle` is a live volume handle and `new_position`
            // is valid writable storage for the duration of the seek call.
            unsafe {
                if let Err(e) = SetFilePointerEx(
                    self.handle,
                    byte_offset,
                    Some(&mut new_position),
                    FILE_BEGIN,
                ) {
                    if verbose {
                        tracing::warn!(
                            volume = %self.volume,
                            extent_index = i,
                            byte_offset,
                            error = ?e,
                            "SetFilePointerEx for MFT bitmap extent failed; falling back to all-valid bitmap"
                        );
                    }
                    return Ok(MftBitmap::new_all_valid(
                        self.estimated_record_count() as usize
                    ));
                }
            }

            let mut bytes_read: u32 = 0;
            // SAFETY: `self.handle` is a live volume handle, the slice points to
            // a contiguous writable region of `extent_bytes`, and `bytes_read`
            // is a valid out-parameter for the duration of the read.
            unsafe {
                if let Err(e) = ReadFile(
                    self.handle,
                    Some(&mut buffer[buffer_offset..buffer_offset + extent_bytes]),
                    Some(&mut bytes_read),
                    None,
                ) {
                    if verbose {
                        tracing::warn!(
                            volume = %self.volume,
                            extent_index = i,
                            extent_bytes,
                            error = ?e,
                            "ReadFile for MFT bitmap extent failed; falling back to all-valid bitmap"
                        );
                    }
                    return Ok(MftBitmap::new_all_valid(
                        self.estimated_record_count() as usize
                    ));
                }
            }

            if verbose && i < 3 {
                tracing::info!(volume = %self.volume, extent_index = i, bytes_read, "Read MFT bitmap extent bytes");
                if i == 0 && bytes_read > 0 {
                    let sample: Vec<String> = buffer
                        [buffer_offset..buffer_offset + 32.min(bytes_read as usize)]
                        .iter()
                        .map(|b| format!("{:02X}", b))
                        .collect();
                    tracing::info!(
                        volume = %self.volume,
                        extent_index = i,
                        sample_hex = %sample.join(" "),
                        "MFT bitmap first-byte sample"
                    );
                }
            }

            buffer_offset += bytes_read as usize;
        }

        if verbose {
            tracing::info!(
                volume = %self.volume,
                total_bytes_read = buffer_offset,
                file_size,
                "Completed MFT bitmap read; truncating to reported file size"
            );
            let all_ff = buffer.iter().take(file_size as usize).all(|&b| b == 0xFF);
            let all_00 = buffer.iter().take(file_size as usize).all(|&b| b == 0x00);
            tracing::info!(volume = %self.volume, all_ff, all_00, "Computed MFT bitmap byte-pattern summary");
        }

        buffer.truncate(file_size as usize);
        Ok(MftBitmap::from_bytes(buffer))
    }
}

impl Drop for VolumeHandle {
    #[expect(unsafe_code, reason = "FFI: windows API (CloseHandle)")]
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: `VolumeHandle` owns this valid handle and closes it once
            // during drop after all safe borrows have ended.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

/// RAII guard for Windows handles.
pub(super) struct HandleGuard(pub(super) HANDLE);

impl Drop for HandleGuard {
    #[expect(unsafe_code, reason = "FFI: windows API (CloseHandle)")]
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `HandleGuard` exclusively owns this valid handle and drops
            // it exactly once when the guard is destroyed.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_wait_error_maps_aborted_waits_to_cancelled() {
        let error = classify_wait_error_code("read_all_index", 995, "wait aborted");

        assert!(matches!(
            error,
            MftError::Cancelled {
                operation: "read_all_index",
                ..
            }
        ));
    }

    #[test]
    fn classify_wait_error_maps_other_wait_failures_to_wait_failed() {
        let error = classify_wait_error_code("read_all_index", 123, "wait failed");

        assert!(matches!(
            error,
            MftError::WaitFailed {
                operation: "read_all_index",
                ..
            }
        ));
    }

    #[test]
    fn wait_deadline_helper_builds_timeout_error() {
        let error = wait_deadline_exceeded(
            "read_all_index",
            Duration::from_secs(31),
            "no completions arrived",
        );

        assert!(matches!(
            error,
            MftError::Timeout {
                operation: "read_all_index",
                ..
            }
        ));
    }
}
