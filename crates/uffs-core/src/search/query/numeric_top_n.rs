// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Internal numeric top-N collection and sort helpers.

use alloc::collections::BinaryHeap;

use rayon::prelude::*;

use super::super::backend::{self, DisplayRow, FilterMode, PhaseTimings};
use super::super::derived::bulkiness_for_record;
use super::super::field::FieldId;
use super::super::filters::SearchFilters;
use super::super::tree::{self, DirCacheExt as _};
use super::{HeapEntry, heap_push_capped, make_display_row, stack_volume_prefix};
use crate::compact::{CompactRecord, DriveCompactIndex};

/// Target chunk size for parallel path resolution inside
/// [`collect_global_top_n_numeric`].
///
/// Chosen so each chunk runs for ~1.5 ms of CPU work, well above
/// rayon's per-task dispatch floor (~100 ns to 1 μs).  Smaller chunks
/// would waste time on scheduler overhead; larger chunks would
/// underutilise worker threads on smaller queries.  Measured at
/// ~370 ns per candidate a 4 K chunk runs in ~1.5 ms.
const RESOLVE_CHUNK_SIZE: usize = 4096;

/// Extract the numeric sort key for `rec` under `sort_column`.
///
/// Pure function of the record + column — no shared mutable state —
/// so it is trivially `Sync` and safe to call from every rayon worker
/// inside the per-drive scan.  Moved out of the scan closure so the
/// drive loop can be parallelised without duplicating the 100-line
/// `match` across each branch.
fn extract_sort_key(rec: &CompactRecord, sort_column: FieldId, drive: &DriveCompactIndex) -> i64 {
    let drive_fold = drive.fold;
    // All `u64 -> i64` conversions below use `u64::cast_signed` (stable
    // since Rust 1.87) to document the exact-bit-pattern reinterpret
    // without needing a `cast_possible_wrap` expect.  Real NTFS file /
    // tree sizes never approach `i64::MAX`, so the high-bit flip is
    // unreachable in practice.
    match sort_column {
        FieldId::Size => rec.size.cast_signed(),
        FieldId::SizeOnDisk => rec.allocated.cast_signed(),
        FieldId::Created => rec.created,
        FieldId::Accessed => rec.accessed,
        FieldId::Descendants => i64::from(rec.descendants),
        FieldId::TreeAllocated => {
            if rec.is_directory() {
                rec.tree_allocated.cast_signed()
            } else {
                rec.allocated.cast_signed()
            }
        }
        FieldId::Bulkiness => bulkiness_for_record(rec).cast_signed(),
        FieldId::Extension | FieldId::Type => i64::from(rec.extension_id),
        FieldId::Name => {
            let name = rec.name(&drive.names);
            let mut key = [0_u8; 8];
            for (dst, ch) in key.iter_mut().zip(name.chars()) {
                let folded = drive_fold.fold_char(ch);
                // Sort-key prefix: the low byte of the folded u16 is the
                // canonical 8-byte name-prefix; `to_be_bytes()[1]` is the
                // lint-free way to take it (vs `folded as u8` which would
                // trigger `clippy::cast_possible_truncation`).
                *dst = folded.to_be_bytes()[1];
            }
            i64::from_be_bytes(key)
        }
        FieldId::Drive => {
            let name = rec.name(&drive.names);
            let mut key = [0_u8; 8];
            key[0] = u8::try_from(u32::from(drive.letter.as_byte())).unwrap_or(b'?');
            for (dst, ch) in key[1..].iter_mut().zip(name.chars()) {
                let folded = drive_fold.fold_char(ch);
                *dst = folded.to_be_bytes()[1];
            }
            i64::from_be_bytes(key)
        }
        FieldId::TreeSize => {
            if rec.is_directory() {
                rec.treesize.cast_signed()
            } else {
                rec.size.cast_signed()
            }
        }
        // Boolean attribute flags: extract the individual bit as 0/1.
        FieldId::DirectoryFlag => i64::from(rec.is_directory()),
        FieldId::Hidden => i64::from(rec.flags & 0x0002 != 0),
        FieldId::System => i64::from(rec.flags & 0x0004 != 0),
        FieldId::ReadOnly => i64::from(rec.flags & 0x0001 != 0),
        FieldId::Archive => i64::from(rec.flags & 0x0020 != 0),
        FieldId::Compressed => i64::from(rec.flags & 0x0800 != 0),
        FieldId::Encrypted => i64::from(rec.flags & 0x4000 != 0),
        FieldId::Sparse => i64::from(rec.flags & 0x0200 != 0),
        FieldId::Reparse => i64::from(rec.flags & 0x0400 != 0),
        FieldId::Offline => i64::from(rec.flags & 0x1000 != 0),
        FieldId::NotIndexed => i64::from(rec.flags & 0x2000 != 0),
        FieldId::Temporary => i64::from(rec.flags & 0x0100 != 0),
        FieldId::Integrity => i64::from(rec.flags & 0x8000 != 0),
        FieldId::NoScrub => i64::from(rec.flags & 0x0002_0000 != 0),
        FieldId::Pinned => i64::from(rec.flags & 0x0008_0000 != 0),
        FieldId::Unpinned => i64::from(rec.flags & 0x0010_0000 != 0),
        FieldId::RecallOnOpen => i64::from(rec.flags & 0x0004_0000 != 0),
        FieldId::RecallOnDataAccess => i64::from(rec.flags & 0x0040_0000 != 0),
        // Composite attribute fields use the raw flags value.
        FieldId::Attributes | FieldId::AttributeValue | FieldId::ParityAttributes => {
            i64::from(rec.flags)
        }
        FieldId::Virtual => i64::from(rec.flags & 0x0001_0000 != 0),
        // Modified is the default; Path/PathOnly handled by tree walk upstream.
        FieldId::Path | FieldId::PathOnly | FieldId::Modified => rec.modified,
        FieldId::NameLength => {
            i64::try_from(rec.name(&drive.names).chars().count()).unwrap_or(i64::MAX)
        }
        FieldId::PathLength => {
            // Use name length as a proxy at the sort-key stage
            // (full path unavailable here).
            i64::try_from(rec.name(&drive.names).chars().count()).unwrap_or(i64::MAX)
        }
    }
}

