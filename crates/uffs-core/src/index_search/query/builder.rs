//! Fluent builder/configuration methods for `IndexQuery`.

use regex::Regex;

use super::{IndexQuery, TypeFilter};
use crate::error::Result;
use crate::index_search::{IndexPattern, compile_extensions, compile_index_pattern};

impl IndexQuery<'_> {
    /// Filter by glob pattern (e.g., `*.rs`, `foo*`, `*bar*`).
    #[must_use]
    pub fn glob(mut self, pattern: &str) -> Self {
        self.pattern = compile_index_pattern(pattern).ok();
        self
    }

    /// Filter by regex pattern.
    ///
    /// If the pattern is invalid, no filter is applied.
    #[must_use]
    pub fn regex(mut self, pattern: &str) -> Self {
        if let Ok(regex) = Regex::new(pattern) {
            let regex_lower =
                Regex::new(&format!("(?i){pattern}")).unwrap_or_else(|_| regex.clone());
            self.pattern = Some(IndexPattern::Regex { regex, regex_lower });
        }
        self
    }

    /// Filter by file extensions (e.g., `["rs", "toml"]`).
    #[must_use]
    pub fn extensions(mut self, exts: &[&str]) -> Self {
        self.pattern = Some(compile_extensions(exts));
        self
    }

    /// Only match files (not directories).
    #[must_use]
    pub const fn files_only(mut self) -> Self {
        self.options.type_filter = TypeFilter::FilesOnly;
        self
    }

    /// Only match directories (not files).
    #[must_use]
    pub const fn dirs_only(mut self) -> Self {
        self.options.type_filter = TypeFilter::DirsOnly;
        self
    }

    /// Filter by minimum size (bytes).
    #[must_use]
    pub const fn min_size(mut self, size: u64) -> Self {
        self.min_size = Some(size);
        self
    }

    /// Filter by maximum size (bytes).
    #[must_use]
    pub const fn max_size(mut self, size: u64) -> Self {
        self.max_size = Some(size);
        self
    }

    /// Limit the number of results.
    #[must_use]
    pub const fn limit(mut self, count: usize) -> Self {
        self.limit = Some(count);
        self
    }

    /// Enable case-sensitive matching (default: case-insensitive).
    #[must_use]
    pub const fn case_sensitive(mut self, yes: bool) -> Self {
        self.options.case_sensitive = yes;
        self
    }

    /// Resolve full paths for results (slower).
    #[must_use]
    pub const fn resolve_paths(mut self) -> Self {
        self.options.resolve_paths = true;
        self
    }

    /// Set whether to resolve full paths for results.
    #[must_use]
    pub const fn with_resolve_paths(mut self, resolve: bool) -> Self {
        self.options.resolve_paths = resolve;
        self
    }

    /// Set the pattern filter directly.
    #[must_use]
    pub fn with_pattern(mut self, pattern: IndexPattern) -> Self {
        self.pattern = Some(pattern);
        self
    }

    /// Set the pattern filter from a `Result`, ignoring errors.
    #[must_use]
    pub fn with_pattern_result(mut self, pattern: Result<IndexPattern>) -> Self {
        if let Ok(pat) = pattern {
            self.pattern = Some(pat);
        }
        self
    }

    /// Set the type filter.
    #[must_use]
    pub const fn with_type_filter(mut self, filter: TypeFilter) -> Self {
        self.options.type_filter = filter;
        self
    }

    /// Enable/disable hard link expansion (default: true).
    ///
    /// When enabled, files with multiple hard links produce multiple results,
    /// one for each path.
    #[must_use]
    pub const fn with_expand_names(mut self, expand: bool) -> Self {
        self.options.expand_names = expand;
        self
    }

    /// Enable/disable ADS expansion (default: true).
    ///
    /// When enabled, files with Alternate Data Streams produce multiple
    /// results, one for each stream.
    #[must_use]
    pub const fn with_expand_streams(mut self, expand: bool) -> Self {
        self.options.expand_streams = expand;
        self
    }
}
