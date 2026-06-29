// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Impure glue for the `uffs --uninstall` analysis (tasks U-10/U-12): turn the
//! reused Phase-A [`DetectionReport`] into resolution [`Candidate`]s, and build
//! the OS executable search-dir list the pure ordering consumes.
//!
//! Side effects are confined to reading the environment (`current_exe`, `PATH`,
//! `current_dir`, `SystemRoot`); nothing here mutates the system.

use std::path::PathBuf;

use super::resolve_order::Candidate;
use crate::commands::update::model::DetectionReport;

/// Flatten the detection report's roots × binaries into resolution candidates
/// (one per discovered binary copy).
pub(crate) fn build_candidates(report: &DetectionReport) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for root in &report.roots {
        for binary in &root.binaries {
            candidates.push(Candidate {
                stem: binary.name.clone(),
                version: binary.version.clone(),
                channel: root.channel,
                scope: root.scope,
                dir: root.dir.clone(),
            });
        }
    }
    candidates
}

/// The ordered list of directories the OS searches for an unqualified
/// executable (design §5.1): the running image's dir, the system dirs
/// (Windows), the current dir, then PATH entries in order. On non-Windows this
/// is the current-exe dir, the current dir, then PATH.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.to_path_buf());
    }
    #[cfg(windows)]
    {
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            let root = PathBuf::from(system_root);
            dirs.push(root.join("System32"));
            dirs.push(root);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&path) {
            dirs.push(entry);
        }
    }
    dirs
}

/// The directories on the current `PATH`, in order. Used to offer removal of a
/// PATH entry that points at a UFFS root.
pub(crate) fn path_entries() -> Vec<PathBuf> {
    std::env::var_os("PATH").map_or_else(Vec::new, |path| std::env::split_paths(&path).collect())
}
