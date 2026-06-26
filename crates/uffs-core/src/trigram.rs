// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Character-level trigram inverted index — CSR (Compressed Sparse Row) layout.
//!
//! Maps 3-codepoint trigrams (folded via NTFS `$UpCase`) to sorted lists
//! of record indices using three contiguous arrays:
//!
//! - `keys`:    sorted `u64` packed char-trigrams (3 × `u16` folded codepoints)
//! - `offsets`: CSR offsets into `values` (len = `keys.len()` + 1)
//! - `values`:  flat `u32` posting entries
//!
//! Lookup is binary-search on `keys` → slice into `values`.
//! This layout is cache-friendly, allocation-free after construction,
//! and can be serialized/deserialized as three bulk `memcpy`s.
//!
//! ## Character trigrams vs byte trigrams
//!
//! The index uses **character-level** trigrams (3 Unicode codepoints folded
//! to uppercase via `CaseFold`) instead of byte-level trigrams.  This gives
//! correct case-insensitive matching for non-ASCII filenames (e.g. ü↔Ü,
//! é↔É, Д↔д).
//!
//! ## Build algorithm
//!
//! Two-pass counting sort (same pattern as `ChildrenIndex`):
//!
//! 1. **Pass 1 — count** (parallel): rayon chunks walk `CompactRecord` name
//!    slices, fold each codepoint via `CaseFold`, deduplicate trigrams per
//!    record via `TinyTriSet`, increment per-trigram counters in chunk-local
//!    `FxHashMap`s. Chunk maps are merged into global counts.
//! 2. **Sort keys + prefix sum** → CSR `keys` + `offsets`.
//! 3. **Pass 2 — scatter**: re-iterate names, write `record_idx` into
//!    `values[write_pos[key_idx]++]` for each unique trigram.
//!
//! Peak memory is only the final CSR arrays (~200 MB for 7M records)
//! plus a ~1.6 MB `FxHashMap` LUT (replacing the old 64 MB flat array).

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use uffs_text::case_fold::CaseFold;

use crate::compact::CompactRecord;
use crate::trigram_key::pack_char_trigram;

/// Trigram inverted index in CSR (Compressed Sparse Row) layout.
///
/// Keys are packed `u64` char-trigrams (3 folded `u16` codepoints).
#[derive(Clone)]
pub struct TrigramIndex {
    /// Sorted packed char-trigram keys (`u64`, see [`pack_char_trigram`]).
    keys: Vec<u64>,
    /// CSR offsets into `values`. Length = `keys.len() + 1`.
    /// Posting list for `keys[i]` is `values[offsets[i]..offsets[i+1]]`.
    offsets: Vec<u32>,
    /// Flat array of all posting values (record indices), sorted per posting
    /// list.
    values: Vec<u32>,
}

impl TrigramIndex {
    /// Total heap capacity of this index (keys + offsets + values) in bytes.
    #[must_use]
    pub const fn heap_size_bytes(&self) -> usize {
        self.keys.capacity() * size_of::<u64>()
            + self.offsets.capacity() * size_of::<u32>()
            + self.values.capacity() * size_of::<u32>()
    }

