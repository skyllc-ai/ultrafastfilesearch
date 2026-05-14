// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs daemon load` — hot-load MFT file(s) into a running daemon.
//!
//! Extracted from `super::daemon_mgmt` so the lifecycle file stays
//! under the workspace 800-LOC ceiling without a file-size exception.
//! Mirrors the sibling-module decomposition pattern already used by
//! `super::daemon_tiering` for the Phase 8 operator commands.
//!
//! The two filesystem-walking helpers (`resolve_drive_subdirs` +
//! `find_best_mft_in_dir`) are private to this module — they are
//! only used by the `daemon_load` shim, not by the start-side
//! data-source resolver in `daemon_mgmt.rs::daemon_start` (that
//! path forwards raw flags to `uffsd` and lets the daemon do its
//! own resolution).  Inline-code style (not intra-doc links) is
//! intentional: `cargo doc` cannot resolve link targets to private
//! `fn` items in another rendering pass, and we want
//! `--no-deps -- -D rustdoc::broken-intra-doc-links` to stay clean.

use anyhow::{Context as _, Result};
use uffs_client::connect_sync::UffsClientSync;

/// `uffs daemon load` — resolve `--mft-file` / `--data-dir` /
/// `--drive` arguments the same way `daemon start` does, but send
/// them to the running daemon via the `load_drive` IPC method
/// instead of spawning a new process.
///
/// # Errors
///
/// Returns an error when the daemon is not running, when no data
/// source resolves to anything load-able, or when the underlying
/// `load_drive` / `load_drive_letters` IPC fails.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn daemon_load(
    mft_files: &[std::path::PathBuf],
    data_dir: Option<&std::path::Path>,
    drives: &[uffs_mft::platform::DriveLetter],
    no_cache: bool,
) -> Result<()> {
    let Ok(mut client) = UffsClientSync::connect_raw() else {
        println!("Daemon is not running. Start it first with `uffs daemon start`.");
        return Ok(());
    };

    // Collect MFT file paths to send to the daemon.
    let mut paths: Vec<String> = Vec::new();

    // Direct --mft-file arguments.
    for mft_path in mft_files {
        paths.push(mft_path.to_string_lossy().into_owned());
    }

    // Resolve --data-dir (optionally filtered by --drive letters).
    if let Some(dir) = data_dir {
        let drive_subdirs = resolve_drive_subdirs(dir, drives);
        for mft_path in &drive_subdirs {
            paths.push(mft_path.to_string_lossy().into_owned());
        }
    }

    // Drive letters without --data-dir → hot-load by letter.
    // On Windows this reads live NTFS; on non-Windows the daemon uses its
    // own data_dir.
    let direct_drives: Vec<uffs_mft::platform::DriveLetter> =
        if data_dir.is_none() && paths.is_empty() {
            drives.to_vec()
        } else {
            Vec::new()
        };

    if paths.is_empty() && direct_drives.is_empty() {
        anyhow::bail!(
            "Nothing to load. Provide --mft-file <path>, --data-dir <path>, \
             --drive <letter>, or --data-dir <path> --drive <letter>."
        );
    }

    // Load MFT files (if any).
    let mut resp = if paths.is_empty() {
        uffs_client::protocol::response::LoadDriveResponse {
            loaded: Vec::new(),
            already_loaded: Vec::new(),
            errors: Vec::new(),
        }
    } else {
        println!("Loading {} MFT file(s)...", paths.len());
        for path in &paths {
            println!("  → {path}");
        }
        client
            .load_drive(&paths, no_cache)
            .with_context(|| "load_drive IPC failed")?
    };

    // Hot-load by drive letter (if any).
    if !direct_drives.is_empty() {
        let drive_list: String = direct_drives
            .iter()
            .map(|ch| format!("{ch}:"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("Hot-loading drive(s): {drive_list}");
        let drive_resp = client
            .load_drive_letters(&direct_drives, no_cache)
            .with_context(|| "load_drive_letters IPC failed")?;
        resp.loaded.extend(drive_resp.loaded);
        resp.already_loaded.extend(drive_resp.already_loaded);
        resp.errors.extend(drive_resp.errors);
    }

    if !resp.loaded.is_empty() {
        println!(
            "Loaded: {}",
            resp.loaded
                .iter()
                .map(|ch| format!("{ch}:"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !resp.already_loaded.is_empty() {
        println!(
            "Already loaded (skipped): {}",
            resp.already_loaded
                .iter()
                .map(|ch| format!("{ch}:"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for err_msg in &resp.errors {
        println!("Error: {err_msg}");
    }

    Ok(())
}

/// Discover MFT files in `data_dir/drive_*` subdirectories.
///
/// If `drives` is non-empty, only look in `drive_c`, `drive_d`, etc.
/// for the specified letters.  Otherwise, scan all `drive_*`
/// subdirs.
///
/// Returns the best MFT file path from each matching subdirectory.
#[expect(clippy::print_stderr, reason = "CLI diagnostic warning")]
fn resolve_drive_subdirs(
    data_dir: &std::path::Path,
    drives: &[uffs_mft::platform::DriveLetter],
) -> Vec<std::path::PathBuf> {
    let mut results = Vec::new();

    let entries = match std::fs::read_dir(data_dir) {
        Ok(iter) => iter,
        Err(err) => {
            eprintln!(
                "Warning: cannot read data-dir {}: {err}",
                data_dir.display()
            );
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|osn| osn.to_str()) else {
            continue;
        };
        // Match `drive_c`, `drive_d`, etc.
        let Some(letter_str) = name.strip_prefix("drive_") else {
            continue;
        };
        let Some(letter_char) = letter_str.chars().next() else {
            continue;
        };
        let Ok(letter) = uffs_mft::platform::DriveLetter::parse(letter_char) else {
            continue;
        };

        // If specific drives requested, skip others.
        if !drives.is_empty() && !drives.contains(&letter) {
            continue;
        }

        // Find the best MFT file in this subdir (prefer .iocp > .uffs > .bin > .raw).
        if let Some(best) = find_best_mft_in_dir(&path) {
            results.push(best);
        }
    }

    results
}

/// Find the best MFT file in a directory by extension preference.
///
/// Preference order: `.iocp` > `.uffs` > `.bin` > `.raw` > `.mft`.
fn find_best_mft_in_dir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    const PRIORITY: &[&str] = &["iocp", "uffs", "bin", "raw", "mft"];

    let entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();

    for ext in PRIORITY {
        if let Some(path) = entries.iter().find(|path| {
            path.extension()
                .and_then(|osn| osn.to_str())
                .is_some_and(|file_ext| file_ext.eq_ignore_ascii_case(ext))
        }) {
            return Some(path.clone());
        }
    }

    None
}
