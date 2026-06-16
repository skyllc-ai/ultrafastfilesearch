// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs --update --apply` — run the **mutating** update end to end.
//!
//! The CLI freezes a snapshot and hands off to the `uffs-update` helper,
//! which performs the journaled flow: pre-flight → quiesce → backup/swap/
//! smoke → commit → restore → prune, rolling back on any pre-commit
//! failure (and never leaving a service down, INV-1). Acquire must have
//! staged + verified the binaries first; the CLI chains the two.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use super::acquire::find_helper;
use super::snapshot;

/// Spawn `uffs-update apply` against a written snapshot, streaming its
/// output. The staging dir is the one acquire filled.
///
/// # Errors
///
/// Fails if the helper cannot be located or the apply exits non-zero (in
/// which case the helper has already rolled back + restored services).
pub(crate) fn spawn(snapshot_path: &Path) -> Result<()> {
    let helper = find_helper()?;
    let stage = snapshot::update_dir().join("stage");
    let status = Command::new(&helper)
        .args(["apply", "--snapshot"])
        .arg(snapshot_path)
        .arg("--stage")
        .arg(&stage)
        .status()
        .with_context(|| format!("spawning {}", helper.display()))?;
    if !status.success() {
        bail!(
            "uffs-update apply failed (exit {:?}) — the helper rolled back + restored services",
            status.code()
        );
    }
    Ok(())
}
