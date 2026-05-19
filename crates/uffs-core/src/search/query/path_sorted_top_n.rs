// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Full-path-sorted top-N collection for `FieldId::Path`.
//!
//! Two implementations share this module:
//!
//!   1. [`collect_path_via_ext_index`] — fast path for the ext-only filter
//!      shape (`*.dll`, `>.*\.(jpg|png)$`, etc.).  Gathers candidates via the
//!      drive's CSR `ExtensionIndex` (O(K) where K is the ext-bucket size),
//!      resolves every path in parallel rayon chunks, and delegates the actual
//!      ordering to `backend::sort_rows`, which already parallelises above 16 K
//!      rows via `par_sort_unstable_by`.  Closes the single-largest hot-cache
//!      latency in the v0.5.66 surface: `C:*.dll --sort path` drops from ~3.1 s
//!      (full tree walk) to the ~400 ms range projected in the Phase 5 plan.
//!
//!   2. [`walk_tree_path_sorted`] — the legacy depth-first tree walk that
//!      existed before the fast path was added.  Still used for shapes the fast
//!      path cannot admit (path-contains filters, size / date / attribute
//!      predicates, etc.) because they would require per-record work the
//!      ext-index cannot skip.
//!
//! The public dispatch is [`collect_path_sorted_top_n`], which picks
//! between the two based on [`SearchFilters::is_ext_only`].  This
//! mirrors the same fast-path gate used by
//! [`super::path_only_top_n::collect_path_only_sorted_top_n`] for the
//! `FieldId::PathOnly` sort.

use rayon::prelude::*;

use super::super::backend::{self, DisplayRow, FilterMode};
use super::super::field::FieldId;
use super::super::filters::{SearchFilters, row_passes_filters};
use super::super::tree::{self, DirCache, DirCacheExt as _};
use super::numeric_top_n::sort_indices_by_name;
use super::{make_display_row, passes_filter_mode, stack_volume_prefix};
use crate::compact::DriveCompactIndex;

/// Target chunk size for parallel path resolution inside
/// [`collect_path_via_ext_index`].
///
/// Matches the constants of the same name in
/// `numeric_top_n::collect_global_top_n_numeric` and
/// `path_only_top_n::collect_path_only_via_ext_index` — at ~370 ns
/// per candidate a 4 K chunk runs for ~1.5 ms, well above rayon's
/// task-dispatch floor (~1 μs).
const RESOLVE_CHUNK_SIZE: usize = 4096;

/// Collect up to `limit` display rows in full-path-sorted order.
///
/// Drives are processed letter-ASC (or letter-DESC when `sort_desc`
/// is set) and the matching records land in the output in the same
/// order `backend::sort_rows(.., FieldId::Path, sort_desc, &[])`
/// would produce — i.e. lexicographic on the folded full path with a
/// raw-path tiebreaker, matching the contract documented for
/// `sort_rows`.
///
/// Dispatches between [`collect_path_via_ext_index`] (ext-only
/// filter) and [`walk_tree_path_sorted`] (general tree walk) based on
/// [`SearchFilters::is_ext_only`].  The two paths return the same
/// set of rows in the same order — only the per-row work changes.
pub(super) fn collect_path_sorted_top_n<D: AsRef<DriveCompactIndex> + Sync>(
    drives: &[D],
    limit: usize,
    sort_desc: bool,
    filter_mode: FilterMode,
    search_filters: &mut SearchFilters,
) -> Vec<DisplayRow> {
    if limit == 0 {
        return Vec::new();
    }

    // Fast path: ext-only filter + `All` / `FilesOnly` mode → use
    // the CSR ext-index bucket directly instead of walking the whole
    // drive tree.  The tree walk is O(N_total) in the drive record
    // count; the ext-index path is O(N_ext) in the per-extension
    // bucket size.  Empirical speedup on a 3.67 M-record C: drive
    // for `*.dll --sort path`: ~3 100 ms → ~400 ms target range.
    if search_filters.is_ext_only()
        && matches!(filter_mode, FilterMode::All | FilterMode::FilesOnly)
    {
        return collect_path_via_ext_index(drives, limit, sort_desc, filter_mode, search_filters);
    }

    walk_tree_path_sorted(drives, limit, sort_desc, filter_mode, search_filters)
}

