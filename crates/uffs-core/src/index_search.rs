//! Direct search on `MftIndex` without `DataFrame` conversion.
//!
//! This module keeps the optimized search execution pipeline together while
//! splitting pattern compilation, routing helpers, and tests into focused
//! submodules.
//!
//! This module provides SIMD-optimized pattern matching directly on `MftIndex`,
//! avoiding the overhead of converting to a Polars `DataFrame` for simple
//! queries.
//!
//! # Performance
//!
//! For simple queries (glob, extension filter, size filter):
//! - **`MftIndex` path**: ~100-200ms for 23M entries
//! - **`DataFrame` path**: ~3-5s (includes conversion overhead)
//!
//! # Example
//!
//! ```rust,ignore
//! use uffs_core::index_search::IndexQuery;
//! use uffs_mft::index::MftIndex;
//!
//! let index: MftIndex = /* load from cache */;
//!
//! // Fast path: search directly on MftIndex
//! let results = IndexQuery::new(&index)
//!     .glob("*.rs")
//!     .files_only()
//!     .min_size(1024)
//!     .limit(100)
//!     .collect();
//!
//! for result in results {
//!     println!("{}: {} bytes", result.path, result.size);
//! }
//! ```

/// Pattern compilation and matching for direct `MftIndex` search.
mod pattern;
/// Query routing helpers for hybrid search execution.
mod routing;
/// Tests for direct `MftIndex` search.
#[cfg(test)]
mod tests;

use rayon::prelude::*;
use regex::Regex;
use uffs_mft::index::{FileRecord, MftIndex};

pub use self::pattern::{
    IndexPattern, compile_extensions, compile_index_pattern, compile_parsed_pattern,
};
pub use self::routing::{QueryComplexity, QueryFeatures, QueryMode, analyze_pattern_complexity};
use crate::error::Result;

// ============================================================================
// SearchResult
// ============================================================================

/// Result of a search on `MftIndex`.
///
/// Each result represents a unique (record, name, stream) combination.
/// Files with hard links produce multiple results (different paths, same FRS).
/// Files with ADS produce multiple results (same path, different stream names).
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The file/directory name (includes `:stream_name` for ADS, legacy-output
    /// parity).
    pub name: String,
    /// The full path (if resolved), including `:stream_name` for ADS.
    pub path: Option<String>,
    /// File size in bytes (for this specific stream).
    pub size: u64,
    /// Allocated size on disk (0 for resident files, cluster-aligned for
    /// non-resident).
    pub allocated_size: u64,
    /// File Reference Segment number.
    pub frs: u64,
    /// Parent FRS (for this specific hard link).
    pub parent_frs: u64,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// Stream name (empty for default `$DATA` stream).
    pub stream_name: String,
    /// Which hard link (0 = primary name).
    pub name_index: u16,
    /// Which stream (0 = default `$DATA`).
    pub stream_index: u16,

    // Tree metrics (pre-computed in MftIndex)
    /// Count of all descendants (files + subdirectories) in subtree (0 for
    /// files).
    pub descendants: u32,
    /// Sum of logical file sizes in subtree (includes this file/directory).
    pub treesize: u64,
    /// Sum of allocated disk sizes in subtree (includes this file/directory).
    pub tree_allocated: u64,
}

impl SearchResult {
    /// Create a new search result from a file record (primary name, default
    /// stream).
    #[must_use]
    pub fn from_record(record: &FileRecord, index: &MftIndex) -> Self {
        let is_directory = record.is_directory();
        // legacy-output parity: directories have empty name, files have actual name
        let name = if is_directory {
            String::new()
        } else {
            index.record_name(record).to_owned()
        };

        Self {
            name,
            path: None, // Path resolution is expensive, done on demand
            size: record.first_stream.size.length,
            allocated_size: record.first_stream.size.allocated,
            frs: record.frs,
            parent_frs: record.first_name.parent_frs,
            is_directory,
            stream_name: String::new(),
            name_index: 0,
            stream_index: 0,
            descendants: record.descendants,
            treesize: record.treesize,
            tree_allocated: record.tree_allocated,
        }
    }

