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

use std::collections::HashMap;

use rayon::prelude::*;
use uffs_polars::{Column, DataFrame};

use crate::error::Result;

/// Tree-derived columns that can be computed on-demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeColumn {
    /// Count of all items (files + directories) under this directory.
    Descendants,
    /// Sum of logical file sizes under this directory.
    TreeSize,
    /// Sum of allocated sizes under this directory.
    TreeAllocated,
    /// Fragmentation metric: `tree_allocated` / `treesize` ratio.
    /// Higher values indicate more fragmentation/overhead.
    Bulkiness,
}

impl TreeColumn {
    /// Get the `DataFrame` column name for this tree column.
    #[must_use]
    pub const fn column_name(&self) -> &'static str {
        match self {
            Self::Descendants => "descendants",
            Self::TreeSize => "treesize",
            Self::TreeAllocated => "tree_allocated",
            Self::Bulkiness => "bulkiness",
        }
    }

    /// Parse a column name into a `TreeColumn`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "descendants" | "decendents" => Some(Self::Descendants),
            "treesize" | "tree_size" => Some(Self::TreeSize),
            "treeallocated" | "tree_allocated" => Some(Self::TreeAllocated),
            "bulkiness" => Some(Self::Bulkiness),
            _ => None,
        }
    }
}

/// Metadata for a single node in the tree.
#[derive(Debug, Clone, Copy, Default)]
struct NodeInfo {
    /// Whether this node is a directory.
    is_directory: bool,
    /// Logical file size (0 for directories).
    size: u64,
    /// Allocated size on disk.
    allocated_size: u64,
}

/// Computed tree metrics for a node.
#[derive(Debug, Clone, Copy, Default)]
struct TreeMetrics {
    /// Count of all descendants.
    descendants: u64,
    /// Sum of logical sizes in subtree.
    treesize: u64,
    /// Sum of allocated sizes in subtree.
    tree_allocated: u64,
    /// Filtered bulkiness sum (excludes large files >= 1% of folder size).
    /// This matches the historical algorithm for identifying fragmented
    /// folders.
    bulkiness_sum: u64,
}

/// Result of building tree column vectors.
/// Used internally to pass computed columns between methods.
#[derive(Default)]
struct TreeColumnVecs {
    /// Descendants count column (directories only).
    descendants: Option<Vec<u64>>,
    /// Tree size column (sum of sizes in subtree).
    treesize: Option<Vec<u64>>,
    /// Tree allocated column (sum of allocated sizes in subtree).
    tree_allocated: Option<Vec<u64>>,
    /// Bulkiness column (fragmented space from small files).
    bulkiness: Option<Vec<u64>>,
}

/// Tree index for efficient parent-child traversal and metric computation.
///
/// Built from a `DataFrame`, this structure enables O(1) child lookup
/// and memoized metric calculations.
pub struct TreeIndex {
    /// Map from FRS to list of child FRS values.
    children: HashMap<u64, Vec<u64>>,
    /// Map from FRS to node metadata.
    nodes: HashMap<u64, NodeInfo>,
    /// Cached tree metrics (computed on demand).
    metrics_cache: HashMap<u64, TreeMetrics>,
}

#[expect(
    clippy::single_call_fn,
    reason = "helper functions separated for clarity and sequential/parallel threshold switching"
)]
impl TreeIndex {
    /// Build a tree index from a `DataFrame`.
    ///
    /// Required columns: `frs`, `parent_frs`, `is_directory`, `size`,
    /// `allocated_size`
    ///
    /// Uses Rayon for parallel processing of large datasets.
    ///
    /// # Errors
    ///
    /// Returns an error if required columns are missing or have wrong types.
    pub fn from_dataframe(df: &DataFrame) -> Result<Self> {
        let frs_col = df.column("frs")?.u64()?;
        let parent_col = df.column("parent_frs")?.u64()?;
        let is_dir_col = df.column("is_directory")?.bool()?;
        let size_col = df.column("size")?.u64()?;
        let alloc_col = df.column("allocated_size")?.u64()?;

        let height = df.height();

        // Use parallel iteration for large datasets (>10K rows)
        // For smaller datasets, sequential is faster due to overhead
        let tree = if height > 10_000 {
            Self::from_dataframe_parallel(
                frs_col, parent_col, is_dir_col, size_col, alloc_col, height,
            )
        } else {
            Self::from_dataframe_sequential(
                frs_col, parent_col, is_dir_col, size_col, alloc_col, height,
            )
        };
        Ok(tree)
    }

