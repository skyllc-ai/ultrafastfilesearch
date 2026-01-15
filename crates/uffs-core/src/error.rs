//! Error types for core query operations.

use thiserror::Error;

/// Result type for core operations.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Errors that can occur during query operations.
#[derive(Error, Debug)]
pub enum CoreError {
    /// Invalid glob pattern.
    #[error("Invalid glob pattern '{pattern}': {reason}")]
    InvalidGlob { pattern: String, reason: String },

    /// Invalid regex pattern.
    #[error("Invalid regex pattern '{pattern}': {reason}")]
    InvalidRegex { pattern: String, reason: String },

    /// Path resolution failed.
    #[error("Failed to resolve path for FRS {0}")]
    PathResolution(u64),

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

