// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows NTFS volume handle and metadata access helpers.
//!
//! Exception: Volume handle + write-protect fallback handles; splitting would
//! fragment the handle lifecycle.

use core::mem::size_of;
use core::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_FLAG_SEQUENTIAL_SCAN, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    SYNCHRONIZE,
};
use windows::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};
use windows::core::PCWSTR;
use zerocopy::FromBytes as _;

use super::bitmap::MftBitmap;
use super::extents::{MftExtent, get_retrieval_pointers};
use crate::error::{MftError, Result};
use crate::index::{frs_to_usize, u32_as_usize};
use crate::ntfs::NtfsBootSector;

/// `FILE_READ_DATA` access right (0x0001) - required to read data from a
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
    let detail_str: String = detail.into();

    match error_code {
        ERROR_OPERATION_ABORTED_CODE => MftError::Cancelled {
            operation,
            reason: format!("{detail_str} (Win32 error {error_code})"),
        },
        _ => MftError::WaitFailed {
            operation,
            reason: format!("{detail_str} (Win32 error {error_code})"),
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
    let detail_str: String = detail.into();

    MftError::Timeout {
        operation,
        reason: format!(
            "{detail_str} after {} ms without observing a completion",
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

#[expect(
    unsafe_code,
    reason = "windows file handles are thread-safe kernel objects"
)]
// SAFETY: `VolumeHandle` owns a Windows `HANDLE` to a kernel-managed file
// object plus immutable metadata (`volume` and `volume_data`). It contains no
// Rust references or unsynchronized interior mutability, so moving ownership
// to another thread does not invalidate any aliasing assumptions. Handle
// cleanup remains centralized in `Drop`.
unsafe impl Send for VolumeHandle {}

#[expect(
    unsafe_code,
    reason = "windows file handles are thread-safe kernel objects"
)]
// SAFETY: Shared references to `VolumeHandle` only expose immutable metadata or
// copy the raw `HANDLE`. The wrapper itself performs no unsynchronized mutable
// access, and Windows file handles are designed to be used from multiple
// threads.
unsafe impl Sync for VolumeHandle {}

/// NTFS volume data retrieved from `FSCTL_GET_NTFS_VOLUME_DATA`.
#[derive(Debug, Clone, Copy)]
pub struct NtfsVolumeData {
    /// Volume serial number.
    pub volume_serial_number: u64,
    /// NTFS major version (e.g. 3 for NTFS 3.1).
    pub ntfs_major_version: u16,
    /// NTFS minor version (e.g. 1 for NTFS 3.1).
    pub ntfs_minor_version: u16,
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
    /// NTFS formula: `TotalReserved * BytesPerCluster`.
    ///
    /// The MFT zone contribution is suppressed by setting
    /// `mft_zone_end = mft_zone_start` before computing `reserved_clusters`
    /// (see `mft_reader_init.hpp` lines 166-171).  So the effective formula
    /// is just `TotalReserved * BytesPerCluster`.
    #[must_use]
    pub const fn reserved_allocated_bytes(&self) -> u64 {
        self.total_reserved * self.bytes_per_cluster as u64
    }
}

impl VolumeHandle {
    /// Opens a volume for direct MFT reading.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if `CreateFileW` on the `\\.\<letter>:`
    /// path fails (typically `ERROR_ACCESS_DENIED` when the caller is not
    /// elevated), or if `FSCTL_GET_NTFS_VOLUME_DATA` cannot read the volume
    /// descriptor for the opened handle.
    #[expect(unsafe_code, reason = "FFI: windows API (CreateFileW)")]
    pub fn open(volume: char) -> Result<Self> {
        let normalized_volume = volume.to_ascii_uppercase();

        if !normalized_volume.is_ascii_alphabetic() {
            return Err(MftError::VolumeOpen {
                volume: normalized_volume,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid volume letter",
                ),
            });
        }

