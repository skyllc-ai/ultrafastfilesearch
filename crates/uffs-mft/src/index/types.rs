//! Core per-record index types and packed name or stream metadata.

// ============================================================================
// Constants
// ============================================================================

/// Sentinel value indicating "no entry" in linked-list fields.
pub const NO_ENTRY: u32 = u32::MAX;

/// Root directory FRS in NTFS
pub const ROOT_FRS: u64 = 5;

// ============================================================================
// StandardInfo - Bit-packed file attributes
// ============================================================================

/// Bit-packed file attributes captured from NTFS metadata.
///
/// Uses a single `u32` for all boolean flags (15 flags = 15 bits).
/// This is more cache-friendly than separate `bool` fields.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct StandardInfo {
    /// Creation time (Windows FILETIME as i64)
    pub created: i64,
    /// Last write time
    pub modified: i64,
    /// Last access time
    pub accessed: i64,
    /// MFT record change time
    pub mft_changed: i64,
    /// Bit-packed attribute flags
    pub flags: u32,
    // NTFS 3.0+ extended fields (forensic value)
    /// Update Sequence Number - correlates with USN journal (`$UsnJrnl`)
    pub usn: u64,
    /// Security ID - index into $Secure file for ACL lookup
    pub security_id: u32,
    /// Owner ID - for quota tracking
    pub owner_id: u32,
}

impl StandardInfo {
    /// Read-only file attribute flag.
    pub const IS_READONLY: u32 = 1 << 0;
    /// Archive file attribute flag.
    pub const IS_ARCHIVE: u32 = 1 << 1;
    /// System file attribute flag.
    pub const IS_SYSTEM: u32 = 1 << 2;
    /// Hidden file attribute flag.
    pub const IS_HIDDEN: u32 = 1 << 3;
    /// Offline file attribute flag.
    pub const IS_OFFLINE: u32 = 1 << 4;
    /// Not content indexed attribute flag.
    pub const IS_NOT_INDEXED: u32 = 1 << 5;
    /// No scrub data attribute flag.
    pub const IS_NO_SCRUB_DATA: u32 = 1 << 6;
    /// Integrity stream attribute flag.
    pub const IS_INTEGRITY_STREAM: u32 = 1 << 7;
    /// Pinned attribute flag.
    pub const IS_PINNED: u32 = 1 << 8;
    /// Unpinned attribute flag.
    pub const IS_UNPINNED: u32 = 1 << 9;
    /// Directory attribute flag.
    pub const IS_DIRECTORY: u32 = 1 << 10;
    /// Compressed file attribute flag.
    pub const IS_COMPRESSED: u32 = 1 << 11;
    /// Encrypted file attribute flag.
    pub const IS_ENCRYPTED: u32 = 1 << 12;
    /// Sparse file attribute flag.
    pub const IS_SPARSE: u32 = 1 << 13;
    /// Reparse point attribute flag.
    pub const IS_REPARSE: u32 = 1 << 14;
    /// Temporary file attribute flag.
    pub const IS_TEMPORARY: u32 = 1 << 15;
    /// Virtual file attribute flag.
    pub const IS_VIRTUAL: u32 = 1 << 16;

    /// Create from Windows `FILE_ATTRIBUTE_*` flags.
    #[must_use]
    pub fn from_attributes(attrs: u32) -> Self {
        let mut flags = 0_u32;
        if attrs & 0x0001 != 0 {
            flags |= Self::IS_READONLY;
        }
        if attrs & 0x0020 != 0 {
            flags |= Self::IS_ARCHIVE;
        }
        if attrs & 0x0004 != 0 {
            flags |= Self::IS_SYSTEM;
        }
        if attrs & 0x0002 != 0 {
            flags |= Self::IS_HIDDEN;
        }
        if attrs & 0x1000 != 0 {
            flags |= Self::IS_OFFLINE;
        }
        if attrs & 0x2000 != 0 {
            flags |= Self::IS_NOT_INDEXED;
        }
        if attrs & 0x0001_0000 != 0 {
            flags |= Self::IS_DIRECTORY;
        }
        if attrs & 0x0800 != 0 {
            flags |= Self::IS_COMPRESSED;
        }
        if attrs & 0x4000 != 0 {
            flags |= Self::IS_ENCRYPTED;
        }
        if attrs & 0x0200 != 0 {
            flags |= Self::IS_SPARSE;
        }
        if attrs & 0x0400 != 0 {
            flags |= Self::IS_REPARSE;
        }
        if attrs & 0x0100 != 0 {
            flags |= Self::IS_TEMPORARY;
        }
        Self {
            flags,
            ..Default::default()
        }
    }