    /// Create an empty trigram index (no postings).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            offsets: vec![0],
            values: Vec::new(),
        }
    }

    /// Build a trigram index from compact records and the **original-case**
    /// names blob, using `CaseFold` for per-codepoint folding.
    ///
    /// Uses a **two-pass counting-sort** algorithm (same pattern as
    /// `ChildrenIndex::build`):
    ///
    /// 1. **Pass 1 — count**: parallel scan of names → per-chunk
    ///    `FxHashMap<packed_char_trigram, count>`. Merge into global counts.
    /// 2. **Sort keys + prefix sum** → CSR `keys` + `offsets`.
    /// 3. **Pass 2 — scatter**: re-iterate names, write `record_idx` into
    ///    `values` at the correct write position for each trigram.
    ///
    /// No `names_lower` clone needed — folding happens inline per codepoint.
    ///
    /// **Peak memory**: final CSR arrays (~200 MB for 7M records)
    /// + ~1.6 MB `FxHashMap` LUT (was 64 MB flat array).
    #[must_use]
    pub fn build(records: &[CompactRecord], names: &[u8], fold: CaseFold) -> Self {
        const CHUNK_SIZE: usize = 64 * 1024;

        if records.is_empty() {
            return Self::empty();
        }

        // ── Pass 1: parallel count (char-level trigrams) ─────────────
        let chunk_counts: Vec<FxHashMap<u64, u32>> = records
            .par_chunks(CHUNK_SIZE)
            .map(|chunk| {
                let mut local: FxHashMap<u64, u32> = FxHashMap::default();
                let mut seen = TinyTriSet::new();
                let mut folded_buf: Vec<u16> = Vec::with_capacity(64);
                for rec in chunk {
                    let start = rec.name_offset as usize;
                    let end = start + rec.name_len as usize;
                    let Some(name_bytes) = names.get(start..end) else {
                        continue;
                    };
                    let name_str = core::str::from_utf8(name_bytes).unwrap_or("");
                    folded_buf.clear();
                    folded_buf.extend(name_str.chars().map(|ch| fold.fold_char(ch)));
                    if folded_buf.len() < 3 {
                        continue;
                    }
                    seen.clear();
                    for window in folded_buf.windows(3) {
                        let Some(&[cp0, cp1, cp2]) = window.first_chunk::<3>() else {
                            continue;
                        };
                        let packed = pack_char_trigram(cp0, cp1, cp2);
                        if seen.insert(packed) {
                            *local.entry(packed).or_insert(0) += 1;
                        }
                    }
                }
                local
            })
            .collect();

        // Merge chunk counts into global counts.
        let mut global_counts: FxHashMap<u64, u32> = FxHashMap::default();
        for chunk_map in &chunk_counts {
            #[expect(
                clippy::iter_over_hash_type,
                reason = "merge target is also a HashMap; insertion order is irrelevant — sorted below"
            )]
            for (&tri, &cnt) in chunk_map {
                *global_counts.entry(tri).or_insert(0) += cnt;
            }
        }

        // ── Prune high-frequency trigrams ──────────────────────────
        // Trigrams appearing in >25% of records are too common to provide
        // useful selectivity.  Pruning them saves memory (posting lists
        // are the dominant cost) and makes searches faster (shorter
        // intersection chains).  The search side handles missing trigrams
        // gracefully — `filter_map` skips them.
        let record_count = records.len();
        // Minimum cap of 1024 so small indices aren't over-pruned.
        let freq_cap = (record_count / 4).max(1024);
        let pre_prune = global_counts.len();
        global_counts.retain(|_tri, cnt| (*cnt as usize) <= freq_cap);
        let pruned = pre_prune.saturating_sub(global_counts.len());
        if pruned > 0 {
            tracing::debug!(
                pruned,
                pre_prune,
                freq_cap,
                "trigram build: pruned high-frequency trigrams"
            );
        }

        // ── Sort keys + prefix sum → CSR offsets ────────────────────
        let mut sorted_keys: Vec<(u64, u32)> = global_counts.into_iter().collect();
        sorted_keys.sort_unstable_by_key(|&(packed, _)| packed);

        let trigram_count = sorted_keys.len();
        let mut keys = Vec::with_capacity(trigram_count);
        let mut offsets = Vec::with_capacity(trigram_count + 1);
        let mut running = 0_u32;

        // FxHashMap LUT: packed_char_trigram → key_index.
        // ~50K entries × 16 B ≈ 1.6 MB (was 64 MB flat array).
        let mut tri_lut: FxHashMap<u64, u32> = FxHashMap::default();
        tri_lut.reserve(trigram_count);

        for (key_idx, &(packed, count)) in sorted_keys.iter().enumerate() {
            keys.push(packed);
            offsets.push(running);
            running = running.saturating_add(count);
            let ki = uffs_mft::len_to_u32(key_idx);
            tri_lut.insert(packed, ki);
        }
        offsets.push(running);
        drop(sorted_keys);

        // ── Pass 2: scatter record_idx into CSR values (parallel) ────
        let values = scatter_postings_parallel(
            records,
            names,
            fold,
            &tri_lut,
            &offsets,
            running,
            &chunk_counts,
        );
        drop(tri_lut);

        Self {
            keys,
            offsets,
            values,
        }
    }

    /// Construct directly from pre-built CSR arrays (cache deserialization).
    ///
    /// This is a zero-rebuild constructor — the three arrays are bulk-copied
    /// from the cache file, no per-element processing needed.
    #[must_use]
    pub const fn from_csr(keys: Vec<u64>, offsets: Vec<u32>, values: Vec<u32>) -> Self {
        Self {
            keys,
            offsets,
            values,
        }
    }

    /// Borrow the CSR components for serialization.
    #[must_use]
    pub(crate) fn as_csr(&self) -> (&[u64], &[u32], &[u32]) {
        (&self.keys, &self.offsets, &self.values)
    }

    /// Number of unique trigrams in the index.
    #[must_use]
    pub const fn posting_count(&self) -> usize {
        self.keys.len()
    }

    /// Look up the base posting list for a single packed char-trigram key.
    ///
    /// `pub(crate)` so [`crate::compact::DriveCompactIndex::trigram_search`]
    /// can merge a base posting with its delta overlay (incremental-index
    /// §5.2) without re-deriving the CSR lookup.
    #[must_use]
    pub(crate) fn get_posting(&self, packed: u64) -> Option<&[u32]> {
        let idx = self.keys.binary_search(&packed).ok()?;
        let start = *self.offsets.get(idx)? as usize;
        let end = *self.offsets.get(idx + 1)? as usize;
        self.values.get(start..end)
    }

    /// Search: intersect posting lists for query char-trigrams, return
    /// candidate record indices.
    ///
    /// For queries < 3 chars, returns `None` (caller should fall back to
    /// linear scan).
    #[must_use]
    pub fn search(&self, needle: &str, fold: CaseFold) -> Option<Vec<u32>> {
        let trigrams = needle_trigrams(needle, fold)?;
        if trigrams.is_empty() {
            return Some(Vec::new());
        }

        let mut lists: Vec<&[u32]> = trigrams
            .iter()
            .filter_map(|&tri| self.get_posting(tri))
            .collect();

        if lists.is_empty() {
            return Some(Vec::new());
        }

        lists.sort_unstable_by_key(|list| list.len());

        let Some(first_list) = lists.first() else {
            return Some(Vec::new());
        };
        let mut result = first_list.to_vec();
        for list in lists.iter().skip(1) {
            intersect_in_place(&mut result, list);
            if result.is_empty() {
                break;
            }
        }

        Some(result)
    }
}

