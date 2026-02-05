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
// Tree Algorithm Selection
// ============================================================================

/// Selects which tree metrics algorithm to use.
///
/// This allows switching between the current Rust implementation and
/// the new C++ port for testing and comparison.
///
/// # Environment Variable
///
/// Set `UFFS_TREE_ALGO` to control the default:
/// - `current` (default): Use the current leaf-peeling algorithm
/// - `cpp_port`: Use the C++ port algorithm (100% faithful port)
///
/// # Example
///
/// ```bash
/// # Use current algorithm (default)
/// UFFS_TREE_ALGO=current uffs index
///
/// # Use C++ port algorithm
/// UFFS_TREE_ALGO=cpp_port uffs index
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TreeAlgorithm {
    /// Legacy Rust leaf-peeling algorithm (deprecated)
    Current,
    /// C++ port algorithm - 100% faithful port of C++ tree algorithm (default)
    ///
    /// This is the default because it has full parity with C++ including:
    /// - Two-channel model (Channel A for propagation, Channel B for printed)
    /// - Exact delta formula for hardlink distribution
    /// - Stream count clamping to max(1) for empty directories
    /// - LIVE scan self-healing via `rebuild_children_from_names()`
    #[default]
    CppPort,
}

impl TreeAlgorithm {
    /// Parse from environment variable `UFFS_TREE_ALGO`.
    ///
    /// Returns `CppPort` (default) unless explicitly set to "current" or
    /// "legacy".
    #[must_use]
    pub fn from_env() -> Self {
        std::env::var("UFFS_TREE_ALGO").map_or(Self::CppPort, |val| {
            match val.to_lowercase().as_str() {
                "current" | "legacy" | "leaf_peeling" => Self::Current,
                _ => Self::CppPort, // Default to CppPort (has all parity fixes)
            }
        })
    }

    /// Returns the algorithm name for display.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Current => "current (leaf-peeling, legacy)",
            Self::CppPort => "cpp_port (C++ faithful port, default)",
        }
    }
}

impl core::str::FromStr for TreeAlgorithm {
    type Err = core::convert::Infallible;

    /// Parse from a string value (for CLI arguments).
    ///
    /// Returns `Current` if unrecognized (never fails).
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value.to_lowercase().as_str() {
            "cpp_port" | "cpp" | "port" => Self::CppPort,
            _ => Self::Current,
        })
    }
}

// ============================================================================
// Parse Algorithm Selection
// ============================================================================

/// Selects which MFT record parsing algorithm to use.
///
/// This allows switching between the current Rust implementation and
/// the new C++ port for testing and comparison.
///
/// # Environment Variable
///
/// Set `UFFS_PARSE_ALGO` to control the default:
/// - `current` (default): Use the current Rust parsing algorithm
/// - `cpp_port`: Use the C++ port algorithm (100% faithful port)
///
/// # Example
///
/// ```bash
/// # Use current algorithm (default)
/// UFFS_PARSE_ALGO=current uffs index
///
/// # Use C++ port algorithm
/// UFFS_PARSE_ALGO=cpp_port uffs index
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseAlgorithm {
    /// Current Rust parsing algorithm (default)
    Current,
    /// C++ port algorithm - 100% faithful port of C++ parsing algorithm
    CppPort,
}

impl Default for ParseAlgorithm {
    /// Default checks the `UFFS_PARSE_ALGO` environment variable.
    fn default() -> Self {
        Self::from_env()
    }
}

impl ParseAlgorithm {
    /// Parse from environment variable `UFFS_PARSE_ALGO`.
    ///
    /// Returns `Current` if not set or unrecognized.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("UFFS_PARSE_ALGO")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "cpp_port" | "cpp" | "port" => Self::CppPort,
            _ => Self::Current,
        }
    }

    /// Returns the algorithm name for display.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Current => "current (Rust)",
            Self::CppPort => "cpp_port (C++ faithful port)",
        }
    }
}

impl core::str::FromStr for ParseAlgorithm {
    type Err = core::convert::Infallible;

    /// Parse from a string value (for CLI arguments).
    ///
    /// Returns `Current` if unrecognized (never fails).
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value.to_lowercase().as_str() {
            "cpp_port" | "cpp" | "port" => Self::CppPort,
            _ => Self::Current,
        })
    }
}

// ============================================================================
// I/O Pipeline Algorithm Selection
// ============================================================================

/// Selects which I/O pipeline algorithm to use.
///
/// This allows switching between the current Rust implementation and
/// the new C++ port for testing and comparison.
///
/// # Environment Variable
///
/// Set `UFFS_IO_ALGO` to control the default:
/// - `current` (default): Use the current Rust I/O pipeline
/// - `cpp_port`: Use the C++ port I/O pipeline (bitmap sync point)
///
/// # Example
///
/// ```bash
/// # Use current algorithm (default)
/// UFFS_IO_ALGO=current uffs index
///
/// # Use C++ port algorithm
/// UFFS_IO_ALGO=cpp_port uffs index
/// ```
///
/// # Background
///
/// The C++ implementation uses a two-phase I/O model with a synchronization
/// point after bitmap reading completes. This ensures skip ranges are
/// calculated from the complete bitmap, not a partial one.
///
/// See `docs/architecture/CPP_IO_PIPELINE_PORT.md` for details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoPipelineAlgorithm {
    /// Current Rust I/O pipeline (default)
    Current,
    /// C++ port I/O pipeline - bitmap sync point before data reads
    CppPort,
}

impl Default for IoPipelineAlgorithm {
    /// Default checks the `UFFS_IO_ALGO` environment variable.
    fn default() -> Self {
        Self::from_env()
    }
}

impl IoPipelineAlgorithm {
    /// Parse from environment variable `UFFS_IO_ALGO`.
    ///
    /// Returns `Current` if not set or unrecognized.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("UFFS_IO_ALGO")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "cpp_port" | "cpp" | "port" => Self::CppPort,
            _ => Self::Current,
        }
    }

    /// Returns the algorithm name for display.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Current => "current (Rust)",
            Self::CppPort => "cpp_port (C++ I/O pipeline port)",
        }
    }
}

impl core::fmt::Display for IoPipelineAlgorithm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl core::str::FromStr for IoPipelineAlgorithm {
    type Err = core::convert::Infallible;

    /// Parse from a string value (for CLI arguments).
    ///
    /// Returns `Current` if unrecognized (never fails).
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value.to_lowercase().as_str() {
            "cpp_port" | "cpp" | "port" => Self::CppPort,
            _ => Self::Current,
        })
    }
}

// ============================================================================
// Chunk Processing Algorithm Selection
// ============================================================================

/// Selects which chunk processing algorithm to use.
///
/// This allows switching between the current Rust implementation and
/// a new chunk processing algorithm for testing and comparison.
///
/// # Environment Variable
///
/// Set `UFFS_CHUNK_ALGO` to control the default:
/// - `current` (default): Use the current Rust chunk processing
/// - `cpp_port`: Use the C++ port chunk processing (investigation target)
///
/// # Example
///
/// ```bash
/// # Use current algorithm (default)
/// UFFS_CHUNK_ALGO=current uffs index
///
/// # Use C++ port algorithm
/// UFFS_CHUNK_ALGO=cpp_port uffs index
/// ```
///
/// # Background
///
/// Investigation into 40 missing files revealed that offline MFT processing
/// achieves 100% parity with C++, but live Windows scanning loses 40 files.
/// This suggests the issue is in the chunk processing/handoff pipeline.
///
/// See `reference/Ultra-Fast-File-Search/LIVE_VS_OFFLINE_PARITY_INVESTIGATION.
/// md`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkAlgorithm {
    /// Current Rust chunk processing (default)
    Current,
    /// C++ port chunk processing - investigation target for 40 missing files
    CppPort,
}

impl Default for ChunkAlgorithm {
    /// Default checks the `UFFS_CHUNK_ALGO` environment variable.
    fn default() -> Self {
        Self::from_env()
    }
}

impl ChunkAlgorithm {
    /// Parse from environment variable `UFFS_CHUNK_ALGO`.
    ///
    /// Returns `Current` if not set or unrecognized.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("UFFS_CHUNK_ALGO")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "cpp_port" | "cpp" | "port" => Self::CppPort,
            _ => Self::Current,
        }
    }

    /// Returns the algorithm name for display.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Current => "current (Rust)",
            Self::CppPort => "cpp_port (C++ chunk processing - TODO)",
        }
    }
}

impl core::fmt::Display for ChunkAlgorithm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl core::str::FromStr for ChunkAlgorithm {
    type Err = core::convert::Infallible;

    /// Parse from a string value (for CLI arguments).
    ///
    /// Returns `Current` if unrecognized (never fails).
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value.to_lowercase().as_str() {
            "cpp_port" | "cpp" | "port" => Self::CppPort,
            _ => Self::Current,
        })
    }
}

// ============================================================================
// Index Build Timing - For Benchmarking
// ============================================================================

/// Timing breakdown for `MftIndex` building phases.
///
/// This is used for benchmarking and comparing with C++ implementation.
/// The C++ `--benchmark-index` includes "preprocessing" which corresponds
/// to our `tree_metrics_ms` phase.
///
/// # Phases
///
/// 1. **Record insertion**: Parsing and inserting records into the index
/// 2. **Extension index**: Building the extension lookup table
/// 3. **Sort children**: Sorting directory children for natural ordering
/// 4. **Tree metrics**: Computing descendants, treesize, `tree_allocated`
///
/// # Example
///
/// ```ignore
/// let (index, timing) = MftIndex::from_parsed_records_with_timing('C', records);
/// println!("Tree metrics: {} ms", timing.tree_metrics_ms);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexBuildTiming {
    /// Time spent inserting records into the index (ms).
    pub record_insert_ms: u64,
    /// Time spent building the extension index (ms).
    pub extension_index_ms: u64,
    /// Time spent sorting directory children (ms).
    pub sort_children_ms: u64,
    /// Time spent computing tree metrics (ms).
    /// This is the "preprocessing" phase in C++ terminology.
    pub tree_metrics_ms: u64,
    /// Total wall-clock time for index building (ms).
    pub total_ms: u64,
}

impl IndexBuildTiming {
    /// Returns the index build time excluding tree metrics.
    ///
    /// This is useful for comparing just the index structure building
    /// without the tree metrics computation.
    #[must_use]
    pub const fn index_only_ms(&self) -> u64 {
        self.record_insert_ms + self.extension_index_ms + self.sort_children_ms
    }

    /// Formats the timing as a human-readable string.
    #[must_use]
    pub fn to_string_pretty(&self) -> String {
        format!(
            "Record insert: {} ms, Ext index: {} ms, Sort: {} ms, Tree metrics: {} ms, Total: {} ms",
            self.record_insert_ms,
            self.extension_index_ms,
            self.sort_children_ms,
            self.tree_metrics_ms,
            self.total_ms
        )
    }
}

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

/// Hard link information.
///
/// Most files have only one name, stored inline in `FileRecord::first_name`.
/// Files with multiple hard links form a linked list via `next_entry`.
///
/// **Note**: Changed from C++ implementation - uses u64 for `parent_frs`
/// instead of u32 to support all valid NTFS volumes (48-bit FRS values).
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
    /// Matches C++ `match_attributes=false` behavior (`ntfs_index.hpp` line
    /// 1388-1392).
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
/// These correspond to internal attributes that the C++ implementation counts
/// as streams for tree metrics (e.g. `$REPARSE_POINT`, `$SECURITY_DESCRIPTOR`,
/// `$OBJECT_ID`, etc) but which UFFS intentionally does not expose as ADS rows.
///
/// They are stored separately so the C++ `delta()` hardlink distribution can be
/// applied per-stream (delta is not additive after integer rounding).
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

/// Directory child entry.
///
/// Directories maintain a linked list of their children for traversal.
///
/// **Note**: Changed from C++ implementation - uses u64 for `child_frs` instead
/// of u32 to support all valid NTFS volumes (48-bit FRS values).
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ChildInfo {
    /// Index of next `ChildInfo` in `MftIndex::children`, or `NO_ENTRY`
    pub next_entry: u32,
    /// FRS of the child file/directory (u64 to support all valid NTFS volumes)
    pub child_frs: u64,
    /// Which name index (for hard links)
    pub name_index: u16,
}

// ============================================================================
// FileRecord - Core file metadata (matches C++ Record)
// ============================================================================