    /// Create a search result for a specific (name, stream) combination.
    #[must_use]
    pub fn from_expanded(
        record: &FileRecord,
        index: &MftIndex,
        name_idx: u16,
        stream_idx: u16,
    ) -> Self {
        let name_info = index
            .get_name_at(record, name_idx)
            .unwrap_or(&record.first_name);
        let stream_info = index
            .get_stream_at(record, stream_idx)
            .unwrap_or(&record.first_stream);
        let is_directory = record.is_directory();

        // Get base filename and stream name
        let stream_name = index.stream_name(stream_info);
        let has_ads = !stream_name.is_empty();

        // legacy-output parity: directories have empty Name for default stream,
        // but ADS entries get "dirname:streamname" format (same as files)
        let name = if is_directory && !has_ads {
            // Default directory stream: empty Name
            String::new()
        } else if has_ads {
            // ADS entry (file or directory): "filename:streamname"
            let base_name = index.get_name(&name_info.name).to_owned();
            format!("{base_name}:{stream_name}")
        } else {
            // Default file stream: just the filename
            index.get_name(&name_info.name).to_owned()
        };

        // legacy-output parity: Only the default stream (stream_idx == 0) gets tree
        // metrics. ADS streams (stream_idx > 0) have
        // descendants/treesize/tree_allocated = 0. In C++, each stream has its
        // own treesize field, and only the default stream accumulates
        // children's treesize (line 4794 in UltraFastFileSearch.cpp).
        let (descendants, treesize, tree_allocated) = if stream_idx == 0 {
            (record.descendants, record.treesize, record.tree_allocated)
        } else {
            (0, 0, 0)
        };

        Self {
            name,
            path: None,
            size: stream_info.size.length,
            allocated_size: stream_info.size.allocated,
            frs: record.frs,
            parent_frs: name_info.parent_frs,
            is_directory,
            stream_name: stream_name.to_owned(),
            name_index: name_idx,
            stream_index: stream_idx,
            descendants,
            treesize,
            tree_allocated,
        }
    }

    /// Create with resolved path.
    #[must_use]
    pub fn with_path(mut self, path: String) -> Self {
        self.path = Some(path);
        self
    }

    /// Check if this is an Alternate Data Stream (ADS).
    #[must_use]
    pub fn is_ads(&self) -> bool {
        !self.stream_name.is_empty()
    }

    /// Check if this is a hard link (not the primary name).
    #[must_use]
    pub const fn is_hard_link(&self) -> bool {
        self.name_index > 0
    }
}

// ============================================================================
// IndexQuery - Fluent Query Builder
// ============================================================================

/// Type filter for `IndexQuery`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TypeFilter {
    /// Match both files and directories.
    #[default]
    All,
    /// Match only files.
    FilesOnly,
    /// Match only directories.
    DirsOnly,
}

/// Query options for `IndexQuery`.
#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "configuration struct with independent boolean flags"
)]
pub struct QueryOptions {
    /// Type filter (files, dirs, or both).
    pub type_filter: TypeFilter,
    /// Whether to use case-sensitive matching.
    pub case_sensitive: bool,
    /// Whether to resolve full paths.
    pub resolve_paths: bool,
    /// Whether to expand hard links (multiple names per FRS).
    pub expand_names: bool,
    /// Whether to expand Alternate Data Streams (ADS).
    pub expand_streams: bool,
    /// Whether to include system metafiles (FRS < 16, except root FRS 5).
    /// Default is `false` to match C++ behavior.
    pub include_system_metafiles: bool,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            type_filter: TypeFilter::default(),
            case_sensitive: false,
            resolve_paths: false,
            expand_names: true,
            expand_streams: true,
            include_system_metafiles: false, // C++ default: exclude $MFT, $Bitmap, etc.
        }
    }
}

/// Fluent query builder for searching `MftIndex` directly.
///
/// Applies filters in optimal order: type → size → pattern (cheap to
/// expensive).
pub struct IndexQuery<'a> {
    /// Reference to the index being queried.
    index: &'a MftIndex,
    /// Optional pattern filter.
    pattern: Option<IndexPattern>,
    /// Query options (type filter, case sensitivity, path resolution).
    options: QueryOptions,
    /// Minimum file size filter.
    min_size: Option<u64>,
    /// Maximum file size filter.
    max_size: Option<u64>,
    /// Maximum number of results to return.
    limit: Option<usize>,
}

impl<'a> IndexQuery<'a> {
    /// Create a new query on the given index.
    #[must_use]
    pub const fn new(index: &'a MftIndex) -> Self {
        Self {
            index,
            pattern: None,
            options: QueryOptions {
                type_filter: TypeFilter::All,
                case_sensitive: false, // Windows default
                resolve_paths: false,
                expand_names: true,              // Match C++ behavior by default
                expand_streams: true,            // Match C++ behavior by default
                include_system_metafiles: false, // Match C++ behavior by default
            },
            min_size: None,
            max_size: None,
            limit: None,
        }
    }

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
            // Build case-insensitive version; if it fails, clone the original
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

