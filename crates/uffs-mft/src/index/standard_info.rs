// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bit-packed `StandardInfo` type and accessor methods.
//!
//! Extracted from `types.rs` to keep it under the 800 LOC threshold.

// ============================================================================
// StandardInfo - Bit-packed file attributes
// ============================================================================

/// Bit-packed file attributes captured from NTFS metadata.
///
/// Uses a single `u32` for all boolean flags (15 flags = 15 bits).
/// This is more cache-friendly than separate `bool` fields.
///
/// 56 bytes with explicit padding.  Derives `bytemuck::Pod` for bulk
/// serialization.
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct StandardInfo {
    /// Creation time — **raw Windows FILETIME** (100-ns ticks since
    /// 1601-01-01 UTC), stored verbatim from `$STANDARD_INFORMATION`.
    ///
    /// v13+ storage invariant: do **not** pre-convert to Unix
    /// microseconds at any layer — FILETIME is the root truth, and
    /// it's required to represent pre-1970 dates (which Unix
    /// micros can't express without negative values).  Callers that
    /// need calendar fields must go through
    /// `uffs_time::filetime_to_calendar`.  Formatters that assume Unix
    /// micros produce year-6220 output for 2026-era timestamps — see
    /// the regression tests in
    /// `uffs-core::output::config::tests::append_datetime_native_*`
    /// and `uffs-core::output::tests::test_write_datetime_column_*`.
    pub created: i64,
    /// Last write time — see [`StandardInfo::created`] for unit
    /// semantics (raw FILETIME).
    pub modified: i64,
    /// Last access time — see [`StandardInfo::created`] for unit
    /// semantics (raw FILETIME).
    pub accessed: i64,
    /// MFT record change time — see [`StandardInfo::created`] for
    /// unit semantics (raw FILETIME).
    pub mft_changed: i64,
    /// Bit-packed attribute flags
    pub flags: u32,
    /// Explicit padding for `u64` alignment of `usn`.
    #[expect(
        clippy::pub_underscore_fields,
        reason = "bytemuck Pod requires all fields same visibility"
    )]
    pub _pad0: [u8; 4],
    // NTFS 3.0+ extended fields (forensic value)
    /// Update Sequence Number - correlates with USN journal (`$UsnJrnl`)
    pub usn: u64,
    /// Security ID - index into $Secure file for ACL lookup
    pub security_id: u32,
    /// Owner ID - for quota tracking
    pub owner_id: u32,
}

impl StandardInfo {
    // ─── Raw NTFS FILE_ATTRIBUTE_* constants ───────────────────────
    // These match the standard Windows NTFS bit layout exactly.
    // See: https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants
    //
    // RULE: Always use these named constants — never raw hex values.

    /// `FILE_ATTRIBUTE_READONLY` (0x0001)
    pub(crate) const IS_READONLY: u32 = 0x0001;
    /// `FILE_ATTRIBUTE_HIDDEN` (0x0002)
    pub(crate) const IS_HIDDEN: u32 = 0x0002;
    /// `FILE_ATTRIBUTE_SYSTEM` (0x0004)
    pub(crate) const IS_SYSTEM: u32 = 0x0004;
    /// `FILE_ATTRIBUTE_DIRECTORY` (0x0010)
    pub(crate) const IS_DIRECTORY: u32 = 0x0010;
    /// `FILE_ATTRIBUTE_ARCHIVE` (0x0020)
    pub const IS_ARCHIVE: u32 = 0x0020;
    /// `FILE_ATTRIBUTE_DEVICE` (0x0040)
    pub const IS_DEVICE: u32 = 0x0040;
    /// `FILE_ATTRIBUTE_NORMAL` (0x0080)
    pub const IS_NORMAL: u32 = 0x0080;
    /// `FILE_ATTRIBUTE_TEMPORARY` (0x0100)
    pub(crate) const IS_TEMPORARY: u32 = 0x0100;
    /// `FILE_ATTRIBUTE_SPARSE_FILE` (0x0200)
    pub(crate) const IS_SPARSE: u32 = 0x0200;
    /// `FILE_ATTRIBUTE_REPARSE_POINT` (0x0400)
    pub(crate) const IS_REPARSE: u32 = 0x0400;
    /// `FILE_ATTRIBUTE_COMPRESSED` (0x0800)
    pub(crate) const IS_COMPRESSED: u32 = 0x0800;
    /// `FILE_ATTRIBUTE_OFFLINE` (0x1000)
    pub(crate) const IS_OFFLINE: u32 = 0x1000;
    /// `FILE_ATTRIBUTE_NOT_CONTENT_INDEXED` (0x2000)
    pub(crate) const IS_NOT_INDEXED: u32 = 0x2000;
    /// `FILE_ATTRIBUTE_ENCRYPTED` (0x4000)
    pub(crate) const IS_ENCRYPTED: u32 = 0x4000;
    /// `FILE_ATTRIBUTE_INTEGRITY_STREAM` (0x8000)
    pub(crate) const IS_INTEGRITY_STREAM: u32 = 0x8000;
    /// `FILE_ATTRIBUTE_VIRTUAL` (0x10000)
    pub(crate) const IS_VIRTUAL: u32 = 0x0001_0000;
    /// `FILE_ATTRIBUTE_NO_SCRUB_DATA` (0x20000)
    pub(crate) const IS_NO_SCRUB_DATA: u32 = 0x0002_0000;
    /// `FILE_ATTRIBUTE_PINNED` (0x80000)
    pub(crate) const IS_PINNED: u32 = 0x0008_0000;
    /// `FILE_ATTRIBUTE_UNPINNED` (0x100000)
    pub(crate) const IS_UNPINNED: u32 = 0x0010_0000;
    /// `FILE_ATTRIBUTE_RECALL_ON_OPEN` (0x40000)
    pub(crate) const IS_RECALL_ON_OPEN: u32 = 0x0004_0000;
    /// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` (0x400000)
    pub(crate) const IS_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

