use std::mem::size_of;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::core::PCWSTR;

use super::volume::HandleGuard;

/// Checks if the current process has Administrator privileges.
///
/// MFT reading requires Administrator privileges or `SE_BACKUP_PRIVILEGE`.
#[must_use]
#[expect(
    unsafe_code,
    reason = "FFI: windows API (OpenProcessToken, GetTokenInformation)"
)]
pub fn is_elevated() -> bool {
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token_handle = HANDLE::default();
    // SAFETY: `GetCurrentProcess()` returns the current pseudo-handle, and
    // `token_handle` points to writable storage for the returned token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle) }.is_err() {
        return false;
    }

    let _token_guard = HandleGuard(token_handle);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut return_length = 0_u32;

    // SAFETY: `token_handle` was successfully opened above, `elevation`
    // provides writable storage matching the advertised buffer size, and
    // `return_length` is a valid out-parameter.
    let result = unsafe {
        GetTokenInformation(
            token_handle,
            TokenElevation,
            Some(core::ptr::from_mut(&mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut return_length,
        )
    };

    result.is_ok() && elevation.TokenIsElevated != 0
}

/// Returns the path to the volume root (e.g., "C:\").
#[must_use]
pub fn volume_root_path(volume: char) -> PathBuf {
    PathBuf::from(format!("{}:\\", volume.to_ascii_uppercase()))
}

/// Infer drive letter from a file path.
#[must_use]
pub fn infer_drive_from_path(path: &Path) -> Option<char> {
    use std::path::{Component, Prefix};

    if let Some(Component::Prefix(prefix)) = path.components().next() {
        match prefix.kind() {
            Prefix::Disk(drive_byte) | Prefix::VerbatimDisk(drive_byte) => {
                return Some((drive_byte as char).to_ascii_uppercase());
            }
            _ => {}
        }
    }

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
#[must_use]
#[expect(unsafe_code, reason = "FFI: windows API (GetLogicalDrives)")]
pub fn detect_ntfs_drives() -> Vec<char> {
    use windows::Win32::Storage::FileSystem::GetLogicalDrives;

    let mut ntfs_drives = Vec::new();

    // SAFETY: `GetLogicalDrives` takes no pointers and returns a bitmask by
    // value, so there are no aliasing or lifetime preconditions to satisfy.
    let drive_mask = unsafe { GetLogicalDrives() };

    if drive_mask == 0 {
        return ntfs_drives;
    }

    for i in 0..26_u32 {
        if (drive_mask & (1 << i)) != 0 {
            let drive_letter = char::from(b'A' + i as u8);

            if is_ntfs_volume(drive_letter) {
                ntfs_drives.push(drive_letter);
            }
        }
    }

    ntfs_drives
}

/// Checks if a drive is an NTFS volume.
#[expect(
    unsafe_code,
    reason = "FFI: windows API (GetDriveTypeW, GetVolumeInformationW)"
)]
fn is_ntfs_volume(drive_letter: char) -> bool {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW};

    let root_path: Vec<u16> = format!("{}:\\", drive_letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `root_path` is UTF-16 and NUL-terminated for the duration of the
    // call.
    let drive_type = unsafe { GetDriveTypeW(PCWSTR(root_path.as_ptr())) };

    if drive_type != 2 && drive_type != 3 {
        return false;
    }

    let mut fs_name_buffer: [u16; 32] = [0; 32];

    // SAFETY: `root_path` is UTF-16 and NUL-terminated, and `fs_name_buffer`
    // points to writable storage for the filesystem name returned by Windows.
    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR(root_path.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs_name_buffer),
        )
    };

    if result.is_err() {
        return false;
    }

    let fs_name = String::from_utf16_lossy(&fs_name_buffer);
    let fs_name = fs_name.trim_end_matches('\0');

    fs_name == "NTFS"
}

