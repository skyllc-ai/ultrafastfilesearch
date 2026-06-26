// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Compact in-memory index for search backends.
//!
//! Replaces the full `MftIndex` (224 bytes/record) with a lean 72-byte
//! `CompactRecord` that covers 100% of sortable/filterable columns.
//! Full metadata (ADS, forensic fields) is resolved on-demand from the
//! `.uffs` cache file.
//!
//! See `docs/architecture/COMPACT_INDEX_DESIGN.md` for the full design.
//!
//! This module owns [`DriveCompactIndex`] (the loaded drive + its search choke
//! points) and re-exports the row type, the CSR indexes, path-length
//! computation, and the MFT→compact builder from focused submodules
//! (`record`, `children`, `extension`, `path_len`, `builder`, `delta`).

use crate::bloom::Bloom;
pub use crate::compact_loader::apply_usn_patch;
// Re-export loader types and functions so callers can still use `compact::*`.
#[expect(deprecated, reason = "re-export kept for backward compatibility")]
pub use crate::compact_loader::{
    IndexSource, LoadTiming, MftSource, PatchStats, load_drive, load_mft_file, refresh_drive,
};
use crate::compact_storage::ColumnStorage;
use crate::path_trie::PathTrie;
use crate::trigram::TrigramIndex;

/// Mutable delta overlay over the immutable base CSR indexes (Phase 2+).
pub mod delta;

// File-size decomposition: the row type, the CSR indexes, path-length
// computation, and the MFT→compact builder live in focused submodules.  Every
// public item is re-exported below so the canonical `crate::compact::X` paths
// (used across the workspace) are unchanged.
mod builder;
mod children;
mod extension;
mod path_len;
mod record;

pub use builder::build_compact_index;
pub(crate) use builder::{INDEX_TTL_SECONDS, resolve_case_fold};
pub use children::ChildrenIndex;
pub use delta::IndexDelta;
pub use extension::ExtensionIndex;
pub(crate) use path_len::{PathChange, compute_path_lengths, update_path_lengths_incremental};
pub(crate) use record::NTFS_METAFILE_NAMES;
pub use record::{CompactRecord, is_ntfs_metafile_name};

