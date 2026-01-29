//! Direct search on `MftIndex` without `DataFrame` conversion.
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

use std::collections::HashSet;

use aho_corasick::AhoCorasick;
// memchr is used by aho-corasick internally; we keep it for future SIMD optimizations
#[allow(unused_imports)]
use memchr as _;
use rayon::prelude::*;
use regex::Regex;
use uffs_mft::index::{FileRecord, MftIndex};

use crate::compiled_pattern::{GlobKind, classify_glob};
use crate::error::{CoreError, Result};
use crate::pattern::{ParsedPattern, PatternType};

// ============================================================================
// IndexPattern - Pattern IR for MftIndex
// ============================================================================

/// Compiled pattern for direct matching on `MftIndex`.
///
/// This mirrors `CompiledPattern` but generates match functions instead of
/// Polars expressions. Uses SIMD-optimized string matching where possible.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum IndexPattern {
    /// Always matches (e.g., `*`).
    Any,

    /// Exact string match.
    Exact {
        /// The exact value to match (case-sensitive).
        value: String,
        /// Lowercase version for case-insensitive matching.
        value_lower: String,
    },

    /// Prefix match (e.g., `foo*`).
    Prefix {
        /// The prefix to match (case-sensitive).
        prefix: String,
        /// Lowercase version for case-insensitive matching.
        prefix_lower: String,
    },

    /// Suffix match (e.g., `*bar`, `*.txt`).
    Suffix {
        /// The suffix to match (case-sensitive).
        suffix: String,
        /// Lowercase version for case-insensitive matching.
        suffix_lower: String,
    },

    /// Literal substring match (e.g., `*needle*`).
    Contains {
        /// The substring to search for (case-sensitive).
        needle: String,
        /// Lowercase version for case-insensitive matching.
        needle_lower: String,
    },

    /// Prefix AND suffix match (e.g., `foo*bar`).
    PrefixSuffix {
        /// The prefix to match (case-sensitive).
        prefix: String,
        /// The suffix to match (case-sensitive).
        suffix: String,
        /// Lowercase prefix for case-insensitive matching.
        prefix_lower: String,
        /// Lowercase suffix for case-insensitive matching.
        suffix_lower: String,
    },

    /// Multiple exact matches (hash set lookup).
    ExactSet {
        /// Set of exact values (case-sensitive).
        values: HashSet<String>,
        /// Lowercase set for case-insensitive matching.
        values_lower: HashSet<String>,
    },

    /// Multiple suffix matches (e.g., extensions).
    SuffixSet {
        /// List of suffixes (case-sensitive).
        suffixes: Vec<String>,
        /// Lowercase suffixes for case-insensitive matching.
        suffixes_lower: Vec<String>,
    },

    /// Multiple literal substrings (Aho-Corasick).
    ContainsAny {
        /// Aho-Corasick automaton for case-sensitive matching.
        automaton: AhoCorasick,
        /// Aho-Corasick automaton for case-insensitive matching.
        automaton_lower: AhoCorasick,
        /// Original patterns for debugging.
        patterns: Vec<String>,
    },

    /// Fallback to regex.
    Regex {
        /// Compiled regex for case-sensitive matching.
        regex: Regex,
        /// Compiled regex for case-insensitive matching.
        regex_lower: Regex,
    },
}

// ============================================================================
// Pattern Matching
// ============================================================================

