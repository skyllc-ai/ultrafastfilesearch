// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage 5 — teardown, host verification, and crash recovery.
//!
//! The "no crumb left behind" policy closes here. [`baseline`] captures a
//! pre-run [`HostFingerprint`] (`fingerprint-before.json`); [`finalize`] drains
//! the live restore stack, captures `fingerprint-after.json`, diffs the two,
//! resolves acquired-tool dispositions, and resets the now-spent
//! `restore-manifest.json` before finalizing `state.json`.
//!
//! Two operator subcommands build on the same machinery: [`restore`] replays
//! the persisted manifest after a hard kill, and [`verify`] re-runs the
//! fingerprint diff against a bundle — both fail closed (non-zero exit) when
//! the host is dirty, via [`BenchError::Crumbs`].

use std::path::{Path, PathBuf};

use crate::cli::Cli;
use crate::error::{BenchError, CrumbError, Result};
use crate::fingerprint::{self, FingerprintSpec, HostFingerprint};
use crate::host::Host;
use crate::restore::{RestoreManifest, RunGuard};
use crate::run::everything_ini_path;
use crate::state::State;
use crate::{stages, tooling};

/// Bundle-relative name of the serialized crash-recovery restore manifest.
pub const RESTORE_MANIFEST: &str = "restore-manifest.json";
/// Bundle-relative name of the pre-run host fingerprint.
const FINGERPRINT_BEFORE: &str = "fingerprint-before.json";
/// Bundle-relative name of the post-run host fingerprint.
const FINGERPRINT_AFTER: &str = "fingerprint-after.json";

/// The `(exe, args)` command whose stdout reports the UFFS daemon state.
fn daemon_status_cmd() -> (String, Vec<String>) {
    ("uffs".to_owned(), vec![
        "daemon".to_owned(),
        "status".to_owned(),
    ])
}

/// Build the [`FingerprintSpec`] describing the volatile host state a run may
/// touch: the read-only `Everything.ini`, the per-drive UFFS cache files, the
/// daemon run-state, and the path-defining environment variables.
fn fingerprint_spec(host: &dyn Host, cli: &Cli) -> FingerprintSpec {
    FingerprintSpec {
        ini_path: everything_ini_path(host),
        cache_files: stages::cache_files(host, &cli.drives_or_default()),
        env_keys: vec!["APPDATA".to_owned(), "LOCALAPPDATA".to_owned()],
        daemon_status_cmd: Some(daemon_status_cmd()),
    }
}

/// Atomically write a fingerprint into the bundle as pretty JSON.
fn write_fingerprint(
    host: &dyn Host,
    bundle_dir: &Path,
    name: &str,
    fingerprint: &HostFingerprint,
) -> Result<()> {
    let path = bundle_dir.join(name);
    let json = serde_json::to_vec_pretty(fingerprint)?;
    host.write_file(&path, &json)
        .map_err(|err| BenchError::io(&path, err))
}

/// Load a previously written fingerprint from the bundle.
fn load_fingerprint(host: &dyn Host, bundle_dir: &Path, name: &str) -> Result<HostFingerprint> {
    let path = bundle_dir.join(name);
    let bytes = host
        .read_file(&path)
        .map_err(|err| BenchError::io(&path, err))?;
    let fingerprint = serde_json::from_slice(&bytes)?;
    Ok(fingerprint)
}

/// Report any restore failures collected during a drain/replay ("crumbs").
fn report_crumbs(host: &dyn Host, context: &str, crumbs: &[CrumbError]) {
    if crumbs.is_empty() {
        return;
    }
    host.out(&format!("WARNING: {context} left crumbs behind:"));
    for crumb in crumbs {
        host.out(&format!("  - {crumb}"));
    }
}

/// Report the fingerprint diff: a friendly all-clear, or each difference.
fn report_diff(host: &dyn Host, diffs: &[String]) {
    if diffs.is_empty() {
        host.out("teardown: host clean (fingerprint-after == fingerprint-before)");
        return;
    }
    host.out(&format!(
        "WARNING: {} host difference(s) remain after restore:",
        diffs.len()
    ));
    for line in diffs {
        host.out(&format!("  - {line}"));
    }
}

/// Require the operator-supplied `--bundle <dir>` for a subcommand.
fn require_bundle(cli: &Cli, action: &str) -> Result<PathBuf> {
    cli.bundle
        .clone()
        .ok_or_else(|| BenchError::Command(format!("{action} requires --bundle <dir>")))
}