/// A loaded drive with compact index.
#[derive(Clone)]
pub struct DriveCompactIndex {
    /// Drive letter (e.g., 'C').
    pub letter: uffs_mft::platform::DriveLetter,
    /// Compact records — one per MFT file/directory.
    ///
    /// Backed by [`ColumnStorage`] so Phase 2b can transparently
    /// swap the heap-resident `Vec` for a memory-mapped runtime
    /// tempfile.  Read-side call sites use [`Deref<[T]>`]; mutating
    /// callers (Windows USN-patch path) go through
    /// `ColumnStorage::as_mut_vec` (internal helper).
    pub records: ColumnStorage<CompactRecord>,
    /// All filenames concatenated (UTF-8 bytes, original case).
    ///
    /// Backed by [`ColumnStorage`]; see [`Self::records`] for the
    /// rationale.
    pub names: ColumnStorage<u8>,
    /// Trigram inverted index built from folded names (char-level, `$UpCase`).
    pub trigram: TrigramIndex,
    /// CSR children index: `children.get(i)` → child indices of record i.
    pub children: ChildrenIndex,
    /// Extension inverted index: `ext_id → record indices`.
    /// Enables O(K) `--ext` queries where K = matching records, not O(N).
    pub ext_index: ExtensionIndex,
    /// NTFS `$UpCase` case folding engine for this volume.
    pub fold: uffs_text::case_fold::CaseFold,
    /// Extension name table: `ext_names[extension_id]` → lowercase extension
    /// string (e.g. `"rs"`, `"txt"`). Index 0 = no extension.
    /// Used to resolve `--ext` filter strings to `u16` IDs for O(1)
    /// per-record matching instead of per-record string parsing.
    pub ext_names: Vec<Box<str>>,
    /// Where this index was loaded from (for future refresh).
    pub source: IndexSource,
    /// `MftIndex.build_epoch` this compact index was built from.
    /// Used as a staleness check when loading from cache.
    pub source_epoch: u64,
    /// Phase 4 bloom filter over folded basenames + extensions.
    ///
    /// `None` for indices built before bloom integration landed (e.g.
    /// in-process unit-test fixtures that don't exercise the Phase 4
    /// search-skip path) and for v ≤ 8 caches before the rebuild step
    /// runs.  After [`build_compact_index`] or a v9+ cache load this
    /// is always `Some(_)`; downstream callers
    /// (`search_dispatch::bloom_pre_check`) treat `None` as "no
    /// pre-check available; fall through to the full search" which
    /// is the safe (correct-but-slower) default.
    pub bloom: Option<Bloom>,
    /// Phase 4 directory-only path trie.  Same `None`-handling
    /// rationale as [`Self::bloom`].
    pub path_trie: Option<PathTrie>,
    /// Phase 8: FRS → `compact_idx` mapping.
    ///
    /// Indexed by FRS-as-`usize`; values are the matching primary
    /// `compact_idx` in [`Self::records`], or [`u32::MAX`] for
    /// unmapped slots (system metafiles 0-15, FRS values higher
    /// than the build-time max, deleted records).
    ///
    /// Populated by [`build_compact_index`] from
    /// [`uffs_mft::MftIndex::frs_to_idx`] (which is otherwise
    /// dropped when the `MftIndex` goes out of scope).  Maintained
    /// in lock-step with [`Self::records`] by
    /// [`crate::compact_loader::apply_usn_patch`] across
    /// create / delete / rename batches: creates extend the table
    /// and assign the new compact slot, deletes mark the slot
    /// `u32::MAX`, renames leave the slot intact (only `parent_idx`
    /// + name move).
    ///
    /// **Why not stored in `MftIndex`?**  The `MftIndex` is
    /// transient — `build_compact_index` consumes it and drops it.
    /// The compact body is what survives to serve search queries
    /// and accept journal patches.  Keeping the mapping next to the
    /// records it indexes means [`crate::compact_loader::apply_usn_patch`]
    /// can patch the body in place without touching the MFT.
    ///
    /// **Backward compatibility**: caches written before v10
    /// (Phase 8) didn't persist this mapping; for those, the field
    /// loads as an empty `Vec` and the surgical patch path
    /// silently degrades to the full-reload fallback.  See the
    /// v9 → v10 cache format bump in `compact_cache::COMPACT_VERSION`.
    pub frs_to_compact: Vec<u32>,
    /// Incremental-index-maintenance overlay (design §5.1).
    ///
    /// `None` on a freshly built / freshly compacted / cache-loaded index:
    /// the base CSR indexes ([`Self::trigram`], [`Self::children`],
    /// [`Self::ext_index`]) are authoritative and search reads them with zero
    /// overhead. Once [`crate::compact_loader::apply_usn_patch`] starts
    /// overlaying USN deltas (Phase 2b) this becomes `Some`, and the search
    /// choke points ([`Self::trigram_search`], …) merge base ∪ delta minus
    /// tombstones. Compaction folds the delta into a fresh base and resets it
    /// to `None`. Never serialized — the on-disk cache is always delta-free
    /// (compact before save), so a cache load yields `None`.
    pub delta: Option<IndexDelta>,
}

/// Per-component heap footprint of a [`DriveCompactIndex`].
#[derive(Debug, Clone)]
pub struct HeapReport {
    /// `records: ColumnStorage<CompactRecord>` capacity in bytes.
    /// Mmap-backed columns (Phase 2b) report `len * sizeof(T)`
    /// since the kernel-mapped pages have no extra slack.
    pub records: usize,
    /// `names: ColumnStorage<u8>` capacity in bytes.
    pub names: usize,
    /// `TrigramIndex` total heap (keys + offsets + values).
    pub trigram: usize,
    /// `ChildrenIndex` total heap (offsets + values).
    pub children: usize,
    /// `ExtensionIndex` total heap (offsets + values).
    pub ext_index: usize,
    /// `ext_names: Vec<Box<str>>` heap (Vec + string data).
    pub ext_names: usize,
    /// `frs_to_compact: Vec<u32>` capacity in bytes (Phase 8 —
    /// `~max_frs * 4` bytes; ~40 MB on a 7M-record drive with
    /// max FRS ≈ 10M).
    pub frs_to_compact: usize,
    /// Sum of all components.
    pub total: usize,
}

