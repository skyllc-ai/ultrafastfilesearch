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

use uffs_polars::DataFrame;

pub use self::column::{CPP_COLUMN_ORDER, OutputColumn};
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
#[expect(
    clippy::unwrap_used,
    reason = "test code uses unwrap on controlled data"
)]
#[expect(
    clippy::expect_used,
    reason = "test code uses expect on controlled data"
)]
mod tests;