impl IndexPattern {
    /// Check if a string matches this pattern.
    #[inline]
    #[must_use]
    pub fn matches(&self, input: &str, case_sensitive: bool) -> bool {
        match self {
            Self::Any => true,

            Self::Exact { value, value_lower } => {
                if case_sensitive {
                    input == value
                } else {
                    input.eq_ignore_ascii_case(value_lower)
                }
            }

            Self::Prefix {
                prefix,
                prefix_lower,
            } => {
                if case_sensitive {
                    input.starts_with(prefix.as_str())
                } else {
                    input
                        .to_ascii_lowercase()
                        .starts_with(prefix_lower.as_str())
                }
            }

            Self::Suffix {
                suffix,
                suffix_lower,
            } => {
                if case_sensitive {
                    input.ends_with(suffix.as_str())
                } else {
                    input.to_ascii_lowercase().ends_with(suffix_lower.as_str())
                }
            }

            Self::Contains {
                needle,
                needle_lower,
            } => {
                if case_sensitive {
                    input.contains(needle.as_str())
                } else {
                    input.to_ascii_lowercase().contains(needle_lower.as_str())
                }
            }

            Self::PrefixSuffix {
                prefix,
                suffix,
                prefix_lower,
                suffix_lower,
            } => {
                if case_sensitive {
                    input.starts_with(prefix.as_str()) && input.ends_with(suffix.as_str())
                } else {
                    let lower = input.to_ascii_lowercase();
                    lower.starts_with(prefix_lower.as_str())
                        && lower.ends_with(suffix_lower.as_str())
                }
            }

            Self::ExactSet {
                values,
                values_lower,
            } => {
                if case_sensitive {
                    values.contains(input)
                } else {
                    values_lower.contains(&input.to_ascii_lowercase())
                }
            }

            Self::SuffixSet {
                suffixes,
                suffixes_lower,
            } => {
                if case_sensitive {
                    suffixes.iter().any(|suf| input.ends_with(suf.as_str()))
                } else {
                    let lower = input.to_ascii_lowercase();
                    suffixes_lower
                        .iter()
                        .any(|suf| lower.ends_with(suf.as_str()))
                }
            }

            Self::ContainsAny {
                automaton,
                automaton_lower,
                ..
            } => {
                if case_sensitive {
                    automaton.is_match(input)
                } else {
                    automaton_lower.is_match(&input.to_ascii_lowercase())
                }
            }

            Self::Regex { regex, regex_lower } => {
                if case_sensitive {
                    regex.is_match(input)
                } else {
                    regex_lower.is_match(&input.to_ascii_lowercase())
                }
            }
        }
    }
}

// ============================================================================
// Pattern Compilation
// ============================================================================

/// Compile a glob pattern into an `IndexPattern`.
///
/// # Errors
///
/// Returns an error if the pattern is invalid (e.g., malformed glob or regex).
pub fn compile_index_pattern(pattern: &str) -> Result<IndexPattern> {
    let kind = classify_glob(pattern);
    match kind {
        GlobKind::Any => Ok(IndexPattern::Any),

        GlobKind::Exact(value) => {
            let value_lower = value.to_ascii_lowercase();
            Ok(IndexPattern::Exact { value, value_lower })
        }

        GlobKind::Prefix(prefix) => {
            let prefix_lower = prefix.to_ascii_lowercase();
            Ok(IndexPattern::Prefix {
                prefix,
                prefix_lower,
            })
        }

        GlobKind::Suffix(suffix) => {
            let suffix_lower = suffix.to_ascii_lowercase();
            Ok(IndexPattern::Suffix {
                suffix,
                suffix_lower,
            })
        }

        GlobKind::Extension(ext) => {
            let suffix = format!(".{ext}");
            let suffix_lower = suffix.to_ascii_lowercase();
            Ok(IndexPattern::Suffix {
                suffix,
                suffix_lower,
            })
        }

        GlobKind::Contains(needle) => {
            let needle_lower = needle.to_ascii_lowercase();
            Ok(IndexPattern::Contains {
                needle,
                needle_lower,
            })
        }

        GlobKind::PrefixSuffix { prefix, suffix } => {
            let prefix_lower = prefix.to_ascii_lowercase();
            let suffix_lower = suffix.to_ascii_lowercase();
            Ok(IndexPattern::PrefixSuffix {
                prefix,
                suffix,
                prefix_lower,
                suffix_lower,
            })
        }

        GlobKind::Complex(glob_pattern) => {
            let glob = globset::Glob::new(&glob_pattern).map_err(|err| CoreError::InvalidGlob {
                pattern: glob_pattern.clone(),
                reason: err.to_string(),
            })?;
            let regex_str = glob.regex();
            let regex = Regex::new(regex_str).map_err(|err| CoreError::InvalidRegex {
                pattern: regex_str.to_owned(),
                reason: err.to_string(),
            })?;
            let regex_lower =
                Regex::new(&format!("(?i){regex_str}")).map_err(|err| CoreError::InvalidRegex {
                    pattern: regex_str.to_owned(),
                    reason: err.to_string(),
                })?;
            Ok(IndexPattern::Regex { regex, regex_lower })
        }
    }
}

