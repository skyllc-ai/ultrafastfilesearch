//! Direct search on `MftIndex` without `DataFrame` conversion.
//!
//! This module keeps the optimized search execution pipeline together while
//! splitting pattern compilation, routing helpers, result modeling, query
//! execution, and tests into focused submodules.
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
/// Query execution and builder types for direct `MftIndex` search.
mod query;
/// Search result modeling for direct `MftIndex` search.
mod result;
/// Query routing helpers for hybrid search execution.
mod routing;
/// Tests for direct `MftIndex` search.
#[cfg(test)]
mod tests;

pub use self::pattern::{
    IndexPattern, compile_extensions, compile_index_pattern, compile_parsed_pattern,
};
pub use self::query::{IndexQuery, QueryOptions, TypeFilter};
pub use self::result::SearchResult;
pub use self::routing::{QueryComplexity, QueryFeatures, QueryMode, analyze_pattern_complexity};