/// Capture the pre-run host fingerprint into `fingerprint-before.json`.
///
/// # Errors
/// Returns an error if the fingerprint cannot be written into the bundle.
pub fn baseline(host: &dyn Host, cli: &Cli, bundle_dir: &Path) -> Result<()> {
    let before = fingerprint::capture(host, &fingerprint_spec(host, cli));
    write_fingerprint(host, bundle_dir, FINGERPRINT_BEFORE, &before)
}

/// Close the run: drain the live restore stack, resolve acquired-tool
/// dispositions, diff the post-run fingerprint against the baseline, reset the
/// now-spent restore manifest, and finalize `state.json`.
///
/// Restore failures ("crumbs") and any residual fingerprint diff are reported
/// loudly but do **not** fail the run — the benchmark artifacts are still
/// valid, and the dedicated [`verify`] subcommand is the fail-closed gate. The
/// host is always left as found on a best-effort basis.
///
/// # Errors
/// Returns an error only if a teardown artifact (fingerprint-after, the
/// manifest reset, or `state.json`) cannot be written into the bundle.
pub fn finalize(
    host: &dyn Host,
    cli: &Cli,
    bundle_dir: &Path,
    guard: RunGuard<'_>,
    state: &mut State,
    state_path: &Path,
) -> Result<()> {
    // 1. Replay every armed undo in LIFO order, reporting any that failed.
    report_crumbs(host, "teardown restore", &guard.finish());
    // 2. Remove acquired tools whose disposition is Remove (keep the rest).
    report_crumbs(
        host,
        "tool teardown",
        &tooling::teardown(host, &state.acquisitions),
    );
    // 3. Capture the post-run fingerprint and diff it against the baseline.
    let after = fingerprint::capture(host, &fingerprint_spec(host, cli));
    write_fingerprint(host, bundle_dir, FINGERPRINT_AFTER, &after)?;
    let before = load_fingerprint(host, bundle_dir, FINGERPRINT_BEFORE)?;
    report_diff(host, &fingerprint::diff(&before, &after));
    // 4. The host is restored, so the manifest no longer describes pending work.
    RestoreManifest::new().save(host, &bundle_dir.join(RESTORE_MANIFEST))?;
    // 5. Finalize the resume/audit record.
    state.save(host, state_path)
}

/// Handle the `restore` subcommand: replay a bundle's persisted manifest to
/// return the host to its as-found state after a hard kill.
///
/// Fails closed: any undo that cannot be replayed is reported and the process
/// exits non-zero via [`BenchError::Crumbs`]. On success the manifest is reset.
///
/// # Errors
/// Returns an error if `--bundle` is absent, the manifest cannot be read, the
/// reset cannot be written, or any undo fails ([`BenchError::Crumbs`]).
pub fn restore(host: &dyn Host, cli: &Cli) -> Result<()> {
    let bundle_dir = require_bundle(cli, "restore")?;
    let manifest_path = bundle_dir.join(RESTORE_MANIFEST);
    let manifest = RestoreManifest::load(host, &manifest_path)?;
    let crumbs = manifest.replay(host);
    report_crumbs(host, "manifest replay", &crumbs);
    RestoreManifest::new().save(host, &manifest_path)?;
    if crumbs.is_empty() {
        host.out("restore: host returned to its as-found state");
        Ok(())
    } else {
        Err(BenchError::Crumbs(crumbs.len()))
    }
}

/// Handle the `verify` subcommand: re-capture the host fingerprint and diff it
/// against a bundle's `fingerprint-before.json` baseline.
///
/// Fails closed: any difference is reported and the process exits non-zero via
/// [`BenchError::Crumbs`]. The fresh capture is written as
/// `fingerprint-after.json` for forensics.
///
/// # Errors
/// Returns an error if `--bundle` is absent, the baseline cannot be read, the
/// fresh capture cannot be written, or the host differs
/// ([`BenchError::Crumbs`]).
pub fn verify(host: &dyn Host, cli: &Cli) -> Result<()> {
    let bundle_dir = require_bundle(cli, "verify")?;
    let before = load_fingerprint(host, &bundle_dir, FINGERPRINT_BEFORE)?;
    let now = fingerprint::capture(host, &fingerprint_spec(host, cli));
    write_fingerprint(host, &bundle_dir, FINGERPRINT_AFTER, &now)?;
    let diffs = fingerprint::diff(&before, &now);
    report_diff(host, &diffs);
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(BenchError::Crumbs(diffs.len()))
    }
}