impl AsRef<Self> for DriveCompactIndex {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl DriveCompactIndex {
    /// Trigram candidate search through the base ∪ delta overlay (design §5.2).
    ///
    /// The single choke point every trigram caller goes through. When
    /// [`Self::delta`] is `None` (fresh / compacted index) it delegates to the
    /// base [`TrigramIndex::search`] with **zero** overhead. When a delta is
    /// present it merges, per needle-trigram, the base posting with the delta
    /// posting, intersects across the needle's trigrams (the trigram AND), then
    /// resolves tombstones on the final candidate set.
    ///
    /// **Tombstone correctness:** a candidate whose record is tombstoned is
    /// kept **iff** it appears in the delta posting of *every* needle
    /// trigram — i.e. it was re-added (renamed-in) under a name that still
    /// contains the needle. A deleted record (tombstoned, no re-add) and a
    /// renamed-away record matched only via its stale base postings are
    /// both dropped. Filtering the final set (not per posting list) is what
    /// lets a renamed file remain visible under its new name while
    /// disappearing from its old one.
    ///
    /// Returns `None` for needles under 3 codepoints (caller falls back to a
    /// linear scan), mirroring [`TrigramIndex::search`].
    #[must_use]
    pub fn trigram_search(&self, needle: &str) -> Option<Vec<u32>> {
        let Some(delta) = &self.delta else {
            return self.trigram.search(needle, self.fold);
        };
        let trigrams = crate::trigram::needle_trigrams(needle, self.fold)?;
        if trigrams.is_empty() {
            return Some(Vec::new());
        }

        // Per needle-trigram effective posting = base ∪ delta. An absent trigram
        // (empty in both) is skipped, never zeroing the result — the trigram
        // index is a candidate pre-filter, exactly as the base search treats it.
        let mut lists: Vec<Vec<u32>> = Vec::with_capacity(trigrams.len());
        for &tri in &trigrams {
            let base = self.trigram.get_posting(tri).unwrap_or(&[]);
            let merged = delta::merge_postings(base, delta.trigram_postings(tri));
            if !merged.is_empty() {
                lists.push(merged);
            }
        }
        if lists.is_empty() {
            return Some(Vec::new());
        }

        lists.sort_unstable_by_key(Vec::len);
        let mut result = lists.first().cloned().unwrap_or_default();
        for list in lists.iter().skip(1) {
            crate::trigram::intersect_in_place(&mut result, list);
            if result.is_empty() {
                break;
            }
        }

        // Final tombstone resolution: keep a tombstoned candidate only if it was
        // re-added under a name covering every needle trigram (see doc above).
        if !delta.tombstones.is_empty() {
            result.retain(|&idx| {
                !delta.is_tombstoned(idx)
                    || trigrams
                        .iter()
                        .all(|&tri| delta.trigram_postings(tri).binary_search(&idx).is_ok())
            });
        }
        Some(result)
    }

    /// Compute the total heap footprint of this index (in bytes).
    ///
    /// This measures *capacity* (what the allocator reserved), not *len*
    /// (what we're using).  The gap between the two is what `shrink_to_fit`
    /// reclaims.  Use this after loading to verify memory usage.
    #[must_use]
    pub fn heap_size_bytes(&self) -> HeapReport {
        let records = self.records.capacity() * size_of::<CompactRecord>();
        let names = self.names.capacity();
        let trigram = self.trigram.heap_size_bytes();
        let children = self.children.heap_size_bytes();
        let ext_index = self.ext_index.heap_size_bytes();
        let ext_names_data: usize = self.ext_names.iter().map(|en| en.len()).sum();
        let ext_names_vec = self.ext_names.capacity() * size_of::<Box<str>>();
        let ext_names = ext_names_data + ext_names_vec;
        let frs_to_compact = self.frs_to_compact.capacity() * size_of::<u32>();
        HeapReport {
            records,
            names,
            trigram,
            children,
            ext_index,
            ext_names,
            frs_to_compact,
            total: records + names + trigram + children + ext_index + ext_names + frs_to_compact,
        }
    }