/// Per-drive candidate output: a `(rec_tuples, filtered_count)` pair.
///
/// Factored into a type alias so the `par_iter().map().collect::<Vec<...>>()`
/// inside [`collect_global_top_n_numeric`] stays readable without
/// violating clippy's `type_complexity` lint.
type DriveCandidates = (Vec<(u16, u32, i64)>, u64);

/// Per-drive scan parameters threaded into [`scan_drive_candidates`].
///
/// Collecting the per-call constants into one struct keeps the
/// per-drive worker signature readable and lets clippy's
/// `too_many_arguments` lint stay happy.  All fields are `Copy` so
/// the struct moves in a single 24-byte copy per drive.
#[derive(Debug, Clone, Copy)]
struct DriveScanCfg {
    /// Maximum number of candidates to retain across all drives.
    limit: usize,
    /// Whether to use a bounded `BinaryHeap` (true when `limit < 1 M`)
    /// or a simple `Vec` fallback (true when `limit` is effectively
    /// unbounded).  Chosen by the caller so every drive-worker makes
    /// the same structural decision.
    use_heap: bool,
    /// Sort column — identifies which `CompactRecord` field feeds
    /// [`extract_sort_key`].
    sort_column: FieldId,
    /// Descending-order flag.  `true` = Modified-DESC / Size-DESC /
    /// etc.; `false` = the matching ascending variant.
    sort_desc: bool,
    /// User's `--files-only` / `--dirs-only` / default filter mode.
    filter_mode: FilterMode,
    /// Short-circuit flag: when `false`, the scan skips
    /// `matches_record` entirely and pushes every non-empty-name
    /// record.  Corresponds to `search_filters.is_empty() &&
    /// filter_mode == All` at the caller.
    has_filters: bool,
}

