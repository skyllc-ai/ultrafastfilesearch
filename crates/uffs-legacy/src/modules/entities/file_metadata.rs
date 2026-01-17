//! File metadata structures for cross-platform file system operations.
//!
//! This module defines data structures for representing file metadata
//! across different platforms (Windows NTFS, Unix, macOS APFS).
//! These structures are infrastructure for future file enumeration features.

// Infrastructure code - structures defined for future integration
#![allow(dead_code)]

use std::fs::Permissions;
use std::time::SystemTime;

use bitflags::bitflags;

bitflags! {
    /// `FileAttributes` represents various attributes that a file or directory can have.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
    pub struct FileAttributes: u32 {
        // Basic file types
        /// Regular directory
        const DIRECTORY             = 1 << 0; // 0x0001
        /// Regular file
        const FILE                  = 1 << 1; // 0x0002
        /// Symbolic link
        const SYMLINK               = 1 << 2; // 0x0004
        /// Hard link
        const HARD_LINK             = 1 << 3; // 0x0008

        // NTFS-specific attributes
        /// NTFS reparse point
        const REPARSE_POINT         = 1 << 4; // 0x0010
        /// NTFS alternate data stream
        const ALTERNATE_DATA_STREAM = 1 << 5; // 0x0020
        /// NTFS compressed file
        const COMPRESSED            = 1 << 6; // 0x0040
        /// NTFS sparse file
        const SPARSE_FILE           = 1 << 7; // 0x0080

        // Unix-specific special files
        /// Unix block device (e.g., `/dev/sda`)
        const BLOCK_DEVICE          = 1 << 8; // 0x0100
        /// Unix character device (e.g., `/dev/tty`)
        const CHARACTER_DEVICE      = 1 << 9; // 0x0200
        /// Unix named pipe (FIFO)
        const FIFO                  = 1 << 10; // 0x0400
        /// Unix socket
        const SOCKET                = 1 << 11; // 0x0800

        // macOS-specific attributes
        /// macOS bundle (directory shown as a single file in Finder)
        const BUNDLE                = 1 << 12; // 0x1000
        /// macOS package (e.g., application or document bundle)
        const PACKAGE               = 1 << 13; // 0x2000
        /// APFS clone (copy-on-write mechanism)
        const APFS_CLONE            = 1 << 14; // 0x4000

        // Other
        /// Fallback for unknown or unsupported file types
        const OTHER                 = 1 << 15; // 0x8000
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct FileMetadata {
    pub extended_permissions: Option<ExtendedPermissions>, // Extended permissions (NTFS, Unix)

    pub file_size: u64, // File size
    pub disk_size: Option<u64>, /* Actual size on disk (may differ due to block allocation,
                         * compression) */

    pub accessed: Option<SystemTime>, // Last accessed time
    pub modified: Option<SystemTime>, // Last modified time
    pub created: Option<SystemTime>,  // Creation time

    // Unix-specific metadata
    pub inode: Option<u64>,      // Inode number (Unix)
    pub hard_links: Option<u64>, // Number of hard links (Unix)
    pub device_id: Option<u64>,  // Device ID (Unix)

    // NTFS-specific metadata
    pub ntfs_attributes: Option<NtfsAttributes>, // NTFS attributes (Windows)
}

// Unified extended permissions enum to handle both NTFS and Unix systems
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExtendedPermissions {
    Unix(UnixPermissions), // Unix ACL
    Ntfs(NtfsAcl),         // NTFS ACL
}

// Unix permissions structure, separating mode bits and extended ACLs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct UnixPermissions {
    pub mode: UnixMode,       // Unix mode bits (rwx for user, group, others)
    pub acl: Option<UnixAcl>, // Optional Access Control List (ACL)
}

bitflags! {
    /// Unix permission bits (mode) for user, group, and others, as well as special bits like setuid.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
    pub struct UnixMode: u32 {
        // Permission bits for user (owner)
        const USER_READ       = 1 << 8;  // 0x0100
        const USER_WRITE      = 1 << 7;  // 0x0080
        const USER_EXECUTE    = 1 << 6;  // 0x0040

        // Permission bits for group
        const GROUP_READ      = 1 << 5;  // 0x0020
        const GROUP_WRITE     = 1 << 4;  // 0x0010
        const GROUP_EXECUTE   = 1 << 3;  // 0x0008

        // Permission bits for others
        const OTHER_READ      = 1 << 2;  // 0x0004
        const OTHER_WRITE     = 1 << 1;  // 0x0002
        const OTHER_EXECUTE   = 1 << 0;  // 0x0001

        // Special permission bits
        const SETUID          = 1 << 11; // 0x0800
        const SETGID          = 1 << 10; // 0x0400
        const STICKY          = 1 << 9;  // 0x0200
    }
}

// POSIX ACL entries for Unix systems
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct UnixAcl {
    pub entries: Vec<AclEntry>, // List of ACL entries for Unix
}

// Refactor ACL entry to use bitflags instead of a string for permissions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct AclEntry {
    pub user: Option<String>,            // User for this ACL entry
    pub group: Option<String>,           // Group for this ACL entry
    pub permissions: UnixAclPermissions, // Permissions as bitflags (rwx for ACL entries)
}

