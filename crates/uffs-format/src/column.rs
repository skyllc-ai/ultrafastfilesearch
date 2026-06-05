// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Output column enum + parse / display-name / alias resolver.
//!
//! This is a deliberate subset of `uffs_core::search::field::FieldId` —
//! the output formatter only needs canonical name, display name, and
//! aliases, so `uffs-format` carries just those three per variant.
//! The heavier metadata (`FieldType`, `AggregateMeta`, `FieldAccess`,
//! etc.) stays on the `FieldId` side where the search / aggregation
//! code actually consumes it.
//!
//! A regression test in `uffs-core` (`field_id_matches_output_column_*`)
//! pins that `FieldId` and `OutputColumn` never drift in variant set,
//! `canonical_name`, or `display_name`.

use serde::{Deserialize, Serialize};

/// Canonical output column identifier.
///
/// 1:1 with `uffs_core::search::field::FieldId::ALL` — every variant
/// here has an equally-named counterpart in core.
///
/// # `#[non_exhaustive]` decision (Phase 3b §3.6)
///
/// **Kept exhaustive.**  The exhaustive `match` in
/// `uffs_core::output::display_rows::write_display_row_columns` is the
/// compile-time safety net that guarantees every variant has display
/// logic.  Marking this `#[non_exhaustive]` would force a wildcard
/// arm (or `unreachable!()`), eliminating that guarantee.  When a new
/// column is added, the missing-arm compile error in that match site
/// is the desired feedback loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputColumn {
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
    /// Tree allocation / logical size ratio (×`1_000_000`).
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
    /// Leaf name is ill-formed (not valid UTF-8). WI-4.4 forensic flag.
    Malformed,
    /// Any path component is ill-formed. WI-4.4 forensic flag.
    MalformedPath,
    /// Hex of the true (WTF-8) leaf-name bytes. WI-4.4 forensic evidence.
    NameHex,
}

impl OutputColumn {
    /// Every output column, in declaration order.
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

