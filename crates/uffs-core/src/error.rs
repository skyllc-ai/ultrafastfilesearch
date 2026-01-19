//! Error types for core query operations.

use thiserror::Error;

/// Result type for core operations.
pub type Result<T> = core::result::Result<T, CoreError>;

/// Errors that can occur during query operations.
#[derive(Error, Debug)]
pub enum CoreError {
    /// Invalid glob pattern.
    #[error("Invalid glob pattern '{pattern}': {reason}")]
    InvalidGlob {
        /// The invalid glob pattern.
        pattern: String,
        /// The reason the pattern is invalid.
        reason: String,
    },

    /// Invalid regex pattern.
    #[error("Invalid regex pattern '{pattern}': {reason}")]
    InvalidRegex {
        /// The invalid regex pattern.
        pattern: String,
        /// The reason the pattern is invalid.
        reason: String,
    },

    /// Invalid search pattern.
    #[error("Invalid pattern '{pattern}': {reason}")]
    InvalidPattern {
        /// The invalid pattern.
        pattern: String,
        /// The reason the pattern is invalid.
        reason: String,
    },

    /// Path resolution failed.
    #[error("Failed to resolve path for FRS {0}")]
    PathResolution(u64),

    /// Missing required column.
    #[error("Missing required column: {0}")]
    MissingColumn(String),

    /// Circular reference detected during path resolution.
    #[error("Circular reference detected at FRS {0}")]
    CircularReference(u64),

    /// Export error.
    #[error("Export failed: {0}")]
    Export(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Polars error.
    #[error("DataFrame error: {0}")]
    Polars(#[from] uffs_polars::PolarsError),

    /// MFT error.
    #[error("MFT error: {0}")]
    Mft(#[from] uffs_mft::MftError),

    /// JSON serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