    /// Sequential implementation for small datasets.
    fn from_dataframe_sequential(
        frs_col: &uffs_polars::UInt64Chunked,
        parent_col: &uffs_polars::UInt64Chunked,
        is_dir_col: &uffs_polars::BooleanChunked,
        size_col: &uffs_polars::UInt64Chunked,
        alloc_col: &uffs_polars::UInt64Chunked,
        height: usize,
    ) -> Self {
        let mut children: HashMap<u64, Vec<u64>> = HashMap::with_capacity(height / 10);
        let mut nodes: HashMap<u64, NodeInfo> = HashMap::with_capacity(height);

        for idx in 0..height {
            let Some(frs) = frs_col.get(idx) else {
                continue;
            };
            let parent = parent_col.get(idx).unwrap_or(0);
            let is_directory = is_dir_col.get(idx).unwrap_or(false);
            let size = size_col.get(idx).unwrap_or(0);
            let allocated_size = alloc_col.get(idx).unwrap_or(0);

            // Add to parent's children list (skip self-references like root)
            if frs != parent {
                children.entry(parent).or_default().push(frs);
            }

            nodes.insert(
                frs,
                NodeInfo {
                    is_directory,
                    size,
                    allocated_size,
                },
            );
        }

        Self {
            children,
            nodes,
            metrics_cache: HashMap::with_capacity(height / 10),
        }
    }