/// The deduped packed char-trigrams of a search needle, or `None` if the needle
/// folds to fewer than 3 codepoints (caller falls back to a linear scan).
///
/// Shared by [`TrigramIndex::search`] and the base+delta
/// [`crate::compact::DriveCompactIndex::trigram_search`] so the needle→trigram
/// packing has exactly one definition.
#[must_use]
pub(crate) fn needle_trigrams(needle: &str, fold: CaseFold) -> Option<Vec<u64>> {
    let folded: Vec<u16> = needle.chars().map(|ch| fold.fold_char(ch)).collect();
    if folded.len() < 3 {
        return None;
    }
    let mut seen = rustc_hash::FxHashSet::default();
    let mut trigrams: Vec<u64> = Vec::new();
    for window in folded.windows(3) {
        let Some(&[cp0, cp1, cp2]) = window.first_chunk::<3>() else {
            continue;
        };
        let packed = pack_char_trigram(cp0, cp1, cp2);
        if seen.insert(packed) {
            trigrams.push(packed);
        }
    }
    Some(trigrams)
}

/// Intersect a sorted `Vec<u32>` with a sorted slice **in place**.
///
/// Retains only elements present in both, preserving sorted order.
/// Shrinks `result` via `truncate` — no allocation, no new `Vec`.
pub(crate) fn intersect_in_place(result: &mut Vec<u32>, other: &[u32]) {
    let mut write = 0_usize;
    let mut j = 0_usize;
    for i in 0..result.len() {
        let Some(&val) = result.get(i) else { break };
        // Advance `j` until other[j] >= val.
        while let Some(&ov) = other.get(j) {
            if ov >= val {
                break;
            }
            j += 1;
        }
        if other.get(j).copied() == Some(val)
            && let Some(slot) = result.get_mut(write)
        {
            *slot = val;
            write += 1;
            j += 1;
        }
    }
    result.truncate(write);
}