/// Compile a `ParsedPattern` into an `IndexPattern`.
///
/// # Errors
///
/// Returns an error if the pattern is invalid (e.g., malformed glob or regex).
pub fn compile_parsed_pattern(parsed: &ParsedPattern) -> Result<IndexPattern> {
    match parsed.pattern_type() {
        PatternType::Glob => compile_index_pattern(parsed.pattern()),
        PatternType::Regex => {
            let pattern_str = parsed.pattern();
            let regex = Regex::new(pattern_str).map_err(|err| CoreError::InvalidRegex {
                pattern: pattern_str.to_owned(),
                reason: err.to_string(),
            })?;
            let regex_lower = Regex::new(&format!("(?i){pattern_str}")).map_err(|err| {
                CoreError::InvalidRegex {
                    pattern: pattern_str.to_owned(),
                    reason: err.to_string(),
                }
            })?;
            Ok(IndexPattern::Regex { regex, regex_lower })
        }
        PatternType::Literal => {
            let value = parsed.pattern().to_owned();
            let value_lower = value.to_ascii_lowercase();
            Ok(IndexPattern::Exact { value, value_lower })
        }
    }
}

/// Compile multiple extension patterns into a `SuffixSet`.
#[must_use]
pub fn compile_extensions(extensions: &[&str]) -> IndexPattern {
    let suffixes: Vec<String> = extensions
        .iter()
        .map(|ext| {
            if ext.starts_with('.') {
                ext.to_string()
            } else {
                format!(".{ext}")
            }
        })
        .collect();
    let suffixes_lower: Vec<String> = suffixes
        .iter()
        .map(|suf| suf.to_ascii_lowercase())
        .collect();
    IndexPattern::SuffixSet {
        suffixes,
        suffixes_lower,
    }
}

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
    /// The file/directory name (includes `:stream_name` for ADS, C++ parity).
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
        // C++ parity: directories have empty name, files have actual name
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

        // Get base filename
        let base_name = if is_directory {
            String::new()
        } else {
            index.get_name(&name_info.name).to_owned()
        };

        // C++ parity: ADS entries include stream name in Name column
        // e.g., "readme.txt:Zone.Identifier" instead of just "readme.txt"
        let stream_name = index.stream_name(stream_info);
        let name = if !stream_name.is_empty() && !is_directory {
            format!("{base_name}:{stream_name}")
        } else {
            base_name
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
            descendants: record.descendants,
            treesize: record.treesize,
            tree_allocated: record.tree_allocated,
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
#[allow(clippy::struct_excessive_bools)] // Configuration struct with boolean flags
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
    #[allow(clippy::single_call_fn)] // Extracted to satisfy clippy::too_many_lines
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
    #[allow(clippy::single_call_fn)] // Extracted to satisfy clippy::too_many_lines
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
            // Add trailing backslash for directories (C++ parity)
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
                    (0..stream_count).map(move |stream_idx| {
                        let result =
                            SearchResult::from_expanded(record, index, name_idx, stream_idx);
                        if resolve_paths {
                            Self::resolve_result_path(
                                result,
                                record,
                                index,
                                name_idx,
                                stream_idx,
                                inner_cached_path.clone(),
                            )
                        } else {
                            result
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

// ============================================================================
// QueryMode - Execution Path Selection
// ============================================================================

/// Query execution mode for hybrid query engine.
///
/// Controls whether queries use the fast `MftIndex` path or the full-featured
/// `DataFrame` path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueryMode {
    /// Automatically choose the best path based on query complexity.
    ///
    /// Simple queries (glob, extension, size filters) use `IndexQuery`.
    /// Complex queries (SQL, aggregations, sorting) use `MftQuery`.
    #[default]
    Auto,

    /// Force use of `IndexQuery` (fast path).
    ///
    /// Best for simple searches where speed is critical.
    /// Some features may not be available (SQL, aggregations).
    ForceIndex,

    /// Force use of `MftQuery` (`DataFrame` path).
    ///
    /// Full feature set including SQL, aggregations, and sorting.
    /// Slower due to `DataFrame` conversion overhead.
    ForceDataFrame,
}

impl QueryMode {
    /// Parse from string (for CLI).
    #[must_use]
    pub fn from_str_opt(input: &str) -> Option<Self> {
        match input.to_ascii_lowercase().as_str() {
            "auto" | "hybrid" => Some(Self::Auto),
            "index" | "fast" => Some(Self::ForceIndex),
            "dataframe" | "df" | "polars" | "full" => Some(Self::ForceDataFrame),
            _ => None,
        }
    }
}

impl core::fmt::Display for QueryMode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Auto => write!(formatter, "auto"),
            Self::ForceIndex => write!(formatter, "index"),
            Self::ForceDataFrame => write!(formatter, "dataframe"),
        }
    }
}

// ============================================================================
// QueryComplexity - Analyze Query for Routing
// ============================================================================

/// Query complexity classification for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryComplexity {
    /// Simple query - can use `IndexQuery`.
    Simple,
    /// Complex query - requires `DataFrame`.
    Complex,
}

