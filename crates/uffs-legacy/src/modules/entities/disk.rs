use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::to_string_pretty;
use sysinfo::DiskKind;
#[cfg(windows)]
use windows::Win32::System::Ioctl::MEDIA_TYPE;

use crate::modules::utils::format_utils::format_size;
use crate::modules::utils::time_utils::format_duration;

/// Define a wrapper around `MEDIA_TYPE` (Windows only)
#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HashableMediaType(pub MEDIA_TYPE);

#[cfg(windows)]
impl Serialize for HashableMediaType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i32(self.0.0) // Serialize the inner i32
    }
}

#[cfg(windows)]
impl<'de> Deserialize<'de> for HashableMediaType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = i32::deserialize(deserializer)?;
        Ok(HashableMediaType(MEDIA_TYPE(value)))
    }
}

#[cfg(windows)]
impl Hash for HashableMediaType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.0.hash(state); // Hash the inner i32 of MEDIA_TYPE
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DriveInfo {
    // General fields common to all platforms
    pub(crate) drive_type: DriveType, // Type of drive (e.g., HDD, SSD, Removable)
    pub(crate) drive_name: OsString,  // Name of the drive (e.g., "C:", "/dev/sda")
    pub(crate) file_system_type: Option<FileSystemType>, /* File system type (e.g., NTFS, ext4,
                                       * APFS) */
    pub(crate) root_path: OsString, // Root path where the drive is mounted (e.g., "/", "C:\\")
    pub(crate) total_space: usize,  // Total space on the drive in bytes
    pub(crate) available_space: usize, // Available space on the drive in bytes
    pub(crate) is_removable: bool,  // Whether the drive is removable (e.g., USB drive)
    pub(crate) num_files: usize,    // Total number of files on the drive
    pub(crate) num_dirs: usize,     // Total number of directories on the drive
    pub(crate) time_nanoseconds: u64, // Time of the last scan or data collection in nanoseconds
    // pub(crate) directory_reader: Box<dyn DirectoryReader + Send + Sync + 'static>,

    // Common cross-platform fields (optional, populated when available)
    pub(crate) uuid: Option<String>, /* Universally unique identifier for the filesystem (common
                                      * across platforms) */
    pub(crate) serial_number: Option<String>, /* Serial number of the physical drive (common
                                               * across platforms) */
    pub(crate) mount_point: Option<OsString>, /* Path where the drive is mounted (e.g.,
                                               * "/mnt/disk", "C:\\") */
    pub(crate) sector_size: Option<u64>, // Size of sectors on the drive in bytes (e.g., 512, 4096)
    pub(crate) block_size: Option<u64>,  /* Size of file system blocks, important for storage
                                          * allocation */
    pub(crate) cylinders: Option<i64>,
    pub(crate) tracks_per_cylinder: Option<u32>,
    pub(crate) sectors_per_track: Option<u32>,
    pub(crate) bytes_per_sector: Option<u32>,
    #[cfg(windows)]
    pub(crate) media_type: Option<HashableMediaType>,

    // Windows-specific fields (optional, only populated on Windows systems)
    pub(crate) volume_serial_number: Option<String>, /* Unique identifier for the volume on
                                                      * Windows (e.g., "ABC123") */
    pub(crate) volume_name: Option<OsString>, /* Unique identifier for the volume on Windows
                                               * (e.g., "ABC123") */
    pub(crate) ntfs_version: Option<String>, // Version of the NTFS file system (e.g., "NTFS 3.1")
    pub(crate) physical_device_path: Option<OsString>, /* Physical device path (useful for
                                              * identifying the device in Windows) */
    pub(crate) is_bitlocker_encrypted: Option<bool>, /* Whether the drive is encrypted with
                                                      * BitLocker */
    pub(crate) max_component_length: Option<u32>, /* Maximum allowed length for a single
                                                   * component (file or directory name) in the
                                                   * file path, as defined by the file system
                                                   * (e.g., NTFS allows 255 characters per
                                                   * name). None if not applicable or
                                                   * unavailable. */

    // Different drive representations (Windows-specific)
    pub(crate) drive_letter: Option<OsString>, // Drive letter representation (e.g., D:)
    pub(crate) drive_root: Option<OsString>,   // Root directory (e.g., D:\\)
    pub(crate) dos_device_name: Option<OsString>, /* DOS device name (e.g.,
                                                * \\Device\\HarddiskVolume6) */
    pub(crate) device_path: Option<OsString>, // Device path (e.g., \\.\D:)
    pub(crate) volume_guid_path: Option<OsString>, // Volume GUID path (e.g., \\?\Volume{GUID}\)
    pub(crate) mounted_folder_path: Option<OsString>, /* Mounted folder path (e.g.,
                                               * C:\Mount\MyDrive\) */
    pub(crate) unc_path: Option<OsString>, /* UNC path for network drives (e.g.,
                                            * \\ServerName\SharedFolder\) */

    // macOS-specific fields (optional, only populated on macOS systems)
    pub(crate) device_identifier: Option<OsString>, /* Device identifier on macOS (e.g.,
                                                     * "/dev/disk1") */
    pub(crate) container_size: Option<usize>, // Size of the APFS container if the drive uses APFS
    pub(crate) volume_role: Option<OsString>, /* Role of the volume in an APFS container (e.g.,
                                               * "System", "Data") */
    pub(crate) apfs_version: Option<OsString>, // Version of the APFS file system (if applicable)

    // Linux/Unix-specific fields (optional, only populated on Linux/Unix systems)
    pub(crate) device_node: Option<OsString>, /* Device file associated with the drive (e.g.,
                                               * "/dev/sda1") */
    pub(crate) raid_info: Option<RaidInfo>, /* RAID information, if the drive is part of a RAID
                                             * array (e.g., mdadm) */
    pub(crate) inode_count: Option<(usize, usize)>, /* Number of inodes (used, total) on the
                                                     * file system */
    pub(crate) mount_options: Option<Vec<OsString>>, /* Mount options (e.g., "ro", "rw",
                                                      * "nosuid", "noatime") */
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct RaidInfo {
    pub(crate) raid_level: String, // RAID level (e.g., "RAID 0", "RAID 1", "RAID 5")
    pub(crate) num_devices: usize, // Number of devices in the RAID array
    pub(crate) active_devices: usize, // Number of active (online) devices in the RAID array
    pub(crate) degraded: bool,     /* Whether the array is degraded (e.g., one or more failed
                                    * disks in RAID 5) */
    pub(crate) spare_devices: usize, // Number of spare devices (if any) in the array
    pub(crate) total_capacity: usize, // Total capacity of the RAID array in bytes
    pub(crate) available_capacity: usize, // Available capacity in bytes
    pub(crate) array_state: String,  /* State of the RAID array (e.g., "active", "degraded",
                                      * "recovering") */
}

impl DriveInfo {
    pub fn print_as_json(&self) {
        // Convert DriveInfo to a pretty-printed JSON string
        match to_string_pretty(self) {
            Ok(json) => println!("{}", json),
            Err(e) => eprintln!("Error serializing DriveInfo: {}", e),
        }
    }
}

impl DriveInfo {
    pub(crate) fn format_with_lengths(&self, lengths: &ColumnLengths) -> String {
        format!(
            "{:<path_length$}   {:<type_length$}   {:>total_space_length$}   {:>available_space_length$}   {:>files_length$}   {:>dirs_length$}   {:>time_seconds_length$.3}  {:>time_length$}",
            self.root_path.display(),
            format!("{:?}", self.drive_type),
            format_size(self.total_space),
            format_size(self.available_space),
            self.num_files,
            self.num_dirs,
            Duration::from_nanos(self.time_nanoseconds).as_secs(),
            format_duration(Duration::from_nanos(self.time_nanoseconds)),
            path_length = lengths.path_length,
            type_length = lengths.type_length,
            total_space_length = lengths.total_space_length,
            available_space_length = lengths.available_space_length,
            files_length = lengths.files_length,
            dirs_length = lengths.dirs_length,
            time_seconds_length = lengths.time_seconds_length,
            time_length = lengths.time_length,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum DriveType {
    HDD,
    SSD,
    Other,
}

impl From<DiskKind> for DriveType {
    fn from(kind: DiskKind) -> Self {
        match kind {
            DiskKind::HDD => DriveType::HDD,
            DiskKind::SSD => DriveType::SSD,
            _ => DriveType::Other, // Covers other variants, like Unknown or Removable
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ColumnLengths {
    pub(crate) path_length: usize,
    pub(crate) type_length: usize,
    pub(crate) total_space_length: usize,
    pub(crate) available_space_length: usize,
    pub(crate) files_length: usize,
    pub(crate) dirs_length: usize,
    pub(crate) time_seconds_length: usize,
    pub(crate) time_length: usize,
}

impl Default for ColumnLengths {
    fn default() -> Self {
        ColumnLengths {
            path_length: 20,
            type_length: 8,
            total_space_length: 10,
            available_space_length: 15,
            files_length: 12,
            dirs_length: 12,
            time_seconds_length: 10,
            time_length: 13,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum FileSystemType {
    // Windows File Systems
    NTFS,  // New Technology File System (default for Windows)
    FAT,   // File Allocation Table (legacy, used in older systems and small partitions)
    FAT32, // File Allocation Table (supports larger volumes than FAT)
    ExFAT, // Extended FAT (designed for larger partitions, widely used on USB drives)
    UDF,   // Universal Disk Format (for DVDs, Blu-rays, and optical media)
    CDFS,  // Compact Disc File System (for CD-ROMs)
    ReFS,  // Resilient File System (Windows Server, data integrity and scalability)
    HPFS,  // High-Performance File System (OS/2, rare in modern systems)

    // Linux File Systems
    Ext2,     // Second Extended Filesystem (older, still in use for small partitions)
    Ext3,     // Third Extended Filesystem (adds journaling, commonly used)
    Ext4,     // Fourth Extended Filesystem (modern, default for many Linux distros)
    XFS,      // High-performance file system (Linux, commonly used for large volumes)
    Btrfs,    // B-tree File System (modern Linux file system with snapshots and redundancy)
    JFS,      // Journaled File System (IBM’s file system for Linux, used in some distros)
    ReiserFS, // Reiser File System (an older Linux file system, now mostly deprecated)
    SquashFS, // Read-only compressed file system (used in embedded systems and live CDs)
    ZFS,      /* Zettabyte File System (highly resilient, used in Linux, OpenSolaris, FreeBSD,
               * and NAS systems) */

    // macOS File Systems
    HFS,     // Hierarchical File System (older Mac file system)
    HFSPlus, // HFS+ (used in older macOS versions, replaced by APFS)
    APFS,    // Apple File System (default file system for macOS since 2017)

    // BSD and Unix-like File Systems
    FFS, // Fast File System (used by BSD systems)

    // Network File Systems
    NFS,       // Network File System (widely used for network file sharing)
    SMB,       // Server Message Block (used for file sharing in Windows, also supported by Linux)
    CIFS,      // Common Internet File System (a dialect of SMB, used for network sharing)
    AFP,       // Apple Filing Protocol (used for file sharing on older Mac systems)
    GlusterFS, // Gluster File System (distributed file system, used for large storage clusters)
    Ceph,      // Ceph File System (distributed, resilient file system used for large clusters)

    // Special-Purpose and Legacy File Systems
    ISO9660, // File system for optical media (CD-ROMs, DVDs)
    JFFS2,   // Journaling Flash File System 2 (used in flash memory devices)
    YAFFS,   // Yet Another Flash File System (for NAND flash memory)
    LogFS,   // Log-structured File System (used in embedded systems)
    NILFS,   /* New Implementation of a Log-structured File System (designed for continuous
              * snapshots) */
    F2FS, // Flash-Friendly File System (optimized for NAND flash storage)
    AFS,  // Andrew File System (used in distributed environments)

    // Other Linux/MacOS/Unix-Like File Systems
    GFS2,  // Global File System 2 (clustered file system, used in Linux)
    OCFS2, // Oracle Cluster File System 2 (used in Linux for clustering)

    // Other/Unknown
    Unknown(String), // For unrecognized or custom file systems
}

impl FileSystemType {
    /// Maps a file system name string to the FileSystemType enum
    pub fn from_name(file_system_name: &str) -> FileSystemType {
        match file_system_name {
            // Windows File Systems
            "NTFS" => FileSystemType::NTFS,
            "FAT" => FileSystemType::FAT,
            "FAT32" => FileSystemType::FAT32,
            "exFAT" => FileSystemType::ExFAT,
            "UDF" => FileSystemType::UDF,
            "CDFS" => FileSystemType::CDFS,
            "ReFS" => FileSystemType::ReFS,
            "HPFS" => FileSystemType::HPFS,

            // Linux File Systems
            "Ext2" => FileSystemType::Ext2,
            "Ext3" => FileSystemType::Ext3,
            "Ext4" => FileSystemType::Ext4,
            "XFS" => FileSystemType::XFS,
            "Btrfs" => FileSystemType::Btrfs,
            "JFS" => FileSystemType::JFS,
            "ReiserFS" => FileSystemType::ReiserFS,
            "SquashFS" => FileSystemType::SquashFS,
            "ZFS" => FileSystemType::ZFS,

            // macOS File Systems
            "HFS" => FileSystemType::HFS,
            "HFS+" => FileSystemType::HFSPlus,
            "APFS" => FileSystemType::APFS,

            // BSD and Unix-like File Systems
            "FFS" => FileSystemType::FFS,

            // Network File Systems
            "NFS" => FileSystemType::NFS,
            "SMB" => FileSystemType::SMB,
            "CIFS" => FileSystemType::CIFS,
            "AFP" => FileSystemType::AFP,
            "GlusterFS" => FileSystemType::GlusterFS,
            "Ceph" => FileSystemType::Ceph,

            // Special-Purpose File Systems
            "ISO9660" => FileSystemType::ISO9660,
            "JFFS2" => FileSystemType::JFFS2,
            "YAFFS" => FileSystemType::YAFFS,
            "LogFS" => FileSystemType::LogFS,
            "NILFS" => FileSystemType::NILFS,
            "F2FS" => FileSystemType::F2FS,
            "AFS" => FileSystemType::AFS,
            "GFS2" => FileSystemType::GFS2,
            "OCFS2" => FileSystemType::OCFS2,

            // Unknown file systems
            _ => FileSystemType::Unknown(file_system_name.to_string()),
        }
    }
}
