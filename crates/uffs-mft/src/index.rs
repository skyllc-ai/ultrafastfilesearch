//! # Lean MFT Index - C++ Performance Parity
//!
//! This module provides a compact, cache-friendly in-memory index that matches
//! the C++ `NtfsIndex` architecture for maximum performance.
//!
//! ## Design Philosophy
//!
//! - **No Polars overhead**: Build index directly from parsed MFT records
//! - **Compact memory layout**: Bit-packed attributes, contiguous names buffer
//! - **O(1) FRS lookup**: Direct indexing via `frs_to_idx` table
//! - **Optional `DataFrame`**: Convert to Polars only when needed for analytics
//!
//! ## Memory Layout (matching C++)
//!
//! ```text
//! MftIndex
//! ├── records: Vec<FileRecord>     // Core file metadata
//! ├── frs_to_idx: Vec<u32>         // FRS → record index (O(1) lookup)
//! ├── names: String                // All filenames concatenated
//! ├── links: Vec<LinkInfo>         // Hard link chain (overflow)
//! ├── streams: Vec<IndexStreamInfo>     // ADS chain (overflow)
//! └── children: Vec<ChildInfo>     // Directory contents
//! ```

// ============================================================================
// Constants
// ============================================================================

/// Sentinel value indicating "no entry" (matches C++ `~0` / `negative_one`)
pub const NO_ENTRY: u32 = u32::MAX;

/// Root directory FRS in NTFS
pub const ROOT_FRS: u64 = 5;

// ============================================================================
// StandardInfo - Bit-packed attributes (matches C++ exactly)
// ============================================================================

/// Bit-packed file attributes matching C++ `StandardInfo`.
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
        if self.is_archive() {
            attrs |= 0x0020;
        }
        if self.is_system() {
            attrs |= 0x0004;
        }
        if self.is_hidden() {
            attrs |= 0x0002;
        }
        if self.is_offline() {
            attrs |= 0x1000;
        }
        if self.is_not_indexed() {
            attrs |= 0x2000;
        }
        if self.is_directory() {
            attrs |= 0x0010;
        }
        if self.is_compressed() {
            attrs |= 0x0800;
        }
        if self.is_encrypted() {
            attrs |= 0x4000;
        }
        if self.is_sparse() {
            attrs |= 0x0200;
        }
        if self.is_reparse() {
            attrs |= 0x0400;
        }
        if self.is_temporary() {
            attrs |= 0x0100;
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
/// Matches C++ `IndexNameRef` - stores offset + length + ASCII flag.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IndexNameRef {
    /// Byte offset into `MftIndex::names`
    pub offset: u32,
    /// Length in characters (not bytes for Unicode)
    pub length: u16,
    /// Packed: bit 0 = `is_ascii`, bits 1-15 reserved
    pub flags: u16,
}

impl IndexNameRef {
    /// Flag indicating the name is pure ASCII.
    const IS_ASCII: u16 = 1 << 0;

    /// Creates a new `IndexNameRef` with the given offset, length, and ASCII
    /// flag.
    #[must_use]
    pub const fn new(offset: u32, length: u16, is_ascii: bool) -> Self {
        Self {
            offset,
            length,
            flags: if is_ascii { Self::IS_ASCII } else { 0 },
        }
    }

    /// Returns true if the name is pure ASCII.
    #[inline]
    #[must_use]
    pub const fn is_ascii(&self) -> bool {
        self.flags & Self::IS_ASCII != 0
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

/// Hard link information (matches C++ `LinkInfo`).
///
/// Most files have only one name, stored inline in `FileRecord::first_name`.
/// Files with multiple hard links form a linked list via `next_entry`.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct LinkInfo {
    /// Index of next `LinkInfo` in `MftIndex::links`, or `NO_ENTRY`
    pub next_entry: u32,
    /// Filename reference
    pub name: IndexNameRef,
    /// Parent directory FRS
    pub parent_frs: u32,
}

// ============================================================================
// SizeInfo - File size information
// ============================================================================

/// File size information (matches C++ `SizeInfo`).
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

/// Alternate Data Stream information (matches C++ `IndexStreamInfo`).
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
    /// Packed flags: `is_sparse`, `type_name_id`
    pub flags: u8,
}

impl IndexStreamInfo {
    /// Returns true if this stream is sparse.
    #[inline]
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        self.flags & 0x01 != 0
    }
    /// Returns the type name ID for this stream.
    #[inline]
    #[must_use]
    pub const fn type_name_id(&self) -> u8 {
        self.flags >> 2
    }
}

// ============================================================================
// ChildInfo - Directory child entry
// ============================================================================

/// Directory child entry (matches C++ `ChildInfo`).
///
/// Directories maintain a linked list of their children for traversal.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ChildInfo {
    /// Index of next `ChildInfo` in `MftIndex::children`, or `NO_ENTRY`
    pub next_entry: u32,
    /// FRS of the child file/directory
    pub child_frs: u32,
    /// Which name index (for hard links)
    pub name_index: u16,
}

// ============================================================================
// FileRecord - Core file metadata (matches C++ Record)
// ============================================================================

/// Core file/directory record (matches C++ `Record`).
///
/// Size: ~80 bytes per record (vs ~200+ bytes with separate bool fields)
#[derive(Debug, Clone, Default)]
#[repr(C)]
pub struct FileRecord {
    /// FRS (File Record Segment) number - primary key
    pub frs: u64,
    /// Timestamps and bit-packed attributes
    pub stdinfo: StandardInfo,
    /// Number of hard links (usually 1)
    pub name_count: u16,
    /// Number of data streams (usually 1)
    pub stream_count: u16,
    /// Index of first child in `MftIndex::children`, or `NO_ENTRY`
    pub first_child: u32,
    /// Primary filename (inline, no allocation)
    pub first_name: LinkInfo,
    /// Primary data stream (inline, no allocation)
    pub first_stream: IndexStreamInfo,
}

