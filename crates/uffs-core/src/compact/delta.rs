// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Mutable overlay over the immutable base CSR indexes
//! (incremental-index-maintenance §5.1).
//!
//! The base [`crate::trigram::TrigramIndex`] / [`super::ChildrenIndex`] /
//! [`super::ExtensionIndex`] are compressed-sparse-row structures: fast to
//! query, immutable, and **expensive to rebuild** (the per-apply rebuild is the
//! cost this project removes). [`IndexDelta`] holds the postings *added* since
//! the last compaction plus a tombstone set for records whose base postings are
//! stale (deleted or renamed away). A search reads `base ∪ delta − tombstones`;
//! an occasional compaction folds the delta back into a fresh base and clears
//! it (`delta = None`).
//!
//! **Invariant:** every posting list is kept **sorted ascending and deduped**
//! on insert, so the base∪delta merge at query time is a linear sorted-merge
//! and tombstone filtering is a sorted-set difference. The base CSR posting
//! lists are already sorted, so the shapes compose.
//!
//! This is Phase-2 scaffolding: the type + its merge primitives land here with
//! unit tests; `DriveCompactIndex` gains the `delta` field and the
//! `trigram_search` choke point when trigram delta is wired (design §4 Phase
//! 2), so each of the ~20 `DriveCompactIndex` construction sites is touched
//! exactly once, with the change that gives the field meaning.

use rustc_hash::{FxHashMap, FxHashSet};

/// Mutable overlay over the immutable base CSR indexes. A `None`
/// `delta` on [`super::DriveCompactIndex`] means "freshly compacted — pure
/// base, zero query overhead".
#[derive(Debug, Default, Clone)]
pub struct IndexDelta {
    /// packed-trigram → sorted, deduped record indices added since compaction.
    pub trigram: FxHashMap<u64, Vec<u32>>,
    /// `ext_id` → sorted, deduped record indices added since compaction.
    pub ext: FxHashMap<u16, Vec<u32>>,
    /// parent record idx → sorted, deduped child record indices added since
    /// compaction.
    pub children: FxHashMap<u32, Vec<u32>>,
    /// record indices whose BASE postings are stale (deleted / renamed-away).
    pub tombstones: FxHashSet<u32>,
    /// count of distinct records touched since compaction (the compaction
    /// trigger input — see [`IndexDelta::len`]).
    pub touched_records: u32,
}

impl IndexDelta {
    /// Register a newly created / renamed-in record's postings across every
    /// index overlay. `trigrams` is the packed-trigram set of the record's name
    /// (deduped by the caller is fine — [`sorted_insert`] dedups anyway).
    ///
    /// A renamed record is `tombstone`d at its stale base postings first, then
    /// `add_record`ed at its new ones; create is `add_record` only.
    pub fn add_record(&mut self, idx: u32, trigrams: &[u64], ext_id: u16, parent_idx: u32) {
        for &key in trigrams {
            sorted_insert(self.trigram.entry(key).or_default(), idx);
        }
        sorted_insert(self.ext.entry(ext_id).or_default(), idx);
        // u32::MAX parent = root sentinel; root has no parent posting to add to.
        if parent_idx != u32::MAX {
            sorted_insert(self.children.entry(parent_idx).or_default(), idx);
        }
        self.touched_records = self.touched_records.saturating_add(1);
    }

    /// Mark a record's BASE postings stale. Idempotent. The record may still
    /// reappear in `delta` postings via a subsequent [`IndexDelta::add_record`]
    /// (rename = tombstone-old + add-new); tombstone filtering is applied to
    /// the final merged set, so that is correct.
    pub fn tombstone(&mut self, idx: u32) {
        if self.tombstones.insert(idx) {
            self.touched_records = self.touched_records.saturating_add(1);
        }
    }

    /// Whether `idx`'s base postings have been tombstoned.
    #[must_use]
    pub fn is_tombstoned(&self, idx: u32) -> bool {
        self.tombstones.contains(&idx)
    }

