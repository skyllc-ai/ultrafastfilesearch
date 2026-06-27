// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Read-path overhead of the Phase-4 base ∪ delta overlay
//! (incremental-index-maintenance).
//!
//! `children_of` / `records_with_ext` return a **borrowed** base slice with
//! zero allocation when the delta is empty (freshly compacted), but when a
//! delta is present they sorted-merge base ∪ delta and validate each candidate
//! against the live records, allocating a `Vec`. This bench measures that
//! overhead directly — the exact concern flagged for Phase 4: does the overlay
//! regress tree search / `--ext` under churn?
//!
//! Each subject is benched in two states on the same fixture:
//!   * `compacted` — `delta = None` (the post-compaction / cold-load fast path)
//!   * `churned`   — `delta = Some` populated by a real `apply_usn_patch` batch
//!     of ~40k creates (near the 50k compaction ceiling = peak overlay size)
//!
//! plus a `for_each_child` (zero-alloc primitive) vs `children_of` (Cow)
//! comparison in the churned state, which sizes the headroom available if the
//! slice-callers ever need the zero-alloc path.
//!
//! Run with: `cargo bench --bench overlay_read`
//!
//! Reference baseline (Apple M-series, ~500k records, ~40k-change delta):
//!
//! ```text
//! subject                          delta=None    churned
//! children_of (one dir)              ~2.1 ns      ~631 ns
//! for_each_child (one dir, churn)         —       ~354 ns
//! records_with_ext (hot ext ~100k)   ~1.8 ns      ~295 µs
//! tree walk (2000 dirs / ~500k)      ~667 µs      ~2.09 ms
//! ```
//!
//! Read: the overlay overhead under churn is real but small in absolute terms
//! — a whole-tree walk stays ~2 ms, and the `records_with_ext` tax on a hot
//! extension is dwarfed by the downstream path-resolution of its ~100k results.
//! The zero-alloc `for_each_child` (~1.8× faster per call than `children_of` in
//! the churned state) is the ready lever if a future workload makes it bite.

#![expect(clippy::missing_docs_in_private_items, reason = "benchmark code")]
#![expect(clippy::min_ident_chars, reason = "benchmark uses short loop vars")]
#![expect(clippy::cast_possible_truncation, reason = "fixture indices fit u32")]
#![expect(
    unused_crate_dependencies,
    reason = "bench links uffs-core's full dependency set but uses only a subset"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "bench fixture indices are modulo a const-length array — always in bounds"
)]

use core::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use uffs_core::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex, IndexSource, apply_usn_patch,
};
use uffs_core::compact_storage::ColumnStorage;
use uffs_core::trigram::TrigramIndex;
use uffs_mft::usn::FileChange;
use uffs_text::case_fold::CaseFold;

/// Fixture shape: 2000 directories under the root, each with 250 files = ~500k
/// records — a realistic multi-hundred-thousand-record drive.
const NUM_DIRS: usize = 2_000;
const FILES_PER_DIR: usize = 250;
/// Churn just under the 50k compaction ceiling, so the delta is at its peak
/// size (worst case for the overlay merge cost) without triggering a refold.
const CHURN_CHANGES: usize = 40_000;
/// Five extensions, interned at ids 1..=5 (id 0 = no extension).
const EXTS: [&str; 5] = ["txt", "rs", "log", "json", "bin"];

fn push_file(
    names: &mut Vec<u8>,
    records: &mut Vec<CompactRecord>,
    name: &str,
    parent: u32,
    dir: bool,
    ext_id: u16,
) {
    let offset = names.len() as u32;
    names.extend_from_slice(name.as_bytes());
    records.push(CompactRecord {
        name_offset: offset,
        flags: if dir { 0x10 } else { 0 },
        parent_idx: parent,
        name_len: name.len() as u16,
        extension_id: ext_id,
        name_first_byte: name.as_bytes().first().copied().unwrap_or(0),
        ..CompactRecord::default()
    });
}

