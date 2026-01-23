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
        }
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

        // Reverse and join
        components.reverse();
        format!("{}:\\{}", self.volume, components.join("\\"))
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
        let capacity = records.len();
        let mut index = Self::with_capacity(volume, capacity);

        for parsed in records {
            if !parsed.in_use {
                continue;
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
}
