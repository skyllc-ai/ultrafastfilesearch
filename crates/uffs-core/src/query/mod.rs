//! MFT Query builder using Polars lazy API.
//!
//! This module provides a fluent API for querying MFT data.

mod filters;
mod matching;
mod operations;

use std::path::Path;

use uffs_polars::{DataFrame, IntoLazy, LazyFrame};

use crate::error::Result;

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
}

#[cfg(test)]
mod tests;
