//! File attribute flags for MFT records.
//!
//! These flags correspond to NTFS file attributes stored in the
//! `$STANDARD_INFORMATION` attribute.

use bitflags::bitflags;

bitflags! {
    /// File attribute flags from NTFS.
    ///
    /// These can be used to filter files by type in queries.
    ///
    /// # Example
    ///
    /// ```rust
    /// use uffs_mft::FileFlags;
    ///
    /// let flags = FileFlags::DIRECTORY | FileFlags::HIDDEN;
    /// assert!(flags.contains(FileFlags::DIRECTORY));
    /// assert!(flags.is_directory());
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FileFlags: u16 {
        /// File is read-only.
        const READONLY    = 0x0001;
        /// File is hidden.
        const HIDDEN      = 0x0002;
        /// File is a system file.
        const SYSTEM      = 0x0004;
        /// Entry is a directory.
        const DIRECTORY   = 0x0010;
        /// File has been modified since last backup.
        const ARCHIVE     = 0x0020;
        /// File is a device.
        const DEVICE      = 0x0040;
        /// File is normal (no other attributes set).
        const NORMAL      = 0x0080;
        /// File is temporary.
        const TEMPORARY   = 0x0100;
        /// File is a sparse file.
        const SPARSE      = 0x0200;
        /// File is a reparse point (symlink, junction, etc.).
        const REPARSE     = 0x0400;
        /// File is compressed.
        const COMPRESSED  = 0x0800;
        /// File is offline.
        const OFFLINE     = 0x1000;
        /// File is not indexed.
        const NOT_INDEXED = 0x2000;
        /// File is encrypted.
        const ENCRYPTED   = 0x4000;
        /// File was deleted (UFFS internal flag for USN tracking).
        /// This uses bit 15 which is reserved in NTFS.
        const DELETED     = 0x8000;
    }
}

impl FileFlags {
    /// Returns true if this is a directory.
    #[inline]
    #[must_use]
    pub const fn is_directory(self) -> bool {
        self.contains(Self::DIRECTORY)
    }

    /// Returns true if this is a regular file (not a directory).
    #[inline]
    #[must_use]
    pub const fn is_file(self) -> bool {
        !self.is_directory()
    }

    /// Returns true if the file is hidden.
    #[inline]
    #[must_use]
    pub const fn is_hidden(self) -> bool {
        self.contains(Self::HIDDEN)
    }

    /// Returns true if the file is a system file.
    #[inline]
    #[must_use]
    pub const fn is_system(self) -> bool {
        self.contains(Self::SYSTEM)
    }

    /// Returns true if the file is a reparse point (symlink, junction).
    #[inline]
    #[must_use]
    pub const fn is_reparse_point(self) -> bool {
        self.contains(Self::REPARSE)
    }

    /// Returns true if the file is compressed.
    #[inline]
    #[must_use]
    pub const fn is_compressed(self) -> bool {
        self.contains(Self::COMPRESSED)
    }

    /// Returns true if the file is encrypted.
    #[inline]
    #[must_use]
    pub const fn is_encrypted(self) -> bool {
        self.contains(Self::ENCRYPTED)
    }
}

/// Raw flag values for use in Polars filtering.
///
/// Use these with bitwise operations on the `flags` column:
///
/// ```rust,ignore
/// use uffs_polars::prelude::*;
/// use uffs_mft::raw_flags;
///
/// // Filter for directories only
/// df.lazy()
///     .filter(col("flags").bitand(lit(raw_flags::DIRECTORY)).neq(lit(0u16)))
///     .collect()?;
/// ```
pub mod raw_flags {
    // Raw flag constants for direct Polars bitwise operations.

    /// Read-only file attribute.
    pub const READONLY: u16 = 0x0001;
    /// Hidden file attribute.
    pub const HIDDEN: u16 = 0x0002;
    /// System file attribute.
    pub const SYSTEM: u16 = 0x0004;
    /// Directory attribute.
    pub const DIRECTORY: u16 = 0x0010;
    /// Archive attribute (file has been modified).
    pub const ARCHIVE: u16 = 0x0020;
    /// Sparse file attribute.
    pub const SPARSE: u16 = 0x0200;
    /// Reparse point attribute (symlinks, junctions).
    pub const REPARSE: u16 = 0x0400;
    /// Compressed file attribute.
    pub const COMPRESSED: u16 = 0x0800;
    /// Encrypted file attribute.
    pub const ENCRYPTED: u16 = 0x4000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directory_flag() {
        let flags = FileFlags::DIRECTORY;
        assert!(flags.is_directory());
        assert!(!flags.is_file());
    }

    #[test]
    fn test_combined_flags() {
        let flags = FileFlags::HIDDEN | FileFlags::SYSTEM | FileFlags::DIRECTORY;
        assert!(flags.is_directory());
        assert!(flags.is_hidden());
        assert!(flags.is_system());
        assert!(!flags.is_compressed());
    }

    #[test]
    fn test_raw_flags() {
        assert_eq!(raw_flags::DIRECTORY, 0x0010);
        assert_eq!(raw_flags::HIDDEN, 0x0002);
    }
}
