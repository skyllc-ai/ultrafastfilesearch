// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Search functions for compact-index drives.
//!
//! Per-drive search (trigram, regex, tree) and global top-N collection
//! for match-all queries. Called by `MultiDriveBackend::search()`.

mod numeric_top_n;
mod path_only_top_n;
mod path_sorted_top_n;

use alloc::collections::BinaryHeap;
use std::sync::LazyLock;

use numeric_top_n::collect_global_top_n_numeric;
use path_only_top_n::collect_path_only_sorted_top_n;
use path_sorted_top_n::collect_path_sorted_top_n;

use super::backend::{DisplayRow, FilterMode, PhaseTimings};
use super::field::FieldId;
use super::filters::SearchFilters;
use crate::compact::{CompactRecord, DriveCompactIndex};
use crate::search::tree;

/// Whether cache profiling is enabled (`UFFS_CACHE_PROFILE` env var).
///
/// Read once at first access to avoid a syscall per search.
static CACHE_PROFILE: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("UFFS_CACHE_PROFILE").is_some());

/// Entry for the top-N binary heap used by `collect_global_top_n_numeric`.
#[derive(Eq, PartialEq)]
pub(super) struct HeapEntry {
    /// Sort key used for ordering.
    pub(super) sort_key: i64,
    /// Drive index.
    pub(super) drive_idx: u16,
    /// Record index within the drive.
    pub(super) rec_idx: u32,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.sort_key
            .cmp(&other.sort_key)
            .then_with(|| self.drive_idx.cmp(&other.drive_idx))
            .then_with(|| self.rec_idx.cmp(&other.rec_idx))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Collect the global top-N records across ALL drives for `*` match-all.
///
/// Dispatches to either tree-walk (`Path` / `PathOnly` sort) or numeric sort
/// based on `sort_column`.  The exhaustive match contributes most of the
/// line count; no logic to extract.
///
/// Returns `(rows, phase_timings)`.  `phase_timings` is `Some` only for the
/// numeric branch (`collect_global_top_n_numeric`), which reports the
/// scan / sort / `path_resolve` sub-phase wall times.  The two tree-walk
/// branches (`Path` and `PathOnly`) emit rows in tree order with no
/// separate sort or `path_resolve` phase, so they return `None`.
#[must_use]
pub(crate) fn collect_global_top_n<D: AsRef<DriveCompactIndex> + Sync>(
    drives: &[D],
    limit: usize,
    sort_column: FieldId,
    sort_desc: bool,
    filter_mode: FilterMode,
    search_filters: &mut SearchFilters,
) -> (Vec<DisplayRow>, Option<PhaseTimings>) {
    tracing::debug!(
        sort_column = ?sort_column,
        sort_desc,
        limit,
        filter_mode = ?filter_mode,
        drives = drives.len(),
        "[2] collect_global_top_n entry"
    );
    match sort_column {
        // Full-path sort: tree-walk emits records in pre-order DFS with
        // name-sorted siblings, which is exactly lexicographic full-path
        // ASC (and DESC when the drive+child orders are reversed).  No
        // post-sort needed.
        FieldId::Path => (
            collect_path_sorted_top_n(drives, limit, sort_desc, filter_mode, search_filters),
            None,
        ),
        // Parent-directory sort: the `path_only_top_n` submodule
        // implements a two-phase tree walk that produces rows in
        // `path_only`-sorted order directly, with early termination
        // at `limit` and a name-ASC tiebreaker matching `sort_rows`.
        // No post-sort or truncate required.  The ext-index fast
        // path populates `PhaseTimings` (scan / sort / path_resolve);
        // the tree-walk branch returns `None` because its single
        // traversal interleaves every phase.
        FieldId::PathOnly => {
            collect_path_only_sorted_top_n(drives, limit, sort_desc, filter_mode, search_filters)
        }
        // All other fields (Size, Name, Extension, Created, Modified, etc.)
        // use the generic numeric sort/collect path.
        FieldId::Size
        | FieldId::SizeOnDisk
        | FieldId::Created
        | FieldId::Modified
        | FieldId::Accessed
        | FieldId::Drive
        | FieldId::Descendants
        | FieldId::TreeAllocated
        | FieldId::Bulkiness
        | FieldId::Name
        | FieldId::Extension
        | FieldId::Type
        | FieldId::Attributes
        | FieldId::AttributeValue
        | FieldId::Hidden
        | FieldId::System
        | FieldId::Archive
        | FieldId::ReadOnly
        | FieldId::Compressed
        | FieldId::Encrypted
        | FieldId::Sparse
        | FieldId::Reparse
        | FieldId::Offline
        | FieldId::NotIndexed
        | FieldId::Temporary
        | FieldId::Virtual
        | FieldId::Pinned
        | FieldId::Unpinned
        | FieldId::TreeSize
        | FieldId::Integrity
        | FieldId::NoScrub
        | FieldId::DirectoryFlag
        | FieldId::RecallOnOpen
        | FieldId::RecallOnDataAccess
        | FieldId::ParityAttributes
        | FieldId::NameLength
        | FieldId::PathLength => {
            let (rows, timings) = collect_global_top_n_numeric(
                drives,
                limit,
                sort_column,
                sort_desc,
                filter_mode,
                search_filters,
            );
            (rows, Some(timings))
        }
    }
}

/// Returns `true` if a record with `is_directory` passes `filter_mode`.
pub(super) const fn passes_filter_mode(is_directory: bool, mode: FilterMode) -> bool {
    match mode {
        FilterMode::All => true,
        FilterMode::FilesOnly => !is_directory,
        FilterMode::DirsOnly => is_directory,
    }
}

/// Sort a slice of compact indices by their name (case-insensitive).
///
/// Uses `CaseFold::cmp_str` for zero-alloc, per-codepoint fold comparison.
#[must_use]
pub(crate) fn search_compact_drive_regex(
    drive: &DriveCompactIndex,
    compiled_re: &regex::Regex,
    limit: usize,
) -> Vec<DisplayRow> {
    let mut vp_buf = [0_u8; 4];
    let volume_prefix = stack_volume_prefix(&mut vp_buf, drive.letter);
    let profile = *CACHE_PROFILE;

    let t_match = std::time::Instant::now();
    let match_indices: Vec<u32> = drive
        .records
        .iter()
        .enumerate()
        .filter(|(_, rec)| {
            let name = rec.name(&drive.names);
            !name.is_empty() && compiled_re.is_match(name)
        })
        .take(limit)
        .map(|(idx, _)| uffs_mft::len_to_u32(idx))
        .collect();
    let match_ms = t_match.elapsed().as_millis();
    let match_count = match_indices.len();

    let t_resolve = std::time::Instant::now();
    let rows = indices_to_rows(drive, &match_indices, volume_prefix);
    let resolve_ms = t_resolve.elapsed().as_millis();

    if profile {
        tracing::debug!(
            target: "cache_profile",
            drive = %drive.letter,
            regex_match_ms = %match_ms,
            match_count,
            scanned = drive.records.len(),
            resolve_ms = %resolve_ms,
            "search_regex"
        );
    }

    rows
}

/// Extract the best trigram lookup needle from a search pattern.
///
/// For OR-queries (`|`), returns empty (no trigram lookup).  For globs,
/// extracts the longest literal segment.  For plain substrings, returns
/// the needle as-is.
#[expect(
    clippy::single_call_fn,
    reason = "extracted from search_compact_drive to satisfy too_many_lines lint"
)]
fn extract_trigram_needle(needle: &str, is_glob: bool, is_or: bool) -> String {
    if is_or {
        String::new()
    } else if is_glob {
        needle
            .split(['*', '?'])
            .max_by_key(|seg| seg.len())
            .unwrap_or("")
            .to_owned()
    } else {
        needle.to_owned()
    }
}