/// Pass 2 of the counting-sort trigram build: scatter `record_idx` values
/// into the pre-allocated CSR `values` array — **parallel** version.
///
/// Uses the per-chunk counts from Pass 1 to compute non-overlapping write
/// regions for each chunk, allowing embarrassingly parallel scatter.
///
/// Within each chunk, records are visited in order, and chunks themselves
/// are ordered, so each posting list remains sorted by record index.
///
/// `tri_lut` is an `FxHashMap<u64, u32>` mapping packed char-trigrams to
/// their CSR key index (~1.6 MB, was 64 MB flat array).
///
/// Uses `AtomicU32` with `Relaxed` ordering for the shared values array.
/// On x86-64 this compiles to plain `mov` — zero overhead vs non-atomic
/// writes. The atomics are a safe alternative to raw pointers in a crate
/// that forbids `unsafe_code`.
#[expect(
    clippy::single_call_fn,
    reason = "extracted to keep build() under line limit"
)]
fn scatter_postings_parallel(
    records: &[CompactRecord],
    names: &[u8],
    fold: CaseFold,
    tri_lut: &FxHashMap<u64, u32>,
    offsets: &[u32],
    total_postings: u32,
    chunk_counts: &[FxHashMap<u64, u32>],
) -> Vec<u32> {
    use core::sync::atomic::AtomicU32;

    const CHUNK_SIZE: usize = 64 * 1024;

    let num_keys = if offsets.len() > 1 {
        offsets.len() - 1
    } else {
        return Vec::new();
    };

    // Build per-chunk write-start positions.
    let mut chunk_write_pos: Vec<Vec<u32>> = Vec::with_capacity(chunk_counts.len());
    let mut accumulated: Vec<u32> = offsets.get(..num_keys).map_or_else(Vec::new, Vec::from);

    for chunk_map in chunk_counts {
        chunk_write_pos.push(accumulated.clone());
        advance_offsets(&mut accumulated, chunk_map, tri_lut);
    }

    // Allocate the shared values array as AtomicU32.
    let values: Vec<AtomicU32> = (0..total_postings as usize)
        .map(|_| AtomicU32::new(0))
        .collect();

    // Parallel scatter: each chunk independently writes its records.
    records
        .par_chunks(CHUNK_SIZE)
        .zip(chunk_write_pos.par_iter())
        .enumerate()
        .for_each(|(chunk_idx, (chunk, base_pos))| {
            let record_offset = chunk_idx * CHUNK_SIZE;
            let mut write_pos = base_pos.clone();
            let mut seen = TinyTriSet::new();
            let mut folded_buf: Vec<u16> = Vec::with_capacity(64);

            for (local_idx, rec) in chunk.iter().enumerate() {
                let start = rec.name_offset as usize;
                let end = start + rec.name_len as usize;
                let Some(name_bytes) = names.get(start..end) else {
                    continue;
                };
                let name_str = core::str::from_utf8(name_bytes).unwrap_or("");
                folded_buf.clear();
                folded_buf.extend(name_str.chars().map(|ch| fold.fold_char(ch)));
                if folded_buf.len() < 3 {
                    continue;
                }
                let rec_idx = uffs_mft::len_to_u32(record_offset + local_idx);

                seen.clear();
                scatter_one_record(
                    &folded_buf,
                    rec_idx,
                    tri_lut,
                    &mut write_pos,
                    &values,
                    &mut seen,
                );
            }
        });

    // Convert AtomicU32 → u32 (zero-cost: same layout, just unwrap).
    values.into_iter().map(AtomicU32::into_inner).collect()
}

/// Advance accumulated offsets by one chunk's trigram counts.
#[inline]
#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity from scatter_postings_parallel"
)]
fn advance_offsets(
    accumulated: &mut [u32],
    chunk_map: &FxHashMap<u64, u32>,
    tri_lut: &FxHashMap<u64, u32>,
) {
    #[expect(
        clippy::iter_over_hash_type,
        reason = "iteration order irrelevant — accumulating counts"
    )]
    for (&packed, &cnt) in chunk_map {
        if let Some(&ki) = tri_lut.get(&packed)
            && let Some(slot) = accumulated.get_mut(ki as usize)
        {
            *slot += cnt;
        }
    }
}

/// Scatter char-trigrams from one record's folded codepoints into the
/// atomic values array.
#[inline]
#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity from scatter_postings_parallel"
)]
fn scatter_one_record(
    folded: &[u16],
    rec_idx: u32,
    tri_lut: &FxHashMap<u64, u32>,
    write_pos: &mut [u32],
    values: &[core::sync::atomic::AtomicU32],
    seen: &mut TinyTriSet,
) {
    use core::sync::atomic::Ordering;

    for window in folded.windows(3) {
        let Some(&[cp0, cp1, cp2]) = window.first_chunk::<3>() else {
            continue;
        };
        let packed = pack_char_trigram(cp0, cp1, cp2);
        if !seen.insert(packed) {
            continue;
        }
        let Some(key_idx) = tri_lut.get(&packed).copied() else {
            continue;
        };
        if let Some(pos) = write_pos.get_mut(key_idx as usize)
            && let Some(slot) = values.get(*pos as usize)
        {
            slot.store(rec_idx, Ordering::Relaxed);
            *pos += 1;
        }
    }
}

/// Tiny inline set for deduplicating packed char-trigram values within a
/// single filename.
///
/// NTFS filenames are at most 255 chars → at most 253 trigrams. We use a
/// small `Vec` with linear scan. For ≤253 elements this is faster than
/// hashing (no hash computation, cache-hot sequential scan).
struct TinyTriSet {
    /// Packed char-trigram values seen so far.
    seen: Vec<u64>,
}

impl TinyTriSet {
    /// Create a new empty set.
    fn new() -> Self {
        Self {
            seen: Vec::with_capacity(32),
        }
    }

    /// Reset for the next record without deallocating.
    fn clear(&mut self) {
        self.seen.clear();
    }

    /// Insert a packed trigram. Returns `true` if it was NOT already present.
    fn insert(&mut self, packed: u64) -> bool {
        if self.seen.contains(&packed) {
            return false;
        }
        self.seen.push(packed);
        true
    }
}
