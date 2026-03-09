//! Core index container and child-link metadata.

use super::*;

/// Directory child entry.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ChildInfo {
    /// Index of next `ChildInfo` in `MftIndex::children`, or `NO_ENTRY`.
    pub next_entry: u32,
    /// FRS of the child file or directory.
    pub child_frs: u64,
    /// Which name index to use for hard links.
    pub name_index: u16,
}

/// Lean in-memory MFT index used by the parser and query layers.
#[derive(Debug, Default)]
pub struct MftIndex {
    /// Volume letter (e.g., 'C').
    pub volume: char,
    /// All file and directory records.
    pub records: Vec<FileRecord>,
    /// FRS → record index lookup (O(1) access).
    pub frs_to_idx: Vec<u32>,
    /// All filenames concatenated into one allocation.
    pub names: String,
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