/// Emit search timing via `tracing::debug!` for a single-drive search.
#[expect(
    clippy::single_call_fn,
    reason = "extracted from search_compact_drive to satisfy too_many_lines lint"
)]
fn log_search_profile(
    letter: uffs_mft::platform::DriveLetter,
    tri_ms: u128,
    match_ms: u128,
    resolve_ms: u128,
    match_count: usize,
    tri_count: usize,
    total_records: usize,
) {
    let scan_mode = if tri_count > 0 { "trigram" } else { "full" };
    let scan_count = if tri_count > 0 {
        tri_count
    } else {
        total_records
    };
    tracing::debug!(
        target: "cache_profile",
        drive = %letter,
        tri_ms = %tri_ms,
        match_ms = %match_ms,
        match_count,
        scan_mode,
        scan_count,
        resolve_ms = %resolve_ms,
        "search_compact"
    );
}

/// Collect record indices that match the name predicate, either from
/// trigram candidates or a full scan, up to `limit` results.
#[expect(
    clippy::single_call_fn,
    reason = "extracted from search_compact_drive to satisfy too_many_lines lint"
)]
fn collect_match_indices(
    drive: &DriveCompactIndex,
    candidates: Option<Vec<u32>>,
    limit: usize,
    lower_buf: &mut Vec<u8>,
    matches: &dyn Fn(&str, &mut Vec<u8>) -> bool,
) -> Vec<u32> {
    match candidates {
        None => {
            let mut out = Vec::new();
            for (idx, rec) in drive.records.iter().enumerate() {
                if out.len() >= limit {
                    break;
                }
                let name = rec.name(&drive.names);
                if matches(name, lower_buf) {
                    out.push(uffs_mft::len_to_u32(idx));
                }
            }
            out
        }
        Some(candidate_indices) => {
            let mut out = Vec::with_capacity(candidate_indices.len().min(limit));
            for &idx in &candidate_indices {
                if out.len() >= limit {
                    break;
                }
                let Some(rec) = drive.records.get(idx as usize) else {
                    continue;
                };
                let name = rec.name(&drive.names);
                if matches(name, lower_buf) {
                    out.push(idx);
                }
            }
            out
        }
    }
}