impl FileRecord {
    /// Create a new record for the given FRS
    #[must_use]
    pub fn new(frs: u64) -> Self {
        Self {
            frs,
            name_count: 1,   // Every file has at least one name
            stream_count: 1, // Every file has at least the default $DATA stream
            first_child: NO_ENTRY,
            first_name: LinkInfo {
                next_entry: NO_ENTRY,
                name: IndexNameRef {
                    offset: NO_ENTRY,
                    length: 0,
                    flags: 0,
                },
                parent_frs: NO_ENTRY,
            },
            first_stream: IndexStreamInfo {
                next_entry: NO_ENTRY,
                name: IndexNameRef {
                    offset: NO_ENTRY,
                    length: 0,
                    flags: 0,
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
}

// ============================================================================
// MftIndex - The main index structure
// ============================================================================

// ============================================================================
// MftStats - Statistics collected during MFT parsing
// ============================================================================

/// Statistics collected during MFT parsing for optimization.
///
/// These stats are collected incrementally as records are parsed and used to:
/// - Pre-allocate data structures with accurate capacity
/// - Enable fast-path optimizations (skip unnecessary work)
/// - Provide insights for debugging and profiling
///
/// All counters are cheap to update (just incrementing) and don't require
/// the complete MFT to be parsed first.
#[derive(Debug, Clone, Default)]
pub struct MftStats {
    /// Total number of in-use records parsed
    pub record_count: u32,
    /// Number of directory records
    pub dir_count: u32,
    /// Number of file records (= `record_count` - `dir_count`)
    pub file_count: u32,
    /// Maximum FRS seen (for sizing `frs_to_idx` lookup table)
    pub max_frs: u64,
    /// Total bytes of all filenames (for path string pre-allocation)
    pub total_name_bytes: u64,
    /// Number of records with multiple names (hard links)
    pub multi_name_count: u32,
    /// Number of records with ADS (alternate data streams)
    pub ads_count: u32,
    /// Number of system metafiles (FRS < 16, except root)
    pub system_metafile_count: u32,
    /// Number of records whose parent FRS is a system metafile.
    /// These will be filtered out in path resolution.
    pub system_child_count: u32,
}

impl MftStats {
    /// Create new empty stats
    #[must_use]
    pub const fn new() -> Self {
        Self {
            record_count: 0,
            dir_count: 0,
            file_count: 0,
            max_frs: 0,
            total_name_bytes: 0,
            multi_name_count: 0,
            ads_count: 0,
            system_metafile_count: 0,
            system_child_count: 0,
        }
    }

    /// Estimate average path depth based on collected stats.
    ///
    /// Uses heuristic: depth ≈ log2(dirs) + 2.
    /// Typical NTFS volumes have depth 5-15.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn estimated_avg_depth(&self) -> usize {
        if self.dir_count == 0 {
            return 5; // Default for empty/small volumes
        }
        // log2(dir_count) + 2, clamped to reasonable range
        let log2 = f64::from(self.dir_count).log2() as usize;
        (log2 + 2).clamp(3, 20)
    }

    /// Estimate average path length in bytes.
    ///
    /// Formula: (name bytes / records) × depth
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn estimated_avg_path_bytes(&self) -> usize {
        if self.record_count == 0 {
            return 50; // Default
        }
        let avg_name_len = (self.total_name_bytes / u64::from(self.record_count)) as usize;
        let depth = self.estimated_avg_depth();
        // path = "c:/" + depth components with "/" separators
        3 + (avg_name_len + 1) * depth
    }

    /// Check if there are any hard links (multi-name records).
    #[must_use]
    pub const fn has_hard_links(&self) -> bool {
        self.multi_name_count > 0
    }

    /// Check if there are any ADS (alternate data streams).
    #[must_use]
    pub const fn has_ads(&self) -> bool {
        self.ads_count > 0
    }

    /// Estimate number of valid (non-system) records for path cache sizing.
    #[must_use]
    pub const fn valid_record_estimate(&self) -> usize {
        // Subtract system metafiles and their children
        let invalid = self.system_metafile_count + self.system_child_count;
        self.record_count.saturating_sub(invalid) as usize
    }
}

/// Lean in-memory MFT index matching C++ `NtfsIndex` architecture.
///
/// This is the core data structure for fast file searching.
/// It can optionally be converted to a Polars `DataFrame` for analytics.
#[derive(Debug, Default)]
pub struct MftIndex {
    /// Volume letter (e.g., 'C')
    pub volume: char,
    /// All file/directory records
    pub records: Vec<FileRecord>,
    /// FRS → record index lookup (O(1) access)
    /// Value is index into `records`, or `NO_ENTRY` if FRS not present
    pub frs_to_idx: Vec<u32>,
    /// All filenames concatenated (single allocation)
    pub names: String,
    /// Overflow hard link entries (for files with multiple names)
    pub links: Vec<LinkInfo>,
    /// Overflow stream entries (for files with ADS)
    pub streams: Vec<IndexStreamInfo>,
    /// Directory child entries
    pub children: Vec<ChildInfo>,
    /// Statistics collected during parsing
    pub stats: MftStats,
}

impl MftIndex {
    /// Create a new empty index for the given volume
    #[must_use]
    pub fn new(volume: char) -> Self {
        Self {
            volume,
            ..Default::default()
        }
    }

    /// Create with pre-allocated capacity
    #[must_use]
    pub fn with_capacity(volume: char, record_capacity: usize) -> Self {
        Self {
            volume,
            records: Vec::with_capacity(record_capacity),
            frs_to_idx: Vec::with_capacity(record_capacity),
            names: String::with_capacity(record_capacity * 20), // ~20 chars avg
            links: Vec::new(),
            streams: Vec::new(),
            children: Vec::with_capacity(record_capacity),
            stats: MftStats::new(),
        }
    }

    /// Recompute stats from the current index data.
    ///
    /// This is useful after deserializing an index from disk,
    /// or after merging fragments.
    pub fn recompute_stats(&mut self) {
        /// System metafiles are FRS 0-15 (except root at FRS 5)
        const SYSTEM_METAFILE_MAX_FRS: u64 = 15;
        const ROOT_FRS_LOCAL: u64 = 5;

        let mut stats = MftStats::new();

        for record in &self.records {
            stats.record_count += 1;

            // Track max FRS
            if record.frs > stats.max_frs {
                stats.max_frs = record.frs;
            }

            // Count directories vs files
            if record.is_directory() {
                stats.dir_count += 1;
            } else {
                stats.file_count += 1;
            }

            // Count multi-name records (hard links)
            if record.name_count > 1 {
                stats.multi_name_count += 1;
            }

            // Count ADS records
            if record.stream_count > 1 {
                stats.ads_count += 1;
            }

            // System metafile detection
            if record.frs <= SYSTEM_METAFILE_MAX_FRS && record.frs != ROOT_FRS_LOCAL {
                stats.system_metafile_count += 1;
            }

            // Child of system metafile detection
            let parent_frs = u64::from(record.first_name.parent_frs);
            if parent_frs <= SYSTEM_METAFILE_MAX_FRS && parent_frs != ROOT_FRS_LOCAL {
                stats.system_child_count += 1;
            }

            // Sum name bytes
            stats.total_name_bytes += u64::from(record.first_name.name.length);
        }

        self.stats = stats;
    }

    /// Get or create a record for the given FRS.
    ///
    /// Returns a mutable reference to the record. Creates a new record if
    /// one doesn't exist for the given FRS.
    #[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
    pub fn get_or_create(&mut self, frs: u64) -> &mut FileRecord {
        let frs_usize = frs as usize;

        // Expand lookup table if needed
        if frs_usize >= self.frs_to_idx.len() {
            self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
        }

        let idx = self.frs_to_idx[frs_usize];
        if idx == NO_ENTRY {
            // Create new record
            let new_idx = self.records.len() as u32;
            self.frs_to_idx[frs_usize] = new_idx;
            self.records.push(FileRecord::new(frs));
            let len = self.records.len();
            &mut self.records[len - 1]
        } else {
            &mut self.records[idx as usize]
        }
    }

    /// Find a record by FRS (returns None if not present)
    #[must_use]
    pub fn find(&self, frs: u64) -> Option<&FileRecord> {
        let frs_usize = usize::try_from(frs).ok()?;
        let idx = *self.frs_to_idx.get(frs_usize)?;
        if idx == NO_ENTRY {
            None
        } else {
            self.records.get(usize::try_from(idx).ok()?)
        }
    }

    /// Add a filename to the names buffer, return the offset
    pub fn add_name(&mut self, name: &str) -> u32 {
        let offset = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.names.push_str(name);
        offset
    }

    /// Get a filename from the names buffer
    #[must_use]
    #[allow(clippy::string_slice)] // Names are stored as valid UTF-8 at known boundaries
    pub fn get_name(&self, info: &IndexNameRef) -> &str {
        if !info.is_valid() {
            return "";
        }
        let start = info.offset as usize;
        let end = start + info.length as usize;
        self.names.get(start..end).unwrap_or("")
    }

    /// Get the primary name of a record
    #[must_use]
    pub fn record_name(&self, record: &FileRecord) -> &str {
        self.get_name(&record.first_name.name)
    }

    /// Get all records as a slice.
    #[must_use]
    pub fn records(&self) -> &[FileRecord] {
        &self.records
    }

    /// Number of records in the index
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Check if index is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Count files (non-directories)
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.records
            .iter()
            .filter(|rec| !rec.is_directory())
            .count()
    }

    /// Count directories
    #[must_use]
    pub fn dir_count(&self) -> usize {
        self.records.iter().filter(|rec| rec.is_directory()).count()
    }

    /// Memory usage estimate in bytes
    #[must_use]
    pub fn memory_usage(&self) -> usize {
        use core::mem::size_of;
        size_of::<Self>()
            + self.records.capacity() * size_of::<FileRecord>()
            + self.frs_to_idx.capacity() * size_of::<u32>()
            + self.names.capacity()
            + self.links.capacity() * size_of::<LinkInfo>()
            + self.streams.capacity() * size_of::<IndexStreamInfo>()
            + self.children.capacity() * size_of::<ChildInfo>()
    }

    /// Convert FRS to record index (returns None if not present).
    #[must_use]
    pub fn frs_to_idx_opt(&self, frs: u64) -> Option<usize> {
        let frs_usize = usize::try_from(frs).ok()?;
        let idx = *self.frs_to_idx.get(frs_usize)?;
        if idx == NO_ENTRY {
            None
        } else {
            Some(usize::try_from(idx).ok()?)
        }
    }

    /// Get a specific hard link by index (0 = `first_name`, 1+ = overflow
    /// links).
    #[must_use]
    pub fn get_link_at<'a>(
        &'a self,
        record: &'a FileRecord,
        name_idx: u16,
    ) -> Option<&'a LinkInfo> {
        if name_idx == 0 {
            return Some(&record.first_name);
        }
        let mut current = record.first_name.next_entry;
        let mut idx = 1_u16;
        while current != NO_ENTRY {
            let link = self.links.get(current as usize)?;
            if idx == name_idx {
                return Some(link);
            }
            current = link.next_entry;
            idx += 1;
        }
        None
    }

    /// Get the name string for a link.
    #[must_use]
    pub fn link_name(&self, link: &LinkInfo) -> &str {
        self.get_name(&link.name)
    }
}

// ============================================================================
// Name/Stream Iteration (for hard link and ADS expansion)
// ============================================================================

/// Iterator over all names (hard links) for a record.
pub struct NameIter<'a> {
    /// Reference to the index for linked list traversal.
    index: &'a MftIndex,
    /// The first name (inline in the record), consumed on first iteration.
    first: Option<&'a LinkInfo>,
    /// Index into `index.links` for the next entry, or `NO_ENTRY` if done.
    next_entry: u32,
    /// Current iteration index (0-based).
    idx: u16,
}