/// Per-drive top-N working state.
///
/// Owns the bounded `BinaryHeap` variants plus the unbounded fallback
/// `Vec` so [`scan_drive_candidates`]'s three scan branches can all
/// call `push` / `drain` without re-implementing the heap dispatch.
/// This keeps each scan branch's cognitive complexity low enough that
/// the clippy `cognitive_complexity` lint never fires, so no
/// `#[expect]` / `#[allow]` suppression is needed.
struct DriveTopN {
    /// Min-heap of `Reverse<HeapEntry>` — used when `sort_desc` is
    /// `true`, keeps the top-`limit` largest sort-keys.
    heap_desc: BinaryHeap<core::cmp::Reverse<HeapEntry>>,
    /// Max-heap of `HeapEntry` — used when `sort_desc` is `false`,
    /// keeps the top-`limit` smallest sort-keys.
    heap_asc: BinaryHeap<HeapEntry>,
    /// Unbounded `Vec` used when `use_heap` is `false` (limit ≥ 1 M).
    /// The caller sorts + truncates once all drives have merged.
    fallback: Vec<(u16, u32, i64)>,
    /// `true` if either heap is active; `false` uses `fallback`.
    use_heap: bool,
    /// `true` selects `heap_desc`; `false` selects `heap_asc`.
    sort_desc: bool,
    /// Bound on the active heap's capacity (`limit + 1`).
    limit: usize,
}

impl DriveTopN {
    /// Build a fresh top-N workspace sized for `cfg.limit`.
    fn new(cfg: DriveScanCfg) -> Self {
        let cap = cfg.limit.saturating_add(1);
        Self {
            heap_desc: if cfg.use_heap && cfg.sort_desc {
                BinaryHeap::with_capacity(cap)
            } else {
                BinaryHeap::new()
            },
            heap_asc: if cfg.use_heap && !cfg.sort_desc {
                BinaryHeap::with_capacity(cap)
            } else {
                BinaryHeap::new()
            },
            fallback: Vec::new(),
            use_heap: cfg.use_heap,
            sort_desc: cfg.sort_desc,
            limit: cfg.limit,
        }
    }

    /// Push `entry` into the active heap / fallback, respecting the
    /// bounded-heap invariant.
    fn push(&mut self, entry: HeapEntry) {
        if self.use_heap {
            if self.sort_desc {
                heap_push_capped(&mut self.heap_desc, core::cmp::Reverse(entry), self.limit);
            } else {
                heap_push_capped(&mut self.heap_asc, entry, self.limit);
            }
        } else {
            self.fallback
                .push((entry.drive_idx, entry.rec_idx, entry.sort_key));
        }
    }

    /// Drain the workspace into the `(drive_idx, rec_idx, sort_key)`
    /// candidate tuples the caller merges across drives.
    fn drain(self) -> Vec<(u16, u32, i64)> {
        if !self.use_heap {
            return self.fallback;
        }
        if self.sort_desc {
            self.heap_desc
                .into_iter()
                .map(|rev| (rev.0.drive_idx, rev.0.rec_idx, rev.0.sort_key))
                .collect()
        } else {
            self.heap_asc
                .into_iter()
                .map(|he| (he.drive_idx, he.rec_idx, he.sort_key))
                .collect()
        }
    }
}

/// Build a `HeapEntry` for a single record.
///
/// Extracted so every scan branch builds entries identically and the
/// `sort_key` computation lives in exactly one place.
#[inline]
fn heap_entry_for(
    drive_idx: usize,
    rec_idx: usize,
    rec: &CompactRecord,
    drive: &DriveCompactIndex,
    sort_column: FieldId,
) -> HeapEntry {
    HeapEntry {
        sort_key: extract_sort_key(rec, sort_column, drive),
        drive_idx: uffs_mft::len_to_u16(drive_idx),
        rec_idx: uffs_mft::len_to_u32(rec_idx),
    }
}