bitflags! {
    /// Unix ACL permissions for an individual user or group (rwx).
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
    pub struct UnixAclPermissions: u32 {
        const READ       = 1 << 2;  // 0x0004
        const WRITE      = 1 << 1;  // 0x0002
        const EXECUTE    = 1 << 0;  // 0x0001
    }
}

// NTFS permissions structure
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct NtfsAcl {
    pub entries: Vec<NtfsAclEntry>, // List of ACL entries for NTFS
}

// ACL entries for NTFS systems
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct NtfsAclEntry {
    pub principal: String,            // User or group
    pub permissions: NtfsPermissions, // Permissions (e.g., "Read, Write")
}

bitflags! {
    /// NTFS permissions for a principal (user or group) in an Access Control List (ACL).
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
    pub struct NtfsPermissions: u32 {
        /// Permission to read the file/directory.
        const READ              = 1 << 0;  // 0x0001
        /// Permission to write to the file/directory.
        const WRITE             = 1 << 1;  // 0x0002
        /// Permission to execute the file or traverse directories.
        const EXECUTE           = 1 << 2;  // 0x0004
        /// Permission to delete the file/directory.
        const DELETE            = 1 << 3;  // 0x0008
        /// Permission to modify the file/directory.
        const MODIFY            = 1 << 4;  // 0x0010
        /// Permission to take ownership of the file/directory.
        const TAKE_OWNERSHIP    = 1 << 5;  // 0x0020
        /// Permission to change permissions on the file/directory.
        const CHANGE_PERMISSIONS = 1 << 6; // 0x0040
        /// Full control over the file/directory (combines all permissions).
        const FULL_CONTROL      = 1 << 7;  // 0x0080
    }
}

// Define NTFS-specific attributes
bitflags! {
    /// Represents NTFS-specific file attributes. Multiple attributes can be applied simultaneously.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
    pub struct NtfsAttributes: u32 {
        // Basic NTFS file attributes
        /// The file is read-only.
        const READ_ONLY            = 1 << 0; // 0x0001
        /// The file is hidden from normal directory listings.
        const HIDDEN               = 1 << 1; // 0x0002
        /// The file is a system file.
        const SYSTEM               = 1 << 2; // 0x0004
        /// The file is marked for archival.
        const ARCHIVE              = 1 << 3; // 0x0008
        /// The file is temporary and may be deleted after use.
        const TEMPORARY            = 1 << 4; // 0x0010
        /// The file is offline and not immediately available.
        const OFFLINE              = 1 << 5; // 0x0020
        /// The file should not be indexed by content indexing services.
        const NOT_CONTENT_INDEXED  = 1 << 6; // 0x0040
        /// The file is encrypted.
        const ENCRYPTED            = 1 << 7; // 0x0080
        /// The file is compressed to save disk space.
        const COMPRESSED           = 1 << 8; // 0x0100
        /// The file is sparse, containing mostly empty space.
        const SPARSE               = 1 << 9; // 0x0200
        /// The file contains a reparse point, used for symbolic links or mount points.
        const REPARSE_POINT        = 1 << 10; // 0x0400
        /// The file contains an integrity stream for verifying data integrity.
        const INTEGRITY_STREAM     = 1 << 11; // 0x0800
        /// The file is excluded from data scrubbing operations.
        const NO_SCRUB_DATA        = 1 << 12; // 0x1000
    }
}
