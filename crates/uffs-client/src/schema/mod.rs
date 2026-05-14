// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Canonical field identifiers and metadata for unified search semantics.
//!
//! This module defines the schema for UFFS search fields — the set of
//! columns, their types, aliases, display names, and aggregation capabilities.
//! These are pure value types with zero polars dependency, suitable for use
//! in both the thin CLI and the daemon.
//!
//! # Field / `#[non_exhaustive]` policy (Phase 3b §3.4 / §3.6)
//!
//! Every `pub struct` here (`FieldMeta`, `AggregateMeta`) is a
//! **schema-metadata DTO** with `pub` fields by design — callers
//! struct-literal-construct one entry per `FieldId` variant in
//! `field_metadata::FIELD_META`.  Adding `#[non_exhaustive]` would
//! force ~40 entries to be rewritten through a builder.
//!
//! Every `pub enum` here (`FieldId`, `Cardinality`, `FieldType`,
//! `FieldAccess`, `SortDirection`) is **exhaustively matched** at
//! hundreds of consumer sites — the exhaustiveness check is the
//! compile-time guarantee that every field has display logic, every
//! type has aggregation rules, every direction has a sort
//! implementation, etc.  This is the playbook §3.6 "state-machine /
//! dispatch enum" exception to applying `#[non_exhaustive]`.
//!
//! **Verdict:** Kept exhaustive workspace-wide; revisit when
//! `uffs-client::schema` is split into an externally-publishable
//! crate (today blocked by Polars dep on `FieldId` consumers in
//! `uffs-core`).
//!
//! No `pub trait` declarations here, so §3.7 is **N/A**.

pub mod field_metadata;
#[cfg(test)]
mod field_tests;

/// Canonical field identifier shared across filter, sort, and projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldId {
    /// Drive letter.
    Drive,
    /// Full resolved path.
    Path,
    /// Filename only.
    Name,
    /// Parent directory path without filename.
    PathOnly,
    /// Logical file size.
    Size,
    /// Allocated size on disk.
    SizeOnDisk,
    /// Creation timestamp.
    Created,
    /// Last-written timestamp.
    Modified,
    /// Last-access timestamp.
    Accessed,
    /// Filename extension.
    Extension,
    /// File category / type.
    Type,
    /// Formatted attribute set.
    Attributes,
    /// Raw attribute value.
    AttributeValue,
    /// Hidden flag.
    Hidden,
    /// System flag.
    System,
    /// Archive flag.
    Archive,
    /// Read-only flag.
    ReadOnly,
    /// Compressed flag.
    Compressed,
    /// Encrypted flag.
    Encrypted,
    /// Sparse flag.
    Sparse,
    /// Reparse-point flag.
    Reparse,
    /// Offline flag.
    Offline,
    /// Not-content-indexed flag.
    NotIndexed,
    /// Temporary flag.
    Temporary,
    /// Virtual flag.
    Virtual,
    /// Pinned flag.
    Pinned,
    /// Unpinned flag.
    Unpinned,
    /// Descendant count.
    Descendants,
    /// Aggregate logical subtree size.
    TreeSize,
    /// Aggregate allocated subtree size.
    TreeAllocated,
    /// Tree allocation / logical size ratio.
    Bulkiness,
    /// Integrity-stream flag.
    Integrity,
    /// No-scrub-data flag.
    NoScrub,
    /// Directory boolean flag.
    DirectoryFlag,
    /// Recall-on-open flag.
    RecallOnOpen,
    /// Recall-on-data-access flag.
    RecallOnDataAccess,
    /// Legacy parity-masked attribute value.
    ParityAttributes,
    /// Filename length in characters.
    NameLength,
    /// Full-path length in characters.
    PathLength,
}

/// Cardinality hint for aggregation planning.
///
/// Tells the aggregation engine what kind of accumulator to expect
/// when this field is used as a group-by key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// Tiny, fixed set of values (≤ 26). Use array-indexed accumulator.
    /// Examples: drive letter, bool attrs, directory flag.
    Fixed,
    /// Small value space (≤ ~100). Use small `HashMap`.
    /// Examples: semantic type / `FileCategory` (~24 variants).
    Low,
    /// Medium value space (≤ ~10 000). Use `HashMap`.
    /// Examples: file extensions (~2 000 on a typical system).
    Medium,
    /// Large value space (≤ ~1 000 000). Guard with top-N + `other_count`.
    /// Examples: folder paths, directory names.
    High,
    /// Potentially millions of distinct values. Only aggregate on explicit
    /// request. Examples: full path, file name (for duplicate detection).
    Unbounded,
}