/// Returns `true` if `rec` passes the two cheap per-candidate
/// predicates `is_ext_only()` admits (`hide_system`, `hide_ads`) plus
/// the `filter_mode` / `name_len` pre-checks.
///
/// Extracted so the ext-fast-path scan loop stays flat instead of
/// nesting four `if` / `continue` pairs that push clippy's
/// `cognitive_complexity` count above threshold.  All four checks
/// must run before `push` so filtered records never enter the heap.
#[inline]
fn ext_candidate_passes(
    rec: &CompactRecord,
    names: &[u8],
    filters: &SearchFilters,
    filter_mode: FilterMode,
) -> bool {
    if rec.name_len == 0 {
        return false;
    }
    if matches!(filter_mode, FilterMode::FilesOnly) && rec.is_directory() {
        return false;
    }
    if filters.hide_system && rec.is_system_metafile() {
        return false;
    }
    if filters.hide_ads {
        let name = rec.name(names);
        if memchr::memchr(b':', name.as_bytes()).is_some() {
            return false;
        }
    }
    true
}

/// Ext-index fast-path scan: iterate `drive.ext_index` for each
/// requested extension and push survivors into `state`.
///
/// The CSR `ext_index` bucket narrows the candidate set from `O(N)`
/// (all records on the drive) to `O(K)` (matches of the requested
/// extension).  Preconditions enforced by the caller:
///   * `filters.is_ext_only()` holds.
///   * `filters.resolved_ext_ids` is non-empty (otherwise use the short-circuit
///     branch).
///   * `filter_mode` is `All` or `FilesOnly`.
fn scan_ext_fast_path(
    drive_idx: usize,
    drive: &DriveCompactIndex,
    filters: &SearchFilters,
    cfg: DriveScanCfg,
    state: &mut DriveTopN,
) {
    for &ext_id in &filters.resolved_ext_ids {
        for &rec_idx_u32 in drive.ext_index.get(ext_id) {
            let rec_idx = rec_idx_u32 as usize;
            let Some(rec) = drive.records.get(rec_idx) else {
                continue;
            };
            if !ext_candidate_passes(rec, &drive.names, filters, cfg.filter_mode) {
                continue;
            }
            state.push(heap_entry_for(
                drive_idx,
                rec_idx,
                rec,
                drive,
                cfg.sort_column,
            ));
        }
    }
}

/// Returns `true` if `rec` passes the full-scan branch's per-record
/// predicates (`name_len`, `filter_mode`, `search_filters.matches_record`).
///
/// Returns `false` for a pure `filter_mode` miss (no counter bump) and
/// bumps `*filtered` when `matches_record` rejects — matches the
/// original inline accounting in the pre-refactor code.
#[inline]
fn full_scan_record_passes(
    rec: &CompactRecord,
    drive: &DriveCompactIndex,
    filters: &SearchFilters,
    filter_mode: FilterMode,
    has_filters: bool,
    fold_buf: &mut Vec<u8>,
    filtered: &mut u64,
) -> bool {
    if rec.name_len == 0 {
        return false;
    }
    if has_filters {
        match filter_mode {
            FilterMode::FilesOnly if rec.is_directory() => return false,
            FilterMode::DirsOnly if !rec.is_directory() => return false,
            FilterMode::All | FilterMode::FilesOnly | FilterMode::DirsOnly => {}
        }
        if !filters.matches_record(rec, &drive.names, fold_buf, drive.fold) {
            *filtered = filtered.saturating_add(1);
            return false;
        }
    }
    true
}

/// Full-scan branch: iterate every record on the drive and push
/// survivors into `state`.  Returns the count of records rejected by
/// `matches_record` for debug tracing — `filter_mode` misses are
/// silent (matches the pre-refactor accounting).
fn scan_full_records(
    drive_idx: usize,
    drive: &DriveCompactIndex,
    filters: &SearchFilters,
    cfg: DriveScanCfg,
    fold_buf: &mut Vec<u8>,
    state: &mut DriveTopN,
) -> u64 {
    let mut filtered = 0_u64;
    for (rec_idx, rec) in drive.records.iter().enumerate() {
        if !full_scan_record_passes(
            rec,
            drive,
            filters,
            cfg.filter_mode,
            cfg.has_filters,
            fold_buf,
            &mut filtered,
        ) {
            continue;
        }
        state.push(heap_entry_for(
            drive_idx,
            rec_idx,
            rec,
            drive,
            cfg.sort_column,
        ));
    }
    filtered
}