    /// Parallel implementation using Rayon for large datasets.
    fn from_dataframe_parallel(
        frs_col: &uffs_polars::UInt64Chunked,
        parent_col: &uffs_polars::UInt64Chunked,
        is_dir_col: &uffs_polars::BooleanChunked,
        size_col: &uffs_polars::UInt64Chunked,
        alloc_col: &uffs_polars::UInt64Chunked,
        height: usize,
    ) -> Self {
        // Collect data into vectors first for parallel processing
        // This allows Rayon to split the work efficiently
        let data: Vec<(u64, u64, bool, u64, u64)> = (0..height)
            .filter_map(|idx| {
                let frs = frs_col.get(idx)?;
                let parent = parent_col.get(idx).unwrap_or(0);
                let is_directory = is_dir_col.get(idx).unwrap_or(false);
                let size = size_col.get(idx).unwrap_or(0);
                let allocated_size = alloc_col.get(idx).unwrap_or(0);
                Some((frs, parent, is_directory, size, allocated_size))
            })
            .collect();

        // Build nodes HashMap in parallel using fold + reduce
        let nodes: HashMap<u64, NodeInfo> = data
            .par_iter()
            .fold(
                || HashMap::with_capacity(data.len() / rayon::current_num_threads()),
                |mut map, &(frs, _, is_directory, size, allocated_size)| {
                    map.insert(
                        frs,
                        NodeInfo {
                            is_directory,
                            size,
                            allocated_size,
                        },
                    );
                    map
                },
            )
            .reduce(HashMap::new, |mut acc, map| {
                acc.extend(map);
                acc
            });

        // Build children HashMap in parallel using fold + reduce
        #[expect(
            clippy::iter_over_hash_type,
            reason = "order doesn't matter for merging child lists"
        )]
        let children: HashMap<u64, Vec<u64>> = data
            .par_iter()
            .filter(|(frs, parent, _, _, _)| frs != parent) // Skip self-references
            .fold(
                || {
                    HashMap::<u64, Vec<u64>>::with_capacity(
                        data.len() / 10 / rayon::current_num_threads(),
                    )
                },
                |mut map, &(frs, parent, _, _, _)| {
                    map.entry(parent).or_default().push(frs);
                    map
                },
            )
            .reduce(HashMap::new, |mut acc, map| {
                for (parent, mut child_list) in map {
                    acc.entry(parent).or_default().append(&mut child_list);
                }
                acc
            });

        Self {
            children,
            nodes,
            metrics_cache: HashMap::with_capacity(height / 10),
        }
    }

    /// Compute tree metrics for a given FRS.
    ///
    /// Uses memoization to avoid recomputation. For files, returns
    /// metrics with just the file's own size. For directories,
    /// recursively computes metrics for all descendants.
    ///
    /// # Bulkiness Algorithm (matches the historical baseline)
    ///
    /// For directories, bulkiness is computed by:
    /// 1. Summing all children's allocated sizes
    /// 2. Filtering out large files (>= 1% of folder's total allocated size)
    /// 3. The remaining sum identifies "fragmented" space from small files
    fn compute_metrics(&mut self, frs: u64) -> TreeMetrics {
        // Check cache first
        if let Some(&metrics) = self.metrics_cache.get(&frs) {
            return metrics;
        }

        // Get node info
        let node = self.nodes.get(&frs).copied().unwrap_or_default();

        // Base metrics from this node (files contribute their own allocated size to
        // bulkiness)
        let mut metrics = TreeMetrics {
            descendants: 0,
            treesize: node.size,
            tree_allocated: node.allocated_size,
            bulkiness_sum: node.allocated_size,
        };

        // If this is a directory, add children's metrics
        if node.is_directory {
            // Clone children list to avoid borrow issues
            let child_frs_list: Vec<u64> = self.children.get(&frs).cloned().unwrap_or_default();

            // Collect children's bulkiness values for threshold filtering
            let mut children_bulkiness: Vec<u64> = Vec::with_capacity(child_frs_list.len());
            let mut children_bulkiness_total: u64 = 0;

            for child_frs in child_frs_list {
                let child_metrics = self.compute_metrics(child_frs);
                metrics.descendants = metrics.descendants.saturating_add(1);
                metrics.descendants = metrics
                    .descendants
                    .saturating_add(child_metrics.descendants);
                metrics.treesize = metrics.treesize.saturating_add(child_metrics.treesize);
                metrics.tree_allocated = metrics
                    .tree_allocated
                    .saturating_add(child_metrics.tree_allocated);

                // Collect bulkiness for threshold filtering
                children_bulkiness.push(child_metrics.bulkiness_sum);
                children_bulkiness_total =
                    children_bulkiness_total.saturating_add(child_metrics.bulkiness_sum);
            }

            // Apply the historical bulkiness algorithm: filter out large files >= 1% of
            // folder size This identifies folders with many small fragmented
            // files
            let threshold = metrics.tree_allocated / 100; // 1% threshold

            if threshold > 0 && !children_bulkiness.is_empty() {
                // Sort descending to efficiently find and remove large values
                children_bulkiness.sort_unstable_by(|lhs, rhs| rhs.cmp(lhs));

                // Remove values >= threshold from the total
                for &val in &children_bulkiness {
                    if val < threshold {
                        break; // All remaining values are below threshold (sorted desc)
                    }
                    children_bulkiness_total = children_bulkiness_total.saturating_sub(val);
                }
            }

            metrics.bulkiness_sum = children_bulkiness_total;
        }

        self.metrics_cache.insert(frs, metrics);
        metrics
    }

    /// Get descendants count for a given FRS.
    pub fn descendants(&mut self, frs: u64) -> u64 {
        let node = self.nodes.get(&frs).copied().unwrap_or_default();
        if !node.is_directory {
            return 0;
        }
        self.compute_metrics(frs).descendants
    }

    /// Get `treesize` (sum of logical sizes) for a given FRS.
    pub fn treesize(&mut self, frs: u64) -> u64 {
        self.compute_metrics(frs).treesize
    }

    /// Get `tree_allocated` (sum of allocated sizes) for a given FRS.
    pub fn tree_allocated(&mut self, frs: u64) -> u64 {
        self.compute_metrics(frs).tree_allocated
    }

    /// Get `bulkiness` sum for a given FRS.
    ///
    /// This is the filtered sum of allocated sizes, excluding large files
    /// that are >= 1% of the folder's total allocated size. This matches
    /// the historical baseline algorithm for identifying fragmented folders.
    pub fn bulkiness(&mut self, frs: u64) -> u64 {
        self.compute_metrics(frs).bulkiness_sum
    }

    /// Add tree columns to a `DataFrame`.
    ///
    /// Only computes the requested columns for efficiency.
    /// Uses Rayon for parallel column building on large datasets.
    ///
    /// # Arguments
    ///
    /// * `df` - The source `DataFrame` (must have `frs` column)
    /// * `columns` - Which tree columns to add
    ///
    /// # Errors
    ///
    /// Returns an error if the `frs` column is missing.
    pub fn add_columns(&mut self, df: &DataFrame, columns: &[TreeColumn]) -> Result<DataFrame> {
        if columns.is_empty() {
            return Ok(df.clone());
        }

        let frs_col = df.column("frs")?.u64()?;
        let height = df.height();

        // Step 1: Pre-compute all metrics (populates the cache)
        // This must be done sequentially due to mutable borrow requirements
        for idx in 0..height {
            let frs = frs_col.get(idx).unwrap_or(0);
            self.compute_metrics(frs);
        }

        // Collect FRS values for parallel processing
        let frs_values: Vec<u64> = (0..height)
            .map(|idx| frs_col.get(idx).unwrap_or(0))
            .collect();

        // Step 2: Build column vectors (parallel for large datasets)
        let vecs = if height > 10_000 {
            self.build_columns_parallel(&frs_values, columns)
        } else {
            self.build_columns_sequential(&frs_values, columns)
        };

        // Add columns to DataFrame
        let mut result = df.clone();

        if let Some(vec) = vecs.descendants {
            result.with_column(Column::new("descendants".into(), vec))?;
        }
        if let Some(vec) = vecs.treesize {
            result.with_column(Column::new("treesize".into(), vec))?;
        }
        if let Some(vec) = vecs.tree_allocated {
            result.with_column(Column::new("tree_allocated".into(), vec))?;
        }
        if let Some(vec) = vecs.bulkiness {
            result.with_column(Column::new("bulkiness".into(), vec))?;
        }

        Ok(result)
    }

    /// Build column vectors sequentially (for small datasets).
    fn build_columns_sequential(
        &self,
        frs_values: &[u64],
        columns: &[TreeColumn],
    ) -> TreeColumnVecs {
        let height = frs_values.len();
        let need_descendants = columns.contains(&TreeColumn::Descendants);
        let need_treesize = columns.contains(&TreeColumn::TreeSize);
        let need_tree_allocated = columns.contains(&TreeColumn::TreeAllocated);
        let need_bulkiness = columns.contains(&TreeColumn::Bulkiness);

        let mut descendants_vec = need_descendants.then(|| Vec::with_capacity(height));
        let mut treesize_vec = need_treesize.then(|| Vec::with_capacity(height));
        let mut tree_allocated_vec = need_tree_allocated.then(|| Vec::with_capacity(height));
        let mut bulkiness_vec = need_bulkiness.then(|| Vec::with_capacity(height));

        for &frs in frs_values {
            let metrics = self.metrics_cache.get(&frs).copied().unwrap_or_default();
            let node = self.nodes.get(&frs).copied().unwrap_or_default();

            if let Some(ref mut vec) = descendants_vec {
                vec.push(if node.is_directory {
                    metrics.descendants
                } else {
                    0
                });
            }
            if let Some(ref mut vec) = treesize_vec {
                vec.push(metrics.treesize);
            }
            if let Some(ref mut vec) = tree_allocated_vec {
                vec.push(metrics.tree_allocated);
            }
            if let Some(ref mut vec) = bulkiness_vec {
                vec.push(metrics.bulkiness_sum);
            }
        }

        TreeColumnVecs {
            descendants: descendants_vec,
            treesize: treesize_vec,
            tree_allocated: tree_allocated_vec,
            bulkiness: bulkiness_vec,
        }
    }

    /// Build column vectors in parallel using Rayon (for large datasets).
    fn build_columns_parallel(&self, frs_values: &[u64], columns: &[TreeColumn]) -> TreeColumnVecs {
        let need_descendants = columns.contains(&TreeColumn::Descendants);
        let need_treesize = columns.contains(&TreeColumn::TreeSize);
        let need_tree_allocated = columns.contains(&TreeColumn::TreeAllocated);
        let need_bulkiness = columns.contains(&TreeColumn::Bulkiness);

        // Build all requested columns in a single parallel pass
        let results: Vec<(u64, u64, u64, u64)> = frs_values
            .par_iter()
            .map(|&frs| {
                let metrics = self.metrics_cache.get(&frs).copied().unwrap_or_default();
                let node = self.nodes.get(&frs).copied().unwrap_or_default();
                let descendants = if node.is_directory {
                    metrics.descendants
                } else {
                    0
                };
                (
                    descendants,
                    metrics.treesize,
                    metrics.tree_allocated,
                    metrics.bulkiness_sum,
                )
            })
            .collect();

        // Extract into separate vectors based on what's needed
        TreeColumnVecs {
            descendants: need_descendants
                .then(|| results.iter().map(|(desc, _, _, _)| *desc).collect()),
            treesize: need_treesize.then(|| results.iter().map(|(_, ts, _, _)| *ts).collect()),
            tree_allocated: need_tree_allocated
                .then(|| results.iter().map(|(_, _, ta, _)| *ta).collect()),
            bulkiness: need_bulkiness
                .then(|| results.iter().map(|(_, _, _, bulk)| *bulk).collect()),
        }
    }
}

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