/// Aggregation capability metadata for a single field.
///
/// Populated on every [`FieldId`] and read by the aggregation engine to
/// decide what operations are valid, which accumulator strategy to use,
/// and how to finalize/sort buckets.
///
/// This struct is `Copy` and const-constructable so it can be returned
/// from `FieldId::metadata()` (a `const fn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggregateMeta {
    /// Can numeric aggregate functions (sum, min, max, avg) be applied?
    /// True for numeric and timestamp fields.
    pub aggregatable: bool,
    /// Can this field be used as a group-by / terms key?
    /// True for enum, string, and bool fields.
    pub groupable: bool,
    /// Can this field be bucketed into ranges or histograms?
    /// True for numeric and timestamp fields.
    pub bucket_support: bool,
    /// Expected number-of-distinct-values hint.
    pub cardinality: Cardinality,
    /// Default top-N limit when used as a terms aggregation key.
    /// 0 means "not suitable for terms by default — use histogram/range
    /// instead".
    pub default_top: u16,
}

/// Semantic type of a field's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// Free-form text (filename, path, extension).
    String,
    /// Integer / unsigned numeric.
    Numeric,
    /// Date / time stamp.
    Timestamp,
    /// True / false.
    Bool,
    /// Fixed set of named values (drive letter, file-vs-dir).
    Enum,
    /// Bitfield / attribute mask.
    Bitmask,
}

/// Performance tier — how cheap a field is to evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldAccess {
    /// Stored directly in the compact index — O(1) column read.
    Hot,
    /// Computed on access (path resolution, extension extraction).
    Derived,
    /// Requires cold-path materialization from extra record data.
    Cold,
}

/// Sort direction used for ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    /// A→Z, 0→9.
    Ascending,
    /// Z→A, 9→0.
    Descending,
}

/// Compile-time descriptor for a single field variant.
///
/// Returned by [`FieldId::metadata()`].  Every field in `FieldMeta` is
/// `&'static str` or primitive — the whole struct is `Copy` and
/// const-constructable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldMeta {
    /// Back-reference to the owning variant.
    pub id: FieldId,
    /// Stable wire name used in protocol messages (`snake_case`).
    pub canonical_name: &'static str,
    /// Alternative names accepted during parsing.
    pub aliases: &'static [&'static str],
    /// Semantic type of the field's values.
    pub field_type: FieldType,
    /// Performance tier.
    pub access: FieldAccess,
    /// Whether results can be sorted by this field.
    pub sortable: bool,
    /// Default sort direction (e.g. size ↓, name ↑).
    pub default_sort_direction: Option<SortDirection>,
    /// Whether this field can appear in filter expressions.
    pub filterable: bool,
    /// Whether this field can appear in output projections.
    pub projectable: bool,
    /// Short label for TUI column headers.
    pub tui_label: &'static str,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Polars `DataFrame` column name (empty if not stored in DF).
    pub df_column: &'static str,
    /// Default value when the field is absent.
    pub default_value: &'static str,
    /// Aggregation capabilities.
    pub aggregate: AggregateMeta,
}

impl FieldId {
    /// Every variant in definition order.
    pub const ALL: &'static [Self] = &[
        Self::Drive,
        Self::Path,
        Self::Name,
        Self::PathOnly,
        Self::Size,
        Self::SizeOnDisk,
        Self::Created,
        Self::Modified,
        Self::Accessed,
        Self::Extension,
        Self::Type,
        Self::Attributes,
        Self::AttributeValue,
        Self::Hidden,
        Self::System,
        Self::Archive,
        Self::ReadOnly,
        Self::Compressed,
        Self::Encrypted,
        Self::Sparse,
        Self::Reparse,
        Self::Offline,
        Self::NotIndexed,
        Self::Temporary,
        Self::Virtual,
        Self::Pinned,
        Self::Unpinned,
        Self::Descendants,
        Self::TreeSize,
        Self::TreeAllocated,
        Self::Bulkiness,
        Self::Integrity,
        Self::NoScrub,
        Self::DirectoryFlag,
        Self::RecallOnOpen,
        Self::RecallOnDataAccess,
        Self::ParityAttributes,
        Self::NameLength,
        Self::PathLength,
    ];

    /// The number of variants in `FieldId`.
    pub const COUNT: usize = Self::ALL.len();

    /// Stable wire name used in protocol messages (e.g. `"size_on_disk"`).
    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        self.metadata().canonical_name
    }

    /// Human-readable display name (e.g. `"Size on Disk"`).
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        self.metadata().display_name
    }

    /// Parse a field name into a [`FieldId`], matching canonical names and
    /// aliases case-insensitively.
    ///
    /// Returns `None` if no variant matches.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let lower = input.to_ascii_lowercase();
        let trimmed = lower.trim();
        for &field in Self::ALL {
            let meta = field.metadata();
            if meta.canonical_name == trimmed {
                return Some(field);
            }
            for &alias in meta.aliases {
                if alias == trimmed {
                    return Some(field);
                }
            }
        }
        None
    }
}

impl core::fmt::Display for FieldId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.canonical_name())
    }
}