    /// Convert back to Windows `FILE_ATTRIBUTE_*` flags.
    #[must_use]
    pub const fn to_attributes(&self) -> u32 {
        let mut attrs = 0_u32;
        if self.is_readonly() {
            attrs |= 0x0001;
        }
        if self.is_hidden() {
            attrs |= 0x0002;
        }
        if self.is_system() {
            attrs |= 0x0004;
        }
        if self.is_directory() {
            attrs |= 0x0010;
        }
        if self.is_archive() {
            attrs |= 0x0020;
        }
        // Note: TEMPORARY (0x0100) is intentionally NOT included here.
        // The serialized attribute view intentionally skips FILE_ATTRIBUTE_TEMPORARY.
        if self.is_sparse() {
            attrs |= 0x0200;
        }
        if self.is_reparse() {
            attrs |= 0x0400;
        }
        if self.is_compressed() {
            attrs |= 0x0800;
        }
        if self.is_offline() {
            attrs |= 0x1000;
        }
        if self.is_not_indexed() {
            attrs |= 0x2000;
        }
        if self.is_encrypted() {
            attrs |= 0x4000;
        }
        if self.is_integrity_stream() {
            attrs |= 0x8000;
        }
        if self.is_no_scrub_data() {
            attrs |= 0x0002_0000;
        }
        if self.is_pinned() {
            attrs |= 0x0008_0000;
        }
        if self.is_unpinned() {
            attrs |= 0x0010_0000;
        }
        attrs
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

// ============================================================================
// IndexNameRef - Reference into names buffer
// ============================================================================

/// Reference to a filename in the contiguous names buffer.
///
/// Bit-packed structure: exactly 8 bytes, zero padding.
///
/// # Bit Layout of `meta` field:
/// - Bits 0-9:   UTF-8 length (max 1023 bytes)
/// - Bits 10-15: flags (`is_ascii`, etc.)
/// - Bits 16-31: `extension_id` (65K unique extensions)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IndexNameRef {
    /// Byte offset into `MftIndex::names`
    pub offset: u32,
    /// Packed metadata: length (10 bits) + flags (6 bits) + `extension_id` (16
    /// bits)
    pub meta: u32,
}

impl IndexNameRef {
    /// Bit mask for extracting length from meta field (bits 0-9)
    const LENGTH_MASK: u32 = 0x3FF;
    /// Bit shift for length field
    const LENGTH_SHIFT: u32 = 0;

    /// Bit mask for extracting flags from meta field (bits 10-15)
    const FLAGS_MASK: u32 = 0x3F << 10;
    /// Bit shift for flags field
    const FLAGS_SHIFT: u32 = 10;

    /// Bit mask for extracting extension ID from meta field (bits 16-31)
    const EXT_ID_MASK: u32 = 0xFFFF << 16;
    /// Bit shift for extension ID field
    const EXT_ID_SHIFT: u32 = 16;

    /// Flag bit indicating the name is pure ASCII
    const IS_ASCII: u32 = 1 << 0;

    /// No extension (`extension_id` = 0 means no extension)
    pub const NO_EXTENSION: u16 = 0;

    /// Creates a new `IndexNameRef` with the given offset, length, ASCII flag,
    /// and `extension_id`.
    #[must_use]
    pub const fn new(offset: u32, length: u16, is_ascii: bool, extension_id: u16) -> Self {
        let length_bits = (length as u32 & Self::LENGTH_MASK) << Self::LENGTH_SHIFT;
        let flags_bits = if is_ascii {
            Self::IS_ASCII << Self::FLAGS_SHIFT
        } else {
            0
        };
        let ext_id_bits = (extension_id as u32) << Self::EXT_ID_SHIFT;

        Self {
            offset,
            meta: length_bits | flags_bits | ext_id_bits,
        }
    }

    /// Returns the UTF-8 length in bytes.
    #[inline]
    #[must_use]
    pub const fn length(&self) -> u16 {
        ((self.meta >> Self::LENGTH_SHIFT) & Self::LENGTH_MASK) as u16
    }