impl<'a> Iterator for NameIter<'a> {
    type Item = (u16, &'a LinkInfo);

    fn next(&mut self) -> Option<Self::Item> {
        // First iteration: return the inline first_name
        if let Some(first) = self.first.take() {
            let idx = self.idx;
            self.idx += 1;
            self.next_entry = first.next_entry;
            return Some((idx, first));
        }

        // Subsequent iterations: follow the linked list
        if self.next_entry == NO_ENTRY {
            return None;
        }

        let link = self.index.links.get(self.next_entry as usize)?;
        let idx = self.idx;
        self.idx += 1;
        self.next_entry = link.next_entry;
        Some((idx, link))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // We don't know the exact count without traversing
        (1, None)
    }
}

/// Iterator over all streams for a record.
pub struct StreamIter<'a> {
    /// Reference to the index for linked list traversal.
    index: &'a MftIndex,
    /// The first stream (inline in the record), consumed on first iteration.
    first: Option<&'a IndexStreamInfo>,
    /// Index into `index.streams` for the next entry, or `NO_ENTRY` if done.
    next_entry: u32,
    /// Current iteration index (0-based).
    idx: u16,
}

impl<'a> Iterator for StreamIter<'a> {
    type Item = (u16, &'a IndexStreamInfo);

    fn next(&mut self) -> Option<Self::Item> {
        // First iteration: return the inline first_stream
        if let Some(first) = self.first.take() {
            let idx = self.idx;
            self.idx += 1;
            self.next_entry = first.next_entry;
            return Some((idx, first));
        }

        // Subsequent iterations: follow the linked list
        if self.next_entry == NO_ENTRY {
            return None;
        }

        let stream = self.index.streams.get(self.next_entry as usize)?;
        let idx = self.idx;
        self.idx += 1;
        self.next_entry = stream.next_entry;
        Some((idx, stream))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (1, None)
    }
}

impl MftIndex {
    /// Iterate over all names (hard links) for a record.
    ///
    /// Most files have only one name (the primary), but files with hard links
    /// will have multiple entries. Each name has its own parent directory.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Iterator construction is not const-compatible
    pub fn iter_names<'a>(&'a self, record: &'a FileRecord) -> NameIter<'a> {
        NameIter {
            index: self,
            first: Some(&record.first_name),
            next_entry: NO_ENTRY,
            idx: 0,
        }
    }

    /// Iterate over all streams for a record.
    ///
    /// Most files have only the default `$DATA` stream, but files with
    /// Alternate Data Streams (ADS) will have multiple entries.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Iterator construction is not const-compatible
    pub fn iter_streams<'a>(&'a self, record: &'a FileRecord) -> StreamIter<'a> {
        StreamIter {
            index: self,
            first: Some(&record.first_stream),
            next_entry: NO_ENTRY,
            idx: 0,
        }
    }

    /// Get the stream name (empty for default `$DATA` stream).
    #[must_use]
    pub fn stream_name(&self, stream: &IndexStreamInfo) -> &str {
        self.get_name(&stream.name)
    }

    /// Get the Nth name (hard link) for a record.
    ///
    /// Returns `None` if the index is out of bounds.
    #[must_use]
    pub fn get_name_at<'a>(&'a self, record: &'a FileRecord, idx: u16) -> Option<&'a LinkInfo> {
        if idx == 0 {
            return Some(&record.first_name);
        }

        // Walk the linked list to find the Nth entry
        let mut current = record.first_name.next_entry;
        let mut current_idx = 1_u16;

        while current != NO_ENTRY {
            if current_idx == idx {
                return self.links.get(current as usize);
            }
            if let Some(link) = self.links.get(current as usize) {
                current = link.next_entry;
                current_idx += 1;
            } else {
                break;
            }
        }
        None
    }

    /// Get the Nth stream for a record.
    ///
    /// Returns `None` if the index is out of bounds.
    #[must_use]
    pub fn get_stream_at<'a>(
        &'a self,
        record: &'a FileRecord,
        idx: u16,
    ) -> Option<&'a IndexStreamInfo> {
        if idx == 0 {
            return Some(&record.first_stream);
        }

        // Walk the linked list to find the Nth entry
        let mut current = record.first_stream.next_entry;
        let mut current_idx = 1_u16;

        while current != NO_ENTRY {
            if current_idx == idx {
                return self.streams.get(current as usize);
            }
            if let Some(stream) = self.streams.get(current as usize) {
                current = stream.next_entry;
                current_idx += 1;
            } else {
                break;
            }
        }
        None
    }

    /// Build the full path for a specific name (hard link) of a record.
    ///
    /// This handles the case where a file has multiple hard links, each
    /// with a different parent directory and thus a different path.
    #[must_use]
    pub fn build_path_for_name(&self, record: &FileRecord, name_idx: u16) -> String {
        let Some(name_info) = self.get_name_at(record, name_idx) else {
            return self.build_path(record.frs); // Fallback to primary
        };

        let mut components = Vec::new();

        // Add the file's own name
        let name = self.get_name(&name_info.name);
        if !name.is_empty() && name != "." {
            components.push(name.to_owned());
        }

        // Walk up the parent chain from this name's parent
        let mut current_frs = u64::from(name_info.parent_frs);

        while current_frs != u64::from(NO_ENTRY) && current_frs != ROOT_FRS {
            if let Some(parent_record) = self.find(current_frs) {
                let parent_name = self.record_name(parent_record);
                if !parent_name.is_empty() && parent_name != "." {
                    components.push(parent_name.to_owned());
                }

                let parent_frs = parent_record.first_name.parent_frs;
                if parent_frs == NO_ENTRY || u64::from(parent_frs) == current_frs {
                    break;
                }
                current_frs = u64::from(parent_frs);
            } else {
                break;
            }
        }

        // Reverse and join with volume prefix (backslash for C++ parity)
        components.reverse();
        format!(
            "{}:\\{}",
            self.volume.to_ascii_uppercase(),
            components.join("\\")
        )
    }

    /// Build the full path including stream name for ADS.
    ///
    /// Format: `C:/path/to/file:stream_name` for ADS
    /// Format: `C:/path/to/file` for default stream
    #[must_use]
    pub fn build_path_with_stream(
        &self,
        record: &FileRecord,
        name_idx: u16,
        stream: &IndexStreamInfo,
    ) -> String {
        let base_path = self.build_path_for_name(record, name_idx);
        let stream_name = self.stream_name(stream);

        if stream_name.is_empty() {
            base_path
        } else {
            format!("{base_path}:{stream_name}")
        }
    }
}

// ============================================================================
// Path Resolution (on-demand, like C++ ParentIterator)
// ============================================================================

impl MftIndex {
    /// Build the full path for a record by traversing parent chain.
    ///
    /// This is done on-demand (not stored) to save memory.
    #[must_use]
    pub fn build_path(&self, frs: u64) -> String {
        let mut components = Vec::new();
        let mut current_frs = frs;

        // Walk up the parent chain
        while let Some(record) = self.find(current_frs) {
            let name = self.record_name(record);
            if !name.is_empty() && name != "." {
                components.push(name.to_owned());
            }

            // Move to parent
            let parent_frs = record.first_name.parent_frs;
            if parent_frs == NO_ENTRY || u64::from(parent_frs) == current_frs {
                break; // Root or self-reference
            }
            if u64::from(parent_frs) == ROOT_FRS {
                break; // Reached root
            }
            current_frs = u64::from(parent_frs);
        }

        // Reverse and join (backslash for C++ parity)
        components.reverse();
        format!(
            "{}:\\{}",
            self.volume.to_ascii_uppercase(),
            components.join("\\")
        )
    }
}

// ============================================================================
// PathResolver - Ultra-fast path validity and on-demand materialization
// ============================================================================

/// System metafiles are FRS 0-15 (except root at FRS 5).
/// These are filtered out by default to match C++ behavior.
const SYSTEM_METAFILE_MAX_FRS: u64 = 15;

/// State values for path resolution.
mod path_state {
    /// Record has not been visited yet.
    pub const UNSEEN: u8 = 0;
    /// Record is currently being visited (cycle detection).
    pub const VISITING: u8 = 1;
    /// Record has a valid path to root.
    pub const VALID: u8 = 2;
    /// Record is invalid (system metafile, cycle, or descendant of invalid).
    pub const INVALID: u8 = 3;
}

/// Ultra-fast path resolver using dense arrays instead of `HashMap`.
///
/// Key optimizations:
/// 1. Dense `Vec<u8>` state array - O(1) validity check, no hashing
/// 2. BFS illegal propagation - marks descendants in one pass
/// 3. On-demand path materialization - no string allocation until needed
/// 4. `SmallVec` for chain - avoids heap allocation for typical depths
/// 5. Two-pass string building - compute length first, single allocation
#[derive(Debug)]
pub struct PathResolver {
    /// State for each record index (UNSEEN, VISITING, VALID, INVALID).
    state: Vec<u8>,
    /// Volume letter for path prefix.
    volume: char,
    /// Count of valid records.
    valid_count: u32,
    /// Count of invalid records.
    invalid_count: u32,
}

impl PathResolver {
    /// Build the path resolver for all records in the index.
    #[must_use]
    pub fn build(index: &MftIndex, include_system_metafiles: bool) -> Self {
        let n = index.records.len();
        let mut resolver = Self {
            state: vec![path_state::UNSEEN; n],
            volume: index.volume,
            valid_count: 0,
            invalid_count: 0,
        };

        if !include_system_metafiles {
            resolver.mark_system_metafiles_invalid(index);
        }
        resolver.propagate_invalid_to_descendants(index);
        resolver.validate_remaining(index);
        resolver
    }