/// Search a single drive's compact index (trigram + glob/substring).
#[must_use]
pub(crate) fn search_compact_drive(
    drive: &DriveCompactIndex,
    needle: &str,
    limit: usize,
    case_sensitive: bool,
    whole_word: bool,
    match_path: bool,
) -> Vec<DisplayRow> {
    if needle.is_empty() {
        return Vec::new();
    }

    let mut vp_buf = [0_u8; 4];
    let volume_prefix = stack_volume_prefix(&mut vp_buf, drive.letter);
    let is_glob = needle.contains('*') || needle.contains('?');
    let is_or = needle.contains('|');

    // $UpCase case folding engine — zero-alloc comparisons, buffer-reuse fold.
    let fold = drive.fold;

    // Pre-fold the needle for case-insensitive matching.
    let mut needle_fold_buf: Vec<u8> = Vec::with_capacity(needle.len());
    let needle_folded = if case_sensitive {
        needle.to_owned()
    } else {
        fold.fold_into(needle, &mut needle_fold_buf).to_owned()
    };

    // Pre-build a SIMD-accelerated substring finder for simple queries.
    // For 1–2 byte needles this is dramatically faster than `str::contains`
    // (memchr uses SSE2/AVX2/NEON vectorised search).
    let simple_substring = !is_glob && !is_or && !whole_word && !case_sensitive;
    let finder = simple_substring.then(|| memchr::memmem::Finder::new(needle_folded.as_bytes()));
    // Reusable buffer for on-the-fly CaseFold (avoids per-record heap alloc).
    let mut fold_buf: Vec<u8> = Vec::with_capacity(256);
    let matches = |name: &str, buf: &mut Vec<u8>| -> bool {
        if name.is_empty() || name == "." {
            return false;
        }
        if whole_word {
            if case_sensitive {
                if is_glob || is_or {
                    tree::name_matches(name, needle)
                } else {
                    name == needle
                }
            } else {
                let folded = fold.fold_into(name, buf);
                if is_glob || is_or {
                    tree::name_matches(folded, &needle_folded)
                } else {
                    folded == needle_folded
                }
            }
        } else if let Some(fnd) = &finder {
            buf.clear();
            let folded = fold.fold_into(name, buf);
            fnd.find(folded.as_bytes()).is_some()
        } else if case_sensitive {
            tree::name_matches(name, needle)
        } else {
            let folded = fold.fold_into(name, buf);
            tree::name_matches(folded, &needle_folded)
        }
    };

    let trigram_needle = extract_trigram_needle(needle, is_glob, is_or);
    let profile = *CACHE_PROFILE;

    let t_tri = std::time::Instant::now();
    let candidates = if !case_sensitive && trigram_needle.len() >= 3 {
        drive.trigram.search(&trigram_needle, fold)
    } else {
        None
    };
    let tri_ms = t_tri.elapsed().as_millis();
    let tri_count = candidates.as_ref().map_or(0, Vec::len);

    let t_match = std::time::Instant::now();
    let mut match_indices =
        collect_match_indices(drive, candidates, limit, &mut fold_buf, &matches);
    let match_ms = t_match.elapsed().as_millis();
    let match_count = match_indices.len();

    // ── path mode: expand matching directories to include all descendants ──
    if match_path && !match_indices.is_empty() {
        expand_directory_descendants(drive, &mut match_indices);
    }

    let t_resolve = std::time::Instant::now();
    let rows = indices_to_rows(drive, &match_indices, volume_prefix);
    let resolve_ms = t_resolve.elapsed().as_millis();

    if profile {
        log_search_profile(
            drive.letter,
            tri_ms,
            match_ms,
            resolve_ms,
            match_count,
            tri_count,
            drive.records.len(),
        );
    }

    rows
}

