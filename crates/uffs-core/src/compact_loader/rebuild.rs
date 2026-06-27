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
use crate::compact::{DriveCompactIndex, PathChange, update_path_lengths_incremental};

/// Above this many touched records, the per-change incremental path update
/// loses to a single O(total) BFS (each create/rename re-walks parents), so we
/// fall back to the full [`crate::compact::compute_path_lengths`].  Sized well
/// above a normal USN poll batch; the 50k disk-save threshold is the practical
/// ceiling on a single apply anyway.
const FULL_PATH_RECOMPUTE_THRESHOLD: usize = 50_000;

/// Rebuild the derived indexes from the mutated records + names and emit the
/// per-batch summary.  `loop_elapsed` is how long the caller's O(changed)
/// per-change loop took, so the `IDXDELTA-TIMING` line can attribute time to
/// the loop vs. each index rebuild.
pub(super) fn rebuild_derived_and_log(
    drive: &mut DriveCompactIndex,
    changes_len: usize,
    stats: &PatchStats,
    loop_elapsed: core::time::Duration,
    path_changes: &[PathChange],
    tombstones: &[u32],
) {
    let loop_us = dur_us(loop_elapsed);

    // Phase 2b + 4a + 4b: overlay this batch's trigram + extension + children
    // postings onto the delta instead of rebuilding those bases. This runs
    // FIRST so the path subtree walk below sees the batch's new children
    // (creates/moves into a renamed directory). `apply_index_delta` masks the
    // tombstoned trigrams and folds back to fresh bases only when the delta
    // crosses the compaction threshold (so `trigram_us` is ~0 on most applies,
    // a full refold on the occasional compaction tick). children + ext are
    // served through the base ∪ delta accessors, so no per-apply rebuild of
    // either — `children_us` / `ext_us` now report ~0.
    let t_trigram = Instant::now();
    let compacted = drive.apply_index_delta(path_changes, tombstones);
    let trigram_us = dur_us(t_trigram.elapsed());
    let children_us = 0_u64;
    let ext_us = 0_u64;

    // Phase 1: refresh path_len only for the records this batch touched
    // (O(changed)). An EMPTY change set means the batch touched no record's
    // path_len (e.g. a delete-only batch), so the work is *none* — the
    // incremental update is a no-op over an empty slice. The full O(total) BFS
    // is reserved for the cold-load builder; the only apply-time fallback is a
    // pathologically huge batch where the per-record re-walk loses to one BFS.
    // The subtree walk reads the base ∪ delta children just populated above.
    let t_paths = Instant::now();
    if path_changes.len() > FULL_PATH_RECOMPUTE_THRESHOLD {
        crate::compact::compute_path_lengths(&mut drive.records, &drive.names, drive.letter);
    } else {
        update_path_lengths_incremental(
            drive.records.as_mut_slice(),
            &drive.names,
            drive.letter,
            &drive.children,
            drive.delta.as_ref(),
            path_changes,
        );
    }
    let paths_us = dur_us(t_paths.elapsed());

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
            compacted,
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
            "IDXDELTA-TIMING apply: loop + children/ext rebuild + trigram delta \
             (paths incremental; trigram_us≈0 unless compacted)"
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
