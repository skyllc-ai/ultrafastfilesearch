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
use crate::cache::journal_loop::JournalLoopHandle;

/// Helper for [`IndexManager::attach_journal_handle`] and
/// [`IndexManager::drain_journal_handles`]: lock the journal-handle
/// map, recovering from poison.  Mirrors the existing
/// `lock_in_flight_promotes` / `verify_shutdown_nonce` poison
/// handling pattern in this crate (per the `in_flight_promotes`
/// field doc rationale).
fn lock_journal_handles(
    map: &std::sync::Mutex<std::collections::HashMap<char, JournalLoopHandle>>,
) -> std::sync::MutexGuard<'_, std::collections::HashMap<char, JournalLoopHandle>> {
    map.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    pub(crate) async fn handle_journal_refresh(&self, letter: char, reason: &str) -> bool {
        // Heavy work: live MFT read + USN replay + compact rebuild.
        // Runs on the blocking pool so the runtime's worker threads
        // stay free to service search RPCs concurrent with the refresh.
        // The closure enters [`BackgroundIoScope`] so the per-letter
        // syscalls run at Windows `THREAD_MODE_BACKGROUND_BEGIN`
        // priority — yielding to any foreground RPC handler under
        // disk contention.  RAII via `_bg_scope` ensures the matching
        // `_END` fires even if the underlying loader panics.  No-op
        // on Mac/Linux.  Mirrors the deleted Phase-5
        // `refresh_usn_for_warm_shards` per-letter wrapping.
        let background_io = Arc::clone(&self.background_io);
        let body_result = tokio::task::spawn_blocking(move || {
            let _bg_scope = crate::cache::background_io::BackgroundIoScope::enter(background_io);
            uffs_core::compact_loader::load_drive_with_usn_refresh(letter)
        })
        .await;

        let Some(body) = unwrap_refreshed_body(body_result, letter, reason) else {
            return false;
        };
        self.apply_journal_body(letter, reason, body).await
    }

    /// Phase 7 activation: store a [`JournalLoopHandle`] for `letter`
    /// in the per-letter handle map.
    ///
    /// Called once per loaded drive from
    /// `lib.rs::spawn_journal_loops_for_warm_shards` after each
    /// [`crate::cache::journal_loop::spawn_journal_loop`] call.  If
    /// a handle is already present for `letter` (defensive — never
    /// happens in the production flow because each drive is loaded
    /// exactly once before this method fires), the previous handle
    /// is **cancelled** so the orphaned loop tears down cleanly.
    pub(crate) fn attach_journal_handle(&self, letter: char, handle: JournalLoopHandle) {
        let mut guard = lock_journal_handles(&self.journal_handles);
        if let Some(previous) = guard.insert(letter, handle) {
            tracing::warn!(
                target: "shard.journal",
                drive = %letter,
                "Replacing existing journal-loop handle (defensive cancel of previous loop)",
            );
            // Best-effort cancel: send the watch signal and forget
            // the JoinHandle.  The previous loop converges on its
            // next tick (~500 ms) and is reaped by the runtime.
            let _join = previous.cancel();
        }
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
