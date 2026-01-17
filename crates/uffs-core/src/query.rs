//! MFT Query builder using Polars lazy API.
//!
//! This module provides a fluent API for querying MFT data.

use std::path::Path;

use uffs_polars::{DataFrame, Expr, IntoLazy, LazyFrame, SortMultipleOptions, col, lit};

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
    /// The underlying lazy frame for query operations.
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
    pub const fn from_lazy(lazy: LazyFrame) -> Self {
        Self { lazy }
    }

    /// Load query from a Parquet file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub fn from_parquet<P: AsRef<Path>>(path: P) -> Result<Self> {
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
            lazy: self
                .lazy
                .filter(col("name").str().contains(lit(regex), false)),
        })
    }

    /// Match files by regex pattern.
    #[must_use]
    pub fn regex(self, pattern: &str) -> Self {
        Self {
            lazy: self
                .lazy
                .filter(col("name").str().contains(lit(pattern), false)),
        }
    }

    /// Match files by regex pattern with case sensitivity control.
    #[must_use]
    pub fn regex_with_case(self, pattern: &str, case_sensitive: bool) -> Self {
        // Polars str().contains() second arg is `literal` not `case_sensitive`
        // For case-insensitive, we need to use (?i) prefix in regex
        let regex_pattern = if case_sensitive {
            pattern.to_owned()
        } else {
            format!("(?i){pattern}")
        };
        Self {
            lazy: self
                .lazy
                .filter(col("name").str().contains(lit(regex_pattern), false)),
        }
    }

    /// Match files using a parsed pattern (supports glob, regex, and literal).
    ///
    /// This is the recommended method for user-provided patterns as it
    /// automatically handles pattern type detection and case sensitivity.
    ///
    /// # Errors
    ///
    /// Returns an error if the pattern is invalid.
    pub fn pattern(self, parsed: &crate::pattern::ParsedPattern) -> Result<Self> {
        let regex = parsed.to_regex()?;
        let case_sensitive = parsed.is_case_sensitive();

        // For case-insensitive matching, prepend (?i)
        let final_regex = if case_sensitive {
            regex
        } else {
            format!("(?i){regex}")
        };

        Ok(Self {
            lazy: self
                .lazy
                .filter(col("name").str().contains(lit(final_regex), false)),
        })
    }

    /// Match files containing exact substring (fastest).
    #[must_use]
    pub fn contains(self, substring: &str) -> Self {
        Self {
            lazy: self
                .lazy
                .filter(col("name").str().contains_literal(lit(substring))),
        }
    }

    /// Filter files by extension(s).
    ///
    /// Accepts an `ExtensionFilter` which can contain individual extensions
    /// or collection aliases like "pictures", "documents", "videos", "music".
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use uffs_core::{MftQuery, extensions::ExtensionFilter};
    ///
    /// let filter = ExtensionFilter::parse("pictures,mp4").unwrap();
    /// let results = MftQuery::new(df)
    ///     .extension_filter(&filter)
    ///     .collect()?;
    /// ```
    #[must_use]
    pub fn extension_filter(self, filter: &crate::extensions::ExtensionFilter) -> Self {
        let regex = filter.to_regex();
        if regex.is_empty() {
            return self;
        }
        // Case-insensitive extension matching
        let regex_pattern = format!("(?i){regex}");
        Self {
            lazy: self
                .lazy
                .filter(col("name").str().contains(lit(regex_pattern), false)),
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
            lazy: self.lazy.filter(col("is_directory").eq(lit(false))),
        }
    }

    /// Filter to directories only.
    #[must_use]
    pub fn directories_only(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_directory").eq(lit(true))),
        }
    }

    /// Exclude hidden files.
    #[must_use]
    pub fn exclude_hidden(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_hidden").eq(lit(false))),
        }
    }

    /// Exclude system files.
    #[must_use]
    pub fn exclude_system(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_system").eq(lit(false))),
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
    // Date Filters
    // =========================================================================

    /// Filter files modified after a given timestamp (Unix microseconds).
    #[must_use]
    pub fn modified_after(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("modified").gt(lit(timestamp_micros))),
        }
    }

    /// Filter files modified before a given timestamp (Unix microseconds).
    #[must_use]
    pub fn modified_before(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("modified").lt(lit(timestamp_micros))),
        }
    }

    /// Filter files created after a given timestamp (Unix microseconds).
    #[must_use]
    pub fn created_after(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("created").gt(lit(timestamp_micros))),
        }
    }

    /// Filter files created before a given timestamp (Unix microseconds).
    #[must_use]
    pub fn created_before(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("created").lt(lit(timestamp_micros))),
        }
    }

    /// Filter files accessed after a given timestamp (Unix microseconds).
    #[must_use]
    pub fn accessed_after(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("accessed").gt(lit(timestamp_micros))),
        }
    }

    /// Filter files accessed before a given timestamp (Unix microseconds).
    #[must_use]
    pub fn accessed_before(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("accessed").lt(lit(timestamp_micros))),
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
            .with_new_streaming(true)
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
        let cols: Vec<Expr> = columns.iter().map(|&col_name| col(col_name)).collect();
        Self {
            lazy: self.lazy.select(cols),
        }
    }
}

#[cfg(test)]
mod tests {
    use uffs_polars::Column;

    use super::*;

    type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

    fn create_test_df() -> core::result::Result<DataFrame, uffs_polars::PolarsError> {
        DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[1_u64, 2, 3, 4]),
            Column::new("parent_frs".into(), &[0_u64, 0, 1, 1]),
            Column::new("name".into(), &["root", "file.txt", "src", "main.rs"]),
            Column::new("size".into(), &[0_u64, 1024, 0, 2048]),
            // Use boolean columns matching MFT reader schema
            Column::new("is_directory".into(), &[true, false, true, false]),
            Column::new("is_hidden".into(), &[false, false, false, false]),
            Column::new("is_system".into(), &[false, false, false, false]),
        ])
    }

    #[test]
    fn test_files_only() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df).files_only().collect()?;
        assert_eq!(result.height(), 2); // file.txt and main.rs
        Ok(())
    }

    #[test]
    fn test_directories_only() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df).directories_only().collect()?;
        assert_eq!(result.height(), 2); // root and src
        Ok(())
    }

    #[test]
    fn test_min_size() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df).min_size(1500).collect()?;
        assert_eq!(result.height(), 1); // only main.rs (2048)
        Ok(())
    }

    #[test]
    fn test_limit() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df).limit(2).collect()?;
        assert_eq!(result.height(), 2);
        Ok(())
    }

    #[test]
    fn test_chained_filters() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df)
            .files_only()
            .min_size(500)
            .sort_by_size(true)
            .limit(10)
            .collect()?;
        assert_eq!(result.height(), 2);
        Ok(())
    }
}