/// Analyze a pattern to determine query complexity.
#[must_use]
pub const fn analyze_pattern_complexity(pattern: &IndexPattern) -> QueryComplexity {
    // All IndexPattern variants are supported by IndexQuery
    match pattern {
        IndexPattern::Any
        | IndexPattern::Exact { .. }
        | IndexPattern::Prefix { .. }
        | IndexPattern::Suffix { .. }
        | IndexPattern::Contains { .. }
        | IndexPattern::PrefixSuffix { .. }
        | IndexPattern::ExactSet { .. }
        | IndexPattern::SuffixSet { .. }
        | IndexPattern::ContainsAny { .. }
        | IndexPattern::Regex { .. } => QueryComplexity::Simple,
    }
}

/// Features that require `DataFrame` path.
///
/// Uses bitflags pattern to avoid excessive bools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryFeatures(u8);

impl QueryFeatures {
    /// No special features.
    pub const NONE: Self = Self(0);
    /// SQL query requested.
    pub const SQL: Self = Self(1 << 0);
    /// Aggregation requested (count by extension, etc.).
    pub const AGGREGATION: Self = Self(1 << 1);
    /// Sorting requested (other than limit).
    pub const SORTING: Self = Self(1 << 2);
    /// Group by requested.
    pub const GROUP_BY: Self = Self(1 << 3);
    /// Join with another dataset.
    pub const JOIN: Self = Self(1 << 4);

    /// Create empty features.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Add a feature.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Check if a feature is set.
    #[must_use]
    pub const fn has(self, feature: Self) -> bool {
        (self.0 & feature.0) != 0
    }