/// Scan one drive and collect up to `limit` top-N candidates.
///
/// Called by [`collect_global_top_n_numeric`] from a rayon worker, so
/// it must take an immutable `&SearchFilters`.  Callers clone the
/// outer filters once per drive and call `resolve_ext_ids_for_drive`
/// on the clone before handing it in here.
///
/// Each drive maintains its own bounded [`BinaryHeap`]
/// (`BinaryHeap::with_capacity(limit + 1)`) so the ~25 M-record
/// cross-drive scan never materialises into a single shared vector.
/// The per-drive heaps are drained into `(u16, u32, i64)` tuples and
/// merged by the caller, which then sorts + truncates to the global
/// top-`limit`.
///
/// Returns `(candidates, drive_filtered_count)`.  `drive_filtered_count`
/// is the number of records that failed `matches_record` — it's merged
/// into the aggregate filter-out count used only for debug tracing.
fn scan_drive_candidates(
    drive_idx: usize,
    drive: &DriveCompactIndex,
    search_filters: &SearchFilters,
    cfg: DriveScanCfg,
) -> DriveCandidates {
    let mut state = DriveTopN::new(cfg);
    let mut fold_buf: Vec<u8> = Vec::with_capacity(256);
    let mut drive_filtered = 0_u64;

    let ext_fast_path = search_filters.is_ext_only()
        && matches!(cfg.filter_mode, FilterMode::All | FilterMode::FilesOnly);

    if ext_fast_path && !search_filters.resolved_ext_ids.is_empty() {
        scan_ext_fast_path(drive_idx, drive, search_filters, cfg, &mut state);
    } else if ext_fast_path && !search_filters.extensions.is_empty() {
        // Short-circuit: `is_ext_only()` holds but none of the
        // requested extensions exist on this drive.  Skip the drive
        // entirely instead of falling through to an O(N) full scan
        // that `matches_record` would reject anyway.
        tracing::debug!(
            drive = %drive.letter,
            requested_extensions = ?search_filters.extensions,
            ext_name_count = drive.ext_names.len(),
            "ext fast-path SHORT-CIRCUIT — no matching extension IDs on this drive"
        );
    } else {
        drive_filtered = scan_full_records(
            drive_idx,
            drive,
            search_filters,
            cfg,
            &mut fold_buf,
            &mut state,
        );
    }

    (state.drain(), drive_filtered)
}

/// Sorts result indices by record name, using case-folded comparison.
pub(super) fn sort_indices_by_name(indices: &mut [u32], drive: &DriveCompactIndex, desc: bool) {
    let fold = drive.fold;
    indices.sort_unstable_by(|&idx_a, &idx_b| {
        let name_a = drive
            .records
            .get(idx_a as usize)
            .map_or("", |rec| rec.name(&drive.names));
        let name_b = drive
            .records
            .get(idx_b as usize)
            .map_or("", |rec| rec.name(&drive.names));
        let ord = fold.cmp_str(name_a, name_b);
        if desc { ord.reverse() } else { ord }
    });
}

/// Aggregate path-resolve deep-profile totals summed across rayon
/// workers inside [`resolve_candidates_to_rows`].
///
/// Kept as a named struct so the `reduce` accumulator stays readable
/// and callers can destructure the fields for `PhaseTimings` assembly
/// without a tuple-positional API.
struct ResolveStats {
    /// Materialised display rows in their path-resolved order.
    rows: Vec<DisplayRow>,
    /// Cumulative nanoseconds spent in `tree::resolve_path_cached`.
    resolve_fn_ns: u128,
    /// Cumulative nanoseconds spent in `make_display_row` + `Vec::push`.
    build_row_ns: u128,
    /// Count of candidates that reached the resolve loop.
    candidates: u64,
    /// Total `DirCache` entries across all per-worker caches at
    /// reduce time — the exact steady-state miss count.
    cache_entries: u64,
}

