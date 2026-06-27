// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase-1 path-length oracle for [`super::apply_usn_patch`]
//! (incremental-index-maintenance §7).
//!
//! The per-change incremental `path_len` update done inside `apply_usn_patch`
//! must be **byte-identical** to a full `compute_path_lengths` rebuild — the
//! correctness gate the design requires ("base+delta must be byte-identical to
//! a full rebuild").  The hardest case is a **directory rename**, whose length
//! delta must propagate to every descendant's `path_len` via the children CSR.
//!
//! Kept in a dedicated sibling submodule so neither `compact_loader.rs` nor
//! `compact_loader_tests.rs` crosses the workspace 800-LOC policy ceiling.

use alloc::sync::Arc;
use std::path::PathBuf;

use uffs_mft::usn::FileChange;
use uffs_text::case_fold::CaseFold;

use super::{IndexSource, apply_usn_patch};
use crate::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex, compute_path_lengths,
};
use crate::compact_storage::ColumnStorage;
use crate::trigram::TrigramIndex;

/// Nested fixture: top dir "C" (frs 5) → dir "sub" (frs 6) → "deep.txt"
/// (frs 7).  Names: C[0..1] sub[1..4] deep.txt[4..12].  `path_len`s are
/// initialised via the cold-load BFS so the apply path takes over from a
/// correct baseline, exactly as it does after a real cold load.
fn build_nested_fixture() -> DriveCompactIndex {
    let names = b"Csubdeep.txt".to_vec();
    let records = vec![
        CompactRecord {
            name_offset: 0,
            flags: 0x10,
            parent_idx: u32::MAX,
            name_len: 1,
            name_first_byte: b'C',
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 1,
            flags: 0x10, // directory — its rename must shift the subtree
            parent_idx: 0,
            name_len: 3,
            name_first_byte: b's',
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 4,
            parent_idx: 1,
            name_len: 8,
            name_first_byte: b'd',
            ..CompactRecord::default()
        },
    ];
    let fold = CaseFold::default_table();
    let frs_to_compact: Vec<u32> = (0_usize..20)
        .map(|frs| match frs {
            5 => 0_u32,
            6 => 1,
            7 => 2,
            _ => u32::MAX,
        })
        .collect();
    let mut drive = DriveCompactIndex {
        letter: uffs_mft::platform::DriveLetter::T,
        records: ColumnStorage::from_vec(records.clone()),
        names: ColumnStorage::from_vec(names.clone()),
        trigram: Arc::new(TrigramIndex::build(&records, &names, fold)),
        children: Arc::new(ChildrenIndex::build(&records)),
        ext_index: Arc::new(ExtensionIndex::build(&records)),
        fold,
        ext_names: vec![Box::from("")],
        source: IndexSource::MftFile(PathBuf::from("T:")),
        source_epoch: 1,
        bloom: None,
        path_trie: None,
        frs_to_compact,
        delta: None,
    };
    // Cold-load init of path_lens (the full BFS the apply path replaces).
    compute_path_lengths(drive.records.as_mut_slice(), &drive.names, drive.letter);
    drive
}

/// Assert the live (incremental) `path_len`s on `drive` equal a from-scratch
/// `compute_path_lengths` BFS over the same (now-mutated) records.
///
/// Only **live** records are compared: a tombstoned record
/// (`name_len == 0 && parent_idx == u32::MAX`, set by `apply_delete`) never
/// surfaces in search or path resolution, so its `path_len` is meaningless —
/// the incremental path leaves it stale while a full BFS recomputes it as a
/// root. That divergence is correct, so it is excluded.
fn assert_path_len_matches_full_rebuild(drive: &mut DriveCompactIndex) {
    let is_live = |rec: &CompactRecord| !(rec.name_len == 0 && rec.parent_idx == u32::MAX);
    let incremental: Vec<(usize, u16)> = drive
        .records
        .iter()
        .enumerate()
        .filter(|(_, rec)| is_live(rec))
        .map(|(idx, rec)| (idx, rec.path_len))
        .collect();
    compute_path_lengths(drive.records.as_mut_slice(), &drive.names, drive.letter);
    let full_rebuild: Vec<(usize, u16)> = drive
        .records
        .iter()
        .enumerate()
        .filter(|(_, rec)| is_live(rec))
        .map(|(idx, rec)| (idx, rec.path_len))
        .collect();
    assert_eq!(
        incremental, full_rebuild,
        "incremental path_len must equal the full rebuild for live records; \
         incremental={incremental:?} full={full_rebuild:?}",
    );
}

