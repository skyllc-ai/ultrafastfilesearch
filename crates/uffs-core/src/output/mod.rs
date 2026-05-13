// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Output configuration and formatting.
//!
//! Provides customizable output formatting with:
//! - Column selection
//! - Custom separators
//! - Quote handling
//! - Boolean representation (pos/neg)
//! - Header control
mod column;
mod config;
pub(crate) mod display_rows;
pub mod display_rows_format_bridge;

use uffs_polars::DataFrame;

pub use self::column::{BASELINE_COLUMN_ORDER, OutputColumn};
pub use self::config::OutputConfig;
use crate::error::Result;

/// Recursively count descendants for a given FRS.
///
/// Uses memoization to avoid recomputing counts for the same FRS.
/// Compute descendants count for each directory in the `DataFrame`.
///
/// This function builds a parent-child tree from the `frs` and `parent_frs`
/// columns, then counts all descendants (files and subdirectories) for each
/// entry.
///
/// For files, the descendants count is 0.
/// For directories, it's the total count of all nested items.
///
/// # Arguments
///
/// * `df` - `DataFrame` with columns: `frs`, `parent_frs`, `is_directory`,
///   `size`, `allocated_size`
///
/// # Returns
///
/// A new `DataFrame` with an added `descendants` column (u64).
///
/// # Errors
///
/// Returns an error if required columns are missing.
///
/// # Note
///
/// This is a convenience wrapper around [`crate::tree::add_tree_columns`].
/// For more tree columns (`treesize`, `tree_allocated`, `bulkiness`), use the
/// tree module directly.
pub fn add_descendants_column(df: &DataFrame) -> Result<DataFrame> {
    crate::tree::add_tree_columns(df, &[crate::tree::TreeColumn::Descendants])
}

#[cfg(test)]
mod tests;

// Byte-parity regression tests between legacy `write_display_rows`
// and `uffs_format::write_rows` live in a sibling test module to
// keep `tests.rs` under the 800-line policy ceiling.
#[cfg(test)]
mod tests_format_parity;
