// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Crate-root unit tests for [`crate`].
//!
//! Extracted from `lib.rs` to keep the daemon orchestrator under the
//! 800-LOC soft limit; mirrors the `compact_cache/tests.rs`,
//! `compact_mmap/tests.rs`, `compact_storage/tests.rs`, and
//! `runtime_orphans/tests.rs` pattern used elsewhere in the workspace.

use std::path::Path;

use super::drive_letter_matches;

/// `drive_letter_matches` keys discovered MFT files to the
/// `--drive` filter by walking `path.parent().file_name()` and
/// matching the `drive_<letter>` prefix case-insensitively.  The
/// contract is regression-pinned here so a future strip / split
/// rewrite can't silently change the prefix shape.
#[test]
fn drive_letter_matches_accepts_canonical_prefix() {
    // Standard discovery layout: `<data_dir>/drive_<letter>/<letter>_mft.iocp`.
    assert!(drive_letter_matches(
        Path::new("/data/drive_c/C_mft.iocp"),
        &['C']
    ));
    // Case-insensitive match: filter is uppercase, dir is lowercase.
    assert!(drive_letter_matches(
        Path::new("/data/drive_d/D_mft.iocp"),
        &['d']
    ));
    // Multi-letter filter: any match in `wanted` succeeds.
    assert!(drive_letter_matches(
        Path::new("/data/drive_e/E_mft.iocp"),
        &['C', 'D', 'E']
    ));
}

#[test]
fn drive_letter_matches_rejects_non_matching_prefix() {
    // Different drive letter.
    assert!(!drive_letter_matches(
        Path::new("/data/drive_c/C_mft.iocp"),
        &['D']
    ));
    // Empty `wanted` slice rejects everything (caller must
    // gate on `is_empty()` for the "all drives" case).
    assert!(!drive_letter_matches(
        Path::new("/data/drive_c/C_mft.iocp"),
        &[]
    ));
}

#[test]
fn drive_letter_matches_rejects_unknown_layout() {
    // Parent dir doesn't carry the `drive_` prefix.
    assert!(!drive_letter_matches(
        Path::new("/data/snapshot/C_mft.iocp"),
        &['C']
    ));
    // No parent at all — root file.
    assert!(!drive_letter_matches(Path::new("C_mft.iocp"), &['C']));
    // `drive_` prefix with no letter after (suffix.chars().next() = None).
    assert!(!drive_letter_matches(
        Path::new("/data/drive_/C_mft.iocp"),
        &['C']
    ));
}
