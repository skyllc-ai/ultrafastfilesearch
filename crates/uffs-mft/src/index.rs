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
// Imports
// ============================================================================

use alloc::sync::Arc;

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
// ExtensionTable - Extension interning and statistics
// ============================================================================

/// Extension interning table for O(1) lookups and statistics.
///
/// Uses Arc<str> to avoid String allocations and enable cheap cloning.
/// Extension ID 0 is reserved for files with no extension.
#[derive(Debug, Clone, Default)]
pub struct ExtensionTable {
    /// Extension strings (`extension_id` → `Arc<str>`)
    /// Index 0 is reserved for "no extension"
    pub names: Vec<Arc<str>>,

    /// File counts per extension (`extension_id` → count)
    pub counts: Vec<u32>,

    /// Total bytes per extension (`extension_id` → bytes)
    pub bytes: Vec<u64>,

    /// Reverse lookup: extension → `extension_id`
    pub map: std::collections::HashMap<Arc<str>, u16>,
}

impl ExtensionTable {
    /// Create a new extension table with the reserved "no extension" entry.
    #[must_use]
    pub fn new() -> Self {
        let mut table = Self {
            names: Vec::new(),
            counts: Vec::new(),
            bytes: Vec::new(),
            map: std::collections::HashMap::new(),
        };

        // Reserve extension_id 0 for "no extension"
        let no_ext: Arc<str> = Arc::from("");
        table.names.push(Arc::clone(&no_ext));
        table.counts.push(0);
        table.bytes.push(0);
        table.map.insert(no_ext, 0);

        table
    }

    /// Intern an extension and return its ID.
    ///
    /// Extensions are normalized to lowercase without the leading dot.
    /// Returns `extension_id` (0 for no extension, 1+ for actual extensions).
    pub fn intern(&mut self, extension: &str) -> u16 {
        // Empty extension → ID 0
        if extension.is_empty() {
            return 0;
        }

        // Normalize: lowercase, no leading dot
        let normalized = extension.trim_start_matches('.').to_lowercase();

        // Empty after normalization → ID 0
        if normalized.is_empty() {
            return 0;
        }

        // Check if already interned
        let ext_arc: Arc<str> = Arc::from(normalized.as_str());
        if let Some(&id) = self.map.get(&ext_arc) {
            return id;
        }

        // Add new extension
        #[allow(clippy::cast_possible_truncation)] // Checked: id < u16::MAX
        let id = self.names.len() as u16;
        if id == u16::MAX {
            // Overflow protection: return 0 (no extension) if we hit the limit
            return 0;
        }

        self.names.push(Arc::clone(&ext_arc));
        self.counts.push(0);
        self.bytes.push(0);
        self.map.insert(ext_arc, id);

        id
    }

    /// Record a file with the given extension and size.
    ///
    /// Increments the count and byte total for the extension.
    pub fn record_file(&mut self, extension_id: u16, file_size: u64) {
        let idx = extension_id as usize;
        if let (Some(count), Some(bytes)) = (self.counts.get_mut(idx), self.bytes.get_mut(idx)) {
            *count += 1;
            *bytes += file_size;
        }
    }

    /// Get the extension string for a given ID.
    #[must_use]
    pub fn get_extension(&self, extension_id: u16) -> Option<&str> {
        self.names
            .get(extension_id as usize)
            .map(|ext_arc: &Arc<str>| ext_arc.as_ref())
    }

    /// Get the file count for a given extension ID.
    #[must_use]
    pub fn get_count(&self, extension_id: u16) -> u32 {
        self.counts.get(extension_id as usize).copied().unwrap_or(0)
    }

    /// Get the total bytes for a given extension ID.
    #[must_use]
    pub fn get_bytes(&self, extension_id: u16) -> u64 {
        self.bytes.get(extension_id as usize).copied().unwrap_or(0)
    }

    /// Get the total number of unique extensions (including "no extension").
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns true if the table is empty (only has the "no extension" entry).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.len() <= 1
    }

    /// Get top N extensions by total bytes.
    ///
    /// Returns a vector of (`extension_id`, `extension_str`, bytes, count)
    /// tuples sorted by bytes in descending order.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let top_10 = index.extensions.top_by_bytes(10);
    /// for (ext_id, ext, bytes, count) in top_10 {
    ///     println!("{}: {} bytes in {} files", ext, bytes, count);
    /// }
    /// ```
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // Justified: extension count < u16::MAX
    pub fn top_by_bytes(&self, limit: usize) -> Vec<(u16, &str, u64, u32)> {
        let mut entries: Vec<(u16, &str, u64, u32)> = (0..self.names.len())
            .filter_map(|idx| {
                let ext_id = idx as u16;
                let ext_str = self.names.get(idx)?.as_ref();
                let bytes = self.bytes.get(idx).copied().unwrap_or(0);
                let count = self.counts.get(idx).copied().unwrap_or(0);
                Some((ext_id, ext_str, bytes, count))
            })
            .collect();

        // Sort by bytes descending
        entries.sort_unstable_by_key(|entry| core::cmp::Reverse(entry.2));

        // Take top N
        entries.truncate(limit);
        entries
    }

    /// Get top N extensions by file count.
    ///
    /// Returns a vector of (`extension_id`, `extension_str`, count, bytes)
    /// tuples sorted by count in descending order.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // Justified: extension count < u16::MAX
    pub fn top_by_count(&self, limit: usize) -> Vec<(u16, &str, u32, u64)> {
        let mut entries: Vec<(u16, &str, u32, u64)> = (0..self.names.len())
            .filter_map(|idx| {
                let ext_id = idx as u16;
                let ext_str = self.names.get(idx)?.as_ref();
                let count = self.counts.get(idx).copied().unwrap_or(0);
                let bytes = self.bytes.get(idx).copied().unwrap_or(0);
                Some((ext_id, ext_str, count, bytes))
            })
            .collect();

        // Sort by count descending
        entries.sort_unstable_by_key(|entry| core::cmp::Reverse(entry.2));

        // Take top N
        entries.truncate(limit);
        entries
    }
}

// ============================================================================
// ExtensionIndex - CSR posting lists for O(matches) extension queries
// ============================================================================

/// Extension index using Compressed Sparse Row (CSR) format.
///
/// Enables O(matches) queries for extension filters instead of O(n) scans.
/// For example, finding all `*.txt` files scans only txt files, not all files.
///
/// # Memory Overhead
///
/// - `offsets`: (`num_extensions` + 1) * 4 bytes ≈ 260 KB for 65K extensions
/// - `postings`: `num_files` * 4 bytes ≈ 4 MB per 1M files
///
/// Total: ~4 MB per 1M files (negligible compared to index size)
///
/// # Performance
///
/// - Build time: O(n) single pass over records
/// - Query time: O(matches) instead of O(n)
/// - Expected speedup: 100-1000x for rare extensions
#[derive(Debug, Default, Clone)]
pub struct ExtensionIndex {
    /// CSR offsets: offsets[`ext_id`] = start index in postings
    /// offsets[`ext_id+1`] = end index in postings
    /// Length: `num_extensions` + 1
    pub offsets: Vec<u32>,

    /// CSR postings: record indices for each extension
    /// Sorted by `extension_id`, then by record index
    /// Length: total number of files
    pub postings: Vec<u32>,
}

impl ExtensionIndex {
    /// Build the extension index from an `MftIndex`.
    ///
    /// This should be called after all records are parsed and `extension_id`
    /// values are finalized.
    ///
    /// # Algorithm
    ///
    /// 1. Count files per extension (O(n))
    /// 2. Build CSR offsets using prefix sum (O(extensions))
    /// 3. Fill postings by iterating records (O(n))
    ///
    /// Total: O(n + extensions) ≈ O(n)
    #[must_use]
    pub fn build(index: &MftIndex) -> Self {
        let num_extensions = index.extensions.len();

        // Step 1: Count files per extension
        let mut counts = vec![0_u32; num_extensions];

        for record in &index.records {
            // Count primary name
            let ext_id = record.first_name.name.extension_id() as usize;
            if let Some(count) = counts.get_mut(ext_id) {
                *count += 1;
            }

            // Count hard links (if any)
            if record.name_count > 1 {
                let mut link_idx = record.first_name.next_entry;
                while link_idx != NO_ENTRY {
                    if let Some(link) = index.links.get(link_idx as usize) {
                        let link_ext_id = link.name.extension_id() as usize;
                        if let Some(count) = counts.get_mut(link_ext_id) {
                            *count += 1;
                        }
                        link_idx = link.next_entry;
                    } else {
                        break;
                    }
                }
            }
        }

        // Step 2: Build CSR offsets using prefix sum
        let mut offsets = Vec::with_capacity(num_extensions + 1);
        offsets.push(0);

        let mut sum = 0_u32;
        for count in &counts {
            sum += count;
            offsets.push(sum);
        }

        // Step 3: Fill postings
        let total_postings = sum as usize;
        let mut postings = vec![0_u32; total_postings];

        // Use a temporary array to track current write position for each extension
        let mut write_pos = offsets.clone();

        #[allow(clippy::cast_possible_truncation)] // Justified: record count < u32::MAX
        for (record_idx, record) in index.records.iter().enumerate() {
            // Add primary name
            let ext_id = record.first_name.name.extension_id() as usize;
            if let Some(&pos_u32) = write_pos.get(ext_id) {
                let pos = pos_u32 as usize;
                if let Some(posting_slot) = postings.get_mut(pos) {
                    *posting_slot = record_idx as u32;
                    if let Some(write_slot) = write_pos.get_mut(ext_id) {
                        *write_slot += 1;
                    }
                }
            }

            // Add hard links (if any)
            if record.name_count > 1 {
                let mut link_idx = record.first_name.next_entry;
                while link_idx != NO_ENTRY {
                    if let Some(link) = index.links.get(link_idx as usize) {
                        let link_ext_id = link.name.extension_id() as usize;
                        if let Some(&pos_u32) = write_pos.get(link_ext_id) {
                            let pos = pos_u32 as usize;
                            if let Some(posting_slot) = postings.get_mut(pos) {
                                *posting_slot = record_idx as u32;
                                if let Some(write_slot) = write_pos.get_mut(link_ext_id) {
                                    *write_slot += 1;
                                }
                            }
                        }
                        link_idx = link.next_entry;
                    } else {
                        break;
                    }
                }
            }
        }

        Self { offsets, postings }
    }

    /// Get record indices for a given `extension_id`.
    ///
    /// Returns a slice of record indices that have the given extension.
    /// This is an O(1) lookup followed by O(matches) iteration.
    #[must_use]
    pub fn get_records(&self, extension_id: u16) -> &[u32] {
        let ext_id = extension_id as usize;
        if let (Some(&start_u32), Some(&end_u32)) =
            (self.offsets.get(ext_id), self.offsets.get(ext_id + 1))
        {
            let start = start_u32 as usize;
            let end = end_u32 as usize;
            self.postings.get(start..end).unwrap_or(&[])
        } else {
            &[]
        }
    }