        let volume_path: Vec<u16> = format!("\\\\.\\{normalized_volume}:")
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // SAFETY: `volume_path` is UTF-16 and NUL-terminated for the duration of
        // the call, optional pointers are passed as `None`, and on success the
        // returned handle is owned by this function.
        let create_result = unsafe {
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

        let handle = match create_result {
            Ok(handle) => handle,
            Err(err) => {
                // `err.code().0` is an `i32` holding an HRESULT bit
                // pattern; `i32::cast_unsigned` reinterprets the same
                // bits as `u32` for comparison against documented Win32
                // error constants (which Microsoft publishes as u32).
                let code_unsigned = err.code().0.cast_unsigned();
                if code_unsigned == 0x8007_0005 {
                    return Err(MftError::InsufficientPrivileges);
                }
                return Err(MftError::VolumeOpen {
                    volume: normalized_volume,
                    source: std::io::Error::from_raw_os_error(err.code().0),
                });
            }
        };

        let volume_data = Self::get_ntfs_volume_data(handle, normalized_volume)?;

        Ok(Self {
            handle,
            volume: normalized_volume,
            volume_data,
        })
    }

    /// Retrieves NTFS volume data using `FSCTL_GET_NTFS_VOLUME_DATA`.
    #[expect(unsafe_code, reason = "FFI: windows API (DeviceIoControl)")]
    fn get_ntfs_volume_data(handle: HANDLE, volume: char) -> Result<NtfsVolumeData> {
        use windows::Win32::System::IO::DeviceIoControl;

        let mut buffer = NTFS_VOLUME_DATA_BUFFER::default();
        let mut bytes_returned: u32 = 0;

        // `size_of::<NTFS_VOLUME_DATA_BUFFER>()` is ~96 bytes — always fits u32.
        let ntfs_volume_data_buffer_size =
            u32::try_from(size_of::<NTFS_VOLUME_DATA_BUFFER>()).unwrap_or(u32::MAX);

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
                ntfs_volume_data_buffer_size,
                Some(&raw mut bytes_returned),
                None,
            )
        };

        if result.is_err() {
            return Err(MftError::NotNtfs(volume));
        }

        // Note: NTFS major/minor version requires NTFS_EXTENDED_VOLUME_DATA
        // (not available in NTFS_VOLUME_DATA_BUFFER).  Default to 0; callers
        // should use `query_ntfs_version()` if they need the actual version.
        //
        // Every `i64 -> u64` reinterpret below comes from an on-disk
        // NTFS count (sector / cluster / LCN / length) that the NTFS
        // on-disk format and Microsoft's `NTFS_VOLUME_DATA_BUFFER` MSDN
        // page document as non-negative.  `i64::cast_unsigned` /
        // `u64::cast_unsigned` are the stable Rust 1.87
        // exact-bit-pattern converters that replace the previous
        // `cast_sign_loss` expect.
        let volume_data = NtfsVolumeData {
            volume_serial_number: buffer.VolumeSerialNumber.cast_unsigned(),
            ntfs_major_version: 0,
            ntfs_minor_version: 0,
            number_of_sectors: buffer.NumberSectors.cast_unsigned(),
            total_clusters: buffer.TotalClusters.cast_unsigned(),
            free_clusters: buffer.FreeClusters.cast_unsigned(),
            total_reserved: buffer.TotalReserved.cast_unsigned(),
            bytes_per_sector: buffer.BytesPerSector,
            bytes_per_cluster: buffer.BytesPerCluster,
            bytes_per_file_record_segment: buffer.BytesPerFileRecordSegment,
            clusters_per_file_record_segment: buffer.ClustersPerFileRecordSegment,
            mft_valid_data_length: buffer.MftValidDataLength.cast_unsigned(),
            mft_start_lcn: buffer.MftStartLcn.cast_unsigned(),
            mft2_start_lcn: buffer.Mft2StartLcn.cast_unsigned(),
            mft_zone_start: buffer.MftZoneStart.cast_unsigned(),
            mft_zone_end: buffer.MftZoneEnd.cast_unsigned(),
        };
        Ok(volume_data)
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
    ///
    /// # Errors
    ///
    /// Returns `MftError::VolumeOpen` if `CreateFileW` fails.
    #[expect(unsafe_code, reason = "FFI: windows API (CreateFileW)")]
    pub fn open_overlapped_handle(&self) -> Result<HANDLE> {
        let volume = self.volume;
        let volume_path: Vec<u16> = format!("\\\\.\\{volume}:")
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

        handle.map_err(|err| MftError::VolumeOpen {
            volume,
            source: std::io::Error::from_raw_os_error(err.code().0),
        })
    }

    /// Opens a read handle to `X:\$MFT` for direct file-based MFT reading.
    ///
    /// This is used as a fallback on write-protected volumes where raw volume
    /// I/O (`\\.\X:`) fails with `ERROR_WRITE_PROTECT`.  Reading `$MFT` as a
    /// file works because the filesystem driver handles the VCN→LCN mapping
    /// internally.  Byte 0 of the returned handle corresponds to FRS 0.
    ///
    /// Automatically enables `SeBackupPrivilege` in the process token before
    /// opening — required for `FILE_FLAG_BACKUP_SEMANTICS` on NTFS metafiles.
    ///
    /// # Errors
    ///
    /// Returns `MftError::VolumeOpen` if `CreateFileW` fails.
    #[expect(unsafe_code, reason = "FFI: windows API (CreateFileW)")]
    pub fn open_mft_read_handle(&self) -> Result<HANDLE> {
        // Enable SeBackupPrivilege — required for $MFT access even as admin
        enable_backup_privilege();

        let volume = self.volume;
        let mft_path: Vec<u16> = format!("{volume}:\\$MFT")
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // SAFETY: `mft_path` is UTF-16 and NUL-terminated for the duration of
        // the call, optional pointers are passed as `None`, and ownership of
        // any returned handle is transferred to the caller.
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(mft_path.as_ptr()),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_SEQUENTIAL_SCAN,
                None,
            )
        };

        handle.map_err(|err| MftError::VolumeOpen {
            volume,
            source: std::io::Error::from_raw_os_error(err.code().0),
        })
    }

    /// Opens an unbuffered volume handle for direct I/O.
    ///
    /// On write-protected volumes the cache-manager path
    /// (`FILE_FLAG_SEQUENTIAL_SCAN`) fails with `ERROR_WRITE_PROTECT`.
    /// `FILE_FLAG_NO_BUFFERING` bypasses the cache manager entirely and
    /// issues I/O directly to the device driver, which only requires
    /// sector-aligned buffers and offsets (already guaranteed by
    /// [`AlignedBuffer`]).
    ///
    /// The caller is responsible for closing the returned handle.
    ///
    /// # Errors
    ///
    /// Returns `MftError::VolumeOpen` if `CreateFileW` fails.
    #[expect(unsafe_code, reason = "FFI: windows API (CreateFileW)")]
    pub fn open_unbuffered_handle(&self) -> Result<HANDLE> {
        let volume = self.volume;
        let volume_path: Vec<u16> = format!("\\\\.\\{volume}:")
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();

        // SAFETY: `volume_path` is UTF-16 and NUL-terminated for the duration
        // of the call, optional pointers are passed as `None`, and the
        // returned handle is transferred to the caller.
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(volume_path.as_ptr()),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_NO_BUFFERING,
                None,
            )
        };

        handle.map_err(|err| MftError::VolumeOpen {
            volume,
            source: std::io::Error::from_raw_os_error(err.code().0),
        })
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
    /// Returns [`MftError::Io`] if `SetFilePointerEx`/`ReadFile` on the
    /// volume handle fails, and [`MftError::InvalidData`] if the sector
    /// returns fewer bytes than `size_of::<NtfsBootSector>()` or decoding
    /// the boot-sector layout fails.
    #[expect(unsafe_code, reason = "FFI: windows API to read the boot sector")]
    pub fn read_boot_sector(&self) -> Result<NtfsBootSector> {
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

        let mut new_position = 0_i64;
        // SAFETY: `self.handle` is a live volume handle and `new_position`
        // points to writable stack storage for the duration of the call.
        unsafe { SetFilePointerEx(self.handle, 0, Some(&raw mut new_position), FILE_BEGIN) }?;

        let mut buffer = [0_u8; 512];
        let mut bytes_read = 0_u32;

        // SAFETY: `self.handle` is a live volume handle, `buffer` is a writable
        // 512-byte stack array, and `bytes_read` is a valid out-parameter.
        unsafe {
            ReadFile(
                self.handle,
                Some(&mut buffer),
                Some(&raw mut bytes_read),
                None,
            )
        }?;

        if bytes_read != 512 {
            return Err(MftError::BootSectorRead(format!(
                "Expected 512 bytes, got {bytes_read}"
            )));
        }

        let Ok((boot_sector, _)) = NtfsBootSector::read_from_prefix(&buffer) else {
            return Err(MftError::InvalidBootSector(
                "Unable to decode NTFS boot sector layout".to_owned(),
            ));
        };

        if !boot_sector.is_valid() {
            return Err(MftError::InvalidBootSector(
                "Invalid OEM ID (not NTFS)".to_owned(),
            ));
        }

        Ok(boot_sector)
    }

    /// Gets the MFT extents (data runs) using `FSCTL_GET_RETRIEVAL_POINTERS`.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if `CreateFileW` on `\\.\<letter>:\$MFT`
    /// or `DeviceIoControl(FSCTL_GET_RETRIEVAL_POINTERS, ..)` fails.
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
        let Ok(mft_handle) = (unsafe {
            CreateFileW(
                PCWSTR::from_raw(mft_path.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }) else {
            return Ok(vec![MftExtent {
                vcn: 0,
                cluster_count: self.volume_data.mft_valid_data_length
                    / u64::from(self.volume_data.bytes_per_cluster),
                lcn: self.volume_data.mft_start_lcn.cast_signed(),
            }]);
        };

        let _guard = HandleGuard(mft_handle);
        get_retrieval_pointers(mft_handle)
    }

    /// Gets the MFT bitmap which indicates which records are in use.
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if opening `\\.\<letter>:\$MFT::$BITMAP`,
    /// seeking to its extents, or reading bitmap bytes via `ReadFile` fails.
    pub fn get_mft_bitmap(&self) -> Result<MftBitmap> {
        self.get_mft_bitmap_internal(false)
    }

    /// Gets the MFT bitmap with optional verbose diagnostic output.
    ///
    /// # Errors
    ///
    /// Same failure modes as [`Self::get_mft_bitmap`]; additionally emits
    /// diagnostic tracing for partial reads before falling back to an
    /// all-valid bitmap.
    pub fn get_mft_bitmap_verbose(&self) -> Result<MftBitmap> {
        self.get_mft_bitmap_internal(true)
    }

    /// Open `$MFT::$BITMAP` and read the entire bitmap stream into memory.
    ///
    /// `verbose` controls whether progress is logged at `info!` (caller-driven
    /// telemetry) or `trace!` (silent path).
    ///
    /// # Errors
    ///
    /// Returns [`MftError::Io`] if `CreateFileW`, `GetFileSizeEx`, or
    /// `ReadFile` fail, or if the bitmap is empty / mis-sized.
    #[expect(
        unsafe_code,
        reason = "FFI: windows API (CreateFileW, GetFileSizeEx, ReadFile)"
    )]
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "open + size + retrieval-pointers + per-extent read + verbose-logging branches form a single bitmap-load operation; splitting them adds plumbing without simplifying control flow"
    )]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "every failure branch returns `Ok(MftBitmap::new_all_valid(...))` (graceful fallback); the `Result<_>` signature documents the fallible Win32 call surface and aligns with `get_mft_bitmap` / `get_mft_bitmap_verbose` callers"
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
        let bitmap_handle = match unsafe {
            CreateFileW(
                PCWSTR::from_raw(bitmap_path.as_ptr()),
                FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_NO_BUFFERING,
                None,
            )
        } {
            Ok(handle) => {
                if verbose {
                    tracing::info!(volume = %self.volume, handle = ?handle, "CreateFileW for MFT bitmap succeeded");
                }
                handle
            }
            Err(err) => {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        error = ?err,
                        "CreateFileW for MFT bitmap failed; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(frs_to_usize(
                    self.estimated_record_count(),
                )));
            }
        };

        let _guard = HandleGuard(bitmap_handle);

        let mut file_size: i64 = 0;
        // SAFETY: `bitmap_handle` is a live file handle and `file_size` points
        // to writable stack storage for the duration of the call.
        unsafe {
            if let Err(err) = GetFileSizeEx(bitmap_handle, &raw mut file_size) {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        error = ?err,
                        "GetFileSizeEx for MFT bitmap failed; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(frs_to_usize(
                    self.estimated_record_count(),
                )));
            }
        }

        if verbose {
            tracing::info!(volume = %self.volume, file_size, "Retrieved MFT bitmap size");
        }

        let extents = match get_retrieval_pointers(bitmap_handle) {
            Ok(extents) if !extents.is_empty() => {
                if verbose {
                    tracing::info!(volume = %self.volume, extent_count = extents.len(), "Retrieved MFT bitmap extents");
                    for (i, ext) in extents.iter().enumerate().take(5) {
                        tracing::info!(
                            volume = %self.volume,
                            extent_index = i,
                            vcn = ext.vcn,
                            cluster_count = ext.cluster_count,
                            lcn = ext.lcn,
                            "MFT bitmap extent sample"
                        );
                    }
                    if extents.len() > 5 {
                        tracing::info!(
                            volume = %self.volume,
                            additional_extent_count = extents.len() - 5,
                            "Additional MFT bitmap extents omitted from verbose sample"
                        );
                    }
                }
                extents
            }
            Ok(_) => {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        "get_retrieval_pointers returned no MFT bitmap extents; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(frs_to_usize(
                    self.estimated_record_count(),
                )));
            }
            Err(err) => {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        error = ?err,
                        "get_retrieval_pointers for MFT bitmap failed; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(frs_to_usize(
                    self.estimated_record_count(),
                )));
            }
        };

        let bytes_per_cluster = self.volume_data.bytes_per_cluster;
        let total_clusters: u64 = extents.iter().map(|ext| ext.cluster_count).sum();
        let aligned_size = frs_to_usize(total_clusters * u64::from(bytes_per_cluster));
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
            let extent_bytes = frs_to_usize(extent.cluster_count * u64::from(bytes_per_cluster));

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
                if let Err(err) = SetFilePointerEx(
                    self.handle,
                    byte_offset,
                    Some(&raw mut new_position),
                    FILE_BEGIN,
                ) {
                    if verbose {
                        tracing::warn!(
                            volume = %self.volume,
                            extent_index = i,
                            byte_offset,
                            error = ?err,
                            "SetFilePointerEx for MFT bitmap extent failed; falling back to all-valid bitmap"
                        );
                    }
                    return Ok(MftBitmap::new_all_valid(frs_to_usize(
                        self.estimated_record_count(),
                    )));
                }
            }

            let mut bytes_read: u32 = 0;
            let Some(extent_window) = buffer.get_mut(buffer_offset..buffer_offset + extent_bytes)
            else {
                if verbose {
                    tracing::warn!(
                        volume = %self.volume,
                        extent_index = i,
                        buffer_offset,
                        extent_bytes,
                        buffer_len = buffer.len(),
                        "MFT bitmap extent exceeds buffer size; falling back to all-valid bitmap"
                    );
                }
                return Ok(MftBitmap::new_all_valid(frs_to_usize(
                    self.estimated_record_count(),
                )));
            };
            // SAFETY: `self.handle` is a live volume handle, the slice points to
            // a contiguous writable region of `extent_bytes`, and `bytes_read`
            // is a valid out-parameter for the duration of the read.
            unsafe {
                if let Err(err) = ReadFile(
                    self.handle,
                    Some(extent_window),
                    Some(&raw mut bytes_read),
                    None,
                ) {
                    if verbose {
                        tracing::warn!(
                            volume = %self.volume,
                            extent_index = i,
                            extent_bytes,
                            error = ?err,
                            "ReadFile for MFT bitmap extent failed; falling back to all-valid bitmap"
                        );
                    }
                    return Ok(MftBitmap::new_all_valid(frs_to_usize(
                        self.estimated_record_count(),
                    )));
                }
            }

            if verbose && i < 3 {
                tracing::info!(volume = %self.volume, extent_index = i, bytes_read, "Read MFT bitmap extent bytes");
                if i == 0 && bytes_read > 0 {
                    let sample_end = buffer_offset + 32.min(u32_as_usize(bytes_read));
                    let sample: Vec<String> = buffer
                        .get(buffer_offset..sample_end)
                        .unwrap_or(&[])
                        .iter()
                        .map(|byte| format!("{byte:02X}"))
                        .collect();
                    tracing::info!(
                        volume = %self.volume,
                        extent_index = i,
                        sample_hex = %sample.join(" "),
                        "MFT bitmap first-byte sample"
                    );
                }
            }

            buffer_offset += u32_as_usize(bytes_read);
        }

        if verbose {
            tracing::info!(
                volume = %self.volume,
                total_bytes_read = buffer_offset,
                file_size,
                "Completed MFT bitmap read; truncating to reported file size"
            );
            let file_size_usize = usize::try_from(file_size).unwrap_or(0);
            let all_ff = buffer
                .iter()
                .take(file_size_usize)
                .all(|&byte| byte == 0xFF);
            let all_00 = buffer
                .iter()
                .take(file_size_usize)
                .all(|&byte| byte == 0x00);
            tracing::info!(volume = %self.volume, all_ff, all_00, "Computed MFT bitmap byte-pattern summary");
        }

        buffer.truncate(usize::try_from(file_size).unwrap_or(0));
        Ok(MftBitmap::from_bytes(buffer))
    }
}

