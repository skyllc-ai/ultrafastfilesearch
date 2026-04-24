// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Error types for MFT operations.

use thiserror::Error;

/// Result type for MFT operations.
pub type Result<T> = core::result::Result<T, MftError>;

/// Errors that can occur during MFT operations.
#[derive(Error, Debug)]
pub enum MftError {
    /// Failed to open volume for reading.
    #[error("Failed to open volume '{volume}': {source}")]
    VolumeOpen {
        /// The volume letter that failed to open.
        volume: char,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Volume is not NTFS formatted.
    #[error("Volume '{0}' is not NTFS formatted")]
    NotNtfs(char),

    /// Insufficient privileges to read MFT.
    #[error("Insufficient privileges to read MFT. Run as Administrator.")]
    InsufficientPrivileges,

    /// Failed to read boot sector.
    #[error("Failed to read boot sector: {0}")]
    BootSectorRead(String),

    /// Invalid boot sector data.
    #[error("Invalid boot sector: {0}")]
    InvalidBootSector(String),

    /// Failed to read MFT record.
    #[error("Failed to read MFT record {frs}: {reason}")]
    RecordRead {
        /// The File Reference Segment number.
        frs: u64,
        /// The reason for the failure.
        reason: String,
    },

    /// Invalid MFT record (bad magic number).
    #[error("Invalid MFT record at FRS {0}: bad magic number")]
    InvalidRecord(u64),

    /// Attribute parsing error.
    #[error("Failed to parse attribute at offset {offset}: {reason}")]
    AttributeParse {
        /// The byte offset where parsing failed.
        offset: u64,
        /// The reason for the parse failure.
        reason: String,
    },

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

    /// I/O error during disk operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Polars error during `DataFrame` operations.
    #[error("DataFrame error: {0}")]
    Polars(#[from] uffs_polars::PolarsError),

    /// Parquet file error.
    #[error("Parquet error: {0}")]
    Parquet(String),

    /// Invalid data format.
    #[error("Invalid data: {0}")]
    InvalidData(String),

    /// Failed to get retrieval pointers (MFT extents).
    #[error("Failed to get MFT extents: {0}")]
    RetrievalPointers(String),

    /// Windows API error.
    #[cfg(windows)]
    #[error("Windows API error: {0}")]
    Windows(#[from] windows::core::Error),

    /// Feature not available on this platform.
    #[error("MFT reading is only available on Windows")]
    PlatformNotSupported,

    /// Invalid input provided.
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

impl MftError {
    /// Classifies a Tokio join error for a long-running MFT operation.
    #[cfg(any(windows, test))]
    #[must_use]
    pub(crate) fn from_join_error(operation: &'static str, error: &tokio::task::JoinError) -> Self {
        if error.is_cancelled() {
            return Self::Cancelled {
                operation,
                reason: error.to_string(),
            };
        }

        Self::WaitFailed {
            operation,
            reason: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MftError;

    #[tokio::test]
    async fn cancelled_join_errors_map_to_cancelled() {
        let handle = tokio::spawn(async {
            core::future::pending::<()>().await;
        });
        handle.abort();

        let outcome = handle.await;
        assert!(outcome.is_err(), "aborted task unexpectedly completed");
        let Err(join_error) = outcome else {
            return;
        };
        let error = MftError::from_join_error("read_all_index", &join_error);

        assert!(matches!(error, MftError::Cancelled {
            operation: "read_all_index",
            ..
        }));
    }

    #[test]
    fn wait_failed_variant_is_matchable() {
        let error = MftError::WaitFailed {
            operation: "read_all_index",
            reason: "task panicked".to_owned(),
        };

        assert!(matches!(error, MftError::WaitFailed {
            operation: "read_all_index",
            ..
        }));
    }
}