    /// Log the heap report at `info` level.
    pub fn log_heap_report(&self) {
        let hr = self.heap_size_bytes();
        let mb = |bytes: usize| bytes / (1024 * 1024);
        tracing::info!(
            drive = %self.letter,
            records_count = self.records.len(),
            records_mb = mb(hr.records),
            names_mb = mb(hr.names),
            trigram_mb = mb(hr.trigram),
            children_mb = mb(hr.children),
            ext_index_mb = mb(hr.ext_index),
            ext_names_mb = mb(hr.ext_names),
            frs_to_compact_mb = mb(hr.frs_to_compact),
            total_mb = mb(hr.total),
            "[HEAP] {}: rec={} names={} tri={} ch={} ext={} f2c={} | total={} MB",
            self.letter,
            mb(hr.records), mb(hr.names), mb(hr.trigram),
            mb(hr.children), mb(hr.ext_index), mb(hr.frs_to_compact),
            mb(hr.total),
        );
    }

    /// Resolve extension filter strings to their `u16` IDs on this drive.
    ///
    /// Returns a sorted, deduplicated `Vec<u16>` of matching IDs.
    /// Extensions not found on this drive are silently ignored.
    ///
    /// The lookup is a linear scan of `ext_names` (~500–2000 short strings),
    /// which takes < 1 µs.  This runs **once per search per drive**, not per
    /// record.
    #[must_use]
    pub(crate) fn resolve_ext_ids(&self, extensions: &[String]) -> Vec<u16> {
        let mut ids = Vec::with_capacity(extensions.len());
        for ext in extensions {
            let normalized = ext.trim().trim_start_matches('.').to_lowercase();
            if normalized.is_empty() {
                continue;
            }
            for (ext_id, name) in (0_u16..).zip(self.ext_names.iter()) {
                if name.as_ref() == normalized {
                    ids.push(ext_id);
                    break;
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Extract `filename`'s extension and intern it into [`Self::ext_names`],
    /// returning its `extension_id`.
    ///
    /// Mirrors the build-time logic (`MftIndex::intern_extension` +
    /// `ExtensionTable::intern`) so a record created by the USN journal patch
    /// path lands under the SAME `extension_id` a full rebuild would assign —
    /// otherwise `--ext <x>` (which resolves the name via [`Self::ext_names`]
    /// and looks it up in the [`ExtensionIndex`]) silently misses the new file.
    ///
    /// Returns `0` (the reserved "no extension" id) for a dotless name, a
    /// leading-dot dotfile (`.gitignore`), a trailing-dot name (`file.`), or if
    /// the table is already at the `u16::MAX` interning ceiling.
    pub(crate) fn intern_extension(&mut self, filename: &str) -> u16 {
        // Extension = substring after the LAST dot, where the dot is neither
        // the first byte (dotfile) nor the last (trailing dot).
        let Some(dot_pos) = filename.rfind('.') else {
            return 0;
        };
        if dot_pos == 0 || dot_pos + 1 >= filename.len() {
            return 0;
        }
        let Some(raw_ext) = filename.get(dot_pos + 1..) else {
            return 0;
        };
        let normalized = raw_ext.trim_start_matches('.').to_lowercase();
        if normalized.is_empty() {
            return 0;
        }

        // Find-or-append. `ext_names[0]` is the reserved "" (no-extension)
        // slot, so a real extension never collides with id 0.
        if let Some(existing) = self
            .ext_names
            .iter()
            .position(|name| name.as_ref() == normalized)
        {
            return u16::try_from(existing).unwrap_or(0);
        }
        let Ok(new_id) = u16::try_from(self.ext_names.len()) else {
            // Interning ceiling reached (>= 65 535 distinct extensions);
            // fall back to "no extension" rather than wrap.
            return 0;
        };
        if new_id == u16::MAX {
            return 0;
        }
        self.ext_names.push(normalized.into_boxed_str());
        new_id
    }
}

// ════════════════════════════════════════════════════════════════════════
// REGRESSION TESTS — Search Pipeline Parity Guards
//
// These tests protect critical behaviors that broke during the v0.4.30
// refactor attempt.  They run on synthetic data (no Windows/MFT needed).
// See `docs/architecture/2026_03_30_04_12_SEARCH_PIPELINE_REGRESSION_ANALYSIS.
// md` ════════════════════════════════════════════════════════════════════════
#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "compact_trigram_delta_tests.rs"]
mod trigram_delta_tests;
