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
        start_usn: crate::usn::Usn,
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
    drive: crate::platform::DriveLetter,
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
            cached_usn = %header.next_usn,
            first_usn = %current_info.first_usn,
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
    drive: crate::platform::DriveLetter,
    header: &IndexHeader,
    current_info: &UsnJournalInfo,
) -> UsnDecision {
    if usn_journal_invalidates_cache(drive, header, current_info) {
        return UsnDecision::Rebuild;
    }

    let start_usn = header.next_usn;
    if start_usn >= current_info.next_usn {
        debug!(drive = %drive, usn = %start_usn, "✅ Index is already up to date");
        return UsnDecision::UseCached;
    }

    UsnDecision::Apply {
        journal_id: current_info.journal_id,
        start_usn,
    }
}

/// Default ceiling, in seconds, for serving a freshly-loaded cache whose
/// drive has **no active USN journal**.  Without a journal there is no
/// incremental-update mechanism, so a cache older than this is rebuilt from
/// a full MFT read rather than served stale.  Chosen to track the daemon's
/// 300 s USN refresh cadence (`shards.usn_refresh_interval_secs`).
#[cfg(any(windows, test))]
const NO_JOURNAL_MAX_AGE_SECS_DEFAULT: u64 = 300;

/// Parse a raw `UFFS_NO_JOURNAL_MAX_AGE_SECS` value into an effective ceiling,
/// falling back to [`NO_JOURNAL_MAX_AGE_SECS_DEFAULT`] when the value is absent
/// or not a valid `u64`.  Split from [`no_journal_max_age_secs`] so the
/// override-parsing logic is host-testable without mutating process env.
#[cfg(any(windows, test))]
fn parse_no_journal_max_age(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(NO_JOURNAL_MAX_AGE_SECS_DEFAULT)
}

/// Effective no-journal cache-age ceiling, honouring the
/// `UFFS_NO_JOURNAL_MAX_AGE_SECS` environment override.  Falls back to
/// [`NO_JOURNAL_MAX_AGE_SECS_DEFAULT`] when the variable is unset or
/// unparseable.
#[cfg(windows)]
pub(super) fn no_journal_max_age_secs() -> u64 {
    parse_no_journal_max_age(
        std::env::var("UFFS_NO_JOURNAL_MAX_AGE_SECS")
            .ok()
            .as_deref(),
    )
}

/// Pure boundary predicate: a no-journal cache strictly older than
/// `max_age_seconds` must be rebuilt; at or under the ceiling it is served
/// as-is.  Extracted so the off-by-one boundary (`>`, not `>=`) is pinned by a
/// host test independent of the Windows-only [`UsnDecision`] mapping.
#[cfg(any(windows, test))]
const fn no_journal_cache_is_stale(age_seconds: u64, max_age_seconds: u64) -> bool {
    age_seconds > max_age_seconds
}

/// Decide what to do with a freshly-loaded cache when the drive's USN
/// journal is **unavailable** (e.g. `query_usn_journal` returned
/// os error 1179 — "the volume change journal is not active").
///
/// Without a journal there is no incremental refresh path, so the cache's
/// own age is the only freshness evidence:
/// - within [`no_journal_max_age_secs`] → [`UsnDecision::UseCached`] (serve
///   as-is; a full rescan would dominate latency for a sub-window cache).
/// - older than the ceiling → [`UsnDecision::Rebuild`] (full MFT read) so files
///   created since the snapshot cannot stay invisible until the long safety-net
///   TTL elapses.
#[cfg(windows)]
pub(super) fn classify_without_journal(
    drive: crate::platform::DriveLetter,
    age_seconds: u64,
) -> UsnDecision {
    let max_age = no_journal_max_age_secs();
    if no_journal_cache_is_stale(age_seconds, max_age) {
        warn!(
            drive = %drive,
            age_seconds,
            max_age_seconds = max_age,
            "🔄 USN Journal unavailable and cache past no-journal window - rebuilding index"
        );
        UsnDecision::Rebuild
    } else {
        warn!(
            drive = %drive,
            age_seconds,
            max_age_seconds = max_age,
            "⚠️ USN Journal unavailable - serving cache within no-journal window"
        );
        UsnDecision::UseCached
    }
}

/// Issue targeted MFT reads for the FRSes the delete pass left behind so
/// non-delete changes can be folded into `index`.  Failures are logged but
/// do not abort the wider USN-update flow — the next refresh will pick up
/// the missed entries.
#[cfg(windows)]
pub(super) fn apply_targeted_usn_reads(
    drive: crate::platform::DriveLetter,
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
pub(super) fn rebuild_derived_after_usn(
    drive: crate::platform::DriveLetter,
    index: &mut MftIndex,
    stats: &UsnApplyStats,
) {
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
    drive: crate::platform::DriveLetter,
    handle: &VolumeHandle,
    index: &MftIndex,
    journal_id: u64,
    next_usn: crate::usn::Usn,
) {
    let volume_serial = handle.volume_data().volume_serial_number;

    if let Err(error) = save_to_cache(index, drive, volume_serial, journal_id, next_usn) {
        warn!(drive = %drive, error = %error, "⚠️ Failed to update cache");
    } else {
        debug!(
            drive = %drive,
            next_usn = %next_usn,
            "💾 Cache updated with new USN checkpoint"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NO_JOURNAL_MAX_AGE_SECS_DEFAULT, no_journal_cache_is_stale, parse_no_journal_max_age,
    };

    #[test]
    fn no_journal_cache_below_ceiling_is_served() {
        assert!(!no_journal_cache_is_stale(
            0,
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT
        ));
        assert!(!no_journal_cache_is_stale(
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT - 1,
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT,
        ));
    }

    #[test]
    fn no_journal_cache_at_ceiling_is_not_stale() {
        // Boundary is strictly `>`, so age == ceiling is still fresh.
        assert!(!no_journal_cache_is_stale(
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT,
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT,
        ));
    }

    #[test]
    fn no_journal_cache_above_ceiling_is_stale() {
        // One second past the ceiling forces a rebuild.
        assert!(no_journal_cache_is_stale(
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT + 1,
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT,
        ));
    }

    #[test]
    fn parse_no_journal_max_age_uses_default_when_absent() {
        assert_eq!(
            parse_no_journal_max_age(None),
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT
        );
    }

    #[test]
    fn parse_no_journal_max_age_uses_default_when_unparseable() {
        assert_eq!(
            parse_no_journal_max_age(Some("not-a-number")),
            NO_JOURNAL_MAX_AGE_SECS_DEFAULT
        );
    }

    #[test]
    fn parse_no_journal_max_age_honours_override() {
        assert_eq!(parse_no_journal_max_age(Some("600")), 600);
        assert_eq!(parse_no_journal_max_age(Some("0")), 0);
    }
}