    /// Canonical wire / config name — matches `FieldMeta::canonical_name`
    /// in `uffs-core`.
    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Drive => "drive",
            Self::Path => "path",
            Self::Name => "name",
            Self::PathOnly => "path_only",
            Self::Size => "size",
            Self::SizeOnDisk => "size_on_disk",
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Accessed => "accessed",
            Self::Extension => "extension",
            Self::Type => "type",
            Self::Attributes => "attributes",
            Self::AttributeValue => "attribute_value",
            Self::Hidden => "hidden",
            Self::System => "system",
            Self::Archive => "archive",
            Self::ReadOnly => "read_only",
            Self::Compressed => "compressed",
            Self::Encrypted => "encrypted",
            Self::Sparse => "sparse",
            Self::Reparse => "reparse",
            Self::Offline => "offline",
            Self::NotIndexed => "not_indexed",
            Self::Temporary => "temporary",
            Self::Virtual => "virtual",
            Self::Pinned => "pinned",
            Self::Unpinned => "unpinned",
            Self::Descendants => "descendants",
            Self::TreeSize => "tree_size",
            Self::TreeAllocated => "tree_allocated",
            Self::Bulkiness => "bulkiness",
            Self::Integrity => "integrity",
            Self::NoScrub => "no_scrub",
            Self::DirectoryFlag => "directory_flag",
            Self::RecallOnOpen => "recall_on_open",
            Self::RecallOnDataAccess => "recall_on_data_access",
            Self::ParityAttributes => "parity_attributes",
            Self::NameLength => "name_length",
            Self::PathLength => "path_length",
            Self::Malformed => "malformed",
            Self::MalformedPath => "malformed_path",
            Self::NameHex => "name_hex",
        }
    }

    /// Human-readable display name used as the CSV header label.
    /// Matches `FieldMeta::display_name` in `uffs-core`.
    ///
    /// `Attributes` and `ParityAttributes` intentionally share the
    /// `"Attributes"` label: the legacy parity baseline writes both
    /// columns under the same header but distinguishes them by
    /// position (column 0 vs column 24 in the 25-column parity
    /// layout).  Changing either label would break the baseline
    /// byte-parity pinned by `uffs-core`'s `format_parity_*` tests,
    /// so `clippy::match_same_arms` is silenced here rather than
    /// collapsed into a merged arm.
    #[must_use]
    #[expect(
        clippy::match_same_arms,
        reason = "Attributes / ParityAttributes deliberately share a label — see fn doc"
    )]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Drive => "Drive",
            Self::Path => "Path",
            Self::Name => "Name",
            Self::PathOnly => "Path Only",
            Self::Size => "Size",
            Self::SizeOnDisk => "Size on Disk",
            Self::Created => "Created",
            Self::Modified => "Last Written",
            Self::Accessed => "Last Accessed",
            Self::Extension => "Extension",
            Self::Type => "Type",
            Self::Attributes => "Attributes",
            Self::AttributeValue => "AttributeValue",
            Self::Hidden => "Hidden",
            Self::System => "System",
            Self::Archive => "Archive",
            Self::ReadOnly => "Read-only",
            Self::Compressed => "Compressed",
            Self::Encrypted => "Encrypted",
            Self::Sparse => "Sparse",
            Self::Reparse => "Reparse",
            Self::Offline => "Offline",
            Self::NotIndexed => "Not content indexed file",
            Self::Temporary => "Temporary",
            Self::Virtual => "Virtual",
            Self::Pinned => "Pinned",
            Self::Unpinned => "Unpinned",
            Self::Descendants => "Descendants",
            Self::TreeSize => "Tree Size",
            Self::TreeAllocated => "Tree Allocated",
            Self::Bulkiness => "Bulkiness",
            Self::Integrity => "Integrity",
            Self::NoScrub => "No scrub file",
            Self::DirectoryFlag => "Directory Flag",
            Self::RecallOnOpen => "Recall on open",
            Self::RecallOnDataAccess => "Recall on data access",
            Self::ParityAttributes => "Attributes",
            Self::NameLength => "Name Length",
            Self::PathLength => "Path Length",
            Self::Malformed => "Malformed",
            Self::MalformedPath => "Malformed Path",
            Self::NameHex => "Name (hex)",
        }
    }

    /// Accepted aliases for this column — alternate names the CLI
    /// `--columns` parser recognises.  Matches `FieldMeta::aliases` in
    /// `uffs-core`.
    #[must_use]
    pub const fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Drive => &["drv"],
            Self::PathOnly => &["pathonly", "path only"],
            Self::SizeOnDisk => &["sizeondisk", "size on disk", "allocated"],
            Self::Modified => &["written", "date"],
            Self::Extension => &["ext"],
            Self::Type => &["folder"],
            Self::Attributes => &["attrs"],
            Self::AttributeValue => &["attributevalue", "attrval"],
            Self::Hidden => &["h"],
            Self::System => &["s"],
            Self::Archive => &["a"],
            Self::ReadOnly => &["readonly", "read-only", "read only", "r"],
            Self::Offline => &["o"],
            Self::NotIndexed => &[
                "notindexed",
                "not indexed",
                "notcontent",
                "not content indexed",
                "not content indexed file",
            ],
            Self::Temporary => &["temp"],
            Self::Descendants => &["decendents"],
            Self::TreeSize => &["treesize"],
            Self::TreeAllocated => &["treeallocated"],
            Self::NoScrub => &["noscrub", "no scrub", "no scrub file"],
            Self::DirectoryFlag => &["directoryflag", "directory flag", "directory", "dir"],
            Self::RecallOnOpen => &["recallonopen", "recall on open"],
            Self::RecallOnDataAccess => &["recallondataaccess", "recall on data access"],
            Self::ParityAttributes => &["parityattributes"],
            Self::NameLength => &["namelength", "name_len", "namelen"],
            Self::PathLength => &["pathlength", "path_len", "pathlen"],
            Self::Malformed => &["ill_formed", "illformed", "bad_name"],
            Self::MalformedPath => &["malformedpath", "ill_formed_path", "bad_path"],
            Self::NameHex => &["namehex", "name_bytes_hex"],
            // Variants with no aliases fall through to the empty slice.
            Self::Path
            | Self::Name
            | Self::Size
            | Self::Created
            | Self::Accessed
            | Self::Compressed
            | Self::Encrypted
            | Self::Sparse
            | Self::Reparse
            | Self::Virtual
            | Self::Pinned
            | Self::Unpinned
            | Self::Bulkiness
            | Self::Integrity => &[],
        }
    }

    /// Parse a column name or alias (case-insensitive) into a
    /// canonical variant.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        for &col in Self::ALL {
            if col.canonical_name().eq_ignore_ascii_case(name) {
                return Some(col);
            }
            if col
                .aliases()
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
            {
                return Some(col);
            }
        }
        None
    }
}

