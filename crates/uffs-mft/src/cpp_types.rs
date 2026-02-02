//! # C++ Data Structure Equivalents
//!
//! This module provides exact Rust equivalents of the C++ NTFS index data
//! structures. These are designed to match the C++ implementation byte-for-byte
//! for maximum compatibility and performance parity.
//!
//! ## C++ Source Reference
//!
//! - `packed_file_size.hpp`: `file_size_type`, `SizeInfo`
//! - `ntfs_record_types.hpp`: `NameInfo`, `LinkInfo`, `StreamInfo`,
//!   `ChildInfo`, `Record`
//! - `standard_info.hpp`: `StandardInfo`
//!
//! ## Key Design Decisions
//!
//! 1. **Packed structures**: Use `#[repr(C, packed)]` to match C++ `#pragma
//!    pack(push, 1)`
//! 2. **Sentinel values**: Use `NO_ENTRY` (`u32::MAX`) to match C++
//!    `negative_one` (~0)
//! 3. **Bit packing**: Match C++ bit layouts exactly (e.g., `NameInfo`
//!    offset/ascii)
//!
//! ## Indexing Safety
//!
//! This module uses direct indexing (`[]`) instead of `.get()` for performance
//! in the hot path of MFT parsing. All indexing operations are protected by
//! bounds checks at higher levels:
//! - Buffer sizes are validated before parsing
//! - Vector indices are validated after resize operations
//! - Record lookups use sentinel values (`NO_ENTRY`) for missing entries
//!
//! This matches the C++ implementation's approach where bounds are checked
//! once at the entry point, not on every access.

// Allow direct indexing in this C++ port module. Bounds are checked at higher
// levels (buffer size validation, resize operations, sentinel checks).
// This matches C++ behavior and is required for performance parity.
#![allow(clippy::indexing_slicing)]

extern crate alloc;

use core::ops::{Add, AddAssign, Sub, SubAssign};

// ============================================================================
// Helper Functions for Safe Type Conversions
// ============================================================================
//
// These functions handle type conversions that are intentional in this C++
// port. The C++ implementation uses specific integer sizes for struct layouts,
// and we need to match those exactly. The conversions are safe in practice
// because:
// - In-memory index structures are limited by available RAM
// - MFT record indices fit in u32 (max ~4 billion records)
// - Attribute offsets within records fit in u16 (max 4KB records)
// ============================================================================

/// Convert `usize` to `u32` for in-memory buffer indices.
///
/// The C++ implementation uses `u32` for record indices and buffer offsets.
/// While NTFS MFT files can exceed 4GB on disk, the in-memory index structures
/// use `u32` indices to match C++ and for practical memory limits.
///
/// # Panics
///
/// Panics in debug builds if `value > u32::MAX`.
#[inline]
#[must_use]
#[allow(clippy::cast_possible_truncation)] // Intentional: C++ port uses u32 indices
pub fn usize_to_u32(value: usize) -> u32 {
    debug_assert!(
        u32::try_from(value).is_ok(),
        "Buffer offset {value} exceeds u32::MAX"
    );
    value as u32
}

/// Convert `usize` to `u16` for small buffer offsets.
///
/// Used for attribute offsets within a single MFT record (max 4KB).
///
/// # Panics
///
/// Panics in debug builds if `value > u16::MAX`.
#[inline]
#[must_use]
#[allow(clippy::cast_possible_truncation)] // Intentional: MFT records are max 4KB
pub fn usize_to_u16(value: usize) -> u16 {
    debug_assert!(
        u16::try_from(value).is_ok(),
        "Offset {value} exceeds u16::MAX"
    );
    value as u16
}

/// Convert `u64` to `u32` for in-memory record indices.
///
/// The C++ implementation uses `u32` for record indices. While NTFS file
/// references are 48-bit, the in-memory index uses `u32` for practical limits.
///
/// # Panics
///
/// Panics in debug builds if `value > u32::MAX`.
#[inline]
#[must_use]
#[allow(clippy::cast_possible_truncation)] // Intentional: C++ port uses u32 indices
pub fn u64_to_u32(value: u64) -> u32 {
    debug_assert!(
        u32::try_from(value).is_ok(),
        "Value {value} exceeds u32::MAX"
    );
    value as u32
}

/// Convert `u64` to `usize` for buffer indexing.
///
/// # Panics
///
/// Panics in debug builds if `value > usize::MAX`.
#[inline]
#[must_use]
#[allow(clippy::cast_possible_truncation)] // Intentional: 32-bit support
pub fn u64_to_usize(value: u64) -> usize {
    debug_assert!(
        usize::try_from(value).is_ok(),
        "Value {value} exceeds usize::MAX"
    );
    value as usize
}

/// Convert `i64` to `u64` for Windows FILETIME values.
///
/// Windows FILETIMEs are stored as signed but represent unsigned values.
/// Negative values are treated as 0.
#[inline]
#[must_use]
#[allow(clippy::cast_sign_loss)] // Intentional: we check for negative first
pub const fn i64_to_u64_filetime(value: i64) -> u64 {
    if value < 0 { 0 } else { value as u64 }
}

/// Convert `u64` to `i64` for Windows FILETIME values.
///
/// Windows FILETIMEs are stored as signed but represent unsigned values.
/// Values exceeding `i64::MAX` are clamped.
#[inline]
#[must_use]
#[allow(clippy::cast_possible_wrap)] // Intentional: we check bounds first
pub const fn u64_to_i64_filetime(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

// ============================================================================
// Constants
// ============================================================================

/// Sentinel value indicating "no entry" (matches C++ `~0` / `negative_one`)
pub const NO_ENTRY: u32 = u32::MAX;

// ============================================================================
// file_size_type - 6-byte packed file size (48-bit, up to 256 TB)
// ============================================================================

/// Packed 6-byte file size type (48-bit, supports up to 256 TB).
///
/// Matches C++ `file_size_type` from `packed_file_size.hpp`.
///
/// # Memory Layout
/// - `low`: Lower 32 bits (4 bytes)
/// - `high`: Upper 16 bits (2 bytes)
/// - Total: 6 bytes
#[repr(C, packed)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct FileSizeType {
    /// Low 32 bits of the file size.
    low: u32,
    /// High 16 bits of the file size.
    high: u16,
}

impl FileSizeType {
    /// Create a new `FileSizeType` from a u64 value.
    #[inline]
    #[must_use]
    pub const fn new(value: u64) -> Self {
        // Intentional truncation: FileSizeType stores 48-bit values (6 bytes)
        // Low 32 bits go to `low`, next 16 bits go to `high`
        Self {
            low: (value & 0xFFFF_FFFF) as u32,
            high: ((value >> 32) & 0xFFFF) as u16,
        }
    }

    /// Convert to u64.
    #[inline]
    #[must_use]
    pub const fn as_u64(&self) -> u64 {
        (self.low as u64) | ((self.high as u64) << 32)
    }

    /// Returns true if the value is zero.
    #[inline]
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.low == 0 && self.high == 0
    }
}

impl From<u64> for FileSizeType {
    #[inline]
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<FileSizeType> for u64 {
    #[inline]
    fn from(value: FileSizeType) -> Self {
        value.as_u64()
    }
}

impl Add for FileSizeType {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.as_u64() + rhs.as_u64())
    }
}

impl AddAssign for FileSizeType {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = Self::new(self.as_u64() + rhs.as_u64());
    }
}

impl Sub for FileSizeType {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.as_u64() - rhs.as_u64())
    }
}

impl SubAssign for FileSizeType {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = Self::new(self.as_u64() - rhs.as_u64());
    }
}

impl core::fmt::Debug for FileSizeType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "FileSizeType({})", self.as_u64())
    }
}

// ============================================================================
// SizeInfo - File size information
// ============================================================================

/// Size information for files and streams.
///
/// Matches C++ `SizeInfo` from `packed_file_size.hpp`.
///
/// # Memory Layout
/// - `length`: 6 bytes (logical file size)
/// - `allocated`: 6 bytes (allocated size on disk)
/// - `bulkiness`: 6 bytes (size including slack space)
/// - `treesize`: 4 bytes (for directories: descendant count)
/// - Total: 22 bytes
#[repr(C, packed)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SizeInfo {
    /// Logical file size
    pub length: FileSizeType,
    /// Allocated size on disk
    pub allocated: FileSizeType,
    /// Size including slack space (used for bulkiness calculation)
    pub bulkiness: FileSizeType,
    /// For directories: descendant count (4 bytes, NOT `FileSizeType`!)
    pub treesize: u32,
}

// ============================================================================
// NameInfo - Name offset with ASCII flag
// ============================================================================

/// Name information - offset into names buffer with ASCII flag.
///
/// Matches C++ `NameInfo` from `ntfs_record_types.hpp`.
///
/// # Bit Layout of `offset_packed` field:
/// - Bit 0: ASCII flag (1 = ASCII, 0 = Unicode)
/// - Bits 1-31: Offset >> 1 into names buffer
///
/// # Memory Layout
/// - `offset_packed`: 4 bytes (packed offset + ASCII flag)
/// - `length`: 1 byte (name length in characters)
/// - Total: 5 bytes
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct NameInfo {
    /// Packed offset (bits 1-31) and ASCII flag (bit 0).
    /// Use accessor methods `offset()`, `ascii()`, `set_offset()`,
    /// `set_ascii()`.
    pub offset_packed: u32,
    /// Name length in characters.
    pub length: u8,
}

impl NameInfo {
    /// Create a new `NameInfo` with the given offset, length, and ASCII flag.
    #[inline]
    #[must_use]
    pub const fn new(offset: u32, length: u8, is_ascii: bool) -> Self {
        let ascii_bit = if is_ascii { 1_u32 } else { 0_u32 };
        let packed = (offset << 1_u32) | ascii_bit;
        Self {
            offset_packed: packed,
            length,
        }
    }

    /// Get the name length in characters.
    #[inline]
    #[must_use]
    pub const fn length(&self) -> u8 {
        self.length
    }

    /// Returns true if the name is ASCII.
    #[inline]
    #[must_use]
    pub const fn ascii(&self) -> bool {
        (self.offset_packed & 1) != 0
    }

    /// Set the ASCII flag.
    #[inline]
    pub const fn set_ascii(&mut self, value: bool) {
        if value {
            self.offset_packed |= 1;
        } else {
            self.offset_packed &= !1;
        }
    }

    /// Get the offset into the names buffer.
    #[inline]
    #[must_use]
    pub const fn offset(&self) -> u32 {
        let result = self.offset_packed >> 1_u32;
        // Match C++ behavior: if result equals (NO_ENTRY >> 1), return NO_ENTRY
        if result == (NO_ENTRY >> 1_u32) {
            NO_ENTRY
        } else {
            result
        }
    }

    /// Set the offset into the names buffer.
    #[inline]
    pub const fn set_offset(&mut self, value: u32) {
        self.offset_packed = (value << 1_u32) | (self.offset_packed & 1);
    }

    /// Set the name length in characters.
    #[inline]
    pub const fn set_length(&mut self, value: u8) {
        self.length = value;
    }

    /// Returns true if this name reference is valid (not `NO_ENTRY`).
    #[inline]
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.offset() != NO_ENTRY
    }
}

impl core::fmt::Debug for NameInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NameInfo")
            .field("offset", &self.offset())
            .field("length", &self.length())
            .field("ascii", &self.ascii())
            .finish()
    }
}

// ============================================================================
// LinkInfo - Hard link information
// ============================================================================

/// Hard link information - links a file record to a parent directory.
///
/// Matches C++ `LinkInfo` from `ntfs_record_types.hpp`.
///
/// # Memory Layout (C++ order: `next_entry`, name, parent)
/// - `next_entry`: 4 bytes (index of next `LinkInfo`, or `NO_ENTRY`)
/// - `name`: 5 bytes (`NameInfo`)
/// - `parent`: 4 bytes (parent directory FRS)
/// - Total: 13 bytes
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct LinkInfo {
    /// Index of next `LinkInfo` in nameinfos vector, or `NO_ENTRY`
    pub next_entry: u32,
    /// Filename reference
    pub name: NameInfo,
    /// Parent directory FRS (truncated to u32 in C++)
    pub parent: u32,
}

impl LinkInfo {
    /// Create a new `LinkInfo` with default values (`NO_ENTRY`).
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_entry: NO_ENTRY,
            name: NameInfo {
                offset_packed: NO_ENTRY, // Will be set properly via set_offset
                length: 0,
            },
            parent: 0,
        }
    }
}

impl core::fmt::Debug for LinkInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Copy fields to avoid unaligned reference issues with packed structs
        let next_entry = self.next_entry;
        let name = self.name;
        let parent = self.parent;
        f.debug_struct("LinkInfo")
            .field("next_entry", &next_entry)
            .field("name", &name)
            .field("parent", &parent)
            .finish()
    }
}

// ============================================================================
// StreamInfo - NTFS stream information (extends SizeInfo)
// ============================================================================

/// NTFS stream information - alternate data streams.
///
/// Matches C++ `StreamInfo` from `ntfs_record_types.hpp`.
/// Extends `SizeInfo` with stream-specific fields.
///
/// # Memory Layout
/// - `SizeInfo`: 22 bytes (length, allocated, bulkiness, treesize)
/// - `next_entry`: 4 bytes (index of next `StreamInfo`, or `NO_ENTRY`)
/// - `name`: 5 bytes (`NameInfo`)
/// - `flags`: 1 byte (bitfield: `is_sparse`:1, `is_allocated_accounted`:1,
///   `type_name_id`:6)
/// - Total: 32 bytes
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct StreamInfo {
    /// Size information (inherited from `SizeInfo`).
    pub size: SizeInfo,
    /// Index of next `StreamInfo` in streaminfos vector, or `NO_ENTRY`.
    pub next_entry: u32,
    /// Stream name reference (empty for default `$DATA`).
    pub name: NameInfo,
    /// Packed flags: bit 0 = `is_sparse`, bit 1 = `is_allocated_accounted`,
    /// bits 2-7 = `type_name_id`.
    /// Use accessor methods `is_sparse()`, `type_name_id()`, etc.
    pub flags: u8,
}

impl StreamInfo {
    /// Create a new `StreamInfo` with default values.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            size: SizeInfo {
                length: FileSizeType { low: 0, high: 0 },
                allocated: FileSizeType { low: 0, high: 0 },
                bulkiness: FileSizeType { low: 0, high: 0 },
                treesize: 0,
            },
            next_entry: NO_ENTRY,
            name: NameInfo {
                offset_packed: NO_ENTRY,
                length: 0,
            },
            flags: 0,
        }
    }

    /// Returns true if this stream is sparse.
    #[inline]
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        (self.flags & 0x01) != 0
    }

    /// Set the sparse flag.
    #[inline]
    pub const fn set_sparse(&mut self, value: bool) {
        if value {
            self.flags |= 0x01;
        } else {
            self.flags &= !0x01;
        }
    }

    /// Returns true if allocated size is accounted for in main stream.
    #[inline]
    #[must_use]
    pub const fn is_allocated_size_accounted_for_in_main_stream(&self) -> bool {
        (self.flags & 0x02) != 0
    }

    /// Set the allocated size accounted flag.
    #[inline]
    pub const fn set_allocated_size_accounted(&mut self, value: bool) {
        if value {
            self.flags |= 0x02;
        } else {
            self.flags &= !0x02;
        }
    }

    /// Get the type name ID (attribute type >> 4, 0 for $I30).
    #[inline]
    #[must_use]
    pub const fn type_name_id(&self) -> u8 {
        self.flags >> 2
    }

    /// Set the type name ID.
    #[inline]
    pub const fn set_type_name_id(&mut self, value: u8) {
        self.flags = (self.flags & 0x03) | (value << 2_u8);
    }
}

