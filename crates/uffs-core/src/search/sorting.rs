// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Sorting comparators for search results.
//!
//! Hosts two cohesive comparator pipelines — the general-purpose
//! Schwartzian decorate path ([`sort_rows_with_fold`]) and the
//! zero-alloc numeric fast path (`sort_rows_numeric_fast`) — plus
//! the shared `compare_by_column` / `compare_numeric_column` helpers
//! and the `field_to_attr_bit` lookup.  Keeping them together is
//! deliberate so a single read shows every ordering rule the engine
//! observes.
//!
//! `DataFrame` ↔ [`DisplayRow`] conversion was split out into
//! `dataframe_convert.rs` (different concern, separate readers).
//! Re-exported via `pub use` in `backend.rs` — callers see no change.

use rayon::prelude::*;

use super::backend::{DisplayRow, SortSpec};
use super::derived::{bulkiness_for_row, semantic_type_for_row, tree_allocated_for_row};
use super::field::FieldId;
use super::filters::extract_extension_after_dot;

/// Minimum row count at which `sort_rows_numeric_fast` switches from
/// sequential `sort_unstable_by` to `par_sort_unstable_by`.
///
/// Below this threshold the sequential sort wins because rayon's
/// parallel merge-sort pays a fixed scratch-buffer + task-dispatch
/// cost that dominates small inputs.  Above it, the ~O(n log n)
/// work splits cleanly across workers.  Measured empirically on a
/// 168 K-row Modified-DESC workload where `par_sort_unstable_by`
/// drops the phase from ~30 ms sequential to ~12 ms parallel.
const PARALLEL_SORT_THRESHOLD: usize = 16_384;

/// Pre-computed folded sort keys for a single row.
///
/// Stored alongside each `DisplayRow` during sorting (Schwartzian transform)
/// to avoid allocating inside the O(n·log n) comparator.
///
/// Only fields that `SortKeyNeeds::analyze` marks as required for the active
/// column + tiers are populated; the rest stay as `String::new()` (zero-cost,
/// no heap allocation because `String` has a null pointer representation
/// for empty).  This is a major hot-path optimisation: for a typical
/// numeric sort (Modified-DESC, Size, etc.) only `name` is needed as a
/// tiebreaker, so we save 4 allocations per row — ~170 ms at 168 K rows
/// on Windows based on the v0.5.54 deep-profile data.
struct RowSortKey {
    /// Folded name (always populated — used as the universal tiebreaker).
    name: String,
    /// Folded path.  Populated only when the column/tiers include
    /// `FieldId::Path`.
    path: String,
    /// Folded directory path only.  Populated only when the column/tiers
    /// include `FieldId::PathOnly`.
    path_only: String,
    /// Folded extension.  Populated only when the column/tiers include
    /// `FieldId::Extension`.
    ext: String,
    /// Folded semantic type/category.  Populated only when the column/tiers
    /// include `FieldId::Type`.
    file_type: String,
}

/// Bitmask of which `RowSortKey` fields the active sort needs.
///
/// Computed once upfront from the primary column + `extra_tiers`; the
/// decorate loop then skips folding + allocating keys that no comparator
/// branch will ever touch.  The name key is always needed because
/// `sort_rows_with_fold` uses it as a universal tiebreaker across all
/// numeric / flag columns, so it is not represented here.
///
/// Stored as a `u8` bitfield (not four booleans) to satisfy clippy's
/// `struct_excessive_bools` lint and keep the struct `Copy`-friendly.
#[derive(Debug, Default, Clone, Copy)]
struct SortKeyNeeds(u8);

impl SortKeyNeeds {
    /// Flag bit: the `path` folded key is read by some comparator branch.
    const PATH: u8 = 0b0001;
    /// Flag bit: the `path_only` folded key is read.
    const PATH_ONLY: u8 = 0b0010;
    /// Flag bit: the `ext` folded key is read.
    const EXT: u8 = 0b0100;
    /// Flag bit: the `file_type` folded key is read.
    const FILE_TYPE: u8 = 0b1000;

