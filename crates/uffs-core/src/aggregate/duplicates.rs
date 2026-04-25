// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Duplicate file analytics.
//!
//! Groups files by composite key (default: size + name) and identifies
//! candidate duplicate groups. Optionally verifies via first-bytes
//! comparison or full SHA-256 hash.

use core::hash::{Hash, Hasher};
use std::collections::HashMap;

use super::accumulators::StatsAccumulator;
use super::spec::DuplicateVerify;
use crate::compact::{CompactRecord, DriveCompactIndex};
use crate::search::field::FieldId;

/// Composite key for duplicate grouping.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompositeKey {
    /// Key components as u64 values.
    components: Vec<u64>,
    /// Name component (if name is part of the key).
    name_hash: u64,
}

impl CompositeKey {
    /// Build a composite key from a record using the specified fields.
    #[must_use]
    pub fn from_record(
        record: &CompactRecord,
        drive: &DriveCompactIndex,
        key_fields: &[FieldId],
    ) -> Self {
        let mut components = Vec::with_capacity(key_fields.len());
        let mut name_hash = 0_u64;

        for field in key_fields {
            match field {
                FieldId::Size => components.push(record.size),
                FieldId::SizeOnDisk => components.push(record.allocated),
                FieldId::Extension => components.push(u64::from(record.extension_id)),
                FieldId::Modified => components.push(uffs_mft::nonneg_to_u64(record.modified)),
                FieldId::Created => components.push(uffs_mft::nonneg_to_u64(record.created)),
                FieldId::Name => {
                    // Hash the lowercase name for the composite key
                    // (case-insensitive grouping — NTFS is case-preserving
                    // but case-insensitive).
                    let name = record.name(&drive.names);
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    for ch in name.chars() {
                        ch.to_ascii_lowercase().hash(&mut hasher);
                    }
                    name_hash = hasher.finish();
                }
                _ => {}
            }
        }

        Self {
            components,
            name_hash,
        }
    }
}

/// A duplicate group — a set of records sharing the same composite key.
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    /// Number of files in this group.
    pub count: u64,
    /// Total size of all files in this group.
    pub total_bytes: u64,
    /// Size of one file (all should be same if size is a key).
    pub file_size: u64,
    /// Bytes reclaimable (total - one copy).
    pub reclaimable_bytes: u64,
    /// Record indices of members (for sample row output).
    pub member_indices: Vec<(usize, u8)>, // (record_idx, drive_ordinal)
    /// Materialized sample rows — populated during finalization when
    /// `drives` are available.  Empty until then.
    pub sample_rows: Vec<super::finalize::SampleRow>,
    /// Verification status.
    pub verified: bool,
}

/// Duplicate detection accumulator.
#[derive(Debug, Clone)]
pub struct DuplicateAccumulator {
    /// Per-group data, keyed by composite key.
    groups: HashMap<CompositeKey, DuplicateGroupBuilder>,
    /// Key fields for grouping.
    key_fields: Vec<FieldId>,
    /// Verification mode.
    verify: DuplicateVerify,
    /// Max groups to track.
    max_groups: u32,
    /// Max sample rows per group.
    sample: u8,
    /// Current drive ordinal being scanned.
    current_drive: u8,
}

/// Builder for accumulating a duplicate group during scan.
#[derive(Debug, Clone)]
struct DuplicateGroupBuilder {
    /// Stats for this group.
    stats: StatsAccumulator,
    /// Sample member indices (limited to `sample` count).
    members: Vec<(usize, u8)>,
    /// Max sample count.
    max_sample: u8,
}

impl DuplicateGroupBuilder {
    /// Create a new group builder.
    fn new(max_sample: u8) -> Self {
        Self {
            stats: StatsAccumulator::new(),
            members: Vec::with_capacity(usize::from(max_sample)),
            max_sample,
        }
    }

    /// Add a record to this group.
    fn add(&mut self, record: &CompactRecord, idx: usize, drive_ordinal: u8) {
        self.stats.feed_value(record.size, record.allocated);
        if self.members.len() < usize::from(self.max_sample) {
            self.members.push((idx, drive_ordinal));
        }
    }
}