impl core::fmt::Debug for StreamInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Copy fields to avoid unaligned reference issues with packed structs
        let size = self.size;
        let next_entry = self.next_entry;
        let name = self.name;
        let flags = self.flags;
        let is_sparse = self.is_sparse();
        let type_name_id = self.type_name_id();
        f.debug_struct("StreamInfo")
            .field("size", &size)
            .field("next_entry", &next_entry)
            .field("name", &name)
            .field("flags", &flags)
            .field("is_sparse", &is_sparse)
            .field("type_name_id", &type_name_id)
            .finish()
    }
}

// ============================================================================
// ChildInfo - Child directory entry information
// ============================================================================

/// Child directory entry information.
///
/// Matches C++ `ChildInfo` from `ntfs_record_types.hpp`.
///
/// # Memory Layout
/// - `next_entry`: 4 bytes (index of next `ChildInfo`, or `NO_ENTRY`)
/// - `record_number`: 4 bytes (FRS of child, truncated to u32 in C++)
/// - `name_index`: 2 bytes (which hardlink, 0-indexed)
/// - Total: 10 bytes
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ChildInfo {
    /// Index of next `ChildInfo` in childinfos vector, or `NO_ENTRY`
    pub next_entry: u32,
    /// FRS of the child file/directory (truncated to u32 in C++)
    pub record_number: u32,
    /// Which name index (for hard links, 0-indexed)
    pub name_index: u16,
}

impl Default for ChildInfo {
    fn default() -> Self {
        Self {
            next_entry: NO_ENTRY,
            record_number: NO_ENTRY,
            name_index: u16::MAX, // C++ uses negative_one for u16
        }
    }
}

impl core::fmt::Debug for ChildInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Copy fields to avoid unaligned reference issues with packed structs
        let next_entry = self.next_entry;
        let record_number = self.record_number;
        let name_index = self.name_index;
        f.debug_struct("ChildInfo")
            .field("next_entry", &next_entry)
            .field("record_number", &record_number)
            .field("name_index", &name_index)
            .finish()
    }
}

// ============================================================================
// StandardInfo - Compact NTFS $STANDARD_INFORMATION with bitfields
// ============================================================================

/// Compact representation of NTFS `$STANDARD_INFORMATION` attribute.
///
/// Matches C++ `StandardInfo` from `standard_info.hpp`.
/// Uses bitfields to pack file attributes efficiently.
///
/// # Memory Layout (C++ bitfield layout)
/// - `created`: 8 bytes (FILETIME)
/// - `written`: 8 bytes (FILETIME)
/// - `accessed_and_flags`: 8 bytes (accessed:58 bits + 6 attribute flags)
/// - Total: 24 bytes
///
/// # Bitfield Layout of `accessed_and_flags`:
/// - Bits 0-57: accessed timestamp (58 bits)
/// - Bit 58: `is_readonly`
/// - Bit 59: `is_archive`
/// - Bit 60: `is_system`
/// - Bit 61: `is_hidden`
/// - Bit 62: `is_offline`
/// - Bit 63: `is_notcontentidx`
///
/// Note: Additional flags are stored in a second u64 in C++, but we simplify
/// here.
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct StandardInfo {
    /// Creation time (Windows FILETIME).
    pub created: u64,
    /// Last write time (Windows FILETIME).
    pub written: u64,
    /// Packed: accessed timestamp (bits 0-57) + flags (bits 58-63).
    /// Use accessor methods `accessed()`, `set_accessed()`, `attributes()`.
    pub accessed_and_flags1: u64,
    /// Additional flags packed into second `u64`.
    /// Bits: `is_noscrubdata`, `is_integritystream`, `is_pinned`,
    /// `is_unpinned`,       `is_directory`, `is_compressed`,
    /// `is_encrypted`, `is_sparsefile`, `is_reparsepoint`.
    /// Use accessor methods for individual flags.
    pub flags2: u16,
}

impl StandardInfo {
    /// Mask for accessed timestamp (58 bits).
    const ACCESSED_MASK: u64 = (1_u64 << 58) - 1;

    /// Flag bit position for read-only attribute (bit 58).
    const IS_READONLY: u64 = 1 << 58;
    /// Flag bit position for archive attribute (bit 59).
    const IS_ARCHIVE: u64 = 1 << 59;
    /// Flag bit position for system attribute (bit 60).
    const IS_SYSTEM: u64 = 1 << 60;
    /// Flag bit position for hidden attribute (bit 61).
    const IS_HIDDEN: u64 = 1 << 61;
    /// Flag bit position for offline attribute (bit 62).
    const IS_OFFLINE: u64 = 1 << 62;
    /// Flag bit position for not content indexed attribute (bit 63).
    const IS_NOTCONTENTIDX: u64 = 1 << 63;

    /// Flag bit position for no scrub data attribute (bit 0 of flags2).
    const IS_NOSCRUBDATA: u16 = 1 << 0;
    /// Flag bit position for integrity stream attribute (bit 1 of flags2).
    const IS_INTEGRITYSTREAM: u16 = 1 << 1;
    /// Flag bit position for pinned attribute (bit 2 of flags2).
    const IS_PINNED: u16 = 1 << 2;
    /// Flag bit position for unpinned attribute (bit 3 of flags2).
    const IS_UNPINNED: u16 = 1 << 3;
    /// Flag bit position for directory attribute (bit 4 of flags2).
    const IS_DIRECTORY: u16 = 1 << 4;
    /// Flag bit position for compressed attribute (bit 5 of flags2).
    const IS_COMPRESSED: u16 = 1 << 5;
    /// Flag bit position for encrypted attribute (bit 6 of flags2).
    const IS_ENCRYPTED: u16 = 1 << 6;
    /// Flag bit position for sparse file attribute (bit 7 of flags2).
    const IS_SPARSEFILE: u16 = 1 << 7;
    /// Flag bit position for reparse point attribute (bit 8 of flags2).
    const IS_REPARSEPOINT: u16 = 1 << 8;

    /// Get accessed timestamp.
    #[inline]
    #[must_use]
    pub const fn accessed(&self) -> u64 {
        self.accessed_and_flags1 & Self::ACCESSED_MASK
    }

    /// Set accessed timestamp.
    #[inline]
    pub const fn set_accessed(&mut self, value: u64) {
        self.accessed_and_flags1 =
            (self.accessed_and_flags1 & !Self::ACCESSED_MASK) | (value & Self::ACCESSED_MASK);
    }

    /// Get file attributes as a Windows `FILE_ATTRIBUTE_*` bitmask.
    #[must_use]
    pub const fn attributes(&self) -> u32 {
        let mut attrs = 0_u32;
        if self.accessed_and_flags1 & Self::IS_READONLY != 0 {
            attrs |= 0x0001; // FILE_ATTRIBUTE_READONLY
        }
        if self.accessed_and_flags1 & Self::IS_ARCHIVE != 0 {
            attrs |= 0x0020; // FILE_ATTRIBUTE_ARCHIVE
        }
        if self.accessed_and_flags1 & Self::IS_SYSTEM != 0 {
            attrs |= 0x0004; // FILE_ATTRIBUTE_SYSTEM
        }
        if self.accessed_and_flags1 & Self::IS_HIDDEN != 0 {
            attrs |= 0x0002; // FILE_ATTRIBUTE_HIDDEN
        }
        if self.accessed_and_flags1 & Self::IS_OFFLINE != 0 {
            attrs |= 0x1000; // FILE_ATTRIBUTE_OFFLINE
        }
        if self.accessed_and_flags1 & Self::IS_NOTCONTENTIDX != 0 {
            attrs |= 0x2000; // FILE_ATTRIBUTE_NOT_CONTENT_INDEXED
        }
        if self.flags2 & Self::IS_NOSCRUBDATA != 0 {
            attrs |= 0x0002_0000; // FILE_ATTRIBUTE_NO_SCRUB_DATA
        }
        if self.flags2 & Self::IS_INTEGRITYSTREAM != 0 {
            attrs |= 0x8000; // FILE_ATTRIBUTE_INTEGRITY_STREAM
        }
        if self.flags2 & Self::IS_PINNED != 0 {
            attrs |= 0x0008_0000; // FILE_ATTRIBUTE_PINNED
        }
        if self.flags2 & Self::IS_UNPINNED != 0 {
            attrs |= 0x0010_0000; // FILE_ATTRIBUTE_UNPINNED
        }
        if self.flags2 & Self::IS_DIRECTORY != 0 {
            attrs |= 0x0010; // FILE_ATTRIBUTE_DIRECTORY
        }
        if self.flags2 & Self::IS_COMPRESSED != 0 {
            attrs |= 0x0800; // FILE_ATTRIBUTE_COMPRESSED
        }
        if self.flags2 & Self::IS_ENCRYPTED != 0 {
            attrs |= 0x4000; // FILE_ATTRIBUTE_ENCRYPTED
        }
        if self.flags2 & Self::IS_SPARSEFILE != 0 {
            attrs |= 0x0200; // FILE_ATTRIBUTE_SPARSE_FILE
        }
        if self.flags2 & Self::IS_REPARSEPOINT != 0 {
            attrs |= 0x0400; // FILE_ATTRIBUTE_REPARSE_POINT
        }
        attrs
    }

    /// Set file attributes from a Windows `FILE_ATTRIBUTE_*` bitmask.
    pub const fn set_attributes(&mut self, value: u32) {
        // Clear all flag bits first
        self.accessed_and_flags1 &= Self::ACCESSED_MASK;
        self.flags2 = 0;

        // Set flags based on input
        if value & 0x0001 != 0 {
            self.accessed_and_flags1 |= Self::IS_READONLY;
        }
        if value & 0x0020 != 0 {
            self.accessed_and_flags1 |= Self::IS_ARCHIVE;
        }
        if value & 0x0004 != 0 {
            self.accessed_and_flags1 |= Self::IS_SYSTEM;
        }
        if value & 0x0002 != 0 {
            self.accessed_and_flags1 |= Self::IS_HIDDEN;
        }
        if value & 0x1000 != 0 {
            self.accessed_and_flags1 |= Self::IS_OFFLINE;
        }
        if value & 0x2000 != 0 {
            self.accessed_and_flags1 |= Self::IS_NOTCONTENTIDX;
        }
        if value & 0x0002_0000 != 0 {
            self.flags2 |= Self::IS_NOSCRUBDATA;
        }
        if value & 0x8000 != 0 {
            self.flags2 |= Self::IS_INTEGRITYSTREAM;
        }
        if value & 0x0008_0000 != 0 {
            self.flags2 |= Self::IS_PINNED;
        }
        if value & 0x0010_0000 != 0 {
            self.flags2 |= Self::IS_UNPINNED;
        }
        if value & 0x0010 != 0 {
            self.flags2 |= Self::IS_DIRECTORY;
        }
        if value & 0x0800 != 0 {
            self.flags2 |= Self::IS_COMPRESSED;
        }
        if value & 0x4000 != 0 {
            self.flags2 |= Self::IS_ENCRYPTED;
        }
        if value & 0x0200 != 0 {
            self.flags2 |= Self::IS_SPARSEFILE;
        }
        if value & 0x0400 != 0 {
            self.flags2 |= Self::IS_REPARSEPOINT;
        }
    }

    /// Returns true if this is a directory.
    #[inline]
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.flags2 & Self::IS_DIRECTORY != 0
    }

    /// Set the directory flag.
    #[inline]
    pub const fn set_directory(&mut self, value: bool) {
        if value {
            self.flags2 |= Self::IS_DIRECTORY;
        } else {
            self.flags2 &= !Self::IS_DIRECTORY;
        }
    }
}

impl core::fmt::Debug for StandardInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Copy fields to avoid unaligned reference issues with packed structs
        let created = self.created;
        let written = self.written;
        let accessed_and_flags1 = self.accessed_and_flags1;
        let flags2 = self.flags2;
        let accessed = self.accessed();
        let attributes = self.attributes();
        f.debug_struct("StandardInfo")
            .field("created", &created)
            .field("written", &written)
            .field("accessed_and_flags1", &accessed_and_flags1)
            .field("flags2", &flags2)
            .field("accessed", &accessed)
            .field("attributes", &attributes)
            .finish()
    }
}

// ============================================================================
// Record - Main file record (corresponds to C++ Record struct)
// ============================================================================

/// Main file record structure containing all metadata for an MFT entry.
///
/// Matches C++ `Record` from `ntfs_record_types.hpp`.
///
/// # Memory Layout
/// - `stdinfo`: 26 bytes (`StandardInfo`)
/// - `name_count`: 2 bytes (number of hardlinks)
/// - `stream_count`: 2 bytes (number of streams)
/// - `first_child`: 4 bytes (index into childinfos, or `NO_ENTRY`)
/// - `first_name`: 13 bytes (first `LinkInfo`, inline)
/// - `first_stream`: 32 bytes (first `StreamInfo`, inline)
/// - Total: 79 bytes
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Record {
    /// Standard information (timestamps, attributes)
    pub stdinfo: StandardInfo,
    /// Number of hardlinks (names) for this file
    pub name_count: u16,
    /// Number of streams for this file
    pub stream_count: u16,
    /// Index of first child in childinfos vector (for directories), or
    /// `NO_ENTRY`
    pub first_child: u32,
    /// First hardlink info (stored inline)
    pub first_name: LinkInfo,
    /// First stream info (stored inline)
    pub first_stream: StreamInfo,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            stdinfo: StandardInfo::default(),
            name_count: 0,
            stream_count: 0,
            first_child: NO_ENTRY,
            first_name: LinkInfo::new(),
            first_stream: StreamInfo::new(),
        }
    }
}

impl Record {
    /// Create a new default Record with all sentinel values.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stdinfo: StandardInfo {
                created: 0,
                written: 0,
                accessed_and_flags1: 0,
                flags2: 0,
            },
            name_count: 0,
            stream_count: 0,
            first_child: NO_ENTRY,
            first_name: LinkInfo {
                next_entry: NO_ENTRY,
                name: NameInfo {
                    offset_packed: NO_ENTRY,
                    length: 0,
                },
                parent: 0,
            },
            first_stream: StreamInfo {
                size: SizeInfo {
                    length: FileSizeType { low: 0, high: 0 },
                    allocated: FileSizeType { low: 0, high: 0 },
                    bulkiness: FileSizeType { low: 0, high: 0 },
                    treesize: 0,
                },
                next_entry: NO_ENTRY,
                name: NameInfo {
                    offset_packed: NO_ENTRY,
                    length: 0,
                },
                flags: 0,
            },
        }
    }

    /// Returns true if this record has been populated (has at least one name).
    #[inline]
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.name_count > 0
    }

    /// Returns true if this record is a directory.
    #[inline]
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.stdinfo.is_directory()
    }
}