    /// Records touched since compaction — the compaction-trigger input. Counts
    /// distinct adds + tombstones (an add and a tombstone of the same idx, as
    /// in a rename, count as two touches, which is the intended "work done"
    /// signal).
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.touched_records
    }

    /// Whether nothing has been overlaid since compaction.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.touched_records == 0
    }

    /// Delta postings for one packed trigram (sorted, deduped), or `&[]`.
    #[must_use]
    pub fn trigram_postings(&self, key: u64) -> &[u32] {
        self.trigram.get(&key).map_or(&[], Vec::as_slice)
    }

    /// Delta postings for one extension id (sorted, deduped), or `&[]`.
    #[must_use]
    pub fn ext_postings(&self, ext_id: u16) -> &[u32] {
        self.ext.get(&ext_id).map_or(&[], Vec::as_slice)
    }

    /// Delta child postings for one parent idx (sorted, deduped), or `&[]`.
    #[must_use]
    pub fn child_postings(&self, parent_idx: u32) -> &[u32] {
        self.children.get(&parent_idx).map_or(&[], Vec::as_slice)
    }
}

/// Insert `value` into a sorted, deduped `Vec<u32>`, keeping it sorted and
/// deduped. No-op if already present. O(log n) search + O(n) shift — postings
/// are small per key (one apply batch's worth) so this is cheap.
fn sorted_insert(list: &mut Vec<u32>, value: u32) {
    if let Err(pos) = list.binary_search(&value) {
        list.insert(pos, value);
    }
}

// NOTE (Phase 2, design §5.2): the per-key `base ∪ delta − tombstones`
// sorted-merge that `DriveCompactIndex::trigram_search` will compose lands in
// the Phase-2 commit, wired directly into that accessor (so it is exercised the
// moment it exists, never dead scaffolding). The sorted/deduped posting
// invariant this type maintains is what makes that merge a linear pass.

#[cfg(test)]
mod tests {
    use super::IndexDelta;

    #[test]
    fn add_record_keeps_postings_sorted_and_deduped() {
        let mut delta = IndexDelta::default();
        // Insert out of order + a duplicate trigram for the same record.
        delta.add_record(5, &[300, 100, 200, 100], 2, 4);
        delta.add_record(3, &[100], 2, 4);
        delta.add_record(9, &[100], 7, 4);

        assert_eq!(delta.trigram_postings(100), &[3, 5, 9], "sorted + deduped");
        assert_eq!(delta.trigram_postings(200), &[5]);
        assert_eq!(delta.trigram_postings(300), &[5]);
        assert_eq!(delta.ext_postings(2), &[3, 5]);
        assert_eq!(delta.ext_postings(7), &[9]);
        assert_eq!(
            delta.child_postings(4),
            &[3, 5, 9],
            "all three share parent 4"
        );
        assert_eq!(delta.trigram_postings(999), &[] as &[u32], "absent key");
    }

    #[test]
    fn root_parent_sentinel_adds_no_child_posting() {
        let mut delta = IndexDelta::default();
        delta.add_record(0, &[10], 1, u32::MAX); // root: no parent posting
        assert!(
            delta.children.is_empty(),
            "u32::MAX parent must not create a posting"
        );
        assert_eq!(delta.trigram_postings(10), &[0]);
    }

    #[test]
    fn tombstone_is_idempotent_and_counted_once() {
        let mut delta = IndexDelta::default();
        delta.tombstone(7);
        delta.tombstone(7);
        assert!(delta.is_tombstoned(7));
        assert!(!delta.is_tombstoned(8));
        assert_eq!(delta.len(), 1, "duplicate tombstone is not double-counted");
    }

    #[test]
    fn len_counts_distinct_touches_including_rename_as_two() {
        let mut delta = IndexDelta::default();
        assert!(delta.is_empty());
        // rename: tombstone old postings, add new — two units of work.
        delta.tombstone(4);
        delta.add_record(4, &[1, 2], 0, 1);
        assert_eq!(delta.len(), 2);
        assert!(!delta.is_empty());
    }
}
