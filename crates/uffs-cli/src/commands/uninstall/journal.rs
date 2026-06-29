// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Crash-awareness for `uffs --uninstall` (task U-90/U-91).
//!
//! The removal operations are **idempotent** (deletes are `try_exists`-guarded,
//! service/winget removals no-op when already gone, the self-delete is
//! reboot-deferred), so resuming an interrupted uninstall is simply *running it
//! again*: re-detection finds whatever is left and removes it. This is the key
//! difference from the self-update flow, whose non-idempotent binary swaps need
//! a full replay journal.
//!
//! So all this needs is a small **in-progress marker**, written to the system
//! temp dir (which survives the lifecycle-dir deletion). If a launch finds the
//! marker, a prior run was interrupted; the CLI says so and the (idempotent)
//! run completes the job. The marker is cleared on a clean finish.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Where the in-progress marker lives: the system temp dir, outside every
/// directory the uninstall deletes.
fn marker_path() -> PathBuf {
    std::env::temp_dir().join("uffs-uninstall.in-progress")
}

/// Record that an uninstall is in progress.
///
/// # Errors
///
/// Returns an error if the marker cannot be written.
pub(crate) fn begin() -> Result<()> {
    write_marker(&marker_path())
}

/// Clear the in-progress marker on a clean finish.
///
/// # Errors
///
/// Returns an error if the marker exists but cannot be removed.
pub(crate) fn finish() -> Result<()> {
    clear_marker(&marker_path())
}

/// Whether a previous uninstall was interrupted (the marker survived).
pub(crate) fn was_interrupted() -> bool {
    marker_present(&marker_path())
}

/// Write the marker at `path`.
fn write_marker(path: &Path) -> Result<()> {
    std::fs::write(path, "uffs uninstall in progress")
        .with_context(|| format!("writing uninstall marker {}", path.display()))
}

/// Remove the marker at `path`; an already-absent marker is success.
fn clear_marker(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(_) if !marker_present(path) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Whether the marker at `path` exists.
fn marker_present(path: &Path) -> bool {
    path.try_exists().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{clear_marker, marker_present, write_marker};

    #[test]
    fn marker_round_trips() {
        let path = std::env::temp_dir().join("uffs-uninstall-journal-test.marker");
        // Start clean.
        clear_marker(&path).unwrap();
        assert!(!marker_present(&path));
        // Begin → present.
        write_marker(&path).unwrap();
        assert!(marker_present(&path));
        // Finish → gone, and finishing again is idempotent.
        clear_marker(&path).unwrap();
        assert!(!marker_present(&path));
        clear_marker(&path).unwrap();
    }
}