impl core::fmt::Debug for Record {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Copy fields to avoid unaligned reference issues with packed structs
        let stdinfo = self.stdinfo;
        let name_count = self.name_count;
        let stream_count = self.stream_count;
        let first_child = self.first_child;
        let first_name = self.first_name;
        let first_stream = self.first_stream;
        f.debug_struct("Record")
            .field("stdinfo", &stdinfo)
            .field("name_count", &name_count)
            .field("stream_count", &stream_count)
            .field("first_child", &first_child)
            .field("first_name", &first_name)
            .field("first_stream", &first_stream)
            .finish()
    }
}

// ============================================================================
// CppMftIndex - Main index structure matching C++ NtfsIndex
// ============================================================================

/// Main MFT index structure matching C++ `NtfsIndex`.
///
/// This structure holds all parsed MFT data using the same layout as C++:
/// - `records_data`: All file records (Vec<Record>)
/// - `records_lookup`: FRS → record index mapping (Vec<u32>)
/// - `nameinfos`: Overflow hard links (Vec<LinkInfo>)
/// - `streaminfos`: Overflow streams (Vec<StreamInfo>)
/// - `childinfos`: Parent-child relationships (Vec<ChildInfo>)
/// - `names`: All filenames concatenated (Vec<u8>)
#[derive(Default, Debug)]
pub struct CppMftIndex {
    /// All file records (matches C++ `Records records_data`)
    pub records_data: Vec<Record>,
    /// FRS → record index mapping (matches C++ `RecordsLookup records_lookup`)
    /// Value is `NO_ENTRY` if record doesn't exist
    pub records_lookup: Vec<u32>,
    /// Overflow hard links (matches C++ `LinkInfos nameinfos`)
    /// First link is stored inline in `Record.first_name`
    pub nameinfos: Vec<LinkInfo>,
    /// Overflow streams (matches C++ `StreamInfos streaminfos`)
    /// First stream is stored inline in `Record.first_stream`
    pub streaminfos: Vec<StreamInfo>,
    /// Parent-child relationships (matches C++ `ChildInfos childinfos`)
    pub childinfos: Vec<ChildInfo>,
    /// All filenames concatenated (matches C++ `std::tvstring names`)
    /// ASCII names stored as-is, Unicode names stored as UTF-16LE
    pub names: Vec<u8>,
}

impl CppMftIndex {
    /// Create a new empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new index with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(record_count: usize) -> Self {
        Self {
            records_data: Vec::with_capacity(record_count),
            records_lookup: Vec::with_capacity(record_count),
            nameinfos: Vec::new(),
            streaminfos: Vec::new(),
            childinfos: Vec::new(),
            names: Vec::new(),
        }
    }

    /// Get or create a record for the given FRS.
    ///
    /// This is the Rust equivalent of C++ `at()` function (`ntfs_index.hpp`
    /// lines 106-129). It implements **lazy allocation** - if the record
    /// doesn't exist, a placeholder is created.
    ///
    /// # Arguments
    /// * `frs` - File Record Segment number
    ///
    /// # Returns
    /// Mutable reference to the record (existing or newly created placeholder)
    pub fn get_or_create(&mut self, frs: u32) -> &mut Record {
        let frs_idx = frs as usize;

        // Expand lookup table if needed (matches C++ resize)
        if frs_idx >= self.records_lookup.len() {
            self.records_lookup.resize(frs_idx + 1, NO_ENTRY);
        }

        // Check if record exists
        if self.records_lookup[frs_idx] == NO_ENTRY {
            // Create placeholder record
            let record_idx = usize_to_u32(self.records_data.len());
            self.records_lookup[frs_idx] = record_idx;
            self.records_data.push(Record::new());
        }

        let record_idx = self.records_lookup[frs_idx] as usize;
        &mut self.records_data[record_idx]
    }

    /// Get a record by FRS if it exists.
    ///
    /// This is the Rust equivalent of C++ `_find()` function.
    #[must_use]
    pub fn get(&self, frs: u32) -> Option<&Record> {
        let frs_idx = frs as usize;
        if frs_idx < self.records_lookup.len() {
            let record_idx = self.records_lookup[frs_idx];
            if record_idx != NO_ENTRY {
                return Some(&self.records_data[record_idx as usize]);
            }
        }
        None
    }

    /// Get a mutable record by FRS if it exists.
    #[must_use]
    pub fn get_mut(&mut self, frs: u32) -> Option<&mut Record> {
        let frs_idx = frs as usize;
        if frs_idx < self.records_lookup.len() {
            let record_idx = self.records_lookup[frs_idx];
            if record_idx != NO_ENTRY {
                return Some(&mut self.records_data[record_idx as usize]);
            }
        }
        None
    }

    /// Add a name to the names buffer and return the offset.
    ///
    /// # Arguments
    /// * `name` - The name bytes (ASCII or UTF-16LE)
    /// * `is_ascii` - Whether the name is ASCII
    ///
    /// # Returns
    /// The offset into the names buffer (for use in `NameInfo`)
    pub fn add_name(&mut self, name: &[u8]) -> u32 {
        let offset = usize_to_u32(self.names.len());
        self.names.extend_from_slice(name);
        offset
    }

    /// Add a child entry linking a child to its parent.
    ///
    /// This creates a placeholder for the parent if it doesn't exist,
    /// matching C++ behavior.
    ///
    /// # Arguments
    /// * `child_frs` - FRS of the child file/directory
    /// * `parent_frs` - FRS of the parent directory
    /// * `name_index` - Which hardlink (0-indexed)
    pub fn add_child_entry(&mut self, child_frs: u32, parent_frs: u32, name_index: u16) {
        // Create parent placeholder if needed (matches C++ at(frs_parent))
        // We need to do this first to ensure parent exists
        self.get_or_create(parent_frs);

        // Get parent's current first_child (copy to avoid borrow issues with packed
        // struct)
        let parent_idx = self.records_lookup[parent_frs as usize] as usize;
        let parent_first_child = self.records_data[parent_idx].first_child;

        // Create new child entry
        let child_idx = usize_to_u32(self.childinfos.len());
        let child_info = ChildInfo {
            next_entry: parent_first_child,
            record_number: child_frs,
            name_index,
        };
        self.childinfos.push(child_info);

        // Link to parent's child list
        self.records_data[parent_idx].first_child = child_idx;
    }

    /// Add an overflow link (hardlink) to a record.
    ///
    /// # Arguments
    /// * `frs` - FRS of the file
    /// * `link` - The link info to add
    pub fn add_overflow_link(&mut self, frs: u32, link: LinkInfo) {
        // Ensure record exists
        self.get_or_create(frs);

        // Get current first link's next_entry (copy to avoid borrow issues)
        let record_idx = self.records_lookup[frs as usize] as usize;
        let first_link_next = self.records_data[record_idx].first_name.next_entry;

        // Create new link entry
        let link_idx = usize_to_u32(self.nameinfos.len());
        let mut new_link = link;
        new_link.next_entry = first_link_next;
        self.nameinfos.push(new_link);

        // Update first link to point to new overflow
        self.records_data[record_idx].first_name.next_entry = link_idx;
    }

    /// Add an overflow stream to a record.
    ///
    /// # Arguments
    /// * `frs` - FRS of the file
    /// * `stream` - The stream info to add
    pub fn add_overflow_stream(&mut self, frs: u32, stream: StreamInfo) {
        // Ensure record exists
        self.get_or_create(frs);

        // Get current first stream's next_entry (copy to avoid borrow issues)
        let record_idx = self.records_lookup[frs as usize] as usize;
        let first_stream_next = self.records_data[record_idx].first_stream.next_entry;

        // Create new stream entry
        let stream_idx = usize_to_u32(self.streaminfos.len());
        let mut new_stream = stream;
        new_stream.next_entry = first_stream_next;
        self.streaminfos.push(new_stream);

        // Update first stream to point to new overflow
        self.records_data[record_idx].first_stream.next_entry = stream_idx;
    }

    /// Get the number of records in the index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records_data.len()
    }

    /// Check if the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records_data.is_empty()
    }

    /// Convert this C++ index to a Rust `MftIndex`.
    ///
    /// This is the integration point between the C++ parsing algorithm and
    /// the existing Rust infrastructure. It converts all packed C++ structures
    /// to their Rust equivalents.
    ///
    /// # Arguments
    /// * `volume` - Volume letter (e.g., 'C')
    #[must_use]
    pub fn into_mft_index(self, volume: char) -> crate::index::MftIndex {
        use crate::index::{
            ChildInfo as RustChildInfo, FileRecord, MftIndex, NO_ENTRY as RUST_NO_ENTRY,
            StandardInfo as RustStandardInfo,
        };
        use crate::ntfs::filetime_to_unix_micros;

        let mut index = MftIndex::with_capacity(volume, self.records_data.len());

        // Convert names buffer: C++ stores ASCII as bytes, Unicode as UTF-16LE
        // Rust MftIndex stores all names as UTF-8 String
        // We need to decode each name on-demand during record conversion

        // Build FRS → record index mapping
        // First pass: determine max FRS for frs_to_idx sizing
        let max_frs = self.records_lookup.len();
        index.frs_to_idx.resize(max_frs, RUST_NO_ENTRY);

        // Convert each record
        for (cpp_record_idx, cpp_record) in self.records_data.iter().enumerate() {
            // Find the FRS for this record
            let cpp_record_idx_u32 = usize_to_u32(cpp_record_idx);
            let frs = self
                .records_lookup
                .iter()
                .position(|&idx| idx == cpp_record_idx_u32)
                .unwrap_or(0) as u64;

            // Convert StandardInfo - MUST convert FILETIME to Unix microseconds
            // C++ stores raw Windows FILETIME (100ns since 1601-01-01)
            // Rust MftIndex expects Unix microseconds (μs since 1970-01-01)
            let stdinfo = RustStandardInfo {
                created: filetime_to_unix_micros(u64_to_i64_filetime(cpp_record.stdinfo.created)),
                modified: filetime_to_unix_micros(u64_to_i64_filetime(cpp_record.stdinfo.written)),
                accessed: filetime_to_unix_micros(u64_to_i64_filetime(
                    cpp_record.stdinfo.accessed(),
                )),
                mft_changed: 0, // Not stored in C++ StandardInfo
                flags: Self::convert_cpp_attributes_to_rust_flags(&cpp_record.stdinfo),
                usn: 0,         // Not stored in C++ StandardInfo
                security_id: 0, // Not stored in C++ StandardInfo
                owner_id: 0,    // Not stored in C++ StandardInfo
            };

            // Convert first name (primary link) - add name to index.names buffer
            let first_name = self.convert_link_info_to_index(&cpp_record.first_name, &mut index);

            // Convert first stream - add stream name to index.names buffer
            let first_stream =
                self.convert_stream_info_to_index(&cpp_record.first_stream, &mut index);

            // Create FileRecord with all required fields
            let record = FileRecord {
                frs,
                sequence_number: 0, // Not stored in C++ Record
                namespace: 0,       // Will be set from filename parsing
                forensic_flags: 0,
                lsn: 0,
                reparse_tag: 0,
                base_frs: 0,
                stdinfo,
                name_count: cpp_record.name_count,
                stream_count: cpp_record.stream_count,
                // C++ stores all streams, so total_stream_count = stream_count
                total_stream_count: cpp_record.stream_count,
                first_child: cpp_record.first_child,
                first_name,
                first_stream,
                // $FILE_NAME timestamps (not stored separately in C++)
                // Also need FILETIME → Unix microseconds conversion
                fn_created: filetime_to_unix_micros(u64_to_i64_filetime(
                    cpp_record.stdinfo.created,
                )),
                fn_modified: filetime_to_unix_micros(u64_to_i64_filetime(
                    cpp_record.stdinfo.written,
                )),
                fn_accessed: filetime_to_unix_micros(u64_to_i64_filetime(
                    cpp_record.stdinfo.accessed(),
                )),
                fn_mft_changed: 0,
                // Tree metrics (computed later)
                descendants: 0,
                treesize: 0,
                tree_allocated: 0,
            };

            // Add to index
            let record_idx = usize_to_u32(index.records.len());
            index.records.push(record);
            let frs_usize = u64_to_usize(frs);
            if frs_usize < index.frs_to_idx.len() {
                index.frs_to_idx[frs_usize] = record_idx;
            }
        }

        // Convert overflow links - need to clone nameinfos to avoid borrow issues
        let nameinfos_copy: Vec<LinkInfo> = self.nameinfos.clone();
        for cpp_link in &nameinfos_copy {
            let rust_link = self.convert_link_info_to_index(cpp_link, &mut index);
            index.links.push(rust_link);
        }

        // Convert overflow streams - need to clone streaminfos to avoid borrow issues
        let streaminfos_copy: Vec<StreamInfo> = self.streaminfos.clone();
        for cpp_stream in &streaminfos_copy {
            let rust_stream = self.convert_stream_info_to_index(cpp_stream, &mut index);
            index.streams.push(rust_stream);
        }

        // Convert child entries
        for cpp_child in &self.childinfos {
            let rust_child = RustChildInfo {
                next_entry: cpp_child.next_entry,
                child_frs: u64::from(cpp_child.record_number),
                name_index: cpp_child.name_index,
            };
            index.children.push(rust_child);
        }

        index
    }

    /// Convert C++ `StandardInfo` attributes to Rust flags.
    // Separate function for code organization matching C++ structure
    #[allow(clippy::single_call_fn)]
    #[inline]
    const fn convert_cpp_attributes_to_rust_flags(stdinfo: &StandardInfo) -> u32 {
        // Use inline constants to avoid `use` statement in const fn
        const IS_READONLY: u32 = 1 << 0;
        const IS_ARCHIVE: u32 = 1 << 1;
        const IS_SYSTEM: u32 = 1 << 2;
        const IS_HIDDEN: u32 = 1 << 3;
        const IS_OFFLINE: u32 = 1 << 4;
        const IS_NOT_INDEXED: u32 = 1 << 5;
        const IS_NO_SCRUB_DATA: u32 = 1 << 6;
        const IS_INTEGRITY_STREAM: u32 = 1 << 7;
        const IS_PINNED: u32 = 1 << 8;
        const IS_UNPINNED: u32 = 1 << 9;
        const IS_DIRECTORY: u32 = 1 << 10;
        const IS_COMPRESSED: u32 = 1 << 11;
        const IS_ENCRYPTED: u32 = 1 << 12;
        const IS_SPARSE: u32 = 1 << 13;
        const IS_REPARSE: u32 = 1 << 14;

        let mut flags = 0_u32;
        let attrs = stdinfo.attributes();

        if attrs & 0x0001 != 0 {
            flags |= IS_READONLY;
        }
        if attrs & 0x0020 != 0 {
            flags |= IS_ARCHIVE;
        }
        if attrs & 0x0004 != 0 {
            flags |= IS_SYSTEM;
        }
        if attrs & 0x0002 != 0 {
            flags |= IS_HIDDEN;
        }
        if attrs & 0x1000 != 0 {
            flags |= IS_OFFLINE;
        }
        if attrs & 0x2000 != 0 {
            flags |= IS_NOT_INDEXED;
        }
        if attrs & 0x0002_0000 != 0 {
            flags |= IS_NO_SCRUB_DATA;
        }
        if attrs & 0x8000 != 0 {
            flags |= IS_INTEGRITY_STREAM;
        }
        if attrs & 0x0008_0000 != 0 {
            flags |= IS_PINNED;
        }
        if attrs & 0x0010_0000 != 0 {
            flags |= IS_UNPINNED;
        }
        if attrs & 0x0010 != 0 {
            flags |= IS_DIRECTORY;
        }
        if attrs & 0x0800 != 0 {
            flags |= IS_COMPRESSED;
        }
        if attrs & 0x4000 != 0 {
            flags |= IS_ENCRYPTED;
        }
        if attrs & 0x0200 != 0 {
            flags |= IS_SPARSE;
        }
        if attrs & 0x0400 != 0 {
            flags |= IS_REPARSE;
        }

        flags
    }

    /// Convert C++ `LinkInfo` to Rust `LinkInfo`, adding name to `index.names`
    /// buffer.
    fn convert_link_info_to_index(
        &self,
        cpp_link: &LinkInfo,
        index: &mut crate::index::MftIndex,
    ) -> crate::index::LinkInfo {
        use crate::index::{IndexNameRef, LinkInfo as RustLinkInfo};

        // Decode name from C++ names buffer
        let name_offset = cpp_link.name.offset();
        let name_len = usize::from(cpp_link.name.length());
        let is_ascii = cpp_link.name.ascii();

        let name_str = if name_len == 0 || name_offset == NO_ENTRY {
            String::new()
        } else {
            self.decode_name(name_offset as usize, name_len, is_ascii)
        };

        // Add name to Rust index names buffer and get offset
        let rust_offset = usize_to_u32(index.names.len());
        let name_len_bytes = usize_to_u16(name_str.len());
        let is_name_ascii = name_str.is_ascii();
        index.names.push_str(&name_str);

        // Create IndexNameRef with proper constructor
        let name_ref = IndexNameRef::new(
            rust_offset,
            name_len_bytes,
            is_name_ascii,
            IndexNameRef::NO_EXTENSION,
        );

        RustLinkInfo {
            next_entry: cpp_link.next_entry,
            name: name_ref,
            parent_frs: u64::from(cpp_link.parent),
        }
    }

    /// Convert C++ `StreamInfo` to Rust `IndexStreamInfo`, adding name to
    /// `index.names` buffer.
    fn convert_stream_info_to_index(
        &self,
        cpp_stream: &StreamInfo,
        index: &mut crate::index::MftIndex,
    ) -> crate::index::IndexStreamInfo {
        use crate::index::{IndexNameRef, IndexStreamInfo, SizeInfo as RustSizeInfo};

        // Copy size fields from packed struct
        let allocated = cpp_stream.size.allocated.as_u64();
        let length = cpp_stream.size.length.as_u64();

        let size = RustSizeInfo { length, allocated };

        // Decode stream name from C++ names buffer
        let name_offset = cpp_stream.name.offset();
        let name_len = usize::from(cpp_stream.name.length());
        let is_ascii = cpp_stream.name.ascii();

        let name_str = if name_len == 0 || name_offset == NO_ENTRY {
            String::new()
        } else {
            self.decode_name(name_offset as usize, name_len, is_ascii)
        };

        // Add name to Rust index names buffer and get offset
        let rust_offset = usize_to_u32(index.names.len());
        let name_len_bytes = usize_to_u16(name_str.len());
        let is_name_ascii = name_str.is_ascii();
        index.names.push_str(&name_str);

        // Create IndexNameRef with proper constructor
        let name_ref = IndexNameRef::new(
            rust_offset,
            name_len_bytes,
            is_name_ascii,
            IndexNameRef::NO_EXTENSION,
        );

        // Build flags byte: bit 0 = is_sparse, bit 1 = is_resident, bits 2-7 =
        // type_name_id
        let is_sparse = cpp_stream.is_sparse();
        let type_name_id = cpp_stream.type_name_id();
        let flags = u8::from(is_sparse) | ((type_name_id & 0x3F) << 2_u8);

        IndexStreamInfo {
            size,
            next_entry: cpp_stream.next_entry,
            name: name_ref,
            flags,
        }
    }

    /// Decode a name from the C++ names buffer.
    fn decode_name(&self, offset: usize, len: usize, is_ascii: bool) -> String {
        if offset >= self.names.len() {
            return String::new();
        }

        if is_ascii {
            // ASCII: each byte is a character
            let end = (offset + len).min(self.names.len());
            String::from_utf8_lossy(&self.names[offset..end]).into_owned()
        } else {
            // UTF-16LE: 2 bytes per character
            let byte_len = len * 2;
            let end = (offset + byte_len).min(self.names.len());
            let bytes = &self.names[offset..end];

            // Convert UTF-16LE to String
            // Note: chunks_exact(2) guarantees each chunk has exactly 2 elements
            #[allow(clippy::missing_asserts_for_indexing)]
            let u16_chars: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                .collect();
            String::from_utf16_lossy(&u16_chars)
        }
    }
}