#[test]
fn incremental_path_len_matches_full_rebuild_oracle() {
    let mut drive = build_nested_fixture();

    // Apply a batch that exercises every path op: directory rename (subtree Δ),
    // a fresh create, and a file rename.
    apply_usn_patch(&mut drive, &[
        FileChange {
            frs: 6_u64.into(),
            parent_frs: 5_u64.into(),
            filename: "subdirectory".to_owned(), // longer → Δ > 0
            renamed: true,
            ..FileChange::default()
        },
        FileChange {
            frs: 8_u64.into(),
            parent_frs: 5_u64.into(),
            filename: "new.bin".to_owned(),
            created: true,
            ..FileChange::default()
        },
        FileChange {
            frs: 7_u64.into(),
            parent_frs: 6_u64.into(),
            filename: "deep-renamed.txt".to_owned(),
            renamed: true,
            ..FileChange::default()
        },
    ]);

    // `apply_usn_patch` used the INCREMENTAL path update (batch < threshold).
    assert_path_len_matches_full_rebuild(&mut drive);
}

/// Regression guard for the delete-only batch: a delete pushes **no**
/// `PathChange` (it tombstones its record and shifts no surviving record's
/// `path_len`), so the apply's `path_changes` slice is empty.  The path update
/// must then be a *no-op* — NOT a fall-back to the full O(total) BFS, which on
/// a live 3.9 M-record drive was a 0.5 s per-apply regression.  Surviving
/// records' `path_len`s must still equal a full rebuild afterwards.
#[test]
fn delete_only_batch_leaves_path_lengths_correct_without_full_recompute() {
    let mut drive = build_nested_fixture();

    // Delete the leaf "deep.txt" (frs 7).  No create / rename → empty
    // path_changes → must take the no-op incremental branch.
    apply_usn_patch(&mut drive, &[FileChange {
        frs: 7_u64.into(),
        parent_frs: 6_u64.into(),
        deleted: true,
        ..FileChange::default()
    }]);

    // "C" and "sub" are untouched survivors; their path_len must match a full
    // rebuild over the post-delete record set.
    assert_path_len_matches_full_rebuild(&mut drive);
}

/// Phase-4b ordering guard: a directory rename (subtree Δ) **and** a child
/// created inside that directory in the **same batch**. The subtree walk reads
/// the base ∪ delta children, and the delta is populated *before* the path
/// walk, so the same-batch create must be found and shifted (or get the right
/// `path_len` directly) regardless of intra-batch order. The created child's
/// `path_len` must equal a full rebuild — which it only can if the walk sees
/// the delta-added child.
#[test]
fn dir_rename_with_same_batch_child_create_matches_full_rebuild() {
    let mut drive = build_nested_fixture();

    apply_usn_patch(&mut drive, &[
        // Rename dir "sub" (frs 6) → "subdirectory" (longer → Δ > 0).
        FileChange {
            frs: 6_u64.into(),
            parent_frs: 5_u64.into(),
            filename: "subdirectory".to_owned(),
            renamed: true,
            ..FileChange::default()
        },
        // Create "inside.txt" (frs 8) as a child of that same dir (frs 6).
        FileChange {
            frs: 8_u64.into(),
            parent_frs: 6_u64.into(),
            filename: "inside.txt".to_owned(),
            created: true,
            ..FileChange::default()
        },
    ]);

    // Every live record (incl. the same-batch create under the renamed dir)
    // must have the byte-identical path_len of a from-scratch BFS.
    assert_path_len_matches_full_rebuild(&mut drive);
}
