//! Directory tree structure and metrics computation.
//!
//! This module provides efficient tree-based calculations for MFT data:
//! - `descendants`: Count of all items under a directory
//! - `treesize`: Sum of logical file sizes under a directory
//! - `tree_allocated`: Sum of allocated sizes under a directory
//! - `bulkiness`: Fragmentation metric (filtered allocated size sum)
//!
//! # Bulkiness Algorithm (matches the historical baseline)
//!
//! Bulkiness identifies folders with many small fragmented files, not just big
//! folders. The algorithm filters out large files that dominate a folder's
//! size:
//!
//! 1. Collect all children's allocated sizes
//! 2. Calculate threshold = 1% of folder's total allocated size
//! 3. Exclude files with allocated size >= threshold from bulkiness sum
//! 4. The remaining sum represents "fragmented" space from small files
//!
//! # Architecture
//!
//! Tree metrics are computed on-demand, not during MFT reading.
//! The [`TreeIndex`] builds a parent-child map from `DataFrame` columns,
//! then computes metrics with memoization for efficiency.
//!
//! # Example
//!
//! ```rust,ignore
//! use uffs_core::tree::{TreeIndex, TreeColumns};
//!
//! // Build tree index from DataFrame
//! let tree = TreeIndex::from_dataframe(&df)?;
//!
//! // Add requested columns
//! let df = tree.add_columns(df, &[TreeColumns::Descendants, TreeColumns::TreeSize])?;
//! ```

use uffs_polars::DataFrame;

use crate::error::Result;

mod column;
mod index;

pub use column::TreeColumn;
pub use index::TreeIndex;

/// Add tree columns to a `DataFrame` on-demand.
///
/// This is a convenience function that builds a [`TreeIndex`] and adds
/// the requested columns in one call.
///
/// # Arguments
///
/// * `df` - `DataFrame` with columns: `frs`, `parent_frs`, `is_directory`,
///   `size`, `allocated_size`
/// * `columns` - Which tree columns to add
///
/// # Errors
///
/// Returns an error if required columns are missing.
pub fn add_tree_columns(df: &DataFrame, columns: &[TreeColumn]) -> Result<DataFrame> {
    if columns.is_empty() {
        return Ok(df.clone());
    }

    let mut tree = TreeIndex::from_dataframe(df)?;
    tree.add_columns(df, columns)
}

/// Apply treesize transformation to directories for baseline-compatible output.
///
/// For directories, replaces:
/// - `size` with `treesize` (sum of logical sizes in subtree)
/// - `allocated_size` with `tree_allocated` (sum of allocated sizes in subtree)
///
/// For files, keeps the original `size` and `allocated_size` values.
///
/// This matches the historical UFFS behavior where directory sizes show the
/// total size of all files under them, not the directory's own metadata size.
///
/// # Requirements
///
/// The `DataFrame` must have these columns:
/// - `is_directory` (bool)
/// - `size` (u64)
/// - `allocated_size` (u64)
/// - `treesize` (u64)
/// - `tree_allocated` (u64)
///
/// # Errors
///
/// Returns an error if required columns are missing or the transformation
/// fails.
pub fn apply_directory_treesize(df: &DataFrame) -> Result<DataFrame> {
    use uffs_polars::{IntoLazy, col, lit, when};

    // Baseline-compatible output: apply treesize to ALL directories, including
    // reparse points. ADS entries keep the stream-specific size (not the
    // parent's treesize).
    let has_stream_name = df.column("stream_name").is_ok();

    let is_default_dir = if has_stream_name {
        col("is_directory").and(col("stream_name").eq(lit("")))
    } else {
        col("is_directory")
    };

    df.clone()
        .lazy()
        .with_column(
            when(is_default_dir.clone())
                .then(col("treesize"))
                .otherwise(col("size"))
                .alias("size"),
        )
        .with_column(
            when(is_default_dir)
                .then(col("tree_allocated"))
                .otherwise(col("allocated_size"))
                .alias("allocated_size"),
        )
        .collect()
        .map_err(crate::CoreError::Polars)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test code uses unwrap on controlled data"
)]
#[expect(clippy::print_stdout, reason = "benchmark test outputs timing info")]
#[expect(clippy::use_debug, reason = "benchmark test outputs debug info")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "test data fits in target types"
)]
#[expect(
    clippy::shadow_unrelated,
    reason = "test variables reused across sections"
)]
mod tests;