    /// Returns the raw flags field (6 bits).
    #[inline]
    #[must_use]
    pub const fn flags(&self) -> u8 {
        ((self.meta >> Self::FLAGS_SHIFT) & (Self::FLAGS_MASK >> Self::FLAGS_SHIFT)) as u8
    }

    /// Returns the extension ID (0 = no extension).
    #[inline]
    #[must_use]
    pub const fn extension_id(&self) -> u16 {
        ((self.meta >> Self::EXT_ID_SHIFT) & (Self::EXT_ID_MASK >> Self::EXT_ID_SHIFT)) as u16
    }

    /// Returns true if the name is pure ASCII.
    #[inline]
    #[must_use]
    pub const fn is_ascii(&self) -> bool {
        (self.meta & (Self::IS_ASCII << Self::FLAGS_SHIFT)) != 0
    }

    /// Remap the `extension_id` to a new value.
    ///
    /// Used during fragment merging to remap extension IDs from fragment-local
    /// to merged-global space.
    #[inline]
    pub fn remap_extension_id(&mut self, new_extension_id: u16) {
        // Clear old extension_id bits and set new ones
        self.meta =
            (self.meta & !Self::EXT_ID_MASK) | (u32::from(new_extension_id) << Self::EXT_ID_SHIFT);
    }

    /// Returns true if this name reference is valid (not `NO_ENTRY`).
    #[inline]
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.offset != NO_ENTRY
    }
}

// ============================================================================
// LinkInfo - Hard link chain entry
// ============================================================================

/// Hard link information.
///
/// Most files have only one name, stored inline in `FileRecord::first_name`.
/// Files with multiple hard links form a linked list via `next_entry`.
///
/// Uses `u64` for `parent_frs` so the index can represent the full NTFS FRS
/// range.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct LinkInfo {
    /// Index of next `LinkInfo` in `MftIndex::links`, or `NO_ENTRY`
    pub next_entry: u32,
    /// Filename reference
    pub name: IndexNameRef,
    /// Parent directory FRS (u64 to support all valid NTFS volumes)
    pub parent_frs: u64,
}

// ============================================================================
// SizeInfo - File size information
// ============================================================================

/// File size information.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct SizeInfo {
    /// Logical file size
    pub length: u64,
    /// Allocated size on disk
    pub allocated: u64,
}

// ============================================================================
// IndexStreamInfo - Alternate Data Stream chain entry
// ============================================================================

/// Alternate Data Stream information.
///
/// Most files have only the default `$DATA` stream, stored inline.
/// Files with ADS form a linked list via `next_entry`.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IndexStreamInfo {
    /// Size information
    pub size: SizeInfo,
    /// Index of next `IndexStreamInfo` in `MftIndex::streams`, or `NO_ENTRY`
    pub next_entry: u32,
    /// Stream name reference (empty for default `$DATA`)
    pub name: IndexNameRef,
    /// Packed flags: bit 0 = `is_sparse`, bit 1 = `is_resident`, bits 2-7 =
    /// `type_name_id`
    pub flags: u8,
}

impl IndexStreamInfo {
    /// Returns true if this stream is sparse.
    #[inline]
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        self.flags & 0x01 != 0
    }
    /// Returns true if this stream's data is resident (stored in MFT record).
    #[inline]
    #[must_use]
    pub const fn is_resident(&self) -> bool {
        self.flags & 0x02 != 0
    }
    /// Returns the type name ID for this stream.
    #[inline]
    #[must_use]
    pub const fn type_name_id(&self) -> u8 {
        self.flags >> 2
    }

    /// Returns true if this stream should be included in output.
    ///
    /// Only `$DATA` streams (`type_name_id=8`, i.e., `0x80 >> 4`) and directory
    /// indexes (`type_name_id=0` for `$I30`) are included. Internal attributes
    /// like `$OBJECT_ID` (`type_name_id=4`), `$EA_INFORMATION`
    /// (`type_name_id=13`) are filtered out.
    #[inline]
    #[must_use]
    pub const fn is_output_stream(&self) -> bool {
        let tid = self.type_name_id();
        // type_name_id == 0: directory index ($I30)
        // type_name_id == 8: $DATA (0x80 >> 4)
        tid == 0 || tid == 8
    }
}

// ============================================================================
// ============================================================================
// InternalStreamInfo - Internal NTFS stream chain entry
// ============================================================================