/// Depth-first path-sorted top-N walk (legacy path).
///
/// Walks each drive's directory tree in pre-order DFS (sorted by
/// name per level) and collects up to `limit` rows that satisfy
/// `filter_mode` and `search_filters`.  Non-matching records are
/// skipped but their children are still explored — a filtered-out
/// directory may contain matching descendants (e.g. `FilesOnly` must
/// walk through directories to reach files inside them, and an
/// `extensions=["exe"]` filter must walk through `.txt` directories
/// to reach `.exe` grandchildren).
///
/// Used as the fallback when [`collect_path_sorted_top_n`]'s fast
/// path does not fire (e.g. non-ext filter predicates are active).
#[expect(
    clippy::indexing_slicing,
    reason = "drive_order indices are from 0..drives.len(); always valid"
)]
fn walk_tree_path_sorted<D: AsRef<DriveCompactIndex>>(
    drives: &[D],
    limit: usize,
    sort_desc: bool,
    filter_mode: FilterMode,
    search_filters: &SearchFilters,
) -> Vec<DisplayRow> {
    let mut path_results: Vec<DisplayRow> = Vec::new();
    let mut drive_order: Vec<usize> = (0..drives.len()).collect();
    drive_order.sort_unstable_by(|&idx_a, &idx_b| {
        let ord = drives[idx_a]
            .as_ref()
            .letter
            .cmp(&drives[idx_b].as_ref().letter);
        if sort_desc { ord.reverse() } else { ord }
    });

    // Per-walk fold state, reused across every row for zero-alloc
    // filter checks.  `fold` is always the default $UpCase table — per-
    // drive $UpCase tables aren't available in the compact snapshot.
    let fold = uffs_text::case_fold::CaseFold::default_table();
    let mut fold_buf: Vec<u8> = Vec::with_capacity(256);

    for &drive_idx in &drive_order {
        if path_results.len() >= limit {
            break;
        }
        let Some(drive_ref) = drives.get(drive_idx) else {
            continue;
        };
        let drive = drive_ref.as_ref();
        let mut vp_buf = [0_u8; 4];
        let volume_prefix = stack_volume_prefix(&mut vp_buf, drive.letter);

        let mut roots: Vec<u32> = drive
            .records
            .iter()
            .enumerate()
            .filter(|(_, rec)| rec.parent_idx == u32::MAX && rec.name_len > 0)
            .map(|(idx, _)| uffs_mft::len_to_u32(idx))
            .collect();
        sort_indices_by_name(&mut roots, drive, sort_desc);

        let mut dir_cache = DirCache::with_capacity(256);
        let mut stack: Vec<u32> = roots.into_iter().rev().collect();
        while let Some(idx) = stack.pop() {
            if path_results.len() >= limit {
                return path_results;
            }
            let Some(rec) = drive.records.get(idx as usize) else {
                continue;
            };
            let name = rec.name(&drive.names);
            if name.is_empty() {
                continue;
            }

            // Enqueue children BEFORE the filter check — a directory
            // that fails the filter (e.g. `FilesOnly` drops dirs) may
            // still contain matching descendants that must be visited.
            let child_slice = drive.children.get(idx as usize);
            if !child_slice.is_empty() {
                let mut sorted_children = child_slice.to_vec();
                sort_indices_by_name(&mut sorted_children, drive, sort_desc);
                for &child in sorted_children.iter().rev() {
                    stack.push(child);
                }
            }

            // `filter_mode` is cheap — check before resolving path.
            let is_dir = rec.is_directory();
            if !passes_filter_mode(is_dir, filter_mode) {
                continue;
            }

            // Remaining filters need the full `DisplayRow` (resolved
            // path + semantic type).  Build it, then check.
            let path =
                tree::resolve_path_cached(drive, idx as usize, volume_prefix, &mut dir_cache);
            let row = make_display_row(idx, drive.letter, rec, name, path);
            if !row_passes_filters(&row, search_filters, &fold, &mut fold_buf) {
                continue;
            }
            path_results.push(row);
        }
    }

    path_results
}