impl Drop for VolumeHandle {
    #[expect(unsafe_code, reason = "FFI: windows API (CloseHandle)")]
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: `VolumeHandle` owns this valid handle and closes it once
            // during drop after all safe borrows have ended.
            _ = unsafe { CloseHandle(self.handle) };
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
            _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Enables `SeBackupPrivilege` in the current process token.
///
/// `FILE_FLAG_BACKUP_SEMANTICS` only bypasses NTFS security checks when the
/// calling thread's token has `SeBackupPrivilege` **enabled** (not just
/// present).  Administrator tokens include it but it's disabled by default.
///
/// This is required to open `$MFT` for reading on write-protected volumes.
/// The privilege is process-wide and persists for the lifetime of the process,
/// so calling this multiple times is harmless.
fn enable_backup_privilege() {
    let Some(token) = open_current_process_token() else {
        return;
    };
    let Some(luid) = lookup_backup_privilege_luid() else {
        unsafe_close_token(token);
        return;
    };

    enable_privilege_with_token(token, luid);
}

/// Open the current process's token with `TOKEN_ADJUST_PRIVILEGES` rights.
///
/// Returns `None` (and logs a debug line) when the underlying Win32 call
/// fails — the only legitimate cause is non-elevated callers, which is
/// expected for the `MftReader::new` fast path.
#[expect(unsafe_code, reason = "FFI: GetCurrentProcess + OpenProcessToken")]
fn open_current_process_token() -> Option<HANDLE> {
    use windows::Win32::Security::TOKEN_ADJUST_PRIVILEGES;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess()` returns a constant pseudo-handle and has
    // no preconditions.
    let current_process = unsafe { GetCurrentProcess() };
    // SAFETY: `current_process` is a valid pseudo-handle and `token` is
    // valid writable stack storage for the call duration.
    if unsafe { OpenProcessToken(current_process, TOKEN_ADJUST_PRIVILEGES, &raw mut token) }
        .is_err()
    {
        tracing::debug!("Could not open process token for privilege adjustment");
        return None;
    }
    Some(token)
}

/// Look up the LUID for `SeBackupPrivilege`.  Returns `None` when the call
/// fails (extremely rare; would indicate a missing privilege constant).
#[expect(
    unsafe_code,
    reason = "FFI: LookupPrivilegeValueW with caller-owned LUID storage"
)]
fn lookup_backup_privilege_luid() -> Option<windows::Win32::Foundation::LUID> {
    use windows::Win32::Foundation::LUID;
    use windows::Win32::Security::{LookupPrivilegeValueW, SE_BACKUP_NAME};

    let mut luid = LUID::default();
    // SAFETY: `SE_BACKUP_NAME` is a static wide string constant and `luid`
    // is valid writable stack storage for the call duration.
    if unsafe { LookupPrivilegeValueW(None, SE_BACKUP_NAME, &raw mut luid) }.is_err() {
        tracing::debug!("Could not look up SeBackupPrivilege LUID");
        return None;
    }
    Some(luid)
}

/// Adjust `token` so `luid` is enabled, then close `token`.  Logs the
/// outcome at info / debug level — this function is best-effort and never
/// returns an error to the caller.
#[expect(unsafe_code, reason = "FFI: AdjustTokenPrivileges + CloseHandle")]
fn enable_privilege_with_token(token: HANDLE, luid: windows::Win32::Foundation::LUID) {
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_PRIVILEGES,
    };

    let tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };

    // SAFETY: `token` was opened with `TOKEN_ADJUST_PRIVILEGES`, `tp` is a
    // valid `TOKEN_PRIVILEGES` struct, and no previous-state buffer is
    // requested.
    let result = unsafe { AdjustTokenPrivileges(token, false, Some(&raw const tp), 0, None, None) };

    // SAFETY: `token` was opened by the caller and is closed exactly once.
    _ = unsafe { CloseHandle(token) };

    match result {
        Ok(()) => tracing::info!("✅ SeBackupPrivilege enabled"),
        Err(err) => tracing::debug!(error = %err, "Could not enable SeBackupPrivilege"),
    }
}

/// Close a privilege-helper token, logging but not propagating any failure.
#[expect(
    unsafe_code,
    reason = "FFI: CloseHandle on a token opened by open_current_process_token"
)]
fn unsafe_close_token(token: HANDLE) {
    // SAFETY: caller passes a token returned from
    // `open_current_process_token` that has not yet been closed.
    _ = unsafe { CloseHandle(token) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_wait_error_maps_aborted_waits_to_cancelled() {
        let error = classify_wait_error_code("read_all_index", 995, "wait aborted");

        assert!(matches!(error, MftError::Cancelled {
            operation: "read_all_index",
            ..
        }));
    }

    #[test]
    fn classify_wait_error_maps_other_wait_failures_to_wait_failed() {
        let error = classify_wait_error_code("read_all_index", 123, "wait failed");

        assert!(matches!(error, MftError::WaitFailed {
            operation: "read_all_index",
            ..
        }));
    }

    #[test]
    fn wait_deadline_helper_builds_timeout_error() {
        let error = wait_deadline_exceeded(
            "read_all_index",
            Duration::from_secs(31),
            "no completions arrived",
        );

        assert!(matches!(error, MftError::Timeout {
            operation: "read_all_index",
            ..
        }));
    }
}