    /// Check if a record at the given index is valid.
    #[inline]
    #[must_use]
    pub fn is_valid_idx(&self, idx: usize) -> bool {
        self.state.get(idx).copied() == Some(path_state::VALID)
    }

    /// Check if a record with the given FRS is valid.
    #[must_use]
    pub fn is_valid(&self, index: &MftIndex, frs: u64) -> bool {
        index
            .frs_to_idx_opt(frs)
            .is_some_and(|idx| self.is_valid_idx(idx))
    }

    /// Get the number of valid records.
    #[must_use]
    pub const fn valid_count(&self) -> u32 {
        self.valid_count
    }

    /// Get the number of invalid records.
    #[must_use]
    pub const fn invalid_count(&self) -> u32 {
        self.invalid_count
    }

    /// Materialize the full path for a record (on-demand).
    #[must_use]
    // Loop has 4 distinct break conditions: record not found, reached root,
    // self-reference, parent not in index. Cannot be simplified to while_let.
    #[allow(clippy::while_let_loop)]
    pub fn materialize_path(&self, index: &MftIndex, idx: usize) -> String {
        let mut chain: smallvec::SmallVec<[usize; 16]> = smallvec::SmallVec::new();
        let mut current_idx = idx;

        // Walk up parent chain
        loop {
            let Some(record) = index.records.get(current_idx) else {
                break;
            };
            chain.push(current_idx);

            let parent_frs = u64::from(record.first_name.parent_frs);
            if parent_frs == ROOT_FRS
                || parent_frs == record.frs
                || parent_frs == u64::from(NO_ENTRY)
            {
                break;
            }
            let Some(parent_idx) = index.frs_to_idx_opt(parent_frs) else {
                break;
            };
            current_idx = parent_idx;
        }

        // Compute total length
        let mut total_len = 2; // "v:"
        for &chain_idx in &chain {
            if let Some(record) = index.records.get(chain_idx) {
                let name = index.record_name(record);
                if !name.is_empty() && name != "." {
                    total_len += 1 + name.len();
                }
            }
        }

        // Build path with single allocation
        let mut path = String::with_capacity(total_len);
        path.push(self.volume.to_ascii_uppercase());
        path.push(':');

        for &chain_idx in chain.iter().rev() {
            if let Some(record) = index.records.get(chain_idx) {
                let name = index.record_name(record);
                if !name.is_empty() && name != "." {
                    path.push('\\');
                    path.push_str(name);
                }
            }
        }
        path
    }

    /// Materialize path for a specific hard link.
    #[must_use]
    pub fn materialize_path_for_name(&self, index: &MftIndex, idx: usize, name_idx: u16) -> String {
        if name_idx == 0 {
            return self.materialize_path(index, idx);
        }

        let Some(record) = index.records.get(idx) else {
            return String::new();
        };
        let Some(link) = index.get_link_at(record, name_idx) else {
            return self.materialize_path(index, idx);
        };

        let parent_frs = u64::from(link.parent_frs);
        let parent_path = if let Some(pidx) = index.frs_to_idx_opt(parent_frs) {
            self.materialize_path(index, pidx)
        } else if parent_frs == ROOT_FRS {
            format!("{}:", self.volume.to_ascii_uppercase())
        } else {
            return String::new();
        };

        let name = index.link_name(link);
        if name.is_empty() || name == "." {
            parent_path
        } else {
            let mut path = String::with_capacity(parent_path.len() + 1 + name.len());
            path.push_str(&parent_path);
            path.push('\\');
            path.push_str(name);
            path
        }
    }

    /// Mark system metafiles (FRS 0-15 except root) as invalid.
    fn mark_system_metafiles_invalid(&mut self, index: &MftIndex) {
        for (idx, record) in index.records.iter().enumerate() {
            if record.frs <= SYSTEM_METAFILE_MAX_FRS && record.frs != ROOT_FRS {
                if let Some(state) = self.state.get_mut(idx) {
                    *state = path_state::INVALID;
                    self.invalid_count += 1;
                }
            }
        }
    }

    /// BFS propagation: mark all descendants of invalid nodes as invalid.
    fn propagate_invalid_to_descendants(&mut self, index: &MftIndex) {
        use alloc::collections::VecDeque;
        let mut queue: VecDeque<usize> = VecDeque::new();

        for (idx, &state) in self.state.iter().enumerate() {
            if state == path_state::INVALID {
                queue.push_back(idx);
            }
        }

        while let Some(parent_idx) = queue.pop_front() {
            let Some(record) = index.records.get(parent_idx) else {
                continue;
            };
            let mut child_entry = record.first_child;

            while child_entry != NO_ENTRY {
                let Some(child_info) = index.children.get(child_entry as usize) else {
                    break;
                };
                if let Some(child_idx) = index.frs_to_idx_opt(u64::from(child_info.child_frs)) {
                    if let Some(state) = self.state.get_mut(child_idx) {
                        if *state == path_state::UNSEEN {
                            *state = path_state::INVALID;
                            self.invalid_count += 1;
                            queue.push_back(child_idx);
                        }
                    }
                }
                child_entry = child_info.next_entry;
            }
        }
    }

    /// Validate remaining unseen records by walking parent chains.
    fn validate_remaining(&mut self, index: &MftIndex) {
        for start_idx in 0..index.records.len() {
            if self.state.get(start_idx).copied() != Some(path_state::UNSEEN) {
                continue;
            }

            let mut chain: smallvec::SmallVec<[usize; 16]> = smallvec::SmallVec::new();
            let mut current_idx = start_idx;

            let final_state = loop {
                match self.state.get(current_idx).copied() {
                    Some(path_state::VALID) => break path_state::VALID,
                    // INVALID or VISITING (cycle) both result in INVALID
                    Some(path_state::INVALID | path_state::VISITING) => break path_state::INVALID,
                    _ => {}
                }

                if let Some(state) = self.state.get_mut(current_idx) {
                    *state = path_state::VISITING;
                }
                chain.push(current_idx);

                let Some(record) = index.records.get(current_idx) else {
                    break path_state::INVALID;
                };

                let parent_frs = u64::from(record.first_name.parent_frs);

                if parent_frs == ROOT_FRS {
                    break path_state::VALID;
                }
                if parent_frs == record.frs || parent_frs == u64::from(NO_ENTRY) {
                    if record.frs == ROOT_FRS {
                        break path_state::VALID;
                    }
                    break path_state::INVALID;
                }

                let Some(parent_idx) = index.frs_to_idx_opt(parent_frs) else {
                    break path_state::INVALID;
                };
                current_idx = parent_idx;
            };

            for &chain_idx in &chain {
                if let Some(state) = self.state.get_mut(chain_idx) {
                    *state = final_state;
                    if final_state == path_state::VALID {
                        self.valid_count += 1;
                    } else {
                        self.invalid_count += 1;
                    }
                }
            }
        }
    }
}

// ============================================================================
// PathCache - Compatibility wrapper using PathResolver
// ============================================================================

/// Cached path result for a record.
pub type CachedPath = Option<String>;

/// Pre-computed path cache using `PathResolver` internally.
///
/// This is a compatibility wrapper that provides the same API as the old
/// `PathCache` but uses the much faster `PathResolver` under the hood.
///
/// ## Usage
///
/// ```ignore
/// let cache = PathCache::build(&index, false);
/// if let Some(path) = cache.get(record.frs) {
///     println!("Valid: {}", path);
/// }
/// ```
#[derive(Debug)]
pub struct PathCache<'a> {
    /// The underlying path resolver.
    resolver: PathResolver,
    /// Reference to the MFT index.
    index: &'a MftIndex,
}

impl<'a> PathCache<'a> {
    /// Build the path cache for all records in the index.
    #[must_use]
    pub fn build(index: &'a MftIndex, include_system_metafiles: bool) -> Self {
        Self {
            resolver: PathResolver::build(index, include_system_metafiles),
            index,
        }
    }

    /// Get the path for a record (materializes on demand).
    #[must_use]
    pub fn get(&self, frs: u64) -> Option<String> {
        let idx = self.index.frs_to_idx_opt(frs)?;
        self.resolver
            .is_valid_idx(idx)
            .then(|| self.resolver.materialize_path(self.index, idx))
    }

    /// Check if a record is valid (has a path, not illegal).
    #[must_use]
    pub fn is_valid(&self, frs: u64) -> bool {
        self.resolver.is_valid(self.index, frs)
    }

    /// Check if a record is illegal (filtered out).
    #[must_use]
    pub fn is_illegal(&self, frs: u64) -> bool {
        self.index
            .frs_to_idx_opt(frs)
            .is_some_and(|idx| !self.resolver.is_valid_idx(idx))
    }

    /// Get the number of valid (non-illegal) records.
    #[must_use]
    pub const fn valid_count(&self) -> usize {
        self.resolver.valid_count() as usize
    }