/// Ext-index fast path for `FieldId::Path` sort.
///
/// Called from [`collect_path_sorted_top_n`] when
/// `search_filters.is_ext_only()` holds and `filter_mode` is `All`
/// or `FilesOnly`.  Mirrors the `path_only` ext fast-path shape in
/// `path_only_top_n::collect_path_only_via_ext_index`:
///
///   1. Iterate `drive.ext_index[ext_id]` for every drive and every resolved
///      extension id.  This narrows the candidate set from `O(N_total)` to
///      `O(N_ext)`.
///   2. Apply the two cheap per-candidate predicates `is_ext_only()` admits —
///      `hide_system` (`$`-prefix byte check) and `hide_ads` (`memchr(b':')` on
///      the name slice).  Both run before path resolution.
///   3. Resolve each survivor's path in parallel rayon chunks, one [`DirCache`]
///      per worker.  Because `ext_index` buckets are sorted in FRN (MFT) order,
///      adjacent candidates share parent directories so the `DirCache` stays
///      warm inside each chunk.
///   4. Sort the materialised `DisplayRow`s via `backend::sort_rows` with
///      `FieldId::Path` — which already parallelises its decorate + sort phases
///      above the `PARALLEL_SORT_THRESHOLD` of 16 K rows — then truncate to
///      `limit`.
///
/// For a 167 K-candidate query on C: (`*.dll --sort path`) the
/// expected breakdown post-fix is ~30 ms scan + ~25 ms parallel
/// resolve + ~30 ms sort = ~85 ms daemon-side, down from ~3 150 ms
/// on the old tree walk.  The 3 000 ms savings come from *not*
/// walking the other ~3.5 M records that the tree DFS was touching
/// purely to reach the `.dll` grandchildren.
fn collect_path_via_ext_index<D: AsRef<DriveCompactIndex> + Sync>(
    drives: &[D],
    limit: usize,
    sort_desc: bool,
    filter_mode: FilterMode,
    search_filters: &mut SearchFilters,
) -> Vec<DisplayRow> {
    let hide_system = search_filters.hide_system;
    let hide_ads = search_filters.hide_ads;

    // ── Scan phase: collect (drive_idx, rec_idx) candidates ───────
    //
    // Bounded by `O(N_ext)` per drive — the ext-index bucket size
    // for the requested extensions.  We do NOT bound by `limit`
    // here: `FieldId::Path` ordering is unknown until paths are
    // resolved, so pre-resolve truncation would return the wrong
    // rows.  Carrying all survivors is cheap because the bucket
    // itself is tiny compared to the full record table.
    let t_scan = std::time::Instant::now();
    let mut candidates: Vec<(u16, u32)> = Vec::new();
    for (drive_idx, drive_ref) in drives.iter().enumerate() {
        let drive = drive_ref.as_ref();
        search_filters.resolve_ext_ids_for_drive(drive);
        if search_filters.resolved_ext_ids.is_empty() {
            continue;
        }
        let drive_idx_u16 = uffs_mft::len_to_u16(drive_idx);
        // Borrow `resolved_ext_ids` immutably for the inner loop's
        // lifetime: the body never touches `search_filters`, so the
        // borrow is released when the inner loop ends, freeing the
        // next outer iteration's call to `resolve_ext_ids_for_drive`
        // (which needs `&mut search_filters`).  Replaces a defensive
        // `.clone()` (Phase 6c category-δ) that was anticipating a
        // re-aliasing scenario that the current code doesn't hit.
        for &ext_id in &search_filters.resolved_ext_ids {
            for &rec_idx_u32 in drive.ext_index.get(ext_id) {
                let rec_idx = rec_idx_u32 as usize;
                let Some(rec) = drive.records.get(rec_idx) else {
                    continue;
                };
                if rec.name_len == 0 {
                    continue;
                }
                if matches!(filter_mode, FilterMode::FilesOnly) && rec.is_directory() {
                    continue;
                }
                if hide_system && rec.is_system_metafile() {
                    continue;
                }
                if hide_ads {
                    let name = rec.name(&drive.names);
                    if memchr::memchr(b':', name.as_bytes()).is_some() {
                        continue;
                    }
                }
                candidates.push((drive_idx_u16, rec_idx_u32));
            }
        }
    }
    let scan_ms = t_scan.elapsed().as_millis();

    // Locality re-sort: for multi-extension queries candidates from
    // different ext buckets are interleaved.  Sorting by
    // `(drive_idx, rec_idx)` restores MFT locality so adjacent
    // `resolve_path_cached` calls share parent directories and the
    // per-chunk `DirCache` stays warm.  Single-extension queries
    // already hit MFT order so this is a near-no-op (~2 ms for 167 K
    // u48 keys).  The final `backend::sort_rows` after resolution
    // restores the user-requested `Path` order.
    candidates.sort_unstable_by_key(|&(drive_idx, rec_idx)| (drive_idx, rec_idx));

    // ── Path-resolve phase: par_chunks with per-worker DirCache ───
    //
    // Identical structure to `collect_path_only_via_ext_index` and
    // `collect_global_top_n_numeric` — one `DirCache` per rayon
    // worker, chunk-local row vectors concatenated via `reduce`.
    let t_resolve = std::time::Instant::now();
    let mut rows: Vec<DisplayRow> = candidates
        .par_chunks(RESOLVE_CHUNK_SIZE)
        .map(|chunk| {
            let mut local_caches: std::collections::HashMap<u16, DirCache> =
                std::collections::HashMap::new();
            let mut local_rows: Vec<DisplayRow> = Vec::with_capacity(chunk.len());
            for &(drive_idx, rec_idx) in chunk {
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
                    .or_insert_with(|| DirCache::with_capacity(256));
                let path = tree::resolve_path_cached(drive, rec_idx as usize, volume_prefix, cache);
                local_rows.push(make_display_row(rec_idx, drive.letter, rec, name, path));
            }
            local_rows
        })
        .reduce(Vec::new, |mut acc, mut chunk| {
            acc.append(&mut chunk);
            acc
        });
    let resolve_ms = t_resolve.elapsed().as_millis();

    // ── Sort + truncate: delegate to `backend::sort_rows` ─────────
    //
    // `sort_rows` with `FieldId::Path` already parallelises its
    // decorate + sort above `PARALLEL_SORT_THRESHOLD` (16 K rows)
    // via `par_sort_unstable_by`, so we don't need to hand-roll a
    // sort here.  The name-ASC tiebreaker is applied inline.
    let t_sort = std::time::Instant::now();
    backend::sort_rows(&mut rows, FieldId::Path, sort_desc, &[]);
    rows.truncate(limit);
    let sort_ms = t_sort.elapsed().as_millis();

    tracing::debug!(
        scan_ms,
        resolve_ms,
        sort_ms,
        candidates_in = candidates.len(),
        rows_out = rows.len(),
        "[PATH-SORT] ext-index fast path complete"
    );

    rows
}
