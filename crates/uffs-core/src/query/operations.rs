//! Sorting, limiting, projection, and execution for `MftQuery`.

use uffs_polars::{DataFrame, Expr, LazyFrame, SortMultipleOptions, col};

use super::MftQuery;
use crate::error::{CoreError, Result};

impl MftQuery {
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