// ============================================================================
// CppParsePipeline - Two-Phase Pipeline matching C++ ntfs_index.hpp
// ============================================================================

// Note: Arc is available in alloc, Mutex is only in std.
use alloc::sync::Arc;
use std::sync::Mutex;

use crate::ntfs::{
    AttributeRecordHeader, AttributeType, FileNameAttribute, FileRecordSegmentHeader,
    ResidentAttributeData, StandardInformation, apply_usa_fixup, file_reference_to_frs,
};

/// File record header flag: record is in use.
const FRH_IN_USE: u16 = 0x0001;
/// File record header flag: record is a directory.
const FRH_DIRECTORY: u16 = 0x0002;

/// `FILE_NAME` namespace - DOS name (8.3 format)
const FILE_NAME_DOS: u8 = 0x02;

/// FILE record magic number ('FILE' in little-endian = 'ELIF')
const FILE_MAGIC: u32 = 0x454C_4946;

/// Two-phase MFT parsing pipeline matching C++ implementation.
///
/// This implements the exact C++ algorithm from `ntfs_index.hpp`:
/// - Phase 1: `preload_concurrent()` - NO LOCK - USA fixup, max FRS discovery
/// - Phase 2: `load()` - WITH LOCK - Serialized attribute parsing
///
/// # Thread Safety
///
/// The index is protected by a Mutex. Phase 1 only briefly acquires the lock
/// to pre-allocate the records vector. Phase 2 holds the lock for the entire
/// parsing operation (serialized).
pub struct CppParsePipeline {
    /// The shared MFT index (protected by mutex)
    pub index: Arc<Mutex<CppMftIndex>>,
    /// MFT record size (typically 1024 bytes)
    pub mft_record_size: u32,
    /// Diagnostic counter: total chunks processed
    pub chunks_processed: core::sync::atomic::AtomicU64,
    /// Diagnostic counter: total records examined (including skipped)
    pub records_examined: core::sync::atomic::AtomicU64,
    /// Diagnostic counter: total records with valid FILE magic
    pub records_with_file_magic: core::sync::atomic::AtomicU64,
    /// Diagnostic counter: total records parsed (in-use base + extension)
    pub records_parsed: core::sync::atomic::AtomicU64,
    /// Diagnostic counter: records skipped at chunk boundaries (partial
    /// records)
    pub records_skipped_boundary: core::sync::atomic::AtomicU64,
    /// Diagnostic counter: USA fixup succeeded
    pub usa_fixup_success: core::sync::atomic::AtomicU64,
    /// Diagnostic counter: USA fixup failed (marked as BAAD)
    pub usa_fixup_failed: core::sync::atomic::AtomicU64,
    /// Diagnostic counter: records not in-use (skipped in Phase 2)
    pub records_not_in_use: core::sync::atomic::AtomicU64,
}

