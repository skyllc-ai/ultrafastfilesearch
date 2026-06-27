// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-apply USN-patch cost — the incremental-index-maintenance perf guard.
//!
//! [`apply_usn_patch`] is the hot path the USN journal loop runs on every poll:
//! it mutates the record columns in O(changed), overlays the batch onto the
//! base ∪ delta trigram / extension / children indexes, and refreshes the
//! touched records' `path_len`. The whole point of the project is that this
//! cost scales with the **batch size**, not the **drive size** — a 256-change
//! poll on a 4-million-record drive must not re-pay an O(total) rebuild.
//!
//! This bench locks that in. The fixture is a ~500k-record drive; each subject
//! applies a representative batch to a **fresh clone** (clone excluded from the
//! timing via `iter_batched`), so what is measured is the apply alone. The
//! profiles span the realistic USN-poll shapes:
//!   * `creates/256`   — a typical settle-debounced poll batch
//!   * `creates/4000`  — a heavy burst (bulk extract / installer)
//!   * `mixed/4000`    — creates + deletes + file renames interleaved
//!   * `deletes/4000`  — the tombstone path
//!
//! It is a guard, not a target: a regression shows up as a profile's time
//! jumping with the **fixture** size (it should not — only
//! `compute_path_lengths` in the >50k fallback is O(total), and these batches
//! stay under that), or a batch's per-change cost ballooning. The numbers
//! replace the ad-hoc development timing the project carried while the overlay
//! was being built.
//!
//! Run with: `cargo bench --bench apply_cost`
//!
//! Reference baseline (Apple M-series, ~500k-record fixture):
//!
//! ```text
//! profile          time/batch
//! creates/256        ~120 µs
//! creates/4000       ~1.9 ms
//! mixed/4000         ~2.4 ms
//! deletes/4000       ~1.1 ms
//! ```

// The bench binary links uffs-core's full dependency set but uses only a
// subset; this is a structural fact of compiling a bench inside the crate, not
// a code-quality lint to fix.
#![expect(
    unused_crate_dependencies,
    reason = "bench links uffs-core's full dependency set but uses only a subset"
)]

use core::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use uffs_core::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex, IndexSource, apply_usn_patch,
};
use uffs_core::compact_storage::ColumnStorage;
use uffs_core::trigram::TrigramIndex;
use uffs_mft::usn::FileChange;
use uffs_text::case_fold::CaseFold;

/// Directories created directly under the fixture root.
const NUM_DIRS: usize = 2_000;
/// Files created in each directory; `NUM_DIRS * FILES_PER_DIR` ≈ 500k records,
/// a realistic multi-hundred-thousand-record drive.
const FILES_PER_DIR: usize = 250;
/// Extensions cycled across the fixture files, interned at ids 1..=5 (id 0 = no
/// extension). The leading const guarantees `EXTS` is never empty, so every
/// `index % EXTS.len()` below is a valid offset.
const EXTS: [&str; 5] = ["txt", "rs", "log", "json", "bin"];

/// The fixture extension for the `index`-th file, wrapping across [`EXTS`].
/// Total (panic-free): `index % EXTS.len()` is always a valid offset, and the
/// const-asserted non-empty `EXTS` makes the `unwrap_or` fallback unreachable.
fn ext_at(index: usize) -> &'static str {
    EXTS.get(index % EXTS.len()).copied().unwrap_or("bin")
}

/// The 1-based extension id (interned offset into the drive's `ext_names`) for
/// the `index`-th file; 0 is reserved for "no extension".
fn ext_id_at(index: usize) -> u16 {
    // index % EXTS.len() ∈ 0..5, so +1 ∈ 1..=5 — always fits u16.
    u16::try_from(index % EXTS.len() + 1).unwrap_or(0)
}

/// Append one record + its name bytes to the growing fixture columns.
fn push_file(
    names: &mut Vec<u8>,
    records: &mut Vec<CompactRecord>,
    name: &str,
    parent: u32,
    is_dir: bool,
    ext_id: u16,
) {
    let name_offset = u32::try_from(names.len()).unwrap_or(u32::MAX);
    names.extend_from_slice(name.as_bytes());
    records.push(CompactRecord {
        name_offset,
        flags: if is_dir { 0x10 } else { 0 },
        parent_idx: parent,
        name_len: u16::try_from(name.len()).unwrap_or(u16::MAX),
        extension_id: ext_id,
        name_first_byte: name.as_bytes().first().copied().unwrap_or(0),
        ..CompactRecord::default()
    });
}

