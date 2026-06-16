// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Canonical field identifiers and metadata for unified search semantics.

mod field_metadata;
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
    /// Whether this record's own leaf name is ill-formed (not valid UTF-8 —
    /// an unpaired UTF-16 surrogate). Computed against the lossless name bytes
    /// (WI-4.4); a forensic flag for names that have no valid UTF-8 form.
    Malformed,
    /// Whether ANY component of the record's full resolved path is ill-formed
    /// (so a clean-named file under a crooked directory is still flagged).
    /// Superset of [`Self::Malformed`].
    MalformedPath,
    /// Hex of the true (WTF-8) leaf-name bytes — the forensic evidence form for
    /// distinguishing ill-formed names that all display as U+FFFD. Projection
    /// only; never filtered or sorted.
    NameHex,
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

/// Canonical field kinds used by predicate compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// String-like values.
    String,
    /// Numeric values.
    Numeric,
    /// Timestamp values.
    Timestamp,
    /// Boolean values.
    Bool,
    /// Enumerated / categorized values.
    Enum,
    /// Bitmask-style values.
    Bitmask,
}

/// Where a field's value becomes available during execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldAccess {
    /// Available during the existing hot path with no additional
    /// materialization.
    Hot,
    /// Computed from hot data without extra disk I/O.
    Derived,
    /// Requires cold-path materialization from extra record data.
    Cold,
}

/// Default sort direction for a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    /// Ascending order.
    Ascending,
    /// Descending order.
    Descending,
}

/// Canonical metadata describing one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldMeta {
    /// The canonical identifier.
    pub id: FieldId,
    /// Canonical wire / config name.
    pub canonical_name: &'static str,
    /// Accepted aliases during parsing.
    pub aliases: &'static [&'static str],
    /// Logical field kind.
    pub field_type: FieldType,
    /// Availability tier for execution planning.
    pub access: FieldAccess,
    /// Whether the field should be sortable in the canonical model.
    pub sortable: bool,
    /// Preferred default sort direction when used in a sort spec.
    pub default_sort_direction: Option<SortDirection>,
    /// Whether the field should be filterable in the canonical model.
    pub filterable: bool,
    /// Whether the field can be projected in results.
    pub projectable: bool,
    /// Short TUI header label (e.g. "Drv", "Name", "Sz").
    pub tui_label: &'static str,
    /// Human-readable display name for CLI output headers.
    pub display_name: &'static str,
    /// Polars `DataFrame` column name (empty if not backed by a DF column).
    pub df_column: &'static str,
    /// Default fallback value when the column is missing from a `DataFrame`.
    pub default_value: &'static str,
    /// Aggregation capability metadata.
    pub aggregate: AggregateMeta,
}

