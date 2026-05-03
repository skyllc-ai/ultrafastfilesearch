// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unit tests for [`super::apply_usn_patch`] — the platform-agnostic
//! in-place USN delta applier shared across the daemon's per-shard
//! USN journal loop (Phase 7 task 7.1).
//!
//! These tests exercise the function with synthesised
//! [`uffs_mft::usn::FileChange`] arrays and a synthesised
//! [`crate::compact::DriveCompactIndex`] so the contract is pinned
//! end-to-end **without** requiring a Windows host or a live MFT
//! source.  The journal source itself
//! ([`uffs_mft::usn::read_usn_journal`]) is Windows-only; everything
//! downstream of `read_usn_journal` is covered here.
//!
//! Extracted into a sibling submodule so `compact_loader.rs` stays
//! well below the file-size policy ceiling.

use std::path::PathBuf;

use uffs_mft::usn::FileChange;
use uffs_text::case_fold::CaseFold;

use super::{IndexSource, apply_usn_patch};
use crate::compact::{ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex};
use crate::compact_storage::ColumnStorage;
use crate::trigram::TrigramIndex;

/// Build a synthetic 4-record drive: root directory + three files.
///
/// FRS layout (NTFS-style) → `compact_idx` mapping:
///
/// * FRS  5 → root dir `"C"`        @ `compact_idx` 0
/// * FRS 10 → `"foo.txt"` parent=5 @ `compact_idx` 1
/// * FRS 11 → `"bar.rs"`  parent=5 @ `compact_idx` 2
/// * FRS 12 → `"baz.md"`  parent=5 @ `compact_idx` 3
///
/// Returns the drive plus a `frs_to_compact[100]` mapping sized to
/// cover FRS 13 (newly-created in tests) and FRS 99 (unmapped /
/// skipped) without bounds-check failures.
fn make_synthetic_drive() -> (DriveCompactIndex, Vec<u32>) {
    // Names blob layout:
    //   "C"       [0..1]
    //   "foo.txt" [1..8]
    //   "bar.rs"  [8..14]
    //   "baz.md"  [14..20]
    let names = b"Cfoo.txtbar.rsbaz.md".to_vec();
    let records = vec![
        CompactRecord {
            name_offset: 0,
            flags: 0x10, // directory
            parent_idx: u32::MAX,
            name_len: 1,
            name_first_byte: b'C',
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 1,
            parent_idx: 0,
            name_len: 7,
            name_first_byte: b'f',
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 8,
            parent_idx: 0,
            name_len: 6,
            name_first_byte: b'b',
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 14,
            parent_idx: 0,
            name_len: 6,
            name_first_byte: b'b',
            ..CompactRecord::default()
        },
    ];

    let fold = CaseFold::default_table();
    let trigram = TrigramIndex::build(&records, &names, fold);
    let children = ChildrenIndex::build(&records);
    let ext_index = ExtensionIndex::build(&records);

    let drive = DriveCompactIndex {
        letter: 'T',
        records: ColumnStorage::from_vec(records),
        names: ColumnStorage::from_vec(names),
        trigram,
        children,
        ext_index,
        fold,
        ext_names: vec![Box::from("")],
        source: IndexSource::MftFile(PathBuf::from("T:")),
        source_epoch: 42,
        bloom: None,
        path_trie: None,
    };

    // FRS → compact_idx mapping.  Sized to cover FRS 13 (newly-created)
    // and FRS 99 (skipped — no compact slot) without bounds checks.
    // Built via iterator-collect to avoid `clippy::indexing_slicing`;
    // FRS 13 left at u32::MAX so the create-branch fires for it.
    let frs_to_compact: Vec<u32> = (0_usize..100)
        .map(|frs| match frs {
            5 => 0_u32,
            10 => 1,
            11 => 2,
            12 => 3,
            _ => u32::MAX,
        })
        .collect();

    (drive, frs_to_compact)
}

/// Headline contract test: a mixed batch of create / delete / rename /
/// skip changes lands the matching counters on `PatchStats` without
/// cross-talk.
#[test]
fn apply_usn_patch_handles_create_delete_rename_skip() {
    let (mut drive, frs_to_compact) = make_synthetic_drive();

    let changes = vec![
        // Delete FRS 10 ("foo.txt").
        FileChange {
            frs: 10,
            deleted: true,
            ..FileChange::default()
        },
        // Rename FRS 11 ("bar.rs" → "bar2.rs"), parent unchanged.
        FileChange {
            frs: 11,
            parent_frs: 5,
            filename: "bar2.rs".to_owned(),
            renamed: true,
            ..FileChange::default()
        },
        // Create FRS 13 ("new.txt", parent=root).
        FileChange {
            frs: 13,
            parent_frs: 5,
            filename: "new.txt".to_owned(),
            created: true,
            ..FileChange::default()
        },
        // Skip: FRS 99 doesn't map to any compact record.
        FileChange {
            frs: 99,
            deleted: true,
            ..FileChange::default()
        },
    ];

    let stats = apply_usn_patch(&mut drive, &changes, &frs_to_compact);

    assert_eq!(
        stats.deleted, 1,
        "exactly one delete should have applied (FRS 10)"
    );
    assert_eq!(
        stats.created, 1,
        "exactly one create should have applied (FRS 13)"
    );
    assert_eq!(
        stats.renamed, 1,
        "exactly one rename should have applied (FRS 11)"
    );
    assert_eq!(
        stats.skipped, 1,
        "exactly one skip should have happened (FRS 99 unmapped)"
    );
}