/// Build the base drive with `delta = None` (cold-load / post-compaction
/// state). The trigram base is left empty (apply overlays it via the delta
/// either way) to keep fixture setup fast; children + ext are real CSR builds.
fn build_drive() -> DriveCompactIndex {
    let mut names: Vec<u8> = Vec::new();
    let mut records: Vec<CompactRecord> = Vec::new();

    push_file(&mut names, &mut records, "C", u32::MAX, true, 0);
    for dir in 0..NUM_DIRS {
        push_file(&mut names, &mut records, &format!("dir{dir}"), 0, true, 0);
    }
    for dir in 0..NUM_DIRS {
        let dir_idx = u32::try_from(1 + dir).unwrap_or(u32::MAX);
        for file in 0..FILES_PER_DIR {
            let name = format!("file{dir}_{file}.{}", ext_at(file));
            push_file(
                &mut names,
                &mut records,
                &name,
                dir_idx,
                false,
                ext_id_at(file),
            );
        }
    }

    let fold = CaseFold::default_table();
    let children = ChildrenIndex::build(&records);
    let ext_index = ExtensionIndex::build(&records);
    let frs_to_compact: Vec<u32> = (0..records.len())
        .map(|idx| u32::try_from(idx).unwrap_or(u32::MAX))
        .collect();
    let ext_names: Vec<Box<str>> = core::iter::once(Box::from(""))
        .chain(EXTS.iter().map(|ext| Box::from(*ext)))
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

/// FRS of an existing base file record (directory `dir`, file `file`); frs ==
/// idx in the base, so this also serves as a valid `parent_frs` for a create.
const fn file_frs(dir: usize, file: usize) -> u64 {
    (1 + NUM_DIRS + dir * FILES_PER_DIR + file) as u64
}

/// `count` pure creates spread across the directories (new FRNs past the base).
fn creates(base_count: usize, count: usize) -> Vec<FileChange> {
    (0..count)
        .map(|idx| FileChange {
            frs: ((base_count + idx) as u64).into(),
            parent_frs: ((idx % NUM_DIRS) as u64 + 1).into(),
            filename: format!("new{idx}.{}", ext_at(idx)),
            created: true,
            ..FileChange::default()
        })
        .collect()
}

/// `count` deletes of existing base file records (the tombstone path).
fn deletes(count: usize) -> Vec<FileChange> {
    (0..count)
        .map(|idx| FileChange {
            frs: file_frs(idx % NUM_DIRS, idx % FILES_PER_DIR).into(),
            deleted: true,
            ..FileChange::default()
        })
        .collect()
}

/// `count` interleaved create / delete / file-rename changes — the realistic
/// installer/extract shape that exercises every apply branch in one batch.
fn mixed(base_count: usize, count: usize) -> Vec<FileChange> {
    (0..count)
        .map(|idx| {
            let dir = idx % NUM_DIRS;
            let file = idx % FILES_PER_DIR;
            match idx % 3 {
                0 => FileChange {
                    frs: ((base_count + idx) as u64).into(),
                    parent_frs: (dir as u64 + 1).into(),
                    filename: format!("add{idx}.{}", ext_at(idx)),
                    created: true,
                    ..FileChange::default()
                },
                1 => FileChange {
                    frs: file_frs(dir, file).into(),
                    deleted: true,
                    ..FileChange::default()
                },
                _ => FileChange {
                    frs: file_frs(dir, file).into(),
                    parent_frs: (dir as u64 + 1).into(),
                    filename: format!("renamed{idx}.{}", ext_at(idx)),
                    renamed: true,
                    ..FileChange::default()
                },
            }
        })
        .collect()
}

/// Time `apply_usn_patch` for each batch profile against a fresh fixture clone.
fn bench_apply(crit: &mut Criterion) {
    let base = build_drive();
    let base_count = base.records.len();

    let profiles: [(&str, Vec<FileChange>); 4] = [
        ("creates/256", creates(base_count, 256)),
        ("creates/4000", creates(base_count, 4_000)),
        ("mixed/4000", mixed(base_count, 4_000)),
        ("deletes/4000", deletes(4_000)),
    ];

    let mut group = crit.benchmark_group("apply_cost");
    for (name, batch) in &profiles {
        group.bench_function(*name, |bencher| {
            bencher.iter_batched(
                || base.clone(),
                |mut drive| black_box(apply_usn_patch(&mut drive, black_box(batch))),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_apply);
criterion_main!(benches);
