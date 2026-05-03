// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-shard USN journal refresh path (Phase 7 activation).
//!
//! The single [`IndexManager::handle_journal_refresh`] method drives
//! a full per-shard body refresh in response to a save / wrap signal
//! from the per-shard journal loop's
//! [`crate::cache::journal_sink::RegistryPatchSink`].
//!
//! ## Why a full refresh and not a surgical patch
//!
//! Phase 7-A landed [`crate::cache::ShardEntry::apply_usn_patch_to_body`]
//! which patches an existing `DriveCompactIndex` in-place from a
//! [`uffs_mft::usn::FileChange`] batch.  But that surgical path
//! requires a `frs_to_compact: &[u32]` mapping that is **not** stored
//! on `DriveCompactIndex` (`CompactRecord` has no `frs` field; the
//! mapping is built once at index-build time from `MftIndex.frs_to_idx`
//! and discarded).  Persisting the mapping is a Phase 8 follow-up
//! (ticket: surgical USN body-patch end-to-end).
//!
//! For activation, the cleanest option is to reuse the Phase-5
//! infrastructure unchanged: every save / wrap trigger drives a full
//! [`uffs_core::compact_loader::load_drive_with_usn_refresh`] which
//! does an MFT read + USN replay + compact rebuild.  Heavy (~2-7 s per
//! drive on a 7-drive box) but correct, and the per-shard
//! event-threshold + age-threshold cadence still cuts the global
//! 5-min tick down to a per-shard schedule that activates only when
//! a drive actually saw churn.
//!
//! ## Mac/Linux behaviour
//!
//! The underlying [`uffs_core::compact_loader::load_drive_with_usn_refresh`]
//! errors out by design on non-Windows targets (USN journals are
//! NTFS-only).  This method preserves that contract: the err arm
//! warn-logs the per-drive failure and returns `false` so the caller
//! can record the no-op without aborting the loop.  In production
//! the Mac/Linux journal loop wires [`MacStubJournalSource`] (always
//! empty) so the threshold-driven save trigger never fires anyway —
//! the err arm is defensive, not a hot path.
//!
//! [`MacStubJournalSource`]: crate::cache::journal_loop::sources::MacStubJournalSource

use alloc::sync::Arc;

use super::IndexManager;

impl IndexManager {
    /// Drive a full per-shard USN refresh + body swap for `letter`.
    ///
    /// Called by the [`crate::cache::journal_sink::RegistryPatchSink`]
    /// applier task on every `Save` / `Wrap` message from the
    /// per-shard journal loop.  `reason` is a diagnostic string
    /// (`"events-exceeded"`, `"age-elapsed"`, `"journal-wrapped"`)
    /// surfaced in the success/failure log so operators can grep
    /// per-drive refresh churn by trigger type.
    ///
    /// ## Returns
    ///
    /// * `true` when the body was successfully Arc-swapped into the registry —
    ///   the call site MAY log a per-drive success event but the success log is
    ///   already emitted here at info-level.
    ///
    /// * `false` on every failure mode:
    ///
    ///   - Underlying
    ///     [`uffs_core::compact_loader::load_drive_with_usn_refresh`] err
    ///     (Windows: live MFT read failed, USN journal apply failed, volume
    ///     revoked; Mac/Linux: always — USN journals are NTFS-only by design).
    ///   - The blocking task aborted before producing a result.
    ///   - Registry race — the shard demoted to `Parked` / `Cold` between the
    ///     per-shard event-threshold trigger and this swap.  The next promote
    ///     on this letter will pick up the fresher snapshot via
    ///     [`crate::cache::body_loader::DiskBodyLoader`].
    ///
    /// `false` is **not** an error to propagate — the caller logs
    /// at warn/debug level depending on whether the failure was a
    /// real I/O error or just a benign demote race.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 7 activation forward reference; the \
                      production caller is the `RegistryPatchSink` \
                      applier task in `crate::cache::journal_sink` \
                      which is constructed by `lib.rs::\
                      spawn_journal_loops_for_warm_shards` (commit \
                      A4).  Until A4 wires the spawner this method \
                      is reachable only from the sink's `cfg(test)` \
                      lifecycle tests."
        )
    )]
    pub(crate) async fn handle_journal_refresh(&self, letter: char, reason: &str) -> bool {
        // Heavy work: live MFT read + USN replay + compact rebuild.
        // Runs on the blocking pool so the runtime's worker threads
        // stay free to service search RPCs concurrent with the refresh.
        let body_result = tokio::task::spawn_blocking(move || {
            uffs_core::compact_loader::load_drive_with_usn_refresh(letter)
        })
        .await;

        let Some(body) = unwrap_refreshed_body(body_result, letter, reason) else {
            return false;
        };
        self.apply_journal_body(letter, reason, body).await
    }

    /// Per-letter write-lock swap of a freshly-loaded body Arc.
    ///
    /// Extracted from [`Self::handle_journal_refresh`] so the parent
    /// stays under clippy's strict-gate cognitive-complexity ceiling.
    /// `replace_warm_body` returns `None` when the shard demoted to
    /// `Parked` / `Cold` between the threshold trigger and this swap
    /// — a benign race that we log at debug-level and absorb.
    async fn apply_journal_body(
        &self,
        letter: char,
        reason: &str,
        body: Arc<uffs_core::compact::DriveCompactIndex>,
    ) -> bool {
        let mut guard = self.index.write().await;
        let Some(new_registry) = guard.replace_warm_body(letter, body) else {
            tracing::debug!(
                target: "shard.journal",
                drive = %letter,
                reason,
                "Shard demoted between trigger and swap; no-op",
            );
            return false;
        };
        *guard = Arc::new(new_registry);
        drop(guard);
        self.bump_index_version();
        tracing::info!(
            target: "shard.journal",
            drive = %letter,
            reason,
            "USN refresh applied",
        );
        true
    }
}

/// Drain a `tokio::task::JoinHandle` result into either the freshly-
/// loaded body Arc or `None` (with the per-failure warn-log already
/// emitted).
///
/// Extracted from [`IndexManager::handle_journal_refresh`] so the
/// parent stays under clippy's strict-gate cognitive-complexity
/// ceiling.  Two failure modes share the `None` return:
///
/// * `Ok(Err(err))` — the underlying
///   [`uffs_core::compact_loader::load_drive_with_usn_refresh`] surfaced an
///   error (Windows: live MFT read / USN apply failed; Mac/Linux: always — the
///   helper's `cfg(not(windows))` arm bails).
/// * `Err(join_err)` — the blocking task itself aborted before producing a
///   result (panic, runtime shutdown).
fn unwrap_refreshed_body(
    body_result: Result<
        anyhow::Result<(
            uffs_core::compact::DriveCompactIndex,
            uffs_core::compact_loader::RefreshTiming,
        )>,
        tokio::task::JoinError,
    >,
    letter: char,
    reason: &str,
) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
    match body_result {
        Ok(Ok((body, _timing))) => Some(Arc::new(body)),
        Ok(Err(err)) => {
            tracing::warn!(
                target: "shard.journal",
                drive = %letter,
                reason,
                error = %err,
                "USN refresh failed; shard kept previous body",
            );
            None
        }
        Err(join_err) => {
            tracing::warn!(
                target: "shard.journal",
                drive = %letter,
                reason,
                error = %join_err,
                "USN refresh blocking task aborted; shard kept previous body",
            );
            None
        }
    }
}