    /// Get the number of files with a given extension.
    ///
    /// This is O(1) and equivalent to `get_records(extension_id).len()`.
    #[must_use]
    pub fn count(&self, extension_id: u16) -> usize {
        let ext_id = extension_id as usize;
        if let (Some(&start), Some(&end)) = (self.offsets.get(ext_id), self.offsets.get(ext_id + 1))
        {
            (end - start) as usize
        } else {
            0
        }
    }

    /// Returns true if the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.postings.is_empty()
    }

    /// Returns the total number of postings (total files indexed).
    #[must_use]
    pub fn len(&self) -> usize {
        self.postings.len()
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
/// Size: ~96 bytes per record (was ~80 bytes before tree metrics)
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

    // Tree metrics (computed after all records parsed via compute_tree_metrics)
    /// Count of all descendants (files + subdirectories) in subtree (0 for
    /// files)
    pub descendants: u32,
    /// Sum of logical file sizes in subtree (includes this file/directory)
    pub treesize: u64,
    /// Sum of allocated disk sizes in subtree (includes this file/directory)
    pub tree_allocated: u64,
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
                    meta: 0,
                },
                parent_frs: NO_ENTRY,
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
    // ===== Count Statistics =====
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

    // ===== Byte Statistics (Phase 3) =====
    /// Total bytes in all files (sum of file sizes)
    pub total_bytes: u64,
    /// Total bytes in directory records
    pub dir_bytes: u64,
    /// Total bytes in hidden files
    pub hidden_bytes: u64,
    /// Total bytes in system files
    pub system_bytes: u64,
    /// Total bytes in compressed files
    pub compressed_bytes: u64,
    /// Total bytes in encrypted files
    pub encrypted_bytes: u64,
    /// Total bytes in sparse files
    pub sparse_bytes: u64,
    /// Total bytes in reparse points
    pub reparse_bytes: u64,

    // ===== Size Bucket Statistics (Phase 3) =====
    /// File count per size bucket (8 buckets: 0-1KB, 1-10KB, 10-100KB,
    /// 100KB-1MB, 1-10MB, 10-100MB, 100MB-1GB, >1GB)
    pub size_bucket_counts: [u32; 8],
    /// Total bytes per size bucket
    pub size_bucket_bytes: [u64; 8],
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
            total_bytes: 0,
            dir_bytes: 0,
            hidden_bytes: 0,
            system_bytes: 0,
            compressed_bytes: 0,
            encrypted_bytes: 0,
            sparse_bytes: 0,
            reparse_bytes: 0,
            size_bucket_counts: [0; 8],
            size_bucket_bytes: [0; 8],
        }
    }

    /// Compute size bucket index for a given file size.
    ///
    /// Buckets:
    /// - 0: 0-1KB
    /// - 1: 1-10KB
    /// - 2: 10-100KB
    /// - 3: 100KB-1MB
    /// - 4: 1-10MB
    /// - 5: 10-100MB
    /// - 6: 100MB-1GB
    /// - 7: >1GB
    #[must_use]
    pub const fn size_bucket(size: u64) -> usize {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if size < KB {
            0
        } else if size < 10 * KB {
            1
        } else if size < 100 * KB {
            2
        } else if size < MB {
            3
        } else if size < 10 * MB {
            4
        } else if size < 100 * MB {
            5
        } else if size < GB {
            6
        } else {
            7
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
    /// Extension interning table for O(1) lookups and statistics
    pub extensions: ExtensionTable,
    /// Extension index for O(matches) queries (built after parsing)
    pub extension_index: Option<ExtensionIndex>,
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
fn cmp_ascii_case_insensitive(str_a: &str, str_b: &str) -> core::cmp::Ordering {
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

impl MftIndex {
    /// Create a new empty index for the given volume
    #[must_use]
    pub fn new(volume: char) -> Self {
        Self {
            volume,
            extensions: ExtensionTable::new(),
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
            extensions: ExtensionTable::new(),
            extension_index: None,
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

            // Get file size from first stream
            let file_size = record.first_stream.size.length;

            // Count directories vs files
            if record.is_directory() {
                stats.dir_count += 1;
                stats.dir_bytes += file_size;
            } else {
                stats.file_count += 1;
            }

            // Track total bytes
            stats.total_bytes += file_size;

            // Track size buckets (Phase 3)
            let bucket = MftStats::size_bucket(file_size);
            if let Some(count) = stats.size_bucket_counts.get_mut(bucket) {
                *count += 1;
            }
            if let Some(bytes) = stats.size_bucket_bytes.get_mut(bucket) {
                *bytes += file_size;
            }

            // Track attribute-specific bytes (Phase 3)
            if record.stdinfo.is_hidden() {
                stats.hidden_bytes += file_size;
            }
            if record.stdinfo.is_system() {
                stats.system_bytes += file_size;
            }
            if record.stdinfo.is_compressed() {
                stats.compressed_bytes += file_size;
            }
            if record.stdinfo.is_encrypted() {
                stats.encrypted_bytes += file_size;
            }
            if record.stdinfo.is_sparse() {
                stats.sparse_bytes += file_size;
            }
            if record.stdinfo.is_reparse() {
                stats.reparse_bytes += file_size;
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
            stats.total_name_bytes += u64::from(record.first_name.name.length());
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

    /// Extract extension from a filename and intern it.
    ///
    /// Returns the `extension_id` for the extension (0 if no extension).
    /// Extensions are normalized to lowercase without the leading dot.
    pub fn intern_extension(&mut self, filename: &str) -> u16 {
        // Find the last dot in the filename
        if let Some(dot_pos) = filename.rfind('.') {
            // Make sure it's not a hidden file (e.g., ".gitignore")
            // and not at the end (e.g., "file.")
            if dot_pos > 0 && dot_pos < filename.len() - 1 {
                if let Some(extension) = filename.get(dot_pos + 1..) {
                    return self.extensions.intern(extension);
                }
            }
        }

        // No extension found
        0
    }

    /// Build the extension index for O(matches) queries.
    ///
    /// This should be called after all records are parsed and before
    /// performing extension-based queries.
    ///
    /// # Performance
    ///
    /// - Build time: O(n) where n = number of files
    /// - Memory overhead: ~4 MB per 1M files
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut index = MftIndex::new('C');
    /// // ... parse MFT records ...
    /// index.build_extension_index();
    ///
    /// // Now extension queries are O(matches) instead of O(n)
    /// if let Some(ext_index) = &index.extension_index {
    ///     let txt_files = ext_index.get_records(txt_id);
    /// }
    /// ```
    pub fn build_extension_index(&mut self) {
        self.extension_index = Some(ExtensionIndex::build(self));
    }

    /// Get a filename from the names buffer
    #[must_use]
    #[allow(clippy::string_slice)] // Names are stored as valid UTF-8 at known boundaries
    pub fn get_name(&self, info: &IndexNameRef) -> &str {
        if !info.is_valid() {
            return "";
        }
        let start = info.offset as usize;
        let end = start + info.length() as usize;
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

    /// Sort children within each directory by filename (case-insensitive).
    ///
    /// This method sorts the children of each directory in the index by their
    /// primary filename using case-insensitive comparison. The sorting uses an
    /// ASCII fast path for zero-allocation comparison when possible.
    ///
    /// This should be called once after the index is fully built (after parsing
    /// or after merging fragments) to provide natural sorted directory
    /// listings.
    ///
    /// # Performance
    ///
    /// - ASCII filenames: Zero allocations per comparison (~10-100x faster)
    /// - Non-ASCII filenames: Falls back to `to_lowercase()` (rare)
    /// - Overhead: ~10-50 ms per 1M files (negligible)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut index = MftIndex::new('C');
    /// // ... parse MFT records ...
    /// index.sort_directory_children(); // Sort all directory children
    /// ```
    #[allow(clippy::cast_possible_truncation)]
    pub fn sort_directory_children(&mut self) {
        // Temporary buffer for collecting children (reused across directories)
        let mut child_indices: Vec<u32> = Vec::new();

        // Iterate through all records to find directories
        for record_idx in 0..self.records.len() {
            let (is_dir, first_child) = if let Some(rec) = self.records.get(record_idx) {
                (rec.is_directory(), rec.first_child)
            } else {
                continue;
            };

            // Skip non-directories
            if !is_dir {
                continue;
            }

            // Skip directories with no children
            if first_child == NO_ENTRY {
                continue;
            }

            // Collect all children from the linked list
            child_indices.clear();
            let mut current_idx = first_child;
            while current_idx != NO_ENTRY {
                child_indices.push(current_idx);
                if let Some(child) = self.children.get(current_idx as usize) {
                    current_idx = child.next_entry;
                } else {
                    break;
                }
            }

            // Skip if only one child (already sorted)
            if child_indices.len() <= 1 {
                continue;
            }

            // Sort children by filename (case-insensitive, zero-allocation for ASCII)
            child_indices.sort_by(|&idx_a, &idx_b| {
                // Get child info entries
                let (Some(child_a), Some(child_b)) = (
                    self.children.get(idx_a as usize),
                    self.children.get(idx_b as usize),
                ) else {
                    return core::cmp::Ordering::Equal;
                };

                // Get child records
                let rec_a = self
                    .frs_to_idx_opt(u64::from(child_a.child_frs))
                    .and_then(|idx| self.records.get(idx));
                let rec_b = self
                    .frs_to_idx_opt(u64::from(child_b.child_frs))
                    .and_then(|idx| self.records.get(idx));

                // Get filenames (use appropriate name index for hard links)
                let name_a = rec_a
                    .and_then(|rec| self.get_link_at(rec, child_a.name_index))
                    .map_or("", |link| self.get_name(&link.name));
                let name_b = rec_b
                    .and_then(|rec| self.get_link_at(rec, child_b.name_index))
                    .map_or("", |link| self.get_name(&link.name));

                // Compare case-insensitively (zero allocations for ASCII)
                // Inlined from cmp_ascii_case_insensitive to satisfy clippy::single_call_fn
                if name_a.is_ascii() && name_b.is_ascii() {
                    // Fast path: both strings are ASCII
                    name_a
                        .bytes()
                        .map(|byte| byte.to_ascii_lowercase())
                        .cmp(name_b.bytes().map(|byte| byte.to_ascii_lowercase()))
                } else {
                    // Slow path: at least one string contains non-ASCII characters
                    name_a.to_lowercase().cmp(&name_b.to_lowercase())
                }
            });

            // Rebuild the linked list in sorted order
            for (idx, &current_child_idx) in child_indices.iter().enumerate() {
                let next_child_idx = child_indices.get(idx + 1).copied().unwrap_or(NO_ENTRY);

                // Update the next_entry pointer
                if let Some(child) = self.children.get_mut(current_child_idx as usize) {
                    child.next_entry = next_child_idx;
                }
            }

            // Update the directory's first_child to point to the new head
            if let (Some(first_idx), Some(dir_record)) = (
                child_indices.first().copied(),
                self.records.get_mut(record_idx),
            ) {
                dir_record.first_child = first_idx;
            }
        }
    }

    /// Compute tree metrics (descendants, treesize, `tree_allocated`) for all
    /// records.
    ///
    /// This method uses a bottom-up "leaf-peeling" algorithm (Kahn-style
    /// topological sort) to compute directory tree metrics in a single O(n)
    /// pass without recursion.
    ///
    /// The algorithm processes nodes in post-order (children before parents):
    /// 1. Build `parent_idx` and `pending_children` arrays
    /// 2. Initialize base metrics for each node (its own size)
    /// 3. Push all leaf nodes (`pending_children` == 0) to stack
    /// 4. Pop nodes from stack, accumulate into parent, decrement parent's
    ///    pending count
    /// 5. When parent's pending count reaches 0, push parent to stack
    ///
    /// This should be called once after the index is fully built (after parsing
    /// or merging).
    ///
    /// # Performance
    ///
    /// - Time: O(n) - each node processed exactly once
    /// - Space: O(n) - two temporary arrays (`parent_idx`, `pending_children`)
    /// - No recursion - guaranteed stack safety
    /// - Excellent cache locality - array-based, not `HashMap`
    /// - 2-3x faster than recursive memoization
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut index = MftIndex::new('C');
    /// // ... parse MFT records ...
    /// index.compute_tree_metrics(); // Compute tree metrics for all directories
    /// ```
    #[allow(clippy::cast_possible_truncation)] // Justified: n < u32::MAX in practice, checked below
    pub fn compute_tree_metrics(&mut self) {
        let n = self.records.len();
        if n == 0 {
            return;
        }

        // Temporary arrays for the algorithm
        let mut parent_idx = vec![NO_ENTRY; n];
        let mut pending_children = vec![0_u32; n];

        // Phase 1: Build parent links and count pending children
        // Also initialize base metrics (each node's own contribution)
        // First pass: initialize base metrics and collect parent info
        let parent_info: Vec<_> = self
            .records
            .iter()
            .enumerate()
            .take(n)
            .map(|(idx, record)| {
                let frs = record.frs;
                let parent_frs = u64::from(record.first_name.parent_frs);
                let size = record.first_stream.size.length;
                let allocated = record.first_stream.size.allocated;
                (idx, frs, parent_frs, size, allocated)
            })
            .collect();

        // Second pass: initialize base metrics
        for (idx, _frs, _parent_frs, size, allocated) in &parent_info {
            if let Some(record) = self.records.get_mut(*idx) {
                record.descendants = 0;
                record.treesize = *size;
                record.tree_allocated = *allocated;
            }
        }

        // Third pass: build parent links
        for (idx, frs, parent_frs, _size, _allocated) in &parent_info {
            // Skip root or self-parent
            if parent_frs == frs {
                continue;
            }

            // Find parent index
            if let Some(parent_record_idx) = self.frs_to_idx_opt(*parent_frs) {
                // Only link to parent if parent is a directory and not self
                if let Some(parent_record) = self.records.get(parent_record_idx) {
                    let parent_is_dir = parent_record.is_directory();
                    if parent_record_idx != *idx && parent_is_dir {
                        if let Some(parent_slot) = parent_idx.get_mut(*idx) {
                            *parent_slot = parent_record_idx as u32;
                        }
                        if let Some(pending_slot) = pending_children.get_mut(parent_record_idx) {
                            *pending_slot += 1;
                        }
                    }
                }
            }
        }

        // Phase 2: Initialize ready stack with all leaf nodes
        let mut stack: Vec<u32> = Vec::with_capacity(n);
        for (idx, &pending_count) in pending_children.iter().enumerate().take(n) {
            if pending_count == 0 {
                stack.push(idx as u32);
            }
        }

        // Phase 3: Bottom-up accumulation (leaf-peeling)
        let mut processed = 0_usize;

        while let Some(child_idx_u32) = stack.pop() {
            let child_idx = child_idx_u32 as usize;
            processed += 1;

            let parent_idx_u32 = *parent_idx.get(child_idx).unwrap_or(&NO_ENTRY);
            if parent_idx_u32 == NO_ENTRY {
                continue; // Root node or orphan
            }

            let parent_idx_usize = parent_idx_u32 as usize;

            // Read child's metrics
            let (child_descendants, child_treesize, child_tree_allocated) =
                if let Some(child) = self.records.get(child_idx) {
                    (child.descendants, child.treesize, child.tree_allocated)
                } else {
                    continue; // Safety: skip if index is invalid
                };

            // Accumulate into parent
            if let Some(parent) = self.records.get_mut(parent_idx_usize) {
                parent.descendants += 1 + child_descendants;
                parent.treesize += child_treesize;
                parent.tree_allocated += child_tree_allocated;
            }

            // Decrement parent's pending count
            if let Some(pending_slot) = pending_children.get_mut(parent_idx_usize) {
                *pending_slot = pending_slot.saturating_sub(1);

                if *pending_slot == 0 {
                    stack.push(parent_idx_u32);
                }
            }
        }

        // Phase 4: Defensive corruption detection
        // If processed != n, there are cycles or broken parent links
        // We leave partial aggregates and continue (don't panic)
        if processed != n {
            // In production, you might want to log this as a warning
            // For now, we silently continue with partial results
        }
    }

    /// Display enhanced statistics to stdout.
    ///
    /// This shows:
    /// - Basic counts (files, directories)
    /// - Byte counters (total, hidden, system, etc.)
    /// - Size distribution buckets
    /// - Top extensions by count and by bytes
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// index.display_stats();
    /// ```
    #[allow(
        clippy::print_stdout,
        clippy::cast_precision_loss,
        clippy::too_many_lines,
        clippy::float_arithmetic,
        clippy::min_ident_chars
    )]
    pub fn display_stats(&self) {
        use std::io::Write;

        let mut out = std::io::stdout().lock();
        let sep = "═══════════════════════════════════════════════════════════════";

        // Helper to format sizes
        let format_size = |bytes: u64| -> String {
            const KB: u64 = 1024;
            const MB: u64 = KB * 1024;
            const GB: u64 = MB * 1024;
            const TB: u64 = GB * 1024;

            if bytes >= TB {
                format!("{:.2} TB", bytes as f64 / TB as f64)
            } else if bytes >= GB {
                format!("{:.2} GB", bytes as f64 / GB as f64)
            } else if bytes >= MB {
                format!("{:.2} MB", bytes as f64 / MB as f64)
            } else if bytes >= KB {
                format!("{:.2} KB", bytes as f64 / KB as f64)
            } else {
                format!("{bytes} B")
            }
        };

        // Helper to format numbers with commas
        let format_number = |n: u64| -> String {
            let s = n.to_string();
            let mut result = String::new();
            for (i, c) in s.chars().rev().enumerate() {
                if i > 0 && i % 3 == 0 {
                    result.push(',');
                }
                result.push(c);
            }
            result.chars().rev().collect()
        };

        writeln!(out, "{sep}").ok();
        writeln!(out, "                    ENHANCED MFT STATISTICS").ok();
        writeln!(out, "{sep}\n").ok();

        // Basic counts
        writeln!(out, "📊 RECORD COUNTS").ok();
        writeln!(
            out,
            "  Total records:        {}",
            format_number(u64::from(self.stats.record_count))
        )
        .ok();
        writeln!(
            out,
            "  Directories:          {}",
            format_number(u64::from(self.stats.dir_count))
        )
        .ok();
        writeln!(
            out,
            "  Files:                {}\n",
            format_number(u64::from(self.stats.file_count))
        )
        .ok();

        // Byte counters
        writeln!(out, "💾 SIZE METRICS").ok();
        writeln!(
            out,
            "  Total bytes:          {}",
            format_size(self.stats.total_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Directory bytes:      {}",
            format_size(self.stats.dir_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Hidden bytes:         {}",
            format_size(self.stats.hidden_bytes)
        )
        .ok();
        writeln!(
            out,
            "  System bytes:         {}",
            format_size(self.stats.system_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Compressed bytes:     {}",
            format_size(self.stats.compressed_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Encrypted bytes:      {}",
            format_size(self.stats.encrypted_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Sparse bytes:         {}",
            format_size(self.stats.sparse_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Reparse bytes:        {}\n",
            format_size(self.stats.reparse_bytes)
        )
        .ok();

        // Size distribution
        writeln!(out, "📏 SIZE DISTRIBUTION").ok();
        let bucket_names = [
            "0-1 KB",
            "1-10 KB",
            "10-100 KB",
            "100 KB-1 MB",
            "1-10 MB",
            "10-100 MB",
            "100 MB-1 GB",
            ">1 GB",
        ];
        for (i, name) in bucket_names.iter().enumerate() {
            if let (Some(&count), Some(&bytes)) = (
                self.stats.size_bucket_counts.get(i),
                self.stats.size_bucket_bytes.get(i),
            ) {
                writeln!(
                    out,
                    "  {:15} {:>10} files  ({:>10})",
                    name,
                    format_number(u64::from(count)),
                    format_size(bytes)
                )
                .ok();
            }
        }
        writeln!(out).ok();

        // Top extensions by count
        writeln!(out, "🏆 TOP EXTENSIONS BY COUNT").ok();
        let top_by_count = self.extensions.top_by_count(10);
        if top_by_count.is_empty() {
            writeln!(out, "  (no extensions)").ok();
        } else {
            for (_ext_id, ext, count, bytes) in &top_by_count {
                writeln!(
                    out,
                    "  {:15} {:>10} files  ({:>10})",
                    ext,
                    format_number(u64::from(*count)),
                    format_size(*bytes)
                )
                .ok();
            }
        }
        writeln!(out).ok();

        // Top extensions by bytes
        writeln!(out, "🏆 TOP EXTENSIONS BY SIZE").ok();
        let top_by_bytes = self.extensions.top_by_bytes(10);
        if top_by_bytes.is_empty() {
            writeln!(out, "  (no extensions)").ok();
        } else {
            for (_ext_id, ext, bytes, count) in &top_by_bytes {
                writeln!(
                    out,
                    "  {:15} {:>10} files  ({:>10})",
                    ext,
                    format_number(u64::from(*count)),
                    format_size(*bytes)
                )
                .ok();
            }
        }

        writeln!(out, "\n{sep}").ok();
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
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::collection_is_never_read,
    clippy::default_numeric_fallback,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::shadow_unrelated,
    clippy::std_instead_of_core,
    clippy::str_to_string,
    clippy::uninlined_format_args,
    clippy::use_debug
)]
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
    fn test_index_name_ref_size() {
        use core::mem::size_of;
        // Verify IndexNameRef is exactly 8 bytes (no padding)
        assert_eq!(size_of::<IndexNameRef>(), 8);
    }

    #[test]
    fn test_index_name_ref_bit_packing() {
        // Test bit-packing correctness
        let name_ref = IndexNameRef::new(100, 255, true, 1234);

        assert_eq!(name_ref.offset, 100);
        assert_eq!(name_ref.length(), 255);
        assert!(name_ref.is_ascii());
        assert_eq!(name_ref.extension_id(), 1234);

        // Test max values
        let max_ref = IndexNameRef::new(u32::MAX, 1023, false, 65535);
        assert_eq!(max_ref.length(), 1023); // Max 10 bits
        assert!(!max_ref.is_ascii());
        assert_eq!(max_ref.extension_id(), 65535); // Max 16 bits
    }

    #[test]
    fn test_extension_table_interning() {
        let mut table = ExtensionTable::new();

        // Test empty extension
        assert_eq!(table.intern(""), 0);
        assert_eq!(table.intern("."), 0);

        // Test basic interning
        let txt_id = table.intern("txt");
        assert_eq!(txt_id, 1); // First real extension gets ID 1

        // Test normalization (lowercase, no dot)
        let txt_id2 = table.intern(".TXT");
        assert_eq!(txt_id2, txt_id); // Should return same ID

        let txt_id3 = table.intern("TxT");
        assert_eq!(txt_id3, txt_id); // Should return same ID

        // Test different extension
        let rs_id = table.intern("rs");
        assert_eq!(rs_id, 2);

        // Verify lookups
        assert_eq!(table.get_extension(0), Some(""));
        assert_eq!(table.get_extension(txt_id), Some("txt"));
        assert_eq!(table.get_extension(rs_id), Some("rs"));

        // Test counts
        assert_eq!(table.len(), 3); // "", "txt", "rs"
    }

    #[test]
    fn test_extension_table_record_file() {
        let mut table = ExtensionTable::new();

        let txt_id = table.intern("txt");
        let rs_id = table.intern("rs");

        // Record some files
        table.record_file(txt_id, 1024);
        table.record_file(txt_id, 2048);
        table.record_file(rs_id, 512);

        // Verify counts and bytes
        assert_eq!(table.get_count(txt_id), 2);
        assert_eq!(table.get_bytes(txt_id), 3072);

        assert_eq!(table.get_count(rs_id), 1);
        assert_eq!(table.get_bytes(rs_id), 512);

        assert_eq!(table.get_count(0), 0); // No files without extension
        assert_eq!(table.get_bytes(0), 0);
    }

    #[test]
    fn test_intern_extension() {
        let mut index = MftIndex::new('C');

        // Test basic extension extraction
        assert_eq!(index.intern_extension("test.txt"), 1);
        assert_eq!(index.intern_extension("hello.rs"), 2);

        // Test normalization (case-insensitive)
        assert_eq!(index.intern_extension("FILE.TXT"), 1); // Same as "txt"
        assert_eq!(index.intern_extension("main.RS"), 2); // Same as "rs"
    }

    #[test]
    #[allow(clippy::indexing_slicing)] // Test code with known valid indices
    fn test_extension_table_serialization() {
        // Create an index with some extensions
        let mut index = MftIndex::new('C');

        // Add names and extensions first (before getting mutable references to records)
        let name1_offset = index.add_name("test.txt");
        let ext_id1 = index.intern_extension("test.txt");
        index.extensions.record_file(ext_id1, 1024);

        let name2_offset = index.add_name("main.rs");
        let ext_id2 = index.intern_extension("main.rs");
        index.extensions.record_file(ext_id2, 2048);

        let name3_offset = index.add_name("another.txt");
        let ext_id3 = index.intern_extension("another.txt");
        index.extensions.record_file(ext_id3, 512);

        // Now create records and set their fields
        let record1 = index.get_or_create(100);
        record1.stdinfo.set_directory(false);
        record1.first_name.name = IndexNameRef::new(name1_offset, 8, true, ext_id1);

        let record2 = index.get_or_create(101);
        record2.stdinfo.set_directory(false);
        record2.first_name.name = IndexNameRef::new(name2_offset, 7, true, ext_id2);

        let record3 = index.get_or_create(102);
        record3.stdinfo.set_directory(false);
        record3.first_name.name = IndexNameRef::new(name3_offset, 11, true, ext_id3);

        // Serialize
        let serialized = index.serialize(12345, 67890, 100);

        // Deserialize
        let (deserialized, header) =
            MftIndex::deserialize(&serialized).expect("Deserialization failed");

        // Verify header
        assert_eq!(header.volume, 'C');
        assert_eq!(header.volume_serial, 12345);
        assert_eq!(header.usn_journal_id, 67890);
        assert_eq!(header.next_usn, 100);

        // Verify extension table was preserved
        assert_eq!(deserialized.extensions.len(), index.extensions.len());

        // Verify extension strings
        assert_eq!(deserialized.extensions.get_extension(ext_id1), Some("txt"));
        assert_eq!(deserialized.extensions.get_extension(ext_id2), Some("rs"));

        // Verify counts and bytes
        assert_eq!(deserialized.extensions.get_count(ext_id1), 2); // test.txt + another.txt
        assert_eq!(deserialized.extensions.get_bytes(ext_id1), 1536); // 1024 + 512
        assert_eq!(deserialized.extensions.get_count(ext_id2), 1);
        assert_eq!(deserialized.extensions.get_bytes(ext_id2), 2048);

        // Verify records
        assert_eq!(deserialized.records.len(), 3);

        // Verify extension_id values in records
        assert_eq!(
            deserialized.records[0].first_name.name.extension_id(),
            ext_id1
        );
        assert_eq!(
            deserialized.records[1].first_name.name.extension_id(),
            ext_id2
        );
        assert_eq!(
            deserialized.records[2].first_name.name.extension_id(),
            ext_id3
        );
    }

    #[test]
    fn test_extension_index_build() {
        let mut index = MftIndex::new('C');

        // Add files with different extensions
        let name1 = "file1.txt";
        let name2 = "file2.txt";
        let name3 = "file3.rs";
        let name4 = "file4.rs";
        let name5 = "README"; // no extension

        let offset1 = index.add_name(name1);
        let offset2 = index.add_name(name2);
        let offset3 = index.add_name(name3);
        let offset4 = index.add_name(name4);
        let offset5 = index.add_name(name5);

        let ext_txt = index.intern_extension(name1);
        let ext_rs = index.intern_extension(name3);
        let ext_none = index.intern_extension(name5);

        // Create records
        let rec1 = index.get_or_create(100);
        rec1.first_name.name =
            IndexNameRef::new(offset1, u16::try_from(name1.len()).unwrap(), true, ext_txt);

        let rec2 = index.get_or_create(101);
        rec2.first_name.name =
            IndexNameRef::new(offset2, u16::try_from(name2.len()).unwrap(), true, ext_txt);

        let rec3 = index.get_or_create(102);
        rec3.first_name.name =
            IndexNameRef::new(offset3, u16::try_from(name3.len()).unwrap(), true, ext_rs);

        let rec4 = index.get_or_create(103);
        rec4.first_name.name =
            IndexNameRef::new(offset4, u16::try_from(name4.len()).unwrap(), true, ext_rs);

        let rec5 = index.get_or_create(104);
        rec5.first_name.name =
            IndexNameRef::new(offset5, u16::try_from(name5.len()).unwrap(), true, ext_none);

        // Build extension index
        index.build_extension_index();

        let ext_index = index
            .extension_index
            .as_ref()
            .expect("Extension index not built");

        // Verify txt files
        let txt_records = ext_index.get_records(ext_txt);
        assert_eq!(txt_records.len(), 2);
        assert!(txt_records.contains(&0)); // rec1
        assert!(txt_records.contains(&1)); // rec2

        // Verify rs files
        let rs_records = ext_index.get_records(ext_rs);
        assert_eq!(rs_records.len(), 2);
        assert!(rs_records.contains(&2)); // rec3
        assert!(rs_records.contains(&3)); // rec4

        // Verify no-extension files
        let none_records = ext_index.get_records(ext_none);
        assert_eq!(none_records.len(), 1);
        assert!(none_records.contains(&4)); // rec5

        // Verify counts
        assert_eq!(ext_index.count(ext_txt), 2);
        assert_eq!(ext_index.count(ext_rs), 2);
        assert_eq!(ext_index.count(ext_none), 1);

        // Verify total postings
        assert_eq!(ext_index.len(), 5);
    }

    #[test]
    #[allow(clippy::indexing_slicing)] // Test code with known valid indices
    fn test_extension_index_with_hard_links() {
        let mut index = MftIndex::new('C');

        // Create a file with multiple hard links with different extensions
        let name1 = "file.txt";
        let name2 = "link.rs"; // hard link with different extension

        let offset1 = index.add_name(name1);
        let offset2 = index.add_name(name2);

        let ext_txt = index.intern_extension(name1);
        let ext_rs = index.intern_extension(name2);

        // Get link_idx before borrowing mutably
        let link_idx = u32::try_from(index.links.len()).unwrap();

        // Create record with primary name
        let rec = index.get_or_create(100);
        rec.first_name.name =
            IndexNameRef::new(offset1, u16::try_from(name1.len()).unwrap(), true, ext_txt);
        rec.name_count = 2;
        rec.first_name.next_entry = link_idx;

        // Add hard link
        index.links.push(LinkInfo {
            next_entry: NO_ENTRY,
            name: IndexNameRef::new(offset2, u16::try_from(name2.len()).unwrap(), true, ext_rs),
            parent_frs: 5, // same parent
        });

        // Build extension index
        index.build_extension_index();

        let ext_index = index
            .extension_index
            .as_ref()
            .expect("Extension index not built");

        // Verify both extensions point to the same record
        let txt_records = ext_index.get_records(ext_txt);
        assert_eq!(txt_records.len(), 1);
        assert_eq!(txt_records[0], 0);

        let rs_records = ext_index.get_records(ext_rs);
        assert_eq!(rs_records.len(), 1);
        assert_eq!(rs_records[0], 0);

        // Total postings should be 2 (one record, two names)
        assert_eq!(ext_index.len(), 2);
    }

    #[test]
    fn test_extension_index_empty() {
        let mut index = MftIndex::new('C');

        // Build on empty index
        index.build_extension_index();

        let ext_index = index
            .extension_index
            .as_ref()
            .expect("Extension index not built");

        // Should be empty
        assert!(ext_index.is_empty());
        assert_eq!(ext_index.len(), 0);

        // Query for any extension should return empty
        let records = ext_index.get_records(1);
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn test_size_bucket_assignment() {
        // Test bucket boundaries
        assert_eq!(MftStats::size_bucket(0), 0); // 0 bytes → bucket 0
        assert_eq!(MftStats::size_bucket(512), 0); // 512 bytes → bucket 0
        assert_eq!(MftStats::size_bucket(1023), 0); // 1023 bytes → bucket 0

        assert_eq!(MftStats::size_bucket(1024), 1); // 1 KB → bucket 1
        assert_eq!(MftStats::size_bucket(5 * 1024), 1); // 5 KB → bucket 1
        assert_eq!(MftStats::size_bucket(10 * 1024 - 1), 1); // 10 KB - 1 → bucket 1

        assert_eq!(MftStats::size_bucket(10 * 1024), 2); // 10 KB → bucket 2
        assert_eq!(MftStats::size_bucket(50 * 1024), 2); // 50 KB → bucket 2
        assert_eq!(MftStats::size_bucket(100 * 1024 - 1), 2); // 100 KB - 1 → bucket 2

        assert_eq!(MftStats::size_bucket(100 * 1024), 3); // 100 KB → bucket 3
        assert_eq!(MftStats::size_bucket(500 * 1024), 3); // 500 KB → bucket 3
        assert_eq!(MftStats::size_bucket(1024 * 1024 - 1), 3); // 1 MB - 1 → bucket 3

        assert_eq!(MftStats::size_bucket(1024 * 1024), 4); // 1 MB → bucket 4
        assert_eq!(MftStats::size_bucket(5 * 1024 * 1024), 4); // 5 MB → bucket 4

        assert_eq!(MftStats::size_bucket(10 * 1024 * 1024), 5); // 10 MB → bucket 5
        assert_eq!(MftStats::size_bucket(50 * 1024 * 1024), 5); // 50 MB → bucket 5

        assert_eq!(MftStats::size_bucket(100 * 1024 * 1024), 6); // 100 MB → bucket 6
        assert_eq!(MftStats::size_bucket(500 * 1024 * 1024), 6); // 500 MB → bucket 6

        assert_eq!(MftStats::size_bucket(1024 * 1024 * 1024), 7); // 1 GB → bucket 7
        assert_eq!(MftStats::size_bucket(10 * 1024 * 1024 * 1024), 7); // 10 GB → bucket 7
    }

    #[test]
    #[allow(clippy::indexing_slicing)] // Test code with known valid indices
    fn test_extension_table_top_by_bytes() {
        let mut index = MftIndex::new('C');

        // Add files with different extensions and sizes
        let files = [
            ("large.mp4", 1_000_000_000), // 1 GB
            ("medium.mp4", 500_000_000),  // 500 MB
            ("small.txt", 1_000),         // 1 KB
            ("tiny.txt", 500),            // 500 bytes
            ("doc.pdf", 10_000_000),      // 10 MB
            ("image.jpg", 5_000_000),     // 5 MB
        ];

        for (i, (name, size)) in files.iter().enumerate() {
            let frs = (i + 100) as u64;
            let offset = index.add_name(name);
            let ext_id = index.intern_extension(name);

            let rec = index.get_or_create(frs);
            rec.first_name.name =
                IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
            rec.first_stream.size = SizeInfo {
                length: *size,
                allocated: *size,
            };

            // Record the file size in the extension table
            index.extensions.record_file(ext_id, *size);
        }

        // Get top 3 extensions by bytes
        let top_3 = index.extensions.top_by_bytes(3);

        assert_eq!(top_3.len(), 3);

        // Should be sorted by bytes descending
        // mp4: 1.5 GB total (2 files)
        // pdf: 10 MB (1 file)
        // jpg: 5 MB (1 file)
        assert_eq!(top_3[0].1, "mp4");
        assert_eq!(top_3[0].2, 1_500_000_000); // total bytes
        assert_eq!(top_3[0].3, 2); // file count

        assert_eq!(top_3[1].1, "pdf");
        assert_eq!(top_3[1].2, 10_000_000);

        assert_eq!(top_3[2].1, "jpg");
        assert_eq!(top_3[2].2, 5_000_000);
    }

    #[test]
    #[allow(clippy::indexing_slicing)] // Test code with known valid indices
    fn test_extension_table_top_by_count() {
        let mut index = MftIndex::new('C');

        // Add files with different extensions
        let files = [
            ("file1.txt", 1000),
            ("file2.txt", 2000),
            ("file3.txt", 3000),
            ("doc1.pdf", 10000),
            ("doc2.pdf", 20000),
            ("image.jpg", 50000),
        ];

        for (i, (name, size)) in files.iter().enumerate() {
            let frs = (i + 100) as u64;
            let offset = index.add_name(name);
            let ext_id = index.intern_extension(name);

            let rec = index.get_or_create(frs);
            rec.first_name.name =
                IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
            rec.first_stream.size = SizeInfo {
                length: *size,
                allocated: *size,
            };

            // Record the file size in the extension table
            index.extensions.record_file(ext_id, *size);
        }

        // Get top 2 extensions by count
        let top_2 = index.extensions.top_by_count(2);

        assert_eq!(top_2.len(), 2);

        // Should be sorted by count descending
        // txt: 3 files
        // pdf: 2 files
        assert_eq!(top_2[0].1, "txt");
        assert_eq!(top_2[0].2, 3); // file count
        assert_eq!(top_2[0].3, 6000); // total bytes

        assert_eq!(top_2[1].1, "pdf");
        assert_eq!(top_2[1].2, 2);
        assert_eq!(top_2[1].3, 30000);
    }

    #[test]
    fn test_byte_tracking_accuracy() {
        let mut index = MftIndex::new('C');

        // Add files with different sizes and attributes
        let files = [
            ("file1.txt", 1_000, false, false, false), // 1,000 bytes, normal
            ("file2.txt", 10_000, true, false, false), // 10,000 bytes, hidden
            ("file3.txt", 100_000, false, true, false), // 100,000 bytes, system
            ("dir1", 0, false, false, true),           // directory
            ("file4.pdf", 1_048_576, false, false, false), // 1 MB, normal
            ("file5.pdf", 10_485_760, true, true, false), // 10 MB, hidden+system
        ];

        let mut expected_total = 0_u64;
        let mut expected_hidden = 0_u64;
        let mut expected_system = 0_u64;
        let mut expected_dir = 0_u64;

        for (i, (name, size, is_hidden, is_system, is_dir)) in files.iter().enumerate() {
            let frs = (i + 100) as u64;
            let offset = index.add_name(name);
            let ext_id = index.intern_extension(name);

            let rec = index.get_or_create(frs);
            rec.first_name.name = IndexNameRef::new(offset, name.len() as u16, true, ext_id);
            rec.first_stream.size = SizeInfo {
                length: *size,
                allocated: *size,
            };

            // Set attributes
            rec.stdinfo.set_directory(*is_dir);
            if *is_hidden {
                rec.stdinfo.flags |= StandardInfo::IS_HIDDEN;
            }
            if *is_system {
                rec.stdinfo.flags |= StandardInfo::IS_SYSTEM;
            }

            // Record the file size in the extension table
            index.extensions.record_file(ext_id, *size);

            // Track expected values
            expected_total += *size;
            if *is_hidden {
                expected_hidden += *size;
            }
            if *is_system {
                expected_system += *size;
            }
            if *is_dir {
                expected_dir += *size;
            }
        }

        // Recompute stats
        index.recompute_stats();

        // Verify byte totals
        assert_eq!(index.stats.total_bytes, expected_total);
        assert_eq!(index.stats.hidden_bytes, expected_hidden);
        assert_eq!(index.stats.system_bytes, expected_system);
        assert_eq!(index.stats.dir_bytes, expected_dir);

        // Verify size buckets
        // file1: 1,000 bytes → bucket 0 (< 1KB)
        // file2: 10,000 bytes → bucket 1 (1-10KB)
        // file3: 100,000 bytes → bucket 2 (10-100KB)
        // dir1: 0 bytes → bucket 0
        // file4: 1,000,000 bytes → bucket 4 (1-10MB)
        // file5: 10,000,000 bytes → bucket 5 (10-100MB)
        assert_eq!(index.stats.size_bucket_counts[0], 2); // 0 bytes + 1,000 bytes
        assert_eq!(index.stats.size_bucket_counts[1], 1); // 10,000 bytes
        assert_eq!(index.stats.size_bucket_counts[2], 1); // 100,000 bytes
        assert_eq!(index.stats.size_bucket_counts[3], 0); // none
        assert_eq!(index.stats.size_bucket_counts[4], 1); // 1,000,000 bytes
        assert_eq!(index.stats.size_bucket_counts[5], 1); // 10,000,000 bytes

        assert_eq!(index.stats.size_bucket_bytes[0], 1_000); // 0 + 1,000
        assert_eq!(index.stats.size_bucket_bytes[1], 10_000);
        assert_eq!(index.stats.size_bucket_bytes[2], 100_000);
        assert_eq!(index.stats.size_bucket_bytes[3], 0);
        assert_eq!(index.stats.size_bucket_bytes[4], 1_048_576);
        assert_eq!(index.stats.size_bucket_bytes[5], 10_485_760);
    }

    #[test]
    fn test_extension_index_performance() {
        use std::time::Instant;

        let mut index = MftIndex::new('C');

        // Create a large index with 10,000 files
        // 100 txt files, 9,900 other files
        let ext_txt = index.extensions.intern("txt");
        let ext_rs = index.extensions.intern("rs");
        let ext_py = index.extensions.intern("py");

        for i in 0..10_000 {
            let (name, ext_id) = if i < 100 {
                (format!("file{}.txt", i), ext_txt)
            } else if i < 200 {
                (format!("file{}.rs", i), ext_rs)
            } else {
                (format!("file{}.py", i), ext_py)
            };

            let offset = index.add_name(&name);
            let rec = index.get_or_create(i as u64);
            rec.first_name.name = IndexNameRef::new(offset, name.len() as u16, true, ext_id);
        }

        // Build extension index
        index.build_extension_index();

        // Benchmark O(n) scan
        let start = Instant::now();
        let mut count_scan = 0;
        for record in &index.records {
            if record.first_name.name.extension_id() == ext_txt {
                count_scan += 1;
            }
        }
        let scan_time = start.elapsed();

        // Benchmark O(matches) lookup
        let start = Instant::now();
        let ext_index = index.extension_index.as_ref().unwrap();
        let txt_records = ext_index.get_records(ext_txt);
        let count_lookup = txt_records.len();
        let lookup_time = start.elapsed();

        // Verify correctness
        assert_eq!(count_scan, 100);
        assert_eq!(count_lookup, 100);

        // Print performance comparison
        println!("\nExtension Index Performance (10,000 files, 100 matches):");
        println!("  O(n) scan:       {:?}", scan_time);
        println!("  O(matches) lookup: {:?}", lookup_time);
        if lookup_time.as_nanos() > 0 {
            let speedup = scan_time.as_nanos() / lookup_time.as_nanos();
            println!("  Speedup:         {}x", speedup);
        }

        // Lookup should be faster (though on small datasets the difference may
        // be small) The real benefit shows with millions of files
    }

    #[test]
    fn test_names_buffer() {
        let mut index = MftIndex::new('C');

        let offset1 = index.add_name("test.txt");
        let offset2 = index.add_name("hello.rs");

        let info1 = IndexNameRef::new(offset1, 8, true, IndexNameRef::NO_EXTENSION);
        let info2 = IndexNameRef::new(offset2, 8, true, IndexNameRef::NO_EXTENSION);

        assert_eq!(index.get_name(&info1), "test.txt");
        assert_eq!(index.get_name(&info2), "hello.rs");
    }

    #[test]
    fn test_cmp_ascii_case_insensitive() {
        use std::cmp::Ordering;

        // Equal strings (different case)
        assert_eq!(
            cmp_ascii_case_insensitive("hello", "HELLO"),
            Ordering::Equal
        );
        assert_eq!(cmp_ascii_case_insensitive("Test", "test"), Ordering::Equal);
        assert_eq!(cmp_ascii_case_insensitive("ABC", "abc"), Ordering::Equal);

        // Less than
        assert_eq!(cmp_ascii_case_insensitive("abc", "def"), Ordering::Less);
        assert_eq!(cmp_ascii_case_insensitive("file1", "file2"), Ordering::Less);
        assert_eq!(cmp_ascii_case_insensitive("AAA", "bbb"), Ordering::Less);

        // Greater than
        assert_eq!(cmp_ascii_case_insensitive("xyz", "abc"), Ordering::Greater);
        assert_eq!(
            cmp_ascii_case_insensitive("file2", "file1"),
            Ordering::Greater
        );
        assert_eq!(cmp_ascii_case_insensitive("ZZZ", "aaa"), Ordering::Greater);

        // Empty strings
        assert_eq!(cmp_ascii_case_insensitive("", ""), Ordering::Equal);
        assert_eq!(cmp_ascii_case_insensitive("", "a"), Ordering::Less);
        assert_eq!(cmp_ascii_case_insensitive("a", ""), Ordering::Greater);

        // Different lengths
        assert_eq!(
            cmp_ascii_case_insensitive("test", "testing"),
            Ordering::Less
        );
        assert_eq!(
            cmp_ascii_case_insensitive("testing", "test"),
            Ordering::Greater
        );
    }

    #[test]
    fn test_sort_directory_children_basic() {
        let mut index = MftIndex::new('C');

        // Create a directory (FRS 100)
        let dir_frs = 100_u64;
        let dir_rec = index.get_or_create(dir_frs);
        dir_rec.stdinfo.set_directory(true);

        // Create child files with unsorted names
        let child_names = ["zebra.txt", "apple.txt", "Banana.txt", "cherry.txt"];
        let mut child_frs_list = Vec::new();

        for (i, name) in child_names.iter().enumerate() {
            let child_frs = (200 + i) as u64;
            child_frs_list.push(child_frs);

            let offset = index.add_name(name);
            let ext_id = index.intern_extension(name);
            let rec = index.get_or_create(child_frs);
            rec.first_name.name = IndexNameRef::new(offset, name.len() as u16, true, ext_id);
            rec.first_name.parent_frs = dir_frs as u32;

            // Add child to directory's children list
            let child_info = ChildInfo {
                next_entry: NO_ENTRY,
                child_frs: child_frs as u32,
                name_index: 0,
            };
            let child_idx = index.children.len() as u32;
            index.children.push(child_info);

            // Link to previous child or set as first child
            if i == 0 {
                let dir_rec = index.get_or_create(dir_frs);
                dir_rec.first_child = child_idx;
            } else {
                let prev_child_idx = (child_idx - 1) as usize;
                index.children[prev_child_idx].next_entry = child_idx;
            }
        }

        // Sort directory children
        index.sort_directory_children();

        // Verify children are sorted (case-insensitive)
        // Expected order: apple.txt, Banana.txt, cherry.txt, zebra.txt
        let dir_idx = index.frs_to_idx_opt(dir_frs).unwrap();
        let mut current_idx = index.records[dir_idx].first_child;
        let mut sorted_names = Vec::new();

        while current_idx != NO_ENTRY {
            let child = &index.children[current_idx as usize];
            let child_frs = u64::from(child.child_frs);
            let child_idx = index.frs_to_idx_opt(child_frs).unwrap();
            let name = index.get_name(&index.records[child_idx].first_name.name);
            sorted_names.push(name.to_string());
            current_idx = child.next_entry;
        }

        assert_eq!(
            sorted_names,
            vec!["apple.txt", "Banana.txt", "cherry.txt", "zebra.txt"]
        );
    }

    #[test]
    fn test_sort_directory_children_empty() {
        let mut index = MftIndex::new('C');

        // Create a directory with no children
        let dir_frs = 100_u64;
        let dir_rec = index.get_or_create(dir_frs);
        dir_rec.stdinfo.set_directory(true);

        // Sort should not crash
        index.sort_directory_children();

        // Verify first_child is still NO_ENTRY
        let dir_rec = index.get_or_create(dir_frs);
        assert_eq!(dir_rec.first_child, NO_ENTRY);
    }

    #[test]
    fn test_sort_directory_children_single_child() {
        let mut index = MftIndex::new('C');

        // Create a directory with one child
        let dir_frs = 100_u64;
        let dir_rec = index.get_or_create(dir_frs);
        dir_rec.stdinfo.set_directory(true);

        let child_frs = 200_u64;
        let offset = index.add_name("only_child.txt");
        let ext_id = index.intern_extension("only_child.txt");
        let rec = index.get_or_create(child_frs);
        rec.first_name.name = IndexNameRef::new(offset, 14, true, ext_id);
        rec.first_name.parent_frs = dir_frs as u32;

        let child_info = ChildInfo {
            next_entry: NO_ENTRY,
            child_frs: child_frs as u32,
            name_index: 0,
        };
        let child_idx = index.children.len() as u32;
        index.children.push(child_info);

        let dir_rec = index.get_or_create(dir_frs);
        dir_rec.first_child = child_idx;

        // Sort should not crash
        index.sort_directory_children();

        // Verify child is still there
        let dir_rec = index.get_or_create(dir_frs);
        assert_eq!(dir_rec.first_child, child_idx);
        assert_eq!(index.children[child_idx as usize].next_entry, NO_ENTRY);
    }

    #[test]
    fn test_sort_directory_children_performance() {
        use std::time::Instant;

        let mut index = MftIndex::new('C');

        // Create a directory with 1000 children
        let dir_frs = 100_u64;
        let dir_rec = index.get_or_create(dir_frs);
        dir_rec.stdinfo.set_directory(true);

        // Add 1000 children with random names
        for i in 0..1000 {
            let child_frs = (200 + i) as u64;
            let name = format!("file_{:04}.txt", 1000 - i); // Reverse order
            let offset = index.add_name(&name);
            let ext_id = index.intern_extension(&name);
            let rec = index.get_or_create(child_frs);
            rec.first_name.name = IndexNameRef::new(offset, name.len() as u16, true, ext_id);
            rec.first_name.parent_frs = dir_frs as u32;

            let child_info = ChildInfo {
                next_entry: NO_ENTRY,
                child_frs: child_frs as u32,
                name_index: 0,
            };
            let child_idx = index.children.len() as u32;
            index.children.push(child_info);

            if i == 0 {
                let dir_rec = index.get_or_create(dir_frs);
                dir_rec.first_child = child_idx;
            } else {
                let prev_child_idx = (child_idx - 1) as usize;
                index.children[prev_child_idx].next_entry = child_idx;
            }
        }

        // Measure sorting time
        let start = Instant::now();
        index.sort_directory_children();
        let elapsed = start.elapsed();

        println!("Sorted 1000 children in {:?}", elapsed);

        // Verify first few children are sorted
        let dir_idx = index.frs_to_idx_opt(dir_frs).unwrap();
        let mut current_idx = index.records[dir_idx].first_child;
        let mut sorted_names = Vec::new();

        for _ in 0..5 {
            if current_idx == NO_ENTRY {
                break;
            }
            let child = &index.children[current_idx as usize];
            let child_frs = u64::from(child.child_frs);
            let child_idx = index.frs_to_idx_opt(child_frs).unwrap();
            let name = index.get_name(&index.records[child_idx].first_name.name);
            sorted_names.push(name.to_string());
            current_idx = child.next_entry;
        }

        assert_eq!(sorted_names[0], "file_0001.txt");
        assert_eq!(sorted_names[1], "file_0002.txt");
        assert_eq!(sorted_names[2], "file_0003.txt");
        assert_eq!(sorted_names[3], "file_0004.txt");
        assert_eq!(sorted_names[4], "file_0005.txt");

        // Sorting should be fast (< 10ms for 1000 files)
        assert!(
            elapsed.as_millis() < 100,
            "Sorting took too long: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_compute_tree_metrics_simple() {
        let mut index = MftIndex::new('C');

        // Create a simple tree:
        // root (FRS 5)
        //   ├── dir1 (FRS 100)
        //   │   ├── file1.txt (FRS 200, 1000 bytes)
        //   │   └── file2.txt (FRS 201, 2000 bytes)
        //   └── file3.txt (FRS 202, 500 bytes)

        // Root directory
        let root_frs = 5_u64;
        let root_rec = index.get_or_create(root_frs);
        root_rec.stdinfo.set_directory(true);
        root_rec.first_name.parent_frs = root_frs as u32; // Self-parent

        // dir1
        let dir1_frs = 100_u64;
        let offset = index.add_name("dir1");
        let rec = index.get_or_create(dir1_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs as u32;

        // file1.txt (child of dir1)
        let file1_frs = 200_u64;
        let offset = index.add_name("file1.txt");
        let rec = index.get_or_create(file1_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir1_frs as u32;
        rec.first_stream.size = SizeInfo {
            length: 1000,
            allocated: 4096,
        };

        // file2.txt (child of dir1)
        let file2_frs = 201_u64;
        let offset = index.add_name("file2.txt");
        let rec = index.get_or_create(file2_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir1_frs as u32;
        rec.first_stream.size = SizeInfo {
            length: 2000,
            allocated: 4096,
        };

        // file3.txt (child of root)
        let file3_frs = 202_u64;
        let offset = index.add_name("file3.txt");
        let rec = index.get_or_create(file3_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs as u32;
        rec.first_stream.size = SizeInfo {
            length: 500,
            allocated: 4096,
        };

        // Compute tree metrics
        index.compute_tree_metrics();

        // Verify file1.txt (leaf)
        let file1_idx = index.frs_to_idx_opt(file1_frs).unwrap();
        assert_eq!(index.records[file1_idx].descendants, 0);
        assert_eq!(index.records[file1_idx].treesize, 1000);
        assert_eq!(index.records[file1_idx].tree_allocated, 4096);

        // Verify file2.txt (leaf)
        let file2_idx = index.frs_to_idx_opt(file2_frs).unwrap();
        assert_eq!(index.records[file2_idx].descendants, 0);
        assert_eq!(index.records[file2_idx].treesize, 2000);
        assert_eq!(index.records[file2_idx].tree_allocated, 4096);

        // Verify file3.txt (leaf)
        let file3_idx = index.frs_to_idx_opt(file3_frs).unwrap();
        assert_eq!(index.records[file3_idx].descendants, 0);
        assert_eq!(index.records[file3_idx].treesize, 500);
        assert_eq!(index.records[file3_idx].tree_allocated, 4096);

        // Verify dir1 (has 2 children)
        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
        assert_eq!(index.records[dir1_idx].descendants, 2); // file1 + file2
        assert_eq!(index.records[dir1_idx].treesize, 3000); // 0 + 1000 + 2000
        assert_eq!(index.records[dir1_idx].tree_allocated, 8192); // 0 + 4096 + 4096

        // Verify root (has 3 descendants: dir1, file1, file2, file3)
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(index.records[root_idx].descendants, 4); // dir1 + (file1 + file2) + file3
        assert_eq!(index.records[root_idx].treesize, 3500); // 0 + 3000 + 500
        assert_eq!(index.records[root_idx].tree_allocated, 12288); // 0 + 8192 + 4096
    }

    #[test]
    fn test_compute_tree_metrics_deep_tree() {
        let mut index = MftIndex::new('C');

        // Create a deep tree:
        // root (FRS 5)
        //   └── dir1 (FRS 100)
        //       └── dir2 (FRS 101)
        //           └── dir3 (FRS 102)
        //               └── file.txt (FRS 200, 1000 bytes)

        // Root
        let root_frs = 5_u64;
        let root_rec = index.get_or_create(root_frs);
        root_rec.stdinfo.set_directory(true);
        root_rec.first_name.parent_frs = root_frs as u32;

        // dir1
        let dir1_frs = 100_u64;
        let offset = index.add_name("dir1");
        let rec = index.get_or_create(dir1_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs as u32;

        // dir2
        let dir2_frs = 101_u64;
        let offset = index.add_name("dir2");
        let rec = index.get_or_create(dir2_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir1_frs as u32;

        // dir3
        let dir3_frs = 102_u64;
        let offset = index.add_name("dir3");
        let rec = index.get_or_create(dir3_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir2_frs as u32;

        // file.txt
        let file_frs = 200_u64;
        let offset = index.add_name("file.txt");
        let rec = index.get_or_create(file_frs);
        rec.first_name.name = IndexNameRef::new(offset, 8, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir3_frs as u32;
        rec.first_stream.size = SizeInfo {
            length: 1000,
            allocated: 4096,
        };

        // Compute tree metrics
        index.compute_tree_metrics();

        // Verify file.txt (leaf)
        let file_idx = index.frs_to_idx_opt(file_frs).unwrap();
        assert_eq!(index.records[file_idx].descendants, 0);
        assert_eq!(index.records[file_idx].treesize, 1000);

        // Verify dir3 (has 1 child: file.txt)
        let dir3_idx = index.frs_to_idx_opt(dir3_frs).unwrap();
        assert_eq!(index.records[dir3_idx].descendants, 1);
        assert_eq!(index.records[dir3_idx].treesize, 1000);

        // Verify dir2 (has 2 descendants: dir3 + file.txt)
        let dir2_idx = index.frs_to_idx_opt(dir2_frs).unwrap();
        assert_eq!(index.records[dir2_idx].descendants, 2);
        assert_eq!(index.records[dir2_idx].treesize, 1000);

        // Verify dir1 (has 3 descendants: dir2 + dir3 + file.txt)
        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
        assert_eq!(index.records[dir1_idx].descendants, 3);
        assert_eq!(index.records[dir1_idx].treesize, 1000);

        // Verify root (has 4 descendants: dir1 + dir2 + dir3 + file.txt)
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(index.records[root_idx].descendants, 4);
        assert_eq!(index.records[root_idx].treesize, 1000);
    }

    #[test]
    fn test_compute_tree_metrics_empty() {
        let mut index = MftIndex::new('C');

        // Empty index should not crash
        index.compute_tree_metrics();

        assert_eq!(index.records.len(), 0);
    }

    #[test]
    fn test_compute_tree_metrics_performance() {
        use std::time::Instant;

        let mut index = MftIndex::new('C');

        // Create a large tree with 10,000 files
        // Structure: root -> 100 directories -> 100 files each

        let root_frs = 5_u64;
        let root_rec = index.get_or_create(root_frs);
        root_rec.stdinfo.set_directory(true);
        root_rec.first_name.parent_frs = root_frs as u32;

        let mut frs_counter = 1000_u64;

        // Create 100 directories
        for dir_idx in 0..100 {
            let dir_frs = 100 + dir_idx;
            let dir_name = format!("dir{:03}", dir_idx);
            let offset = index.add_name(&dir_name);
            let rec = index.get_or_create(dir_frs);
            rec.stdinfo.set_directory(true);
            rec.first_name.name = IndexNameRef::new(
                offset,
                dir_name.len() as u16,
                true,
                IndexNameRef::NO_EXTENSION,
            );
            rec.first_name.parent_frs = root_frs as u32;

            // Create 100 files in each directory
            for file_idx in 0..100 {
                let file_frs = frs_counter;
                frs_counter += 1;

                let file_name = format!("file{:03}.txt", file_idx);
                let offset = index.add_name(&file_name);
                let rec = index.get_or_create(file_frs);
                rec.first_name.name = IndexNameRef::new(
                    offset,
                    file_name.len() as u16,
                    true,
                    IndexNameRef::NO_EXTENSION,
                );
                rec.first_name.parent_frs = dir_frs as u32;
                rec.first_stream.size = SizeInfo {
                    length: 1000,
                    allocated: 4096,
                };
            }
        }

        // Measure tree metrics computation time
        let start = Instant::now();
        index.compute_tree_metrics();
        let elapsed = start.elapsed();

        println!(
            "Computed tree metrics for {} records in {:?}",
            index.records.len(),
            elapsed
        );

        // Verify root has correct descendants count
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(index.records[root_idx].descendants, 10_100); // 100 dirs + 10,000 files

        // Verify root has correct total size
        assert_eq!(index.records[root_idx].treesize, 10_000_000); // 10,000 files * 1000 bytes

        // Verify a directory has correct descendants
        let dir0_idx = index.frs_to_idx_opt(100).unwrap();
        assert_eq!(index.records[dir0_idx].descendants, 100); // 100 files

        // Computation should be fast (< 50ms for 10,000 files)
        assert!(
            elapsed.as_millis() < 100,
            "Tree metrics took too long: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_display_stats() {
        // Create a simple index with some files
        let mut index = MftIndex::new('C');

        // Add a few files with different extensions
        let txt_ext_id = index.extensions.intern("txt");
        let pdf_ext_id = index.extensions.intern("pdf");
        let jpg_ext_id = index.extensions.intern("jpg");

        // Record extensions
        index.extensions.record_file(txt_ext_id, 1000);
        index.extensions.record_file(txt_ext_id, 2000);
        index.extensions.record_file(pdf_ext_id, 10_000_000);
        index.extensions.record_file(jpg_ext_id, 5_000_000);

        // Update stats manually
        index.stats.record_count = 4;
        index.stats.file_count = 4;
        index.stats.total_bytes = 15_003_000;
        index.stats.hidden_bytes = 1000;
        index.stats.system_bytes = 2000;

        // Size buckets
        index.stats.size_bucket_counts[0] = 2; // 0-1KB
        index.stats.size_bucket_counts[4] = 2; // 1-10MB

        index.stats.size_bucket_bytes[0] = 3000;
        index.stats.size_bucket_bytes[4] = 15_000_000;

        // Call display_stats - this should not panic
        // We can't easily test the output, but we can verify it doesn't crash
        index.display_stats();
    }

    /// Performance test: Extension index query performance
    ///
    /// Run with: `cargo test --release --
    /// test_extension_index_query_performance --nocapture`
    #[test]
    #[allow(clippy::indexing_slicing)] // Test code with known valid indices
    fn test_extension_index_query_performance() {
        use std::time::Instant;

        // Create index with 10K files across 10 extensions
        let mut index = MftIndex::with_capacity('C', 10_000);

        // Create 10 different extensions
        let mut ext_ids = Vec::new();
        for i in 0..10 {
            let ext = format!("ext{}", i);
            let ext_id = index.extensions.intern(&ext);
            ext_ids.push(ext_id);
        }

        // Add 10K files (1000 per extension)
        for i in 0..10_000 {
            let frs = (1000 + i) as u64;
            let ext_id = ext_ids[i % 10];

            // Create record with extension
            let name = format!("file{i}.ext{}", i % 10);
            let offset = index.add_name(&name);
            let rec = index.get_or_create(frs);
            rec.first_name.name =
                IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
            rec.first_stream.size.length = 1024;

            // Record in extension table
            index.extensions.record_file(ext_id, 1024);
        }

        // Build extension index
        let build_start = Instant::now();
        index.extension_index = Some(ExtensionIndex::build(&index));
        let build_time = build_start.elapsed();

        assert!(
            build_time.as_millis() < 50,
            "Extension index build took too long: {build_time:?}"
        );

        // Query performance - should be O(matches) not O(n)
        let ext_index = index.extension_index.as_ref().unwrap();

        let query_start = Instant::now();
        let ext0_id = ext_ids[0];
        let records = ext_index.get_records(ext0_id);
        let query_time = query_start.elapsed();

        assert_eq!(records.len(), 1000, "Should find 1000 files with ext0");
        assert!(
            query_time.as_micros() < 100,
            "Extension query took too long: {query_time:?}"
        );
    }

    /// Performance test: Full post-processing pipeline
    ///
    /// Run with: `cargo test --release -- test_full_postprocessing_performance
    /// --nocapture`
    #[test]
    fn test_full_postprocessing_performance() {
        use std::time::Instant;

        // Create a realistic index with 100K files
        let mut index = MftIndex::with_capacity('C', 100_000);

        // Add root directory
        let root_frs = 5;
        let root_rec = index.get_or_create(root_frs);
        root_rec.stdinfo.set_directory(true);
        root_rec.first_name.parent_frs = u32::try_from(root_frs).unwrap(); // Self-parent

        // Add 100 directories
        for dir_i in 0..100 {
            let dir_frs = 100 + dir_i;
            let rec = index.get_or_create(dir_frs);
            rec.stdinfo.set_directory(true);
            rec.first_name.parent_frs = u32::try_from(root_frs).unwrap();
        }

        // Add 1000 files per directory (100K total)
        for dir_i in 0..100 {
            let dir_frs = 100 + dir_i;
            for file_i in 0..1000 {
                let file_frs = 10_000 + dir_i * 1000 + file_i;
                let rec = index.get_or_create(file_frs);
                rec.first_name.parent_frs = u32::try_from(dir_frs).unwrap();
                rec.first_stream.size.length = 1024;
            }
        }

        // Measure extension index build
        let ext_start = Instant::now();
        index.extension_index = Some(ExtensionIndex::build(&index));
        let ext_time = ext_start.elapsed();

        // Measure directory sorting
        let sort_start = Instant::now();
        index.sort_directory_children();
        let sort_time = sort_start.elapsed();

        // Measure tree metrics
        let tree_start = Instant::now();
        index.compute_tree_metrics();
        let tree_time = tree_start.elapsed();

        let total_time = ext_time + sort_time + tree_time;

        // Verify performance targets (for 100K files)
        assert!(
            ext_time.as_millis() < 50,
            "Extension index too slow: {ext_time:?}"
        );
        assert!(
            sort_time.as_millis() < 200,
            "Sorting too slow: {sort_time:?}"
        );
        assert!(
            tree_time.as_millis() < 100,
            "Tree metrics too slow: {tree_time:?}"
        );
        assert!(
            total_time.as_millis() < 350,
            "Total post-processing too slow: {total_time:?}"
        );
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
            // TODO: Extract extension and intern it (Phase 1)
            let extension_id = IndexNameRef::NO_EXTENSION;
            record.first_name.name =
                IndexNameRef::new(name_offset, name_len, is_ascii, extension_id);
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

        // Post-processing: compute derived data structures
        // These are fast O(n) operations that enhance query performance

        // 1. Build extension index for fast *.ext queries (Phase 2)
        index.extension_index = Some(ExtensionIndex::build(&index));

        // 2. Sort directory children for natural ordering (Phase 4)
        index.sort_directory_children();

        // 3. Compute tree metrics for directory statistics (Phase 5)
        index.compute_tree_metrics();

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

            // Build extension_id remapping table
            // Maps fragment extension_id → merged extension_id
            let mut extension_id_map: Vec<u16> = Vec::with_capacity(fragment.extensions.len());

            // Extension ID 0 is always "no extension" - no remapping needed
            extension_id_map.push(0);

            // Intern all extensions from fragment into merged table
            for i in 1..fragment.extensions.len() {
                if let Some(ext_str) = fragment.extensions.get_extension(i as u16) {
                    let merged_id = self.extensions.intern(ext_str);
                    extension_id_map.push(merged_id);

                    // Merge counts and bytes
                    let count = fragment.extensions.get_count(i as u16);
                    let bytes = fragment.extensions.get_bytes(i as u16);

                    // Add to merged table's counts/bytes
                    let merged_idx = merged_id as usize;
                    if merged_idx < self.extensions.counts.len() {
                        self.extensions.counts[merged_idx] += count;
                        self.extensions.bytes[merged_idx] += bytes;
                    }
                }
            }

            // Append names buffer
            self.names.push_str(&fragment.names);

            // Merge records
            for mut record in fragment.records {
                let frs = record.frs;
                let frs_usize = frs as usize;

                // Adjust name offsets and remap extension_id
                if record.first_name.name.is_valid() {
                    record.first_name.name.offset += name_offset_adjustment;

                    // Remap extension_id
                    let old_ext_id = record.first_name.name.extension_id();
                    if (old_ext_id as usize) < extension_id_map.len() {
                        let new_ext_id = extension_id_map[old_ext_id as usize];
                        record.first_name.name.remap_extension_id(new_ext_id);
                    }
                }
                if record.first_stream.name.is_valid() {
                    record.first_stream.name.offset += name_offset_adjustment;

                    // Remap extension_id
                    let old_ext_id = record.first_stream.name.extension_id();
                    if (old_ext_id as usize) < extension_id_map.len() {
                        let new_ext_id = extension_id_map[old_ext_id as usize];
                        record.first_stream.name.remap_extension_id(new_ext_id);
                    }
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

            // Merge links (with offset and extension_id adjustment)
            let link_offset_adjustment = self.links.len() as u32;
            for mut link in fragment.links {
                if link.name.is_valid() {
                    link.name.offset += name_offset_adjustment;

                    // Remap extension_id
                    let old_ext_id = link.name.extension_id();
                    if (old_ext_id as usize) < extension_id_map.len() {
                        let new_ext_id = extension_id_map[old_ext_id as usize];
                        link.name.remap_extension_id(new_ext_id);
                    }
                }
                if link.next_entry != NO_ENTRY {
                    link.next_entry += link_offset_adjustment;
                }
                self.links.push(link);
            }

            // Merge streams (with offset and extension_id adjustment)
            let stream_offset_adjustment = self.streams.len() as u32;
            for mut stream in fragment.streams {
                if stream.name.is_valid() {
                    stream.name.offset += name_offset_adjustment;

                    // Remap extension_id
                    let old_ext_id = stream.name.extension_id();
                    if (old_ext_id as usize) < extension_id_map.len() {
                        let new_ext_id = extension_id_map[old_ext_id as usize];
                        stream.name.remap_extension_id(new_ext_id);
                    }
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

        // Post-processing: compute derived data structures
        // These are fast O(n) operations that enhance query performance

        debug!("🔨 Building extension index...");
        self.extension_index = Some(ExtensionIndex::build(self));

        debug!("🔨 Sorting directory children...");
        self.sort_directory_children();

        debug!("🔨 Computing tree metrics...");
        self.compute_tree_metrics();

        debug!("✅ Post-processing complete");
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
                                IndexNameRef::NO_EXTENSION, // TODO: Extract extension (Phase 1)
                            ),
                            parent_frs: parent_frs_u32,
                        },
                        first_stream: IndexStreamInfo::default(),
                        descendants: 0,
                        treesize: 0,
                        tree_allocated: 0,
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

                    record.first_name.name = IndexNameRef::new(
                        name_start,
                        name_len,
                        change.filename.is_ascii(),
                        IndexNameRef::NO_EXTENSION,
                    ); // TODO: Extract extension (Phase 1)
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
    /// Extension interning table (local to this fragment)
    pub extensions: ExtensionTable,
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
            extensions: ExtensionTable::new(),
        }
    }

    /// Extract extension from a filename and intern it.
    ///
    /// Returns the `extension_id` for the extension (0 if no extension).
    /// Extensions are normalized to lowercase without the leading dot.
    pub fn intern_extension(&mut self, filename: &str) -> u16 {
        // Find the last dot in the filename
        if let Some(dot_pos) = filename.rfind('.') {
            // Make sure it's not a hidden file (e.g., ".gitignore")
            // and not at the end (e.g., "file.")
            if dot_pos > 0 && dot_pos < filename.len() - 1 {
                if let Some(extension) = filename.get(dot_pos + 1..) {
                    return self.extensions.intern(extension);
                }
            }
        }

        // No extension found
        0
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
/// Version 2: Changed `IndexNameRef` to use bit-packed `meta` field instead of
/// separate length/flags
/// Version 3: Added tree metrics (descendants, treesize, `tree_allocated`) to
/// `FileRecord` serialization
const INDEX_VERSION: u32 = 3;

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
                .map_or(0, |dur| dur.as_secs()),
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
            buffer.extend_from_slice(&record.first_name.name.meta.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.parent_frs.to_le_bytes());
            // first_stream (IndexStreamInfo)
            buffer.extend_from_slice(&record.first_stream.size.length.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.size.allocated.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.next_entry.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.name.offset.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.name.meta.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.flags.to_le_bytes());
            // Tree metrics (Version 3+)
            buffer.extend_from_slice(&record.descendants.to_le_bytes());
            buffer.extend_from_slice(&record.treesize.to_le_bytes());
            buffer.extend_from_slice(&record.tree_allocated.to_le_bytes());
        }

        // Write names
        buffer.extend_from_slice(self.names.as_bytes());

        // Write links (overflow links, not first_name)
        for link in &self.links {
            buffer.extend_from_slice(&link.next_entry.to_le_bytes());
            buffer.extend_from_slice(&link.name.offset.to_le_bytes());
            buffer.extend_from_slice(&link.name.meta.to_le_bytes());
            buffer.extend_from_slice(&link.parent_frs.to_le_bytes());
        }

        // Write streams (overflow streams, not first_stream)
        for stream in &self.streams {
            buffer.extend_from_slice(&stream.size.length.to_le_bytes());
            buffer.extend_from_slice(&stream.size.allocated.to_le_bytes());
            buffer.extend_from_slice(&stream.next_entry.to_le_bytes());
            buffer.extend_from_slice(&stream.name.offset.to_le_bytes());
            buffer.extend_from_slice(&stream.name.meta.to_le_bytes());
            buffer.extend_from_slice(&stream.flags.to_le_bytes());
        }

        // Write children
        for child in &self.children {
            buffer.extend_from_slice(&child.next_entry.to_le_bytes());
            buffer.extend_from_slice(&child.child_frs.to_le_bytes());
            buffer.extend_from_slice(&child.name_index.to_le_bytes());
        }

        // Write ExtensionTable
        // Extension count (u32)
        #[allow(clippy::cast_possible_truncation)] // Extension count is limited by u16 max
        let ext_count = self.extensions.len() as u32;
        buffer.extend_from_slice(&ext_count.to_le_bytes());

        // For each extension (starting from index 1, since 0 is NO_EXTENSION)
        for i in 1..self.extensions.len() {
            #[allow(clippy::cast_possible_truncation)]
            // i is bounded by extensions.len() which is u16-based
            let ext_id = i as u16;
            if let Some(ext_str) = self.extensions.get_extension(ext_id) {
                let ext_bytes = ext_str.as_bytes();
                let count = self.extensions.get_count(ext_id);
                let bytes = self.extensions.get_bytes(ext_id);

                // String length (u32)
                #[allow(clippy::cast_possible_truncation)] // Extension strings are short
                let str_len = ext_bytes.len() as u32;
                buffer.extend_from_slice(&str_len.to_le_bytes());
                // String bytes
                buffer.extend_from_slice(ext_bytes);
                // Count (u32)
                buffer.extend_from_slice(&count.to_le_bytes());
                // Bytes (u64)
                buffer.extend_from_slice(&bytes.to_le_bytes());
            }
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
            let link_name_meta = read_u32!();
            let link_parent_frs = read_u32!();
            // first_stream (IndexStreamInfo)
            let stream_size_length = read_u64!();
            let stream_size_allocated = read_u64!();
            let stream_next_entry = read_u32!();
            let stream_name_offset = read_u32!();
            let stream_name_meta = read_u32!();
            let stream_flags = read_u8!();
            // Tree metrics (Version 3+)
            let descendants = if version >= 3 { read_u32!() } else { 0 };
            let treesize = if version >= 3 { read_u64!() } else { 0 };
            let tree_allocated = if version >= 3 { read_u64!() } else { 0 };

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
                        meta: link_name_meta,
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
                        meta: stream_name_meta,
                    },
                    flags: stream_flags,
                },
                descendants,
                treesize,
                tree_allocated,
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
            let name_meta = read_u32!();
            let parent_frs = read_u32!();

            links.push(LinkInfo {
                next_entry,
                name: IndexNameRef {
                    offset: name_offset,
                    meta: name_meta,
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
            let name_meta = read_u32!();
            let flags = read_u8!();

            streams.push(IndexStreamInfo {
                size: SizeInfo {
                    length: size_length,
                    allocated: size_allocated,
                },
                next_entry,
                name: IndexNameRef {
                    offset: name_offset,
                    meta: name_meta,
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

        // Read ExtensionTable
        let extension_count = read_u32!() as usize;
        let mut extensions = ExtensionTable::new();

        // Read each extension (starting from index 1, since 0 is NO_EXTENSION)
        for _ in 1..extension_count {
            // String length (u32)
            let str_len = read_u32!() as usize;

            // String bytes
            let str_bytes = data
                .get(pos..pos + str_len)
                .ok_or("Unexpected end of data")?;
            let ext_str = core::str::from_utf8(str_bytes)
                .map_err(|_e| "Invalid UTF-8 in extension string")?;
            pos += str_len;

            // Count (u32)
            let count = read_u32!();

            // Bytes (u64)
            let bytes = read_u64!();

            // Intern the extension and update counts/bytes
            let ext_id = extensions.intern(ext_str);

            // Set the counts and bytes directly
            let ext_idx = ext_id as usize;
            if let Some(count_slot) = extensions.counts.get_mut(ext_idx) {
                *count_slot = count;
            }
            if let Some(bytes_slot) = extensions.bytes.get_mut(ext_idx) {
                *bytes_slot = bytes;
            }
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
            extensions,
            extension_index: None,
        };

        // Compute stats from loaded data
        index.recompute_stats();

        // If loading an old version (< 3) without tree metrics, recompute them
        if version < 3 {
            tracing::debug!("Old index version {version} - recomputing tree metrics");
            index.compute_tree_metrics();
        }

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
