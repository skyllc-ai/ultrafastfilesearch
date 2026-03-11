//! Pattern, name, and extension matching for `MftQuery`.

use uffs_polars::{Expr, NamedFrom, PlSmallStr, Series, col, lit};

use super::MftQuery;
use crate::compiled_pattern::compile_pattern;
use crate::error::Result;
use crate::glob::glob_to_regex;

impl MftQuery {
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
            None => self,
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
        let expr = col("ext").is_in(lit(series).implode(true), false);

        Self {
            lazy: self.lazy.filter(expr),
        }
    }
}