    /// Create from [`ExtendedStandardInfo`] - the canonical conversion point.
    ///
    /// This is the **single source of truth** for converting parsed NTFS
    /// attributes to compact index storage. All code paths should use:
    /// 1. `ExtendedStandardInfo::from_attributes()` (internal helper) to parse
    ///    raw flags
    /// 2. This method to convert to compact [`StandardInfo`]
    ///
    /// [`ExtendedStandardInfo`]: crate::ntfs::ExtendedStandardInfo
    #[must_use]
    pub const fn from_extended(ext: &crate::ntfs::ExtendedStandardInfo) -> Self {
        let mut flags = 0_u32;

        // Core file attributes
        if ext.is_readonly {
            flags |= Self::IS_READONLY;
        }
        if ext.is_archive {
            flags |= Self::IS_ARCHIVE;
        }
        if ext.is_system {
            flags |= Self::IS_SYSTEM;
        }
        if ext.is_hidden {
            flags |= Self::IS_HIDDEN;
        }
        if ext.is_offline {
            flags |= Self::IS_OFFLINE;
        }
        if ext.is_not_content_indexed {
            flags |= Self::IS_NOT_INDEXED;
        }
        if ext.is_compressed {
            flags |= Self::IS_COMPRESSED;
        }
        if ext.is_encrypted {
            flags |= Self::IS_ENCRYPTED;
        }
        if ext.is_sparse {
            flags |= Self::IS_SPARSE;
        }
        if ext.is_reparse {
            flags |= Self::IS_REPARSE;
        }
        if ext.is_temporary {
            flags |= Self::IS_TEMPORARY;
        }

        // Extended attributes (NTFS 3.1+ / Windows 8+)
        if ext.is_integrity_stream {
            flags |= Self::IS_INTEGRITY_STREAM;
        }
        if ext.is_no_scrub_data {
            flags |= Self::IS_NO_SCRUB_DATA;
        }
        if ext.is_pinned {
            flags |= Self::IS_PINNED;
        }
        if ext.is_unpinned {
            flags |= Self::IS_UNPINNED;
        }
        if ext.is_virtual {
            flags |= Self::IS_VIRTUAL;
        }

        // Note: is_directory is set separately via set_directory() based on
        // MFT record flags, not $STANDARD_INFORMATION attributes.

        Self {
            created: ext.created,
            modified: ext.modified,
            accessed: ext.accessed,
            mft_changed: ext.mft_changed,
            flags,
            _pad0: [0; 4],
            usn: ext.usn,
            security_id: ext.security_id,
            owner_id: ext.owner_id,
        }
    }

    /// Create directly from raw NTFS `FILE_ATTRIBUTE_*` flags.
    ///
    /// Since `StandardInfo` constants now match raw NTFS bit layout,
    /// this is a direct store — no remapping needed.
    ///
    /// Timestamps, USN, security/owner IDs must be set separately by the
    /// caller.
    #[inline]
    #[must_use]
    pub const fn from_raw_ntfs_flags(attrs: u32) -> Self {
        Self {
            flags: attrs,
            ..Self::DEFAULT
        }
    }

    /// Zero-valued constant for use in `from_raw_ntfs_flags`.
    const DEFAULT: Self = Self {
        created: 0,
        modified: 0,
        accessed: 0,
        mft_changed: 0,
        flags: 0,
        _pad0: [0; 4],
        usn: 0,
        security_id: 0,
        owner_id: 0,
    };

    /// Create from raw NTFS `FILE_ATTRIBUTE_*` flags.
    ///
    /// Since `StandardInfo` constants now match raw NTFS bit layout,
    /// this is a direct store — no remapping needed.
    #[must_use]
    pub const fn from_attributes(attrs: u32) -> Self {
        Self {
            flags: attrs,
            ..Self::DEFAULT
        }
    }

    /// Convert to raw NTFS `FILE_ATTRIBUTE_*` flags.
    ///
    /// Since `StandardInfo.flags` now stores raw NTFS bits directly,
    /// this is an identity operation.
    #[must_use]
    pub const fn to_attributes(self) -> u32 {
        self.flags
    }