/// DFS expansion: for every matching directory, collect all descendant indices.
///
/// Extracted from `search_compact_drive` to stay under the `too_many_lines`
/// lint limit (the caller was 103/100 before extraction).
#[expect(
    clippy::single_call_fn,
    reason = "factored out to keep search_compact_drive under too_many_lines"
)]
fn expand_directory_descendants(drive: &DriveCompactIndex, indices: &mut Vec<u32>) {
    let mut extra: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    for &idx in indices.iter() {
        if let Some(rec) = drive.records.get(idx as usize)
            && rec.is_directory()
        {
            stack.push(idx);
            while let Some(dir_idx) = stack.pop() {
                for &child_idx in drive.children.get(dir_idx as usize) {
                    extra.push(child_idx);
                    if let Some(child_rec) = drive.records.get(child_idx as usize)
                        && child_rec.is_directory()
                    {
                        stack.push(child_idx);
                    }
                }
            }
        }
    }
    if !extra.is_empty() {
        indices.extend(extra);
        indices.sort_unstable();
        indices.dedup();
    }
}

/// Search a single drive using tree-based path traversal.
#[must_use]
pub(crate) fn search_compact_drive_tree(
    drive: &DriveCompactIndex,
    pattern_lower: &str,
    limit: usize,
) -> Vec<DisplayRow> {
    let mut vp_buf = [0_u8; 4];
    let volume_prefix = stack_volume_prefix(&mut vp_buf, drive.letter);
    let profile = *CACHE_PROFILE;

    let t_tree = std::time::Instant::now();
    let match_indices = tree::tree_search(drive, pattern_lower, limit);
    let tree_ms = t_tree.elapsed().as_millis();
    let match_count = match_indices.len();

    let t_resolve = std::time::Instant::now();
    let mut dir_cache = tree::dir_cache_with_capacity(256);
    let rows: Vec<DisplayRow> = match_indices
        .iter()
        .filter_map(|&record_idx| {
            let rec = drive.records.get(record_idx as usize)?;
            let name = rec.name(&drive.names);
            if name.is_empty() {
                return None;
            }
            let path = tree::resolve_path_cached(
                drive,
                record_idx as usize,
                volume_prefix,
                &mut dir_cache,
            );
            Some(make_display_row(record_idx, drive.letter, rec, name, path))
        })
        .collect();
    let resolve_ms = t_resolve.elapsed().as_millis();

    if profile {
        tracing::debug!(
            target: "cache_profile",
            drive = %drive.letter,
            tree_ms = %tree_ms,
            match_count,
            resolve_ms = %resolve_ms,
            "search_tree"
        );
    }

    rows
}

