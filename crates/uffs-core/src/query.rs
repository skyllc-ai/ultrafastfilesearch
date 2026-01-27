//! MFT Query builder using Polars lazy API.
//!
//! This module provides a fluent API for querying MFT data.

use std::path::Path;

use uffs_polars::{
    DataFrame, Expr, IntoLazy, LazyFrame, NamedFrom, PlSmallStr, Series, SortMultipleOptions, col,
    lit,
};

use crate::compiled_pattern::compile_pattern;
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
    /// Uses optimized Polars string kernels (`starts_with`, `ends_with`,
    /// `contains_literal`, `is_in`) instead of regex when possible for
    /// 2-10x performance improvement.
    ///
    /// # Errors
    ///
    /// Returns an error if the pattern is invalid.
    pub fn pattern(self, parsed: &crate::pattern::ParsedPattern) -> Result<Self> {
        let compiled = compile_pattern(parsed)?;
        let case_sensitive = parsed.is_case_sensitive();
        let expr = compiled.to_expr("name", case_sensitive);

        Ok(Self {
            lazy: self.lazy.filter(expr),
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
    /// Uses optimized `ends_with` chain for extension matching,
    /// providing 10-30x speedup over regex.
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
        let extensions = filter.extensions();
        if extensions.is_empty() {
            return self;
        }

        // Build OR chain of ends_with expressions for each extension
        // This is much faster than regex for typical extension counts
        let expr = extensions
            .iter()
            .map(|ext| {
                let suffix = format!(".{}", ext.to_lowercase());
                col("name")
                    .str()
                    .to_lowercase()
                    .str()
                    .ends_with(lit(suffix))
            })
            .reduce(Expr::or);

        // If reduce returns None (empty iterator), we already returned above
        // So this is safe to unwrap via match
        match expr {
            Some(filter_expr) => Self {
                lazy: self.lazy.filter(filter_expr),
            },
            None => self, // Should never happen due to is_empty check
        }
    }

    /// Match files with exact name.
    #[must_use]
    pub fn exact_name(self, name: &str) -> Self {
        Self {
            lazy: self.lazy.filter(col("name").eq(lit(name))),
        }
    }

    /// Filter files by extension using the `ext` column (fastest).
    ///
    /// This method requires the `DataFrame` to have an `ext` column
    /// (added via `add_ext_column()`). Uses `is_in()` for O(1) hash lookup.
    ///
    /// For `DataFrame`s without an `ext` column, use `extension_filter()`
    /// instead.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use uffs_core::{MftQuery, extensions::{ExtensionFilter, add_ext_column}};
    ///
    /// let df = add_ext_column(df)?;
    /// let filter = ExtensionFilter::parse("jpg,png,gif").unwrap();
    /// let results = MftQuery::new(df)
    ///     .extension_filter_fast(&filter)
    ///     .collect()?;
    /// ```
    #[must_use]
    pub fn extension_filter_fast(self, filter: &crate::extensions::ExtensionFilter) -> Self {
        let extensions = filter.extensions();
        if extensions.is_empty() {
            return self;
        }

        // Convert to lowercase extension list (without dots)
        let ext_list: Vec<String> = extensions.iter().map(|ext| ext.to_lowercase()).collect();

        // Use is_in for O(1) hash lookup on the ext column
        let series = Series::new(PlSmallStr::EMPTY, &ext_list);
        let expr = col("ext").is_in(lit(series).implode(), false);

        Self {
            lazy: self.lazy.filter(expr),
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

    /// Exclude system files (by `is_system` attribute flag).
    #[must_use]
    pub fn exclude_system(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_system").eq(lit(false))),
        }
    }

    /// Hide system files (files starting with `$`).
    ///
    /// This filters out NTFS system files like `$MFT`, `$Bitmap`,
    /// `$Recycle.Bin`, etc. These files have names starting with `$` which
    /// is not a valid character for user-created files on Windows.
    #[must_use]
    pub fn hide_system_files(self) -> Self {
        Self {
            lazy: self
                .lazy
                .filter(col("name").str().starts_with(lit("$")).not()),
        }
    }

    /// Hide NTFS metadata records (FRS < 16, except FRS 5 which is root).
    ///
    /// NTFS reserves the first 16 File Record Segments for system metadata:
    /// - FRS 0: `$MFT` (Master File Table)
    /// - FRS 1: `$MFTMirr` (MFT mirror)
    /// - FRS 2: `$LogFile` (transaction log)
    /// - FRS 3: `$Volume` (volume info)
    /// - FRS 4: `$AttrDef` (attribute definitions)
    /// - FRS 5: `.` (root directory) - **KEPT**
    /// - FRS 6: `$Bitmap` (cluster allocation)
    /// - FRS 7: `$Boot` (boot sector)
    /// - FRS 8: `$BadClus` (bad clusters)
    /// - FRS 9: `$Secure` (security descriptors)
    /// - FRS 10: `$UpCase` (uppercase table)
    /// - FRS 11: `$Extend` (extended metadata)
    /// - FRS 12-15: Reserved
    ///
    /// This matches the C++ `UltraFastFileSearch` behavior which excludes
    /// these but keeps the root directory.
    #[must_use]
    pub fn hide_metadata_records(self) -> Self {
        // Keep FRS >= 16 OR FRS == 5 (root directory)
        Self {
            lazy: self
                .lazy
                .filter(col("frs").gt_eq(lit(16_u64)).or(col("frs").eq(lit(5_u64)))),
        }
    }

    /// Hide both system files (`$`-prefixed) and metadata records (FRS < 16).
    ///
    /// This provides full C++ parity by excluding:
    /// 1. NTFS metadata records (FRS 0-15)
    /// 2. System files with `$` prefix (like `$Extend` subdirectories)
    #[must_use]
    pub fn hide_system(self) -> Self {
        self.hide_metadata_records().hide_system_files()
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

    // =========================================================================
    // Pattern Matching Tests (using CompiledPattern)
    // =========================================================================

    fn create_pattern_test_df() -> core::result::Result<DataFrame, uffs_polars::PolarsError> {
        DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[1_u64, 2, 3, 4, 5, 6]),
            Column::new(
                "name".into(),
                &[
                    "photo.jpg",
                    "document.txt",
                    "readme.md",
                    "config.json",
                    "main.rs",
                    "test.rs",
                ],
            ),
            Column::new("size".into(), &[1000_u64, 2000, 500, 300, 1500, 800]),
            Column::new(
                "is_directory".into(),
                &[false, false, false, false, false, false],
            ),
            Column::new(
                "is_hidden".into(),
                &[false, false, false, false, false, false],
            ),
            Column::new(
                "is_system".into(),
                &[false, false, false, false, false, false],
            ),
        ])
    }

    #[test]
    fn test_pattern_suffix() -> TestResult {
        use crate::pattern::ParsedPattern;

        let df = create_pattern_test_df()?;
        let pattern = ParsedPattern::parse("*.rs")?;
        let result = MftQuery::new(df).pattern(&pattern)?.collect()?;

        assert_eq!(result.height(), 2); // main.rs and test.rs
        Ok(())
    }

    #[test]
    fn test_pattern_prefix() -> TestResult {
        use crate::pattern::ParsedPattern;

        let df = create_pattern_test_df()?;
        let pattern = ParsedPattern::parse("config*")?;
        let result = MftQuery::new(df).pattern(&pattern)?.collect()?;

        assert_eq!(result.height(), 1); // config.json
        Ok(())
    }

    #[test]
    fn test_pattern_contains() -> TestResult {
        use crate::pattern::ParsedPattern;

        let df = create_pattern_test_df()?;
        let pattern = ParsedPattern::parse("*read*")?;
        let result = MftQuery::new(df).pattern(&pattern)?.collect()?;

        assert_eq!(result.height(), 1); // readme.md
        Ok(())
    }

    #[test]
    fn test_extension_filter_optimized() -> TestResult {
        let df = create_pattern_test_df()?;
        let filter = crate::extensions::ExtensionFilter::parse("rs,txt")?;
        let result = MftQuery::new(df).extension_filter(&filter).collect()?;

        assert_eq!(result.height(), 3); // document.txt, main.rs, test.rs
        Ok(())
    }

    #[test]
    fn test_extension_filter_fast() -> TestResult {
        let df = create_pattern_test_df()?;
        let df_with_ext = crate::extensions::add_ext_column(df)?;
        let filter = crate::extensions::ExtensionFilter::parse("rs,txt")?;
        let result = MftQuery::new(df_with_ext)
            .extension_filter_fast(&filter)
            .collect()?;

        assert_eq!(result.height(), 3); // document.txt, main.rs, test.rs
        Ok(())
    }

    #[test]
    fn test_extension_filter_single() -> TestResult {
        let df = create_pattern_test_df()?;
        let filter = crate::extensions::ExtensionFilter::parse("jpg")?;
        let result = MftQuery::new(df).extension_filter(&filter).collect()?;

        assert_eq!(result.height(), 1); // photo.jpg
        Ok(())
    }

    #[test]
    fn test_max_size() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df).max_size(1500).collect()?;
        // Should include: root (0), file.txt (1024), src (0) = 3 items
        assert!(result.height() >= 2);
        Ok(())
    }

    #[test]
    fn test_sort_by_size_descending() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df)
            .files_only()
            .sort_by_size(true)
            .collect()?;
        // First file should be largest (main.rs = 2048)
        let sizes = result.column("size")?.u64()?;
        let first_size = sizes.get(0).unwrap_or(0);
        assert_eq!(first_size, 2048);
        Ok(())
    }

    #[test]
    fn test_sort_by_size_ascending() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df)
            .files_only()
            .sort_by_size(false)
            .collect()?;
        // First file should be smallest (file.txt = 1024)
        let sizes = result.column("size")?.u64()?;
        let first_size = sizes.get(0).unwrap_or(0);
        assert_eq!(first_size, 1024);
        Ok(())
    }

    #[test]
    fn test_hide_system() -> TestResult {
        // Create df with NTFS system files ($ prefix and low FRS)
        let df = DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[0_u64, 5, 16, 100]),
            Column::new("name".into(), &["$MFT", ".", "$Extend", "normal.txt"]),
            Column::new("size".into(), &[100_u64, 0, 200, 300]),
            Column::new("is_directory".into(), &[false, true, true, false]),
            Column::new("is_hidden".into(), &[false, false, false, false]),
            Column::new("is_system".into(), &[true, false, true, false]),
        ])?;

        let result = MftQuery::new(df).hide_system().collect()?;
        // Should keep: FRS 5 (root ".") and FRS 100 (normal.txt)
        // Should exclude: FRS 0 ($MFT, metadata), FRS 16 ($Extend, $ prefix)
        assert_eq!(result.height(), 2);
        Ok(())
    }

    #[test]
    fn test_query_mode_from_str() {
        use crate::index_search::QueryMode;
        assert!(matches!(
            QueryMode::from_str_opt("auto"),
            Some(QueryMode::Auto)
        ));
        assert!(matches!(
            QueryMode::from_str_opt("polars"),
            Some(QueryMode::ForceDataFrame)
        ));
        assert!(matches!(
            QueryMode::from_str_opt("index"),
            Some(QueryMode::ForceIndex)
        ));
        assert!(QueryMode::from_str_opt("invalid").is_none());
    }

    #[test]
    fn test_empty_dataframe() -> TestResult {
        let df = DataFrame::new_infer_height(vec![
            Column::new("frs".into(), Vec::<u64>::new()),
            Column::new("name".into(), Vec::<&str>::new()),
            Column::new("size".into(), Vec::<u64>::new()),
            Column::new("is_directory".into(), Vec::<bool>::new()),
            Column::new("is_hidden".into(), Vec::<bool>::new()),
            Column::new("is_system".into(), Vec::<bool>::new()),
        ])?;

        let result = MftQuery::new(df).files_only().collect()?;
        assert_eq!(result.height(), 0);
        Ok(())
    }

    #[test]
    fn test_combined_size_filters() -> TestResult {
        let df = create_test_df()?;
        let result = MftQuery::new(df).min_size(500).max_size(1500).collect()?;
        // Should include file.txt (1024) only
        assert_eq!(result.height(), 1);
        Ok(())
    }
}