/// Checks if a volume is read-only.
#[cfg(windows)]
#[must_use]
#[expect(unsafe_code, reason = "FFI: windows API (GetVolumeInformationW)")]
pub fn is_volume_read_only(drive_letter: char) -> bool {
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

    const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

    let root_path: Vec<u16> = format!("{}:\\", drive_letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut fs_flags: u32 = 0;

    // SAFETY: `root_path` is UTF-16 and NUL-terminated, and `fs_flags` points
    // to writable storage for the returned filesystem flags.
    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR(root_path.as_ptr()),
            None,
            None,
            None,
            Some(&mut fs_flags),
            None,
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
    #[must_use]
    pub const fn optimal_chunk_size(&self) -> usize {
        match self {
            Self::Nvme => 4 * 1024 * 1024,
            Self::Ssd => 2 * 1024 * 1024,
            Self::Hdd => 1024 * 1024,
            Self::Unknown => 1024 * 1024,
        }
    }

    /// Returns the optimal number of prefetch buffers.
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
    #[must_use]
    pub const fn optimal_concurrency(&self) -> usize {
        match self {
            Self::Nvme => 32,
            Self::Ssd => 8,
            Self::Hdd => 4,
            Self::Unknown => 4,
        }
    }

    /// Returns the optimal I/O concurrency for HDD based on MFT fragmentation.
    #[must_use]
    pub const fn optimal_concurrency_for_hdd(extent_count: usize) -> usize {
        if extent_count > 50 {
            2
        } else if extent_count > 20 {
            4
        } else {
            6
        }
    }

    /// Returns the optimal I/O chunk size for this drive type.
    #[must_use]
    pub const fn optimal_io_size(&self) -> usize {
        self.optimal_chunk_size()
    }

    /// Returns true if this is a high-performance drive (SSD or NVMe).
    #[must_use]
    pub const fn is_high_performance(&self) -> bool {
        matches!(self, Self::Nvme | Self::Ssd)
    }

    /// Returns true if this drive benefits from parallel parsing.
    #[must_use]
    pub const fn benefits_from_parallel_parsing(&self) -> bool {
        matches!(self, Self::Nvme)
    }
}

/// Detects whether a drive is NVMe, SSD, or HDD.
#[must_use]
#[expect(
    unsafe_code,
    reason = "FFI: windows API (CreateFileW, DeviceIoControl) and unaligned descriptor read"
)]
pub fn detect_drive_type(drive_letter: char) -> DriveType {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    const STORAGE_DEVICE_PROPERTY: u32 = 0;
    const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;
    const PROPERTY_STANDARD_QUERY: u32 = 0;
    const BUS_TYPE_NVME: u32 = 17;

    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: u32,
        query_type: u32,
        additional_parameters: [u8; 1],
    }

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
        bus_type: u32,
        raw_properties_length: u32,
        raw_device_properties: [u8; 1],
    }

    #[repr(C)]
    struct DeviceSeekPenaltyDescriptor {
        version: u32,
        size: u32,
        incurs_seek_penalty: u8,
    }

    let drive_path: Vec<u16> = format!("\\\\.\\{}:", drive_letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `drive_path` is UTF-16 and NUL-terminated for the duration of the
    // call, optional pointers are `None`, and any returned handle is wrapped in
    // `HandleGuard` before use.
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

    let drive_classification = {
        let _handle_guard = HandleGuard(handle);

        let is_nvme = {
            let query = StoragePropertyQuery {
                property_id: STORAGE_DEVICE_PROPERTY,
                query_type: PROPERTY_STANDARD_QUERY,
                additional_parameters: [0],
            };

            let mut buffer = [0_u8; 1024];
            let mut bytes_returned: u32 = 0;

            // SAFETY: `handle` is a live device handle, `query` contains the
            // exact input bytes Windows expects for this IOCTL, `buffer` is a
            // writable output buffer of the advertised length, and
            // `bytes_returned` is a valid out-parameter.
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
                // SAFETY: `bytes_returned` proves the prefix needed for
                // `StorageDeviceDescriptor` is present in `buffer`, the struct is
                // `repr(C)` plain data, and `read_unaligned` handles the buffer's
                // byte alignment.
                let descriptor = unsafe {
                    core::ptr::read_unaligned(buffer.as_ptr().cast::<StorageDeviceDescriptor>())
                };
                descriptor.bus_type == BUS_TYPE_NVME
            } else {
                false
            }
        };

        if is_nvme {
            return DriveType::Nvme;
        }

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

        // SAFETY: `handle` is a live device handle, `query` and `descriptor`
        // point to initialized storage matching the advertised sizes, and
        // `bytes_returned` is a valid out-parameter.
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

        if result.is_ok() && bytes_returned >= size_of::<DeviceSeekPenaltyDescriptor>() as u32 {
            Some(if descriptor.incurs_seek_penalty == 0 {
                DriveType::Ssd
            } else {
                DriveType::Hdd
            })
        } else {
            None
        }
    };

    drive_classification.unwrap_or_else(|| detect_drive_type_via_trim(drive_letter))
}

/// Fallback detection using TRIM support (SSDs support TRIM).
#[expect(
    unsafe_code,
    reason = "FFI: windows API (CreateFileW, DeviceIoControl) for trim detection"
)]
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

    // SAFETY: `drive_path` is UTF-16 and NUL-terminated for the duration of the
    // call, optional pointers are `None`, and any returned handle is wrapped in
    // `HandleGuard` before use.
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

    let _handle_guard = HandleGuard(handle);

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

    // SAFETY: `handle` is a live device handle, `query` and `descriptor`
    // point to initialized storage matching the advertised sizes, and
    // `bytes_returned` is a valid out-parameter.
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