/// Internal NTFS attribute stream information.
///
/// These correspond to internal attributes counted during tree metrics (for
/// example `$REPARSE_POINT`, `$SECURITY_DESCRIPTOR`, and `$OBJECT_ID`) but not
/// exposed as user-visible ADS rows.
///
/// They are stored separately so proportional hard-link attribution can be
/// applied per stream (the delta is not additive after integer rounding).
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct InternalStreamInfo {
    /// Size information for the internal stream.
    pub size: SizeInfo,
    /// Index of next `InternalStreamInfo` in `MftIndex::internal_streams`, or
    /// `NO_ENTRY`.
    pub next_entry: u32,
    /// Packed flags (`bit0=is_sparse`, `bit1=is_resident`). Other bits are
    /// reserved.
    pub flags: u8,
}

// ============================================================================
// FileRecord - Core file metadata
// ============================================================================

/// Core file and directory record.
///
/// Size: 224 bytes per record (includes sequence number, `$FILE_NAME`
/// timestamps, forensic fields)
#[derive(Debug, Clone, Default)]
#[repr(C)]
pub struct FileRecord {
    /// FRS (File Record Segment) number - primary key
    pub frs: u64,
    /// Sequence number (incremented when FRS is reused, forensic value)
    pub sequence_number: u16,
    /// Primary filename namespace (0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS)
    pub namespace: u8,
    /// Forensic flags (bit-packed): bit 0 = `is_deleted`, bit 1 = `is_corrupt`,
    /// bit 2 = `is_extension`
    pub forensic_flags: u8,
    /// Log File Sequence Number - correlates with `$LogFile` journal (forensic)
    pub lsn: u64,
    /// Reparse tag from `$REPARSE_POINT` (0 if not a reparse point).
    /// Common: symlink (0xA000000C), junction (0xA0000003), `OneDrive`, etc.
    pub reparse_tag: u32,
    /// Base FRS for extension records (0 for base records).
    /// Only meaningful when `is_extension()` returns true.
    pub base_frs: u64,
    /// Timestamps and bit-packed attributes from `$STANDARD_INFORMATION`
    pub stdinfo: StandardInfo,
    /// Number of hard links (usually 1)
    pub name_count: u16,
    /// Number of user-visible data streams (usually 1, excludes internal
    /// Windows streams)
    pub stream_count: u16,
    /// Total number of all streams including internal Windows streams.
    pub total_stream_count: u16,
    /// Head of linked list of internal streams for this record (indexes into
    /// `MftIndex::internal_streams`), or `NO_ENTRY`
    pub first_internal_stream: u32,
    /// Index of first child in `MftIndex::children`, or `NO_ENTRY`
    pub first_child: u32,
    /// Primary filename (inline, no allocation)
    pub first_name: LinkInfo,
    /// Primary data stream (inline, no allocation)
    pub first_stream: IndexStreamInfo,

    // $FILE_NAME timestamps (often differ from $STANDARD_INFORMATION)
    /// Creation time from `$FILE_NAME` (Unix microseconds)
    pub fn_created: i64,
    /// Modification time from `$FILE_NAME` (Unix microseconds)
    pub fn_modified: i64,
    /// Access time from `$FILE_NAME` (Unix microseconds)
    pub fn_accessed: i64,
    /// MFT change time from `$FILE_NAME` (Unix microseconds)
    pub fn_mft_changed: i64,

    // Tree metrics (computed after all records parsed via compute_tree_metrics)
    /// Count of all descendants (files + subdirectories) in subtree (0 for
    /// files)
    pub descendants: u32,
    /// Sum of logical file sizes in subtree (includes this file/directory)
    pub treesize: u64,
    /// Sum of allocated disk sizes in subtree (includes this file/directory)
    pub tree_allocated: u64,

    /// Size of internal Windows streams (like `$REPARSE_POINT`) that are
    /// filtered from user-visible output but still included in tree metrics.
    pub internal_streams_size: u64,
    /// Allocated size of internal Windows streams.
    pub internal_streams_allocated: u64,
}