/// Run one rayon-worker chunk of the path-resolve phase.
///
/// Each chunk owns a per-drive `DirCache` map — siblings in the
/// chunk keep the cache warm because the caller sorted the
/// candidates by `(drive_idx, rec_idx)` first.  Splitting this off
/// keeps the outer `par_chunks().map().reduce()` shape flat and the
/// `collect_global_top_n_numeric` cognitive complexity low enough
/// that no clippy suppression is needed.
fn resolve_chunk<D: AsRef<DriveCompactIndex>>(
    drives: &[D],
    chunk: &[(u16, u32, i64)],
) -> ResolveStats {
    let mut local_caches: std::collections::HashMap<u16, tree::DirCache> =
        std::collections::HashMap::new();
    let mut rows: Vec<DisplayRow> = Vec::with_capacity(chunk.len());
    let mut resolve_fn_ns: u128 = 0;
    let mut build_row_ns: u128 = 0;
    let mut candidates: u64 = 0;

    for &(drive_idx, rec_idx, _) in chunk {
        let Some(drive_ref) = drives.get(drive_idx as usize) else {
            continue;
        };
        let drive = drive_ref.as_ref();
        let Some(rec) = drive.records.get(rec_idx as usize) else {
            continue;
        };
        let name = rec.name(&drive.names);
        if name.is_empty() {
            continue;
        }
        let mut vp_buf = [0_u8; 4];
        let volume_prefix = stack_volume_prefix(&mut vp_buf, drive.letter);
        let cache = local_caches
            .entry(drive_idx)
            .or_insert_with(|| tree::DirCache::with_capacity(256));
        let t_resolve = std::time::Instant::now();
        let path = tree::resolve_path_cached(drive, rec_idx as usize, volume_prefix, cache);
        resolve_fn_ns += t_resolve.elapsed().as_nanos();
        let t_build = std::time::Instant::now();
        rows.push(make_display_row(rec_idx, drive.letter, rec, name, path));
        build_row_ns += t_build.elapsed().as_nanos();
        candidates += 1;
    }

    let cache_entries: u64 = local_caches.values().map(|cache| cache.len() as u64).sum();
    ResolveStats {
        rows,
        resolve_fn_ns,
        build_row_ns,
        candidates,
        cache_entries,
    }
}

/// Parallel path-resolve: turn `(drive_idx, rec_idx, sort_key)` tuples
/// into fully-materialised `DisplayRow`s across rayon worker chunks.
///
/// The caller must pre-sort `candidates` by `(drive_idx, rec_idx)`
/// (MFT locality) so each chunk's per-worker `DirCache` stays warm.
/// This function is size- and order-agnostic: it always processes
/// `candidates` in the order given and reduces worker outputs in the
/// same order, matching the pre-refactor behaviour exactly.
fn resolve_candidates_to_rows<D: AsRef<DriveCompactIndex> + Sync>(
    drives: &[D],
    candidates: &[(u16, u32, i64)],
) -> ResolveStats {
    candidates
        .par_chunks(RESOLVE_CHUNK_SIZE)
        .map(|chunk| resolve_chunk(drives, chunk))
        .reduce(
            || ResolveStats {
                rows: Vec::new(),
                resolve_fn_ns: 0,
                build_row_ns: 0,
                candidates: 0,
                cache_entries: 0,
            },
            |mut acc, mut part| {
                acc.rows.append(&mut part.rows);
                acc.resolve_fn_ns += part.resolve_fn_ns;
                acc.build_row_ns += part.build_row_ns;
                acc.candidates += part.candidates;
                acc.cache_entries += part.cache_entries;
                acc
            },
        )
}

