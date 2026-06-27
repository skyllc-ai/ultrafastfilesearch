// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

// Build scripts run on the build host, not the shipping binary's target, so
// the workspace `deny(unwrap_used)` / `deny(expect_used)` runtime lints do not
// apply here; best-effort error handling with sensible fallbacks is the
// idiomatic shape for a build script whose only "failure" is "git not present".
#![allow(
    clippy::expect_used,
    reason = "build scripts may panic on build-host failure; workspace deny-expect targets runtime code"
)]

//! Build script for `uffs-daemon`.
//!
//! Emits `UFFS_GIT_SHA` — the short commit the daemon was built from, with a
//! `-dirty` suffix when the working tree had uncommitted changes — so the
//! startup log can stamp **which build** is running. A definitive build stamp
//! in the daemon log is how a field log (or a WIN test-script) is tied back to
//! the exact binary that produced it, closing the "ran the wrong/stale binary"
//! trap. Read back via `option_env!("UFFS_GIT_SHA")` in `startup.rs`.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|raw| raw.trim().to_owned())
        .filter(|trimmed| !trimmed.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());

    // Append `-dirty` when the working tree has uncommitted changes, so a
    // hand-tweaked local build is never mistaken for the clean commit.
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| !out.stdout.is_empty());

    let stamp = if dirty { format!("{sha}-dirty") } else { sha };
    println!("cargo:rustc-env=UFFS_GIT_SHA={stamp}");

    // Re-run when HEAD moves so the stamp tracks the checked-out commit.
    // Best-effort relative path from the crate dir to the repo `.git`; a wrong
    // path just means the stamp can lag one commit on an exotic layout, which
    // is acceptable for a dev-only marker.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=build.rs");
}
