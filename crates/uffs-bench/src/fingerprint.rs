// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Host fingerprinting: capture a snapshot of the volatile host state before a
//! run and diff it afterwards to prove "no crumb left behind".
//!
//! [`capture`] hashes the UFFS ini and cache files, records the daemon state
//! and a chosen set of environment variables. [`diff`] returns a human-readable
//! list of every difference — an **empty** vector means the host was fully
//! restored. `state.json` and any *kept* tooling are deliberately excluded
//! (they are logged decisions, not crumbs), so they are simply never part of
//! the spec.
//!
//! The `cache`/`env` maps use [`BTreeMap`] rather than the design sketch's
//! `Vec<(String, String)>` so both serialization and the diff are
//! deterministic.

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::host::Host;

/// Describes which volatile host state to fingerprint.
#[derive(Debug, Clone, Default)]
pub struct FingerprintSpec {
    /// Path to the UFFS ini whose content hash anchors the fingerprint.
    pub ini_path: PathBuf,
    /// Cache files whose content hashes are tracked.
    pub cache_files: Vec<PathBuf>,
    /// Environment variable names to capture.
    pub env_keys: Vec<String>,
    /// Optional `(exe, args)` command whose stdout reports the daemon state.
    pub daemon_status_cmd: Option<(String, Vec<String>)>,
}

/// A captured snapshot of volatile host state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFingerprint {
    /// Content hash of the ini file (`"<absent>"` if missing).
    pub ini_sha: String,
    /// Content hash per cache file, keyed by display path.
    pub cache: BTreeMap<String, String>,
    /// Reported daemon state (`"n/a"` when no command was configured).
    pub daemon_state: String,
    /// Captured environment variables.
    pub env: BTreeMap<String, String>,
}

/// Hash a file's contents, or report `"<absent>"` if it cannot be read.
fn sha_of_file(host: &dyn Host, path: &Path) -> String {
    host.read_file(path).map_or_else(
        |_| "<absent>".to_owned(),
        |bytes| {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex::encode(hasher.finalize())
        },
    )
}

/// Capture a [`HostFingerprint`] for the given [`FingerprintSpec`].
#[must_use]
pub fn capture(host: &dyn Host, spec: &FingerprintSpec) -> HostFingerprint {
    let ini_sha = sha_of_file(host, &spec.ini_path);

    let cache = spec
        .cache_files
        .iter()
        .map(|path| (path.display().to_string(), sha_of_file(host, path)))
        .collect();

    let daemon_state = match &spec.daemon_status_cmd {
        Some((exe, args)) => {
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            host.run(exe, &arg_refs)
                .map_or_else(|_| "unknown".to_owned(), |out| out.stdout.trim().to_owned())
        }
        None => "n/a".to_owned(),
    };

    let env = spec
        .env_keys
        .iter()
        .filter_map(|key| host.env(key).map(|value| (key.clone(), value)))
        .collect();

    HostFingerprint {
        ini_sha,
        cache,
        daemon_state,
        env,
    }
}

/// Diff two keyed string maps, appending human-readable changes to `out`.
fn diff_maps(
    kind: &str,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    out: &mut Vec<String>,
) {
    for (key, before_value) in before {
        match after.get(key) {
            None => out.push(format!("{kind} '{key}' removed")),
            Some(after_value) if after_value != before_value => {
                out.push(format!(
                    "{kind} '{key}' changed: {before_value} -> {after_value}"
                ));
            }
            Some(_) => {}
        }
    }
    for key in after.keys() {
        if !before.contains_key(key) {
            out.push(format!("{kind} '{key}' added"));
        }
    }
}

/// Diff two fingerprints; an empty result means the host is clean.
#[must_use]
pub fn diff(before: &HostFingerprint, after: &HostFingerprint) -> Vec<String> {
    let mut diffs = Vec::new();
    if before.ini_sha != after.ini_sha {
        diffs.push(format!(
            "ini changed: {} -> {}",
            before.ini_sha, after.ini_sha
        ));
    }
    if before.daemon_state != after.daemon_state {
        diffs.push(format!(
            "daemon state changed: {} -> {}",
            before.daemon_state, after.daemon_state
        ));
    }
    diff_maps("cache", &before.cache, &after.cache, &mut diffs);
    diff_maps("env", &before.env, &after.env, &mut diffs);
    diffs
}
