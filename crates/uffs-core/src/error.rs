// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Error types for core query operations.

use thiserror::Error;

/// Result type for core operations.
pub type Result<T> = core::result::Result<T, CoreError>;

/// Errors that can occur during query operations.
///
/// `#[non_exhaustive]` is applied per Phase 5 §5c: future operation
/// taxonomies (e.g. a `RateLimited` variant for the daemon's
/// admission-control gate, or a `Degraded { reason }` variant once
/// partial-result handling lands) can be added as additive minor-
/// version bumps without breaking downstream exhaustive matchers.
/// Workspace-wide audit at PR-time confirmed all 23 `CoreError::*`
/// references live inside `uffs-core` itself — zero external
/// exhaustive matches today (refs #192).
#[derive(Error, Debug)]
#[non_exhaustive]
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
    /// Translates an [`uffs_mft::MftError`] into a [`CoreError`],
    /// promoting operation-lifecycle variants (`Timeout`, `Cancelled`,
    /// `WaitFailed`) into the matching [`CoreError`] taxonomy so
    /// callers can match on `CoreError::Timeout` / etc. uniformly
    /// regardless of which layer raised the error.
    ///
    /// All other [`MftError`] variants flow through the catchall as
    /// `Self::Mft(other)` — including any future variants added in
    /// `uffs-mft`, which is `#[non_exhaustive]` (refs #192).
    ///
    /// **Contributor contract:** when `uffs-mft` adds a new variant,
    /// audit whether it should be promoted to a [`CoreError`] taxonomy
    /// variant.  Operation-lifecycle additions MUST get an explicit arm
    /// here and a regression test below (see
    /// `promotes_*_taxonomy_from_mft_errors`).  Pass-through additions
    /// (raw Win32 / FS errors) need no source change here — the
    /// catchall handles them safely.
    ///
    /// [`MftError`]: uffs_mft::MftError
    ///
    /// # `clippy::wildcard_enum_match_arm` expectation
    ///
    /// `MftError` is `#[non_exhaustive]` from `uffs-mft` (Phase 5 §5c,
    /// refs #192): a catchall is structurally required — without one,
    /// the match would be incomplete; with one, the lint fires.  The
    /// two are fundamentally incompatible for cross-crate matches on
    /// `#[non_exhaustive]` enums.  The deliberate-decision property
    /// the lint normally guards is preserved by:
    ///
    /// 1. The three explicit taxonomy arms below, asserted by the
    ///    `promotes_*_taxonomy_from_mft_errors` regression tests at the bottom
    ///    of this file — adding a new lifecycle variant to `MftError` without
    ///    updating those tests surfaces as a CI failure.
    /// 2. The contributor contract in `MftError`'s rustdoc, which tells
    ///    contributors when a new variant must be promoted here vs. passed
    ///    through.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "MftError is #[non_exhaustive] from the cross-crate \
                  perspective of uffs-core; the wildcard catchall is \
                  structurally required for forward-compat.  \
                  Deliberate-decision property preserved by explicit \
                  arms + their regression tests + the contributor \
                  docstring on MftError (see fn-level rustdoc)."
    )]
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
            other => Self::Mft(other),
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

        assert!(matches!(error, CoreError::Timeout {
            operation: "load_index",
            ..
        }));
    }

    #[test]
    fn promotes_cancelled_taxonomy_from_mft_errors() {
        let error = CoreError::from(uffs_mft::MftError::Cancelled {
            operation: "load_index",
            reason: "shutdown requested".to_owned(),
        });

        assert!(matches!(error, CoreError::Cancelled {
            operation: "load_index",
            ..
        }));
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