/// Build the base drive with `delta = None`. The trigram base is left empty
/// (not a subject of this bench) to keep fixture setup fast; children + ext are
/// real CSR builds since they are what the overlay reads.
fn build_drive() -> DriveCompactIndex {
    let mut names: Vec<u8> = Vec::new();
    let mut records: Vec<CompactRecord> = Vec::new();

    push_file(&mut names, &mut records, "C", u32::MAX, true, 0);
    for d in 0..NUM_DIRS {
        push_file(&mut names, &mut records, &format!("dir{d}"), 0, true, 0);
    }
    for d in 0..NUM_DIRS {
        let dir_idx = (1 + d) as u32;
        for f in 0..FILES_PER_DIR {
            let ext_id = (f % EXTS.len()) as u16 + 1;
            let name = format!("file{d}_{f}.{}", EXTS[f % EXTS.len()]);
            push_file(&mut names, &mut records, &name, dir_idx, false, ext_id);
        }
    }

    let fold = CaseFold::default_table();
    let children = ChildrenIndex::build(&records);
    let ext_index = ExtensionIndex::build(&records);
    let frs_to_compact: Vec<u32> = (0..records.len() as u32).collect();
    let ext_names: Vec<Box<str>> = core::iter::once(Box::from(""))
        .chain(EXTS.iter().map(|e| Box::from(*e)))
        .collect();
    DriveCompactIndex {
        letter: uffs_mft::platform::DriveLetter::T,
        records: ColumnStorage::from_vec(records),
        names: ColumnStorage::from_vec(names),
        trigram: TrigramIndex::empty().into(),
        children: children.into(),
        ext_index: ext_index.into(),
        fold,
        ext_names,
        source: IndexSource::MftFile(std::path::PathBuf::from("T:")),
        source_epoch: 1,
        bloom: None,
        path_trie: None,
        frs_to_compact,
        delta: None,
    }
}

/// Clone the base drive and populate a delta via a real `apply_usn_patch` batch
/// of `CHURN_CHANGES` creates spread across the directories.
fn churned_drive(base: &DriveCompactIndex) -> DriveCompactIndex {
    let mut drive = base.clone();
    let base_count = drive.records.len();
    let changes: Vec<FileChange> = (0..CHURN_CHANGES)
        .map(|k| {
            let parent_dir = (k % NUM_DIRS) as u64 + 1; // frs == idx for base records
            FileChange {
                frs: ((base_count + k) as u64).into(),
                parent_frs: parent_dir.into(),
                filename: format!("churn{k}.{}", EXTS[k % EXTS.len()]),
                created: true,
                ..FileChange::default()
            }
        })
        .collect();
    apply_usn_patch(&mut drive, &changes);
    assert!(drive.delta.is_some(), "churn must leave a populated delta");
    drive
}

/// Recursively walk every directory from the root via `children_of`, summing
/// child indices — the cumulative overlay cost a tree-scoped search pays.
fn tree_walk(drive: &DriveCompactIndex) -> u64 {
    let mut sum: u64 = 0;
    let mut stack: Vec<u32> = vec![0];
    while let Some(parent) = stack.pop() {
        for &child in drive.children_of(parent).iter() {
            sum = sum.wrapping_add(u64::from(child));
            // Only directories have children; recurse into them.
            if drive
                .records
                .get(child as usize)
                .is_some_and(|rec| rec.is_directory())
            {
                stack.push(child);
            }
        }
    }
    sum
}

fn bench_overlay(c: &mut Criterion) {
    let base = build_drive();
    let churned = churned_drive(&base);

    // A directory with a full base child list plus churn additions, and the
    // "txt" extension (id 1) with a large posting — the realistic hot lookups.
    let probe_dir: u32 = 1;
    let txt_ext: u16 = 1;

    let mut group = c.benchmark_group("overlay_read");

    group.bench_function("children_of/compacted", |b| {
        b.iter(|| black_box(base.children_of(black_box(probe_dir)).len()));
    });
    group.bench_function("children_of/churned", |b| {
        b.iter(|| black_box(churned.children_of(black_box(probe_dir)).len()));
    });
    // Zero-alloc primitive in the churned state — the headroom if the
    // slice-callers ever migrate off children_of.
    group.bench_function("for_each_child/churned", |b| {
        b.iter(|| {
            let mut n = 0_u64;
            churned.for_each_child(black_box(probe_dir), |_| n = n.wrapping_add(1));
            black_box(n)
        });
    });

    group.bench_function("records_with_ext/compacted", |b| {
        b.iter(|| black_box(base.records_with_ext(black_box(txt_ext)).len()));
    });
    group.bench_function("records_with_ext/churned", |b| {
        b.iter(|| black_box(churned.records_with_ext(black_box(txt_ext)).len()));
    });

    group.bench_function("tree_walk/compacted", |b| {
        b.iter(|| black_box(tree_walk(black_box(&base))));
    });
    group.bench_function("tree_walk/churned", |b| {
        b.iter(|| black_box(tree_walk(black_box(&churned))));
    });

    group.finish();
}

criterion_group!(benches, bench_overlay);
criterion_main!(benches);
