// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Post-apply rebuild of the derived indexes for [`super::apply_usn_patch`].
//!
//! After the per-change loop mutates the record columns + `frs_to_compact`,
//! the derived structures (children CSR, path lengths, trigram, extension
//! inverted index) are rebuilt from scratch so newly created / renamed /
//! deleted files appear in tree traversal AND trigram / `--ext` search.
//!
//! This is the **O(total-records)** step the incremental-index-maintenance
//! work (`docs/architecture/incremental-index-maintenance.md`) replaces with a
//! base+delta overlay; until then the per-index rebuild cost is captured here
//! under the `IDXDELTA-TIMING` dev marker so a baseline can be measured and a
//! regression detected.  Extracted from `compact_loader.rs` to keep that file
//! under the workspace 800-LOC policy and to house the temporary IDXDELTA
//! timing in one place for Phase-5 removal.

use std::time::Instant;

use super::PatchStats;
use crate::compact::{ChildrenIndex, DriveCompactIndex, ExtensionIndex};
use crate::trigram::TrigramIndex;

/// Rebuild the derived indexes from the mutated records + names and emit the
/// per-batch summary.  `loop_elapsed` is how long the caller's O(changed)
/// per-change loop took, so the `IDXDELTA-TIMING` line can attribute time to
/// the loop vs. each index rebuild.
pub(super) fn rebuild_derived_and_log(
    drive: &mut DriveCompactIndex,
    changes_len: usize,
    stats: &PatchStats,
    loop_elapsed: core::time::Duration,
) {
    let loop_us = dur_us(loop_elapsed);

    // IDXDELTA-TIMING: per-index full-rebuild cost of one apply — the
    // O(total-records) baseline the incremental (base+delta) work drives
    // toward O(changed).  Remove with the IDXDELTA dev instrumentation (Phase 5).
    let t_children = Instant::now();
    drive.children = ChildrenIndex::build(&drive.records);
    let children_us = dur_us(t_children.elapsed());
    // Recompute path_len for all records (picks up creates + renames).
    let t_paths = Instant::now();
    crate::compact::compute_path_lengths(&mut drive.records, &drive.names, drive.letter);
    let paths_us = dur_us(t_paths.elapsed());
    // Rebuild trigram index using CaseFold — no names_lower clone needed.
    let t_trigram = Instant::now();
    drive.trigram = TrigramIndex::build(&drive.records, &drive.names, drive.fold);
    let trigram_us = dur_us(t_trigram.elapsed());
    // Rebuild extension inverted index so --ext queries reflect USN changes.
    let t_ext = Instant::now();
    drive.ext_index = ExtensionIndex::build(&drive.records);
    let ext_us = dur_us(t_ext.elapsed());

    if changes_len != 0 {
        tracing::info!(
            marker = "IDXDELTA-TIMING",
            drive = %drive.letter,
            records = drive.records.len(),
            changes = changes_len,
            loop_us,
            children_us,
            paths_us,
            trigram_us,
            ext_us,
            rebuild_us = children_us
                .saturating_add(paths_us)
                .saturating_add(trigram_us)
                .saturating_add(ext_us),
            total_us = loop_us
                .saturating_add(children_us)
                .saturating_add(paths_us)
                .saturating_add(trigram_us)
                .saturating_add(ext_us),
            "IDXDELTA-TIMING apply: per-change loop + full index rebuild (baseline)"
        );
        log_batch_summary(drive, changes_len, stats);
    }
}

/// IDXDELTA-TIMING helper: a `Duration` as whole microseconds (`u64`).
/// Integer to satisfy uffs-core's `float_arithmetic` deny and to keep sub-ms
/// precision for the O(changed) loop; the WIN idx-delta-verify script renders
/// these as ms.  Remove with the IDXDELTA dev instrumentation (Phase 5).
fn dur_us(elapsed: core::time::Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

/// Emit the per-batch USN-apply summary (how the poll mutated the index) at
/// DEBUG.
fn log_batch_summary(drive: &DriveCompactIndex, changes: usize, stats: &PatchStats) {
    tracing::debug!(
        drive = %drive.letter,
        changes,
        created = stats.created,
        deleted = stats.deleted,
        renamed = stats.renamed,
        skipped = stats.skipped,
        records = drive.records.len(),
        ext_index_entries = drive.ext_index.total_entries(),
        "usn apply: batch applied"
    );
}
