// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Incremental USN-driven updates for persisted indexes.

use super::{MftIndex, NO_ENTRY, frs_to_usize};

// USN Journal Incremental Update Support
// ============================================================================

/// Statistics from applying USN changes to an index.
#[derive(Debug, Clone, Default)]
pub struct UsnApplyStats {
    /// Number of records marked as deleted
    pub deleted: usize,
    /// Number of records updated via targeted MFT read (full data)
    pub targeted_reads: usize,
    /// Number of records created (placeholder — targeted read failed)
    pub created: usize,
    /// Number of records modified (name/metadata)
    pub modified: usize,
    /// Number of changes skipped (FRS not in index)
    pub skipped: usize,
}

impl MftIndex {
    /// Applies USN Journal changes to update the index incrementally.
    ///
    /// **Phase 1 — Deletions only.** Marks deleted records via bit 31 of
    /// `stdinfo.flags`. Non-delete changes (creates, renames, size/metadata
    /// changes) are collected and returned so the caller can perform targeted
    /// MFT reads to get full record data.
    ///
    /// Returns `(stats, frs_to_read)`:
    /// - `stats` — counts of deleted / skipped records
    /// - `frs_to_read` — FRS values that need a targeted MFT read
    pub fn apply_usn_deletes(
        &mut self,
        changes: &[crate::usn::FileChange],
    ) -> (UsnApplyStats, Vec<u64>) {
        const DELETED_FLAG: u32 = 0x8000_0000;

        let mut stats = UsnApplyStats::default();
        let mut frs_to_read: Vec<u64> = Vec::new();

        for change in changes {
            // `change.frs` is typed `Frs`; lift to raw `u64` once at the
            // `frs_to_idx` / `frs_to_read` boundary because both the
            // index lookup table is `Vec<u32>` keyed by `usize` and the
            // returned `frs_to_read` feeds the kernel-loop arithmetic
            // input of `read_targeted_frs_records(&[u64])`.
            let frs_raw = change.frs.raw();
            let frs_usize = frs_to_usize(frs_raw);
            let idx = self.frs_to_idx.get(frs_usize).copied().unwrap_or(NO_ENTRY);

            if change.deleted {
                if idx == NO_ENTRY {
                    stats.skipped += 1;
                } else if let Some(record) = self.records.get_mut(idx as usize) {
                    record.stdinfo.flags |= DELETED_FLAG;
                    stats.deleted += 1;
                }
            } else if change.created
                || change.renamed
                || change.size_changed
                || change.metadata_changed
            {
                // Collect FRS for targeted MFT read — the read will
                // overwrite or create the full record with correct data.
                frs_to_read.push(frs_raw);
            } else {
                stats.skipped += 1;
            }
        }

        // Deduplicate (same FRS may appear in multiple changes)
        frs_to_read.sort_unstable();
        frs_to_read.dedup();

        // Mark the index as mutated so downstream caches detect staleness.
        if stats.deleted > 0 || !frs_to_read.is_empty() {
            self.bump_epoch();
        }

        (stats, frs_to_read)
    }
}