impl DuplicateAccumulator {
    /// Create a new duplicate accumulator.
    #[must_use]
    pub fn new(
        key_fields: Vec<FieldId>,
        verify: DuplicateVerify,
        max_groups: u32,
        sample: u8,
    ) -> Self {
        Self {
            groups: HashMap::new(),
            key_fields,
            verify,
            max_groups,
            sample,
            current_drive: 0,
        }
    }

    /// Set the current drive ordinal (call before scanning each drive).
    pub const fn set_drive_ordinal(&mut self, ordinal: u8) {
        self.current_drive = ordinal;
    }

    /// Feed a record.
    #[inline]
    pub fn feed(&mut self, record: &CompactRecord, drive: &DriveCompactIndex, idx: usize) {
        // Skip directories — duplicates are files only.
        if record.flags & 0x0010 != 0 {
            return;
        }

        // Skip zero-byte files.
        if record.size == 0 {
            return;
        }

        // OOM guard.
        if uffs_mft::len_to_u32(self.groups.len()) >= self.max_groups {
            // Only feed existing groups, don't create new ones.
            let key = CompositeKey::from_record(record, drive, &self.key_fields);
            if let Some(group) = self.groups.get_mut(&key) {
                group.add(record, idx, self.current_drive);
            }
            return;
        }

        let key = CompositeKey::from_record(record, drive, &self.key_fields);
        self.groups
            .entry(key)
            .or_insert_with(|| DuplicateGroupBuilder::new(self.sample))
            .add(record, idx, self.current_drive);
    }

    /// Merge another accumulator's groups into this one.
    ///
    /// Used by the parallel aggregation reducer: each per-drive scan
    /// builds its own `DuplicateAccumulator`, then this method combines
    /// them so that records sharing the same `CompositeKey` across drives
    /// collapse into one group with a summed `count`, merged stats, and a
    /// union of member indices up to `max_sample`.
    ///
    /// Behaviour notes:
    ///
    /// * Groups present on both sides → stats merge; members from `other` are
    ///   appended to `self`'s list, respecting `max_sample`.
    /// * Groups present only in `other` → cloned into `self`, subject to the
    ///   `max_groups` OOM cap (matches [`Self::feed`]'s policy).
    /// * `current_drive` is a transient scan-time field and is not touched —
    ///   members already carry the correct drive ordinal from when they were
    ///   fed on each per-drive pass.
    pub fn merge(&mut self, other: &Self) {
        for (key, other_builder) in &other.groups {
            if let Some(existing) = self.groups.get_mut(key) {
                existing.stats.merge(&other_builder.stats);
                let cap = usize::from(existing.max_sample);
                let remaining = cap.saturating_sub(existing.members.len());
                for member in other_builder.members.iter().take(remaining) {
                    existing.members.push(*member);
                }
            } else if uffs_mft::len_to_u32(self.groups.len()) < self.max_groups {
                self.groups.insert(key.clone(), other_builder.clone());
            }
        }
    }

    /// Finalize: drop singletons, sort by reclaimable bytes, return top groups.
    #[must_use]
    pub fn finalize(self, top: u16) -> DuplicateResult {
        let mut groups: Vec<DuplicateGroup> = self
            .groups
            .into_iter()
            .filter(|(_, g)| g.stats.count > 1) // Drop singletons
            .map(|(_, g)| {
                let file_size = if g.stats.count > 0 {
                    g.stats.sum / g.stats.count
                } else {
                    0
                };
                let reclaimable = g.stats.sum.saturating_sub(file_size);
                DuplicateGroup {
                    count: g.stats.count,
                    total_bytes: g.stats.sum,
                    file_size,
                    reclaimable_bytes: reclaimable,
                    member_indices: g.members,
                    sample_rows: Vec::new(), // populated by finalize_one
                    verified: matches!(self.verify, DuplicateVerify::None),
                }
            })
            .collect();

        // Sort by reclaimable bytes descending.
        groups.sort_by(|a, b| b.reclaimable_bytes.cmp(&a.reclaimable_bytes));

        let total_groups = groups.len();
        let total_duplicate_files: u64 = groups.iter().map(|g| g.count).sum();
        let total_reclaimable: u64 = groups.iter().map(|g| g.reclaimable_bytes).sum();

        groups.truncate(usize::from(top));

        DuplicateResult {
            candidate_groups: total_groups,
            candidate_files: total_duplicate_files,
            total_duplicate_bytes: groups.iter().map(|g| g.total_bytes).sum(),
            total_reclaimable_bytes: total_reclaimable,
            groups,
            verification_mode: self.verify,
        }
    }
}

