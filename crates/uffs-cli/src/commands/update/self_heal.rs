// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase H self-heal trigger (design §19.4).
//!
//! On every `uffs` invocation, if a **live** update journal is present —
//! meaning an update is in-flight or was interrupted by a crash, kill, or
//! power loss — spawn the `uffs-update recover` helper to deterministically
//! finish it (roll forward past the commit point, else roll back) and bring
//! any stopped service back up (INV-1).
//!
//! It is **best-effort and non-blocking**: it never delays or fails a
//! normal command. The steady-state cost is a single `stat` (no live
//! journal → no spawn). The helper itself no-ops when the owning updater
//! is still alive, so a concurrent in-flight update is never disturbed.

use std::process::{Command, Stdio};

use super::acquire::find_helper;
use super::snapshot;

/// Live-journal file name written by the apply phase (`uffs-update apply`).
const LIVE_JOURNAL: &str = "journal.json";

/// Spawn `uffs-update recover` detached when a live journal exists.
pub(crate) fn trigger() {
    let journal = snapshot::update_dir().join(LIVE_JOURNAL);
    if !journal.exists() {
        return; // steady state: one stat, no spawn
    }
    let Ok(helper) = find_helper() else {
        return; // helper not installed → nothing we can self-heal with
    };
    // Detached + silenced: the heal runs in the background so this command
    // returns immediately; we never wait on or inspect the child.
    let _child = Command::new(helper)
        .arg("recover")
        .arg("--journal")
        .arg(&journal)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
