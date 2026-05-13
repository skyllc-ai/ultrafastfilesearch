// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bridge between `uffs-core`'s [`OutputColumn`] (= `FieldId` alias)
//! and `uffs-format`'s lightweight [`uffs_format::OutputColumn`].
//!
//! `uffs-core` keeps `OutputColumn` as a `FieldId` alias so the full
//! metadata suite (aggregation caps, field type, sort direction, …)
//! stays co-located with the search code.  `uffs-format` defines a
//! narrower enum with only the metadata the CSV writer needs
//! (canonical name, display name, aliases), so the daemon's
//! `--out=file` path and the thin CLI can share one writer without
//! pulling polars / chrono / rayon into the CLI binary.
//!
//! This module hosts the one-way conversion `FieldId → OutputColumn`
//! used by `super::display_rows::write_display_rows` and by the
//! `format_parity_*` regression tests in `super::tests_format_parity`
//! (split out of `super::tests` during the v0.5.64 file-size
//! reduction).  The `OutputColumn` variant set is pinned to match
//! `FieldId::ALL` by the `field_id_matches_output_column_*`
//! regression tests in `uffs_core::search::field::field_tests`.

use uffs_format::OutputColumn as FmtColumn;

use super::OutputColumn;

/// Translate a core [`OutputColumn`] (= [`crate::search::field::FieldId`])
/// into the matching `uffs-format` variant.
///
/// Exhaustive match — adding a new `FieldId` variant trips a
/// non-exhaustive-match error here, preventing silent omission from
/// the shared formatter.
#[must_use]
pub const fn field_id_to_format_column(col: OutputColumn) -> FmtColumn {
    match col {
        OutputColumn::Drive => FmtColumn::Drive,
        OutputColumn::Path => FmtColumn::Path,
        OutputColumn::Name => FmtColumn::Name,
        OutputColumn::PathOnly => FmtColumn::PathOnly,
        OutputColumn::Size => FmtColumn::Size,
        OutputColumn::SizeOnDisk => FmtColumn::SizeOnDisk,
        OutputColumn::Created => FmtColumn::Created,
        OutputColumn::Modified => FmtColumn::Modified,
        OutputColumn::Accessed => FmtColumn::Accessed,
        OutputColumn::Extension => FmtColumn::Extension,
        OutputColumn::Type => FmtColumn::Type,
        OutputColumn::Attributes => FmtColumn::Attributes,
        OutputColumn::AttributeValue => FmtColumn::AttributeValue,
        OutputColumn::Hidden => FmtColumn::Hidden,
        OutputColumn::System => FmtColumn::System,
        OutputColumn::Archive => FmtColumn::Archive,
        OutputColumn::ReadOnly => FmtColumn::ReadOnly,
        OutputColumn::Compressed => FmtColumn::Compressed,
        OutputColumn::Encrypted => FmtColumn::Encrypted,
        OutputColumn::Sparse => FmtColumn::Sparse,
        OutputColumn::Reparse => FmtColumn::Reparse,
        OutputColumn::Offline => FmtColumn::Offline,
        OutputColumn::NotIndexed => FmtColumn::NotIndexed,
        OutputColumn::Temporary => FmtColumn::Temporary,
        OutputColumn::Virtual => FmtColumn::Virtual,
        OutputColumn::Pinned => FmtColumn::Pinned,
        OutputColumn::Unpinned => FmtColumn::Unpinned,
        OutputColumn::Descendants => FmtColumn::Descendants,
        OutputColumn::TreeSize => FmtColumn::TreeSize,
        OutputColumn::TreeAllocated => FmtColumn::TreeAllocated,
        OutputColumn::Bulkiness => FmtColumn::Bulkiness,
        OutputColumn::Integrity => FmtColumn::Integrity,
        OutputColumn::NoScrub => FmtColumn::NoScrub,
        OutputColumn::DirectoryFlag => FmtColumn::DirectoryFlag,
        OutputColumn::RecallOnOpen => FmtColumn::RecallOnOpen,
        OutputColumn::RecallOnDataAccess => FmtColumn::RecallOnDataAccess,
        OutputColumn::ParityAttributes => FmtColumn::ParityAttributes,
        OutputColumn::NameLength => FmtColumn::NameLength,
        OutputColumn::PathLength => FmtColumn::PathLength,
    }
}
