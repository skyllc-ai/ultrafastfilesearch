//! Query execution and builder types for direct `MftIndex` search.

mod builder;
mod execution;
mod expansion;
mod filtering;
mod planning;

use uffs_mft::index::MftIndex;

use super::pattern::IndexPattern;

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
}
