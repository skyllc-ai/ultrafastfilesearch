// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs update doctor` — end-to-end health check of the self-update flow.
//!
//! The CLI is the thin half: it runs Phase-A detection, freezes a snapshot
//! (so the helper has full install/version/service context), and spawns
//! `uffs-update doctor` with it. The helper (which owns the HTTP stack and
//! every repair primitive) runs the actual checks and prints the report;
//! we inherit its stdio and exit code.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use super::acquire::{DEFAULT_REPO, find_helper};
use super::snapshot;

/// Spawn `uffs-update doctor` against a written snapshot, forwarding the
/// pass-through flags (`--repair`, `--offline`, `--version <tag>`).
///
/// # Errors
///
/// Fails if the helper cannot be located or it exits non-zero (the latter
/// means the doctor found a hard failure).
pub(crate) fn spawn(snapshot_path: &Path, args: &[String]) -> Result<()> {
    let helper = find_helper()?;
    let stage = snapshot::update_dir().join("stage");
    let mut command = Command::new(&helper);
    command
        .args(["doctor", "--repo", DEFAULT_REPO, "--snapshot"])
        .arg(snapshot_path)
        .arg("--stage")
        .arg(&stage);
    if args.iter().any(|arg| arg == "--repair") {
        command.arg("--repair");
    }
    if args.iter().any(|arg| arg == "--offline") {
        command.arg("--offline");
    }
    if let Some(tag) = flag_value(args, "--version") {
        command.args(["--version", &tag]);
    }
    let status = command
        .status()
        .with_context(|| format!("spawning {}", helper.display()))?;
    if !status.success() {
        bail!(
            "uffs-update doctor reported failures (exit {:?})",
            status.code()
        );
    }
    Ok(())
}

/// Return the value following `name` in `args` (`--name value`).
fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}
