// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! The `state.json` model and resume engine.
//!
//! Each step records its [`Status`] and an `input_hash` derived from the
//! decisions it depends on. On a re-run, a `Done` step whose `input_hash` still
//! matches is skipped ("cached"); changing a decision changes the hash and the
//! step re-runs automatically. The state file is written atomically (temp file
//! + rename) so a crash never leaves a half-written `state.json`.

use alloc::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::error::{BenchError, Result};
use crate::host::Host;
use crate::tooling::Acquisition;

/// Stable identifier for a step, for example `"stage2/cold-purge"`.
pub type StepId = String;

/// Lifecycle status of a single step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Not yet run.
    Pending,
    /// Completed successfully.
    Done,
    /// Deliberately skipped by the operator.
    Skipped,
    /// Attempted but failed.
    Failed,
}

/// Per-step record persisted in `state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    /// Current lifecycle status.
    pub status: Status,
    /// Hash of the decisions this step depended on when it ran.
    pub input_hash: String,
    /// Artifact paths the step produced.
    pub outputs: Vec<String>,
    /// When the step started.
    pub started_at: DateTime<Utc>,
    /// When the step finished, if it did.
    pub finished_at: Option<DateTime<Utc>>,
}

/// The operator decisions that scope a run (and feed step `input_hash`es).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Decisions {
    /// Selected mode name (`"guided"`, `"interactive"`, ...).
    pub mode: String,
    /// Drives under test.
    pub drives: Vec<String>,
    /// Competitor/tool ids participating in the run.
    pub tools: Vec<String>,
    /// Number of measurement rounds.
    pub rounds: u32,
    /// Whether caches are dropped before cold measurements.
    pub drop_cache: bool,
}

/// The full persisted benchmark-run state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    /// Suite version that wrote this state.
    pub suite_version: String,
    /// When the run started.
    pub started_at: DateTime<Utc>,
    /// When the state was last written.
    pub updated_at: DateTime<Utc>,
    /// Operator decisions scoping the run.
    pub decisions: Decisions,
    /// Tools acquired by the suite and their keep/remove disposition.
    pub acquisitions: Vec<Acquisition>,
    /// Per-step records, keyed by [`StepId`] (sorted for deterministic output).
    pub steps: BTreeMap<StepId, StepRecord>,
}

/// Compute a stable `input_hash` from the decision strings a step depends on.
///
/// Parts are domain-separated with a NUL byte so `["a", "bc"]` and `["ab",
/// "c"]` hash differently.
#[must_use]
pub fn input_hash(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0_u8]);
    }
    hex::encode(hasher.finalize())
}

/// Derive the sibling temp path used for atomic writes (`<path>.tmp`).
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".tmp");
    PathBuf::from(name)
}

impl State {
    /// Start a fresh state for `suite_version` with the given `decisions`.
    #[must_use]
    pub fn new<V: Into<String>>(host: &dyn Host, suite_version: V, decisions: Decisions) -> Self {
        let now = host.now();
        Self {
            suite_version: suite_version.into(),
            started_at: now,
            updated_at: now,
            decisions,
            acquisitions: Vec::new(),
            steps: BTreeMap::new(),
        }
    }

    /// Load a state file from disk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or is not valid state JSON.
    pub fn load(host: &dyn Host, path: &Path) -> Result<Self> {
        let bytes = host
            .read_file(path)
            .map_err(|err| BenchError::io(path, err))?;
        let state = serde_json::from_slice(&bytes)?;
        Ok(state)
    }

    /// Atomically persist the state to `path` (temp file + rename), stamping
    /// `updated_at` from the host clock first.
    ///
    /// # Errors
    /// Returns an error if serialization, the temp write, or the rename fails.
    pub fn save(&mut self, host: &dyn Host, path: &Path) -> Result<()> {
        self.updated_at = host.now();
        let json = serde_json::to_vec_pretty(self)?;
        let tmp = tmp_path(path);
        host.write_file(&tmp, &json)
            .map_err(|err| BenchError::io(&tmp, err))?;
        host.rename(&tmp, path)
            .map_err(|err| BenchError::io(path, err))?;
        Ok(())
    }

    /// Record the outcome of a step.
    pub fn set_step<I: Into<StepId>, H: Into<String>>(
        &mut self,
        host: &dyn Host,
        id: I,
        status: Status,
        input_hash: H,
        outputs: Vec<String>,
    ) {
        let now = host.now();
        let finished_at = if status == Status::Pending {
            None
        } else {
            Some(now)
        };
        self.steps.insert(id.into(), StepRecord {
            status,
            input_hash: input_hash.into(),
            outputs,
            started_at: now,
            finished_at,
        });
    }

    /// Whether a step is already `Done` with a matching `input_hash` and can be
    /// skipped on resume.
    #[must_use]
    pub fn should_skip(&self, id: &str, input_hash: &str) -> bool {
        self.steps
            .get(id)
            .is_some_and(|record| record.status == Status::Done && record.input_hash == input_hash)
    }

    /// Invalidate a single step (so it re-runs on the next pass).
    pub fn invalidate(&mut self, id: &str) {
        self.steps.remove(id);
    }

    /// Invalidate every step (the `--force` path).
    pub fn invalidate_all(&mut self) {
        self.steps.clear();
    }
}