    /// Build extension filter indices for simple extension queries.
    ///
    /// Returns `Some(Vec<u32>)` if the pattern is a simple suffix (extension)
    /// pattern and the index has an extension index. Returns `None`
    /// otherwise.
    ///
    /// Extracted to reduce line count of `collect` method.
    #[expect(
        clippy::single_call_fn,
        reason = "extracted from collect() to satisfy too_many_lines"
    )]
    fn build_extension_filter_indices(
        pattern: Option<&IndexPattern>,
        index: &MftIndex,
    ) -> Option<Vec<u32>> {
        let pat = pattern?;
        let ext_index = index.extension_index.as_ref()?;

        // Check if pattern is a simple suffix (extension) pattern
        let IndexPattern::Suffix { suffix, .. } = pat else {
            return None;
        };

        // Extract extension from suffix (e.g., ".txt" → "txt")
        let ext_str = suffix.strip_prefix('.')?;

        // Check if it's a simple extension (no additional dots)
        if ext_str.contains('.') {
            return None;
        }

        // Look up extension_id
        // Extensions are stored lowercase in ExtensionTable
        let ext_lower = ext_str.to_ascii_lowercase();
        let ext_id = index.extensions.map.get(ext_lower.as_str())?;

        // Get record indices from extension index
        let record_indices = ext_index.get_records(*ext_id);
        Some(record_indices.to_vec())
    }

    /// Resolve the full path for a search result.
    ///
    /// Handles both primary names and hard links, and appends ADS names if
    /// needed.
    ///
    /// Extracted to reduce line count of `collect` method.
    #[expect(
        clippy::single_call_fn,
        reason = "extracted from collect() to satisfy too_many_lines"
    )]
    fn resolve_result_path(
        result: SearchResult,
        record: &FileRecord,
        index: &MftIndex,
        name_idx: u16,
        stream_idx: u16,
        cached_path: Option<String>,
    ) -> SearchResult {
        // Get stream info, falling back to first_stream if not found
        // This handles cases where stream_count is higher than actual stored streams
        let stream = index
            .get_stream_at(record, stream_idx)
            .unwrap_or(&record.first_stream);

        // Use cached path for primary name (idx 0), build for hard links
        let mut base_path = if name_idx == 0 {
            cached_path.unwrap_or_else(|| index.build_path(record.frs))
        } else {
            index.build_path_for_name(record, name_idx)
        };

        // Append stream name for ADS
        let stream_name = index.stream_name(stream);
        let path = if stream_name.is_empty() {
            // Add trailing backslash for directories (legacy-output parity)
            if record.is_directory() && !base_path.ends_with('\\') {
                base_path.push('\\');
            }
            base_path
        } else {
            format!("{base_path}:{stream_name}")
        };

        result.with_path(path)
    }

    /// Execute the query and collect results.
    ///
    /// Uses Rayon for parallel execution across all records.
    /// Filters are applied in optimal order: type → size → pattern.
    /// When expansion is enabled, each (name × stream) combination produces a
    /// result.
    #[must_use]
    pub fn collect(self) -> Vec<SearchResult> {
        use uffs_mft::index::PathCache;

        let records = self.index.records();
        let case_sensitive = self.options.case_sensitive;
        let type_filter = self.options.type_filter;
        let resolve_paths = self.options.resolve_paths;
        let expand_names = self.options.expand_names;
        let expand_streams = self.options.expand_streams;
        let include_system_metafiles = self.options.include_system_metafiles;
        let pattern = &self.pattern;
        let min_size = self.min_size;
        let max_size = self.max_size;
        let limit = self.limit;
        let index = self.index;

        // Build path cache once - O(n) amortized with memoization
        // This pre-computes all paths and marks illegal records (system metafiles,
        // descendants of system metafiles, cycles) so we can filter with O(1) lookup.
        let path_cache = PathCache::build(index, include_system_metafiles);

        // Fast path: Use extension index for simple extension queries (*.ext)
        // This reduces O(n) scan to O(matches) lookup
        let extension_filter_indices =
            Self::build_extension_filter_indices(pattern.as_ref(), index);

        // Choose iteration strategy based on whether we have extension filter
        let records_to_scan: Vec<&FileRecord> = extension_filter_indices.as_ref().map_or_else(
            || records.iter().collect(),
            |indices| {
                indices
                    .iter()
                    .filter_map(|&idx| records.get(idx as usize))
                    .collect()
            },
        );

        // Parallel filter with early termination via take_any
        // Then expand (names × streams) for each matching record
        let filtered: Vec<SearchResult> = records_to_scan
            .par_iter()
            .filter(|record| {
                // 0. System metafile filter - O(1) cache lookup
                // PathCache has already computed validity for all records,
                // including descendants of system metafiles like $Extend/$RmMetadata
                if !path_cache.is_valid(record.frs) {
                    return false;
                }

                // 1. Type filter (cheapest - bit check)
                match type_filter {
                    TypeFilter::FilesOnly if record.is_directory() => return false,
                    TypeFilter::DirsOnly if !record.is_directory() => return false,
                    TypeFilter::All | TypeFilter::FilesOnly | TypeFilter::DirsOnly => {}
                }

                // 2. Size filter (cheap - u64 compare)
                // Note: We check the first stream's size here; ADS may have different sizes
                let size = record.first_stream.size.length;
                if let Some(min) = min_size {
                    if size < min {
                        return false;
                    }
                }
                if let Some(max) = max_size {
                    if size > max {
                        return false;
                    }
                }

                // 3. Pattern filter (expensive - string ops)
                // Note: We match against the primary name; hard links may have different names
                if let Some(pat) = pattern {
                    let name = index.record_name(record);
                    if !pat.matches(name, case_sensitive) {
                        return false;
                    }
                }

                true
            })
            .take_any(limit.unwrap_or(usize::MAX))
            .flat_map_iter(|record| {
                // Expand (names × streams) for each matching record
                // Fast path: most files have 1 name and 1 stream
                // Use max(1, count) to ensure at least one iteration (every file has at least
                // one name and one stream, even if count is 0 for placeholder records)
                let name_count = if expand_names {
                    record.name_count.max(1)
                } else {
                    1
                };
                let stream_count = if expand_streams {
                    record.stream_count.max(1)
                } else {
                    1
                };

                // Get cached path for primary name (idx 0) once, outside the inner loops
                let outer_cached_path = if resolve_paths {
                    path_cache.get(record.frs)
                } else {
                    None
                };

                (0..name_count).flat_map(move |name_idx| {
                    let inner_cached_path = outer_cached_path.clone();
                    (0..stream_count).filter_map(move |stream_idx| {
                        // Filter out non-$DATA streams (matches the legacy baseline
                        // match_attributes=false) Only $DATA
                        // (type_name_id=8) and $I30 (type_name_id=0) are output
                        let stream_info = index.get_stream_at(record, stream_idx)?;
                        if !stream_info.is_output_stream() {
                            return None;
                        }

                        let result =
                            SearchResult::from_expanded(record, index, name_idx, stream_idx);
                        if resolve_paths {
                            Some(Self::resolve_result_path(
                                result,
                                record,
                                index,
                                name_idx,
                                stream_idx,
                                inner_cached_path.clone(),
                            ))
                        } else {
                            Some(result)
                        }
                    })
                })
            })
            .collect();

        filtered
    }

    /// Count matching records without collecting results.
    ///
    /// More efficient than `collect().len()` when you only need the count.
    #[must_use]
    pub fn count(self) -> usize {
        let records = self.index.records();
        let case_sensitive = self.options.case_sensitive;
        let type_filter = self.options.type_filter;
        let pattern = &self.pattern;
        let min_size = self.min_size;
        let max_size = self.max_size;
        let index = self.index;

        records
            .par_iter()
            .filter(|record| {
                match type_filter {
                    TypeFilter::FilesOnly if record.is_directory() => return false,
                    TypeFilter::DirsOnly if !record.is_directory() => return false,
                    TypeFilter::All | TypeFilter::FilesOnly | TypeFilter::DirsOnly => {}
                }
                let size = record.first_stream.size.length;
                if let Some(min) = min_size {
                    if size < min {
                        return false;
                    }
                }
                if let Some(max) = max_size {
                    if size > max {
                        return false;
                    }
                }
                if let Some(pat) = pattern {
                    let name = index.record_name(record);
                    if !pat.matches(name, case_sensitive) {
                        return false;
                    }
                }
                true
            })
            .count()
    }
}