    /// Walk the primary column + tiers and flag which string keys the
    /// comparator will actually read.  Numeric / boolean-flag columns
    /// never read a folded key, so they contribute no bits.
    fn analyze(column: FieldId, extra_tiers: &[SortSpec]) -> Self {
        let mut bits = Self::bit_for(column);
        for tier in extra_tiers {
            bits |= Self::bit_for(tier.column);
        }
        Self(bits)
    }

    /// Map a single `FieldId` to the key-needs bit it implies.  `FieldId::Name`
    /// and every numeric / flag column map to zero because either the
    /// always-populated `key.name` is sufficient (Name) or no folded key is
    /// consulted at all (numeric / flag).
    const fn bit_for(column: FieldId) -> u8 {
        match column {
            FieldId::Path => Self::PATH,
            FieldId::PathOnly => Self::PATH_ONLY,
            FieldId::Extension => Self::EXT,
            FieldId::Type => Self::FILE_TYPE,
            FieldId::Name
            | FieldId::Size
            | FieldId::SizeOnDisk
            | FieldId::Created
            | FieldId::Modified
            | FieldId::Accessed
            | FieldId::Drive
            | FieldId::Descendants
            | FieldId::TreeSize
            | FieldId::TreeAllocated
            | FieldId::Bulkiness
            | FieldId::NameLength
            | FieldId::PathLength
            | FieldId::Hidden
            | FieldId::System
            | FieldId::Archive
            | FieldId::ReadOnly
            | FieldId::Compressed
            | FieldId::Encrypted
            | FieldId::Sparse
            | FieldId::Reparse
            | FieldId::Offline
            | FieldId::NotIndexed
            | FieldId::Temporary
            | FieldId::Virtual
            | FieldId::Pinned
            | FieldId::Unpinned
            | FieldId::Integrity
            | FieldId::NoScrub
            | FieldId::DirectoryFlag
            | FieldId::RecallOnOpen
            | FieldId::RecallOnDataAccess
            | FieldId::Attributes
            | FieldId::AttributeValue
            | FieldId::ParityAttributes
            | FieldId::Malformed
            | FieldId::MalformedPath
            | FieldId::NameHex => 0,
        }
    }

    /// Returns `true` if the folded `path` key is referenced by any
    /// comparator branch for the active sort.
    const fn path(self) -> bool {
        self.0 & Self::PATH != 0
    }

    /// Returns `true` if the folded `path_only` key is referenced.
    const fn path_only(self) -> bool {
        self.0 & Self::PATH_ONLY != 0
    }

    /// Returns `true` if the folded `ext` key is referenced.
    const fn ext(self) -> bool {
        self.0 & Self::EXT != 0
    }

    /// Returns `true` if the folded `file_type` key is referenced.
    const fn file_type(self) -> bool {
        self.0 & Self::FILE_TYPE != 0
    }

