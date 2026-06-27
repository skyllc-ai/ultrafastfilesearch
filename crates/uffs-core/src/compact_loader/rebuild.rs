// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Post-apply derived-index maintenance for [`super::apply_usn_patch`].
//!
//! After the per-change loop mutates the record columns + `frs_to_compact`,
//! this overlays the batch onto the base ∪ delta indexes
//! (incremental-index-maintenance: trigram + extension + children) and
//! refreshes the touched records' `path_len`, so newly created / renamed /
//! deleted files appear in tree traversal AND trigram / `--ext` search — all in
//! O(changed), with an occasional O(total) compaction folding the delta back
//! into fresh bases. Extracted from `compact_loader.rs` to keep that file under
//! the workspace 800-LOC policy and to house this post-loop concern as one
//! unit.

use super::PatchStats;
use crate::compact::{DriveCompactIndex, PathChange, update_path_lengths_incremental};

/// Above this many touched records, the per-change incremental path update
/// loses to a single O(total) BFS (each create/rename re-walks parents), so we
/// fall back to the full [`crate::compact::compute_path_lengths`].  Sized well
/// above a normal USN poll batch; the 50k disk-save threshold is the practical
/// ceiling on a single apply anyway.
const FULL_PATH_RECOMPUTE_THRESHOLD: usize = 50_000;

/// Overlay the batch onto the base ∪ delta indexes and refresh the touched
/// records' `path_len`.  Returns `true` if the delta crossed the compaction
/// threshold and the bases were refolded this call.
///
/// The order matters: the trigram / extension / children overlay
/// ([`DriveCompactIndex::apply_index_delta`]) runs FIRST so the
/// directory-rename subtree walk in the path refresh below sees the batch's new
/// children (creates / moves into a renamed directory).
pub(super) fn rebuild_derived(
    drive: &mut DriveCompactIndex,
    path_changes: &[PathChange],
    tombstones: &[u32],
) -> bool {
    let compacted = drive.apply_index_delta(path_changes, tombstones);

    // Refresh path_len only for the records this batch touched (O(changed)). An
    // empty change set (e.g. a delete-only batch) is a no-op over an empty
    // slice; the full O(total) BFS is reserved for the cold-load builder, with
    // an apply-time fallback only for a pathologically huge batch where the
    // per-record re-walk loses to one BFS.
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

    compacted
}

/// Emit the per-batch USN-apply summary (how the poll mutated the index, the
/// wall-clock cost, and whether it triggered a delta compaction) at DEBUG.
pub(super) fn log_batch_summary(
    drive: &DriveCompactIndex,
    changes: usize,
    stats: &PatchStats,
    compacted: bool,
    apply_us: u128,
) {
    tracing::debug!(
        drive = %drive.letter,
        changes,
        created = stats.created,
        deleted = stats.deleted,
        renamed = stats.renamed,
        skipped = stats.skipped,
        records = drive.records.len(),
        ext_index_entries = drive.ext_index.total_entries(),
        compacted,
        apply_us,
        "usn apply: batch applied"
    );
}