    /// Returns true if the read-only flag is set.
    #[inline]
    #[must_use]
    pub const fn is_readonly(&self) -> bool {
        self.flags & Self::IS_READONLY != 0
    }
    /// Returns true if the archive flag is set.
    #[inline]
    #[must_use]
    pub const fn is_archive(&self) -> bool {
        self.flags & Self::IS_ARCHIVE != 0
    }
    /// Returns true if the system flag is set.
    #[inline]
    #[must_use]
    pub const fn is_system(&self) -> bool {
        self.flags & Self::IS_SYSTEM != 0
    }
    /// Returns true if the hidden flag is set.
    #[inline]
    #[must_use]
    pub const fn is_hidden(&self) -> bool {
        self.flags & Self::IS_HIDDEN != 0
    }
    /// Returns true if the offline flag is set.
    #[inline]
    #[must_use]
    pub const fn is_offline(&self) -> bool {
        self.flags & Self::IS_OFFLINE != 0
    }
    /// Returns true if the not-indexed flag is set.
    #[inline]
    #[must_use]
    pub const fn is_not_indexed(&self) -> bool {
        self.flags & Self::IS_NOT_INDEXED != 0
    }
    /// Returns true if this is a directory.
    #[inline]
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.flags & Self::IS_DIRECTORY != 0
    }
    /// Returns true if the compressed flag is set.
    #[inline]
    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        self.flags & Self::IS_COMPRESSED != 0
    }
    /// Returns true if the encrypted flag is set.
    #[inline]
    #[must_use]
    pub const fn is_encrypted(&self) -> bool {
        self.flags & Self::IS_ENCRYPTED != 0
    }
    /// Returns true if the sparse flag is set.
    #[inline]
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        self.flags & Self::IS_SPARSE != 0
    }
    /// Returns true if the reparse point flag is set.
    #[inline]
    #[must_use]
    pub const fn is_reparse(&self) -> bool {
        self.flags & Self::IS_REPARSE != 0
    }
    /// Returns true if the temporary flag is set.
    #[inline]
    #[must_use]
    pub const fn is_temporary(&self) -> bool {
        self.flags & Self::IS_TEMPORARY != 0
    }
    /// Returns true if the integrity stream flag is set.
    #[inline]
    #[must_use]
    pub const fn is_integrity_stream(&self) -> bool {
        self.flags & Self::IS_INTEGRITY_STREAM != 0
    }
    /// Returns true if the no scrub data flag is set.
    #[inline]
    #[must_use]
    pub const fn is_no_scrub_data(&self) -> bool {
        self.flags & Self::IS_NO_SCRUB_DATA != 0
    }
    /// Returns true if the pinned flag is set.
    #[inline]
    #[must_use]
    pub const fn is_pinned(&self) -> bool {
        self.flags & Self::IS_PINNED != 0
    }
    /// Returns true if the unpinned flag is set.
    #[inline]
    #[must_use]
    pub const fn is_unpinned(&self) -> bool {
        self.flags & Self::IS_UNPINNED != 0
    }
    /// Returns true if the virtual flag is set.
    #[inline]
    #[must_use]
    pub const fn is_virtual(&self) -> bool {
        self.flags & Self::IS_VIRTUAL != 0
    }
    /// Returns true if the recall-on-open flag is set (tiered storage).
    #[inline]
    #[must_use]
    pub const fn is_recall_on_open(&self) -> bool {
        self.flags & Self::IS_RECALL_ON_OPEN != 0
    }
    /// Returns true if the recall-on-data-access flag is set (tiered storage).
    #[inline]
    #[must_use]
    pub const fn is_recall_on_data_access(&self) -> bool {
        self.flags & Self::IS_RECALL_ON_DATA_ACCESS != 0
    }

    /// Convert to raw NTFS attributes masked to the 15 bits the baseline output
    /// tracks. Used by `--parity-compat` to produce deterministic output.
    #[must_use]
    pub const fn parity_attributes(&self) -> u32 {
        self.flags
            & (Self::IS_READONLY
                | Self::IS_HIDDEN
                | Self::IS_SYSTEM
                | Self::IS_DIRECTORY
                | Self::IS_ARCHIVE
                | Self::IS_SPARSE
                | Self::IS_REPARSE
                | Self::IS_COMPRESSED
                | Self::IS_OFFLINE
                | Self::IS_NOT_INDEXED
                | Self::IS_ENCRYPTED
                | Self::IS_INTEGRITY_STREAM
                | Self::IS_NO_SCRUB_DATA
                | Self::IS_PINNED
                | Self::IS_UNPINNED)
    }

    /// Sets or clears the directory flag.
    #[inline]
    pub const fn set_directory(&mut self, val: bool) {
        if val {
            self.flags |= Self::IS_DIRECTORY;
        } else {
            self.flags &= !Self::IS_DIRECTORY;
        }
    }
}
