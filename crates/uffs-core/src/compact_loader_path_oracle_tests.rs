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

use std::path::PathBuf;

use uffs_mft::usn::FileChange;
use uffs_text::case_fold::CaseFold;

use super::{IndexSource, apply_usn_patch};
use crate::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex, compute_path_lengths,
};
use crate::compact_storage::ColumnStorage;
use crate::trigram::TrigramIndex;

#[test]
fn incremental_path_len_matches_full_rebuild_oracle() {
    // Nested fixture: top dir "C" (frs 5) → dir "sub" (frs 6) → "deep.txt"
    // (frs 7).  Names: C[0..1] sub[1..4] deep.txt[4..12].
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
        trigram: TrigramIndex::build(&records, &names, fold),
        children: ChildrenIndex::build(&records),
        ext_index: ExtensionIndex::build(&records),
        fold,
        ext_names: vec![Box::from("")],
        source: IndexSource::MftFile(PathBuf::from("T:")),
        source_epoch: 1,
        bloom: None,
        path_trie: None,
        frs_to_compact,
    };
    // Cold-load init of path_lens (the full BFS the apply path replaces).
    compute_path_lengths(drive.records.as_mut_slice(), &drive.names, drive.letter);

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
    let incremental: Vec<u16> = drive.records.iter().map(|rec| rec.path_len).collect();
    // Ground truth: a from-scratch BFS over the now-mutated records.
    compute_path_lengths(drive.records.as_mut_slice(), &drive.names, drive.letter);
    let full_rebuild: Vec<u16> = drive.records.iter().map(|rec| rec.path_len).collect();

    assert_eq!(
        incremental, full_rebuild,
        "incremental path_len must equal the full rebuild (incl. directory-rename \
         subtree propagation); incremental={incremental:?} full={full_rebuild:?}",
    );
}
