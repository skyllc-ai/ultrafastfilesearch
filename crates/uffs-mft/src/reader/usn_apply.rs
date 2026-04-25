// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shared helpers for applying USN-Journal updates to a cached
//! [`crate::index::MftIndex`].
//!
//! Both the single-drive cached reader (`reader::index_cache`) and the
//! multi-drive cached reader (`reader::multi_drive::index`) need to:
//!
//! 1. Decide whether the cached index can be incrementally updated or whether
//!    the journal state forces a full rebuild.
//! 2. After applying USN deletes, issue **targeted** MFT reads for the
//!    non-delete FRSes so file/dir mutations are reflected in the index.
//! 3. Rebuild derived structures (extension index + tree metrics) when anything
//!    changed.
//! 4. Persist the updated index plus the new USN checkpoint to cache.
//!
//! Keeping this logic in one place avoids drift between the two reader
//! paths and lets both functions stay below clippy's
//! `cognitive_complexity` bar without carrying duplicated bodies.

#[cfg(windows)]
use tracing::{debug, info, warn};

#[cfg(windows)]
use crate::cache::save_to_cache;
#[cfg(windows)]
use crate::index::{IndexHeader, MftIndex, UsnApplyStats};
#[cfg(windows)]
use crate::platform::VolumeHandle;
#[cfg(windows)]
use crate::usn::UsnJournalInfo;

/// Outcome of inspecting a drive's USN-journal state against the cached
/// `IndexHeader`.
///
/// Returned by [`classify_usn_state`] so cached-reader call sites can
/// dispatch with a single `match` rather than the deeply-nested early-
/// return ladders the original implementations used.
#[cfg(windows)]
#[derive(Debug)]
pub(super) enum UsnDecision {
    /// Cache is authoritative; return the loaded index as-is.
    UseCached,
    /// Cache is invalid (journal id changed or wrapped); rebuild from disk.
    Rebuild,
    /// Read records from `start_usn` against `journal_id` and apply them.
    Apply {
        /// Active journal id at the time of inspection.
        journal_id: u64,
        /// USN at which to resume reading (the cached `next_usn`).
        start_usn: i64,
    },
}

/// Returns `true` when the USN journal's current state means the cached
/// index can no longer be incrementally updated and must be rebuilt.
///
/// Two scenarios trigger a rebuild:
/// - **Journal-id mismatch**: NTFS reset the USN journal (e.g. via `fsutil usn
///   deletejournal`), so the cached `next_usn` checkpoint addresses an extinct
///   ID space.
/// - **Journal wrap**: the cached `next_usn` is older than the current
///   journal's `first_usn`, meaning entries from the gap were truncated.
///
/// Each branch logs at `info` level so cache-rebuild causes are visible
/// in production traces; callers only need to act on the boolean.
#[cfg(windows)]
pub(super) fn usn_journal_invalidates_cache(
    drive: char,
    header: &IndexHeader,
    current_info: &UsnJournalInfo,
) -> bool {
    if header.usn_journal_id != 0 && current_info.journal_id != header.usn_journal_id {
        info!(
            drive = %drive,
            cached_journal_id = header.usn_journal_id,
            current_journal_id = current_info.journal_id,
            "🔄 USN Journal ID changed - rebuilding index"
        );
        return true;
    }

    if header.next_usn < current_info.first_usn {
        info!(
            drive = %drive,
            cached_usn = header.next_usn,
            first_usn = current_info.first_usn,
            "🔄 USN Journal wrapped - rebuilding index"
        );
        return true;
    }

    false
}

/// Classify the journal state against `header`.
///
/// Returns the dispatch decision the caller should act on, plus logs the
/// cause when the cache is rejected.  Caller is responsible for handling
/// the `Err` arm of `query_usn_journal` — a missing journal is treated as
/// [`UsnDecision::UseCached`] so the cached index is still served.
#[cfg(windows)]
pub(super) fn classify_usn_state(
    drive: char,
    header: &IndexHeader,
    current_info: &UsnJournalInfo,
) -> UsnDecision {
    if usn_journal_invalidates_cache(drive, header, current_info) {
        return UsnDecision::Rebuild;
    }

    let start_usn = header.next_usn;
    if start_usn >= current_info.next_usn {
        debug!(drive = %drive, usn = start_usn, "✅ Index is already up to date");
        return UsnDecision::UseCached;
    }

    UsnDecision::Apply {
        journal_id: current_info.journal_id,
        start_usn,
    }
}

/// Issue targeted MFT reads for the FRSes the delete pass left behind so
/// non-delete changes can be folded into `index`.  Failures are logged but
/// do not abort the wider USN-update flow — the next refresh will pick up
/// the missed entries.
#[cfg(windows)]
pub(super) fn apply_targeted_usn_reads(
    drive: char,
    handle: &VolumeHandle,
    index: &mut MftIndex,
    frs_to_read: &[u64],
    stats: &mut UsnApplyStats,
) {
    if frs_to_read.is_empty() {
        return;
    }

    debug!(
        drive = %drive,
        count = frs_to_read.len(),
        "🎯 Reading targeted MFT records for USN changes"
    );

    match crate::usn::read_targeted_frs_records(handle, index, frs_to_read) {
        Ok(count) => {
            stats.targeted_reads = count;
            debug!(
                drive = %drive,
                targeted_reads = count,
                total_requested = frs_to_read.len(),
                "✅ Targeted MFT reads complete"
            );
        }
        Err(error) => {
            warn!(
                drive = %drive,
                error = %error,
                "⚠️ Targeted MFT reads failed — records may have incomplete data"
            );
        }
    }
}

/// Rebuilds the extension index and recomputes tree metrics when phase 1+2
/// actually mutated `index`.  When nothing changed we skip both passes;
/// the cached structures are still valid.
#[cfg(windows)]
pub(super) fn rebuild_derived_after_usn(drive: char, index: &mut MftIndex, stats: &UsnApplyStats) {
    let had_changes = stats.deleted > 0 || stats.targeted_reads > 0;
    if had_changes {
        debug!(drive = %drive, "🔨 Rebuilding extension index after USN updates");
        index.build_extension_index();
        debug!(drive = %drive, "🔨 Recomputing tree metrics after USN updates");
        index.compute_tree_metrics();
    }

    info!(
        drive = %drive,
        targeted_reads = stats.targeted_reads,
        deleted = stats.deleted,
        skipped = stats.skipped,
        "✅ USN updates applied"
    );
}

/// Persists `index` plus the `(journal_id, next_usn)` checkpoint to cache.
/// Errors are logged at warn level only; cache failure must not break the
/// in-memory index that callers continue to use.
#[cfg(windows)]
pub(super) fn persist_usn_checkpoint(
    drive: char,
    handle: &VolumeHandle,
    index: &MftIndex,
    journal_id: u64,
    next_usn: i64,
) {
    let volume_serial = handle.volume_data().volume_serial_number;

    if let Err(error) = save_to_cache(index, drive, volume_serial, journal_id, next_usn) {
        warn!(drive = %drive, error = %error, "⚠️ Failed to update cache");
    } else {
        debug!(
            drive = %drive,
            next_usn,
            "💾 Cache updated with new USN checkpoint"
        );
    }
}
