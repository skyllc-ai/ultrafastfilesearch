// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase-2a correctness tests for [`DriveCompactIndex::trigram_search`] — the
//! base ∪ delta overlay choke point (incremental-index-maintenance §5.2).
//!
//! These pin the *semantics* of the overlay by populating an [`IndexDelta`]
//! **manually** (the apply path that fills it for real lands in Phase 2b), so
//! the merge + tombstone resolution is locked down independently of the USN
//! plumbing. The hard case is a rename: the record must become visible under
//! its new name yet vanish from its old one — which is exactly why tombstone
//! filtering is applied to the final candidate set, never per posting list.

use alloc::sync::Arc;
use std::path::PathBuf;

use uffs_text::case_fold::CaseFold;

use crate::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex, IndexDelta, IndexSource,
};
use crate::compact_storage::ColumnStorage;
use crate::trigram::{TrigramIndex, needle_trigrams};

/// Append a record (and its name bytes) to the fixture columns.
fn push_record(
    names: &mut Vec<u8>,
    records: &mut Vec<CompactRecord>,
    name: &str,
    parent: u32,
    dir: bool,
) {
    let offset = u32::try_from(names.len()).expect("fixture names blob fits u32");
    names.extend_from_slice(name.as_bytes());
    records.push(CompactRecord {
        name_offset: offset,
        flags: if dir { 0x10 } else { 0 },
        parent_idx: parent,
        name_len: u16::try_from(name.len()).expect("fixture name fits u16"),
        name_first_byte: name.as_bytes().first().copied().unwrap_or(0),
        ..CompactRecord::default()
    });
}

/// Build a flat fixture: a root "C" (idx 0) plus one file per name, each a
/// child of the root.  Returns the index with `delta = None` (pure base).
fn build_drive(file_names: &[&str]) -> DriveCompactIndex {
    let mut names: Vec<u8> = Vec::new();
    let mut records: Vec<CompactRecord> = Vec::new();

    push_record(&mut names, &mut records, "C", u32::MAX, true);
    for name in file_names {
        push_record(&mut names, &mut records, name, 0, false);
    }

    let fold = CaseFold::default_table();
    // Build the base CSR indexes before moving the columns into storage.
    let trigram = TrigramIndex::build(&records, &names, fold);
    let children = ChildrenIndex::build(&records);
    let ext_index = ExtensionIndex::build(&records);
    let count = u32::try_from(records.len()).expect("fixture record count fits u32");
    let frs_to_compact: Vec<u32> = (0..count).collect();
    // Fields in struct-definition order (clippy::inconsistent_struct_constructor).
    DriveCompactIndex {
        letter: uffs_mft::platform::DriveLetter::T,
        records: ColumnStorage::from_vec(records),
        names: ColumnStorage::from_vec(names),
        trigram: Arc::new(trigram),
        children: Arc::new(children),
        ext_index: Arc::new(ext_index),
        fold,
        ext_names: vec![Box::from("")],
        source: IndexSource::MftFile(PathBuf::from("T:")),
        source_epoch: 1,
        bloom: None,
        path_trie: None,
        frs_to_compact,
        delta: None,
    }
}

/// Trigram candidates as a sorted Vec for stable assertions.
fn candidates(drive: &DriveCompactIndex, needle: &str) -> Vec<u32> {
    let mut got = drive.trigram_search(needle).unwrap_or_default();
    got.sort_unstable();
    got
}

#[test]
fn delta_none_delegates_to_base_search() {
    let drive = build_drive(&["report.txt", "alpha.txt"]);
    // "repo" matches report.txt (idx 1) only; pure-base fast path.
    assert_eq!(candidates(&drive, "repo"), vec![1]);
    assert_eq!(candidates(&drive, "alpha"), vec![2]);
}

#[test]
fn create_via_delta_becomes_searchable() {
    let mut drive = build_drive(&["report.txt"]);
    let fold = drive.fold;
    // Simulate a create of "summary.log" at idx 2 (record need not exist for the
    // candidate set; trigram_search is a pre-filter over postings).
    let delta = drive.delta.get_or_insert_with(Default::default);
    let tris = needle_trigrams("summary.log", fold).unwrap();
    delta.add_record(2, &tris, 0, 0);

    assert_eq!(
        candidates(&drive, "summ"),
        vec![2],
        "new file visible via delta"
    );
    assert_eq!(
        candidates(&drive, "repo"),
        vec![1],
        "base file still visible"
    );
}

#[test]
fn rename_visible_under_new_name_and_gone_from_old() {
    let mut drive = build_drive(&["report.txt", "alpha.txt"]);
    let fold = drive.fold;
    // Rename idx 1 "report.txt" -> "summary.txt": tombstone its stale base
    // postings, then re-add under the new name's trigrams.
    let delta = drive.delta.get_or_insert_with(Default::default);
    delta.tombstone(1);
    let tris = needle_trigrams("summary.txt", fold).unwrap();
    delta.add_record(1, &tris, 0, 0);

    assert_eq!(
        candidates(&drive, "summ"),
        vec![1],
        "visible under NEW name"
    );
    assert_eq!(
        candidates(&drive, "repo"),
        Vec::<u32>::new(),
        "INVISIBLE under OLD name despite stale base postings"
    );
    assert_eq!(
        candidates(&drive, "alpha"),
        vec![2],
        "unrelated file untouched"
    );
}

#[test]
fn delete_via_tombstone_disappears() {
    let mut drive = build_drive(&["report.txt", "alpha.txt"]);
    // Delete idx 2 "alpha.txt": tombstone, no re-add.
    let delta = drive.delta.get_or_insert_with(Default::default);
    delta.tombstone(2);

    assert_eq!(
        candidates(&drive, "alpha"),
        Vec::<u32>::new(),
        "deleted file no longer a candidate"
    );
    assert_eq!(candidates(&drive, "repo"), vec![1], "sibling unaffected");
}

#[test]
fn short_needle_returns_none_like_base() {
    let mut drive = build_drive(&["report.txt"]);
    drive.delta = Some(IndexDelta::default());
    // < 3 codepoints -> None (caller falls back to linear scan), even with a
    // delta present.
    assert!(drive.trigram_search("re").is_none());
}