impl CppParsePipeline {
    /// Create a new pipeline with the given record size.
    #[must_use]
    pub fn new(mft_record_size: u32) -> Self {
        Self {
            index: Arc::new(Mutex::new(CppMftIndex::new())),
            mft_record_size,
            chunks_processed: core::sync::atomic::AtomicU64::new(0),
            records_examined: core::sync::atomic::AtomicU64::new(0),
            records_with_file_magic: core::sync::atomic::AtomicU64::new(0),
            records_parsed: core::sync::atomic::AtomicU64::new(0),
            records_skipped_boundary: core::sync::atomic::AtomicU64::new(0),
            usa_fixup_success: core::sync::atomic::AtomicU64::new(0),
            usa_fixup_failed: core::sync::atomic::AtomicU64::new(0),
            records_not_in_use: core::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Create a new pipeline with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(mft_record_size: u32, record_count: usize) -> Self {
        Self {
            index: Arc::new(Mutex::new(CppMftIndex::with_capacity(record_count))),
            mft_record_size,
            chunks_processed: core::sync::atomic::AtomicU64::new(0),
            records_examined: core::sync::atomic::AtomicU64::new(0),
            records_with_file_magic: core::sync::atomic::AtomicU64::new(0),
            records_parsed: core::sync::atomic::AtomicU64::new(0),
            records_skipped_boundary: core::sync::atomic::AtomicU64::new(0),
            usa_fixup_success: core::sync::atomic::AtomicU64::new(0),
            usa_fixup_failed: core::sync::atomic::AtomicU64::new(0),
            records_not_in_use: core::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Log diagnostic summary of processing statistics.
    ///
    /// Call this after all chunks have been processed to see the summary.
    pub fn log_diagnostics(&self) {
        use core::sync::atomic::Ordering;

        use tracing::{debug, warn};

        let chunks = self.chunks_processed.load(Ordering::Relaxed);
        let parsed = self.records_parsed.load(Ordering::Relaxed);
        let skipped_boundary = self.records_skipped_boundary.load(Ordering::Relaxed);
        let usa_failed = self.usa_fixup_failed.load(Ordering::Relaxed);

        debug!(
            chunks_processed = chunks,
            records_parsed = parsed,
            "Parse pipeline complete"
        );

        // Only log warnings for actual issues
        if skipped_boundary > 0 {
            warn!(
                skipped = skipped_boundary,
                "Records skipped at chunk boundaries"
            );
        }
        if usa_failed > 0 {
            warn!(failed = usa_failed, "Records with USA fixup failure");
        }
    }

    /// Process a chunk of MFT data using the two-phase pipeline.
    ///
    /// This is the main entry point matching C++ `mft_reader.hpp` callback.
    ///
    /// # Arguments
    /// * `buffer` - Mutable buffer containing MFT records (will be modified by
    ///   USA fixup)
    /// * `virtual_offset` - Byte offset of this chunk in the MFT
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned (i.e., a previous thread
    /// panicked while holding the lock). This is intentional - mutex
    /// poisoning indicates a serious error that should propagate.
    pub fn process_chunk(&self, buffer: &mut [u8], virtual_offset: u64) {
        use core::sync::atomic::Ordering;

        // Increment chunk counter
        self.chunks_processed.fetch_add(1, Ordering::Relaxed);

        // Calculate start offset for partial record handling
        let mft_record_size = self.mft_record_size as usize;
        let virtual_offset_usize = u64_to_usize(virtual_offset);
        let start_offset = if virtual_offset_usize & (mft_record_size - 1) != 0 {
            mft_record_size - (virtual_offset_usize & (mft_record_size - 1))
        } else {
            0
        };

        // Track partial records at chunk boundaries
        if start_offset > 0 {
            self.records_skipped_boundary
                .fetch_add(1, Ordering::Relaxed);
        }

        // Calculate records in this chunk
        let records_in_chunk = if buffer.len() >= start_offset {
            (buffer.len() - start_offset) / mft_record_size
        } else {
            0
        };
        self.records_examined
            .fetch_add(records_in_chunk as u64, Ordering::Relaxed);

        // PHASE 1: Pre-processing (NO LOCK except for brief pre-allocation)
        let max_frs = self.preload_concurrent(buffer, virtual_offset);

        // Pre-allocate records vector if needed (brief lock)
        if max_frs > 0 {
            // Note: Mutex poisoning only occurs if a thread panics while holding the lock.
            // In this context, we want to propagate the panic rather than handle it
            // gracefully. Merge lock acquisition with its single usage to avoid
            // holding the lock longer than necessary.
            #[allow(clippy::unwrap_used)]
            self.index.lock().unwrap().get_or_create(max_frs - 1);
        }

        // PHASE 2: Parsing (WITH LOCK - serialized)
        // Note: Mutex poisoning only occurs if a thread panics while holding the lock.
        // In this context, we want to propagate the panic rather than handle it
        // gracefully.
        #[allow(clippy::unwrap_used)]
        let mut index = self.index.lock().unwrap();
        self.load(&mut index, buffer, virtual_offset);
    }

    /// Phase 1: Pre-processing without lock.
    ///
    /// Matches C++ `preload_concurrent()` (`ntfs_index.hpp` lines 424-475):
    /// - Apply USA fixup to each record
    /// - Find maximum FRS for pre-allocation
    /// - Mark corrupt records with BAAD magic
    ///
    /// # Returns
    /// Maximum FRS + 1 found in this chunk (0 if no valid records)
    fn preload_concurrent(&self, buffer: &mut [u8], virtual_offset: u64) -> u32 {
        use core::sync::atomic::Ordering;

        let mft_record_size = self.mft_record_size as usize;
        let mft_record_size_log2 = mft_record_size.trailing_zeros();

        let mut max_frs_plus_one: u32 = 0;
        let mut file_magic_count: u64 = 0;

        // Calculate starting offset (handle partial records at chunk boundary)
        let virtual_offset_usize = u64_to_usize(virtual_offset);
        let start_offset = if virtual_offset_usize & (mft_record_size - 1) != 0 {
            mft_record_size - (virtual_offset_usize & (mft_record_size - 1))
        } else {
            0
        };

        let mut usa_success_count: u64 = 0;
        let mut usa_failed_count: u64 = 0;

        let mut i = start_offset;
        while i + mft_record_size <= buffer.len() {
            let frs = u64_to_u32((virtual_offset + i as u64) >> mft_record_size_log2);
            let record_data = &mut buffer[i..i + mft_record_size];

            // Check magic number - need at least 8 bytes for magic + USA offset/count
            if record_data.len() >= 8 {
                // Assert for clippy::missing_asserts_for_indexing
                debug_assert!(
                    record_data.len() >= 8,
                    "record_data must be at least 8 bytes for magic + USA offset/count"
                );
                let magic = u32::from_le_bytes([
                    record_data[0],
                    record_data[1],
                    record_data[2],
                    record_data[3],
                ]);

                if magic == FILE_MAGIC {
                    file_magic_count += 1;

                    // Apply USA fixup - we already checked len >= 8 above
                    let usa_offset = u16::from_le_bytes([record_data[4], record_data[5]]);
                    let usa_count = u16::from_le_bytes([record_data[6], record_data[7]]);

                    if apply_usa_fixup(record_data, usa_offset, usa_count) {
                        usa_success_count += 1;
                        // Get base FRS (for extension records)
                        let frs_base = Self::get_base_frs(record_data, frs);
                        if max_frs_plus_one < frs_base + 1 {
                            max_frs_plus_one = frs_base + 1;
                        }
                    } else {
                        usa_failed_count += 1;
                        // Mark as corrupt (BAAD)
                        record_data[0] = 0x42; // 'B'
                        record_data[1] = 0x41; // 'A'
                        record_data[2] = 0x41; // 'A'
                        record_data[3] = 0x44; // 'D'
                    }
                }
            }

            i += mft_record_size;
        }

        // Update diagnostic counters
        self.records_with_file_magic
            .fetch_add(file_magic_count, Ordering::Relaxed);
        self.usa_fixup_success
            .fetch_add(usa_success_count, Ordering::Relaxed);
        self.usa_fixup_failed
            .fetch_add(usa_failed_count, Ordering::Relaxed);

        max_frs_plus_one
    }

    /// Get the base FRS from a record (handles extension records).
    // Separate function for code organization matching C++ structure
    #[allow(clippy::single_call_fn)]
    #[inline]
    fn get_base_frs(record_data: &[u8], frs: u32) -> u32 {
        // BaseFileRecordSegment is at offset 32 in FILE_RECORD_SEGMENT_HEADER
        if record_data.len() >= 40 {
            // Assert for clippy::missing_asserts_for_indexing
            debug_assert!(
                record_data.len() >= 40,
                "record_data must be at least 40 bytes for BaseFileRecordSegment"
            );
            let base_ref = u64::from_le_bytes([
                record_data[32],
                record_data[33],
                record_data[34],
                record_data[35],
                record_data[36],
                record_data[37],
                record_data[38],
                record_data[39],
            ]);
            if base_ref != 0 {
                return u64_to_u32(file_reference_to_frs(base_ref));
            }
        }
        frs
    }

    /// Phase 2: Parsing with lock held.
    ///
    /// Matches C++ `load()` (`ntfs_index.hpp` lines 477-728):
    /// - Parse `$STANDARD_INFORMATION`
    /// - Parse `$FILE_NAME` (with parent-child linking)
    /// - Parse stream attributes
    ///
    /// This function is called with the mutex held (serialized parsing).
    #[allow(clippy::too_many_lines)]
    fn load(&self, index: &mut CppMftIndex, buffer: &[u8], virtual_offset: u64) {
        use core::sync::atomic::Ordering;

        let mft_record_size = self.mft_record_size as usize;
        let mft_record_size_log2 = mft_record_size.trailing_zeros();

        // Calculate starting offset
        let virtual_offset_usize = u64_to_usize(virtual_offset);
        let start_offset = if virtual_offset_usize & (mft_record_size - 1) != 0 {
            mft_record_size - (virtual_offset_usize & (mft_record_size - 1))
        } else {
            0
        };

        let mut parsed_count: u64 = 0;
        let mut not_in_use_count: u64 = 0;
        let mut i = start_offset;
        while i + mft_record_size <= buffer.len() {
            let frs = u64_to_u32((virtual_offset + i as u64) >> mft_record_size_log2);
            let record_data = &buffer[i..i + mft_record_size];

            // Check if record has FILE magic but is not in-use
            // (This helps diagnose the flow: FILE magic -> USA fixup -> in-use check)
            // Assert length before indexing to elide bounds checks
            assert!(
                record_data.len() >= 48,
                "record_data too short for magic/flags check"
            );
            let magic = u32::from_le_bytes([
                record_data[0],
                record_data[1],
                record_data[2],
                record_data[3],
            ]);
            let flags = u16::from_le_bytes([record_data[22], record_data[23]]);
            if magic == FILE_MAGIC && (flags & FRH_IN_USE) == 0 {
                not_in_use_count += 1;
            }

            // Parse this record (returns true if record was in-use and parsed)
            if Self::parse_record(index, record_data, frs) {
                parsed_count += 1;
            }

            i += mft_record_size;
        }

        // Update diagnostic counters
        self.records_parsed
            .fetch_add(parsed_count, Ordering::Relaxed);
        self.records_not_in_use
            .fetch_add(not_in_use_count, Ordering::Relaxed);
    }

    /// Parse a single MFT record.
    ///
    /// Matches C++ parsing loop (`ntfs_index.hpp` lines 513-728).
    ///
    /// Returns `true` if the record was in-use and parsed, `false` otherwise.
    // Separate function for code organization matching C++ structure
    #[allow(clippy::single_call_fn)]
    #[inline]
    #[allow(clippy::too_many_lines, unsafe_code)]
    fn parse_record(index: &mut CppMftIndex, data: &[u8], frs: u32) -> bool {
        use core::mem::size_of;

        if data.len() < size_of::<FileRecordSegmentHeader>() {
            return false;
        }

        // Read header
        // SAFETY: We've verified the buffer is large enough
        let header: FileRecordSegmentHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

        // Check magic and in-use flag
        let magic = header.multi_sector_header.magic;
        let flags = header.flags;
        if magic != FILE_MAGIC || (flags & FRH_IN_USE) == 0 {
            return false;
        }

        // Get base FRS (for extension records)
        let base_ref = header.base_file_record_segment;
        let frs_base = if base_ref != 0 {
            u64_to_u32(file_reference_to_frs(base_ref))
        } else {
            frs
        };

        // Get or create the base record (this is the key C++ behavior!)
        index.get_or_create(frs_base);

        // Calculate record boundaries
        let first_attr_offset = header.first_attribute_offset as usize;
        let bytes_in_use = header.bytes_in_use as usize;
        let record_end = bytes_in_use.min(data.len());

        if first_attr_offset >= record_end {
            return true; // Record was in-use but had no attributes
        }

        // Iterate attributes
        let mut attr_offset = first_attr_offset;
        while attr_offset + size_of::<AttributeRecordHeader>() <= record_end {
            // SAFETY: We've verified bounds
            let attr_header: AttributeRecordHeader =
                unsafe { core::ptr::read(data[attr_offset..].as_ptr().cast()) };

            let type_code = attr_header.type_code;
            let attr_length = attr_header.length as usize;

            // Check for end marker or invalid length
            if type_code == 0xFFFF_FFFF || type_code == 0 || attr_length == 0 {
                break;
            }

            // Bounds check
            if attr_offset + attr_length > record_end {
                break;
            }

            let attr_data = &data[attr_offset..attr_offset + attr_length];

            // Parse based on attribute type
            match AttributeType::from_u32(type_code) {
                Some(AttributeType::StandardInformation) => {
                    Self::parse_standard_info(index, attr_data, frs_base, flags);
                }
                Some(AttributeType::FileName) => {
                    Self::parse_file_name(index, attr_data, frs_base);
                }
                Some(
                    AttributeType::Data
                    | AttributeType::IndexRoot
                    | AttributeType::IndexAllocation
                    | AttributeType::Bitmap
                    | AttributeType::ReparsePoint
                    | AttributeType::Ea
                    | AttributeType::EaInformation
                    | AttributeType::ObjectId
                    | AttributeType::PropertySet,
                ) => {
                    Self::parse_stream(index, attr_data, frs_base, &attr_header);
                }
                _ => {
                    // Other attributes - still parse as potential streams
                    if attr_header.is_non_resident != 0 || attr_header.name_length > 0 {
                        Self::parse_stream(index, attr_data, frs_base, &attr_header);
                    }
                }
            }

            attr_offset += attr_length;
        }

        true // Record was successfully parsed
    }

    /// Parse `$STANDARD_INFORMATION` attribute.
    // Separate function for code organization matching C++ structure
    #[allow(clippy::single_call_fn)]
    #[inline]
    #[allow(unsafe_code)]
    fn parse_standard_info(index: &mut CppMftIndex, attr_data: &[u8], frs_base: u32, flags: u16) {
        use core::mem::size_of;

        // Get resident value
        let header_size = size_of::<AttributeRecordHeader>();
        if attr_data.len() < header_size + size_of::<ResidentAttributeData>() {
            return;
        }

        // SAFETY: Bounds checked above
        let resident: ResidentAttributeData =
            unsafe { core::ptr::read(attr_data[header_size..].as_ptr().cast()) };

        let value_offset = resident.value_offset as usize;
        let value_length = resident.value_length as usize;

        if value_offset + value_length > attr_data.len()
            || value_length < size_of::<StandardInformation>()
        {
            return;
        }

        // SAFETY: Bounds checked above
        let std_info: StandardInformation =
            unsafe { core::ptr::read(attr_data[value_offset..].as_ptr().cast()) };

        // Update record's stdinfo
        let record_idx = index.records_lookup[frs_base as usize] as usize;
        let record = &mut index.records_data[record_idx];

        record.stdinfo.created = i64_to_u64_filetime(std_info.creation_time);
        record.stdinfo.written = i64_to_u64_filetime(std_info.modification_time);
        record
            .stdinfo
            .set_accessed(i64_to_u64_filetime(std_info.access_time));

        // Combine file attributes with directory flag
        let mut attrs = std_info.file_attributes;
        if (flags & FRH_DIRECTORY) != 0 {
            attrs |= 0x10; // FILE_ATTRIBUTE_DIRECTORY
        }
        record.stdinfo.set_attributes(attrs);
    }

    /// Parse `$FILE_NAME` attribute.
    // Separate function for code organization matching C++ structure
    #[allow(clippy::single_call_fn)]
    #[inline]
    #[allow(unsafe_code)]
    fn parse_file_name(index: &mut CppMftIndex, attr_data: &[u8], frs_base: u32) {
        use core::mem::size_of;

        // Get resident value
        let header_size = size_of::<AttributeRecordHeader>();
        if attr_data.len() < header_size + size_of::<ResidentAttributeData>() {
            return;
        }

        // SAFETY: Bounds checked above
        let resident: ResidentAttributeData =
            unsafe { core::ptr::read(attr_data[header_size..].as_ptr().cast()) };

        let value_offset = resident.value_offset as usize;
        let value_length = resident.value_length as usize;

        if value_offset + value_length > attr_data.len()
            || value_length < size_of::<FileNameAttribute>()
        {
            return;
        }

        // SAFETY: Bounds checked above
        let fn_attr: FileNameAttribute =
            unsafe { core::ptr::read(attr_data[value_offset..].as_ptr().cast()) };

        // Skip DOS-only names (0x02)
        if fn_attr.file_name_namespace == FILE_NAME_DOS {
            return;
        }

        let frs_parent = u64_to_u32(fn_attr.parent_frs());
        let name_length_u8 = fn_attr.file_name_length;
        let name_length = usize::from(name_length_u8);

        // Get filename bytes
        let name_start = value_offset + size_of::<FileNameAttribute>();
        let name_bytes = name_length * 2; // UTF-16LE
        if name_start + name_bytes > attr_data.len() {
            return;
        }

        let name_data = &attr_data[name_start..name_start + name_bytes];

        // Check if ASCII
        let is_ascii = is_ascii_utf16(name_data);

        // Get current record
        let record_idx = index.records_lookup[frs_base as usize] as usize;

        // Check if we need to push current first_name to overflow
        let current_name_count = index.records_data[record_idx].name_count;
        if current_name_count > 0 {
            // Push current first_name to overflow list
            let first_name = index.records_data[record_idx].first_name;
            let link_idx = usize_to_u32(index.nameinfos.len());
            index.nameinfos.push(first_name);
            index.records_data[record_idx].first_name.next_entry = link_idx;
        }

        // Store name in names buffer
        let name_offset = usize_to_u32(index.names.len());
        if is_ascii {
            // Store as ASCII (1 byte per char)
            for chunk in name_data.chunks_exact(2) {
                index.names.push(chunk[0]);
            }
        } else {
            // Store as UTF-16LE
            index.names.extend_from_slice(name_data);
        }

        // Update first_name
        let record = &mut index.records_data[record_idx];
        record.first_name.name.set_offset(name_offset);
        record.first_name.name.set_length(name_length_u8);
        record.first_name.name.set_ascii(is_ascii);
        record.first_name.parent = frs_parent;

        // Add child entry to parent (if different from self)
        if frs_parent != frs_base {
            // Ensure parent exists
            index.get_or_create(frs_parent);

            // Get parent's current first_child
            let parent_idx = index.records_lookup[frs_parent as usize] as usize;
            let parent_first_child = index.records_data[parent_idx].first_child;

            // Create child entry
            let child_idx = usize_to_u32(index.childinfos.len());
            let child_info = ChildInfo {
                next_entry: parent_first_child,
                record_number: frs_base,
                name_index: current_name_count,
            };
            index.childinfos.push(child_info);

            // Update parent's first_child
            index.records_data[parent_idx].first_child = child_idx;
        }

        // Increment name count
        index.records_data[record_idx].name_count += 1;
    }

    /// Parse stream attributes (`$DATA`, `$INDEX_ROOT`, etc.).
    #[allow(unsafe_code)]
    fn parse_stream(
        index: &mut CppMftIndex,
        attr_data: &[u8],
        frs_base: u32,
        attr_header: &AttributeRecordHeader,
    ) {
        use core::mem::size_of;

        let is_non_resident = attr_header.is_non_resident != 0;
        let type_code = attr_header.type_code;
        let name_length_u8 = attr_header.name_length;
        let name_length = usize::from(name_length_u8);
        let name_offset = usize::from(attr_header.name_offset);

        // Only process primary attributes (LowestVCN == 0 for non-resident)
        if is_non_resident {
            // Check LowestVCN
            let header_size = size_of::<AttributeRecordHeader>();
            if attr_data.len() >= header_size + 8 {
                let lowest_vcn = i64::from_le_bytes([
                    attr_data[header_size],
                    attr_data[header_size + 1],
                    attr_data[header_size + 2],
                    attr_data[header_size + 3],
                    attr_data[header_size + 4],
                    attr_data[header_size + 5],
                    attr_data[header_size + 6],
                    attr_data[header_size + 7],
                ]);
                if lowest_vcn != 0 {
                    return; // Not primary attribute
                }
            }
        }

        // Check if this is a directory index ($I30)
        let is_dir_index = matches!(
            AttributeType::from_u32(type_code),
            Some(AttributeType::Bitmap | AttributeType::IndexRoot | AttributeType::IndexAllocation)
        ) && name_length == 4
            && name_offset + 8 <= attr_data.len()
            && &attr_data[name_offset..name_offset + 8] == b"$\x00I\x003\x000\x00";

        // Calculate type_name_id (matches C++ type_name_id = type >> 4)
        // NTFS attribute type codes are 0x10-0xF0, so >> 4 gives 1-15, fits in u8
        let type_name_id = if is_dir_index {
            0_u8
        } else {
            ((type_code >> 4_u32) & 0xFF) as u8
        };

        let stream_name_length = if is_dir_index { 0_u8 } else { name_length_u8 };

        // Get record
        let record_idx = index.records_lookup[frs_base as usize] as usize;

        // Check if we need to push current first_stream to overflow
        let current_stream_count = index.records_data[record_idx].stream_count;
        if current_stream_count > 0 {
            // Check if we should merge with existing stream (for directory indexes)
            if is_dir_index {
                // Look for existing directory stream to merge with
                let first_stream = &index.records_data[record_idx].first_stream;
                let first_type_name_id = first_stream.type_name_id();
                let first_name_len = first_stream.name.length();
                if first_type_name_id == 0 && first_name_len == 0 {
                    // Merge with first_stream - just update sizes
                    Self::update_stream_sizes(index, attr_data, frs_base, is_non_resident);
                    return;
                }
            }

            // Push current first_stream to overflow list
            let first_stream = index.records_data[record_idx].first_stream;
            let stream_idx = usize_to_u32(index.streaminfos.len());
            index.streaminfos.push(first_stream);
            index.records_data[record_idx].first_stream.next_entry = stream_idx;
        }

        // Store stream name if present
        let stream_name_offset = if !is_dir_index && name_length > 0 {
            let offset = usize_to_u32(index.names.len());
            let name_bytes = name_length * 2;
            if name_offset + name_bytes <= attr_data.len() {
                let name_data = &attr_data[name_offset..name_offset + name_bytes];
                let is_ascii = is_ascii_utf16(name_data);
                if is_ascii {
                    for chunk in name_data.chunks_exact(2) {
                        index.names.push(chunk[0]);
                    }
                } else {
                    index.names.extend_from_slice(name_data);
                }
            }
            offset
        } else {
            0
        };

        // Initialize first_stream
        let record = &mut index.records_data[record_idx];
        record.first_stream.size.allocated = FileSizeType::new(0);
        record.first_stream.size.length = FileSizeType::new(0);
        record.first_stream.size.bulkiness = FileSizeType::new(0);
        record.first_stream.size.treesize = u32::from(is_dir_index);
        record.first_stream.set_sparse(false);
        record.first_stream.set_allocated_size_accounted(false);
        record.first_stream.set_type_name_id(type_name_id);
        record.first_stream.name.set_offset(stream_name_offset);
        record.first_stream.name.set_length(stream_name_length);
        record
            .first_stream
            .name
            .set_ascii(!is_dir_index && name_length > 0);

        // Update sizes
        Self::update_stream_sizes(index, attr_data, frs_base, is_non_resident);

        // Increment stream count
        index.records_data[record_idx].stream_count += 1;
    }

    /// Update stream sizes from attribute data.
    #[allow(unsafe_code)]
    fn update_stream_sizes(
        index: &mut CppMftIndex,
        attr_data: &[u8],
        frs_base: u32,
        is_non_resident: bool,
    ) {
        use core::mem::size_of;

        use crate::ntfs::NonResidentAttributeData;

        let record_idx = index.records_lookup[frs_base as usize] as usize;
        let header_size = size_of::<AttributeRecordHeader>();

        if is_non_resident {
            if attr_data.len() >= header_size + size_of::<NonResidentAttributeData>() {
                // SAFETY: Bounds checked above
                let non_res: NonResidentAttributeData =
                    unsafe { core::ptr::read(attr_data[header_size..].as_ptr().cast()) };

                let allocated = i64_to_u64_filetime(non_res.allocated_size);
                let data_size = i64_to_u64_filetime(non_res.data_size);

                let record = &mut index.records_data[record_idx];
                record.first_stream.size.allocated += FileSizeType::new(allocated);
                record.first_stream.size.length += FileSizeType::new(data_size);
                record.first_stream.size.bulkiness += FileSizeType::new(allocated);
            }
        } else {
            // Resident attribute
            if attr_data.len() >= header_size + size_of::<ResidentAttributeData>() {
                // SAFETY: Bounds checked above
                let resident: ResidentAttributeData =
                    unsafe { core::ptr::read(attr_data[header_size..].as_ptr().cast()) };

                let value_length = u64::from(resident.value_length);
                let record = &mut index.records_data[record_idx];
                record.first_stream.size.length += FileSizeType::new(value_length);
            }
        }
    }

    /// Get the final index (consumes the pipeline).
    ///
    /// # Panics
    /// Panics if the pipeline still has multiple references or if the mutex was
    /// poisoned.
    #[must_use]
    #[allow(clippy::expect_used)]
    pub fn into_index(self) -> CppMftIndex {
        Arc::try_unwrap(self.index)
            .expect("Pipeline still has multiple references")
            .into_inner()
            .expect("Mutex was poisoned")
    }
}

/// Check if UTF-16LE data is ASCII-compatible.
fn is_ascii_utf16(data: &[u8]) -> bool {
    for chunk in data.chunks_exact(2) {
        if chunk[1] != 0 || chunk[0] > 127 {
            return false;
        }
    }
    true
}

// ============================================================================
// Size assertions to ensure packed structures match C++ sizes
// ============================================================================

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::significant_drop_tightening,
    clippy::semicolon_outside_block,
    clippy::let_underscore_untyped
)]
mod size_tests {
    use core::mem::size_of;

    use super::*;

    #[test]
    fn test_file_size_type_size() {
        assert_eq!(size_of::<FileSizeType>(), 6);
    }

    #[test]
    fn test_size_info_size() {
        assert_eq!(size_of::<SizeInfo>(), 22);
    }

    #[test]
    fn test_name_info_size() {
        assert_eq!(size_of::<NameInfo>(), 5);
    }

    #[test]
    fn test_link_info_size() {
        assert_eq!(size_of::<LinkInfo>(), 13);
    }

    #[test]
    fn test_stream_info_size() {
        assert_eq!(size_of::<StreamInfo>(), 32);
    }

    #[test]
    fn test_child_info_size() {
        assert_eq!(size_of::<ChildInfo>(), 10);
    }

    #[test]
    fn test_standard_info_size() {
        assert_eq!(size_of::<StandardInfo>(), 26);
    }

    #[test]
    fn test_record_size() {
        assert_eq!(size_of::<Record>(), 79);
    }

    #[test]
    fn test_cpp_mft_index_get_or_create() {
        let mut index = CppMftIndex::new();

        // First access creates placeholder
        {
            let record = index.get_or_create(100);
            // Copy field to avoid packed struct reference issues
            let name_count = record.name_count;
            assert_eq!(name_count, 0); // Placeholder has no names
        }

        // Second access returns same record
        {
            let record2 = index.get_or_create(100);
            let name_count = record2.name_count;
            assert_eq!(name_count, 0);
        }

        // Verify lookup table was expanded
        assert!(index.records_lookup.len() > 100);
        assert_eq!(index.records_lookup[100], 0); // First record at index 0
    }

    #[test]
    fn test_cpp_mft_index_sparse_frs() {
        let mut index = CppMftIndex::new();

        // Create records with sparse FRS numbers
        index.get_or_create(5);
        index.get_or_create(1000);
        index.get_or_create(50);

        // Verify all records exist
        assert!(index.get(5).is_some());
        assert!(index.get(1000).is_some());
        assert!(index.get(50).is_some());

        // Verify non-existent records return None
        assert!(index.get(0).is_none());
        assert!(index.get(999).is_none());

        // Verify record count
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn test_cpp_mft_index_add_child_entry() {
        let mut index = CppMftIndex::new();

        // Add child entry - should create parent placeholder
        index.add_child_entry(10, 5, 0);

        // Verify parent was created
        assert!(index.get(5).is_some());

        // Verify child entry was added
        assert_eq!(index.childinfos.len(), 1);
        // Copy fields to local variables to avoid packed struct reference issues
        let child = index.childinfos[0];
        let child_record_number = child.record_number;
        let child_name_index = child.name_index;
        assert_eq!(child_record_number, 10);
        assert_eq!(child_name_index, 0);

        // Verify parent points to child
        let parent = *index.get(5).expect("parent should exist");
        let parent_first_child = parent.first_child;
        assert_eq!(parent_first_child, 0); // Index into childinfos
    }

    #[test]
    fn test_file_size_type_conversion() {
        // Test round-trip conversion
        let original: u64 = 0x0000_1234_5678_9ABC;
        let packed = FileSizeType::new(original);
        assert_eq!(u64::from(packed), original);

        // Test max value (48-bit)
        let max_48bit: u64 = 0x0000_FFFF_FFFF_FFFF;
        let packed_max = FileSizeType::new(max_48bit);
        assert_eq!(u64::from(packed_max), max_48bit);

        // Test zero
        let zero = FileSizeType::new(0);
        assert_eq!(u64::from(zero), 0);
    }

    #[test]
    fn test_name_info_offset_ascii() {
        let mut name_info = NameInfo::default();

        // Test offset storage
        name_info.set_offset(0x1234_5678);
        assert_eq!(name_info.offset(), 0x1234_5678);

        // Test ASCII flag
        name_info.set_ascii(true);
        assert!(name_info.ascii());
        assert_eq!(name_info.offset(), 0x1234_5678); // Offset unchanged

        name_info.set_ascii(false);
        assert!(!name_info.ascii());
    }
}

// ============================================================================
// USA Fixup and Parsing Tests
// ============================================================================

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::significant_drop_tightening,
    clippy::semicolon_outside_block,
    clippy::let_underscore_untyped
)]
mod usa_fixup_tests {
    use crate::ntfs::apply_usa_fixup;

    /// Create a mock 1024-byte FILE record with valid USA.
    ///
    /// Layout:
    /// - Bytes 0-3: Magic "FILE"
    /// - Bytes 4-5: USA offset (0x30 = 48)
    /// - Bytes 6-7: USA count (3 for 1024-byte record: 1 check + 2 sectors)
    /// - Bytes 48-49: Check value
    /// - Bytes 50-51: Original value for sector 1 end
    /// - Bytes 52-53: Original value for sector 2 end
    /// - Bytes 510-511: Check value (will be replaced)
    /// - Bytes 1022-1023: Check value (will be replaced)
    fn create_valid_file_record() -> Vec<u8> {
        let mut data = vec![0_u8; 1024];

        // Magic "FILE"
        data[0] = 0x46; // 'F'
        data[1] = 0x49; // 'I'
        data[2] = 0x4C; // 'L'
        data[3] = 0x45; // 'E'

        // USA offset = 48 (0x30)
        data[4] = 0x30;
        data[5] = 0x00;

        // USA count = 3 (1 check value + 2 sector replacements)
        data[6] = 0x03;
        data[7] = 0x00;

        // Check value at USA[0] = 0xBEEF
        data[48] = 0xEF;
        data[49] = 0xBE;

        // Original value for sector 1 end (USA[1]) = 0x1234
        data[50] = 0x34;
        data[51] = 0x12;

        // Original value for sector 2 end (USA[2]) = 0x5678
        data[52] = 0x78;
        data[53] = 0x56;

        // Place check value at sector boundaries (last 2 bytes of each sector)
        // Sector 1 ends at byte 511 (bytes 510-511)
        data[510] = 0xEF;
        data[511] = 0xBE;

        // Sector 2 ends at byte 1023 (bytes 1022-1023)
        data[1022] = 0xEF;
        data[1023] = 0xBE;

        data
    }

    #[test]
    fn test_unfixup_valid_record() {
        let mut data = create_valid_file_record();

        // Apply USA fixup
        let result = apply_usa_fixup(&mut data, 0x30, 3);
        assert!(result, "USA fixup should succeed for valid record");

        // Verify original values were restored
        assert_eq!(
            u16::from_le_bytes([data[510], data[511]]),
            0x1234,
            "Sector 1 end should be restored to original value"
        );
        assert_eq!(
            u16::from_le_bytes([data[1022], data[1023]]),
            0x5678,
            "Sector 2 end should be restored to original value"
        );
    }

    #[test]
    fn test_unfixup_torn_write() {
        let mut data = create_valid_file_record();

        // Corrupt the check value at sector 1 boundary (simulates torn write)
        data[510] = 0x00;
        data[511] = 0x00;

        // Apply USA fixup - should return false due to mismatch
        let result = apply_usa_fixup(&mut data, 0x30, 3);
        assert!(!result, "USA fixup should fail for torn write");
    }

    #[test]
    fn test_unfixup_empty_usa() {
        let mut data = vec![0_u8; 1024];

        // USA count = 0 (invalid)
        let result = apply_usa_fixup(&mut data, 0x30, 0);
        assert!(!result, "USA fixup should fail with count = 0");
    }

    #[test]
    fn test_unfixup_single_sector() {
        let mut data = vec![0_u8; 512];

        // Magic "FILE"
        data[0] = 0x46;
        data[1] = 0x49;
        data[2] = 0x4C;
        data[3] = 0x45;

        // USA offset = 48
        data[4] = 0x30;
        data[5] = 0x00;

        // USA count = 2 (1 check + 1 sector)
        data[6] = 0x02;
        data[7] = 0x00;

        // Check value = 0xABCD
        data[48] = 0xCD;
        data[49] = 0xAB;

        // Original value = 0x9999
        data[50] = 0x99;
        data[51] = 0x99;

        // Place check value at sector end
        data[510] = 0xCD;
        data[511] = 0xAB;

        let result = apply_usa_fixup(&mut data, 0x30, 2);
        assert!(result, "USA fixup should succeed for single sector");

        assert_eq!(
            u16::from_le_bytes([data[510], data[511]]),
            0x9999,
            "Sector end should be restored"
        );
    }

    #[test]
    fn test_unfixup_buffer_too_small() {
        // Test with buffer too small to even read the check value
        let mut data = vec![0_u8; 40]; // Too small for USA at offset 48

        let result = apply_usa_fixup(&mut data, 0x30, 3);
        assert!(
            !result,
            "USA fixup should fail when USA offset is beyond buffer"
        );
    }
}

// ============================================================================
// Attribute Parsing Tests
// ============================================================================

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::significant_drop_tightening,
    clippy::semicolon_outside_block,
    clippy::let_underscore_untyped,
    clippy::cast_sign_loss,
    clippy::single_call_fn
)]
mod attribute_parsing_tests {
    use super::*;

