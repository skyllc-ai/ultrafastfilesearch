// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Polars `DataFrame` ↔ [`DisplayRow`] conversions.
//!
//! Pulled out of `sorting.rs` so each module owns one concern: this
//! file converts between the search backend's row type and Polars
//! tables; `sorting.rs` ranks rows.  The split lets readers find
//! either contract without scrolling past the other and keeps both
//! files comfortably under the workspace's 800-LOC ceiling.
//!
//! Public functions are re-exported from `search/backend.rs` so
//! callers continue to write `use uffs_core::search::backend::*;`
//! exactly as before.

use super::backend::DisplayRow;
use super::derived::{bulkiness_for_row, tree_allocated_for_row};

/// Static lookup table for the 26 valid drive labels (`"A:"` …
/// `"Z:"`), indexed by [`uffs_mft::platform::DriveLetter::alphabet_index`].
///
/// Replaces a per-row `format!("{}:", drive)` allocation in the
/// `Vec<DisplayRow>` → `DataFrame` conversion hot path (Phase 6d
/// category-δ).  Each entry is a `&'static str` carved into the rodata
/// segment; the resulting `Vec<&'static str>` carries no heap copies
/// of the labels.  Polars'/Arrow's column-builder still copies into a
/// contiguous string buffer, but we save the upstream per-row
/// `String` allocation + free pair.
const DRIVE_LABELS: [&str; 26] = [
    "A:", "B:", "C:", "D:", "E:", "F:", "G:", "H:", "I:", "J:", "K:", "L:", "M:", //
    "N:", "O:", "P:", "Q:", "R:", "S:", "T:", "U:", "V:", "W:", "X:", "Y:", "Z:",
];

/// Convert `DisplayRow` results to a Polars `DataFrame` with standard MFT
/// column names so existing CLI output formatters can consume it.
///
/// This creates a **small** `DataFrame` (only matching rows, not the full MFT).
///
/// # Errors
///
/// Returns an error if `DataFrame` construction fails.
pub fn display_rows_to_dataframe(
    rows: &[DisplayRow],
) -> uffs_polars::PolarsResult<uffs_polars::DataFrame> {
    use uffs_polars::{Column, DataFrame, columns};

    let names: Vec<&str> = rows.iter().map(DisplayRow::name).collect();
    let paths: Vec<&str> = rows.iter().map(|row| row.path.as_str()).collect();
    let sizes: Vec<u64> = rows.iter().map(|row| row.size).collect();
    let allocated: Vec<u64> = rows.iter().map(|row| row.allocated).collect();
    let created: Vec<i64> = rows.iter().map(|row| row.created).collect();
    let modified: Vec<i64> = rows.iter().map(|row| row.modified).collect();
    let accessed: Vec<i64> = rows.iter().map(|row| row.accessed).collect();
    let flags: Vec<u32> = rows.iter().map(|row| row.flags).collect();
    let drives: Vec<&str> = rows
        .iter()
        .map(|row| {
            DRIVE_LABELS
                .get(row.drive.alphabet_index())
                .copied()
                .unwrap_or("?:")
        })
        .collect();
    let descendants: Vec<u32> = rows.iter().map(|row| row.descendants).collect();
    let treesize: Vec<u64> = rows.iter().map(|row| row.treesize).collect();
    let tree_allocated: Vec<u64> = rows.iter().map(tree_allocated_for_row).collect();
    let bulkiness: Vec<u64> = rows.iter().map(bulkiness_for_row).collect();

    // path_only = directory portion of path (up to and including last backslash).
    let path_only: Vec<&str> = rows.iter().map(DisplayRow::path_dir).collect();

    DataFrame::new(rows.len(), vec![
        Column::new(columns::NAME.into(), &names),
        Column::new(columns::PATH.into(), &paths),
        Column::new("path_only".into(), &path_only),
        Column::new(columns::SIZE.into(), &sizes),
        Column::new("allocated_size".into(), &allocated),
        Column::new(columns::CREATED.into(), &created),
        Column::new(columns::MODIFIED.into(), &modified),
        Column::new(columns::ACCESSED.into(), &accessed),
        Column::new(columns::FLAGS.into(), &flags),
        Column::new("drive".into(), &drives),
        Column::new("descendants".into(), &descendants),
        Column::new("treesize".into(), &treesize),
        Column::new("tree_allocated".into(), &tree_allocated),
        Column::new("bulkiness".into(), &bulkiness),
    ])
}

