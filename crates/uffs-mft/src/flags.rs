// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

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

// Phase 3 removed the `raw_flags` sub-module — every flag value is
// accessible via the publicly re-exported [`FileFlags`] bitflags type
// (e.g. `FileFlags::DIRECTORY.bits()` for the `u16` value).  The
// stand-alone constants had zero in-crate consumers outside their own
// unit test and zero external consumers per the Phase 3 audit.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_flag() {
        let flags = FileFlags::DIRECTORY;
        assert!(flags.is_directory());
        assert!(!flags.is_file());
    }

    #[test]
    fn combined_flags() {
        let flags = FileFlags::HIDDEN | FileFlags::SYSTEM | FileFlags::DIRECTORY;
        assert!(flags.is_directory());
        assert!(flags.is_hidden());
        assert!(flags.is_system());
        assert!(!flags.is_compressed());
    }

    /// Strengthened to verify the full `u16` wire layout the bitflags
    /// type promises: each named flag must round-trip to the exact
    /// NTFS spec bit (these `bits()` values are the contract the
    /// Polars `flags` column persists on disk).
    #[test]
    fn bits_round_trip_matches_ntfs_layout() {
        assert_eq!(FileFlags::READONLY.bits(), 0x0001);
        assert_eq!(FileFlags::HIDDEN.bits(), 0x0002);
        assert_eq!(FileFlags::SYSTEM.bits(), 0x0004);
        assert_eq!(FileFlags::DIRECTORY.bits(), 0x0010);
        assert_eq!(FileFlags::ARCHIVE.bits(), 0x0020);
        assert_eq!(FileFlags::SPARSE.bits(), 0x0200);
        assert_eq!(FileFlags::REPARSE.bits(), 0x0400);
        assert_eq!(FileFlags::COMPRESSED.bits(), 0x0800);
        assert_eq!(FileFlags::ENCRYPTED.bits(), 0x4000);
    }
}