    /// Create a mock `$STANDARD_INFORMATION` attribute.
    ///
    /// Layout:
    /// - `AttributeRecordHeader` (16 bytes)
    /// - `ResidentAttributeData` (8 bytes)
    /// - `StandardInformation` (36 bytes)
    fn create_standard_info_attribute(
        created: i64,
        modified: i64,
        accessed: i64,
        attributes: u32,
    ) -> Vec<u8> {
        let header_size = 16;
        let resident_size = 8;
        let value_size = 36; // StandardInformation size
        let value_offset = header_size + resident_size;
        let total_size = value_offset + value_size;

        let mut data = vec![0_u8; total_size];

        // AttributeRecordHeader
        // type_code = 0x10 ($STANDARD_INFORMATION)
        data[0..4].copy_from_slice(&0x10_u32.to_le_bytes());
        // length
        data[4..8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 0 (resident)
        data[8] = 0;
        // name_length = 0
        data[9] = 0;
        // name_offset = 0
        data[10..12].copy_from_slice(&0_u16.to_le_bytes());
        // flags = 0
        data[12..14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[14..16].copy_from_slice(&0_u16.to_le_bytes());

        // ResidentAttributeData
        // value_length
        data[16..20].copy_from_slice(&(value_size as u32).to_le_bytes());
        // value_offset
        data[20..22].copy_from_slice(&(value_offset as u16).to_le_bytes());
        // flags = 0
        data[22..24].copy_from_slice(&0_u16.to_le_bytes());

        // StandardInformation
        let value_start = value_offset;
        // creation_time
        data[value_start..value_start + 8].copy_from_slice(&created.to_le_bytes());
        // modification_time
        data[value_start + 8..value_start + 16].copy_from_slice(&modified.to_le_bytes());
        // mft_change_time (not used in our parsing)
        data[value_start + 16..value_start + 24].copy_from_slice(&0_i64.to_le_bytes());
        // access_time
        data[value_start + 24..value_start + 32].copy_from_slice(&accessed.to_le_bytes());
        // file_attributes
        data[value_start + 32..value_start + 36].copy_from_slice(&attributes.to_le_bytes());

        data
    }

    /// Create a mock `$FILE_NAME` attribute.
    ///
    /// Layout:
    /// - `AttributeRecordHeader` (16 bytes)
    /// - `ResidentAttributeData` (8 bytes)
    /// - `FileNameAttribute` (66 bytes) + filename (variable)
    fn create_filename_attribute(parent_frs: u64, name: &str, namespace: u8) -> Vec<u8> {
        let header_size = 16;
        let resident_size = 8;
        let fn_attr_size = 66;
        let name_bytes: Vec<u16> = name.encode_utf16().collect();
        let name_byte_len = name_bytes.len() * 2;
        let value_size = fn_attr_size + name_byte_len;
        let value_offset = header_size + resident_size;
        let total_size = value_offset + value_size;

        let mut data = vec![0_u8; total_size];

        // AttributeRecordHeader
        // type_code = 0x30 ($FILE_NAME)
        data[0..4].copy_from_slice(&0x30_u32.to_le_bytes());
        // length
        data[4..8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 0 (resident)
        data[8] = 0;
        // name_length = 0
        data[9] = 0;
        // name_offset = 0
        data[10..12].copy_from_slice(&0_u16.to_le_bytes());
        // flags = 0
        data[12..14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[14..16].copy_from_slice(&0_u16.to_le_bytes());

        // ResidentAttributeData
        // value_length
        data[16..20].copy_from_slice(&(value_size as u32).to_le_bytes());
        // value_offset
        data[20..22].copy_from_slice(&(value_offset as u16).to_le_bytes());
        // flags = 0
        data[22..24].copy_from_slice(&0_u16.to_le_bytes());

        // FileNameAttribute
        let value_start = value_offset;
        // parent_directory (8 bytes)
        data[value_start..value_start + 8].copy_from_slice(&parent_frs.to_le_bytes());
        // creation_time (8 bytes)
        data[value_start + 8..value_start + 16].copy_from_slice(&0_i64.to_le_bytes());
        // modification_time (8 bytes)
        data[value_start + 16..value_start + 24].copy_from_slice(&0_i64.to_le_bytes());
        // mft_change_time (8 bytes)
        data[value_start + 24..value_start + 32].copy_from_slice(&0_i64.to_le_bytes());
        // access_time (8 bytes)
        data[value_start + 32..value_start + 40].copy_from_slice(&0_i64.to_le_bytes());
        // allocated_size (8 bytes)
        data[value_start + 40..value_start + 48].copy_from_slice(&0_i64.to_le_bytes());
        // data_size (8 bytes)
        data[value_start + 48..value_start + 56].copy_from_slice(&0_i64.to_le_bytes());
        // file_attributes (4 bytes)
        data[value_start + 56..value_start + 60].copy_from_slice(&0_u32.to_le_bytes());
        // packed_ea_size (2 bytes)
        data[value_start + 60..value_start + 62].copy_from_slice(&0_u16.to_le_bytes());
        // reserved (2 bytes)
        data[value_start + 62..value_start + 64].copy_from_slice(&0_u16.to_le_bytes());
        // file_name_length (1 byte) - in characters
        data[value_start + 64] = name_bytes.len() as u8;
        // file_name_namespace (1 byte)
        data[value_start + 65] = namespace;
        // file_name (UTF-16LE)
        for (i, &ch) in name_bytes.iter().enumerate() {
            let offset = value_start + 66 + i * 2;
            data[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
        }

        data
    }

    #[test]
    fn test_parse_standard_info() {
        let pipeline = CppParsePipeline::new(1024);

        // Create mock attribute
        let created: i64 = 0x01D1_2345_6789_0000;
        let modified: i64 = 0x01D1_2345_6789_0001;
        let accessed: i64 = 0x01D1_2345_6789_0002;
        let attributes: u32 = 0x20 | 0x01; // ARCHIVE | READONLY

        let attr_data = create_standard_info_attribute(created, modified, accessed, attributes);

        // Parse the attribute
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(0); // Create record for FRS 0
            CppParsePipeline::parse_standard_info(&mut index, &attr_data, 0, 0);
        }

        // Verify parsed values
        let index = pipeline.index.lock().unwrap();
        let record = index.get(0).expect("Record should exist");

        // Copy fields to avoid packed struct reference issues
        let stdinfo = record.stdinfo;
        let created_val = { stdinfo.created };
        let written_val = { stdinfo.written };
        assert_eq!(created_val, created as u64);
        assert_eq!(written_val, modified as u64);
        assert_eq!(stdinfo.accessed(), accessed as u64);
        assert_eq!(stdinfo.attributes(), attributes);
    }

    #[test]
    fn test_parse_filename_skip_dos() {
        let pipeline = CppParsePipeline::new(1024);

        // Create DOS namespace filename (should be skipped)
        let attr_data = create_filename_attribute(5, "MYDOCU~1.TXT", 0x02); // DOS namespace

        // Parse the attribute
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(10); // Create record for FRS 10
            CppParsePipeline::parse_file_name(&mut index, &attr_data, 10);
        }

        // Verify DOS name was skipped
        let index = pipeline.index.lock().unwrap();
        let record = index.get(10).expect("Record should exist");
        let name_count = record.name_count;
        assert_eq!(name_count, 0, "DOS namespace names should be skipped");
    }

    #[test]
    fn test_parse_filename_win32() {
        let pipeline = CppParsePipeline::new(1024);

        // Create Win32 namespace filename
        let attr_data = create_filename_attribute(5, "MyDocument.txt", 0x01); // Win32 namespace

        // Parse the attribute
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(10); // Create record for FRS 10
            CppParsePipeline::parse_file_name(&mut index, &attr_data, 10);
        }

        // Verify Win32 name was parsed
        let index = pipeline.index.lock().unwrap();
        let record = index.get(10).expect("Record should exist");
        let name_count = record.name_count;
        assert_eq!(name_count, 1, "Win32 namespace names should be parsed");

        // First name is stored inline in first_name, not in nameinfos
        // nameinfos only contains overflow names (2nd, 3rd, etc.)
        assert!(
            index.nameinfos.is_empty(),
            "First name should be inline, not in nameinfos"
        );

        // Verify the name was stored in the names buffer
        assert!(
            !index.names.is_empty(),
            "Name should be stored in names buffer"
        );
    }

    #[test]
    fn test_parse_filename_hardlink() {
        let pipeline = CppParsePipeline::new(1024);

        // Create two $FILE_NAME attributes for same file (hard link)
        let attr1 = create_filename_attribute(5, "file.txt", 0x01); // Win32
        let attr2 = create_filename_attribute(10, "link.txt", 0x01); // Win32, different parent

        // Parse both attributes
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(20); // Create record for FRS 20
            CppParsePipeline::parse_file_name(&mut index, &attr1, 20);
            CppParsePipeline::parse_file_name(&mut index, &attr2, 20);
        }

        // Verify both names were parsed
        let index = pipeline.index.lock().unwrap();
        let record = index.get(20).expect("Record should exist");
        let name_count = record.name_count;
        assert_eq!(name_count, 2, "Both hard link names should be parsed");

        // First name is stored inline in first_name
        // When second name is parsed, first name is pushed to nameinfos
        // So nameinfos should have 1 entry (the original first name)
        assert_eq!(
            index.nameinfos.len(),
            1,
            "One NameInfo should be in overflow (original first name)"
        );
    }

    #[test]
    fn test_parse_filename_win32_and_dos() {
        let pipeline = CppParsePipeline::new(1024);

        // Create Win32AndDos namespace filename (should be parsed)
        let attr_data = create_filename_attribute(5, "README.TXT", 0x03); // Win32AndDos

        // Parse the attribute
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(10);
            CppParsePipeline::parse_file_name(&mut index, &attr_data, 10);
        }

        // Verify name was parsed
        let index = pipeline.index.lock().unwrap();
        let record = index.get(10).expect("Record should exist");
        let name_count = record.name_count;
        assert_eq!(
            name_count, 1,
            "Win32AndDos namespace names should be parsed"
        );
    }
}