impl FileRecord {
    /// Create a new record for the given FRS
    #[must_use]
    pub fn new(frs: u64) -> Self {
        Self {
            frs,
            name_count: 1,         // Every file has at least one name
            stream_count: 1,       // User-visible streams (default $DATA)
            total_stream_count: 1, // All streams including internal (for tree metrics)
            first_internal_stream: NO_ENTRY,
            first_child: NO_ENTRY,
            first_name: LinkInfo {
                next_entry: NO_ENTRY,
                name: IndexNameRef {
                    offset: NO_ENTRY,
                    meta: 0,
                },
                parent_frs: u64::from(NO_ENTRY),
            },
            first_stream: IndexStreamInfo {
                next_entry: NO_ENTRY,
                name: IndexNameRef {
                    offset: NO_ENTRY,
                    meta: 0,
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Returns true if this record is a directory.
    #[inline]
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.stdinfo.is_directory()
    }
    /// Returns true if this record has a valid name.
    #[inline]
    #[must_use]
    pub const fn has_name(&self) -> bool {
        self.first_name.name.is_valid()
    }

    /// Returns true if this record has base record data (not just extension
    /// data).
    ///
    /// A placeholder created by extension record processing will have a name
    /// (from the extension) but no stdinfo (created timestamp = 0). A real base
    /// record will have non-zero timestamps from `$STANDARD_INFORMATION`.
    ///
    /// This is used during fragment merging to determine which record to keep
    /// when both have names.
    #[inline]
    #[must_use]
    pub const fn has_base_data(&self) -> bool {
        // Real base records always have a creation timestamp from $STANDARD_INFORMATION
        // Placeholders created by extension processing have stdinfo = Default (all
        // zeros)
        self.stdinfo.created != 0
    }

    // ===== P3 Forensic Flag Accessors =====
    // forensic_flags bit layout: bit 0 = is_deleted, bit 1 = is_corrupt, bit 2 =
    // is_extension

    /// Returns true if this record is deleted (MFT record not in use).
    /// Only meaningful when parsed with `--forensic` flag.
    #[inline]
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.forensic_flags & 0b001 != 0
    }

    /// Returns true if this record is corrupt (USA fixup failed or BAAD magic).
    /// Only meaningful when parsed with `--forensic` flag.
    #[inline]
    #[must_use]
    pub const fn is_corrupt(&self) -> bool {
        self.forensic_flags & 0b010 != 0
    }

    /// Returns true if this is an extension record (not a base record).
    /// Only meaningful when parsed with `--forensic` flag.
    #[inline]
    #[must_use]
    pub const fn is_extension(&self) -> bool {
        self.forensic_flags & 0b100 != 0
    }

    /// Sets the forensic flags from parsed record fields.
    #[inline]
    pub fn set_forensic_flags(&mut self, is_deleted: bool, is_corrupt: bool, is_extension: bool) {
        self.forensic_flags = u8::from(is_deleted)
            | (u8::from(is_corrupt) << 1_u8)
            | (u8::from(is_extension) << 2_u8);
    }

    /// Returns the tree metrics tuple (descendants, treesize,
    /// `tree_allocated`).
    ///
    /// This is the **single source of truth** for tree metrics extraction.
    /// Both OFFLINE (`MftIndex::to_dataframe`) and LIVE
    /// (`results_to_dataframe`) paths should use this method to ensure
    /// consistent behavior.
    ///
    /// # Tree-metrics Notes
    ///
    /// - Directories (including reparse points like junctions/symlinks) always
    ///   return their computed tree metrics. Junctions are directory leaves
    ///   with `descendants=1`, not files with `descendants=0`.
    /// - Files return `descendants=0` and their own size/allocated values.
    #[inline]
    #[must_use]
    pub const fn tree_metrics(&self) -> (u32, u64, u64) {
        (self.descendants, self.treesize, self.tree_allocated)
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Compare two strings case-insensitively (zero allocations for ASCII).
///
/// For ASCII strings, this performs a zero-allocation comparison by comparing
/// bytes after converting to lowercase. For non-ASCII strings, it falls back
/// to allocating lowercase versions.
#[cfg(test)]
pub fn cmp_ascii_case_insensitive(str_a: &str, str_b: &str) -> core::cmp::Ordering {
    if str_a.is_ascii() && str_b.is_ascii() {
        // Fast path: both strings are ASCII
        str_a
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .cmp(str_b.bytes().map(|byte| byte.to_ascii_lowercase()))
    } else {
        // Slow path: at least one string contains non-ASCII characters
        str_a.to_lowercase().cmp(&str_b.to_lowercase())
    }
}