    /// Get the number of illegal records.
    #[must_use]
    pub const fn illegal_count(&self) -> usize {
        self.resolver.invalid_count() as usize
    }

    /// Get the underlying resolver for direct access.
    #[must_use]
    pub const fn resolver(&self) -> &PathResolver {
        &self.resolver
    }

    /// Get the index reference.
    #[must_use]
    pub const fn index(&self) -> &MftIndex {
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_info_flags() {
        let mut info = StandardInfo::default();
        assert!(!info.is_directory());

        info.set_directory(true);
        assert!(info.is_directory());

        info.set_directory(false);
        assert!(!info.is_directory());
    }

    #[test]
    fn test_file_record_size() {
        // Verify compact size - should be reasonably compact (< 150 bytes)
        let size = size_of::<FileRecord>();
        assert!(size < 150, "FileRecord too large: {size} bytes");
    }

    #[test]
    fn test_index_basic_operations() {
        let mut index = MftIndex::new('C');

        // Add a record
        let record = index.get_or_create(100);
        record.stdinfo.set_directory(true);

        // Find it
        let found = index.find(100);
        assert!(found.is_some());
        assert!(found.unwrap().is_directory());

        // Not found
        assert!(index.find(999).is_none());
    }

    #[test]
    fn test_names_buffer() {
        let mut index = MftIndex::new('C');

        let offset1 = index.add_name("test.txt");
        let offset2 = index.add_name("hello.rs");

        let info1 = IndexNameRef::new(offset1, 8, true);
        let info2 = IndexNameRef::new(offset2, 8, true);

        assert_eq!(index.get_name(&info1), "test.txt");
        assert_eq!(index.get_name(&info2), "hello.rs");
    }
}

// ============================================================================
// Polars DataFrame Conversion (optional, on-demand)
// ============================================================================

#[cfg(windows)]
impl MftIndex {
    /// Convert the lean index to a Polars `DataFrame`.
    ///
    /// This is an **optional** conversion for when you need:
    /// - Complex SQL-like queries
    /// - Analytics and aggregations
    /// - Export to Parquet/CSV
    ///
    /// For simple searches, use the lean index directly (faster).
    pub fn to_dataframe(&self) -> crate::Result<uffs_polars::DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

        // Pre-allocate vectors
        let cap = self.records.len();
        let mut frs_vec: Vec<u64> = Vec::with_capacity(cap);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(cap);
        let mut name_vec: Vec<String> = Vec::with_capacity(cap);
        let mut size_vec: Vec<u64> = Vec::with_capacity(cap);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(cap);
        let mut created_vec: Vec<i64> = Vec::with_capacity(cap);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(cap);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(cap);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(cap);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(cap);
        let mut flags_vec: Vec<u16> = Vec::with_capacity(cap);

        // Extract data from records
        for record in &self.records {
            frs_vec.push(record.frs);
            parent_frs_vec.push(record.first_name.parent_frs as u64);
            name_vec.push(self.record_name(record).to_string());
            size_vec.push(record.first_stream.size.length);
            allocated_size_vec.push(record.first_stream.size.allocated);
            created_vec.push(record.stdinfo.created);
            modified_vec.push(record.stdinfo.modified);
            accessed_vec.push(record.stdinfo.accessed);
            mft_changed_vec.push(record.stdinfo.mft_changed);
            is_directory_vec.push(record.stdinfo.is_directory());
            is_readonly_vec.push(record.stdinfo.is_readonly());
            is_hidden_vec.push(record.stdinfo.is_hidden());
            is_system_vec.push(record.stdinfo.is_system());
            is_compressed_vec.push(record.stdinfo.is_compressed());
            is_encrypted_vec.push(record.stdinfo.is_encrypted());
            is_sparse_vec.push(record.stdinfo.is_sparse());
            is_reparse_vec.push(record.stdinfo.is_reparse());
            flags_vec.push(record.stdinfo.to_attributes() as u16);
        }

        // Build DataFrame columns
        let columns = vec![
            Series::new("frs".into(), frs_vec).into_column(),
            Series::new("parent_frs".into(), parent_frs_vec).into_column(),
            Series::new("name".into(), name_vec).into_column(),
            Series::new("size".into(), size_vec).into_column(),
            Series::new("allocated_size".into(), allocated_size_vec).into_column(),
            Series::new("created".into(), created_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), modified_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), accessed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("mft_changed".into(), mft_changed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("is_directory".into(), is_directory_vec).into_column(),
            Series::new("is_readonly".into(), is_readonly_vec).into_column(),
            Series::new("is_hidden".into(), is_hidden_vec).into_column(),
            Series::new("is_system".into(), is_system_vec).into_column(),
            Series::new("is_compressed".into(), is_compressed_vec).into_column(),
            Series::new("is_encrypted".into(), is_encrypted_vec).into_column(),
            Series::new("is_sparse".into(), is_sparse_vec).into_column(),
            Series::new("is_reparse".into(), is_reparse_vec).into_column(),
            Series::new("flags".into(), flags_vec).into_column(),
        ];

        uffs_polars::DataFrame::new_infer_height(columns).map_err(crate::MftError::from)
    }
}

// ============================================================================
// Building MftIndex from ParsedRecords
// ============================================================================

#[cfg(windows)]
impl MftIndex {
    /// Build an `MftIndex` from a vector of parsed records.
    ///
    /// This is the fast path - directly builds the lean index without
    /// going through Polars DataFrame.
    pub fn from_parsed_records(volume: char, records: Vec<crate::io::ParsedRecord>) -> Self {
        /// System metafiles are FRS 0-15 (except root at FRS 5)
        const SYSTEM_METAFILE_MAX_FRS: u64 = 15;
        const ROOT_FRS_LOCAL: u64 = 5;

        let capacity = records.len();
        let mut index = Self::with_capacity(volume, capacity);

        for parsed in records {
            if !parsed.in_use {
                continue;
            }

            // === Collect stats (cheap - just incrementing counters) ===
            index.stats.record_count += 1;
            index.stats.total_name_bytes += parsed.name.len() as u64;
            if parsed.frs > index.stats.max_frs {
                index.stats.max_frs = parsed.frs;
            }
            if parsed.is_directory {
                index.stats.dir_count += 1;
            } else {
                index.stats.file_count += 1;
            }
            if parsed.names.len() > 1 {
                index.stats.multi_name_count += 1;
            }
            if parsed.streams.len() > 1 {
                index.stats.ads_count += 1;
            }
            // System metafile detection
            if parsed.frs <= SYSTEM_METAFILE_MAX_FRS && parsed.frs != ROOT_FRS_LOCAL {
                index.stats.system_metafile_count += 1;
            }
            // Child of system metafile detection
            if parsed.parent_frs <= SYSTEM_METAFILE_MAX_FRS && parsed.parent_frs != ROOT_FRS_LOCAL {
                index.stats.system_child_count += 1;
            }

            // Add primary name to names buffer FIRST (before borrowing record)
            let name_offset = index.add_name(&parsed.name);
            let name_len = parsed.name.len() as u16;
            let is_ascii = parsed.name.is_ascii();

            // Get or create the record
            let record = index.get_or_create(parsed.frs);

            // Set timestamps and flags
            record.stdinfo.created = parsed.std_info.created;
            record.stdinfo.modified = parsed.std_info.modified;
            record.stdinfo.accessed = parsed.std_info.accessed;
            record.stdinfo.mft_changed = parsed.std_info.mft_changed;
            record.stdinfo.set_directory(parsed.is_directory);

            // Set attribute flags from ExtendedStandardInfo
            if parsed.std_info.is_readonly {
                record.stdinfo.flags |= StandardInfo::IS_READONLY;
            }
            if parsed.std_info.is_archive {
                record.stdinfo.flags |= StandardInfo::IS_ARCHIVE;
            }
            if parsed.std_info.is_system {
                record.stdinfo.flags |= StandardInfo::IS_SYSTEM;
            }
            if parsed.std_info.is_hidden {
                record.stdinfo.flags |= StandardInfo::IS_HIDDEN;
            }
            if parsed.std_info.is_offline {
                record.stdinfo.flags |= StandardInfo::IS_OFFLINE;
            }
            if parsed.std_info.is_not_content_indexed {
                record.stdinfo.flags |= StandardInfo::IS_NOT_INDEXED;
            }
            if parsed.std_info.is_compressed {
                record.stdinfo.flags |= StandardInfo::IS_COMPRESSED;
            }
            if parsed.std_info.is_encrypted {
                record.stdinfo.flags |= StandardInfo::IS_ENCRYPTED;
            }
            if parsed.std_info.is_sparse {
                record.stdinfo.flags |= StandardInfo::IS_SPARSE;
            }
            if parsed.std_info.is_reparse {
                record.stdinfo.flags |= StandardInfo::IS_REPARSE;
            }
            if parsed.std_info.is_temporary {
                record.stdinfo.flags |= StandardInfo::IS_TEMPORARY;
            }
            if parsed.std_info.is_integrity_stream {
                record.stdinfo.flags |= StandardInfo::IS_INTEGRITY_STREAM;
            }
            if parsed.std_info.is_no_scrub_data {
                record.stdinfo.flags |= StandardInfo::IS_NO_SCRUB_DATA;
            }
            if parsed.std_info.is_pinned {
                record.stdinfo.flags |= StandardInfo::IS_PINNED;
            }
            if parsed.std_info.is_unpinned {
                record.stdinfo.flags |= StandardInfo::IS_UNPINNED;
            }
            if parsed.std_info.is_virtual {
                record.stdinfo.flags |= StandardInfo::IS_VIRTUAL;
            }

            // Set name info (offset was computed before borrowing record)
            record.first_name.name = IndexNameRef::new(name_offset, name_len, is_ascii);
            record.first_name.parent_frs = parsed.parent_frs as u32;
            record.name_count = parsed.names.len() as u16;

            // Set size from first stream
            if !parsed.streams.is_empty() {
                record.first_stream.size.length = parsed.size;
                record.first_stream.size.allocated = parsed.allocated_size;
                record.stream_count = parsed.streams.len() as u16;
            }

            // Build parent-child relationship
            if parsed.parent_frs != parsed.frs && parsed.parent_frs != NO_ENTRY as u64 {
                // Ensure parent exists
                let parent_idx = {
                    let parent_frs_usize = parsed.parent_frs as usize;
                    if parent_frs_usize >= index.frs_to_idx.len() {
                        index.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
                    }
                    if index.frs_to_idx[parent_frs_usize] == NO_ENTRY {
                        // Create placeholder parent
                        let new_idx = index.records.len() as u32;
                        index.frs_to_idx[parent_frs_usize] = new_idx;
                        index.records.push(FileRecord::new(parsed.parent_frs));
                    }
                    index.frs_to_idx[parent_frs_usize]
                };

                // Add child entry
                let child_idx = index.children.len() as u32;

                // Get parent's first_child and update
                let parent = &mut index.records[parent_idx as usize];
                let old_first_child = parent.first_child;
                parent.first_child = child_idx;

                index.children.push(ChildInfo {
                    next_entry: old_first_child,
                    child_frs: parsed.frs as u32,
                    name_index: 0,
                });
            }
        }

        index
    }