    /// Check if any feature requires `DataFrame`.
    #[must_use]
    pub const fn requires_dataframe(self) -> bool {
        self.0 != 0
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_pattern_any() {
        let pattern = compile_index_pattern("*").unwrap();
        assert!(pattern.matches("anything", true));
        assert!(pattern.matches("", true));
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_pattern_exact() {
        let pattern = compile_index_pattern("foo.txt").unwrap();
        assert!(pattern.matches("foo.txt", true));
        assert!(!pattern.matches("FOO.TXT", true));
        assert!(pattern.matches("FOO.TXT", false)); // case-insensitive
        assert!(!pattern.matches("foo.txt.bak", true));
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_pattern_prefix() {
        let pattern = compile_index_pattern("foo*").unwrap();
        assert!(pattern.matches("foo", true));
        assert!(pattern.matches("foobar", true));
        assert!(!pattern.matches("barfoo", true));
        assert!(pattern.matches("FOOBAR", false)); // case-insensitive
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_pattern_suffix() {
        let pattern = compile_index_pattern("*.txt").unwrap();
        assert!(pattern.matches("foo.txt", true));
        assert!(pattern.matches(".txt", true));
        assert!(!pattern.matches("foo.txt.bak", true));
        assert!(pattern.matches("FOO.TXT", false)); // case-insensitive
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_pattern_contains() {
        let pattern = compile_index_pattern("*needle*").unwrap();
        assert!(pattern.matches("needle", true));
        assert!(pattern.matches("haystackneedlehaystack", true));
        assert!(!pattern.matches("haystack", true));
        assert!(pattern.matches("NEEDLE", false)); // case-insensitive
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_pattern_prefix_suffix() {
        let pattern = compile_index_pattern("foo*bar").unwrap();
        assert!(pattern.matches("foobar", true));
        assert!(pattern.matches("foo123bar", true));
        assert!(!pattern.matches("foobarbaz", true));
        assert!(!pattern.matches("bazfoobar", true));
    }

    #[test]
    fn test_extensions() {
        let pattern = compile_extensions(&["rs", "toml"]);
        assert!(pattern.matches("main.rs", true));
        assert!(pattern.matches("Cargo.toml", true));
        assert!(!pattern.matches("main.py", true));
        assert!(pattern.matches("MAIN.RS", false)); // case-insensitive
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_extension_index_integration() {
        use uffs_mft::index::{IndexNameRef, MftIndex, ROOT_FRS, SizeInfo};

        // Create index with various extensions
        let mut index = MftIndex::new('C');

        // Create root directory (FRS 5) so path validation works
        let root_name_offset = index.add_name(".");
        let root = index.get_or_create(ROOT_FRS);
        root.stdinfo.set_directory(true);
        root.first_name.name = IndexNameRef::new(root_name_offset, 1, true, 0);
        root.first_name.parent_frs = ROOT_FRS; // Root points to itself

        // Add files with different extensions
        let files = [
            ("readme.txt", 1000),
            ("notes.txt", 2000),
            ("data.csv", 3000),
            ("script.py", 4000),
            ("config.json", 5000),
            ("test.txt", 6000),
        ];

        for (i, (name, size)) in files.iter().enumerate() {
            let frs = (i + 100) as u64; // Start at FRS 100 to avoid system metafiles
            let offset = index.add_name(name);
            let ext_id = index.intern_extension(name);

            let rec = index.get_or_create(frs);
            rec.first_name.name =
                IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
            rec.first_name.parent_frs = ROOT_FRS; // All files are in root
            rec.first_stream.size = SizeInfo {
                length: *size,
                allocated: *size,
            };

            // Record the file size in the extension table
            index.extensions.record_file(ext_id, *size);
        }

        // Build extension index
        index.build_extension_index();

        // Query for *.txt files
        let pattern = compile_index_pattern("*.txt").unwrap();
        let results: Vec<_> = IndexQuery::new(&index).with_pattern(pattern).collect();

        // Should find exactly 3 .txt files
        assert_eq!(results.len(), 3, "Should find 3 .txt files");

        // Verify the results
        let names: Vec<String> = results.iter().map(|rec| rec.name.clone()).collect();
        assert!(names.contains(&"readme.txt".to_owned()));
        assert!(names.contains(&"notes.txt".to_owned()));
        assert!(names.contains(&"test.txt".to_owned()));

        // Verify sizes
        let total_size: u64 = results.iter().map(|rec| rec.size).sum();
        assert_eq!(total_size, 1000 + 2000 + 6000);
    }

    #[test]
    fn test_query_mode_from_str() {
        assert_eq!(QueryMode::from_str_opt("auto"), Some(QueryMode::Auto));
        assert_eq!(QueryMode::from_str_opt("hybrid"), Some(QueryMode::Auto));
        assert_eq!(
            QueryMode::from_str_opt("index"),
            Some(QueryMode::ForceIndex)
        );
        assert_eq!(QueryMode::from_str_opt("fast"), Some(QueryMode::ForceIndex));
        assert_eq!(
            QueryMode::from_str_opt("dataframe"),
            Some(QueryMode::ForceDataFrame)
        );
        assert_eq!(
            QueryMode::from_str_opt("df"),
            Some(QueryMode::ForceDataFrame)
        );
        assert_eq!(
            QueryMode::from_str_opt("polars"),
            Some(QueryMode::ForceDataFrame)
        );
        assert_eq!(QueryMode::from_str_opt("invalid"), None);
    }

    #[test]
    fn test_query_mode_display() {
        assert_eq!(QueryMode::Auto.to_string(), "auto");
        assert_eq!(QueryMode::ForceIndex.to_string(), "index");
        assert_eq!(QueryMode::ForceDataFrame.to_string(), "dataframe");
    }

    #[test]
    fn test_query_features_requires_dataframe() {
        let empty = QueryFeatures::empty();
        assert!(!empty.requires_dataframe());

        let with_sql = QueryFeatures::empty().with(QueryFeatures::SQL);
        assert!(with_sql.requires_dataframe());
        assert!(with_sql.has(QueryFeatures::SQL));
        assert!(!with_sql.has(QueryFeatures::AGGREGATION));

        let with_agg = QueryFeatures::empty().with(QueryFeatures::AGGREGATION);
        assert!(with_agg.requires_dataframe());

        let combined = QueryFeatures::empty()
            .with(QueryFeatures::SQL)
            .with(QueryFeatures::SORTING);
        assert!(combined.requires_dataframe());
        assert!(combined.has(QueryFeatures::SQL));
        assert!(combined.has(QueryFeatures::SORTING));
        assert!(!combined.has(QueryFeatures::JOIN));
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code - unwrap is acceptable
    fn test_analyze_pattern_complexity() {
        let any = compile_index_pattern("*").unwrap();
        assert_eq!(analyze_pattern_complexity(&any), QueryComplexity::Simple);

        let suffix = compile_index_pattern("*.rs").unwrap();
        assert_eq!(analyze_pattern_complexity(&suffix), QueryComplexity::Simple);

        let regex = IndexPattern::Regex {
            regex: Regex::new(".*").unwrap(),
            regex_lower: Regex::new("(?i).*").unwrap(),
        };
        assert_eq!(analyze_pattern_complexity(&regex), QueryComplexity::Simple);
    }
}