    /// Returns `true` if no folded directional key is needed.  Callers use
    /// this to gate the zero-alloc fast path in `sort_rows_with_fold`.
    const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Returns `true` if the column can be compared purely from the numeric /
/// flag fields of a `DisplayRow` without touching any `RowSortKey` field.
///
/// Strict-numeric columns unlock the zero-alloc fast path: we skip the
/// whole Schwartzian decorate and sort the `DisplayRow` slice in-place
/// with a raw-slice name tiebreaker.
///
/// Intentionally excludes `FieldId::Name`, `FieldId::Attributes`,
/// `FieldId::AttributeValue`, and `FieldId::ParityAttributes` even though
/// they don't set a `SortKeyNeeds` bit — they compare via the always-
/// populated folded name key and rely on case-insensitive ordering.
const fn is_strict_numeric(column: FieldId) -> bool {
    matches!(
        column,
        FieldId::Size
            | FieldId::SizeOnDisk
            | FieldId::Created
            | FieldId::Modified
            | FieldId::Accessed
            | FieldId::Drive
            | FieldId::Descendants
            | FieldId::TreeSize
            | FieldId::TreeAllocated
            | FieldId::Bulkiness
            | FieldId::NameLength
            | FieldId::PathLength
            | FieldId::Hidden
            | FieldId::System
            | FieldId::Archive
            | FieldId::ReadOnly
            | FieldId::Compressed
            | FieldId::Encrypted
            | FieldId::Sparse
            | FieldId::Reparse
            | FieldId::Offline
            | FieldId::NotIndexed
            | FieldId::Temporary
            | FieldId::Virtual
            | FieldId::Pinned
            | FieldId::Unpinned
            | FieldId::Integrity
            | FieldId::NoScrub
            | FieldId::DirectoryFlag
            | FieldId::RecallOnOpen
            | FieldId::RecallOnDataAccess
    )
}

/// Zero-allocation in-place sort for strict-numeric columns.
///
/// Sorts `rows` by `column` (with optional reverse for `descending`), then
/// by each tier in `extra_tiers`, with a final raw-slice `row.name()`
/// tiebreaker.  Every comparison is pure numeric / bitmask arithmetic on
/// the `DisplayRow` fields — no `String` allocation, no decorate vector.
///
/// Precondition: callers must verify that `column` and every
/// `extra_tiers[*]` column satisfy `is_strict_numeric`; otherwise the
/// comparator would silently fall through to `Ordering::Equal` and
/// produce an arbitrary order.  The guard in `sort_rows_with_fold`
/// enforces this precondition.
fn sort_rows_numeric_fast(
    rows: &mut [DisplayRow],
    column: FieldId,
    descending: bool,
    extra_tiers: &[SortSpec],
) {
    // Comparator closure — pure function of its two row references, so
    // it is `Sync` and safe for `par_sort_unstable_by`.  Extracted once
    // so both the sequential and parallel branches below reuse it
    // verbatim.
    let compare = |row_a: &DisplayRow, row_b: &DisplayRow| {
        let mut ord = compare_numeric_column(row_a, row_b, column);
        if descending {
            ord = ord.reverse();
        }
        for tier in extra_tiers {
            if ord != core::cmp::Ordering::Equal {
                break;
            }
            ord = compare_numeric_column(row_a, row_b, tier.column);
            if tier.descending {
                ord = ord.reverse();
            }
        }
        // Raw-slice name tiebreaker (case-sensitive).  Only hit when every
        // numeric tier compared equal, which is rare for Modified /
        // Created / Accessed (100 ns FILETIME resolution) but common for
        // Size-sorted duplicates.  Raw ordering is deterministic and
        // matches the Everything baseline behaviour.
        if ord == core::cmp::Ordering::Equal {
            ord = row_a.name().cmp(row_b.name());
        }
        ord
    };

    // Parallel sort above the empirically-tuned threshold: rayon's
    // merge-sort splits the comparator work across workers at the cost
    // of a scratch buffer + task dispatch.  Below the threshold the
    // sequential sort wins because those fixed costs dominate.
    if rows.len() >= PARALLEL_SORT_THRESHOLD {
        rows.par_sort_unstable_by(compare);
    } else {
        rows.sort_unstable_by(compare);
    }
}

/// Compare two `DisplayRow`s by a strict-numeric / flag column.
///
/// Mirrors the numeric arms of `compare_by_column` but takes no
/// `RowSortKey` — it's the zero-alloc comparator used by
/// `sort_rows_numeric_fast`.  For columns that are *not* strict-numeric
/// (`Name`, `Path`, `PathOnly`, `Extension`, `Type`, `Attributes`,
/// `AttributeValue`, `ParityAttributes`) this returns `Ordering::Equal`;
/// the caller's `is_strict_numeric` guard ensures that never happens in
/// practice.
fn compare_numeric_column(
    row_a: &DisplayRow,
    row_b: &DisplayRow,
    column: FieldId,
) -> core::cmp::Ordering {
    match column {
        FieldId::Size => row_a.size.cmp(&row_b.size),
        FieldId::SizeOnDisk => row_a.allocated.cmp(&row_b.allocated),
        FieldId::Created => row_a.created.cmp(&row_b.created),
        FieldId::Modified => row_a.modified.cmp(&row_b.modified),
        FieldId::Accessed => row_a.accessed.cmp(&row_b.accessed),
        FieldId::Drive => row_a.drive.cmp(&row_b.drive),
        FieldId::Descendants => row_a.descendants.cmp(&row_b.descendants),
        FieldId::TreeSize => row_a.treesize.cmp(&row_b.treesize),
        FieldId::TreeAllocated => tree_allocated_for_row(row_a).cmp(&tree_allocated_for_row(row_b)),
        FieldId::Bulkiness => bulkiness_for_row(row_a).cmp(&bulkiness_for_row(row_b)),
        FieldId::NameLength => row_a
            .name()
            .chars()
            .count()
            .cmp(&row_b.name().chars().count()),
        FieldId::PathLength => row_a.path.chars().count().cmp(&row_b.path.chars().count()),
        // Boolean flag columns include an **inline** raw-slice name
        // tiebreaker that participates in the outer `descending` reversal.
        // This matches the documented `compare_by_column` semantics for
        // flag sorts: `DirectoryFlag desc` puts dirs first *and* sorts
        // names Z→A within each block (tiebreaker reverses along with
        // primary), whereas `DirectoryFlag asc` yields A→Z names per
        // block.  Keeping the tiebreaker inline here preserves that
        // behaviour on the fast path.
        FieldId::Hidden
        | FieldId::System
        | FieldId::Archive
        | FieldId::ReadOnly
        | FieldId::Compressed
        | FieldId::Encrypted
        | FieldId::Sparse
        | FieldId::Reparse
        | FieldId::Offline
        | FieldId::NotIndexed
        | FieldId::Temporary
        | FieldId::Virtual
        | FieldId::Pinned
        | FieldId::Unpinned
        | FieldId::Integrity
        | FieldId::NoScrub
        | FieldId::DirectoryFlag
        | FieldId::RecallOnOpen
        | FieldId::RecallOnDataAccess => {
            let mask = field_to_attr_bit(column);
            let a_set = row_a.flags & mask != 0;
            let b_set = row_b.flags & mask != 0;
            a_set
                .cmp(&b_set)
                .then_with(|| row_a.name().cmp(row_b.name()))
        }
        // WI-4.4 forensic bool columns — backed by precomputed row fields, not
        // an attribute-flag mask. Same flag-sort semantics (name tiebreaker).
        FieldId::Malformed => row_a
            .malformed
            .cmp(&row_b.malformed)
            .then_with(|| row_a.name().cmp(row_b.name())),
        FieldId::MalformedPath => row_a
            .malformed_path
            .cmp(&row_b.malformed_path)
            .then_with(|| row_a.name().cmp(row_b.name())),
        // String-based columns never reach this function — the caller's
        // `is_strict_numeric` guard excludes them.  Return `Equal` as a
        // defensive default (the name tiebreaker in `sort_rows_numeric_fast`
        // will then provide the ordering).  `NameHex` is a non-sortable
        // projection column and lands here too.
        FieldId::Name
        | FieldId::Path
        | FieldId::PathOnly
        | FieldId::Extension
        | FieldId::Type
        | FieldId::Attributes
        | FieldId::AttributeValue
        | FieldId::ParityAttributes
        | FieldId::NameHex => core::cmp::Ordering::Equal,
    }
}

/// Sort display rows by the given column, then by additional tiers, with a
/// final name-ascending tiebreaker.
///
/// String-based columns (Name, Path, Extension) use pre-computed folded
/// keys via `CaseFold` to avoid per-comparison allocation (Schwartzian
/// transform).
pub fn sort_rows(
    rows: &mut [DisplayRow],
    column: FieldId,
    descending: bool,
    extra_tiers: &[SortSpec],
) {
    sort_rows_with_fold(
        rows,
        column,
        descending,
        extra_tiers,
        uffs_text::case_fold::CaseFold::default_table(),
    );
}

/// Sort display rows using a specific `CaseFold` engine.
pub fn sort_rows_with_fold(
    rows: &mut [DisplayRow],
    column: FieldId,
    descending: bool,
    extra_tiers: &[SortSpec],
    fold: uffs_text::case_fold::CaseFold,
) {
    if rows.len() <= 1 {
        return;
    }
    // Determine which folded-key fields the active sort column + tiers will
    // actually read.  For numeric / boolean-flag sorts this leaves the four
    // directional keys empty, saving 4 heap allocations per row.  The name
    // key is always populated (used as the universal tiebreaker).
    let needs = SortKeyNeeds::analyze(column, extra_tiers);

    // Zero-alloc fast path: the primary column and every tier are strictly
    // numeric / flag-based (no folded key referenced at all).  Skip the
    // decorate + undecorate passes entirely and sort the `DisplayRow` slice
    // in-place with a raw-slice name tiebreaker.  This is the hot path for
    // the default sort (Modified-DESC) and saves ~1 heap allocation per row
    // (~30-50 ms at 168 K rows on Windows).
    if needs.is_empty()
        && is_strict_numeric(column)
        && extra_tiers
            .iter()
            .all(|tier| is_strict_numeric(tier.column))
    {
        sort_rows_numeric_fast(rows, column, descending, extra_tiers);
        return;
    }

    // Decorate: zip each row with pre-computed folded keys.
    //
    // For large row counts (`>= PARALLEL_SORT_THRESHOLD`) the decorate
    // pass itself is parallelized: each `fold_into` call allocates a
    // `String` and the Schwartzian trick needs one such allocation per
    // row × up to four key fields, so the decorate cost dominates the
    // actual comparator work on large inputs (~60 ms for 167 K rows
    // on a single thread, ~12 ms with 8 rayon workers).  Each worker
    // uses its own `fold_buf` to avoid contention.
    let mut decorated: Vec<(DisplayRow, RowSortKey)> = if rows.len() >= PARALLEL_SORT_THRESHOLD {
        let owned_rows: Vec<DisplayRow> = rows.iter_mut().map(core::mem::take).collect();
        owned_rows
            .into_par_iter()
            .map_with(Vec::<u8>::with_capacity(256), |fold_buf, row| {
                let key = build_row_sort_key(&row, needs, fold, fold_buf);
                (row, key)
            })
            .collect()
    } else {
        let mut fold_buf: Vec<u8> = Vec::with_capacity(256);
        rows.iter_mut()
            .map(|row| {
                let key = build_row_sort_key(row, needs, fold, &mut fold_buf);
                // Take ownership; we'll put it back after sorting.
                (core::mem::take(row), key)
            })
            .collect()
    };

    // Comparator closure — used by both the sequential and parallel
    // sorts below, so extracted once to avoid duplication.  The
    // tiebreaker check mirrors the original `sort_rows_numeric_fast`
    // pattern: raw-slice name break when every key compared equal.
    let compare = |entry_a: &(DisplayRow, RowSortKey),
                   entry_b: &(DisplayRow, RowSortKey)|
     -> core::cmp::Ordering {
        let (row_a, key_a) = entry_a;
        let (row_b, key_b) = entry_b;
        let mut ord = compare_by_column(row_a, key_a, row_b, key_b, column);
        if descending {
            ord = ord.reverse();
        }
        for tier in extra_tiers {
            if ord != core::cmp::Ordering::Equal {
                break;
            }
            ord = compare_by_column(row_a, key_a, row_b, key_b, tier.column);
            if tier.descending {
                ord = ord.reverse();
            }
        }
        // Name tiebreaker (case-folded, then raw for determinism).
        if ord == core::cmp::Ordering::Equal
            && column != FieldId::Name
            && !extra_tiers.iter().any(|tier| tier.column == FieldId::Name)
        {
            ord = key_a
                .name
                .cmp(&key_b.name)
                .then_with(|| row_a.name().cmp(row_b.name()));
        }
        ord
    };

    // Parallel sort kicks in at the same threshold as
    // `sort_rows_numeric_fast` — rayon's merge-sort amortises task
    // dispatch only above ~16 K rows.  For the path_only ext fast
    // path at 167 K rows this drops the sort phase from ~30 ms
    // sequential to ~9 ms parallel on a 12-core host.
    if decorated.len() >= PARALLEL_SORT_THRESHOLD {
        decorated.par_sort_unstable_by(compare);
    } else {
        decorated.sort_unstable_by(compare);
    }

    // Undecorate: move sorted rows back into the slice.
    for (dest, (row, _key)) in rows.iter_mut().zip(decorated) {
        *dest = row;
    }
}

/// Build a single `RowSortKey` with only the fields the active sort
/// actually reads.  Extracted from `sort_rows_with_fold` so the
/// sequential + parallel decorate branches share one implementation.
fn build_row_sort_key(
    row: &DisplayRow,
    needs: SortKeyNeeds,
    fold: uffs_text::case_fold::CaseFold,
    fold_buf: &mut Vec<u8>,
) -> RowSortKey {
    RowSortKey {
        name: fold.fold_into(row.name(), fold_buf).to_owned(),
        path: if needs.path() {
            fold.fold_into(&row.path, fold_buf).to_owned()
        } else {
            String::new()
        },
        path_only: if needs.path_only() {
            fold.fold_into(row.path_dir(), fold_buf).to_owned()
        } else {
            String::new()
        },
        ext: if needs.ext() {
            // Dot-gated extraction so dotless names sort with the
            // empty-extension group (matching the indexer's
            // `extension_id = 0` assignment) instead of with
            // whatever alphabetic group their name happens to
            // start with.
            fold.fold_into(extract_extension_after_dot(row.name()), fold_buf)
                .to_owned()
        } else {
            String::new()
        },
        file_type: if needs.file_type() {
            fold.fold_into(semantic_type_for_row(row), fold_buf)
                .to_owned()
        } else {
            String::new()
        },
    }
}

/// Compare two rows by a single column (natural / ascending order).
///
/// String-based columns use a **two-phase comparison** for deterministic
/// ordering:
///   1. Case-folded keys (groups variants together: `TEXT` ≈ `text`)
///   2. Unicode codepoint tiebreaker (deterministic within the group: `TEXT` <
///      `text`)
///
/// This ensures stable, reproducible sort order regardless of the
/// underlying `sort_unstable_by` implementation.
fn compare_by_column(
    row_a: &DisplayRow,
    key_a: &RowSortKey,
    row_b: &DisplayRow,
    key_b: &RowSortKey,
    column: FieldId,
) -> core::cmp::Ordering {
    match column {
        FieldId::Size => row_a.size.cmp(&row_b.size),
        FieldId::SizeOnDisk => row_a.allocated.cmp(&row_b.allocated),
        FieldId::Created => row_a.created.cmp(&row_b.created),
        FieldId::Modified => row_a.modified.cmp(&row_b.modified),
        FieldId::Accessed => row_a.accessed.cmp(&row_b.accessed),
        FieldId::Path => key_a
            .path
            .cmp(&key_b.path)
            .then_with(|| row_a.path.cmp(&row_b.path)),
        FieldId::PathOnly => key_a
            .path_only
            .cmp(&key_b.path_only)
            .then_with(|| row_a.path_dir().cmp(row_b.path_dir())),
        FieldId::Drive => row_a.drive.cmp(&row_b.drive),
        FieldId::Extension => key_a.ext.cmp(&key_b.ext).then_with(|| {
            let ext_a = extract_extension_after_dot(row_a.name());
            let ext_b = extract_extension_after_dot(row_b.name());
            ext_a.cmp(ext_b)
        }),
        FieldId::Type => key_a
            .file_type
            .cmp(&key_b.file_type)
            .then_with(|| semantic_type_for_row(row_a).cmp(semantic_type_for_row(row_b))),
        FieldId::Descendants => row_a.descendants.cmp(&row_b.descendants),
        FieldId::TreeSize => row_a.treesize.cmp(&row_b.treesize),
        FieldId::TreeAllocated => tree_allocated_for_row(row_a).cmp(&tree_allocated_for_row(row_b)),
        FieldId::Bulkiness => bulkiness_for_row(row_a).cmp(&bulkiness_for_row(row_b)),
        FieldId::NameLength => row_a
            .name()
            .chars()
            .count()
            .cmp(&row_b.name().chars().count()),
        FieldId::PathLength => row_a.path.chars().count().cmp(&row_b.path.chars().count()),
        // ── Boolean attribute fields: sort by flag bit, tiebreak on name ──
        FieldId::Hidden
        | FieldId::System
        | FieldId::Archive
        | FieldId::ReadOnly
        | FieldId::Compressed
        | FieldId::Encrypted
        | FieldId::Sparse
        | FieldId::Reparse
        | FieldId::Offline
        | FieldId::NotIndexed
        | FieldId::Temporary
        | FieldId::Virtual
        | FieldId::Pinned
        | FieldId::Unpinned
        | FieldId::Integrity
        | FieldId::NoScrub
        | FieldId::DirectoryFlag
        | FieldId::RecallOnOpen
        | FieldId::RecallOnDataAccess => {
            let mask = field_to_attr_bit(column);
            let a_set = row_a.flags & mask != 0;
            let b_set = row_b.flags & mask != 0;
            // true > false so that desc puts flagged files first
            a_set
                .cmp(&b_set)
                .then_with(|| key_a.name.cmp(&key_b.name))
                .then_with(|| row_a.name().cmp(row_b.name()))
        }
        // ── WI-4.4 forensic bool columns: precomputed row fields ──
        FieldId::Malformed => row_a
            .malformed
            .cmp(&row_b.malformed)
            .then_with(|| key_a.name.cmp(&key_b.name))
            .then_with(|| row_a.name().cmp(row_b.name())),
        FieldId::MalformedPath => row_a
            .malformed_path
            .cmp(&row_b.malformed_path)
            .then_with(|| key_a.name.cmp(&key_b.name))
            .then_with(|| row_a.name().cmp(row_b.name())),
        // ── Remaining non-sortable fields: name tiebreaker (incl. NameHex) ──
        FieldId::Name
        | FieldId::Attributes
        | FieldId::AttributeValue
        | FieldId::ParityAttributes
        | FieldId::NameHex => key_a
            .name
            .cmp(&key_b.name)
            .then_with(|| row_a.name().cmp(row_b.name())),
    }
}

/// Map a boolean-attribute `FieldId` to its NTFS `FILE_ATTRIBUTE_*` bitmask.
///
/// Non-boolean fields return `0` — the caller skips attribute-based sorting.
///
/// Kept as a separate function (rather than inlined into `compare_by_column`)
/// because inlining 42 match arms into a nested closure harms readability.
pub(crate) const fn field_to_attr_bit(field: FieldId) -> u32 {
    match field {
        FieldId::Hidden => 0x0002,
        FieldId::System => 0x0004,
        FieldId::Archive => 0x0020,
        FieldId::ReadOnly => 0x0001,
        FieldId::Compressed => 0x0800,
        FieldId::Encrypted => 0x4000,
        FieldId::Sparse => 0x0200,
        FieldId::Reparse => 0x0400,
        FieldId::Offline => 0x1000,
        FieldId::NotIndexed => 0x2000,
        FieldId::Temporary => 0x0100,
        FieldId::Virtual => 0x0001_0000,
        FieldId::Pinned => 0x0008_0000,
        FieldId::Unpinned => 0x0010_0000,
        FieldId::Integrity => 0x8000,
        FieldId::NoScrub => 0x0002_0000,
        FieldId::DirectoryFlag => 0x0010,
        FieldId::RecallOnOpen => 0x0004_0000,
        FieldId::RecallOnDataAccess => 0x0040_0000,
        // Non-boolean fields — no attribute bit.
        FieldId::Drive
        | FieldId::Path
        | FieldId::Name
        | FieldId::PathOnly
        | FieldId::Size
        | FieldId::SizeOnDisk
        | FieldId::Created
        | FieldId::Modified
        | FieldId::Accessed
        | FieldId::Extension
        | FieldId::Type
        | FieldId::Attributes
        | FieldId::AttributeValue
        | FieldId::Descendants
        | FieldId::TreeSize
        | FieldId::TreeAllocated
        | FieldId::Bulkiness
        | FieldId::ParityAttributes
        | FieldId::NameLength
        | FieldId::PathLength
        | FieldId::Malformed
        | FieldId::MalformedPath
        | FieldId::NameHex => 0,
    }
}

/// Parse a `--sort` value like `"name:asc,modified:desc"` into sort specs.
///
/// Supports three direction syntaxes:
/// - Prefix: `-size` means descending, bare `size` means ascending
/// - Suffix: `size:desc` or `size:asc` (explicit)
///
/// Without any direction hint, the field-type default is used.
///
/// Any field recognised by `FieldId::parse` that is also sortable is accepted.
#[must_use]
pub fn parse_sort_spec(sort_str: &str) -> Vec<SortSpec> {
    let mut specs = Vec::new();
    for raw_part in sort_str.split(',') {
        let trimmed = raw_part.trim();

        // Check for `-` prefix (e.g. "-modified" → descending).
        let (has_dash_prefix, after_dash) = trimmed
            .strip_prefix('-')
            .map_or((false, trimmed), |rest| (true, rest));

        let (col_str, dir_str) = if let Some((col, dir)) = after_dash.split_once(':') {
            (col.trim(), Some(dir.trim()))
        } else {
            (after_dash, None)
        };
        let Some(field) = FieldId::parse(col_str) else {
            continue;
        };
        if !field.metadata().sortable {
            continue;
        }
        let descending = match dir_str {
            Some("desc") => true,
            Some("asc") => false,
            _ if has_dash_prefix => true,
            _ => matches!(
                field.default_sort_direction(),
                Some(super::field::SortDirection::Descending)
            ),
        };
        specs.push(SortSpec {
            column: field,
            descending,
        });
    }
    specs
}

/// Format the current sort state back into a CLI-compatible sort string.
#[must_use]
pub fn format_sort_spec(primary: FieldId, primary_desc: bool, extra: &[SortSpec]) -> String {
    let mut parts = Vec::with_capacity(1 + extra.len());
    let dir = |desc: bool| if desc { "desc" } else { "asc" };
    parts.push(format!(
        "{}:{}",
        primary.canonical_name(),
        dir(primary_desc)
    ));
    for spec in extra {
        parts.push(format!(
            "{}:{}",
            spec.column.canonical_name(),
            dir(spec.descending)
        ));
    }
    parts.join(",")
}

// `DataFrame` ↔ `DisplayRow` conversion lives in `dataframe_convert.rs`
// (split out so each module owns one concern).  Re-exported via
// `backend.rs` so callers see no API change.