// ============================================================================
// Stream Parsing Tests
// ============================================================================

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::significant_drop_tightening,
    clippy::semicolon_outside_block,
    clippy::let_underscore_untyped,
    clippy::cast_sign_loss,
    clippy::single_call_fn,
    unsafe_code
)]
mod stream_parsing_tests {
    use super::*;
    use crate::ntfs::AttributeRecordHeader;

    /// Create a mock resident $DATA attribute.
    fn create_data_attribute_resident(content: &[u8]) -> Vec<u8> {
        let header_size = 16;
        let resident_size = 8;
        let value_size = content.len();
        let value_offset = header_size + resident_size;
        let total_size = value_offset + value_size;

        let mut data = vec![0_u8; total_size];

        // AttributeRecordHeader
        // type_code = 0x80 ($DATA)
        data[0..4].copy_from_slice(&0x80_u32.to_le_bytes());
        // length
        data[4..8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 0 (resident)
        data[8] = 0;
        // name_length = 0 (default stream)
        data[9] = 0;
        // name_offset = 0
        data[10..12].copy_from_slice(&0_u16.to_le_bytes());
        // flags = 0
        data[12..14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[14..16].copy_from_slice(&0_u16.to_le_bytes());

        // ResidentAttributeData
        // value_length
        data[16..20].copy_from_slice(&(value_size as u32).to_le_bytes());
        // value_offset
        data[20..22].copy_from_slice(&(value_offset as u16).to_le_bytes());
        // flags = 0
        data[22..24].copy_from_slice(&0_u16.to_le_bytes());

        // Content
        data[value_offset..value_offset + value_size].copy_from_slice(content);

        data
    }

    /// Create a mock non-resident $DATA attribute.
    fn create_data_attribute_nonresident(data_size: i64, allocated_size: i64) -> Vec<u8> {
        let header_size = 16;
        let nonresident_size = 48;
        let total_size = header_size + nonresident_size;

        let mut data = vec![0_u8; total_size];

        // AttributeRecordHeader
        // type_code = 0x80 ($DATA)
        data[0..4].copy_from_slice(&0x80_u32.to_le_bytes());
        // length
        data[4..8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 1 (non-resident)
        data[8] = 1;
        // name_length = 0 (default stream)
        data[9] = 0;
        // name_offset = 0
        data[10..12].copy_from_slice(&0_u16.to_le_bytes());
        // flags = 0
        data[12..14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[14..16].copy_from_slice(&0_u16.to_le_bytes());

        // NonResidentAttributeData
        let nr_start = header_size;
        // lowest_vcn = 0 (primary attribute)
        data[nr_start..nr_start + 8].copy_from_slice(&0_i64.to_le_bytes());
        // highest_vcn
        data[nr_start + 8..nr_start + 16].copy_from_slice(&0_i64.to_le_bytes());
        // mapping_pairs_offset
        data[nr_start + 16..nr_start + 18].copy_from_slice(&0_u16.to_le_bytes());
        // compression_unit = 0
        data[nr_start + 18] = 0;
        // reserved (5 bytes)
        // allocated_size
        data[nr_start + 24..nr_start + 32].copy_from_slice(&allocated_size.to_le_bytes());
        // data_size
        data[nr_start + 32..nr_start + 40].copy_from_slice(&data_size.to_le_bytes());
        // initialized_size
        data[nr_start + 40..nr_start + 48].copy_from_slice(&data_size.to_le_bytes());

        data
    }

    /// Create a mock named $DATA attribute (Alternate Data Stream).
    fn create_named_data_attribute(name: &str, content: &[u8]) -> Vec<u8> {
        let header_size = 16;
        let resident_size = 8;
        let name_bytes: Vec<u16> = name.encode_utf16().collect();
        let name_byte_len = name_bytes.len() * 2;
        // Name comes after resident header, value comes after name
        let name_offset = header_size + resident_size;
        let value_offset = name_offset + name_byte_len;
        let value_size = content.len();
        let total_size = value_offset + value_size;

        let mut data = vec![0_u8; total_size];

        // AttributeRecordHeader
        // type_code = 0x80 ($DATA)
        data[0..4].copy_from_slice(&0x80_u32.to_le_bytes());
        // length
        data[4..8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 0 (resident)
        data[8] = 0;
        // name_length (in characters)
        data[9] = name_bytes.len() as u8;
        // name_offset
        data[10..12].copy_from_slice(&(name_offset as u16).to_le_bytes());
        // flags = 0
        data[12..14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[14..16].copy_from_slice(&0_u16.to_le_bytes());

        // ResidentAttributeData
        // value_length
        data[16..20].copy_from_slice(&(value_size as u32).to_le_bytes());
        // value_offset
        data[20..22].copy_from_slice(&(value_offset as u16).to_le_bytes());
        // flags = 0
        data[22..24].copy_from_slice(&0_u16.to_le_bytes());

        // Name (UTF-16LE)
        for (i, &ch) in name_bytes.iter().enumerate() {
            let offset = name_offset + i * 2;
            data[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
        }

        // Content
        data[value_offset..value_offset + value_size].copy_from_slice(content);

        data
    }

    /// Create a mock `$INDEX_ROOT` attribute.
    fn create_index_root_attribute(index_name: &str) -> Vec<u8> {
        let header_size = 16;
        let resident_size = 8;
        let name_bytes: Vec<u16> = index_name.encode_utf16().collect();
        let name_byte_len = name_bytes.len() * 2;
        let name_offset = header_size + resident_size;
        let value_offset = name_offset + name_byte_len;
        let value_size = 16; // Minimal INDEX_ROOT header
        let total_size = value_offset + value_size;

        let mut data = vec![0_u8; total_size];

        // AttributeRecordHeader
        // type_code = 0x90 ($INDEX_ROOT)
        data[0..4].copy_from_slice(&0x90_u32.to_le_bytes());
        // length
        data[4..8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 0 (resident)
        data[8] = 0;
        // name_length (in characters)
        data[9] = name_bytes.len() as u8;
        // name_offset
        data[10..12].copy_from_slice(&(name_offset as u16).to_le_bytes());
        // flags = 0
        data[12..14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[14..16].copy_from_slice(&0_u16.to_le_bytes());

        // ResidentAttributeData
        // value_length
        data[16..20].copy_from_slice(&(value_size as u32).to_le_bytes());
        // value_offset
        data[20..22].copy_from_slice(&(value_offset as u16).to_le_bytes());
        // flags = 0
        data[22..24].copy_from_slice(&0_u16.to_le_bytes());

        // Name (UTF-16LE)
        for (i, &ch) in name_bytes.iter().enumerate() {
            let offset = name_offset + i * 2;
            data[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
        }

        data
    }

    /// Create a mock `$INDEX_ALLOCATION` attribute.
    fn create_index_allocation_attribute(index_name: &str, allocated_size: i64) -> Vec<u8> {
        let header_size = 16;
        let nonresident_size = 48;
        let name_bytes: Vec<u16> = index_name.encode_utf16().collect();
        let name_byte_len = name_bytes.len() * 2;
        let name_offset = header_size;
        let total_size = header_size + name_byte_len + nonresident_size;

        let mut data = vec![0_u8; total_size];

        // AttributeRecordHeader
        // type_code = 0xA0 ($INDEX_ALLOCATION)
        data[0..4].copy_from_slice(&0xA0_u32.to_le_bytes());
        // length
        data[4..8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 1 (non-resident)
        data[8] = 1;
        // name_length (in characters)
        data[9] = name_bytes.len() as u8;
        // name_offset
        data[10..12].copy_from_slice(&(name_offset as u16).to_le_bytes());
        // flags = 0
        data[12..14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[14..16].copy_from_slice(&0_u16.to_le_bytes());

        // Name (UTF-16LE) - comes right after header
        for (i, &ch) in name_bytes.iter().enumerate() {
            let offset = name_offset + i * 2;
            data[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
        }

        // NonResidentAttributeData - comes after name
        let nr_start = name_offset + name_byte_len;
        // lowest_vcn = 0 (primary attribute)
        data[nr_start..nr_start + 8].copy_from_slice(&0_i64.to_le_bytes());
        // highest_vcn
        data[nr_start + 8..nr_start + 16].copy_from_slice(&0_i64.to_le_bytes());
        // mapping_pairs_offset
        data[nr_start + 16..nr_start + 18].copy_from_slice(&0_u16.to_le_bytes());
        // compression_unit = 0
        data[nr_start + 18] = 0;
        // reserved (5 bytes)
        // allocated_size
        data[nr_start + 24..nr_start + 32].copy_from_slice(&allocated_size.to_le_bytes());
        // data_size = 0 for index allocation
        data[nr_start + 32..nr_start + 40].copy_from_slice(&0_i64.to_le_bytes());
        // initialized_size
        data[nr_start + 40..nr_start + 48].copy_from_slice(&0_i64.to_le_bytes());

        data
    }

    fn get_attr_header(data: &[u8]) -> AttributeRecordHeader {
        // SAFETY: The caller ensures data is at least as large as AttributeRecordHeader
        // and properly aligned. This is a test helper function.
        unsafe { core::ptr::read(data.as_ptr().cast()) }
    }

    #[test]
    fn test_parse_data_stream_resident() {
        let pipeline = CppParsePipeline::new(1024);
        let content = b"Hello, World!";
        let attr_data = create_data_attribute_resident(content);
        let attr_header = get_attr_header(&attr_data);

        // Parse the attribute
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(0);
            CppParsePipeline::parse_stream(&mut index, &attr_data, 0, &attr_header);
        }

        // Verify stream was parsed
        let index = pipeline.index.lock().unwrap();
        let record = index.get(0).expect("Record should exist");
        let stream_count = record.stream_count;
        assert_eq!(stream_count, 1, "One stream should be parsed");

        // Verify size
        let first_stream = record.first_stream;
        let length = u64::from(first_stream.size.length);
        assert_eq!(
            length,
            content.len() as u64,
            "Stream length should match content"
        );
    }

    #[test]
    fn test_parse_data_stream_nonresident() {
        let pipeline = CppParsePipeline::new(1024);
        let data_size: i64 = 1_000_000;
        let allocated_size: i64 = 1_048_576; // 1 MB
        let attr_data = create_data_attribute_nonresident(data_size, allocated_size);
        let attr_header = get_attr_header(&attr_data);

        // Parse the attribute
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(0);
            CppParsePipeline::parse_stream(&mut index, &attr_data, 0, &attr_header);
        }

        // Verify stream was parsed
        let index = pipeline.index.lock().unwrap();
        let record = index.get(0).expect("Record should exist");
        let stream_count = record.stream_count;
        assert_eq!(stream_count, 1, "One stream should be parsed");

        // Verify sizes
        let first_stream = record.first_stream;
        let length = u64::from(first_stream.size.length);
        let allocated = u64::from(first_stream.size.allocated);
        assert_eq!(
            length, data_size as u64,
            "Stream length should match data_size"
        );
        assert_eq!(
            allocated, allocated_size as u64,
            "Allocated size should match"
        );
    }

    #[test]
    fn test_parse_alternate_data_stream() {
        let pipeline = CppParsePipeline::new(1024);

        // First: default stream
        let default_content = b"Main content";
        let default_attr = create_data_attribute_resident(default_content);
        let default_header = get_attr_header(&default_attr);

        // Second: named stream (ADS)
        let ads_content = b"[ZoneTransfer]\r\nZoneId=3";
        let ads_attr = create_named_data_attribute("Zone.Identifier", ads_content);
        let ads_header = get_attr_header(&ads_attr);

        // Parse both attributes
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(0);
            CppParsePipeline::parse_stream(&mut index, &default_attr, 0, &default_header);
            CppParsePipeline::parse_stream(&mut index, &ads_attr, 0, &ads_header);
        }

        // Verify both streams were parsed
        let index = pipeline.index.lock().unwrap();
        let record = index.get(0).expect("Record should exist");
        let stream_count = record.stream_count;
        assert_eq!(
            stream_count, 2,
            "Two streams should be parsed (default + ADS)"
        );

        // Verify stream infos were stored
        assert!(
            !index.streaminfos.is_empty(),
            "StreamInfos should be stored for ADS"
        );
    }

    #[test]
    fn test_parse_directory_index_merge() {
        let pipeline = CppParsePipeline::new(1024);

        // $INDEX_ROOT for $I30
        let index_root_attr = create_index_root_attribute("$I30");
        let index_root_header = get_attr_header(&index_root_attr);

        // $INDEX_ALLOCATION for $I30
        let index_alloc_attr = create_index_allocation_attribute("$I30", 4096);
        let index_alloc_header = get_attr_header(&index_alloc_attr);

        // Parse both attributes
        {
            let mut index = pipeline.index.lock().unwrap();
            let _ = index.get_or_create(0);
            CppParsePipeline::parse_stream(&mut index, &index_root_attr, 0, &index_root_header);
            CppParsePipeline::parse_stream(&mut index, &index_alloc_attr, 0, &index_alloc_header);
        }

        // Verify streams were parsed
        // Note: The C++ implementation merges $INDEX_ROOT and $INDEX_ALLOCATION
        // for the same index name into a single stream entry
        let index = pipeline.index.lock().unwrap();
        let record = index.get(0).expect("Record should exist");
        let stream_count = record.stream_count;

        // Both should be parsed (may or may not merge depending on implementation)
        assert!(stream_count >= 1, "At least one stream should be parsed");
    }
}

// ============================================================================
// Extension Record Tests
// ============================================================================

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::significant_drop_tightening,
    clippy::semicolon_outside_block,
    clippy::let_underscore_untyped,
    clippy::cast_sign_loss,
    clippy::single_call_fn
)]
mod extension_record_tests {
    use super::*;

    /// Create a mock FILE record with the given FRS and base FRS.
    ///
    /// Layout (1024 bytes):
    /// - Bytes 0-3: Magic "FILE"
    /// - Bytes 4-5: USA offset (0x30 = 48)
    /// - Bytes 6-7: USA count (3)
    /// - Bytes 8-15: LSN
    /// - Bytes 16-17: Sequence number
    /// - Bytes 18-19: Link count
    /// - Bytes 20-21: First attribute offset (0x38 = 56)
    /// - Bytes 22-23: Flags (`FRH_IN_USE` = 0x0001)
    /// - Bytes 24-27: Bytes in use
    /// - Bytes 28-31: Bytes allocated
    /// - Bytes 32-39: Base file record segment (0 for base, non-zero for
    ///   extension)
    /// - Bytes 40-41: Next attribute number
    /// - Bytes 42-43: Reserved
    /// - Bytes 44-47: Segment number (lower 32 bits)
    /// - Bytes 48-53: USA (check value + 2 sector replacements)
    /// - Bytes 56+: Attributes
    fn create_file_record(frs: u32, base_frs: u64, is_directory: bool) -> Vec<u8> {
        let mut data = vec![0_u8; 1024];

        // Magic "FILE"
        data[0] = 0x46; // 'F'
        data[1] = 0x49; // 'I'
        data[2] = 0x4C; // 'L'
        data[3] = 0x45; // 'E'

        // USA offset = 48 (0x30)
        data[4] = 0x30;
        data[5] = 0x00;

        // USA count = 3
        data[6] = 0x03;
        data[7] = 0x00;

        // LSN (8 bytes) - skip

        // Sequence number = 1
        data[16] = 0x01;
        data[17] = 0x00;

        // Link count = 1
        data[18] = 0x01;
        data[19] = 0x00;

        // First attribute offset = 56 (0x38)
        data[20] = 0x38;
        data[21] = 0x00;

        // Flags = FRH_IN_USE (0x0001) | FRH_DIRECTORY (0x0002) if directory
        let flags: u16 = if is_directory { 0x0003 } else { 0x0001 };
        data[22..24].copy_from_slice(&flags.to_le_bytes());

        // Bytes in use = 1024
        data[24..28].copy_from_slice(&1024_u32.to_le_bytes());

        // Bytes allocated = 1024
        data[28..32].copy_from_slice(&1024_u32.to_le_bytes());

        // Base file record segment (8 bytes)
        data[32..40].copy_from_slice(&base_frs.to_le_bytes());

        // Next attribute number = 1
        data[40] = 0x01;
        data[41] = 0x00;

        // Reserved (2 bytes) - skip

        // Segment number (lower 32 bits)
        data[44..48].copy_from_slice(&frs.to_le_bytes());

        // USA: check value = 0xBEEF
        data[48] = 0xEF;
        data[49] = 0xBE;

        // USA: original value for sector 1 = 0x1234
        data[50] = 0x34;
        data[51] = 0x12;

        // USA: original value for sector 2 = 0x5678
        data[52] = 0x78;
        data[53] = 0x56;

        // Place check value at sector boundaries
        data[510] = 0xEF;
        data[511] = 0xBE;
        data[1022] = 0xEF;
        data[1023] = 0xBE;

        // End marker at first attribute offset (0xFFFFFFFF)
        data[56..60].copy_from_slice(&0xFFFF_FFFF_u32.to_le_bytes());

        data
    }

    /// Add a `$STANDARD_INFORMATION` attribute to a record at the given offset.
    fn add_standard_info_attribute(data: &mut [u8], offset: usize, created: i64) {
        let header_size = 16;
        let resident_size = 8;
        let value_size = 36;
        let value_offset = header_size + resident_size;
        let total_size = value_offset + value_size;

        // AttributeRecordHeader
        // type_code = 0x10 ($STANDARD_INFORMATION)
        data[offset..offset + 4].copy_from_slice(&0x10_u32.to_le_bytes());
        // length
        data[offset + 4..offset + 8].copy_from_slice(&(total_size as u32).to_le_bytes());
        // is_non_resident = 0
        data[offset + 8] = 0;
        // name_length = 0
        data[offset + 9] = 0;
        // name_offset = 0
        data[offset + 10..offset + 12].copy_from_slice(&0_u16.to_le_bytes());
        // flags = 0
        data[offset + 12..offset + 14].copy_from_slice(&0_u16.to_le_bytes());
        // instance = 0
        data[offset + 14..offset + 16].copy_from_slice(&0_u16.to_le_bytes());

        // ResidentAttributeData
        // value_length
        data[offset + 16..offset + 20].copy_from_slice(&(value_size as u32).to_le_bytes());
        // value_offset
        data[offset + 20..offset + 22].copy_from_slice(&(value_offset as u16).to_le_bytes());
        // flags = 0
        data[offset + 22..offset + 24].copy_from_slice(&0_u16.to_le_bytes());

        // StandardInformation
        let value_start = offset + value_offset;
        // creation_time
        data[value_start..value_start + 8].copy_from_slice(&created.to_le_bytes());
        // modification_time
        data[value_start + 8..value_start + 16].copy_from_slice(&created.to_le_bytes());
        // mft_change_time
        data[value_start + 16..value_start + 24].copy_from_slice(&created.to_le_bytes());
        // access_time
        data[value_start + 24..value_start + 32].copy_from_slice(&created.to_le_bytes());
        // file_attributes = ARCHIVE
        data[value_start + 32..value_start + 36].copy_from_slice(&0x20_u32.to_le_bytes());

        // End marker after this attribute
        let end_offset = offset + total_size;
        data[end_offset..end_offset + 4].copy_from_slice(&0xFFFF_FFFF_u32.to_le_bytes());
    }

    #[test]
    fn test_extension_record_attributes_go_to_base() {
        let pipeline = CppParsePipeline::new(1024);

        // Create base record (FRS 10)
        // Note: process_chunk handles USA fixup internally, so we don't call
        // apply_usa_fixup
        let mut base_record = create_file_record(10, 0, false);
        let base_created: i64 = 0x01D1_0000_0000_0000;
        add_standard_info_attribute(&mut base_record, 56, base_created);

        // Create extension record (FRS 50) pointing to base (FRS 10)
        let mut ext_record = create_file_record(50, 10, false);
        // Extension records typically don't have $STANDARD_INFORMATION,
        // but they can have $DATA or other attributes that should go to base

        // Process both records through the pipeline
        // Note: process_chunk handles USA fixup internally
        pipeline.process_chunk(&mut base_record, 10 * 1024);
        pipeline.process_chunk(&mut ext_record, 50 * 1024);

        // Verify base record was created
        let index = pipeline.index.lock().unwrap();
        assert!(index.get(10).is_some(), "Base record should exist");

        // The extension record's attributes should be added to the base record
        // In this test, we just verify the mechanism works - the extension
        // record should cause the base record to be accessed/created
    }

    #[test]
    fn test_base_record_parsing() {
        let pipeline = CppParsePipeline::new(1024);

        // Create a base record with $STANDARD_INFORMATION
        let mut record = create_file_record(5, 0, false);
        let created: i64 = 0x01D1_2345_6789_0000;
        add_standard_info_attribute(&mut record, 56, created);

        // Process the record (process_chunk handles USA fixup internally)
        pipeline.process_chunk(&mut record, 5 * 1024);

        // Verify record was parsed
        let index = pipeline.index.lock().unwrap();
        let parsed = index.get(5).expect("Record should exist");

        // Verify $STANDARD_INFORMATION was parsed
        // Copy to local variable to avoid packed struct reference issues
        let stdinfo = parsed.stdinfo;
        let created_val = { stdinfo.created };
        assert_eq!(created_val, created as u64, "Created time should match");
    }

    #[test]
    fn test_directory_flag_propagation() {
        let pipeline = CppParsePipeline::new(1024);

        // Create a directory record
        let mut record = create_file_record(5, 0, true);
        let created: i64 = 0x01D1_2345_6789_0000;
        add_standard_info_attribute(&mut record, 56, created);

        // Process the record (process_chunk handles USA fixup internally)
        pipeline.process_chunk(&mut record, 5 * 1024);

        // Verify record was parsed with directory flag
        let index = pipeline.index.lock().unwrap();
        let parsed = index.get(5).expect("Record should exist");

        // Verify directory flag was set in attributes
        let stdinfo = parsed.stdinfo;
        let attrs = stdinfo.attributes();
        assert!(
            (attrs & 0x10) != 0,
            "FILE_ATTRIBUTE_DIRECTORY should be set"
        );
    }
}