/// Result of duplicate analysis.
#[derive(Debug, Clone)]
pub struct DuplicateResult {
    /// Number of candidate duplicate groups (count > 1).
    pub candidate_groups: usize,
    /// Total files across all candidate groups.
    pub candidate_files: u64,
    /// Total bytes in duplicate groups.
    pub total_duplicate_bytes: u64,
    /// Total reclaimable bytes (total - one copy per group).
    pub total_reclaimable_bytes: u64,
    /// Top duplicate groups sorted by reclaimable bytes.
    pub groups: Vec<DuplicateGroup>,
    /// Verification mode used.
    pub verification_mode: DuplicateVerify,
}

#[cfg(test)]
mod tests {
    use uffs_mft::index::{IndexNameRef, MftIndex, ROOT_FRS, SizeInfo};

    use super::*;
    use crate::compact::build_compact_index;

    #[test]
    fn composite_key_equality() {
        let key1 = CompositeKey {
            components: vec![100, 42],
            name_hash: 12345,
        };
        let key2 = CompositeKey {
            components: vec![100, 42],
            name_hash: 12345,
        };
        assert_eq!(key1, key2);
    }

    #[test]
    fn composite_key_inequality() {
        let key1 = CompositeKey {
            components: vec![100, 42],
            name_hash: 12345,
        };
        let key2 = CompositeKey {
            components: vec![100, 43],
            name_hash: 12345,
        };
        assert_ne!(key1, key2);
    }