/// Parity-compat column order (25 columns).
///
/// Mirrors the legacy baseline output.  Selected by `--parity-compat`
/// at the CLI surface and by `output_columns: "parity"` in the wire
/// protocol.
pub const PARITY_COLUMN_ORDER: &[OutputColumn] = &[
    OutputColumn::Path,
    OutputColumn::Name,
    OutputColumn::PathOnly,
    OutputColumn::Size,
    OutputColumn::SizeOnDisk,
    OutputColumn::Created,
    OutputColumn::Modified,
    OutputColumn::Accessed,
    OutputColumn::Descendants,
    OutputColumn::ReadOnly,
    OutputColumn::Archive,
    OutputColumn::System,
    OutputColumn::Hidden,
    OutputColumn::Offline,
    OutputColumn::NotIndexed,
    OutputColumn::NoScrub,
    OutputColumn::Integrity,
    OutputColumn::Pinned,
    OutputColumn::Unpinned,
    OutputColumn::DirectoryFlag,
    OutputColumn::Compressed,
    OutputColumn::Encrypted,
    OutputColumn::Sparse,
    OutputColumn::Reparse,
    OutputColumn::ParityAttributes,
];

/// Default column order used when `--columns all` is specified.
///
/// Data columns first, then boolean attributes in NTFS flag value
/// order (lowest → highest bit), then raw aggregates and derived
/// columns.  Matches the layout consumers expect from `--columns all`.
pub const BASELINE_COLUMN_ORDER: &[OutputColumn] = &[
    // ── Data columns ──────────────────────────────────────────
    OutputColumn::Path,
    OutputColumn::Name,
    OutputColumn::PathOnly,
    OutputColumn::Size,
    OutputColumn::SizeOnDisk,
    OutputColumn::Created,
    OutputColumn::Modified,
    OutputColumn::Accessed,
    OutputColumn::Descendants,
    // ── Boolean attributes in NTFS flag value order ───────────
    OutputColumn::ReadOnly,           // 0x0001
    OutputColumn::Hidden,             // 0x0002
    OutputColumn::System,             // 0x0004
    OutputColumn::DirectoryFlag,      // 0x0010
    OutputColumn::Archive,            // 0x0020
    OutputColumn::Sparse,             // 0x0200
    OutputColumn::Reparse,            // 0x0400
    OutputColumn::Compressed,         // 0x0800
    OutputColumn::Offline,            // 0x1000
    OutputColumn::NotIndexed,         // 0x2000
    OutputColumn::Encrypted,          // 0x4000
    OutputColumn::Integrity,          // 0x8000
    OutputColumn::NoScrub,            // 0x20000
    OutputColumn::RecallOnOpen,       // 0x40000
    OutputColumn::Pinned,             // 0x80000
    OutputColumn::Unpinned,           // 0x100000
    OutputColumn::RecallOnDataAccess, // 0x400000
    // ── Raw aggregate ─────────────────────────────────────────
    OutputColumn::Attributes,
    // ── Computed / derived columns ────────────────────────────
    OutputColumn::TreeSize,
    OutputColumn::TreeAllocated,
    OutputColumn::Bulkiness,
    OutputColumn::Type,
    OutputColumn::Extension,
    OutputColumn::NameLength,
    OutputColumn::PathLength,
];

#[cfg(test)]
mod tests {
    use super::OutputColumn;

    /// Every variant in [`OutputColumn::ALL`] must have a unique
    /// canonical name — otherwise `OutputColumn::parse` would return
    /// the first match and silently mis-resolve the others.
    #[test]
    fn canonical_names_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for &col in OutputColumn::ALL {
            let name = col.canonical_name();
            assert!(
                !seen.contains(&name),
                "duplicate canonical_name {name:?} for variant {col:?}"
            );
            seen.push(name);
        }
    }

    /// `parse` must round-trip every canonical name back to the
    /// original variant.  Case-insensitive because the CLI accepts
    /// mixed-case `--columns` arguments.
    #[test]
    fn parse_round_trips_canonical_names() {
        for &col in OutputColumn::ALL {
            assert_eq!(OutputColumn::parse(col.canonical_name()), Some(col));
            assert_eq!(
                OutputColumn::parse(&col.canonical_name().to_ascii_uppercase()),
                Some(col)
            );
        }
    }

    /// `parse` must accept every declared alias.
    #[test]
    fn parse_accepts_every_alias() {
        for &col in OutputColumn::ALL {
            for &alias in col.aliases() {
                assert_eq!(
                    OutputColumn::parse(alias),
                    Some(col),
                    "alias {alias:?} did not resolve to {col:?}"
                );
            }
        }
    }
}
