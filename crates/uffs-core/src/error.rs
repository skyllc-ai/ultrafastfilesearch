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

    /// Long-running operation timed out.
    #[error("Operation '{operation}' timed out: {reason}")]
    Timeout {
        /// The operation that timed out.
        operation: &'static str,
        /// Additional context for the timeout.
        reason: String,
    },

    /// Long-running operation was cancelled.
    #[error("Operation '{operation}' was cancelled: {reason}")]
    Cancelled {
        /// The operation that was cancelled.
        operation: &'static str,
        /// Additional context for the cancellation.
        reason: String,
    },

    /// Waiting for a long-running operation failed.
    #[error("Waiting for operation '{operation}' failed: {reason}")]
    WaitFailed {
        /// The operation whose wait failed.
        operation: &'static str,
        /// Additional context for the wait failure.
        reason: String,
    },

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Polars error.
    #[error("DataFrame error: {0}")]
    Polars(#[from] uffs_polars::PolarsError),

    /// MFT error.
    #[error("MFT error: {0}")]
    Mft(uffs_mft::MftError),

    /// JSON serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<uffs_mft::MftError> for CoreError {
    fn from(error: uffs_mft::MftError) -> Self {
        match error {
            uffs_mft::MftError::Timeout { operation, reason } => {
                Self::Timeout { operation, reason }
            }
            uffs_mft::MftError::Cancelled { operation, reason } => {
                Self::Cancelled { operation, reason }
            }
            uffs_mft::MftError::WaitFailed { operation, reason } => {
                Self::WaitFailed { operation, reason }
            }
            other @ (uffs_mft::MftError::VolumeOpen { .. }
            | uffs_mft::MftError::NotNtfs(_)
            | uffs_mft::MftError::InsufficientPrivileges
            | uffs_mft::MftError::BootSectorRead(_)
            | uffs_mft::MftError::InvalidBootSector(_)
            | uffs_mft::MftError::RecordRead { .. }
            | uffs_mft::MftError::InvalidRecord(_)
            | uffs_mft::MftError::AttributeParse { .. }
            | uffs_mft::MftError::Io(_)
            | uffs_mft::MftError::Polars(_)
            | uffs_mft::MftError::Parquet(_)
            | uffs_mft::MftError::InvalidData(_)
            | uffs_mft::MftError::RetrievalPointers(_)
            | uffs_mft::MftError::PlatformNotSupported
            | uffs_mft::MftError::InvalidInput(_)) => Self::Mft(other),
            #[cfg(windows)]
            other @ uffs_mft::MftError::Windows(_) => Self::Mft(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CoreError;

    #[test]
    fn promotes_timeout_taxonomy_from_mft_errors() {
        let error = CoreError::from(uffs_mft::MftError::Timeout {
            operation: "load_index",
            reason: "deadline exceeded".to_owned(),
        });

        assert!(matches!(
            error,
            CoreError::Timeout {
                operation: "load_index",
                ..
            }
        ));
    }

    #[test]
    fn promotes_cancelled_taxonomy_from_mft_errors() {
        let error = CoreError::from(uffs_mft::MftError::Cancelled {
            operation: "load_index",
            reason: "shutdown requested".to_owned(),
        });

        assert!(matches!(
            error,
            CoreError::Cancelled {
                operation: "load_index",
                ..
            }
        ));
    }

    #[test]
    fn preserves_non_taxonomy_mft_errors() {
        let error = CoreError::from(uffs_mft::MftError::InvalidInput("bad drive".to_owned()));

        assert!(matches!(
            error,
            CoreError::Mft(uffs_mft::MftError::InvalidInput(message)) if message == "bad drive"
        ));
    }
}