/// Fan the per-drive scan across rayon workers and return the
/// merged `(drive_filtered_sum, candidate_tuples)` pair.
///
/// Extracted so the outer entry point stays flat: all of the
/// per-worker tracing, filter-cloning, and heap-merging logic lives
/// in one named function instead of nesting inside the phase loop.
fn scan_all_drives_parallel<D: AsRef<DriveCompactIndex> + Sync>(
    drives: &[D],
    search_filters: &SearchFilters,
    cfg: DriveScanCfg,
) -> (u64, Vec<(u16, u32, i64)>) {
    let has_filters = cfg.has_filters;
    let per_drive: Vec<DriveCandidates> = drives
        .par_iter()
        .enumerate()
        .map(|(drive_idx, drive_ref)| {
            let drive = drive_ref.as_ref();
            let t_drive = std::time::Instant::now();
            // Per-worker clone so `resolve_ext_ids_for_drive` writes
            // into a local copy, never a shared `&mut`.  See struct
            // docs on `SearchFilters` for the clone-cost rationale.
            let mut local_filters = search_filters.clone();
            local_filters.resolve_ext_ids_for_drive(drive);
            let (cands, drive_filtered) =
                scan_drive_candidates(drive_idx, drive, &local_filters, cfg);
            tracing::debug!(
                drive = %drive.letter,
                records = drive.records.len(),
                filtered_out = drive_filtered,
                has_filters,
                elapsed_ms = t_drive.elapsed().as_millis(),
                "[SCAN] drive scan complete"
            );
            (cands, drive_filtered)
        })
        .collect();
    let total_filtered: u64 = per_drive.iter().map(|(_, filtered)| *filtered).sum();
    let total_candidates: usize = per_drive.iter().map(|(cands, _)| cands.len()).sum();
    let mut candidates: Vec<(u16, u32, i64)> = Vec::with_capacity(total_candidates);
    for (drive_cands, _) in per_drive {
        candidates.extend(drive_cands);
    }
    (total_filtered, candidates)
}

/// Sort merged candidates by `sort_key`, truncate to `limit`, then
/// re-sort by `(drive_idx, rec_idx)` for MFT locality during the
/// downstream path-resolve phase.
///
/// The numeric sort is the user-visible ordering; the locality
/// re-sort is an internal optimisation that `backend::sort_rows`
/// reverses after resolution using the user's requested column + tiebreakers.
///
/// Measured on a 1 M-record C: drive with `*.dll --sort modified`:
/// `path_resolve_ms` drops from ~226 ms to well under 100 ms because
/// the per-directory `DirCache` entry is reused across all `.dll`
/// siblings of `System32\` etc.
fn sort_and_localise(
    mut candidates: Vec<(u16, u32, i64)>,
    sort_desc: bool,
    limit: usize,
) -> Vec<(u16, u32, i64)> {
    if sort_desc {
        candidates.sort_unstable_by_key(|entry| core::cmp::Reverse(entry.2));
    } else {
        candidates.sort_unstable_by_key(|entry| entry.2);
    }
    candidates.truncate(limit);
    candidates.sort_unstable_by_key(|&(drive_idx, rec_idx, _)| (drive_idx, rec_idx));
    candidates
}