impl FieldId {
    /// All currently known canonical fields.
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
        Self::Malformed,
        Self::MalformedPath,
        Self::NameHex,
    ];

    /// Parse a field name or alias into the canonical identifier.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        for &field in Self::ALL {
            let meta = field.metadata();
            if meta.canonical_name.eq_ignore_ascii_case(name)
                || meta
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(name))
            {
                return Some(field);
            }
        }
        None
    }

    /// Return canonical metadata for this field.
    ///
    /// Canonical wire/config name for this field.
    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        self.metadata().canonical_name
    }

    /// Preferred default sort direction for this field, when sortable.
    #[must_use]
    pub const fn default_sort_direction(self) -> Option<SortDirection> {
        self.metadata().default_sort_direction
    }

    /// Short TUI header label.
    #[must_use]
    pub const fn tui_label(self) -> &'static str {
        self.metadata().tui_label
    }

    /// Human-readable display name for CLI output headers.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        self.metadata().display_name
    }

    /// Polars `DataFrame` column name.
    #[must_use]
    pub const fn df_column(self) -> &'static str {
        self.metadata().df_column
    }

    /// Default fallback value when the column is missing from a DF.
    #[must_use]
    pub const fn default_value(self) -> &'static str {
        self.metadata().default_value
    }

    /// Whether this is a tree-derived metric field.
    #[must_use]
    pub const fn is_tree_field(self) -> bool {
        matches!(
            self,
            Self::Descendants | Self::TreeSize | Self::TreeAllocated | Self::Bulkiness
        )
    }

    /// Convert to a tree column if applicable.
    #[must_use]
    pub const fn to_tree_column(self) -> Option<crate::tree::TreeColumn> {
        match self {
            Self::Descendants => Some(crate::tree::TreeColumn::Descendants),
            Self::TreeSize => Some(crate::tree::TreeColumn::TreeSize),
            Self::TreeAllocated => Some(crate::tree::TreeColumn::TreeAllocated),
            Self::Bulkiness => Some(crate::tree::TreeColumn::Bulkiness),
            Self::Drive
            | Self::Path
            | Self::Name
            | Self::PathOnly
            | Self::Size
            | Self::SizeOnDisk
            | Self::Created
            | Self::Modified
            | Self::Accessed
            | Self::Extension
            | Self::Type
            | Self::Attributes
            | Self::AttributeValue
            | Self::Hidden
            | Self::System
            | Self::Archive
            | Self::ReadOnly
            | Self::Compressed
            | Self::Encrypted
            | Self::Sparse
            | Self::Reparse
            | Self::Offline
            | Self::NotIndexed
            | Self::Temporary
            | Self::Virtual
            | Self::Pinned
            | Self::Unpinned
            | Self::Integrity
            | Self::NoScrub
            | Self::DirectoryFlag
            | Self::RecallOnOpen
            | Self::RecallOnDataAccess
            | Self::ParityAttributes
            | Self::NameLength
            | Self::PathLength
            | Self::Malformed
            | Self::MalformedPath
            | Self::NameHex => None,
        }
    }

    /// Sortable fields in TUI cycle order.
    pub(crate) const SORT_CYCLE: &'static [Self] = &[
        Self::Name,
        Self::Size,
        Self::SizeOnDisk,
        Self::Created,
        Self::Modified,
        Self::Accessed,
        Self::Path,
        Self::Drive,
        Self::Extension,
        Self::Type,
        Self::Descendants,
        Self::TreeAllocated,
        Self::Bulkiness,
    ];

    /// Return the next sort field in the cycle, wrapping around.
    #[must_use]
    pub fn cycle_next(self) -> Self {
        let mut found = false;
        for &candidate in Self::SORT_CYCLE {
            if found {
                return candidate;
            }
            if candidate == self {
                found = true;
            }
        }
        // Wrap around or fall back to Name for non-cycle fields.
        Self::SORT_CYCLE.first().copied().unwrap_or(Self::Name)
    }

    /// Return the nearest sortable field for a non-sortable field.
    ///
    /// For example, attribute boolean columns map to `Name` sort,
    /// `PathOnly` maps to `Path`, `TreeSize` maps to `Size`.
    #[must_use]
    pub const fn nearest_sort_field(self) -> Self {
        match self {
            Self::Path | Self::PathOnly => Self::Path,
            Self::Size | Self::TreeSize => Self::Size,
            Self::SizeOnDisk => Self::SizeOnDisk,
            Self::Created => Self::Created,
            Self::Modified => Self::Modified,
            Self::Accessed => Self::Accessed,
            Self::Extension => Self::Extension,
            Self::Type => Self::Type,
            Self::Drive => Self::Drive,
            Self::Descendants => Self::Descendants,
            Self::TreeAllocated => Self::TreeAllocated,
            Self::Bulkiness => Self::Bulkiness,
            Self::NameLength => Self::NameLength,
            Self::PathLength => Self::PathLength,
            Self::Name
            | Self::Attributes
            | Self::AttributeValue
            | Self::Hidden
            | Self::System
            | Self::Archive
            | Self::ReadOnly
            | Self::Compressed
            | Self::Encrypted
            | Self::Sparse
            | Self::Reparse
            | Self::Offline
            | Self::NotIndexed
            | Self::Temporary
            | Self::Virtual
            | Self::Pinned
            | Self::Unpinned
            | Self::Integrity
            | Self::NoScrub
            | Self::DirectoryFlag
            | Self::RecallOnOpen
            | Self::RecallOnDataAccess
            | Self::ParityAttributes
            | Self::Malformed
            | Self::MalformedPath
            | Self::NameHex => Self::Name,
        }
    }
}