// ── Shared helpers ──────────────────────────────────────────────────────────

/// Build a `DisplayRow` from a compact record.
///
/// ADS entries (name contains `:`) are always rendered as file-like rows
/// even when the underlying MFT record is a directory.  The raw `flags`
/// field preserves the NTFS ground truth — only the `is_directory`
/// display hint is adjusted.
pub(super) fn make_display_row(
    record_index: u32,
    drive_letter: uffs_mft::platform::DriveLetter,
    rec: &CompactRecord,
    name: &str,
    path: String,
) -> DisplayRow {
    // ADS entries on directories must not render as directories
    // (no trailing backslash, name shown, stream size used).
    let is_ads = name.contains(':');
    DisplayRow::new(
        record_index,
        drive_letter,
        path,
        rec.size,
        rec.is_directory() && !is_ads,
        rec.modified,
        rec.created,
        rec.accessed,
        rec.flags,
        rec.allocated,
        rec.descendants,
        rec.treesize,
        rec.tree_allocated,
    )
}

/// Build a `"X:\\"` volume prefix on the stack.
///
/// Returns a 3-byte `&str` without heap allocation.  Uses safe
/// `from_utf8` with a fallback — the bytes are always valid ASCII.
#[inline]
pub(super) fn stack_volume_prefix(
    buf: &mut [u8; 4],
    letter: uffs_mft::platform::DriveLetter,
) -> &str {
    buf[0] = letter.as_char() as u8;
    buf[1] = b':';
    buf[2] = b'\\';
    core::str::from_utf8(buf.get(..3).unwrap_or(b"?:\\")).unwrap_or("?:\\")
}

/// Push an element into a `BinaryHeap` capped at `limit`.
///
/// If the heap is below capacity, always push.  If at capacity, only push
/// if the new element would displace the current top (and pop the old top).
/// This keeps the heap at most `limit` entries.
#[inline]
pub(super) fn heap_push_capped<T: Ord>(heap: &mut BinaryHeap<T>, entry: T, limit: usize) {
    if heap.len() < limit {
        heap.push(entry);
    } else if let Some(top) = heap.peek()
        && entry < *top
    {
        // New entry is "better" — displace the worst.
        // (For Reverse<T> this means the underlying T is *larger*.)
        drop(heap.pop());
        heap.push(entry);
    }
}

/// Convert a list of record indices into `DisplayRow`s with resolved paths.
fn indices_to_rows(
    drive: &DriveCompactIndex,
    indices: &[u32],
    volume_prefix: &str,
) -> Vec<DisplayRow> {
    let mut dir_cache = tree::dir_cache_with_capacity(256);
    indices
        .iter()
        .filter_map(|&record_idx| {
            let rec = drive.records.get(record_idx as usize)?;
            let name = rec.name(&drive.names);
            if name.is_empty() {
                return None;
            }
            let path = tree::resolve_path_cached(
                drive,
                record_idx as usize,
                volume_prefix,
                &mut dir_cache,
            );
            Some(make_display_row(record_idx, drive.letter, rec, name, path))
        })
        .collect()
}

// ════════════════════════════════════════════════════════════════════════
// REGRESSION TESTS — End-to-End Compact Search Parity
//
// These tests build a synthetic MftIndex → compact index → search and
// verify DisplayRow correctness. They protect against field mapping,
// filter wiring, and system metafile handling regressions.
// See `docs/architecture/2026_03_30_04_12_SEARCH_PIPELINE_REGRESSION_ANALYSIS.
// md` ════════════════════════════════════════════════════════════════════════
#[cfg(test)]
#[path = "../query_tests.rs"]
mod tests;
