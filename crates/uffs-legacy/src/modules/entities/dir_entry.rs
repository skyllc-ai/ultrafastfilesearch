use std::ffi::OsString;
use std::fs::Permissions;
use std::path::PathBuf;

use sysinfo::{Gid, Uid};

use crate::modules::entities::file_metadata::{FileAttributes, FileMetadata};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry<T> {
    pub path: PathBuf,                   // Full path to the file/directory
    pub file_name: OsString,             // File name
    pub file_attributes: FileAttributes, // Multiple attributes (Directory, File, Symlink, etc.)
    pub metadata: Option<FileMetadata>,  // Metadata associated with the file (platform-specific)
    pub depth: Option<usize>,            /* Depth in directory hierarchy (useful for directory
                                          * walkers) */
    pub custom_data: Option<T>, // Custom data for flexibility (e.g., jwalk's extra data)
    pub permissions: Permissions, // Basic permissions
    pub owner: Option<Uid>,     // Unix user ID or NTFS owner SID
    pub group: Option<Gid>,     // Unix group ID or NTFS group SID
    pub is_async: bool,         // Whether the entry was fetched asynchronously
}