    /// Merge multiple index fragments into this index.
    ///
    /// This is used for parallel parsing where each worker builds a local
    /// fragment, then all fragments are merged into the final index.
    ///
    /// # Performance
    ///
    /// O(n) merge - each fragment is processed once. The merge handles:
    /// - Deduplication of records (same FRS from different fragments)
    /// - Name buffer concatenation with offset adjustment
    /// - Link/stream/child list merging
    pub fn merge_fragments(&mut self, fragments: Vec<MftIndexFragment>) {
        use tracing::debug;

        let total_records: usize = fragments.iter().map(|f| f.records.len()).sum();
        let total_names: usize = fragments.iter().map(|f| f.names.len()).sum();

        debug!(
            fragments = fragments.len(),
            total_records, total_names, "🔀 Merging index fragments"
        );

        // Reserve capacity
        self.records.reserve(total_records);
        self.names.reserve(total_names);

        for fragment in fragments {
            let name_offset_adjustment = self.names.len() as u32;

            // Append names buffer
            self.names.push_str(&fragment.names);

            // Merge records
            for mut record in fragment.records {
                let frs = record.frs;
                let frs_usize = frs as usize;

                // Adjust name offsets
                if record.first_name.name.is_valid() {
                    record.first_name.name.offset += name_offset_adjustment;
                }
                if record.first_stream.name.is_valid() {
                    record.first_stream.name.offset += name_offset_adjustment;
                }

                // Expand lookup table if needed
                if frs_usize >= self.frs_to_idx.len() {
                    self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
                }

                let existing_idx = self.frs_to_idx[frs_usize];
                if existing_idx == NO_ENTRY {
                    // New record - add it
                    let new_idx = self.records.len() as u32;
                    self.frs_to_idx[frs_usize] = new_idx;
                    self.records.push(record);
                } else {
                    // Record exists - merge (keep the one with more data)
                    let existing = &mut self.records[existing_idx as usize];
                    // If existing is a placeholder (no name), replace with new
                    if !existing.has_name() && record.has_name() {
                        *existing = record;
                    }
                    // Otherwise keep existing (first wins)
                }
            }

            // Merge links (with offset adjustment)
            let link_offset_adjustment = self.links.len() as u32;
            for mut link in fragment.links {
                if link.name.is_valid() {
                    link.name.offset += name_offset_adjustment;
                }
                if link.next_entry != NO_ENTRY {
                    link.next_entry += link_offset_adjustment;
                }
                self.links.push(link);
            }

            // Merge streams (with offset adjustment)
            let stream_offset_adjustment = self.streams.len() as u32;
            for mut stream in fragment.streams {
                if stream.name.is_valid() {
                    stream.name.offset += name_offset_adjustment;
                }
                if stream.next_entry != NO_ENTRY {
                    stream.next_entry += stream_offset_adjustment;
                }
                self.streams.push(stream);
            }

            // Merge children (with offset adjustment)
            let child_offset_adjustment = self.children.len() as u32;
            for mut child in fragment.children {
                if child.next_entry != NO_ENTRY {
                    child.next_entry += child_offset_adjustment;
                }
                self.children.push(child);
            }
        }

        debug!(
            records = self.records.len(),
            names_kb = self.names.len() / 1024,
            "✅ Fragment merge complete"
        );
    }
}

// ============================================================================
// USN Journal Incremental Update Support
// ============================================================================

/// Statistics from applying USN changes to an index.
#[derive(Debug, Clone, Default)]
pub struct UsnApplyStats {
    /// Number of records marked as deleted
    pub deleted: usize,
    /// Number of records created (placeholder)
    pub created: usize,
    /// Number of records modified (name/metadata)
    pub modified: usize,
    /// Number of changes skipped (FRS not in index)
    pub skipped: usize,
}

impl MftIndex {
    /// Applies USN Journal changes to update the index incrementally.
    ///
    /// This is much faster than a full MFT scan for typical workloads where
    /// only a small percentage of files change between runs.
    ///
    /// # Limitations
    ///
    /// - **Deletes**: Marks records as deleted (sets flags)
    /// - **Creates**: Creates placeholder records (limited info from USN)
    /// - **Renames**: Updates filename if FRS exists
    /// - **Metadata**: Marks as modified (actual values need MFT read)
    ///
    /// For full accuracy on creates/renames, a selective MFT read would be
    /// needed. This implementation provides a fast approximation that's
    /// sufficient for most search use cases.
    #[allow(clippy::cast_possible_truncation)]
    pub fn apply_usn_changes(&mut self, changes: &[crate::usn::FileChange]) -> UsnApplyStats {
        // DELETED flag uses bit 31 of the u32 flags field
        const DELETED_FLAG: u32 = 0x8000_0000;

        let mut stats = UsnApplyStats::default();

        for change in changes {
            let frs = change.frs;
            let frs_usize = frs as usize;

            // Check if FRS is in our lookup table
            let idx = self.frs_to_idx.get(frs_usize).copied().unwrap_or(NO_ENTRY);

            if change.deleted {
                // Handle deletion
                if idx == NO_ENTRY {
                    stats.skipped += 1;
                } else if let Some(record) = self.records.get_mut(idx as usize) {
                    // Mark as deleted using bit 31 of flags
                    record.stdinfo.flags |= DELETED_FLAG;
                    stats.deleted += 1;
                }
            } else if change.created {
                // Handle creation - create placeholder record
                // Note: We only have limited info from USN (FRS, parent, name)
                // For full info, we'd need to read the actual MFT record
                if idx == NO_ENTRY {
                    // Expand lookup table if needed
                    if frs_usize >= self.frs_to_idx.len() {
                        self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
                    }

                    // Create new record
                    let new_idx = self.records.len() as u32;
                    if let Some(slot) = self.frs_to_idx.get_mut(frs_usize) {
                        *slot = new_idx;
                    }

                    // Add filename to names buffer
                    let name_start = self.names.len() as u32;
                    self.names.push_str(&change.filename);
                    let name_len = change.filename.len() as u16;

                    // Truncate parent_frs to u32 (FRS values fit in 32 bits for most volumes)
                    let parent_frs_u32 = change.parent_frs as u32;

                    // Create placeholder record
                    let record = FileRecord {
                        frs,
                        stdinfo: StandardInfo::default(),
                        name_count: 1,
                        stream_count: 1, // Every file has at least the default $DATA stream
                        first_child: NO_ENTRY,
                        first_name: LinkInfo {
                            next_entry: NO_ENTRY,
                            name: IndexNameRef::new(
                                name_start,
                                name_len,
                                change.filename.is_ascii(),
                            ),
                            parent_frs: parent_frs_u32,
                        },
                        first_stream: IndexStreamInfo::default(),
                    };
                    self.records.push(record);
                    stats.created += 1;
                } else {
                    // FRS already exists - might be a re-create after delete
                    // Clear the deleted flag if it was set
                    if let Some(record) = self.records.get_mut(idx as usize) {
                        record.stdinfo.flags &= !DELETED_FLAG;
                    }
                    stats.skipped += 1;
                }
            } else if change.renamed {
                // Handle rename - update filename
                if idx == NO_ENTRY {
                    stats.skipped += 1;
                } else if let Some(record) = self.records.get_mut(idx as usize) {
                    // Update the primary name
                    // Note: This appends to names buffer (old name becomes orphaned)
                    // A compaction pass could reclaim this space if needed
                    let name_start = self.names.len() as u32;
                    self.names.push_str(&change.filename);
                    let name_len = change.filename.len() as u16;

                    record.first_name.name =
                        IndexNameRef::new(name_start, name_len, change.filename.is_ascii());
                    record.first_name.parent_frs = change.parent_frs as u32;
                    stats.modified += 1;
                }
            } else if change.size_changed || change.metadata_changed {
                // Handle size/metadata change
                // We can't update the actual values without reading MFT
                // Just mark as modified for now
                if idx == NO_ENTRY {
                    stats.skipped += 1;
                } else {
                    stats.modified += 1;
                }
            } else {
                stats.skipped += 1;
            }
        }

        stats
    }
}

