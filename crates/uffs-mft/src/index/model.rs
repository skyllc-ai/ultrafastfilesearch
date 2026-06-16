// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Core index container and child-link metadata.

use super::{
    ExtensionIndex, ExtensionTable, FileRecord, IndexStreamInfo, InternalStreamInfo, LinkInfo,
    MftStats,
};
use crate::frs::Frs;
use crate::platform::DriveLetter;

/// Directory child entry.
///
/// 24 bytes per entry (with explicit padding).  Derives `bytemuck::Pod`
/// so the entire children array can be serialized/deserialized as a single
/// `memcpy` (v11+).
///
/// The [`Frs`] newtype is `#[repr(transparent)]` over `u64`, so the on-disk
/// layout is byte-identical to the historic `u64` `child_frs` field.
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct ChildInfo {
    /// Index of next `ChildInfo` in `MftIndex::children`, or `NO_ENTRY`.
    pub next_entry: u32,
    /// Explicit padding for `u64` alignment of `child_frs`.
    #[expect(
        clippy::pub_underscore_fields,
        reason = "bytemuck Pod requires all fields same visibility"
    )]
    pub _pad0: [u8; 4],
    /// FRS of the child file or directory (typed [`Frs`]; bit-identical
    /// to `u64` on disk).
    pub child_frs: Frs,
    /// Which name index to use for hard links.
    pub name_index: u16,
    /// Explicit tail padding for struct alignment (8-byte boundary).
    #[expect(
        clippy::pub_underscore_fields,
        reason = "bytemuck Pod requires all fields same visibility"
    )]
    pub _pad1: [u8; 6],
}

/// Lean in-memory MFT index used by the parser and query layers.
///
/// `Default` is implemented manually rather than derived because
/// [`DriveLetter`] has no canonical default — `Default::default()`
/// supplies [`DriveLetter::C`] as a placeholder and the four
/// production constructors ([`MftIndex::new`], `with_capacity`,
/// `with_capacity_optimized`, plus the test fixtures in
/// `index/tests_ads.rs`) all overwrite it with the caller's actual
/// volume immediately after the struct-update.
#[derive(Debug)]
pub struct MftIndex {
    /// Volume letter (e.g., `C`).
    pub volume: DriveLetter,
    /// All file and directory records.
    pub records: Vec<FileRecord>,
    /// FRS → record index lookup (O(1) access).
    pub frs_to_idx: Vec<u32>,
    /// All filenames concatenated into one allocation, stored as **WTF-8
    /// bytes** (not a `String`).
    ///
    /// NTFS names are UTF-16 with no well-formedness guarantee — unpaired
    /// surrogates are legal on disk. Holding the *raw* bytes (WTF-8: UTF-8 for
    /// well-formed names, plus the surrogate-bearing remainder encoded
    /// byte-faithfully) makes every real on-disk name retainable and findable
    /// — so a file cannot hide from search behind an ill-formed name
    /// (WI-4.4, Category 4). Access via [`MftIndex::get_name`] (a lossy `&str`
    /// view for display, U+FFFD-rendered) or [`MftIndex::get_name_bytes`] (the
    /// lossless bytes, used by the byte-native search/trigram path).
    pub names: Vec<u8>,
    /// Overflow hard-link entries.
    pub links: Vec<LinkInfo>,
    /// Overflow stream entries.
    pub streams: Vec<IndexStreamInfo>,
    /// Internal NTFS streams filtered from user-visible output but retained for
    /// tree metrics.
    pub internal_streams: Vec<InternalStreamInfo>,
    /// Directory child entries.
    pub children: Vec<ChildInfo>,
    /// Statistics collected during parsing.
    pub stats: MftStats,
    /// Extension interning table for O(1) lookups and statistics.
    pub extensions: ExtensionTable,
    /// Extension index for O(matches) queries (built after parsing).
    pub extension_index: Option<ExtensionIndex>,
    /// Whether this index was built with forensic mode enabled.
    pub forensic_mode: bool,
    /// Bytes of NTFS reserved clusters to add to the root directory's
    /// `tree_allocated`.
    ///
    /// Computed as `(TotalReserved + MftZoneEnd - MftZoneStart) *
    /// BytesPerCluster` and added to the root's children allocated at
    /// depth 0. Without this adjustment the root `Size on Disk` will be
    /// off by this amount.
    pub reserved_allocated_bytes: u64,
    /// Monotonically increasing epoch (Unix microseconds) stamped on every
    /// build or mutation (e.g. USN update).  Downstream caches (compact
    /// index) compare their `source_epoch` against this to detect staleness.
    pub build_epoch: u64,
}

impl Default for MftIndex {
    /// Placeholder index used as the `..Default::default()` spread source
    /// inside [`MftIndex::new`].  The `volume` field is supplied as
    /// [`DriveLetter::C`] only to satisfy the type system — every public
    /// constructor overwrites it with the caller-provided letter
    /// immediately after the struct-update.
    fn default() -> Self {
        Self {
            volume: DriveLetter::C,
            records: Vec::new(),
            frs_to_idx: Vec::new(),
            names: Vec::new(),
            links: Vec::new(),
            streams: Vec::new(),
            internal_streams: Vec::new(),
            children: Vec::new(),
            stats: MftStats::default(),
            extensions: ExtensionTable::default(),
            extension_index: None,
            forensic_mode: false,
            reserved_allocated_bytes: 0,
            build_epoch: 0,
        }
    }
}

/// Proportional hard-link size division formula used by tree metrics.
#[inline]
#[expect(dead_code, reason = "utility for future hardlink size attribution")]
fn hardlink_delta(value: u64, name_info: u16, total_names: u16) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let i = u64::from(name_info);
    let n = u64::from(total_names);
    (value * (i + 1) / n) - (value * i / n)
}