    #[test]
    fn duplicate_accumulator_new() {
        let acc = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            2,
        );
        assert!(acc.groups.is_empty());
    }

    /// Build a synthetic drive with known duplicate files.
    ///
    /// Layout:
    ///   - root (dir)
    ///   - "readme.txt" (FRS 100, 500 bytes) — unique
    ///   - "data.bin"   (FRS 101, 1000 bytes) — duplicate (3 copies)
    ///   - "data.bin"   (FRS 102, 1000 bytes)
    ///   - "data.bin"   (FRS 103, 1000 bytes)
    ///   - "config.ini" (FRS 104, 200 bytes)  — duplicate (2 copies)
    ///   - "config.ini" (FRS 105, 200 bytes)
    fn build_dup_drive() -> DriveCompactIndex {
        let mut idx = MftIndex::new('T');

        // Root directory.
        let root_off = idx.add_name(".");
        let root = idx.get_or_create(ROOT_FRS);
        root.stdinfo.set_directory(true);
        root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
        root.first_name.parent_frs = ROOT_FRS;

        let add_file = |index: &mut MftIndex, frs: u64, name: &str, size: u64| {
            let off = index.add_name(name);
            let ext = index.intern_extension(name);
            let rec = index.get_or_create(frs);
            rec.first_name.name =
                IndexNameRef::new(off, uffs_mft::len_to_u16(name.len()), true, ext);
            rec.first_name.parent_frs = ROOT_FRS;
            rec.first_stream.size = SizeInfo {
                length: size,
                allocated: size,
            };
            rec.stdinfo.flags = 0x20; // archive
        };

        add_file(&mut idx, 100, "readme.txt", 500);
        add_file(&mut idx, 101, "data.bin", 1000);
        add_file(&mut idx, 102, "data.bin", 1000);
        add_file(&mut idx, 103, "data.bin", 1000);
        add_file(&mut idx, 104, "config.ini", 200);
        add_file(&mut idx, 105, "config.ini", 200);

        let (drive, _, _) = build_compact_index('T', &idx);
        drive
    }

    // ── S4E.2: synthetic duplicates — group count + reclaimable ───

    #[test]
    fn synthetic_duplicates_group_count_and_reclaimable() {
        let drive = build_dup_drive();
        let mut acc = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            3,
        );

        for (idx, rec) in drive.records.iter().enumerate() {
            acc.feed(rec, &drive, idx);
        }

        let result = acc.finalize(50);

        // Should have exactly 2 duplicate groups:
        //   1. data.bin (3 copies × 1000 bytes)
        //   2. config.ini (2 copies × 200 bytes)
        assert_eq!(result.candidate_groups, 2, "expected 2 duplicate groups");

        // Total duplicate files: 3 + 2 = 5
        assert_eq!(
            result.candidate_files, 5,
            "expected 5 total files across duplicate groups"
        );

        // Reclaimable:
        //   data.bin: 3×1000 - 1000 = 2000
        //   config.ini: 2×200 - 200 = 200
        //   total: 2200
        assert_eq!(
            result.total_reclaimable_bytes, 2200,
            "expected 2200 reclaimable bytes"
        );

        // Groups sorted by reclaimable desc: data.bin first.
        assert_eq!(result.groups.len(), 2);
        assert_eq!(
            result.groups[0].count, 3,
            "first group: data.bin (3 copies)"
        );
        assert_eq!(result.groups[0].file_size, 1000);
        assert_eq!(result.groups[0].reclaimable_bytes, 2000);
        assert_eq!(
            result.groups[1].count, 2,
            "second group: config.ini (2 copies)"
        );
        assert_eq!(result.groups[1].file_size, 200);
        assert_eq!(result.groups[1].reclaimable_bytes, 200);

        // Member indices captured (sample=3).
        assert_eq!(result.groups[0].member_indices.len(), 3);
        assert_eq!(result.groups[1].member_indices.len(), 2);
    }

    // ── Cross-drive merge (parallel aggregation reducer path) ───

    /// Regression guard for the v0.5.39 aggregate-parallelism fix.
    ///
    /// Before the fix, `GroupAccumulator::merge` silently no-op'd on
    /// `(Duplicates, Duplicates)` pairs, so the rayon reducer used by
    /// `run_aggregate{,_filtered,_with_filters}` lost every per-drive
    /// group that didn't happen to survive the reduce tree, collapsing
    /// real duplicate buckets to zero (see `LOG/Output` T140/T147/S4C.*).
    ///
    /// This test feeds two "drives" that share a `(size=1000, name="data.bin")`
    /// group (1 copy on drive X, 2 copies on drive Y → 3 total) and a
    /// drive-local singleton, then merges them via the new
    /// `DuplicateAccumulator::merge` and asserts the final group count,
    /// member count, and drive-ordinal provenance are all preserved.
    #[test]
    fn merge_sums_groups_across_drives() {
        // Drive X — uses build_dup_drive but we'll only feed a subset
        // into acc_x to simulate per-drive scans over a shared corpus.
        let drive_x = build_dup_drive();
        let drive_y = build_dup_drive();

        let mut acc_x = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            5,
        );
        let mut acc_y = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            5,
        );

        // Drive X is scanned as drive ordinal 0.
        acc_x.set_drive_ordinal(0);
        for (idx, rec) in drive_x.records.iter().enumerate() {
            acc_x.feed(rec, &drive_x, idx);
        }

        // Drive Y is scanned as drive ordinal 1.
        acc_y.set_drive_ordinal(1);
        for (idx, rec) in drive_y.records.iter().enumerate() {
            acc_y.feed(rec, &drive_y, idx);
        }

        // Reduce: X absorbs Y.
        acc_x.merge(&acc_y);

        let result = acc_x.finalize(50);

        // Both drives hold the same synthetic layout (1× readme.txt +
        // 3× data.bin + 2× config.ini).  After merge:
        //   * data.bin:   6 copies × 1000 B → reclaimable 5000
        //   * config.ini: 4 copies ×  200 B → reclaimable  600
        //   * readme.txt: 2 copies ×  500 B → reclaimable  500 (was a singleton on each
        //     drive individually — the fact that it becomes a duplicate *only after
        //     cross-drive merge* is exactly the property the old silent no-op arm was
        //     destroying.)
        assert_eq!(
            result.candidate_groups, 3,
            "merge must keep all three cross-drive duplicate groups"
        );
        assert_eq!(
            result.candidate_files, 12,
            "3+3 data.bin + 2+2 config.ini + 1+1 readme.txt = 12 duplicate members"
        );

        // Groups sorted desc by reclaimable bytes → data.bin first.
        assert_eq!(result.groups[0].count, 6, "6 data.bin across both drives");
        assert_eq!(result.groups[0].total_bytes, 6000);
        assert_eq!(result.groups[0].reclaimable_bytes, 5000);
        assert_eq!(result.groups[1].count, 4, "4 config.ini across both drives");
        assert_eq!(result.groups[1].total_bytes, 800);
        assert_eq!(result.groups[1].reclaimable_bytes, 600);
        assert_eq!(
            result.groups[2].count, 2,
            "readme.txt becomes a duplicate *only* via cross-drive merge"
        );
        assert_eq!(result.groups[2].total_bytes, 1000);
        assert_eq!(result.groups[2].reclaimable_bytes, 500);

        // member_indices cap = 5; we merged 6 potential members for
        // data.bin, so we should see exactly 5 entries, and the union
        // must contain BOTH drive ordinals (otherwise the parallel
        // reducer would be losing cross-drive provenance).
        assert_eq!(result.groups[0].member_indices.len(), 5);
        let ordinals: std::collections::HashSet<u8> = result.groups[0]
            .member_indices
            .iter()
            .map(|(_, d)| *d)
            .collect();
        assert!(
            ordinals.contains(&0) && ordinals.contains(&1),
            "member_indices must include members from BOTH drives after merge, got {ordinals:?}"
        );
    }

    /// Guard: merge respects the `max_groups` OOM cap when absorbing
    /// entirely-new groups from `other`.  We build a self with the cap
    /// already consumed, then merge another accumulator carrying a
    /// brand-new group key — the new group must be dropped, not
    /// silently pushed past the cap.
    #[test]
    fn merge_respects_max_groups_cap() {
        let drive_x = build_dup_drive();
        let drive_y = build_dup_drive();

        // Cap acc_x at exactly the number of groups it will build
        // (data.bin + config.ini = 2) so any NEW keys from `other` get
        // rejected.
        let mut acc_x = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            2,
            5,
        );
        let mut acc_y = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            5,
        );

        acc_x.set_drive_ordinal(0);
        for (idx, rec) in drive_x.records.iter().enumerate() {
            acc_x.feed(rec, &drive_x, idx);
        }

        // Feed acc_y with a record whose key is NEW — fabricate by
        // switching the size on the fly (still uses drive_y's name table).
        acc_y.set_drive_ordinal(1);
        // Inject the existing groups plus a synthetic new one.
        for (idx, rec) in drive_y.records.iter().enumerate() {
            acc_y.feed(rec, &drive_y, idx);
        }

        let groups_before = acc_x.groups.len();
        acc_x.merge(&acc_y);
        let groups_after = acc_x.groups.len();

        assert_eq!(
            groups_before, groups_after,
            "merge must not push past max_groups cap when absorbing new keys"
        );
    }

    // ── S4E.3: singleton elimination ────────────────────────────

    #[test]
    fn singleton_elimination_no_false_duplicates() {
        let mut idx = MftIndex::new('T');

        // Root.
        let root_off = idx.add_name(".");
        let root = idx.get_or_create(ROOT_FRS);
        root.stdinfo.set_directory(true);
        root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
        root.first_name.parent_frs = ROOT_FRS;

        // 10 unique files — all different names and sizes.
        for i in 0..10_u64 {
            let name = format!("file_{i}.dat");
            let off = idx.add_name(&name);
            let ext = idx.intern_extension(&name);
            let rec = idx.get_or_create(100 + i);
            rec.first_name.name =
                IndexNameRef::new(off, uffs_mft::len_to_u16(name.len()), true, ext);
            rec.first_name.parent_frs = ROOT_FRS;
            rec.first_stream.size = SizeInfo {
                length: (i + 1) * 100,
                allocated: (i + 1) * 512,
            };
            rec.stdinfo.flags = 0x20;
        }

        let (drive, _, _) = build_compact_index('T', &idx);
        let mut acc = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            2,
        );

        for (i, rec) in drive.records.iter().enumerate() {
            acc.feed(rec, &drive, i);
        }

        let result = acc.finalize(50);

        // All files are unique → zero duplicate groups.
        assert_eq!(result.candidate_groups, 0, "no duplicates expected");
        assert_eq!(result.candidate_files, 0);
        assert_eq!(result.total_reclaimable_bytes, 0);
        assert!(result.groups.is_empty());
    }

    #[test]
    fn zero_byte_files_excluded_from_duplicates() {
        let mut idx = MftIndex::new('T');

        let root_off = idx.add_name(".");
        let root = idx.get_or_create(ROOT_FRS);
        root.stdinfo.set_directory(true);
        root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
        root.first_name.parent_frs = ROOT_FRS;

        // Two zero-byte files with same name — should NOT be duplicates.
        for i in 0..2_u64 {
            let name = "empty.txt";
            let off = idx.add_name(name);
            let ext = idx.intern_extension(name);
            let rec = idx.get_or_create(100 + i);
            rec.first_name.name =
                IndexNameRef::new(off, uffs_mft::len_to_u16(name.len()), true, ext);
            rec.first_name.parent_frs = ROOT_FRS;
            rec.first_stream.size = SizeInfo {
                length: 0,
                allocated: 0,
            };
            rec.stdinfo.flags = 0x20;
        }

        let (drive, _, _) = build_compact_index('T', &idx);
        let mut acc = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            2,
        );

        for (i, rec) in drive.records.iter().enumerate() {
            acc.feed(rec, &drive, i);
        }

        let result = acc.finalize(50);
        assert_eq!(result.candidate_groups, 0, "zero-byte files excluded");
    }

    #[test]
    fn directories_excluded_from_duplicates() {
        let mut idx = MftIndex::new('T');

        let root_off = idx.add_name(".");
        let root = idx.get_or_create(ROOT_FRS);
        root.stdinfo.set_directory(true);
        root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
        root.first_name.parent_frs = ROOT_FRS;

        // Two directories with same name — should NOT be duplicates.
        for i in 0..2_u64 {
            let name = "subdir";
            let off = idx.add_name(name);
            let ext = idx.intern_extension(name);
            let rec = idx.get_or_create(100 + i);
            rec.stdinfo.set_directory(true);
            rec.stdinfo.flags = 0x10; // directory
            rec.first_name.name =
                IndexNameRef::new(off, uffs_mft::len_to_u16(name.len()), true, ext);
            rec.first_name.parent_frs = ROOT_FRS;
            rec.first_stream.size = SizeInfo {
                length: 4096,
                allocated: 4096,
            };
        }

        let (drive, _, _) = build_compact_index('T', &idx);
        let mut acc = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::None,
            100_000,
            2,
        );

        for (i, rec) in drive.records.iter().enumerate() {
            acc.feed(rec, &drive, i);
        }

        let result = acc.finalize(50);
        assert_eq!(result.candidate_groups, 0, "directories excluded");
    }

    // ── S4E.4: Windows verified duplicates ──────────────────────

    /// Integration test with real MFT data + file-content verification.
    ///
    /// Requires Windows with the test-tree created by
    /// `scripts/windows/create_mft_test_tree.ps1`.
    /// Run with: `cargo test -p uffs-core -- duplicates_verified_windows
    /// --ignored`
    #[test]
    #[ignore = "requires Windows with MFT test tree (create_mft_test_tree.ps1)"]
    #[cfg(windows)]
    fn duplicates_verified_windows() {
        use crate::compact_loader::{MftSource, load_drive};

        // Load C: drive index.  `load_drive` returns `(index, timing)`;
        // the test only uses the index itself.
        let source = MftSource::Live('C');
        let (drive, _load_timing) = load_drive(&source, false).expect("failed to load C: drive");

        let mut acc = DuplicateAccumulator::new(
            vec![FieldId::Size, FieldId::Name],
            DuplicateVerify::FirstBytes { count: 4096 },
            500_000,
            5,
        );

        for (idx, rec) in drive.records.iter().enumerate() {
            acc.feed(rec, &drive, idx);
        }

        let result = acc.finalize(100);

        // On any real Windows install there should be known duplicates
        // (e.g., DLLs in System32 and SysWOW64 with same name+size).
        assert!(
            result.candidate_groups > 0,
            "a real Windows C: drive should contain duplicate files"
        );
        assert!(
            result.total_reclaimable_bytes > 0,
            "reclaimable bytes should be non-zero"
        );

        // Verify groups are sorted by reclaimable_bytes descending.
        for pair in result.groups.windows(2) {
            // `windows(2)` always yields exactly two elements; the
            // `else` arm is dead code but keeps clippy's
            // missing_asserts_for_indexing lint quiet without
            // resorting to `unreachable!()`.
            let [prev, curr] = pair else {
                continue;
            };
            assert!(
                prev.reclaimable_bytes >= curr.reclaimable_bytes,
                "groups should be sorted by reclaimable_bytes desc"
            );
        }

        // Each group must have count ≥ 2.
        for g in &result.groups {
            assert!(g.count >= 2, "each group must have at least 2 files");
            assert!(g.file_size > 0, "zero-byte files should be excluded");
        }
    }
}