/// Core file/directory record (matches C++ `Record`).
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
    /// Total number of ALL streams including internal Windows streams
    /// (`$REPARSE_POINT`, etc.) Used for tree metrics calculation to match
    /// C++ behavior.
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
    /// filtered from user-visible output but need to be included in tree
    /// metrics. This is used by the tree algorithm to match C++ behavior.
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
    /// # C++ Parity Notes
    ///
    /// - Directories (including reparse points like junctions/symlinks) always
    ///   return their computed tree metrics. Junctions are directory leaves
    ///   with `descendants=1`, not files with `descendants=0`.
    /// - Files return `descendants=0` and their own size/allocated values.
    /// - This matches the C++ reference implementation behavior.
    #[inline]
    #[must_use]
    pub const fn tree_metrics(&self) -> (u32, u64, u64) {
        (self.descendants, self.treesize, self.tree_allocated)
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
    /// Internal NTFS streams filtered from user-visible output but required for
    /// exact C++ tree-metrics parity.
    pub internal_streams: Vec<InternalStreamInfo>,
    /// Directory child entries
    pub children: Vec<ChildInfo>,
    /// Statistics collected during parsing
    pub stats: MftStats,
    /// Extension interning table for O(1) lookups and statistics
    pub extensions: ExtensionTable,
    /// Extension index for O(matches) queries (built after parsing)
    pub extension_index: Option<ExtensionIndex>,
    /// Whether this index was built with forensic mode enabled.
    /// When true, `to_dataframe()` includes `is_deleted`, `is_corrupt`,
    /// `is_extension`, and `base_frs` columns.
    pub forensic_mode: bool,
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

/// C++ `Accumulator::delta` formula for proportional hardlink size division.
///
/// When a file has N hardlinks, each hardlink parent gets a proportional share
/// of the file's size. This ensures the total size across all parents equals
/// the file's actual size.
///
/// Formula: `value * (i + 1) / n - value * i / n`
///
/// # Arguments
/// * `value` - The size to divide (e.g., file size in bytes)
/// * `name_info` - Which hardlink this is (0, 1, 2, ..., n-1)
/// * `total_names` - Total number of hardlinks
///
/// # Example
/// For a 99-byte file with 3 hardlinks:
/// - Hardlink 0: delta(99, 0, 3) = 99*1/3 - 99*0/3 = 33
/// - Hardlink 1: delta(99, 1, 3) = 99*2/3 - 99*1/3 = 33
/// - Hardlink 2: delta(99, 2, 3) = 99*3/3 - 99*2/3 = 33
/// - Total: 33 + 33 + 33 = 99 ✓
#[inline]
fn hardlink_delta(value: u64, name_info: u16, total_names: u16) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let i = u64::from(name_info);
    let n = u64::from(total_names);
    (value * (i + 1) / n) - (value * i / n)
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
            internal_streams: Vec::new(),
            children: Vec::with_capacity(record_capacity),
            stats: MftStats::new(),
            extensions: ExtensionTable::new(),
            extension_index: None,
            forensic_mode: false,
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
            let parent_frs = record.first_name.parent_frs;
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

    /// Add a child entry to a parent directory.
    ///
    /// This creates a `ChildInfo` entry linking the child to the parent.
    /// Used for building parent-child relationships for tree metrics.
    ///
    /// # Arguments
    /// * `parent_frs` - FRS of the parent directory
    /// * `child_frs` - FRS of the child file/directory
    /// * `name_index` - Which hardlink this is (0 for primary, 1+ for
    ///   additional)
    ///
    /// # C++ Parity
    ///
    /// This method now matches C++ behavior by creating a placeholder parent
    /// record if it doesn't exist (via `get_or_create()`). This ensures child
    /// entries are never lost due to out-of-order chunk processing.
    ///
    /// See: `ntfs_index.hpp` lines 568-579 where C++ uses `at(frs_parent)` to
    /// create parent placeholders on-demand.
    #[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
    pub fn add_child_entry(&mut self, parent_frs: u64, child_frs: u64, name_index: u16) {
        // C++ parity: Create parent placeholder if it doesn't exist
        // This matches C++ at(frs_parent) behavior in ntfs_index.hpp
        let parent_frs_usize = parent_frs as usize;

        // Expand lookup table if needed
        if parent_frs_usize >= self.frs_to_idx.len() {
            self.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
        }

        // Get or create parent record index
        let parent_idx = if self.frs_to_idx[parent_frs_usize] == NO_ENTRY {
            // Create placeholder parent record
            let new_idx = self.records.len() as u32;
            self.frs_to_idx[parent_frs_usize] = new_idx;
            self.records.push(FileRecord::new(parent_frs));
            new_idx as usize
        } else {
            self.frs_to_idx[parent_frs_usize] as usize
        };

        // Create child entry
        let child_idx = self.children.len() as u32;
        let old_first_child = self.records[parent_idx].first_child;

        // Update parent's first_child before pushing to children
        self.records[parent_idx].first_child = child_idx;

        self.children.push(ChildInfo {
            next_entry: old_first_child,
            child_frs,
            name_index,
        });
    }

    /// Rebuilds `first_child` linked lists from `FILE_NAME` parent references.
    ///
    /// This is a self-heal for LIVE pipelines where child lists may be
    /// incomplete due to ordering / placeholder timing. It is deterministic
    /// and based only on already-parsed `parent_frs` links.
    ///
    /// # Algorithm
    ///
    /// 1. Collect `(parent_frs, child_frs, name_index)` edges from all records
    /// 2. Clear existing children and reset `first_child` pointers
    /// 3. Rebuild using `add_child_entry()`
    ///
    /// # When to Use
    ///
    /// Call this as a fallback if tree metrics computation leaves directories
    /// with `descendants == 0`, which indicates the child graph was incomplete.
    #[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
    pub fn rebuild_children_from_names(&mut self) {
        tracing::debug!(
            records = self.records.len(),
            "[TRIP] MftIndex::rebuild_children_from_names ENTER"
        );
        let no_entry_frs: u64 = u64::from(NO_ENTRY);

        // Phase 1: collect (parent_frs, child_frs, name_index) edges.
        // Indexing is safe: child_idx is bounded by 0..self.records.len()
        let mut edges: Vec<(u64, u64, u16)> =
            Vec::with_capacity(self.records.len().saturating_mul(2));

        for child_idx in 0..self.records.len() {
            let child_frs = self.records[child_idx].frs;
            let name_count = usize::from(self.records[child_idx].name_count);

            // Walk the child's link chain in stored order.
            let mut current_link = self.records[child_idx].first_name;
            for name_index in 0..name_count {
                let parent_frs = current_link.parent_frs;

                // Skip missing/placeholder parents and self-references (root has parent==self).
                if parent_frs != no_entry_frs && parent_frs != child_frs {
                    #[allow(clippy::cast_possible_truncation)]
                    // NOTE: ChildInfo.name_index must match the C++ parse-order index (FILE_NAME
                    // attribute encounter order). The stored link chain order
                    // is the *reverse* of parse order because each new name becomes `first_name`.
                    // Therefore, list_index (0..name_count-1) must be remapped back to parse_index.
                    let parse_index = (name_count - 1 - name_index) as u16;
                    edges.push((parent_frs, child_frs, parse_index));
                }

                if current_link.next_entry == NO_ENTRY {
                    break;
                }
                // Bounds checked via .get() - breaks if out of range
                if let Some(next_link) = self.links.get(current_link.next_entry as usize) {
                    current_link = *next_link;
                } else {
                    break;
                }
            }
        }

        tracing::debug!(
            edges_collected = edges.len(),
            "[TRIP] MftIndex::rebuild_children_from_names -> Phase 1 done"
        );

        // Phase 2: reset child lists and rebuild.
        self.children.clear();
        for rec in &mut self.records {
            rec.first_child = NO_ENTRY;
        }

        for (parent_frs, child_frs, name_index) in edges {
            self.add_child_entry(parent_frs, child_frs, name_index);
        }
        tracing::debug!(
            children = self.children.len(),
            "[TRIP] MftIndex::rebuild_children_from_names EXIT"
        );
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
                    .frs_to_idx_opt(child_a.child_frs)
                    .and_then(|idx| self.records.get(idx));
                let rec_b = self
                    .frs_to_idx_opt(child_b.child_frs)
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
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cognitive_complexity,
        clippy::too_many_lines
    )] // Justified: n < u32::MAX in practice, algorithm is inherently complex
    pub fn compute_tree_metrics(&mut self) {
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics ENTER (default algo)");
        self.compute_tree_metrics_with_algo(TreeAlgorithm::default(), false);
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics EXIT");
    }

    /// Compute tree metrics with optional debug output.
    ///
    /// When `debug` is true, prints detailed information about hardlink
    /// handling to stdout for debugging purposes.
    #[allow(clippy::print_stdout)]
    pub fn compute_tree_metrics_debug(&mut self) {
        self.compute_tree_metrics_with_algo(TreeAlgorithm::default(), true);
    }

    /// Compute tree metrics using a specific algorithm.
    ///
    /// This allows switching between the current leaf-peeling algorithm
    /// and the new C++ port algorithm for testing and comparison.
    ///
    /// # Arguments
    /// * `algo` - Which tree algorithm to use
    /// * `debug` - If true, prints detailed debug information
    #[allow(clippy::print_stdout)]
    pub fn compute_tree_metrics_with_algo(&mut self, algo: TreeAlgorithm, debug: bool) {
        tracing::debug!(algo = ?algo, "[TRIP] MftIndex::compute_tree_metrics_with_algo dispatching");
        match algo {
            TreeAlgorithm::Current => self.compute_tree_metrics_impl(debug),
            TreeAlgorithm::CppPort => self.compute_tree_metrics_cpp_port(debug),
        }
    }

    /// C++ port tree metrics algorithm.
    ///
    /// This is a 100% faithful port of the C++ tree algorithm with matching
    /// structures, entities, and algorithm flow. It uses recursive DFS
    /// traversal starting from root (FRS 5) and the delta formula for
    /// proportional hardlink share calculation.
    ///
    /// # Self-Healing for LIVE Scans
    ///
    /// LIVE scans can occasionally produce incomplete child lists due to
    /// ordering/timing issues. If the first tree pass leaves any directories
    /// with `descendants == 0`, this method rebuilds the child lists from
    /// `FILE_NAME` parent references and reruns tree metrics.
    ///
    /// See `docs/architecture/CPP_TREE_ALGORITHM_PORT.md` for full
    /// documentation.
    #[allow(clippy::print_stdout)]
    fn compute_tree_metrics_cpp_port(&mut self, debug: bool) {
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics_cpp_port ENTER (first pass)");
        // First pass: compute tree metrics
        crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics_cpp_port -> first pass done");

        // Detect "unstamped directory" condition (LIVE scan symptom).
        // Directories should have descendants >= 1 (at least themselves).
        let bad_dir_count = self
            .records
            .iter()
            .filter(|rec| rec.stdinfo.is_directory() && rec.descendants == 0)
            .count();

        // Also check root specifically for treesize=0 (belt-and-suspenders).
        // Root should always have treesize > 0 if there are any files on the volume.
        let root_looks_bad = self
            .frs_to_idx_opt(5)
            .and_then(|root_idx| self.records.get(root_idx))
            .is_some_and(|root| {
                root.stdinfo.is_directory() && (root.descendants == 0 || root.treesize == 0)
            });

        tracing::debug!(
            bad_dir_count,
            root_looks_bad,
            "[TRIP] MftIndex::compute_tree_metrics_cpp_port -> self-heal check"
        );

        if bad_dir_count != 0 || root_looks_bad {
            tracing::debug!(
                "[TRIP] MftIndex::compute_tree_metrics_cpp_port -> SELF-HEAL TRIGGERED"
            );
            tracing::warn!(
                bad_dir_count,
                root_looks_bad,
                "[tree] unstamped directories or root after first pass; \
                 rebuilding child lists from names and rerunning"
            );

            // Rebuild child lists from FILE_NAME parent references
            tracing::debug!(
                "[TRIP] MftIndex::compute_tree_metrics_cpp_port -> calling rebuild_children_from_names"
            );
            self.rebuild_children_from_names();

            // Second pass: recompute tree metrics with fixed child lists
            tracing::debug!(
                "[TRIP] MftIndex::compute_tree_metrics_cpp_port -> second pass (after self-heal)"
            );
            crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
        }

        // Post-tree diagnostic: log which directories STILL have descendants==0
        // after all passes (including self-heal). This runs in RELEASE builds
        // to help diagnose LIVE scan issues.
        // Interpretation:
        // - If bad_dirs is non-empty here → Failure mode A/C (not stamped)
        // - If bad_dirs is empty here but CSV shows bad rows → Failure mode B (reset
        //   after compute)
        let final_bad_dirs: Vec<_> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, rec)| rec.stdinfo.is_directory() && rec.descendants == 0)
            .map(|(idx, rec)| {
                (
                    idx,
                    rec.frs,
                    rec.first_child,
                    rec.name_count,
                    rec.total_stream_count,
                    rec.stdinfo.is_reparse(),
                )
            })
            .collect();

        if !final_bad_dirs.is_empty() {
            tracing::warn!(
                bad_dir_count = final_bad_dirs.len(),
                "[tree] FINAL: directories with descendants==0 after all tree metrics passes"
            );
            // Log first 10 bad directories for debugging
            for (idx, frs, first_child, name_count, stream_count, is_reparse) in
                final_bad_dirs.iter().take(10)
            {
                tracing::warn!(
                    idx,
                    frs,
                    first_child,
                    name_count,
                    stream_count,
                    is_reparse,
                    "[tree] FINAL: bad directory details"
                );
            }
        }

        tracing::debug!("[TRIP] MftIndex::compute_tree_metrics_cpp_port EXIT");
    }

    /// Internal implementation of tree metrics computation.
    ///
    /// This method implements the C++ tree metrics algorithm using a Kahn-style
    /// leaf-peeling approach. It computes descendants, treesize, and
    /// `tree_allocated` for each record by processing leaves first and
    /// propagating values up to parents.
    ///
    /// # Arguments
    /// * `debug` - If true, prints detailed debug information during
    ///   computation
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cognitive_complexity,
        clippy::float_arithmetic,
        clippy::map_unwrap_or,
        clippy::min_ident_chars,
        clippy::print_stdout,
        clippy::too_many_lines,
        clippy::uninlined_format_args,
        clippy::unnecessary_sort_by
    )]
    fn compute_tree_metrics_impl(&mut self, debug: bool) {
        let n = self.records.len();
        if n == 0 {
            tracing::debug!("⏭️  Skipping tree metrics - no records");
            return;
        }

        if debug {
            println!("=== TREE METRICS DEBUG ===");
            println!("Total records: {n}");
            println!("Total children entries: {}", self.children.len());
        }

        tracing::debug!(records = n, "🔨 Computing tree metrics (C++ algorithm)...");

        // =========================================================================
        // C++ TREE METRICS ALGORITHM (Q18 answer from C++ team)
        // =========================================================================
        //
        // Key insight: Each hardlink creates a SEPARATE child entry in its parent.
        // The tree traversal visits each child entry, and each child knows which
        // hardlink index it represents. The proportional share is calculated using
        // the delta formula: delta(value, name_info, total_names).
        //
        // Algorithm:
        // 1. Build base metrics for each record (size, allocated, stream_count)
        // 2. Build pending_children count from the children list
        // 3. Leaf-peeling: for each child entry, calculate proportional share and add
        //    to parent
        // =========================================================================

        // Phase 1: Calculate base metrics for each record
        // Sum ALL streams' sizes (default + ADS) for C++ parity
        let base_metrics: Vec<_> = self
            .records
            .iter()
            .map(|record| {
                let is_directory = record.is_directory();
                // Use total_stream_count for tree metrics (includes internal streams)
                // This matches C++ behavior where ALL streams contribute to treesize
                let stream_count = u32::from(record.total_stream_count).max(1);
                let name_count = record.name_count.max(1);

                // Sum sizes across ALL streams (default + ADS)
                let mut total_size = record.first_stream.size.length;
                let mut total_allocated = record.first_stream.size.allocated;

                let mut next_entry = record.first_stream.next_entry;
                while next_entry != NO_ENTRY {
                    if let Some(stream) = self.streams.get(next_entry as usize) {
                        total_size = total_size.saturating_add(stream.size.length);
                        total_allocated = total_allocated.saturating_add(stream.size.allocated);
                        next_entry = stream.next_entry;
                    } else {
                        break;
                    }
                }

                (
                    is_directory,
                    stream_count,
                    name_count,
                    total_size,
                    total_allocated,
                )
            })
            .collect();

        // Phase 2: Build pending_children count from the children list
        // Each child entry represents one hardlink, so we count child entries, not
        // records
        let mut pending_children = vec![0_u32; n];

        // Build pending_children: count children for each parent
        pending_children.fill(0);
        for (parent_idx, record) in self.records.iter().enumerate() {
            let mut child_entry_idx = record.first_child;
            while child_entry_idx != NO_ENTRY {
                if let Some(child_entry) = self.children.get(child_entry_idx as usize) {
                    // Verify the child exists
                    if self.frs_to_idx_opt(child_entry.child_frs).is_some() {
                        if let Some(slot) = pending_children.get_mut(parent_idx) {
                            *slot += 1;
                        }
                    }
                    child_entry_idx = child_entry.next_entry;
                } else {
                    break;
                }
            }
        }

        // Phase 3: Initialize records with base metrics
        // Files: descendants = 0, treesize = own size, tree_allocated = own allocated
        // Directories: descendants = 1, treesize = own index size, tree_allocated = own
        // allocated Note: C++ includes ALL records in tree metrics (including
        // system metafiles). System metafiles are just not output to CSV, but
        // their sizes ARE included in the root's treesize.

        for (idx, (is_directory, _stream_count, _name_count, size, allocated)) in
            base_metrics.iter().enumerate()
        {
            if let Some(record) = self.records.get_mut(idx) {
                // C++ includes ALL records in tree metrics, including system metafiles.
                // System metafiles are just not output to CSV, but their sizes ARE
                // included in the root's treesize. C++ starts from FRS 5 (root) and
                // visits all children via the child entry tree - no explicit exclusion.
                //
                // C++ algorithm (line 4628): info->treesize = isdir;
                // The default stream's treesize is initialized to 1 for directories ($I30)
                // and 0 for files ($DATA). During tree traversal:
                // - Line 4788: result.treesize += 1 for each stream (parent's accumulated)
                // - Line 4794: k->treesize += children_size.treesize (default stream only)
                // The output shows the default stream's treesize, not result.treesize.
                //
                // So for directories: initial descendants = 1 (the $I30 stream's treesize)
                // For files: initial descendants = 0 (the $DATA stream's treesize)
                record.descendants = u32::from(*is_directory);
                // Both directories and files have their own size in treesize
                // Directories: size comes from $INDEX_ROOT + $INDEX_ALLOCATION + $BITMAP
                // Files: size comes from $DATA stream(s)
                record.treesize = *size;
                record.tree_allocated = *allocated;
            }
        }

        // Phase 4: Build reverse mapping - for each record, which parents have child
        // entries? This is needed because a record with hardlinks has multiple
        // parents Structure: child_to_parents[child_idx] = Vec<(parent_idx,
        // name_index)>
        let mut child_to_parents: Vec<Vec<(usize, u16)>> = vec![Vec::new(); n];

        for (parent_idx, record) in self.records.iter().enumerate() {
            let mut child_entry_idx = record.first_child;
            while child_entry_idx != NO_ENTRY {
                if let Some(child_entry) = self.children.get(child_entry_idx as usize) {
                    if let Some(child_record_idx) = self.frs_to_idx_opt(child_entry.child_frs) {
                        // C++ parity: Skip if child is the same as parent (root directory
                        // is the only one that is a child of itself)
                        // See C++ line 4691: if (fr2 != fr)
                        if child_record_idx != parent_idx {
                            if let Some(parents_list) = child_to_parents.get_mut(child_record_idx) {
                                parents_list.push((parent_idx, child_entry.name_index));
                            }
                        }
                    }
                    child_entry_idx = child_entry.next_entry;
                } else {
                    break;
                }
            }
        }

        // Debug: Show hardlink statistics
        if debug {
            let records_with_multiple_parents: usize = child_to_parents
                .iter()
                .filter(|parents| parents.len() > 1)
                .count();
            let records_with_hardlinks: usize = base_metrics
                .iter()
                .filter(|(_, _, name_count, _, _)| *name_count > 1)
                .count();
            let total_parent_entries: usize = child_to_parents.iter().map(Vec::len).sum();

            println!();
            println!("=== HARDLINK STATISTICS ===");
            println!("Records with name_count > 1: {records_with_hardlinks}");
            println!("Records with multiple parent entries: {records_with_multiple_parents}");
            println!("Total parent entries in child_to_parents: {total_parent_entries}");
            println!("Expected (if all hardlinks have entries): should equal total_parent_entries");

            // Show first 10 records with hardlinks
            println!();
            println!("=== SAMPLE HARDLINKS (first 10) ===");
            let mut shown = 0_u32;
            for (idx, (_, _, name_count, size, _)) in base_metrics.iter().enumerate() {
                if *name_count > 1 && shown < 10_u32 {
                    let Some(parents) = child_to_parents.get(idx) else {
                        continue;
                    };
                    let frs = self.records.get(idx).map_or(0, |rec| rec.frs);
                    println!(
                        "  FRS {}: name_count={}, size={}, parent_entries={}",
                        frs,
                        name_count,
                        size,
                        parents.len()
                    );
                    for (parent_i, (parent_idx, name_index)) in parents.iter().enumerate() {
                        let parent_frs = self.records.get(*parent_idx).map_or(0, |rec| rec.frs);
                        println!(
                            "    [{parent_i}] parent_idx={parent_idx} (FRS {parent_frs}), name_index={name_index}"
                        );
                    }
                    shown += 1_u32;
                }
            }
        }

        // Phase 5: Leaf-peeling with proportional share calculation
        // Start with all leaf nodes (records with no children)
        let mut stack: Vec<usize> = Vec::with_capacity(n);
        for (idx, &pending_count) in pending_children.iter().enumerate() {
            if pending_count == 0 {
                stack.push(idx);
            }
        }

        let mut processed = 0_usize;
        let mut debug_hardlink_contributions: Vec<(u64, u16, u16, u64, u64)> = Vec::new();

        while let Some(child_idx) = stack.pop() {
            processed += 1;

            // Get child's metrics (safe: child_idx comes from stack which only contains
            // valid indices)
            let Some(&(is_directory, stream_count, name_count, _size, _allocated)) =
                base_metrics.get(child_idx)
            else {
                continue;
            };
            let (child_frs, child_descendants, child_treesize, child_tree_allocated) =
                if let Some(child) = self.records.get(child_idx) {
                    (
                        child.frs,
                        child.descendants,
                        child.treesize,
                        child.tree_allocated,
                    )
                } else {
                    continue;
                };

            // Calculate contribution for descendants (C++ parity)
            // C++ algorithm:
            // - result.treesize = children_size.treesize + stream_count (line 4726, 4788)
            // - k->treesize = isdir + children_size.treesize (line 4628, 4794)
            // So: result.treesize = stream_count + (k->treesize - isdir)
            //
            // The contribution to parent is result.treesize, which is:
            // - stream_count + (child_descendants - isdir)
            // Where isdir = 1 for directories, 0 for files
            let isdir = u32::from(is_directory);
            let descendants_contribution = stream_count + child_descendants.saturating_sub(isdir);

            // For each parent that has a child entry pointing to this record
            let Some(parents_list) = child_to_parents.get(child_idx) else {
                continue;
            };
            for &(parent_idx, name_index) in parents_list {
                // C++ formula: name_info = name_count - 1 - name_index
                let name_info = name_count.saturating_sub(1).saturating_sub(name_index);

                // Calculate proportional share using delta formula
                let size_share = hardlink_delta(child_treesize, name_info, name_count);
                let allocated_share = hardlink_delta(child_tree_allocated, name_info, name_count);

                // Debug: track hardlink contributions
                if debug && name_count > 1 && debug_hardlink_contributions.len() < 20 {
                    debug_hardlink_contributions.push((
                        child_frs,
                        name_index,
                        name_count,
                        size_share,
                        allocated_share,
                    ));
                }

                // Add to parent
                if let Some(parent) = self.records.get_mut(parent_idx) {
                    parent.descendants += descendants_contribution;
                    parent.treesize += size_share;
                    parent.tree_allocated += allocated_share;
                }

                // Decrement parent's pending count
                if let Some(slot) = pending_children.get_mut(parent_idx) {
                    *slot = slot.saturating_sub(1);
                    if *slot == 0 {
                        stack.push(parent_idx);
                    }
                }
            }
        }

        // Debug: Show hardlink contributions
        if debug && !debug_hardlink_contributions.is_empty() {
            println!();
            println!("=== HARDLINK CONTRIBUTIONS (first 20) ===");
            for (frs, name_index, name_count, size_share, alloc_share) in
                &debug_hardlink_contributions
            {
                let name_info = name_count.saturating_sub(1).saturating_sub(*name_index);
                println!(
                    "  FRS {}: name_index={}, name_count={}, name_info={} → size_share={}, alloc_share={}",
                    frs, name_index, name_count, name_info, size_share, alloc_share
                );
            }
        }

        // Phase 6: Defensive corruption detection
        if processed == n {
            tracing::debug!(
                processed,
                total = n,
                "✅ Tree metrics computed successfully for all records"
            );
        } else {
            tracing::warn!(
                processed,
                total = n,
                missing = n - processed,
                "⚠️ Tree metrics computation incomplete - possible cycles or broken parent links"
            );
        }

        // Debug: Show root directory metrics
        if debug {
            // Show size distribution of base metrics
            let total_size: u64 = base_metrics.iter().map(|(_, _, _, s, _)| *s).sum();
            let total_alloc: u64 = base_metrics.iter().map(|(_, _, _, _, a)| *a).sum();
            let file_count = base_metrics
                .iter()
                .filter(|(is_dir, _, _, _, _)| !*is_dir)
                .count();
            let dir_count = base_metrics
                .iter()
                .filter(|(is_dir, _, _, _, _)| *is_dir)
                .count();

            println!();
            println!("=== BASE METRICS SUMMARY ===");
            println!("  Files: {file_count}");
            println!("  Directories: {dir_count}");
            println!(
                "  Total base size: {} ({:.2} MB)",
                total_size,
                total_size as f64 / 1_000_000.0_f64
            );
            println!(
                "  Total base allocated: {} ({:.2} MB)",
                total_alloc,
                total_alloc as f64 / 1_000_000.0_f64
            );

            // Show top 10 files by allocated size
            let mut by_alloc: Vec<_> = base_metrics
                .iter()
                .enumerate()
                .filter(|(_, (is_dir, _, _, _, _))| !*is_dir)
                .map(|(idx, (_, _, _, size, alloc))| (idx, *size, *alloc))
                .collect();
            by_alloc.sort_by(|a, b| b.2.cmp(&a.2));

            println!();
            println!("=== TOP 10 FILES BY ALLOCATED SIZE ===");
            for (idx, size, alloc) in by_alloc.iter().take(10) {
                let frs = self.records.get(*idx).map(|r| r.frs).unwrap_or(0);
                println!("  FRS {}: size={}, allocated={}", frs, size, alloc);
            }

            println!();
            println!("=== ROOT DIRECTORY (FRS 5) ===");
            if let Some(root_idx) = self.frs_to_idx_opt(5) {
                if let Some(root) = self.records.get(root_idx) {
                    println!("  FRS: {}", root.frs);
                    println!("  Descendants: {}", root.descendants);
                    println!(
                        "  Treesize: {} ({:.2} MB)",
                        root.treesize,
                        root.treesize as f64 / 1_000_000.0_f64
                    );
                    println!(
                        "  Tree Allocated: {} ({:.2} MB)",
                        root.tree_allocated,
                        root.tree_allocated as f64 / 1_000_000.0_f64
                    );
                }
            } else {
                println!("  Root not found!");
            }
            println!();
            println!("Processed: {processed} / {n} records");
        }

        // Debug: Show sample of computed metrics (first 5 directories)
        if tracing::enabled!(tracing::Level::DEBUG) {
            let sample_dirs: Vec<_> = self
                .records
                .iter()
                .filter(|rec| rec.is_directory())
                .take(5)
                .map(|rec| (rec.frs, rec.descendants, rec.treesize, rec.tree_allocated))
                .collect();
            if !sample_dirs.is_empty() {
                tracing::debug!("📊 Sample tree metrics (first 5 dirs): {:?}", sample_dirs);
            }
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
        let mut current_frs = name_info.parent_frs;

        while current_frs != u64::from(NO_ENTRY) && current_frs != ROOT_FRS {
            if let Some(parent_record) = self.find(current_frs) {
                let parent_name = self.record_name(parent_record);
                if !parent_name.is_empty() && parent_name != "." {
                    components.push(parent_name.to_owned());
                }

                let parent_frs = parent_record.first_name.parent_frs;
                if parent_frs == u64::from(NO_ENTRY) || parent_frs == current_frs {
                    break;
                }
                current_frs = parent_frs;
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
            if parent_frs == u64::from(NO_ENTRY) || parent_frs == current_frs {
                break; // Root or self-reference
            }
            if parent_frs == ROOT_FRS {
                break; // Reached root
            }
            current_frs = parent_frs;
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

            let parent_frs = record.first_name.parent_frs;
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
        // Normalize volume root to "X:\\"
        // NTFS root record (FRS=5) often has FILE_NAME="." which we skip above,
        // so without this normalization we'd return "X:" which is drive-relative.
        if path.len() == 2 && path.as_bytes().last() == Some(&b':') {
            path.push('\\');
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

        let parent_frs = link.parent_frs;
        let parent_path = if let Some(pidx) = index.frs_to_idx_opt(parent_frs) {
            self.materialize_path(index, pidx)
        } else if parent_frs == ROOT_FRS {
            // Normalize root to "X:\\" (not "X:") so hardlink paths match C++ / Win32
            // semantics.
            let mut root_path = String::with_capacity(3);
            root_path.push(self.volume.to_ascii_uppercase());
            root_path.push(':');
            root_path.push('\\');
            root_path
        } else {
            return String::new();
        };

        let name = index.link_name(link);
        if name.is_empty() || name == "." {
            parent_path
        } else {
            let mut path = String::with_capacity(parent_path.len() + 1 + name.len());
            path.push_str(&parent_path);
            // Avoid double separators if parent is already the volume root ("X:\\").
            let ends_with_sep = parent_path.as_bytes().last() == Some(&b'\\');
            if !ends_with_sep {
                path.push('\\');
            }
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
                if let Some(child_idx) = index.frs_to_idx_opt(child_info.child_frs) {
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

                let parent_frs = record.first_name.parent_frs;

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
    clippy::use_debug,
    clippy::unwrap_used, // Test code - unwrap is acceptable
    clippy::expect_used  // Test code - expect is acceptable
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
        // Verify compact size - should be reasonably compact (<= 240 bytes)
        // Version 4 added: sequence_number (2), namespace (1), reserved (1),
        // fn_created/modified/accessed/mft_changed (4 × 8 = 32) = 36 bytes extra
        // Version 5 added: lsn (8 bytes) for forensic correlation
        // Version 6 added: reparse_tag (4 bytes)
        // Version 7 added: base_frs (8 bytes) for forensic extension records
        // Version 8 added: total_stream_count (2 bytes, with padding = 4 bytes)
        //                  internal_streams_size (8 bytes)
        //                  internal_streams_allocated (8 bytes)
        //                  = 20 bytes extra for C++ tree metrics parity
        let size = size_of::<FileRecord>();
        assert!(size <= 240, "FileRecord too large: {size} bytes");
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
            rec.first_name.parent_frs = dir_frs;

            // Add child to directory's children list
            let child_info = ChildInfo {
                next_entry: NO_ENTRY,
                child_frs,
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
            let child_idx = index.frs_to_idx_opt(child.child_frs).unwrap();
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
        rec.first_name.parent_frs = dir_frs;

        let child_info = ChildInfo {
            next_entry: NO_ENTRY,
            child_frs,
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
            rec.first_name.parent_frs = dir_frs;

            let child_info = ChildInfo {
                next_entry: NO_ENTRY,
                child_frs,
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
            let child_idx = index.frs_to_idx_opt(child.child_frs).unwrap();
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
        root_rec.first_name.parent_frs = root_frs; // Self-parent

        // dir1
        let dir1_frs = 100_u64;
        let offset = index.add_name("dir1");
        let rec = index.get_or_create(dir1_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs;

        // file1.txt (child of dir1)
        let file1_frs = 200_u64;
        let offset = index.add_name("file1.txt");
        let rec = index.get_or_create(file1_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir1_frs;
        rec.first_stream.size = SizeInfo {
            length: 1000,
            allocated: 4096,
        };

        // file2.txt (child of dir1)
        let file2_frs = 201_u64;
        let offset = index.add_name("file2.txt");
        let rec = index.get_or_create(file2_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir1_frs;
        rec.first_stream.size = SizeInfo {
            length: 2000,
            allocated: 4096,
        };

        // file3.txt (child of root)
        let file3_frs = 202_u64;
        let offset = index.add_name("file3.txt");
        let rec = index.get_or_create(file3_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs;
        rec.first_stream.size = SizeInfo {
            length: 500,
            allocated: 4096,
        };

        // Add child entries (required for tree metrics algorithm)
        index.add_child_entry(root_frs, dir1_frs, 0);
        index.add_child_entry(root_frs, file3_frs, 0);
        index.add_child_entry(dir1_frs, file1_frs, 0);
        index.add_child_entry(dir1_frs, file2_frs, 0);

        // Compute tree metrics
        index.compute_tree_metrics();

        // Verify file1.txt (leaf)
        // C++ parity: Files have descendants = 0, but contribute 1 to parent
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

        // Verify dir1 (has 2 children: file1 and file2)
        // C++ parity: descendants = 1 (self) + sum(max(1, child.descendants))
        // dir1 = 1 + 1 + 1 = 3
        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
        assert_eq!(index.records[dir1_idx].descendants, 3); // 1 + file1(1) + file2(1)
        assert_eq!(index.records[dir1_idx].treesize, 3000); // 0 + 1000 + 2000
        assert_eq!(index.records[dir1_idx].tree_allocated, 8192); // 0 + 4096 + 4096

        // Verify root (has dir1 + file3)
        // C++ parity: descendants = 1 (self) + sum(child.descendants)
        // root = 1 + 3 + 1 = 5
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(index.records[root_idx].descendants, 5); // 1 + dir1(3) + file3(1)
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
        root_rec.first_name.parent_frs = root_frs;

        // dir1
        let dir1_frs = 100_u64;
        let offset = index.add_name("dir1");
        let rec = index.get_or_create(dir1_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs;

        // dir2
        let dir2_frs = 101_u64;
        let offset = index.add_name("dir2");
        let rec = index.get_or_create(dir2_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir1_frs;

        // dir3
        let dir3_frs = 102_u64;
        let offset = index.add_name("dir3");
        let rec = index.get_or_create(dir3_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir2_frs;

        // file.txt
        let file_frs = 200_u64;
        let offset = index.add_name("file.txt");
        let rec = index.get_or_create(file_frs);
        rec.first_name.name = IndexNameRef::new(offset, 8, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir3_frs;
        rec.first_stream.size = SizeInfo {
            length: 1000,
            allocated: 4096,
        };

        // Add child entries (required for tree metrics algorithm)
        index.add_child_entry(root_frs, dir1_frs, 0);
        index.add_child_entry(dir1_frs, dir2_frs, 0);
        index.add_child_entry(dir2_frs, dir3_frs, 0);
        index.add_child_entry(dir3_frs, file_frs, 0);

        // Compute tree metrics
        index.compute_tree_metrics();

        // C++ parity: Files have descendants = 0, dirs have descendants = 1 +
        // sum(max(1, child.descendants)) Formula: parent.descendants = 1 +
        // sum(max(1, child.descendants)) file.txt = 0, dir3 = 1+max(1,0)=2,
        // dir2 = 1+2=3, dir1 = 1+3=4, root = 1+4=5

        // Verify file.txt (leaf)
        let file_idx = index.frs_to_idx_opt(file_frs).unwrap();
        assert_eq!(index.records[file_idx].descendants, 0); // Files have 0
        assert_eq!(index.records[file_idx].treesize, 1000);

        // Verify dir3 (has 1 child: file.txt)
        let dir3_idx = index.frs_to_idx_opt(dir3_frs).unwrap();
        assert_eq!(index.records[dir3_idx].descendants, 2); // 1 + max(1, file.txt(0)) = 1 + 1 = 2
        assert_eq!(index.records[dir3_idx].treesize, 1000);

        // Verify dir2 (has 1 child: dir3)
        let dir2_idx = index.frs_to_idx_opt(dir2_frs).unwrap();
        assert_eq!(index.records[dir2_idx].descendants, 3); // 1 + dir3(2)
        assert_eq!(index.records[dir2_idx].treesize, 1000);

        // Verify dir1 (has 1 child: dir2)
        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
        assert_eq!(index.records[dir1_idx].descendants, 4); // 1 + dir2(3)
        assert_eq!(index.records[dir1_idx].treesize, 1000);

        // Verify root (has 1 child: dir1)
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(index.records[root_idx].descendants, 5); // 1 + dir1(4)
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
        root_rec.first_name.parent_frs = root_frs;

        let mut frs_counter = 1000_u64;

        // Create 100 directories
        for dir_idx in 0..100_u64 {
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
            rec.first_name.parent_frs = root_frs;

            // Add child entry for directory
            index.add_child_entry(root_frs, dir_frs, 0);

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
                rec.first_name.parent_frs = dir_frs;
                rec.first_stream.size = SizeInfo {
                    length: 1000,
                    allocated: 4096,
                };

                // Add child entry for file
                index.add_child_entry(dir_frs, file_frs, 0);
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
        // C++ parity: Files have descendants = 0, dirs have descendants = 1 +
        // sum(max(1, child.descendants)) Each file = 0 (but contributes 1 to
        // parent) Each dir_i = 1 (self) + 100 files * max(1,0) = 1 + 100 = 101
        // root = 1 (self) + 100 dirs * 101 = 10,101
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(index.records[root_idx].descendants, 10_101); // 1 + 100 * 101

        // Verify root has correct total size
        assert_eq!(index.records[root_idx].treesize, 10_000_000); // 10,000 files * 1000 bytes

        // Verify a directory has correct descendants
        // Each dir = 1 (self) + 100 files * max(1,0) = 1 + 100 = 101
        let dir0_idx = index.frs_to_idx_opt(100).unwrap();
        assert_eq!(index.records[dir0_idx].descendants, 101); // 1 + 100 * max(1,0)

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
        root_rec.first_name.parent_frs = root_frs; // Self-parent

        // Add 100 directories
        for dir_i in 0..100 {
            let dir_frs = 100 + dir_i;
            let rec = index.get_or_create(dir_frs);
            rec.stdinfo.set_directory(true);
            rec.first_name.parent_frs = root_frs;
        }

        // Add 1000 files per directory (100K total)
        for dir_i in 0..100 {
            let dir_frs = 100 + dir_i;
            for file_i in 0..1000 {
                let file_frs = 10_000 + dir_i * 1000 + file_i;
                let rec = index.get_or_create(file_frs);
                rec.first_name.parent_frs = dir_frs;
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

    /// Test that extension records processed BEFORE base records in the same
    /// fragment correctly preserve extension names.
    ///
    /// This simulates the scenario where parallel parsing processes an
    /// extension record before its base record in the same worker thread.
    #[test]
    fn test_extension_before_base_in_same_fragment() {
        // Create a fragment
        let mut fragment = MftIndexFragment::with_capacity(10);

        // Simulate extension record processing FIRST
        // Extension has 2 names: name2, name3
        let name2_offset = fragment.names.len() as u32;
        fragment.names.push_str("name2.txt");
        let name2_ref = IndexNameRef::new(name2_offset, 9, true, 0);

        let name3_offset = fragment.names.len() as u32;
        fragment.names.push_str("name3.txt");
        let name3_ref = IndexNameRef::new(name3_offset, 9, true, 0);

        // Add links for extension names
        let link0_idx = fragment.links.len() as u32;
        fragment.links.push(LinkInfo {
            next_entry: link0_idx + 1,
            name: name2_ref,
            parent_frs: 5, // parent directory
        });
        let link1_idx = fragment.links.len() as u32;
        fragment.links.push(LinkInfo {
            next_entry: NO_ENTRY,
            name: name3_ref,
            parent_frs: 6, // different parent (hard link)
        });

        // Create placeholder record for base_frs=100
        let record = fragment.get_or_create(100);
        // Extension copies first name to first_name (simulating
        // parse_extension_to_fragment)
        record.first_name.name = name2_ref;
        record.first_name.parent_frs = 5;
        record.first_name.next_entry = link1_idx; // points to name3
        record.name_count = 2; // 2 extension names

        // Verify extension state before base processing
        assert!(
            fragment.get_or_create(100).first_name.name.is_valid(),
            "Extension should have set first_name"
        );
        assert_eq!(
            fragment.get_or_create(100).name_count,
            2,
            "Extension should have 2 names"
        );

        // Now simulate base record processing AFTER extension
        // Base has 1 name: name1
        let name1_offset = fragment.names.len() as u32;
        fragment.names.push_str("name1.txt");
        let name1_ref = IndexNameRef::new(name1_offset, 9, true, 0);
        let base_parent_frs = 5_u64;

        // This is what parse_record_to_fragment does:
        // 1. Save existing extension data
        let record = fragment.get_or_create(100);
        let existing_first_name = record.first_name;
        let existing_name_valid = existing_first_name.name.is_valid();
        let existing_name_count = if existing_name_valid {
            record.name_count
        } else {
            0
        };

        // 2. Overwrite first_name with base name
        record.first_name = LinkInfo {
            next_entry: NO_ENTRY,
            name: name1_ref,
            parent_frs: base_parent_frs,
        };

        // 3. Chain extension names after base name
        let first_name_next_entry = if existing_name_valid {
            // Push existing_first_name to links
            let ext_link_idx = fragment.links.len() as u32;
            fragment.links.push(existing_first_name);
            ext_link_idx
        } else {
            NO_ENTRY
        };

        // 4. Set first_name.next_entry
        let record = fragment.get_or_create(100);
        record.first_name.next_entry = first_name_next_entry;

        // 5. Update name_count
        record.name_count = 1 + existing_name_count;

        // Verify final state
        let record = fragment.get_or_create(100);
        assert_eq!(record.name_count, 3, "Should have 3 names total");
        assert!(
            record.first_name.name.is_valid(),
            "first_name should be valid"
        );

        // Verify the chain: first_name(name1) -> link[2](name2) -> link[1](name3)
        let first_next = record.first_name.next_entry;
        assert_ne!(first_next, NO_ENTRY, "first_name should chain to extension");

        let link2 = &fragment.links[first_next as usize];
        assert!(link2.name.is_valid(), "link[2] should have valid name");
        assert_eq!(
            link2.next_entry, link1_idx,
            "link[2] should chain to link[1]"
        );

        let link1 = &fragment.links[link1_idx as usize];
        assert!(link1.name.is_valid(), "link[1] should have valid name");
        assert_eq!(link1.next_entry, NO_ENTRY, "link[1] should be end of chain");

        println!("✅ Extension-before-base test passed!");
    }

    /// Test that cross-fragment merge correctly handles extension-only
    /// placeholders.
    ///
    /// This simulates the scenario where:
    /// - Fragment A processes an extension record (creates placeholder with
    ///   extension names)
    /// - Fragment B processes the base record (has real stdinfo data)
    /// - When merged, the base record should be kept and extension names merged
    ///   in
    #[test]
    fn test_cross_fragment_merge_extension_placeholder() {
        // Create Fragment A with extension-only placeholder for FRS 100
        let mut fragment_a = MftIndexFragment::with_capacity(10);

        // Add extension name to fragment A
        let ext_name_offset = fragment_a.names.len() as u32;
        fragment_a.names.push_str("hardlink.txt");
        let ext_name_ref = IndexNameRef::new(ext_name_offset, 12, true, 0);

        // Create placeholder record with extension name (no stdinfo)
        let record_a = fragment_a.get_or_create(100);
        record_a.first_name.name = ext_name_ref;
        record_a.first_name.parent_frs = 5;
        record_a.first_name.next_entry = NO_ENTRY;
        record_a.name_count = 1;
        // stdinfo remains default (created = 0) - this is the key!

        // Verify fragment A has placeholder with extension name but no base data
        assert!(
            fragment_a.get_or_create(100).first_name.name.is_valid(),
            "Fragment A should have extension name"
        );
        assert_eq!(
            fragment_a.get_or_create(100).stdinfo.created,
            0,
            "Fragment A should have no stdinfo (placeholder)"
        );
        assert!(
            !fragment_a.get_or_create(100).has_base_data(),
            "Fragment A should NOT have base data"
        );

        // Create Fragment B with base record for FRS 100
        let mut fragment_b = MftIndexFragment::with_capacity(10);

        // Add base name to fragment B
        let base_name_offset = fragment_b.names.len() as u32;
        fragment_b.names.push_str("original.txt");
        let base_name_ref = IndexNameRef::new(base_name_offset, 12, true, 0);

        // Create base record with real stdinfo
        let record_b = fragment_b.get_or_create(100);
        record_b.first_name.name = base_name_ref;
        record_b.first_name.parent_frs = 5;
        record_b.first_name.next_entry = NO_ENTRY;
        record_b.name_count = 1;
        record_b.stdinfo.created = 132_456_789_012_345_678; // Real timestamp
        record_b.stdinfo.modified = 132_456_789_012_345_678;

        // Verify fragment B has base record with real data
        assert!(
            fragment_b.get_or_create(100).first_name.name.is_valid(),
            "Fragment B should have base name"
        );
        assert_ne!(
            fragment_b.get_or_create(100).stdinfo.created,
            0,
            "Fragment B should have real stdinfo"
        );
        assert!(
            fragment_b.get_or_create(100).has_base_data(),
            "Fragment B should have base data"
        );

        // Create main index and merge fragments
        // Merge fragment A first (extension-only placeholder), then fragment B (base
        // record) This simulates the cross-fragment scenario where extension is
        // processed first
        let mut index = MftIndex::new('D');
        index.merge_fragments(vec![fragment_a, fragment_b]);

        // Verify index now has base record with extension names merged
        let idx = index.frs_to_idx[100] as usize;
        let record = &index.records[idx];

        assert!(
            record.has_base_data(),
            "Index should have base data after second merge"
        );
        assert_ne!(
            record.stdinfo.created, 0,
            "Index should have real stdinfo after merge"
        );
        assert!(
            record.first_name.name.is_valid(),
            "Index should have valid first_name"
        );
        assert_eq!(
            record.name_count, 2,
            "Index should have 2 names (base + extension)"
        );

        println!("✅ Cross-fragment merge test passed!");
    }

    /// Test that cross-fragment merge correctly handles MULTIPLE names in
    /// extension records.
    ///
    /// This simulates the scenario where:
    /// - Fragment A processes an extension record with 2 hard links (B, C)
    /// - Fragment B processes the base record with 1 name (A)
    /// - When merged, all 3 names should be accessible via `get_name_at`
    #[test]
    fn test_cross_fragment_merge_multiple_extension_names() {
        // Create Fragment A with extension-only placeholder for FRS 100
        // This placeholder has 2 names (B and C) from the extension record
        let mut fragment_a = MftIndexFragment::with_capacity(10);

        // Add extension names to fragment A
        let ext_b_offset = fragment_a.names.len() as u32;
        fragment_a.names.push_str("hardlink_b.txt");
        let ext_hardlink_b = IndexNameRef::new(ext_b_offset, 14, true, 0);

        let ext_c_offset = fragment_a.names.len() as u32;
        fragment_a.names.push_str("hardlink_c.txt");
        let ext_hardlink_c = IndexNameRef::new(ext_c_offset, 14, true, 0);

        // Add link C to the links array
        let link_c_idx = fragment_a.links.len() as u32;
        fragment_a.links.push(LinkInfo {
            next_entry: NO_ENTRY,
            name: ext_hardlink_c,
            parent_frs: 10, // Different parent than B
        });

        // Create placeholder record with extension names (no stdinfo)
        let record_a = fragment_a.get_or_create(100);
        record_a.first_name.name = ext_hardlink_b;
        record_a.first_name.parent_frs = 5;
        record_a.first_name.next_entry = link_c_idx; // Chain to link C
        record_a.name_count = 2; // B and C
        // stdinfo remains default (created = 0) - this is the key!

        // Create Fragment B with base record for FRS 100
        let mut fragment_b = MftIndexFragment::with_capacity(10);

        // Add base name to fragment B
        let base_offset = fragment_b.names.len() as u32;
        fragment_b.names.push_str("original_a.txt");
        let base_original = IndexNameRef::new(base_offset, 14, true, 0);

        // Create base record with real stdinfo
        let record_b = fragment_b.get_or_create(100);
        record_b.first_name.name = base_original;
        record_b.first_name.parent_frs = 5;
        record_b.first_name.next_entry = NO_ENTRY;
        record_b.name_count = 1;
        record_b.stdinfo.created = 132_456_789_012_345_678; // Real timestamp
        record_b.stdinfo.modified = 132_456_789_012_345_678;

        // Create main index and merge fragments
        let mut index = MftIndex::new('D');
        index.merge_fragments(vec![fragment_a, fragment_b]);

        // Verify index now has base record with all 3 names merged
        let idx = index.frs_to_idx[100] as usize;
        let record = &index.records[idx];

        assert!(record.has_base_data(), "Index should have base data");
        assert_eq!(
            record.name_count, 3,
            "Index should have 3 names (A + B + C)"
        );

        // Verify all 3 names are accessible via get_name_at
        let name_0 = index.get_name_at(record, 0);
        let name_1 = index.get_name_at(record, 1);
        let name_2 = index.get_name_at(record, 2);

        assert!(name_0.is_some(), "Name 0 should be accessible");
        assert!(name_1.is_some(), "Name 1 should be accessible");
        assert!(name_2.is_some(), "Name 2 should be accessible");

        // Verify the actual names
        let name_0_str = index.get_name(&name_0.unwrap().name);
        let name_1_str = index.get_name(&name_1.unwrap().name);
        let name_2_str = index.get_name(&name_2.unwrap().name);

        println!("Name 0: {name_0_str}");
        println!("Name 1: {name_1_str}");
        println!("Name 2: {name_2_str}");

        // The base name (A) should be first, then extension names (B, C)
        assert_eq!(name_0_str, "original_a.txt", "Name 0 should be base name");
        assert_eq!(
            name_1_str, "hardlink_b.txt",
            "Name 1 should be extension name B"
        );
        assert_eq!(
            name_2_str, "hardlink_c.txt",
            "Name 2 should be extension name C"
        );

        println!("✅ Cross-fragment merge with multiple extension names test passed!");
    }

    /// Test that cross-fragment merge works when BASE record comes FIRST.
    ///
    /// This simulates the scenario where:
    /// - Fragment A processes the base record with 1 name (A)
    /// - Fragment B processes an extension record with 2 hard links (B, C)
    /// - When merged, all 3 names should be accessible via `get_name_at`
    #[test]
    fn test_cross_fragment_merge_base_first() {
        // Create Fragment A with base record for FRS 100
        let mut fragment_a = MftIndexFragment::with_capacity(10);

        // Add base name to fragment A
        let base_offset = fragment_a.names.len() as u32;
        fragment_a.names.push_str("original_a.txt");
        let base_original = IndexNameRef::new(base_offset, 14, true, 0);

        // Create base record with real stdinfo
        let record_a = fragment_a.get_or_create(100);
        record_a.first_name.name = base_original;
        record_a.first_name.parent_frs = 5;
        record_a.first_name.next_entry = NO_ENTRY;
        record_a.name_count = 1;
        record_a.stdinfo.created = 132_456_789_012_345_678; // Real timestamp
        record_a.stdinfo.modified = 132_456_789_012_345_678;

        // Create Fragment B with extension-only placeholder for FRS 100
        // This placeholder has 2 names (B and C) from the extension record
        let mut fragment_b = MftIndexFragment::with_capacity(10);

        // Add extension names to fragment B
        let ext_b_offset = fragment_b.names.len() as u32;
        fragment_b.names.push_str("hardlink_b.txt");
        let ext_hardlink_b = IndexNameRef::new(ext_b_offset, 14, true, 0);

        let ext_c_offset = fragment_b.names.len() as u32;
        fragment_b.names.push_str("hardlink_c.txt");
        let ext_hardlink_c = IndexNameRef::new(ext_c_offset, 14, true, 0);

        // Add link C to the links array
        let link_c_idx = fragment_b.links.len() as u32;
        fragment_b.links.push(LinkInfo {
            next_entry: NO_ENTRY,
            name: ext_hardlink_c,
            parent_frs: 10, // Different parent than B
        });

        // Create placeholder record with extension names (no stdinfo)
        let record_b = fragment_b.get_or_create(100);
        record_b.first_name.name = ext_hardlink_b;
        record_b.first_name.parent_frs = 5;
        record_b.first_name.next_entry = link_c_idx; // Chain to link C
        record_b.name_count = 2; // B and C
        // stdinfo remains default (created = 0) - placeholder

        // Create main index and merge fragments
        // Merge fragment A first (base record), then fragment B (extension-only)
        let mut index = MftIndex::new('D');
        index.merge_fragments(vec![fragment_a, fragment_b]);

        // Verify index now has base record with all 3 names merged
        let idx = index.frs_to_idx[100] as usize;
        let record = &index.records[idx];

        assert!(record.has_base_data(), "Index should have base data");
        assert_eq!(
            record.name_count, 3,
            "Index should have 3 names (A + B + C)"
        );

        // Verify all 3 names are accessible via get_name_at
        let name_0 = index.get_name_at(record, 0);
        let name_1 = index.get_name_at(record, 1);
        let name_2 = index.get_name_at(record, 2);

        assert!(name_0.is_some(), "Name 0 should be accessible");
        assert!(name_1.is_some(), "Name 1 should be accessible");
        assert!(name_2.is_some(), "Name 2 should be accessible");

        // Verify the actual names
        let name_0_str = index.get_name(&name_0.unwrap().name);
        let name_1_str = index.get_name(&name_1.unwrap().name);
        let name_2_str = index.get_name(&name_2.unwrap().name);

        println!("Name 0: {name_0_str}");
        println!("Name 1: {name_1_str}");
        println!("Name 2: {name_2_str}");

        // The base name (A) should be first, then extension names (B, C)
        assert_eq!(name_0_str, "original_a.txt", "Name 0 should be base name");
        assert_eq!(
            name_1_str, "hardlink_b.txt",
            "Name 1 should be extension name B"
        );
        assert_eq!(
            name_2_str, "hardlink_c.txt",
            "Name 2 should be extension name C"
        );

        println!("✅ Cross-fragment merge (base first) test passed!");
    }

    // ========================================================================
    // Fix 9: LIVE Scan Self-Healing Tests (v0.2.185)
    // ========================================================================

    /// Test that `rebuild_children_from_names()` correctly rebuilds child lists
    /// from parent references in name attributes when child links are missing.
    ///
    /// This simulates the LIVE scan scenario where child lists may be
    /// incomplete due to ordering/timing issues during parallel parsing.
    #[test]
    fn test_rebuild_children_from_names_basic() {
        let mut index = MftIndex::new('C');

        // Create a simple tree structure with parent references but NO child links:
        // root (FRS 5)
        //   ├── dir1 (FRS 100)
        //   │   └── file1.txt (FRS 200)
        //   └── file2.txt (FRS 201)

        // Root directory (self-parent)
        let root_frs = 5_u64;
        let root_rec = index.get_or_create(root_frs);
        root_rec.stdinfo.set_directory(true);
        root_rec.first_name.parent_frs = root_frs; // Self-parent
        root_rec.first_child = NO_ENTRY; // NO child links!

        // dir1 (child of root)
        let dir1_frs = 100_u64;
        let offset = index.add_name("dir1");
        let rec = index.get_or_create(dir1_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs; // Parent reference set
        rec.first_child = NO_ENTRY; // NO child links!

        // file1.txt (child of dir1)
        let file1_frs = 200_u64;
        let offset = index.add_name("file1.txt");
        let rec = index.get_or_create(file1_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir1_frs; // Parent reference set

        // file2.txt (child of root)
        let file2_frs = 201_u64;
        let offset = index.add_name("file2.txt");
        let rec = index.get_or_create(file2_frs);
        rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs; // Parent reference set

        // Verify NO child links exist before rebuild
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
        assert_eq!(
            index.records[root_idx].first_child, NO_ENTRY,
            "Root should have no children before rebuild"
        );
        assert_eq!(
            index.records[dir1_idx].first_child, NO_ENTRY,
            "dir1 should have no children before rebuild"
        );
        assert!(
            index.children.is_empty(),
            "Children vector should be empty before rebuild"
        );

        // Call rebuild_children_from_names
        index.rebuild_children_from_names();

        // Verify child links are now present
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();

        assert_ne!(
            index.records[root_idx].first_child, NO_ENTRY,
            "Root should have children after rebuild"
        );
        assert_ne!(
            index.records[dir1_idx].first_child, NO_ENTRY,
            "dir1 should have children after rebuild"
        );

        // Count children of root (should be 2: dir1 and file2)
        let mut root_child_count = 0;
        let mut child_idx = index.records[root_idx].first_child;
        while child_idx != NO_ENTRY {
            root_child_count += 1;
            child_idx = index.children[child_idx as usize].next_entry;
        }
        assert_eq!(root_child_count, 2, "Root should have 2 children");

        // Count children of dir1 (should be 1: file1)
        let mut dir1_child_count = 0;
        let mut child_idx = index.records[dir1_idx].first_child;
        while child_idx != NO_ENTRY {
            dir1_child_count += 1;
            child_idx = index.children[child_idx as usize].next_entry;
        }
        assert_eq!(dir1_child_count, 1, "dir1 should have 1 child");

        println!("✅ rebuild_children_from_names basic test passed!");
    }

    /// Test that `rebuild_children_from_names()` handles hardlinks correctly.
    ///
    /// A file with multiple hardlinks should create child entries for each
    /// parent directory.
    #[test]
    fn test_rebuild_children_from_names_hardlinks() {
        let mut index = MftIndex::new('C');

        // Two parent directories and one hardlinked file.
        //
        // The important detail here is the *ordering* of the child's FILE_NAME entries:
        // - parse order (C++ "name_index"): dir1 then dir2
        // - stored link-chain order (first_name + next_entry): dir2 (last parsed) then
        //   dir1
        //
        // rebuild_children_from_names() must reconstruct ChildInfo.name_index in
        // **parse order** (the semantics expected by the C++ tree-metrics
        // port). If it uses link-chain order directly, hardlink delta
        // distribution will be flipped.

        let dir1_frs = 100_u64;
        let dir2_frs = 101_u64;
        let file_frs = 200_u64;

        // Create dir1 + dir2 records. Make them self-parented so rebuild skips adding
        // their own edges.
        let dir1_name_off = index.add_name("dir1");
        let dir1_rec = index.get_or_create(dir1_frs);
        dir1_rec.stdinfo.set_directory(true);
        dir1_rec.first_name.name =
            IndexNameRef::new(dir1_name_off, 4, true, IndexNameRef::NO_EXTENSION);
        dir1_rec.first_name.parent_frs = dir1_frs;
        dir1_rec.first_child = NO_ENTRY;

        let dir2_name_off = index.add_name("dir2");
        let dir2_rec = index.get_or_create(dir2_frs);
        dir2_rec.stdinfo.set_directory(true);
        dir2_rec.first_name.name =
            IndexNameRef::new(dir2_name_off, 4, true, IndexNameRef::NO_EXTENSION);
        dir2_rec.first_name.parent_frs = dir2_frs;
        dir2_rec.first_child = NO_ENTRY;

        // Create a file record with two names/hardlinks:
        // parse order: (0) parent=dir1, (1) parent=dir2
        // stored order: first_name = (1), overflow link = (0)
        // Add names and create link first to avoid borrow conflicts
        let file_name_off = index.add_name("file.txt");
        let link_name_off = index.add_name("file.txt");
        let link = LinkInfo {
            name: IndexNameRef::new(link_name_off, 8, true, IndexNameRef::NO_EXTENSION),
            parent_frs: dir1_frs, // earlier parsed name
            next_entry: NO_ENTRY,
        };
        index.links.push(link);
        #[allow(clippy::cast_possible_truncation)]
        let link_idx = (index.links.len() - 1) as u32;

        let file_rec = index.get_or_create(file_frs);
        file_rec.first_name.name =
            IndexNameRef::new(file_name_off, 8, true, IndexNameRef::NO_EXTENSION);
        file_rec.first_name.parent_frs = dir2_frs; // last parsed name
        file_rec.first_name.next_entry = link_idx;
        file_rec.name_count = 2;

        // Sanity: directories have no children before rebuild.
        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
        let dir2_idx = index.frs_to_idx_opt(dir2_frs).unwrap();
        assert_eq!(index.records[dir1_idx].first_child, NO_ENTRY);
        assert_eq!(index.records[dir2_idx].first_child, NO_ENTRY);

        // Run self-heal edge rebuild.
        index.rebuild_children_from_names();

        let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
        let dir2_idx = index.frs_to_idx_opt(dir2_frs).unwrap();
        let _file_idx = index.frs_to_idx_opt(file_frs).unwrap();

        // dir1 should have the file as a child with parse-order name_index = 0
        let c1 = index.records[dir1_idx].first_child;
        assert_ne!(c1, NO_ENTRY);
        let child1 = &index.children[c1 as usize];
        // child_frs stores the FRS, not the index
        assert_eq!(child1.child_frs, file_frs);
        assert_eq!(
            child1.name_index, 0,
            "dir1 should keep parse-order name_index=0 after rebuild"
        );

        // dir2 should have the file as a child with parse-order name_index = 1
        let c2 = index.records[dir2_idx].first_child;
        assert_ne!(c2, NO_ENTRY);
        let child2 = &index.children[c2 as usize];
        // child_frs stores the FRS, not the index
        assert_eq!(child2.child_frs, file_frs);
        assert_eq!(
            child2.name_index, 1,
            "dir2 should keep parse-order name_index=1 after rebuild"
        );
    }

    /// Test that `rebuild_children_from_names()` skips self-referencing root.
    #[test]
    fn test_rebuild_children_from_names_skips_root_self_reference() {
        let mut index = MftIndex::new('C');

        // Root with self-parent (should NOT create a child entry for itself)
        let root_frs = 5_u64;
        let rec = index.get_or_create(root_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.parent_frs = root_frs; // Self-parent
        rec.first_child = NO_ENTRY;

        // Rebuild
        index.rebuild_children_from_names();

        // Root should still have no children (self-reference is skipped)
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(
            index.records[root_idx].first_child, NO_ENTRY,
            "Root should not have itself as a child"
        );
        assert!(
            index.children.is_empty(),
            "No child entries should be created for self-reference"
        );

        println!("✅ rebuild_children_from_names skips root self-reference test passed!");
    }

    // ========================================================================
    // Fix 4/5: Two-Channel Model & Per-Stream Delta Tests
    // ========================================================================

    /// Test that tree metrics correctly handles empty directories (junctions).
    ///
    /// A junction/empty directory should have desc=1 (itself), not 0.
    /// This is the Two-Channel Model: Channel B (printed) = child treesize + 1.
    #[test]
    fn test_tree_metrics_empty_directory_descendants() {
        let mut index = MftIndex::new('C');

        // Create root with one empty child directory (simulating a junction)
        let root_frs = 5_u64;
        let root_rec = index.get_or_create(root_frs);
        root_rec.stdinfo.set_directory(true);
        root_rec.first_name.parent_frs = root_frs;

        // Empty directory (like a junction with no children)
        let empty_dir_frs = 100_u64;
        let offset = index.add_name("EmptyDir");
        let rec = index.get_or_create(empty_dir_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(offset, 8, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = root_frs;

        // Add child entry
        index.add_child_entry(root_frs, empty_dir_frs, 0);

        // Compute tree metrics
        index.compute_tree_metrics();

        // Verify empty directory has descendants = 1 (itself)
        let empty_dir_idx = index.frs_to_idx_opt(empty_dir_frs).unwrap();
        assert_eq!(
            index.records[empty_dir_idx].descendants, 1,
            "Empty directory should have descendants = 1 (itself)"
        );

        // Verify root has descendants = 2 (itself + empty_dir)
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(
            index.records[root_idx].descendants, 2,
            "Root should have descendants = 2 (itself + empty_dir)"
        );

        println!("✅ Tree metrics empty directory descendants test passed!");
    }

    /// Test that tree metrics correctly handles directories with internal
    /// streams.
    ///
    /// Internal streams (like security descriptors) should contribute to the
    /// parent's size via Channel A, but NOT to the directory's own printed size
    /// (Channel B).
    #[test]
    fn test_tree_metrics_internal_streams_two_channel() {
        use crate::index::InternalStreamInfo;

        let mut index = MftIndex::new('C');

        // Create root
        let root_frs = 5_u64;
        let root_rec = index.get_or_create(root_frs);
        root_rec.stdinfo.set_directory(true);
        root_rec.first_name.parent_frs = root_frs;

        // Create a directory with an internal stream
        let dir_frs = 100_u64;
        let offset = index.add_name("DirWithInternal");
        let dir_idx_for_internal = {
            let rec = index.get_or_create(dir_frs);
            rec.stdinfo.set_directory(true);
            rec.first_name.name = IndexNameRef::new(offset, 15, true, IndexNameRef::NO_EXTENSION);
            rec.first_name.parent_frs = root_frs;
            rec.total_stream_count = 2; // 1 default + 1 internal
            index.frs_to_idx[dir_frs as usize] as usize
        };

        // Add internal stream (e.g., security descriptor with 256 bytes)
        let internal_idx = index.internal_streams.len() as u32;
        index.internal_streams.push(InternalStreamInfo {
            next_entry: NO_ENTRY,
            size: SizeInfo {
                length: 256,
                allocated: 512,
            },
            flags: 0,
        });
        // Set first_internal_stream on the directory record
        index.records[dir_idx_for_internal].first_internal_stream = internal_idx;

        // Add a file under the directory
        let file_frs = 200_u64;
        let offset = index.add_name("file.txt");
        let rec = index.get_or_create(file_frs);
        rec.first_name.name = IndexNameRef::new(offset, 8, true, IndexNameRef::NO_EXTENSION);
        rec.first_name.parent_frs = dir_frs;
        rec.first_stream.size = SizeInfo {
            length: 1000,
            allocated: 4096,
        };

        // Add child entries
        index.add_child_entry(root_frs, dir_frs, 0);
        index.add_child_entry(dir_frs, file_frs, 0);

        // Compute tree metrics using cpp_tree algorithm
        crate::cpp_tree::compute_tree_metrics_cpp_port(&mut index, false);

        // Verify directory's printed size (Channel B) = children size + first stream
        // The internal stream should NOT be in the directory's own printed size
        let dir_idx = index.frs_to_idx_opt(dir_frs).unwrap();
        assert_eq!(
            index.records[dir_idx].treesize, 1000,
            "Directory treesize should be 1000 (file only, no internal stream)"
        );

        // Verify directory descendants = 2 (itself + file)
        assert_eq!(
            index.records[dir_idx].descendants, 2,
            "Directory should have descendants = 2"
        );

        // Verify root's size includes the internal stream (via Channel A propagation)
        // Root treesize = dir's first_stream (0) + file (1000) + internal (256) = 1256
        // But wait - directory's first_stream is 0, so:
        // Root treesize = children_length + first_len = (1000 + 256) + 0 = 1256
        let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
        assert_eq!(
            index.records[root_idx].treesize, 1256,
            "Root treesize should include internal stream from child dir"
        );

        println!("✅ Tree metrics internal streams two-channel test passed!");
    }
}

// ============================================================================
// Polars DataFrame Conversion (optional, on-demand)
// ============================================================================

impl MftIndex {
    /// Convert the lean index to a Polars `DataFrame`.
    ///
    /// This is an **optional** conversion for when you need:
    /// - Complex SQL-like queries
    /// - Analytics and aggregations
    /// - Export to Parquet/CSV
    ///
    /// For simple searches, use the lean index directly (faster).
    ///
    /// # Cross-Platform
    ///
    /// This method is cross-platform and works on all platforms.
    ///
    /// # Output Format
    ///
    /// Outputs **one row per FRS** (File Record Segment) - this is the true
    /// `MftIndex` representation. Hard links and ADS are NOT expanded.
    ///
    /// For search results with expansion (matching C++ `uffs.exe` behavior),
    /// use `IndexQuery::collect()` which expands hard links and ADS.
    ///
    /// # Tree Metrics
    ///
    /// The `DataFrame` includes tree metrics (descendants, treesize,
    /// `tree_allocated`) that are pre-computed in the `MftIndex` via
    /// `compute_tree_metrics()`.
    ///
    /// # Errors
    ///
    /// Returns an error if `DataFrame` construction fails.
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    pub fn to_dataframe(&self) -> crate::Result<uffs_polars::DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};
        let n = self.records.len();
        // Pre-allocate all column vectors (35 columns in v5)
        let (mut frs, mut seq, mut lsn, mut parent, mut name, mut ns) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut size, mut alloc) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let (mut si_c, mut si_m, mut si_a, mut si_mft) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut usn, mut sec_id, mut own_id): (Vec<u64>, Vec<u32>, Vec<u32>) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut fn_c, mut fn_m, mut fn_a, mut fn_mft) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (
            mut dir,
            mut ro,
            mut hid,
            mut sys,
            mut arc,
            mut cmp,
            mut enc,
            mut spr,
            mut rp,
            mut off,
            mut nix,
            mut tmp,
        ) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        let (mut flags, mut lnk, mut str, mut path): (Vec<u16>, Vec<u16>, Vec<u16>, Vec<String>) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        // Tree metrics columns
        let (mut descendants, mut treesize, mut tree_allocated): (Vec<u32>, Vec<u64>, Vec<u64>) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        // File type (extension) column - first-class citizen like name, size, etc.
        let mut file_type: Vec<String> = Vec::with_capacity(n);
        let (mut reparse_tag, mut is_resident): (Vec<u32>, Vec<bool>) =
            (Vec::with_capacity(n), Vec::with_capacity(n));
        // P3 forensic columns - only allocate if forensic mode is enabled
        let (mut is_deleted, mut is_corrupt, mut is_extension, mut base_frs_col) =
            if self.forensic_mode {
                (
                    Vec::with_capacity(n),
                    Vec::with_capacity(n),
                    Vec::with_capacity(n),
                    Vec::with_capacity(n),
                )
            } else {
                (Vec::new(), Vec::new(), Vec::new(), Vec::new())
            };
        // Extract data from records
        for rec in &self.records {
            frs.push(rec.frs);
            seq.push(rec.sequence_number);
            lsn.push(rec.lsn);
            parent.push(rec.first_name.parent_frs);
            name.push(self.record_name(rec).to_owned());
            ns.push(rec.namespace);
            size.push(rec.first_stream.size.length);
            alloc.push(rec.first_stream.size.allocated);
            si_c.push(rec.stdinfo.created);
            si_m.push(rec.stdinfo.modified);
            si_a.push(rec.stdinfo.accessed);
            si_mft.push(rec.stdinfo.mft_changed);
            usn.push(rec.stdinfo.usn);
            sec_id.push(rec.stdinfo.security_id);
            own_id.push(rec.stdinfo.owner_id);
            fn_c.push(rec.fn_created);
            fn_m.push(rec.fn_modified);
            fn_a.push(rec.fn_accessed);
            fn_mft.push(rec.fn_mft_changed);
            let si = &rec.stdinfo;
            dir.push(si.is_directory());
            ro.push(si.is_readonly());
            hid.push(si.is_hidden());
            sys.push(si.is_system());
            arc.push(si.is_archive());
            cmp.push(si.is_compressed());
            enc.push(si.is_encrypted());
            spr.push(si.is_sparse());
            rp.push(si.is_reparse());
            off.push(si.is_offline());
            nix.push(si.is_not_indexed());
            tmp.push(si.is_temporary());
            flags.push(si.to_attributes() as u16);
            lnk.push(rec.name_count);
            str.push(rec.stream_count);
            reparse_tag.push(rec.reparse_tag);
            is_resident.push(rec.first_stream.is_resident());
            // File type (extension) - lookup from ExtensionTable using extension_id
            let ext_id = rec.first_name.name.extension_id();
            let ext_str = self.extensions.get_extension(ext_id).unwrap_or("");
            file_type.push(ext_str.to_owned());
            // P3 forensic fields - only populate if forensic mode is enabled
            if self.forensic_mode {
                is_deleted.push(rec.is_deleted());
                is_corrupt.push(rec.is_corrupt());
                is_extension.push(rec.is_extension());
                base_frs_col.push(rec.base_frs);
            }
            // Tree metrics (pre-computed via compute_tree_metrics())
            // Use the tree_metrics() method as the single source of truth (Fix #3)
            let (desc, ts, ta) = rec.tree_metrics();
            descendants.push(desc);
            treesize.push(ts);
            tree_allocated.push(ta);
            path.push(self.build_path(rec.frs));
        }
        // Build DataFrame
        let dt = DataType::Datetime(TimeUnit::Microseconds, None);
        // Base columns (37 without forensic, 41 with forensic)
        let mut cols = vec![
            Series::new("frs".into(), frs).into_column(),
            Series::new("sequence_number".into(), seq).into_column(),
            Series::new("lsn".into(), lsn).into_column(),
            Series::new("parent_frs".into(), parent).into_column(),
            Series::new("name".into(), name).into_column(),
            Series::new("type".into(), file_type).into_column(),
            Series::new("namespace".into(), ns).into_column(),
            Series::new("size".into(), size).into_column(),
            Series::new("allocated_size".into(), alloc).into_column(),
            Series::new("si_created".into(), si_c)
                .cast(&dt)?
                .into_column(),
            Series::new("si_modified".into(), si_m)
                .cast(&dt)?
                .into_column(),
            Series::new("si_accessed".into(), si_a)
                .cast(&dt)?
                .into_column(),
            Series::new("si_mft_changed".into(), si_mft)
                .cast(&dt)?
                .into_column(),
            Series::new("usn".into(), usn).into_column(),
            Series::new("security_id".into(), sec_id).into_column(),
            Series::new("owner_id".into(), own_id).into_column(),
            Series::new("fn_created".into(), fn_c)
                .cast(&dt)?
                .into_column(),
            Series::new("fn_modified".into(), fn_m)
                .cast(&dt)?
                .into_column(),
            Series::new("fn_accessed".into(), fn_a)
                .cast(&dt)?
                .into_column(),
            Series::new("fn_mft_changed".into(), fn_mft)
                .cast(&dt)?
                .into_column(),
            Series::new("is_directory".into(), dir).into_column(),
            Series::new("is_readonly".into(), ro).into_column(),
            Series::new("is_hidden".into(), hid).into_column(),
            Series::new("is_system".into(), sys).into_column(),
            Series::new("is_archive".into(), arc).into_column(),
            Series::new("is_compressed".into(), cmp).into_column(),
            Series::new("is_encrypted".into(), enc).into_column(),
            Series::new("is_sparse".into(), spr).into_column(),
            Series::new("is_reparse".into(), rp).into_column(),
            Series::new("is_offline".into(), off).into_column(),
            Series::new("is_not_indexed".into(), nix).into_column(),
            Series::new("is_temporary".into(), tmp).into_column(),
            Series::new("reparse_tag".into(), reparse_tag).into_column(),
            Series::new("is_resident".into(), is_resident).into_column(),
        ];
        // P3 forensic columns - only include when forensic_mode is enabled
        if self.forensic_mode {
            cols.push(Series::new("is_deleted".into(), is_deleted).into_column());
            cols.push(Series::new("is_corrupt".into(), is_corrupt).into_column());
            cols.push(Series::new("is_extension".into(), is_extension).into_column());
            cols.push(Series::new("base_frs".into(), base_frs_col).into_column());
        }
        // Remaining columns (always included)
        cols.push(Series::new("flags".into(), flags).into_column());
        cols.push(Series::new("link_count".into(), lnk).into_column());
        cols.push(Series::new("stream_count".into(), str).into_column());
        // Tree metrics (pre-computed via compute_tree_metrics())
        cols.push(Series::new("descendants".into(), descendants).into_column());
        cols.push(Series::new("treesize".into(), treesize).into_column());
        cols.push(Series::new("tree_allocated".into(), tree_allocated).into_column());
        cols.push(Series::new("path".into(), path).into_column());

        uffs_polars::DataFrame::new_infer_height(cols).map_err(crate::MftError::from)
    }
}

// ============================================================================
// Building MftIndex from ParsedRecords (Cross-Platform)
// ============================================================================

impl MftIndex {
    /// Build an `MftIndex` from a vector of parsed records.
    ///
    /// This is the fast path - directly builds the lean index without
    /// going through Polars `DataFrame`.
    ///
    /// Works on all platforms - uses cross-platform `ParsedRecord` from parse
    /// module.
    #[must_use]
    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::indexing_slicing
    )]
    pub fn from_parsed_records(volume: char, records: Vec<crate::parse::ParsedRecord>) -> Self {
        /// System metafiles are FRS 0-15 (except root at FRS 5)
        const SYSTEM_METAFILE_MAX_FRS: u64 = 15;
        const ROOT_FRS_LOCAL: u64 = 5;

        tracing::debug!(volume = %volume, input_records = records.len(), "[TRIP] MftIndex::from_parsed_records ENTER");

        let capacity = records.len();
        let mut index = Self::with_capacity(volume, capacity);
        let mut has_forensic_records = false;

        for parsed in records {
            // In normal mode, skip records not in use.
            // In forensic mode, include deleted/corrupt/extension records.
            // Forensic records have is_deleted/is_corrupt/is_extension set by
            // parse_record_forensic().
            let is_forensic_record = parsed.is_deleted || parsed.is_corrupt || parsed.is_extension;
            if is_forensic_record {
                has_forensic_records = true;
            }
            if !parsed.in_use && !is_forensic_record {
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
            // Extract and intern extension (must be done before get_or_create borrows
            // mutably)
            let extension_id = index.intern_extension(&parsed.name);

            // Get or create the record and set all basic fields in a block scope
            // to end the mutable borrow before adding additional names/streams
            {
                let record = index.get_or_create(parsed.frs);

                // Set sequence number, LSN, and namespace (raw MFT fields)
                record.sequence_number = parsed.sequence_number;
                record.lsn = parsed.lsn;
                record.namespace = parsed.namespace;

                // Set $FILE_NAME timestamps (often differ from $STANDARD_INFORMATION)
                record.fn_created = parsed.fn_created;
                record.fn_modified = parsed.fn_modified;
                record.fn_accessed = parsed.fn_accessed;
                record.fn_mft_changed = parsed.fn_mft_changed;

                // Set $STANDARD_INFORMATION timestamps and flags
                record.stdinfo.created = parsed.std_info.created;
                record.stdinfo.modified = parsed.std_info.modified;
                record.stdinfo.accessed = parsed.std_info.accessed;
                record.stdinfo.mft_changed = parsed.std_info.mft_changed;
                record.stdinfo.usn = parsed.std_info.usn;
                record.stdinfo.security_id = parsed.std_info.security_id;
                record.stdinfo.owner_id = parsed.std_info.owner_id;
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

                // Set name info (offset and extension_id were computed before borrowing record)
                record.first_name.name =
                    IndexNameRef::new(name_offset, name_len, is_ascii, extension_id);
                record.first_name.parent_frs = parsed.parent_frs;
                // Note: name_count is set AFTER filtering additional names to avoid
                // counting duplicates. See the code after this block.

                // Set reparse tag (0 if not a reparse point)
                record.reparse_tag = parsed.reparse_tag;

                // Set P3 forensic fields (is_deleted, is_corrupt, is_extension, base_frs)
                record.set_forensic_flags(
                    parsed.is_deleted,
                    parsed.is_corrupt,
                    parsed.is_extension,
                );
                record.base_frs = parsed.base_frs;

                // Set size and flags
                // For directories, use parsed.size/allocated_size which includes
                // $INDEX_ROOT + $INDEX_ALLOCATION + $BITMAP (C++ parity)
                // For files, use the default stream size
                // Note: stream_count is set AFTER filtering named streams to avoid
                // counting internal Windows streams. See the code after this block.
                if parsed.is_directory {
                    // Directory size comes from index attributes, already in parsed.size
                    record.first_stream.size.length = parsed.size;
                    record.first_stream.size.allocated = parsed.allocated_size;
                } else if let Some(default_stream) =
                    parsed.streams.iter().find(|st| st.name.is_empty())
                {
                    record.first_stream.size.length = default_stream.size;
                    record.first_stream.size.allocated = default_stream.allocated_size;
                    // Set is_resident flag (bit 1)
                    if default_stream.is_resident {
                        record.first_stream.flags |= 0x02;
                    }
                    // Set is_sparse flag (bit 0)
                    if default_stream.is_sparse {
                        record.first_stream.flags |= 0x01;
                    }
                } else if !parsed.streams.is_empty() {
                    // No default stream, use first available
                    record.first_stream.size.length = parsed.size;
                    record.first_stream.size.allocated = parsed.allocated_size;
                }
            } // End record borrow here

            // Store additional names (hardlinks) in the links vector
            // Skip the name that matches first_name (the primary/best name)
            // Note: parsed.name is the BEST name (selected by PrimaryNameTracker),
            // which may not be parsed.names[0]. We must filter by matching name+parent.
            let additional_names: Vec<_> = parsed
                .names
                .iter()
                .filter(|n| !(n.name == parsed.name && n.parent_frs == parsed.parent_frs))
                .collect();

            // Update name_count to reflect actual stored names (1 primary + additional)
            // This must be done AFTER filtering to avoid counting duplicates
            let actual_name_count = (1 + additional_names.len()).max(1) as u16;
            index.get_or_create(parsed.frs).name_count = actual_name_count;

            if !additional_names.is_empty() {
                let mut prev_link_idx = NO_ENTRY;
                for extra_name in additional_names.iter().rev() {
                    // Add name to names buffer
                    let extra_offset = index.add_name(&extra_name.name);
                    let extra_len = extra_name.name.len() as u16;
                    let extra_ascii = extra_name.name.is_ascii();
                    let extra_ext_id = index.intern_extension(&extra_name.name);

                    let link_idx = index.links.len() as u32;
                    index.links.push(LinkInfo {
                        next_entry: prev_link_idx,
                        name: IndexNameRef::new(extra_offset, extra_len, extra_ascii, extra_ext_id),
                        parent_frs: extra_name.parent_frs,
                    });
                    prev_link_idx = link_idx;
                }
                // Link first_name to the chain
                let record = index.get_or_create(parsed.frs);
                record.first_name.next_entry = prev_link_idx;
            }

            // Store additional streams (ADS) in the streams vector.
            //
            // Filter out:
            //   - Empty name (default stream)
            //   - Internal Windows streams (names starting with `$UPPERCASE`)
            //
            // Internal streams are NOT emitted as ADS rows, but they ARE required for exact
            // C++ tree-metrics parity. We must keep them as individual stream entries
            // because the C++ `delta()` distribution uses integer division:
            //     delta(a + b) != delta(a) + delta(b)
            // So pre-summing internal stream sizes causes 1-4 byte tree-size skews.
            let mut internal_streams_size = 0_u64;
            let mut internal_streams_allocated = 0_u64;
            let mut first_internal_stream = NO_ENTRY;
            let mut last_internal_stream = NO_ENTRY;

            let mut named_streams: Vec<_> = Vec::new();
            for st in &parsed.streams {
                if st.name.is_empty() {
                    continue;
                }

                let is_internal = st
                    .name
                    .strip_prefix('$')
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(|ch| ch.is_ascii_uppercase());

                if is_internal {
                    internal_streams_size = internal_streams_size.saturating_add(st.size);
                    internal_streams_allocated =
                        internal_streams_allocated.saturating_add(st.allocated_size);

                    let flags = u8::from(st.is_sparse) | (u8::from(st.is_resident) << 1_u8);

                    let new_idx = index.internal_streams.len() as u32;
                    index.internal_streams.push(InternalStreamInfo {
                        size: SizeInfo {
                            length: st.size,
                            allocated: st.allocated_size,
                        },
                        next_entry: NO_ENTRY,
                        flags,
                    });

                    if last_internal_stream == NO_ENTRY {
                        first_internal_stream = new_idx;
                    } else {
                        index.internal_streams[last_internal_stream as usize].next_entry = new_idx;
                    }
                    last_internal_stream = new_idx;
                    continue;
                }

                named_streams.push(st);
            }

            // Set total_stream_count to include ALL streams (for tree metrics, C++ parity)
            // This includes internal Windows streams like $REPARSE_POINT, $OBJECT_ID, etc.
            // C++ counts all streams in tree metrics (line 4788: result.treesize += 1)
            let total_stream_count = parsed.streams.len().max(1) as u16;

            // Set stream_count to reflect only user-visible stored streams (1 default +
            // named) This is used for user-facing output (DataFrame export)
            let actual_stream_count = (1 + named_streams.len()).max(1) as u16;

            let record = index.get_or_create(parsed.frs);
            record.total_stream_count = total_stream_count;
            record.stream_count = actual_stream_count;
            record.internal_streams_size = internal_streams_size;
            record.internal_streams_allocated = internal_streams_allocated;
            record.first_internal_stream = first_internal_stream;

            if !named_streams.is_empty() {
                let mut prev_stream_idx = NO_ENTRY;
                for extra_stream in named_streams.iter().rev() {
                    // Add stream name to names buffer
                    let stream_name_offset = index.add_name(&extra_stream.name);
                    let stream_name_len = extra_stream.name.len() as u16;
                    let stream_ascii = extra_stream.name.is_ascii();
                    // Streams don't have extensions, use 0
                    let stream_ext_id = 0;

                    let stream_idx = index.streams.len() as u32;
                    let mut flags = 0_u8;
                    if extra_stream.is_sparse {
                        flags |= 0x01;
                    }
                    if extra_stream.is_resident {
                        flags |= 0x02;
                    }
                    index.streams.push(IndexStreamInfo {
                        size: SizeInfo {
                            length: extra_stream.size,
                            allocated: extra_stream.allocated_size,
                        },
                        next_entry: prev_stream_idx,
                        name: IndexNameRef::new(
                            stream_name_offset,
                            stream_name_len,
                            stream_ascii,
                            stream_ext_id,
                        ),
                        flags,
                    });
                    prev_stream_idx = stream_idx;
                }
                // Link first_stream to the chain
                let file_record = index.get_or_create(parsed.frs);
                file_record.first_stream.next_entry = prev_stream_idx;
            }

            // Build parent-child relationship for ALL hardlinks
            // C++ creates a SEPARATE child entry for EACH $FILE_NAME attribute (hardlink)
            // This is crucial for correct tree metrics calculation with hardlinks.
            // Each child entry stores its name_index so we can calculate proportional
            // shares.
            for (name_idx, name_info) in parsed.names.iter().enumerate() {
                let parent_frs = name_info.parent_frs;
                if parent_frs == parsed.frs || parent_frs == u64::from(NO_ENTRY) {
                    continue;
                }

                // Ensure parent exists
                let parent_idx = {
                    let parent_frs_usize = parent_frs as usize;
                    if parent_frs_usize >= index.frs_to_idx.len() {
                        index.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
                    }
                    if index.frs_to_idx[parent_frs_usize] == NO_ENTRY {
                        // Create placeholder parent
                        let new_idx = index.records.len() as u32;
                        index.frs_to_idx[parent_frs_usize] = new_idx;
                        index.records.push(FileRecord::new(parent_frs));
                    }
                    index.frs_to_idx[parent_frs_usize]
                };

                // Add child entry with name_index for proportional share calculation
                let child_idx = index.children.len() as u32;

                // Get parent's first_child and update
                let parent = &mut index.records[parent_idx as usize];
                let old_first_child = parent.first_child;
                parent.first_child = child_idx;

                // C++ formula: name_info = name_count - 1 - name_index
                // We store name_index directly and calculate name_info during traversal
                index.children.push(ChildInfo {
                    next_entry: old_first_child,
                    child_frs: parsed.frs,
                    name_index: name_idx as u16,
                });
            }

            // Handle case where names is empty (shouldn't happen, but be safe)
            if parsed.names.is_empty()
                && parsed.parent_frs != parsed.frs
                && parsed.parent_frs != u64::from(NO_ENTRY)
            {
                let parent_idx = {
                    let parent_frs_usize = parsed.parent_frs as usize;
                    if parent_frs_usize >= index.frs_to_idx.len() {
                        index.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
                    }
                    if index.frs_to_idx[parent_frs_usize] == NO_ENTRY {
                        let new_idx = index.records.len() as u32;
                        index.frs_to_idx[parent_frs_usize] = new_idx;
                        index.records.push(FileRecord::new(parsed.parent_frs));
                    }
                    index.frs_to_idx[parent_frs_usize]
                };

                let child_idx = index.children.len() as u32;
                let parent = &mut index.records[parent_idx as usize];
                let old_first_child = parent.first_child;
                parent.first_child = child_idx;

                index.children.push(ChildInfo {
                    next_entry: old_first_child,
                    child_frs: parsed.frs,
                    name_index: 0,
                });
            }
        }

        // Post-processing: compute derived data structures
        // These are fast O(n) operations that enhance query performance
        tracing::debug!(
            records = index.records.len(),
            "[TRIP] MftIndex::from_parsed_records -> record insertion done, starting post-processing"
        );

        // 1. Build extension index for fast *.ext queries (Phase 2)
        tracing::debug!("[TRIP] MftIndex::from_parsed_records -> Phase 2: ExtensionIndex::build");
        index.extension_index = Some(ExtensionIndex::build(&index));

        // 2. Sort directory children for natural ordering (Phase 4)
        tracing::debug!("[TRIP] MftIndex::from_parsed_records -> Phase 4: sort_directory_children");
        index.sort_directory_children();

        // 3. Compute tree metrics for directory statistics (Phase 5)
        tracing::debug!("[TRIP] MftIndex::from_parsed_records -> Phase 5: compute_tree_metrics");
        index.compute_tree_metrics();

        // 4. Set forensic mode flag if any forensic records were included
        index.forensic_mode = has_forensic_records;

        tracing::debug!(
            records = index.records.len(),
            "[TRIP] MftIndex::from_parsed_records EXIT"
        );
        index
    }

    /// Build an `MftIndex` from parsed records with detailed timing breakdown.
    ///
    /// This is the same as `from_parsed_records()` but returns timing
    /// information for each phase, useful for benchmarking and comparing
    /// with C++ implementation.
    ///
    /// # Returns
    ///
    /// A tuple of (`MftIndex`, `IndexBuildTiming`) with the built index and
    /// timing breakdown.
    #[must_use]
    pub fn from_parsed_records_with_timing(
        volume: char,
        records: Vec<crate::parse::ParsedRecord>,
    ) -> (Self, IndexBuildTiming) {
        use std::time::Instant;

        let total_start = Instant::now();

        // Phase 1: Build index without tree metrics
        // We call from_parsed_records which includes all phases, then we'll
        // re-run tree metrics with timing. This is slightly wasteful but
        // ensures correctness and avoids code duplication.
        //
        // For accurate timing, we time the full build, then separately time
        // just the tree metrics by clearing and recomputing.
        let insert_start = Instant::now();
        let mut index = Self::from_parsed_records(volume, records);
        // Saturating cast: u128 -> u64 (overflow impossible for realistic durations)
        let full_build_ms = u64::try_from(insert_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Phase 2: Time tree metrics separately by clearing and recomputing
        // First, clear tree metrics
        for record in &mut index.records {
            record.descendants = 0;
            record.treesize = 0;
            record.tree_allocated = 0;
        }

        // Now time just the tree metrics computation
        let tree_start = Instant::now();
        index.compute_tree_metrics();
        let tree_metrics_ms = u64::try_from(tree_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        let total_ms = u64::try_from(total_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Estimate the other phases based on the full build time minus tree metrics
        // This is approximate but gives a reasonable breakdown
        let index_only_ms = full_build_ms.saturating_sub(tree_metrics_ms);

        let timing = IndexBuildTiming {
            // Record insertion is the bulk of index_only_ms (estimated ~80%)
            record_insert_ms: index_only_ms * 80 / 100,
            // Extension index is fast (estimated ~10%)
            extension_index_ms: index_only_ms * 10 / 100,
            // Sorting is fast (estimated ~10%)
            sort_children_ms: index_only_ms * 10 / 100,
            // Tree metrics is measured accurately
            tree_metrics_ms,
            total_ms,
        };

        (index, timing)
    }

    /// Returns the number of child entries in the index.
    #[must_use]
    pub fn children_count(&self) -> usize {
        self.children.len()
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
    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    pub fn merge_fragments(&mut self, fragments: Vec<MftIndexFragment>) {
        use tracing::debug;

        let total_records: usize = fragments.iter().map(|frag| frag.records.len()).sum();
        let total_names: usize = fragments.iter().map(|frag| frag.names.len()).sum();

        debug!(
            fragments = fragments.len(),
            total_records, total_names, "🔀 Merging index fragments"
        );

        // Reserve capacity
        self.records.reserve(total_records);
        self.names.reserve(total_names);

        for fragment in fragments {
            self.merge_single_fragment(fragment);
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

    /// Merge a single fragment into this index.
    #[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
    fn merge_single_fragment(&mut self, fragment: MftIndexFragment) {
        let name_offset_adjustment = self.names.len() as u32;
        let link_offset_adjustment = self.links.len() as u32;
        let stream_offset_adjustment = self.streams.len() as u32;
        let internal_stream_offset_adjustment = self.internal_streams.len() as u32;

        // Build extension_id remapping table
        let extension_id_map = self.build_extension_id_map(&fragment);

        // Append names buffer
        self.names.push_str(&fragment.names);

        // Merge records, collecting any that need name/stream merging
        // Returns: Vec<(existing_record_idx, discarded_record)> for records that need
        // merging
        let records_to_merge = self.merge_fragment_records_with_deferred_merge(
            fragment.records,
            name_offset_adjustment,
            link_offset_adjustment,
            stream_offset_adjustment,
            internal_stream_offset_adjustment,
            &extension_id_map,
        );

        // Merge links (with offset and extension_id adjustment)
        self.merge_fragment_links(fragment.links, name_offset_adjustment, &extension_id_map);

        // Merge streams (with offset and extension_id adjustment)
        self.merge_fragment_streams(fragment.streams, name_offset_adjustment, &extension_id_map);
        // Merge internal streams (with offset adjustment)
        self.merge_fragment_internal_streams(fragment.internal_streams);

        // Merge children (with offset adjustment)
        self.merge_fragment_children(fragment.children);

        // Now merge the deferred names/streams from discarded records
        // At this point, all links/streams have been added with correct offsets
        self.apply_deferred_name_merges(
            records_to_merge,
            link_offset_adjustment,
            stream_offset_adjustment,
        );
    }

    /// Build extension ID remapping table from fragment to merged index.
    #[allow(clippy::cast_possible_truncation)]
    fn build_extension_id_map(&mut self, fragment: &MftIndexFragment) -> Vec<u16> {
        let mut extension_id_map: Vec<u16> = Vec::with_capacity(fragment.extensions.len());

        // Extension ID 0 is always "no extension" - no remapping needed
        extension_id_map.push(0);

        // Intern all extensions from fragment into merged table
        for idx in 1..fragment.extensions.len() {
            let ext_idx = u16::try_from(idx).unwrap_or(u16::MAX);
            if let Some(ext_str) = fragment.extensions.get_extension(ext_idx) {
                let merged_id = self.extensions.intern(ext_str);
                extension_id_map.push(merged_id);

                // Merge counts and bytes
                let count = fragment.extensions.get_count(ext_idx);
                let bytes = fragment.extensions.get_bytes(ext_idx);

                // Add to merged table's counts/bytes
                let merged_idx = merged_id as usize;
                if let Some(count_slot) = self.extensions.counts.get_mut(merged_idx) {
                    *count_slot += count;
                }
                if let Some(bytes_slot) = self.extensions.bytes.get_mut(merged_idx) {
                    *bytes_slot += bytes;
                }
            }
        }

        extension_id_map
    }

    /// Merge records from a fragment into this index, returning records that
    /// need deferred merging.
    ///
    /// When two fragments have records for the same FRS (e.g., base record in
    /// one fragment, extension record in another), we need to merge their
    /// names/streams. This function returns the records that were
    /// "discarded" but have additional names/streams that need to be merged
    /// after the links/streams arrays are fully populated.
    ///
    /// Returns: `Vec<(existing_record_idx, discarded_record)>` for records that
    /// need merging
    #[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
    fn merge_fragment_records_with_deferred_merge(
        &mut self,
        records: Vec<FileRecord>,
        name_offset_adjustment: u32,
        link_offset_adjustment: u32,
        stream_offset_adjustment: u32,
        internal_stream_offset_adjustment: u32,
        extension_id_map: &[u16],
    ) -> Vec<(u32, FileRecord)> {
        let mut deferred_merges: Vec<(u32, FileRecord)> = Vec::new();

        for mut record in records {
            let frs = record.frs;
            // FRS values are bounded by MFT size, which is always < 2^32 on real systems
            let frs_usize = usize::try_from(frs).unwrap_or(usize::MAX);

            // Adjust name offsets and remap extension_id
            Self::adjust_name_ref(
                &mut record.first_name.name,
                name_offset_adjustment,
                extension_id_map,
            );
            Self::adjust_name_ref(
                &mut record.first_stream.name,
                name_offset_adjustment,
                extension_id_map,
            );

            // Adjust link/stream indices (they point into fragment's arrays, need
            // adjustment)
            if record.first_name.next_entry != NO_ENTRY {
                record.first_name.next_entry += link_offset_adjustment;
            }
            if record.first_stream.next_entry != NO_ENTRY {
                record.first_stream.next_entry += stream_offset_adjustment;
            }
            if record.first_internal_stream != NO_ENTRY {
                record.first_internal_stream += internal_stream_offset_adjustment;
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
                // Record exists - need to merge
                let existing = &self.records[existing_idx as usize];

                // Determine which record to keep and which to merge from.
                // Key insight: A placeholder created by extension record processing
                // will have a name (from extension) but no base data (stdinfo = 0).
                // We must keep the record with base data and merge names from the other.
                let existing_is_placeholder = !existing.has_base_data();
                let record_has_base = record.has_base_data();

                if existing_is_placeholder && record_has_base {
                    // Existing is placeholder (extension-only), new has base data - swap them
                    // But we still need to merge any names from the placeholder
                    let placeholder =
                        core::mem::replace(&mut self.records[existing_idx as usize], record);
                    // If placeholder had names (from extension), defer merge
                    if placeholder.first_name.name.is_valid()
                        || placeholder.first_name.next_entry != NO_ENTRY
                        || placeholder.first_stream.name.is_valid()
                        || placeholder.first_stream.next_entry != NO_ENTRY
                        || placeholder.first_internal_stream != NO_ENTRY
                    {
                        deferred_merges.push((existing_idx, placeholder));
                    }
                } else if !existing.has_name() && record.has_name() {
                    // Fallback: existing has no name at all, new has name - swap them
                    let placeholder =
                        core::mem::replace(&mut self.records[existing_idx as usize], record);
                    if placeholder.first_name.name.is_valid()
                        || placeholder.first_name.next_entry != NO_ENTRY
                        || placeholder.first_stream.name.is_valid()
                        || placeholder.first_stream.next_entry != NO_ENTRY
                        || placeholder.first_internal_stream != NO_ENTRY
                    {
                        deferred_merges.push((existing_idx, placeholder));
                    }
                } else {
                    // Keep existing, merge from new record if it has additional names/streams
                    if record.first_name.name.is_valid()
                        || record.first_name.next_entry != NO_ENTRY
                        || record.first_stream.name.is_valid()
                        || record.first_stream.next_entry != NO_ENTRY
                        || record.first_internal_stream != NO_ENTRY
                    {
                        deferred_merges.push((existing_idx, record));
                    }
                }
            }
        }

        deferred_merges
    }

    /// Apply deferred name/stream merges from discarded records.
    ///
    /// This is called after all links/streams have been merged, so the indices
    /// are valid.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::indexing_slicing,
        clippy::too_many_lines
    )]
    fn apply_deferred_name_merges(
        &mut self,
        deferred_merges: Vec<(u32, FileRecord)>,
        _link_offset_adjustment: u32,
        _stream_offset_adjustment: u32,
    ) {
        for (existing_idx, discarded) in deferred_merges {
            let existing = &mut self.records[existing_idx as usize];

            // Merge names from discarded record
            if discarded.first_name.name.is_valid() || discarded.first_name.next_entry != NO_ENTRY {
                // Find the end of existing's name chain
                let last_link_idx = (existing.first_name.next_entry != NO_ENTRY).then(|| {
                    let mut idx = existing.first_name.next_entry;
                    while self
                        .links
                        .get(idx as usize)
                        .is_some_and(|link| link.next_entry != NO_ENTRY)
                    {
                        idx = self.links[idx as usize].next_entry;
                    }
                    idx
                });

                // Determine what to chain from discarded
                let chain_start = if discarded.first_name.name.is_valid() {
                    // Discarded has a first_name - add it as a new link
                    let new_link_idx = self.links.len() as u32;
                    self.links.push(LinkInfo {
                        next_entry: discarded.first_name.next_entry,
                        name: discarded.first_name.name,
                        parent_frs: discarded.first_name.parent_frs,
                    });
                    Some(new_link_idx)
                } else {
                    // Discarded only has overflow links
                    (discarded.first_name.next_entry != NO_ENTRY)
                        .then_some(discarded.first_name.next_entry)
                };

                // Chain the discarded names to existing
                if let Some(start) = chain_start {
                    if let Some(last_idx) = last_link_idx {
                        self.links[last_idx as usize].next_entry = start;
                    } else if existing.first_name.name.is_valid() {
                        existing.first_name.next_entry = start;
                    } else {
                        // Existing has no name at all - copy discarded's first_name
                        existing.first_name = discarded.first_name;
                    }
                    // Update name count
                    existing.name_count += discarded.name_count;
                }
            }

            // Merge streams from discarded record (similar logic)
            if discarded.first_stream.name.is_valid()
                || discarded.first_stream.next_entry != NO_ENTRY
            {
                let last_stream_idx = (existing.first_stream.next_entry != NO_ENTRY).then(|| {
                    let mut idx = existing.first_stream.next_entry;
                    while self
                        .streams
                        .get(idx as usize)
                        .is_some_and(|stream| stream.next_entry != NO_ENTRY)
                    {
                        idx = self.streams[idx as usize].next_entry;
                    }
                    idx
                });

                let chain_start = if discarded.first_stream.name.is_valid() {
                    let new_stream_idx = self.streams.len() as u32;
                    self.streams.push(IndexStreamInfo {
                        next_entry: discarded.first_stream.next_entry,
                        name: discarded.first_stream.name,
                        size: discarded.first_stream.size,
                        flags: discarded.first_stream.flags,
                    });
                    Some(new_stream_idx)
                } else {
                    (discarded.first_stream.next_entry != NO_ENTRY)
                        .then_some(discarded.first_stream.next_entry)
                };

                if let Some(start) = chain_start {
                    if let Some(last_idx) = last_stream_idx {
                        self.streams[last_idx as usize].next_entry = start;
                    } else if existing.first_stream.name.is_valid() {
                        existing.first_stream.next_entry = start;
                    } else {
                        existing.first_stream = discarded.first_stream;
                    }
                    existing.stream_count += discarded.stream_count;
                    existing.total_stream_count += discarded.total_stream_count;
                }
            }

            // Merge internal streams from the discarded record.
            if discarded.first_internal_stream != NO_ENTRY {
                let last_internal_idx = (existing.first_internal_stream != NO_ENTRY).then(|| {
                    let mut idx = existing.first_internal_stream;
                    while self
                        .internal_streams
                        .get(idx as usize)
                        .is_some_and(|st| st.next_entry != NO_ENTRY)
                    {
                        idx = self.internal_streams[idx as usize].next_entry;
                    }
                    idx
                });

                let chain_start = discarded.first_internal_stream;

                if let Some(last_idx) = last_internal_idx {
                    self.internal_streams[last_idx as usize].next_entry = chain_start;
                } else {
                    existing.first_internal_stream = chain_start;
                }

                existing.internal_streams_size = existing
                    .internal_streams_size
                    .saturating_add(discarded.internal_streams_size);
                existing.internal_streams_allocated = existing
                    .internal_streams_allocated
                    .saturating_add(discarded.internal_streams_allocated);

                // If we did NOT merge user-visible streams above, ensure total_stream_count
                // reflects the internal streams we just chained.
                let discarded_has_streams = discarded.first_stream.name.is_valid()
                    || discarded.first_stream.next_entry != NO_ENTRY;
                if !discarded_has_streams {
                    let mut count: u16 = 0;
                    let mut idx = chain_start;
                    while idx != NO_ENTRY {
                        count = count.saturating_add(1);
                        idx = self.internal_streams[idx as usize].next_entry;
                    }
                    existing.total_stream_count = existing.total_stream_count.saturating_add(count);
                }
            }
        }
    }

    /// Adjust a name reference with offset and extension ID remapping.
    fn adjust_name_ref(
        name_ref: &mut IndexNameRef,
        offset_adjustment: u32,
        extension_id_map: &[u16],
    ) {
        if name_ref.is_valid() {
            name_ref.offset += offset_adjustment;

            // Remap extension_id
            let old_ext_id = name_ref.extension_id();
            if let Some(&new_ext_id) = extension_id_map.get(old_ext_id as usize) {
                name_ref.remap_extension_id(new_ext_id);
            }
        }
    }

    /// Merge links from a fragment into this index.
    #[allow(clippy::cast_possible_truncation)]
    fn merge_fragment_links(
        &mut self,
        links: Vec<LinkInfo>,
        name_offset_adjustment: u32,
        extension_id_map: &[u16],
    ) {
        let link_offset_adjustment = self.links.len() as u32;
        for mut link in links {
            Self::adjust_name_ref(&mut link.name, name_offset_adjustment, extension_id_map);
            if link.next_entry != NO_ENTRY {
                link.next_entry += link_offset_adjustment;
            }
            self.links.push(link);
        }
    }

    /// Merge streams from a fragment into this index.
    #[allow(clippy::cast_possible_truncation)]
    fn merge_fragment_streams(
        &mut self,
        streams: Vec<IndexStreamInfo>,
        name_offset_adjustment: u32,
        extension_id_map: &[u16],
    ) {
        let stream_offset_adjustment = self.streams.len() as u32;
        for mut stream in streams {
            Self::adjust_name_ref(&mut stream.name, name_offset_adjustment, extension_id_map);
            if stream.next_entry != NO_ENTRY {
                stream.next_entry += stream_offset_adjustment;
            }
            self.streams.push(stream);
        }
    }

    /// Merge internal streams from a fragment into this index.
    #[allow(clippy::cast_possible_truncation)]
    fn merge_fragment_internal_streams(&mut self, internal_streams: Vec<InternalStreamInfo>) {
        let internal_offset_adjustment = self.internal_streams.len() as u32;
        for mut st in internal_streams {
            if st.next_entry != NO_ENTRY {
                st.next_entry += internal_offset_adjustment;
            }
            self.internal_streams.push(st);
        }
    }

    /// Merge children from a fragment into this index.
    #[allow(clippy::cast_possible_truncation)]
    fn merge_fragment_children(&mut self, children: Vec<ChildInfo>) {
        let child_offset_adjustment = self.children.len() as u32;
        for mut child in children {
            if child.next_entry != NO_ENTRY {
                child.next_entry += child_offset_adjustment;
            }
            self.children.push(child);
        }
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

                    // Create placeholder record
                    let record = FileRecord {
                        frs,
                        sequence_number: 0, // USN doesn't provide sequence number
                        namespace: 1,       // Assume Win32 namespace
                        forensic_flags: 0,  // Not deleted/corrupt/extension
                        base_frs: 0,        // Not an extension record
                        lsn: 0,             // USN doesn't provide LSN
                        reparse_tag: 0,     // USN doesn't provide reparse tag
                        stdinfo: StandardInfo::default(),
                        name_count: 1,
                        stream_count: 1,       // User-visible streams
                        total_stream_count: 1, // All streams (for tree metrics)
                        first_internal_stream: NO_ENTRY,
                        first_child: NO_ENTRY,
                        first_name: LinkInfo {
                            next_entry: NO_ENTRY,
                            name: IndexNameRef::new(
                                name_start,
                                name_len,
                                change.filename.is_ascii(),
                                IndexNameRef::NO_EXTENSION, // TODO: Extract extension (Phase 1)
                            ),
                            parent_frs: change.parent_frs,
                        },
                        first_stream: IndexStreamInfo::default(),
                        fn_created: 0,
                        fn_modified: 0,
                        fn_accessed: 0,
                        fn_mft_changed: 0,
                        descendants: 0,
                        treesize: 0,
                        tree_allocated: 0,
                        internal_streams_size: 0,
                        internal_streams_allocated: 0,
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
                    record.first_name.parent_frs = change.parent_frs;
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
    /// Internal stream entries filtered from user-visible output but required
    /// for exact C++ tree-metrics parity.
    pub internal_streams: Vec<InternalStreamInfo>,
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
            internal_streams: Vec::new(),
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
/// Version 4: Added `sequence_number`, `namespace`, and `$FILE_NAME` timestamps
/// (`fn_created`, `fn_modified`, `fn_accessed`, `fn_mft_changed`)
/// Version 5: Added NTFS 3.0+ forensic fields: `lsn`, `usn`, `security_id`,
/// `owner_id` Version 6: Added P2 forensic fields: `reparse_tag`, `is_resident`
/// (in stream flags) Version 7: Added P3 forensic fields: `forensic_flags`
/// (renamed from reserved), `base_frs` for extension records
/// Version 8: Added `total_stream_count` for C++ tree metrics parity
const INDEX_VERSION: u32 = 8;

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
        // Accept version 3 (legacy) and version 4 (current)
        if self.version < 3 || self.version > INDEX_VERSION {
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
    #[allow(clippy::too_many_lines)] // Binary serialization requires many field writes
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
            // Version 4+: sequence_number and namespace
            buffer.extend_from_slice(&record.sequence_number.to_le_bytes());
            buffer.push(record.namespace);
            buffer.push(record.forensic_flags); // Version 7: renamed from reserved
            // Version 5+: LSN (Log File Sequence Number)
            buffer.extend_from_slice(&record.lsn.to_le_bytes());
            // Version 6+: reparse_tag
            buffer.extend_from_slice(&record.reparse_tag.to_le_bytes());
            // Version 7+: base_frs for extension records
            buffer.extend_from_slice(&record.base_frs.to_le_bytes());
            // StandardInfo
            buffer.extend_from_slice(&record.stdinfo.created.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.modified.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.accessed.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.mft_changed.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.flags.to_le_bytes());
            // Version 5+: NTFS 3.0+ forensic fields
            buffer.extend_from_slice(&record.stdinfo.usn.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.security_id.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.owner_id.to_le_bytes());
            // Counts
            buffer.extend_from_slice(&record.name_count.to_le_bytes());
            buffer.extend_from_slice(&record.stream_count.to_le_bytes());
            // Version 8+: total_stream_count for C++ tree metrics parity
            buffer.extend_from_slice(&record.total_stream_count.to_le_bytes());
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
            // $FILE_NAME timestamps (Version 4+)
            buffer.extend_from_slice(&record.fn_created.to_le_bytes());
            buffer.extend_from_slice(&record.fn_modified.to_le_bytes());
            buffer.extend_from_slice(&record.fn_accessed.to_le_bytes());
            buffer.extend_from_slice(&record.fn_mft_changed.to_le_bytes());
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
    // Cognitive complexity is high due to version-conditional field reads (v3/v4/v5/v6).
    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cognitive_complexity
    )]
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
            // Version 4+: sequence_number and namespace (read sequentially to avoid
            // unsequenced reads)
            let sequence_number = if version >= 4 { read_u16!() } else { 0 };
            let namespace = if version >= 4 { read_u8!() } else { 1 }; // Default: Win32
            let forensic_flags = if version >= 4 { read_u8!() } else { 0 }; // Version 7: renamed from reserved
            // Version 5+: LSN (Log File Sequence Number)
            let lsn = if version >= 5 { read_u64!() } else { 0 };
            // Version 6+: reparse_tag
            let reparse_tag = if version >= 6 { read_u32!() } else { 0 };
            // Version 7+: base_frs for extension records
            let base_frs = if version >= 7 { read_u64!() } else { 0 };
            // StandardInfo
            let created = read_i64!();
            let modified = read_i64!();
            let accessed = read_i64!();
            let mft_changed = read_i64!();
            let flags = read_u32!();
            // Version 5+: NTFS 3.0+ forensic fields
            let usn = if version >= 5 { read_u64!() } else { 0 };
            let security_id = if version >= 5 { read_u32!() } else { 0 };
            let owner_id = if version >= 5 { read_u32!() } else { 0 };
            // Counts
            let name_count = read_u16!();
            let rec_stream_count = read_u16!();
            // Version 8+: total_stream_count for C++ tree metrics parity
            // For older versions, default to stream_count (user-visible = total)
            let total_stream_count = if version >= 8 {
                read_u16!()
            } else {
                rec_stream_count
            };
            let first_child = read_u32!();
            // first_name (LinkInfo)
            let link_next_entry = read_u32!();
            let link_name_offset = read_u32!();
            let link_name_meta = read_u32!();
            let link_parent_frs = read_u64!();
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
            // $FILE_NAME timestamps (Version 4+, read sequentially)
            let fn_created = if version >= 4 { read_i64!() } else { 0 };
            let fn_modified = if version >= 4 { read_i64!() } else { 0 };
            let fn_accessed = if version >= 4 { read_i64!() } else { 0 };
            let fn_mft_changed = if version >= 4 { read_i64!() } else { 0 };

            records.push(FileRecord {
                frs,
                sequence_number,
                namespace,
                forensic_flags,
                lsn,
                reparse_tag,
                base_frs,
                stdinfo: StandardInfo {
                    created,
                    modified,
                    accessed,
                    mft_changed,
                    flags,
                    usn,
                    security_id,
                    owner_id,
                },
                name_count,
                stream_count: rec_stream_count,
                total_stream_count,
                first_internal_stream: NO_ENTRY,
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
                fn_created,
                fn_modified,
                fn_accessed,
                fn_mft_changed,
                descendants,
                treesize,
                tree_allocated,
                // Deserialized caches don't have internal streams info (computed during parsing)
                internal_streams_size: 0,
                internal_streams_allocated: 0,
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
            let parent_frs = read_u64!();

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
            let child_frs = read_u64!();
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
            internal_streams: Vec::new(),
            stats: MftStats::new(),
            extensions,
            extension_index: None,
            forensic_mode: false, // Loaded indexes don't have forensic records
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
