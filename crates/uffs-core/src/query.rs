//! MFT Query builder using Polars lazy API.
//!
//! This module provides a fluent API for querying MFT data.

use std::path::Path;

use uffs_mft::flags::raw_flags;
use uffs_polars::{
    col, lit, DataFrame, DataType, Expr, IntoLazy, LazyFrame, SortMultipleOptions,
};

use crate::error::{CoreError, Result};
use crate::glob::glob_to_regex;

/// Query builder for MFT data.
///
/// Wraps a Polars `LazyFrame` and provides a fluent API for common
/// file search operations.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_core::MftQuery;
///
/// let results = MftQuery::new(df)
///     .glob("*.rs")
///     .files_only()
///     .min_size(1024)
///     .sort_by_size(true)
///     .limit(100)
///     .collect()?;
/// ```
#[derive(Clone)]
pub struct MftQuery {
    lazy: LazyFrame,
}

impl MftQuery {
    /// Create a new query from a `DataFrame`.
    #[must_use]
    pub fn new(df: DataFrame) -> Self {
        Self { lazy: df.lazy() }
    }

    /// Create a new query from a `LazyFrame`.
    #[must_use]
    pub fn from_lazy(lazy: LazyFrame) -> Self {
        Self { lazy }
    }

    /// Load query from a Parquet file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub fn from_parquet(path: impl AsRef<Path>) -> Result<Self> {
        let df = uffs_mft::MftReader::load_parquet(path)?;
        Ok(Self::new(df))
    }

    // =========================================================================
    // Pattern Matching
    // =========================================================================

    /// Match files by glob pattern (e.g., "*.rs", "src/**/*.txt").
    ///
    /// # Errors
    ///
    /// Returns an error if the glob pattern is invalid.
    pub fn glob(self, pattern: &str) -> Result<Self> {
        let regex = glob_to_regex(pattern)?;
        Ok(Self {
            lazy: self.lazy.filter(col("name").str().contains(lit(regex), false)),
        })
    }

    /// Match files by regex pattern.
    #[must_use]
    pub fn regex(self, pattern: &str) -> Self {
        Self {
            lazy: self.lazy.filter(col("name").str().contains(lit(pattern), false)),
        }
    }

    /// Match files containing exact substring (fastest).
    #[must_use]
    pub fn contains(self, substring: &str) -> Self {
        Self {
            lazy: self.lazy.filter(col("name").str().contains_literal(lit(substring))),
        }
    }

    /// Match files with exact name.
    #[must_use]
    pub fn exact_name(self, name: &str) -> Self {
        Self {
            lazy: self.lazy.filter(col("name").eq(lit(name))),
        }
    }

    // =========================================================================
    // Type Filters
    // =========================================================================

    /// Filter to files only (exclude directories).
    #[must_use]
    pub fn files_only(self) -> Self {
        Self {
            lazy: self.lazy.filter(
                col("flags")
                    .cast(DataType::UInt16)
                    .and(lit(raw_flags::DIRECTORY))
                    .eq(lit(0u16)),
            ),
        }
    }

    /// Filter to directories only.
    #[must_use]
    pub fn directories_only(self) -> Self {
        Self {
            lazy: self.lazy.filter(
                col("flags")
                    .cast(DataType::UInt16)
                    .and(lit(raw_flags::DIRECTORY))
                    .neq(lit(0u16)),
            ),
        }
    }

    /// Exclude hidden files.
    #[must_use]
    pub fn exclude_hidden(self) -> Self {
        Self {
            lazy: self.lazy.filter(
                col("flags")
                    .cast(DataType::UInt16)
                    .and(lit(raw_flags::HIDDEN))
                    .eq(lit(0u16)),
            ),
        }
    }

    /// Exclude system files.
    #[must_use]
    pub fn exclude_system(self) -> Self {
        Self {
            lazy: self.lazy.filter(
                col("flags")
                    .cast(DataType::UInt16)
                    .and(lit(raw_flags::SYSTEM))
                    .eq(lit(0u16)),
            ),
        }
    }

    // =========================================================================
    // Size Filters
    // =========================================================================

    /// Filter files with size >= bytes.
    #[must_use]
    pub fn min_size(self, bytes: u64) -> Self {
        Self {
            lazy: self.lazy.filter(col("size").gt_eq(lit(bytes))),
        }
    }

    /// Filter files with size <= bytes.
    #[must_use]
    pub fn max_size(self, bytes: u64) -> Self {
        Self {
            lazy: self.lazy.filter(col("size").lt_eq(lit(bytes))),
        }
    }

    /// Filter files within size range.
    #[must_use]
    pub fn size_between(self, min: u64, max: u64) -> Self {
        Self {
            lazy: self
                .lazy
                .filter(col("size").gt_eq(lit(min)).and(col("size").lt_eq(lit(max)))),
        }
    }

    // =========================================================================
    // Sorting
    // =========================================================================

    /// Sort by file size.
    #[must_use]
    pub fn sort_by_size(self, descending: bool) -> Self {
        Self {
            lazy: self.lazy.sort(
                ["size"],
                SortMultipleOptions::default().with_order_descending(descending),
            ),
        }
    }

    /// Sort by file name.
    #[must_use]
    pub fn sort_by_name(self) -> Self {
        Self {
            lazy: self.lazy.sort(["name"], SortMultipleOptions::default()),
        }
    }

    /// Sort by modification time.
    #[must_use]
    pub fn sort_by_modified(self, descending: bool) -> Self {
        Self {
            lazy: self.lazy.sort(
                ["modified"],
                SortMultipleOptions::default().with_order_descending(descending),
            ),
        }
    }

    /// Sort by creation time.
    #[must_use]
    pub fn sort_by_created(self, descending: bool) -> Self {
        Self {
            lazy: self.lazy.sort(
                ["created"],
                SortMultipleOptions::default().with_order_descending(descending),
            ),
        }
    }

    // =========================================================================
    // Limiting
    // =========================================================================

    /// Limit the number of results.
    #[must_use]
    pub fn limit(self, n: u32) -> Self {
        Self {
            lazy: self.lazy.limit(n),
        }
    }

    /// Skip the first n results.
    #[must_use]
    pub fn offset(self, n: i64) -> Self {
        Self {
            lazy: self.lazy.slice(n, u32::MAX),
        }
    }

    // =========================================================================
    // Execution
    // =========================================================================

    /// Execute the query and return a `DataFrame`.
    ///
    /// # Errors
    ///
    /// Returns an error if query execution fails.
    pub fn collect(self) -> Result<DataFrame> {
        self.lazy.collect().map_err(CoreError::from)
    }

    /// Execute with streaming mode (memory efficient for large results).
    ///
    /// # Errors
    ///
    /// Returns an error if query execution fails.
    pub fn collect_streaming(self) -> Result<DataFrame> {
        self.lazy
            .with_streaming(true)
            .collect()
            .map_err(CoreError::from)
    }

    /// Get the underlying `LazyFrame` for advanced operations.
    pub fn into_lazy(self) -> LazyFrame {
        self.lazy
    }

    /// Select specific columns.
    #[must_use]
    pub fn select(self, columns: &[&str]) -> Self {
        let cols: Vec<Expr> = columns.iter().map(|&c| col(c)).collect();
        Self {
            lazy: self.lazy.select(cols),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uffs_polars::Column;

    fn create_test_df() -> DataFrame {
        DataFrame::new(vec![
            Column::new("frs".into(), &[1u64, 2, 3, 4]),
            Column::new("parent_frs".into(), &[0u64, 0, 1, 1]),
            Column::new("name".into(), &["root", "file.txt", "src", "main.rs"]),
            Column::new("size".into(), &[0u64, 1024, 0, 2048]),
            Column::new("flags".into(), &[0x0010u16, 0x0000, 0x0010, 0x0000]),
        ])
        .unwrap()
    }

    #[test]
    fn test_files_only() {
        let df = create_test_df();
        let result = MftQuery::new(df).files_only().collect().unwrap();
        assert_eq!(result.height(), 2); // file.txt and main.rs
    }

    #[test]
    fn test_directories_only() {
        let df = create_test_df();
        let result = MftQuery::new(df).directories_only().collect().unwrap();
        assert_eq!(result.height(), 2); // root and src
    }

    #[test]
    fn test_min_size() {
        let df = create_test_df();
        let result = MftQuery::new(df).min_size(1500).collect().unwrap();
        assert_eq!(result.height(), 1); // only main.rs (2048)
    }

    #[test]
    fn test_limit() {
        let df = create_test_df();
        let result = MftQuery::new(df).limit(2).collect().unwrap();
        assert_eq!(result.height(), 2);
    }

    #[test]
    fn test_chained_filters() {
        let df = create_test_df();
        let result = MftQuery::new(df)
            .files_only()
            .min_size(500)
            .sort_by_size(true)
            .limit(10)
            .collect()
            .unwrap();
        assert_eq!(result.height(), 2);
    }
}
