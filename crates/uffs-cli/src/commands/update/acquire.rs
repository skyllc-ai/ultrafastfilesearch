// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase C trigger: spawn the `uffs-update` helper to download + verify a
//! release into the staging dir.
//!
//! The HTTP/TLS stack lives in the separate `uffs-update` binary, so the
//! lean `uffs` CLI just locates it (next to `uffs`, else on `PATH`) and
//! shells out — keeping reqwest/rustls out of `uffs.exe`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use super::snapshot;

/// Default upstream repository for self-update artifacts.
const DEFAULT_REPO: &str = "skyllc-ai/UltraFastFileSearch";

/// Platform file name of the helper binary.
const fn helper_file_name() -> &'static str {
    if cfg!(windows) {
        "uffs-update.exe"
    } else {
        "uffs-update"
    }
}

/// Locate the `uffs-update` helper — next to the running `uffs` first,
/// then on `PATH`.
fn find_helper() -> Result<PathBuf> {
    let name = helper_file_name();
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.exists())
    {
        return Ok(sibling);
    }
    which::which(name)
        .with_context(|| format!("cannot find `{name}` — install it alongside `uffs`"))
}

/// Spawn the acquire helper for an optional target version tag.
///
/// # Errors
///
/// Fails if the helper cannot be located or it exits non-zero.
pub(crate) fn spawn(version: Option<&str>) -> Result<()> {
    let helper = find_helper()?;
    let stage = snapshot::update_dir().join("stage");
    let mut command = Command::new(&helper);
    command
        .args(["acquire", "--repo", DEFAULT_REPO, "--stage"])
        .arg(&stage);
    if let Some(tag) = version {
        command.args(["--version", tag]);
    }
    let status = command
        .status()
        .with_context(|| format!("spawning {}", helper.display()))?;
    if !status.success() {
        bail!("uffs-update acquire failed (exit {:?})", status.code());
    }
    Ok(())
}