/// Convert a legacy Polars `DataFrame` into `Vec<DisplayRow>`.
///
/// Handles both "new" column layouts (from `display_rows_to_dataframe`) and
/// legacy MFT layouts (from `results_to_dataframe`). Timestamps may be
/// plain `Int64` or `Datetime(Microseconds)` — both are handled.
///
/// Columns that don't exist get sensible defaults (0 for numbers, empty
/// strings, `'?'` for drive).
/// Converts a `DataFrame` into a `Vec<DisplayRow>` for rendering.
#[must_use]
pub fn dataframe_to_display_rows(data_frame: &uffs_polars::DataFrame) -> Vec<DisplayRow> {
    let height = data_frame.height();
    if height == 0 {
        return Vec::new();
    }

    let mut rows = Vec::with_capacity(height);
    for row_idx in 0..height {
        let path = col_str(data_frame, "path", row_idx).unwrap_or_default();
        let drive = col_str(data_frame, "drive", row_idx)
            .and_then(|val| val.chars().next())
            .and_then(|ch| uffs_mft::platform::DriveLetter::parse(ch).ok())
            .unwrap_or(uffs_mft::platform::DriveLetter::X);
        let size = col_u64(data_frame, "size", row_idx);
        let allocated = col_u64(data_frame, "allocated_size", row_idx);
        let flags = u32::try_from(col_u64(data_frame, "flags", row_idx)).unwrap_or(u32::MAX);
        let is_directory = col_bool(data_frame, "is_directory", row_idx);
        let created = col_timestamp(data_frame, "created", row_idx);
        let modified = col_timestamp(data_frame, "modified", row_idx);
        let accessed = col_timestamp(data_frame, "accessed", row_idx);
        let descendants =
            u32::try_from(col_u64(data_frame, "descendants", row_idx)).unwrap_or(u32::MAX);
        let treesize = col_u64(data_frame, "treesize", row_idx);
        let tree_allocated = col_u64(data_frame, "tree_allocated", row_idx);

        rows.push(DisplayRow::new(
            uffs_mft::len_to_u32(row_idx),
            drive,
            path,
            size,
            is_directory,
            modified,
            created,
            accessed,
            flags,
            allocated,
            descendants,
            treesize,
            tree_allocated,
        ));
    }
    rows
}

// ── DataFrame column helpers (private) ────────────────────────────────

/// Extract a `String` from a `DataFrame` column.
fn col_str(data_frame: &uffs_polars::DataFrame, col_name: &str, row_idx: usize) -> Option<String> {
    data_frame
        .column(col_name)
        .ok()
        .and_then(|column| column.str().ok())
        .and_then(|chunked| chunked.get(row_idx).map(String::from))
}

/// Extract a `u64` value from a `DataFrame` column (handles `UInt64`,
/// `Int64`, `UInt32` dtype).
fn col_u64(data_frame: &uffs_polars::DataFrame, col_name: &str, row_idx: usize) -> u64 {
    data_frame
        .column(col_name)
        .ok()
        .and_then(|column| {
            column
                .u64()
                .ok()
                .and_then(|arr| arr.get(row_idx))
                .or_else(|| {
                    column
                        .i64()
                        .ok()
                        .and_then(|arr| arr.get(row_idx).map(uffs_mft::nonneg_to_u64))
                })
                .or_else(|| {
                    column
                        .u32()
                        .ok()
                        .and_then(|arr| arr.get(row_idx).map(u64::from))
                })
        })
        .unwrap_or(0)
}

/// Extract a boolean value from a `DataFrame` column.
fn col_bool(data_frame: &uffs_polars::DataFrame, col_name: &str, row_idx: usize) -> bool {
    data_frame
        .column(col_name)
        .ok()
        .and_then(|column| column.bool().ok())
        .and_then(|chunked| chunked.get(row_idx))
        .unwrap_or(false)
}

/// Extract a timestamp (microseconds `i64`) from a `DataFrame` column.
///
/// Handles both plain `Int64` and `Datetime(Microseconds)` dtypes.
fn col_timestamp(data_frame: &uffs_polars::DataFrame, col_name: &str, row_idx: usize) -> i64 {
    data_frame
        .column(col_name)
        .ok()
        .and_then(|column| {
            // Try direct i64 first (from display_rows_to_dataframe).
            column
                .i64()
                .ok()
                .and_then(|arr| arr.get(row_idx))
                .or_else(|| {
                    // Try Datetime(Microseconds) (from legacy MftIndex DataFrames).
                    // `.phys` gives the underlying Int64 chunked array.
                    column.datetime().ok().and_then(|dt| dt.phys.get(row_idx))
                })
        })
        .unwrap_or(0)
}
