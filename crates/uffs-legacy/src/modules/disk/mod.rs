// ├── disk/                       # Disk-level operations (raw disk reading,
// block devices, etc.) │   ├── mod.rs                  # Disk operations module
// entry point │   ├── ntfs_disk.rs            # NTFS raw disk reader (handling
// partitioning, MFT parsing) │   ├── ext_disk.rs             # EXT raw disk
// reader │   ├── macfs_disk.rs           # macOS raw disk reader (APFS, HFS+)
// │   └── common.rs               # Shared disk-level utilities (sector
// reading, caching, etc.)
pub(crate) mod common;
pub mod drive_info;
pub(crate) mod ext_disk;
pub(crate) mod macfs_disk;
pub(crate) mod ntfs_disk;

// Windows-only WMI modules
#[cfg(windows)]
pub mod wim_defrag_analysis;
#[cfg(windows)]
pub mod wim_disk_quota;
#[cfg(windows)]
pub mod wmi_disk_drive;
#[cfg(windows)]
pub mod wmi_disk_partition;
#[cfg(windows)]
pub mod wmi_encryptable_volume;
#[cfg(windows)]
pub mod wmi_logical_disk;
#[cfg(windows)]
pub mod wmi_mount_point;
#[cfg(windows)]
pub mod wmi_msft_disk;
#[cfg(windows)]
pub mod wmi_msft_partition;
#[cfg(windows)]
pub mod wmi_perf_disk_physical_disk;
#[cfg(windows)]
pub mod wmi_physical_media;
#[cfg(windows)]
pub mod wmi_quota_setting;
#[cfg(windows)]
pub mod wmi_shadow_copy;
#[cfg(windows)]
pub mod wmi_volume;
#[cfg(windows)]
pub mod wmi_volume_change_event;
#[cfg(windows)]
pub mod wmi_volume_quota;