// ============================================================================
// MftIndexFragment - Partial index for parallel parsing
// ============================================================================

/// A partial MFT index built by a worker thread during parallel parsing.
///
/// Each worker builds its own fragment, which is later merged into the
/// final `MftIndex`. This avoids contention on a shared index.
///
/// # Thread Safety
///
/// Fragments are built by a single thread and then moved to the merge
/// thread. No synchronization is needed during building.
#[derive(Debug, Default)]
pub struct MftIndexFragment {
    /// File records parsed by this worker
    pub records: Vec<FileRecord>,
    /// FRS → record index lookup (local to this fragment)
    pub frs_to_idx: Vec<u32>,
    /// Filenames concatenated (local buffer)
    pub names: String,
    /// Overflow hard link entries
    pub links: Vec<LinkInfo>,
    /// Overflow stream entries
    pub streams: Vec<IndexStreamInfo>,
    /// Directory child entries
    pub children: Vec<ChildInfo>,
}

impl MftIndexFragment {
    /// Create a new empty fragment with estimated capacity.
    #[must_use]
    pub fn with_capacity(record_capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(record_capacity),
            frs_to_idx: Vec::with_capacity(record_capacity),
            names: String::with_capacity(record_capacity * 20), // ~20 chars avg
            links: Vec::new(),
            streams: Vec::new(),
            children: Vec::with_capacity(record_capacity / 10), // ~10% are dirs
        }
    }

    /// Get or create a record for the given FRS.
    #[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
    pub fn get_or_create(&mut self, frs: u64) -> &mut FileRecord {
        let frs_usize = frs as usize;

        // Expand lookup table if needed
        if frs_usize >= self.frs_to_idx.len() {
            self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
        }

        let idx = self.frs_to_idx[frs_usize];
        if idx == NO_ENTRY {
            // Create new record
            let new_idx = self.records.len() as u32;
            self.frs_to_idx[frs_usize] = new_idx;
            self.records.push(FileRecord::new(frs));
            let len = self.records.len();
            &mut self.records[len - 1]
        } else {
            &mut self.records[idx as usize]
        }
    }

    /// Add a filename to the names buffer, return the offset.
    pub fn add_name(&mut self, name: &str) -> u32 {
        let offset = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.names.push_str(name);
        offset
    }

    /// Number of records in this fragment.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Check if fragment is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

// ============================================================================
// M5: Persistent Index Storage
// ============================================================================

/// Magic bytes for index file format.
const INDEX_MAGIC: &[u8; 8] = b"UFFSIDX\0";

/// Current index file format version.
const INDEX_VERSION: u32 = 1;

/// Persistent index header stored at the beginning of the index file.
#[derive(Debug, Clone)]
pub struct IndexHeader {
    /// Magic bytes for format identification
    pub magic: [u8; 8],
    /// Format version for compatibility
    pub version: u32,
    /// Volume letter (e.g., 'C')
    pub volume: char,
    /// Volume serial number for validation
    pub volume_serial: u64,
    /// USN Journal ID at time of index creation
    pub usn_journal_id: u64,
    /// Next USN to read from (checkpoint)
    pub next_usn: i64,
    /// Timestamp when index was created (Unix epoch seconds)
    pub created_at: u64,
    /// Number of records in the index
    pub record_count: u64,
    /// Size of names buffer in bytes
    pub names_size: u64,
    /// Number of link entries
    pub links_count: u64,
    /// Number of stream entries
    pub streams_count: u64,
    /// Number of children entries
    pub children_count: u64,
}

impl IndexHeader {
    /// Creates a new header for the given index.
    #[must_use]
    pub fn new(index: &MftIndex, volume_serial: u64, usn_journal_id: u64, next_usn: i64) -> Self {
        Self {
            magic: *INDEX_MAGIC,
            version: INDEX_VERSION,
            volume: index.volume,
            volume_serial,
            usn_journal_id,
            next_usn,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|dur| dur.as_secs())
                .unwrap_or(0),
            record_count: index.records.len() as u64,
            names_size: index.names.len() as u64,
            links_count: index.links.len() as u64,
            streams_count: index.streams.len() as u64,
            children_count: index.children.len() as u64,
        }
    }

    /// Validates the header magic and version.
    ///
    /// # Errors
    ///
    /// Returns an error if the magic bytes are invalid or the version is
    /// unsupported.
    pub fn validate(&self) -> Result<(), &'static str> {
        if &self.magic != INDEX_MAGIC {
            return Err("Invalid index file magic");
        }
        if self.version != INDEX_VERSION {
            return Err("Unsupported index version");
        }
        Ok(())
    }
}

impl MftIndex {
    /// Serializes the index to a byte vector.
    ///
    /// Format:
    /// - Header (fixed size)
    /// - `frs_to_idx` table (u32 array)
    /// - records (`FileRecord` array)
    /// - names (UTF-8 string)
    /// - links (`LinkInfo` array)
    /// - streams (`IndexStreamInfo` array)
    /// - children (`ChildInfo` array)
    ///
    /// # Arguments
    ///
    /// * `volume_serial` - Volume serial number for validation
    /// * `usn_journal_id` - USN Journal ID at time of serialization
    /// * `next_usn` - Next USN to read from (checkpoint)
    #[must_use]
    pub fn serialize(&self, volume_serial: u64, usn_journal_id: u64, next_usn: i64) -> Vec<u8> {
        let header = IndexHeader::new(self, volume_serial, usn_journal_id, next_usn);

        // Estimate size (rough estimate for capacity)
        let estimated_size = 128 // header
            + self.frs_to_idx.len() * 4
            + self.records.len() * 128 // rough estimate per record
            + self.names.len()
            + self.links.len() * 24
            + self.streams.len() * 32
            + self.children.len() * 16;

        let mut buffer = Vec::with_capacity(estimated_size);

        // Write header
        buffer.extend_from_slice(&header.magic);
        buffer.extend_from_slice(&header.version.to_le_bytes());
        buffer.extend_from_slice(&(header.volume as u32).to_le_bytes());
        buffer.extend_from_slice(&header.volume_serial.to_le_bytes());
        buffer.extend_from_slice(&header.usn_journal_id.to_le_bytes());
        buffer.extend_from_slice(&header.next_usn.to_le_bytes());
        buffer.extend_from_slice(&header.created_at.to_le_bytes());
        buffer.extend_from_slice(&header.record_count.to_le_bytes());
        buffer.extend_from_slice(&header.names_size.to_le_bytes());
        buffer.extend_from_slice(&header.links_count.to_le_bytes());
        buffer.extend_from_slice(&header.streams_count.to_le_bytes());
        buffer.extend_from_slice(&header.children_count.to_le_bytes());

        // Write frs_to_idx table size and data
        buffer.extend_from_slice(&(self.frs_to_idx.len() as u64).to_le_bytes());
        for &idx in &self.frs_to_idx {
            buffer.extend_from_slice(&idx.to_le_bytes());
        }

        // Write records
        for record in &self.records {
            // FileRecord fields
            buffer.extend_from_slice(&record.frs.to_le_bytes());
            // StandardInfo
            buffer.extend_from_slice(&record.stdinfo.created.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.modified.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.accessed.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.mft_changed.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.flags.to_le_bytes());
            // Counts
            buffer.extend_from_slice(&record.name_count.to_le_bytes());
            buffer.extend_from_slice(&record.stream_count.to_le_bytes());
            buffer.extend_from_slice(&record.first_child.to_le_bytes());
            // first_name (LinkInfo)
            buffer.extend_from_slice(&record.first_name.next_entry.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.name.offset.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.name.length.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.name.flags.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.parent_frs.to_le_bytes());
            // first_stream (IndexStreamInfo)
            buffer.extend_from_slice(&record.first_stream.size.length.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.size.allocated.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.next_entry.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.name.offset.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.name.length.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.name.flags.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.flags.to_le_bytes());
        }

        // Write names
        buffer.extend_from_slice(self.names.as_bytes());

        // Write links (overflow links, not first_name)
        for link in &self.links {
            buffer.extend_from_slice(&link.next_entry.to_le_bytes());
            buffer.extend_from_slice(&link.name.offset.to_le_bytes());
            buffer.extend_from_slice(&link.name.length.to_le_bytes());
            buffer.extend_from_slice(&link.name.flags.to_le_bytes());
            buffer.extend_from_slice(&link.parent_frs.to_le_bytes());
        }

        // Write streams (overflow streams, not first_stream)
        for stream in &self.streams {
            buffer.extend_from_slice(&stream.size.length.to_le_bytes());
            buffer.extend_from_slice(&stream.size.allocated.to_le_bytes());
            buffer.extend_from_slice(&stream.next_entry.to_le_bytes());
            buffer.extend_from_slice(&stream.name.offset.to_le_bytes());
            buffer.extend_from_slice(&stream.name.length.to_le_bytes());
            buffer.extend_from_slice(&stream.name.flags.to_le_bytes());
            buffer.extend_from_slice(&stream.flags.to_le_bytes());
        }

        // Write children
        for child in &self.children {
            buffer.extend_from_slice(&child.next_entry.to_le_bytes());
            buffer.extend_from_slice(&child.child_frs.to_le_bytes());
            buffer.extend_from_slice(&child.name_index.to_le_bytes());
        }

        buffer
    }