/// Core numeric sort + collect logic for all non-path-sorted fields.
///
/// Extracted from `collect_global_top_n` because inlining 300 lines of sort-key
/// extraction + heap management would make the dispatch function unreadable.
///
/// # Parallel per-drive scan
///
/// The scan phase fans out across drives with `rayon::par_iter` so each
/// drive's linear record loop runs on its own worker.  Every worker
/// maintains a private bounded [`BinaryHeap`] (`limit + 1` capacity)
/// inside [`scan_drive_candidates`]; the outer merge drains the
/// per-drive heaps into a single `Vec` and re-sorts + truncates to the
/// global top-`limit`.  For the default 7-drive / 25 M-record layout
/// this drops the `* --limit 100 --hide-system --hide-ads` scan from
/// ~1 080 ms (sequential drive loop) to ~150-200 ms on a 12-core host
/// — the single largest hot-path regression identified in the v0.5.66
/// cross-tool benchmark.
///
/// `search_filters` is cloned once per drive so each rayon worker can
/// call `resolve_ext_ids_for_drive` on its own copy without sharing a
/// `&mut` reference.  The clone is a `Vec<String>` + `Vec<u16>` —
/// trivially cheap against the per-drive scan work it unblocks.  The
/// outer `search_filters` is therefore read-only here; the caller
/// continues to hold a `&mut` reference only because downstream
/// display-row filters need it.
pub(super) fn collect_global_top_n_numeric<D: AsRef<DriveCompactIndex> + Sync>(
    drives: &[D],
    limit: usize,
    sort_column: FieldId,
    sort_desc: bool,
    filter_mode: FilterMode,
    search_filters: &SearchFilters,
) -> (Vec<DisplayRow>, PhaseTimings) {
    let has_filters = !search_filters.is_empty() || !matches!(filter_mode, FilterMode::All);
    tracing::debug!(
        has_filters,
        hide_system = search_filters.hide_system,
        filters_empty = search_filters.is_empty(),
        filter_mode = ?filter_mode,
        limit,
        sort_column = ?sort_column,
        sort_desc,
        num_drives = drives.len(),
        "[TOP-N] entering collect_global_top_n_numeric"
    );

    // For bounded queries use a BinaryHeap capped at `limit` — O(N log K)
    // instead of O(N log N).  For "unlimited" (limit >= 1M or usize::MAX)
    // fall back to collect-sort-truncate since a heap that large is wasteful.
    let use_heap = limit < 1_000_000;
    let cfg = DriveScanCfg {
        limit,
        use_heap,
        sort_column,
        sort_desc,
        filter_mode,
        has_filters,
    };

    // ── Parallel per-drive scan ────────────────────────────────
    let t_scan_all = std::time::Instant::now();
    let (total_filtered_out, raw_candidates) =
        scan_all_drives_parallel(drives, search_filters, cfg);
    let scan_ms = u64::try_from(t_scan_all.elapsed().as_millis()).unwrap_or(u64::MAX);
    let total_records_scanned: u64 = drives
        .iter()
        .map(|drive_ref| drive_ref.as_ref().records.len() as u64)
        .sum();
    tracing::debug!(
        total_records = total_records_scanned,
        total_filtered = total_filtered_out,
        scan_ms,
        merged_size = raw_candidates.len(),
        use_heap,
        "[SCAN] all drives scanned (parallel)"
    );

    // ── Sort phase: sort by key, truncate to limit, MFT-locality re-sort ──
    let t_sort = std::time::Instant::now();
    let sorted_candidates = sort_and_localise(raw_candidates, sort_desc, limit);
    let sort_ms = u64::try_from(t_sort.elapsed().as_millis()).unwrap_or(u64::MAX);

    // ── Path-resolve phase (parallel): `par_chunks` over candidates
    //    with one `DirCache` per rayon worker.  Candidates are in
    //    MFT order (from `sort_and_localise`'s locality re-sort), so
    //    adjacent items in each chunk share parents and hit the
    //    per-chunk cache warm.  Measured 4× speedup on 168 K candidates
    //    across 8 workers (63 ms sequential → 17 ms parallel).
    let t_path_resolve = std::time::Instant::now();
    let stats = resolve_candidates_to_rows(drives, &sorted_candidates);
    let path_resolve_ms = u64::try_from(t_path_resolve.elapsed().as_millis()).unwrap_or(u64::MAX);

    let mut rows = stats.rows;
    backend::sort_rows(&mut rows, sort_column, sort_desc, &[]);

    let timings = PhaseTimings {
        scan_ms,
        sort_ms,
        path_resolve_ms,
        path_candidates: stats.candidates,
        path_cache_entries: stats.cache_entries,
        path_resolve_fn_ns: u64::try_from(stats.resolve_fn_ns).unwrap_or(u64::MAX),
        path_build_row_ns: u64::try_from(stats.build_row_ns).unwrap_or(u64::MAX),
    };
    tracing::debug!(
        scan_ms,
        sort_ms,
        path_resolve_ms,
        path_candidates = timings.path_candidates,
        path_cache_entries = timings.path_cache_entries,
        path_resolve_fn_ns = timings.path_resolve_fn_ns,
        path_build_row_ns = timings.path_build_row_ns,
        rows = rows.len(),
        "[PHASE] collect_global_top_n_numeric complete"
    );
    (rows, timings)
}