/// Delete contract: the deleted record's `name_len` is zeroed and its
/// `parent_idx` is set to `u32::MAX` so the CSR rebuild excludes it
/// from any directory's child list (tombstone semantics).
#[test]
fn apply_usn_patch_marks_deleted_record_with_zero_name_len() {
    let (mut drive, frs_to_compact) = make_synthetic_drive();
    let changes = vec![FileChange {
        frs: 10,
        deleted: true,
        ..FileChange::default()
    }];

    apply_usn_patch(&mut drive, &changes, &frs_to_compact);

    let record = drive
        .records
        .as_slice()
        .get(1)
        .expect("synthetic drive has at least 2 records");
    assert_eq!(record.name_len, 0, "deleted record's name_len must be 0");
    assert_eq!(
        record.parent_idx,
        u32::MAX,
        "deleted record's parent_idx must be u32::MAX so CSR rebuild excludes it"
    );
}

/// Rename contract: the renamed record's `name_offset` points at a
/// fresh slot in the names blob holding the new bytes, and `name_len`
/// reflects the new byte count.
#[test]
fn apply_usn_patch_renamed_record_has_new_name_in_blob() {
    let (mut drive, frs_to_compact) = make_synthetic_drive();
    let changes = vec![FileChange {
        frs: 11,
        parent_frs: 5,
        filename: "bar2.rs".to_owned(),
        renamed: true,
        ..FileChange::default()
    }];

    apply_usn_patch(&mut drive, &changes, &frs_to_compact);

    let record = drive
        .records
        .as_slice()
        .get(2)
        .expect("synthetic drive has at least 3 records");
    assert_eq!(record.name_len, 7, "renamed name 'bar2.rs' is 7 bytes");

    let name_start = record.name_offset as usize;
    let name_end = name_start + record.name_len as usize;
    let name_bytes = drive
        .names
        .as_slice()
        .get(name_start..name_end)
        .expect("name slice must lie within names blob");
    assert_eq!(
        name_bytes, b"bar2.rs",
        "renamed record's name slot must hold the new bytes"
    );
}

/// Create contract: a newly-created FRS that doesn't map to an
/// existing compact slot (`frs_to_compact[frs] == u32::MAX`) appends
/// a fresh record at the end with the correct `parent_idx`,
/// `name_len`, and `name_first_byte`.
#[test]
fn apply_usn_patch_created_record_appended_with_correct_parent() {
    let (mut drive, frs_to_compact) = make_synthetic_drive();
    let initial_record_count = drive.records.len();

    let changes = vec![FileChange {
        frs: 13,
        parent_frs: 5,
        filename: "new.txt".to_owned(),
        created: true,
        ..FileChange::default()
    }];

    apply_usn_patch(&mut drive, &changes, &frs_to_compact);

    assert_eq!(
        drive.records.len(),
        initial_record_count + 1,
        "create must append exactly one record"
    );

    let record = drive
        .records
        .as_slice()
        .get(initial_record_count)
        .expect("appended record must be reachable at the new tail");
    assert_eq!(record.name_len, 7, "'new.txt' is 7 bytes");
    assert_eq!(
        record.parent_idx, 0,
        "parent compact_idx should be 0 (root)"
    );
    assert_eq!(
        record.name_first_byte, b'n',
        "first-byte cache must reflect the new name"
    );
}

/// Empty-batch fast path: passing zero changes produces all-zero
/// stats and leaves the drive byte-for-byte unchanged in shape (same
/// record count, same names blob length).  Pins that the rebuilt
/// derived structures don't grow on an empty batch.
#[test]
fn apply_usn_patch_no_changes_is_no_op_with_zero_stats() {
    let (mut drive, frs_to_compact) = make_synthetic_drive();
    let initial_record_count = drive.records.len();
    let initial_names_len = drive.names.len();

    let stats = apply_usn_patch(&mut drive, &[], &frs_to_compact);

    assert_eq!(stats.deleted, 0);
    assert_eq!(stats.created, 0);
    assert_eq!(stats.renamed, 0);
    assert_eq!(stats.skipped, 0);
    assert_eq!(drive.records.len(), initial_record_count);
    assert_eq!(drive.names.len(), initial_names_len);
}

/// Rebuild invariant: after `apply_usn_patch` returns, the children
/// CSR reflects the post-mutation `parent_idx` state.  Specifically,
/// once a record is deleted (`parent_idx` → `u32::MAX`), the root's
/// children list must no longer include that record's `compact_idx`.
#[test]
fn apply_usn_patch_rebuilds_children_csr_excluding_deletes() {
    let (mut drive, frs_to_compact) = make_synthetic_drive();

    // Pre-state sanity: root (compact_idx 0) starts with three
    // children — compact_idx 1 ("foo.txt"), 2 ("bar.rs"), 3 ("baz.md").
    let initial_root_children: Vec<u32> = drive.children.get(0).to_vec();
    assert_eq!(
        initial_root_children.len(),
        3,
        "synthetic root starts with three children"
    );

    let changes = vec![FileChange {
        frs: 10,
        deleted: true,
        ..FileChange::default()
    }];

    apply_usn_patch(&mut drive, &changes, &frs_to_compact);

    let post_root_children: Vec<u32> = drive.children.get(0).to_vec();
    assert!(
        !post_root_children.contains(&1),
        "deleted compact_idx 1 must not appear in root's children CSR after rebuild"
    );
    assert_eq!(
        post_root_children.len(),
        2,
        "root should have two surviving children after one delete"
    );
}