    /// Deserializes an index from a byte slice.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is corrupted or incompatible.
    // This function is intentionally long to keep all deserialization logic together
    // for performance and maintainability. Splitting would add function call overhead
    // and make the binary format harder to follow.
    // The u64->usize casts are safe: this is a 64-bit Windows NTFS tool.
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    pub fn deserialize(data: &[u8]) -> Result<(Self, IndexHeader), &'static str> {
        if data.len() < 96 {
            return Err("Data too short for header");
        }

        let mut pos = 0;

        // Helper macro to read bytes safely
        macro_rules! read_u8 {
            () => {{
                let val = *data.get(pos).ok_or("Unexpected end of data")?;
                pos += 1;
                val
            }};
        }
        macro_rules! read_u16 {
            () => {{
                let bytes: [u8; 2] = data
                    .get(pos..pos + 2)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_| "Invalid u16 slice")?;
                let val = u16::from_le_bytes(bytes);
                pos += 2;
                val
            }};
        }
        macro_rules! read_u32 {
            () => {{
                let bytes: [u8; 4] = data
                    .get(pos..pos + 4)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_| "Invalid u32 slice")?;
                let val = u32::from_le_bytes(bytes);
                pos += 4;
                val
            }};
        }
        macro_rules! read_u64 {
            () => {{
                let bytes: [u8; 8] = data
                    .get(pos..pos + 8)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_| "Invalid u64 slice")?;
                let val = u64::from_le_bytes(bytes);
                pos += 8;
                val
            }};
        }
        macro_rules! read_i64 {
            () => {{
                let bytes: [u8; 8] = data
                    .get(pos..pos + 8)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_| "Invalid i64 slice")?;
                let val = i64::from_le_bytes(bytes);
                pos += 8;
                val
            }};
        }

        // Read header
        let mut magic = [0_u8; 8];
        magic.copy_from_slice(data.get(pos..pos + 8).ok_or("Unexpected end of data")?);
        pos += 8;

        let version = read_u32!();
        let volume = char::from_u32(read_u32!()).ok_or("Invalid volume char")?;
        let volume_serial = read_u64!();
        let usn_journal_id = read_u64!();
        let next_usn = read_i64!();
        let created_at = read_u64!();
        let record_count = read_u64!();
        let names_size = read_u64!();
        let links_count = read_u64!();
        let streams_count = read_u64!();
        let children_count = read_u64!();

        let header = IndexHeader {
            magic,
            version,
            volume,
            volume_serial,
            usn_journal_id,
            next_usn,
            created_at,
            record_count,
            names_size,
            links_count,
            streams_count,
            children_count,
        };

        header.validate()?;

        // Read frs_to_idx table
        let frs_to_idx_len = read_u64!() as usize;
        let mut frs_to_idx = Vec::with_capacity(frs_to_idx_len);
        for _ in 0..frs_to_idx_len {
            frs_to_idx.push(read_u32!());
        }

        // Read records
        let mut records = Vec::with_capacity(record_count as usize);
        for _ in 0..record_count {
            let frs = read_u64!();
            // StandardInfo
            let created = read_i64!();
            let modified = read_i64!();
            let accessed = read_i64!();
            let mft_changed = read_i64!();
            let flags = read_u32!();
            // Counts
            let name_count = read_u16!();
            let rec_stream_count = read_u16!();
            let first_child = read_u32!();
            // first_name (LinkInfo)
            let link_next_entry = read_u32!();
            let link_name_offset = read_u32!();
            let link_name_length = read_u16!();
            let link_name_flags = read_u16!();
            let link_parent_frs = read_u32!();
            // first_stream (IndexStreamInfo)
            let stream_size_length = read_u64!();
            let stream_size_allocated = read_u64!();
            let stream_next_entry = read_u32!();
            let stream_name_offset = read_u32!();
            let stream_name_length = read_u16!();
            let stream_name_flags = read_u16!();
            let stream_flags = read_u8!();

            records.push(FileRecord {
                frs,
                stdinfo: StandardInfo {
                    created,
                    modified,
                    accessed,
                    mft_changed,
                    flags,
                },
                name_count,
                stream_count: rec_stream_count,
                first_child,
                first_name: LinkInfo {
                    next_entry: link_next_entry,
                    name: IndexNameRef {
                        offset: link_name_offset,
                        length: link_name_length,
                        flags: link_name_flags,
                    },
                    parent_frs: link_parent_frs,
                },
                first_stream: IndexStreamInfo {
                    size: SizeInfo {
                        length: stream_size_length,
                        allocated: stream_size_allocated,
                    },
                    next_entry: stream_next_entry,
                    name: IndexNameRef {
                        offset: stream_name_offset,
                        length: stream_name_length,
                        flags: stream_name_flags,
                    },
                    flags: stream_flags,
                },
            });
        }

        // Read names
        let names_end = pos + names_size as usize;
        let names_bytes = data.get(pos..names_end).ok_or("Unexpected end of data")?;
        let names = String::from_utf8(names_bytes.to_vec())
            .map_err(|_utf8_err| "Invalid UTF-8 in names")?;
        pos = names_end;

        // Read links (overflow links)
        let mut links = Vec::with_capacity(links_count as usize);
        for _ in 0..links_count {
            let next_entry = read_u32!();
            let name_offset = read_u32!();
            let name_length = read_u16!();
            let name_flags = read_u16!();
            let parent_frs = read_u32!();

            links.push(LinkInfo {
                next_entry,
                name: IndexNameRef {
                    offset: name_offset,
                    length: name_length,
                    flags: name_flags,
                },
                parent_frs,
            });
        }

        // Read streams (overflow streams)
        let mut streams = Vec::with_capacity(streams_count as usize);
        for _ in 0..streams_count {
            let size_length = read_u64!();
            let size_allocated = read_u64!();
            let next_entry = read_u32!();
            let name_offset = read_u32!();
            let name_length = read_u16!();
            let name_flags = read_u16!();
            let flags = read_u8!();

            streams.push(IndexStreamInfo {
                size: SizeInfo {
                    length: size_length,
                    allocated: size_allocated,
                },
                next_entry,
                name: IndexNameRef {
                    offset: name_offset,
                    length: name_length,
                    flags: name_flags,
                },
                flags,
            });
        }

        // Read children
        let mut children = Vec::with_capacity(children_count as usize);
        for _ in 0..children_count {
            let next_entry = read_u32!();
            let child_frs = read_u32!();
            let name_index = read_u16!();

            children.push(ChildInfo {
                next_entry,
                child_frs,
                name_index,
            });
        }

        let mut index = Self {
            volume,
            records,
            frs_to_idx,
            names,
            links,
            streams,
            children,
            stats: MftStats::new(),
        };

        // Compute stats from loaded data
        index.recompute_stats();

        Ok((index, header))
    }

    /// Saves the index to a file.
    ///
    /// # Errors
    ///
    /// Returns an error if file writing fails.
    pub fn save_to_file(
        &self,
        path: &std::path::Path,
        volume_serial: u64,
        usn_journal_id: u64,
        next_usn: i64,
    ) -> std::io::Result<()> {
        use std::io::Write;

        let data = self.serialize(volume_serial, usn_journal_id, next_usn);
        let mut file = std::fs::File::create(path)?;
        file.write_all(&data)?;
        Ok(())
    }

    /// Loads an index from a file.
    ///
    /// # Errors
    ///
    /// Returns an error if file reading fails or data is corrupted.
    pub fn load_from_file(
        path: &std::path::Path,
    ) -> Result<(Self, IndexHeader), Box<dyn core::error::Error>> {
        let data = std::fs::read(path)?;
        let (index, header) = Self::deserialize(&data)?;
        Ok((index, header))
    }
}
