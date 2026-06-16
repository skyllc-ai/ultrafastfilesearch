// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Lifecycle tracking for tools the suite acquires (downloads/installs).
//!
//! A tool the operator already had is a *resource* — never removed; its
//! mutations are undone via the [`crate::restore`] registry. A tool the suite
//! itself acquired is an [`Acquisition`] carrying a [`Disposition`]: at
//! teardown it is removed unless the operator chose to keep it. Pre-existing
//! resources are therefore never deleted by [`teardown`].

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{BenchError, CrumbError};
use crate::host::Host;

/// What to do with an acquired tool when the run finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Disposition {
    /// Leave the acquired tool in place after the run.
    Keep,
    /// Remove the acquired tool at teardown (the default for transient
    /// fetches).
    Remove,
}

/// A tool the suite acquired, with provenance and teardown disposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Acquisition {
    /// Human-readable tool name (for example `"es.exe"`).
    pub name: String,
    /// Where the acquired binary lives on disk.
    pub path: PathBuf,
    /// Provenance (URL, package id, ...).
    pub source: String,
    /// SHA-256 of the acquired bytes, hex-encoded.
    pub sha256: String,
    /// When it was acquired.
    pub acquired_at: DateTime<Utc>,
    /// Keep or remove at teardown.
    pub disposition: Disposition,
}

impl Acquisition {
    /// Record a freshly acquired tool, stamped with the host clock.
    #[must_use]
    pub fn new<N, P, S, H>(
        host: &dyn Host,
        name: N,
        path: P,
        source: S,
        sha256: H,
        disposition: Disposition,
    ) -> Self
    where
        N: Into<String>,
        P: Into<PathBuf>,
        S: Into<String>,
        H: Into<String>,
    {
        Self {
            name: name.into(),
            path: path.into(),
            source: source.into(),
            sha256: sha256.into(),
            acquired_at: host.now(),
            disposition,
        }
    }
}

/// Remove every acquired tool whose disposition is [`Disposition::Remove`].
///
/// [`Disposition::Keep`] acquisitions and any path that no longer exists are
/// left untouched. Pre-existing resources are not represented here, so they are
/// never removed. Returns one [`CrumbError`] per failed removal.
#[must_use]
pub fn teardown(host: &dyn Host, acquisitions: &[Acquisition]) -> Vec<CrumbError> {
    let mut crumbs = Vec::new();
    for acquisition in acquisitions {
        if acquisition.disposition != Disposition::Remove {
            continue;
        }
        let path: &Path = &acquisition.path;
        if !host.path_exists(path) {
            continue;
        }
        if let Err(source) = host.remove_file(path) {
            crumbs.push(CrumbError {
                label: format!("remove acquired tool '{}'", acquisition.name),
                source: BenchError::io(path, source),
            });
        }
    }
    crumbs
}
