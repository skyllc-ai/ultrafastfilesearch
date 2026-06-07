// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Crate-level error types for the benchmark orchestrator.
//!
//! Two distinct error shapes coexist:
//!
//! - [`BenchError`] — the fallible-operation error returned by orchestrator
//!   logic (state I/O, (de)serialization, interactive-confirmation guards).
//! - [`CrumbError`] — a single *restore* failure captured during teardown.
//!   Restore failures are **collected**, never propagated, so one failing undo
//!   never prevents the remaining undos from running ("no crumb left behind").

use std::path::PathBuf;

use thiserror::Error;

/// Convenient `Result` alias used throughout `uffs-bench`.
pub type Result<T> = core::result::Result<T, BenchError>;

/// Top-level error type for orchestrator logic.
#[derive(Debug, Error)]
pub enum BenchError {
    /// An I/O operation failed, annotated with the path it concerned.
    #[error("I/O error at {path}: {source}")]
    Io {
        /// Path the failing operation targeted.
        path: PathBuf,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// (De)serializing the state file (or another JSON document) failed.
    #[error("JSON (de)serialization failed: {0}")]
    Serde(#[from] serde_json::Error),

    /// A non-interactive host was asked to make an interactive decision.
    ///
    /// Raised when a guided/interactive gate runs without a TTY so the
    /// orchestrator fails closed instead of blocking on absent input.
    #[error("interactive confirmation required but host has no TTY")]
    NoTty,

    /// A spawned command could not be run or returned a non-zero exit code.
    ///
    /// Used by the measurement stages and their restore actions, where the
    /// failure is a process outcome rather than an [`std::io::Error`] tied to a
    /// single filesystem path.
    #[error("command failed: {0}")]
    Command(String),

    /// Competitor provisioning failed (malformed manifest or a SHA-256
    /// mismatch on a downloaded binary).
    ///
    /// The fetch path treats a hash mismatch as fatal and *fails closed*
    /// (deletes the suspect download), so the pinned competitor is never run
    /// from unverified bytes.
    #[error("competitor provisioning failed: {0}")]
    Provision(String),

    /// One or more restore actions or fingerprint differences were detected.
    ///
    /// Used by the `restore` and `verify` subcommands to fail closed (non-zero
    /// exit) when the host is not returned to its as-found state, while still
    /// allowing callers to report every crumb before returning.
    #[error("{0} host difference(s) or restore failure(s) detected")]
    Crumbs(usize),

    /// One or more required tools could not be invoked.
    ///
    /// Raised during Stage 0a when a version probe returns `"unknown"` for a
    /// tool the operator requested, so the run aborts loudly before any
    /// measurements start rather than silently producing incomplete results.
    #[error("required tool(s) not found: {0}")]
    MissingTools(String),
}

impl BenchError {
    /// Build an [`BenchError::Io`] from a path and the underlying OS error.
    ///
    /// Small constructor so call sites read as
    /// `.map_err(|e| BenchError::io(path, e))` instead of repeating the
    /// struct-literal boilerplate.
    #[must_use]
    pub fn io<P: Into<PathBuf>>(path: P, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// A single restore failure ("crumb") captured during teardown.
///
/// Returned in a `Vec` by [`crate::restore::RestoreRegistry::drain`]; an empty
/// vector means every registered undo succeeded.
#[derive(Debug, Error)]
#[error("restore '{label}' failed: {source}")]
pub struct CrumbError {
    /// Human-readable label of the restore action that failed.
    pub label: String,
    /// Underlying error returned by the undo closure.
    pub source: BenchError,
}
