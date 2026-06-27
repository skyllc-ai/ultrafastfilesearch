// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 8-D — operator-driven `forget` RPC.
//!
//! Pairs the registry-eviction half of `forget` with the on-disk
//! cleanup half ([`crate::cache::cache_cleaner::CacheCleaner`]).
//!
//! Three-phase orchestration mirrors the
//! [`crate::index::IndexManager::hibernate_shards`] /
//! [`crate::index::IndexManager::preload_drive`] pattern:
//!
//! 1. **Read-lock detect.**  A single `self.index.read()` enumerates the
//!    `(letter, current_tier)` tuples for every drive in the request.  If
//!    `force == false` and any drive is non-`Cold`, the entire request is
//!    refused with [`ForgetOutcomeOrBusy::Busy`] — handler maps this to
//!    [`uffs_client::protocol::ERR_DRIVE_BUSY`] so a typo on one of five drives
//!    doesn't accidentally forget the other four.
//! 2. **Optional auto-hibernate (force only).**  Each non-`Cold` drive is
//!    demoted to `Cold` via
//!    [`crate::cache::registry::ShardRegistry::demote_letter_with_reason`]
//!    tagged with [`crate::cache::registry::DemoteReason::OperatorHibernate`].
//!    The pin is cleared as a side effect (the rebuilt `ShardEntry` starts with
//!    `pin_until_ms = 0`).
//! 3. **Evict + clean.**  The drive is removed from the registry via
//!    [`crate::cache::ShardRegistry::remove`] and the on-disk cache files are
//!    unlinked via the injected [`crate::cache::cache_cleaner::CacheCleaner`].
//!    Per-drive classification:
//!
//!    * `freed_bytes > 0` ⇒ [`ForgetOutcome::forgotten`].
//!    * `freed_bytes == 0` and no errors ⇒ [`ForgetOutcome::already_absent`]
//!      (idempotent re-run after a previous successful `forget`).
//!    * Any per-path errors ⇒ [`ForgetOutcome::errors`] entries prefixed with
//!      the drive letter; the drive still goes into `forgotten` if anything was
//!      unlinked, otherwise into `already_absent` only when there were truly no
//!      errors.

use alloc::sync::Arc;

use super::IndexManager;
use crate::cache::registry::DemoteReason;
use crate::cache::{ShardState, unix_now_ms};

/// Outcome of a successful [`IndexManager::forget_drives`] call.
///
/// Each drive in the input request lands in exactly one of the response's
/// `forgotten` or `already_absent` lists (unless an I/O error pushed it into
/// `errors` only).  Mirrors the
/// [`uffs_client::protocol::response::ForgetResponse`] wire shape so
/// the handler can build the wire response with one
/// `serde::Serialize` call.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ForgetOutcome {
    /// Drives whose cache files were deleted in this call (any
    /// `freed_bytes > 0`).
    pub forgotten: Vec<uffs_mft::platform::DriveLetter>,
    /// Drives that had no cache files on disk and weren't loaded in
    /// the registry (idempotent no-op, e.g. a re-run after a previous
    /// successful `forget`).
    pub already_absent: Vec<uffs_mft::platform::DriveLetter>,
    /// Cumulative bytes freed across every successfully-forgotten
    /// drive.
    pub freed_bytes: u64,
    /// Per-drive errors, prefixed with the drive letter
    /// (`"Z: <path>: permission denied"`).
    pub errors: Vec<String>,
}

/// Result of the read-lock detection phase.
///
/// `Busy` is the all-or-nothing refusal path: at least one
/// requested drive is non-`Cold` and the caller didn't pass
/// `force = true`, so the handler returns
/// [`uffs_client::protocol::ERR_DRIVE_BUSY`] with the full list in
/// the message.  Operators get a single actionable error rather
/// than discovering halfway through that 4 of 5 drives were
/// already freed.
///
/// `Ok` carries the populated [`ForgetOutcome`] — including any
/// per-drive I/O errors from the cache-cleaner phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForgetOutcomeOrBusy {
    /// Forget completed; some drives may still have I/O errors
    /// surfaced under [`ForgetOutcome::errors`].
    Ok(ForgetOutcome),
    /// All-or-nothing refusal: one or more drives are non-`Cold`
    /// and `force` was `false`.  Vec carries `(letter, current_tier)`
    /// for every refused drive in registry order.
    Busy(Vec<(uffs_mft::platform::DriveLetter, ShardState)>),
}

impl IndexManager {
    /// Phase 8-D — multi-drive `forget` orchestration.
    ///
    /// Empty `drives` is rejected at the handler layer with
    /// [`uffs_client::protocol::ERR_INVALID_PARAMS`]; this method
    /// trusts the caller to have already enforced that invariant.
    ///
    /// `force = false` (default) refuses the whole request when any
    /// drive is non-`Cold`.  `force = true` auto-hibernates every
    /// non-`Cold` drive (clearing pins implicitly via the registry
    /// rebuild) before evicting + cleaning.
    ///
    /// Returns [`ForgetOutcomeOrBusy::Busy`] when the all-or-nothing
    /// refusal triggers; otherwise [`ForgetOutcomeOrBusy::Ok`] with
    /// the populated [`ForgetOutcome`].
    pub(crate) async fn forget_drives(
        &self,
        drives: &[uffs_mft::platform::DriveLetter],
        force: bool,
    ) -> ForgetOutcomeOrBusy {
        // ── Phase 1: read-lock detect ──────────────────────────────
        //
        // Enumerate (letter, current_tier) tuples for every requested
        // drive.  Drives not in the registry record `None` so the
        // cleaner phase still gets a chance to remove stale on-disk
        // files (idempotent re-run semantics).
        let mut snapshots: Vec<(uffs_mft::platform::DriveLetter, Option<ShardState>)> =
            Vec::with_capacity(drives.len());
        let mut busy: Vec<(uffs_mft::platform::DriveLetter, ShardState)> = Vec::new();
        let guard = self.index.read().await;
        for &requested in drives {
            let found = guard
                .iter()
                .find(|shard| shard.drive == requested)
                .map(|shard| (shard.drive, shard.state()));
            match found {
                Some((drive, state)) => {
                    if !force && state != ShardState::Cold {
                        busy.push((drive, state));
                    }
                    snapshots.push((drive, Some(state)));
                }
                None => {
                    // Use the requested letter verbatim — case
                    // preserved for the operator audit trail.
                    snapshots.push((requested, None));
                }
            }
        }
        // Explicit drop to release the read lock before the
        // (potentially long-lived) write-lock work below.
        drop(guard);

        if !busy.is_empty() {
            return ForgetOutcomeOrBusy::Busy(busy);
        }

        // ── Phase 2: optional auto-hibernate (force only) ──────────
        //
        // Demote every non-Cold drive to Cold first so the eviction
        // step below sees a stable shard.  The registry rebuild
        // implicitly clears any pin (the new `ShardEntry` starts at
        // `pin_until_ms = 0`).  Each demote bumps `index_version`
        // independently — same pattern as `hibernate_shards`'s
        // per-letter `demote_letter_with_reason` calls.
        if force {
            self.auto_hibernate_for_forget(&snapshots).await;
        }

        // ── Phase 3: per-drive evict + clean ───────────────────────
        let mut outcome = ForgetOutcome::default();
        for &(letter, _) in &snapshots {
            self.forget_one(letter, &mut outcome).await;
        }
        ForgetOutcomeOrBusy::Ok(outcome)
    }

    /// Auto-hibernate every non-`Cold` drive in `snapshots` to
    /// `Cold` so the eviction phase below works against a stable
    /// shard.  Caller must have already verified `force = true`.
    ///
    /// One write-lock acquisition per non-Cold drive (matches
    /// `hibernate_shards`'s per-letter rebuild pattern).  Pins are
    /// cleared implicitly — the rebuilt `ShardEntry` starts at
    /// `pin_until_ms = 0`.
    async fn auto_hibernate_for_forget(
        &self,
        snapshots: &[(uffs_mft::platform::DriveLetter, Option<ShardState>)],
    ) {
        for &(drive, state_opt) in snapshots {
            let Some(state) = state_opt else {
                continue;
            };
            if state == ShardState::Cold {
                continue;
            }
            let mut guard = self.index.write().await;
            let Some(new_registry) = guard.demote_letter_with_reason(
                drive,
                ShardState::Cold,
                DemoteReason::OperatorHibernate,
            ) else {
                // Race: shard moved to Cold or vanished between
                // detect and write-lock.  Either way, the eviction
                // phase below will pick up where we left off.
                continue;
            };
            *guard = Arc::new(new_registry);
            drop(guard);
            self.bump_index_version();
        }
    }

    /// Single-drive evict + clean step.
    ///
    /// Removes the shard from the registry (no-op if it wasn't
    /// loaded) and unlinks every on-disk cache artefact via the
    /// injected [`crate::cache::cache_cleaner::CacheCleaner`].
    /// Classifies the result into [`ForgetOutcome::forgotten`] /
    /// [`ForgetOutcome::already_absent`] / [`ForgetOutcome::errors`]
    /// per the rules documented on [`ForgetOutcome`] above.
    async fn forget_one(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        outcome: &mut ForgetOutcome,
    ) {
        // Step 1: evict from registry (idempotent — `remove` is a
        // no-op when the letter isn't present).
        let mut guard = self.index.write().await;
        let new_registry = guard.remove(letter);
        *guard = Arc::new(new_registry);
        drop(guard);
        self.bump_index_version();

        // Step 2: unlink on-disk cache files (idempotent — missing
        // files are silent no-ops in the platform cleaner).
        let (freed, errors) = self.cache_cleaner.forget(letter);

        // Step 3: classify.
        outcome.freed_bytes = outcome.freed_bytes.saturating_add(freed);
        for err in errors {
            outcome.errors.push(format!("{letter}: {err}"));
        }
        if freed > 0 {
            outcome.forgotten.push(letter);
        } else {
            // No bytes freed and no errors: idempotent no-op.  No
            // bytes freed but errors present: the per-drive errors
            // already capture the failure — don't double-report by
            // adding the letter to `already_absent`.
            //
            // The wire contract is "every input drive lands in
            // exactly one of forgotten/already_absent unless errors
            // captured it"; the handler reads `errors` separately
            // so the operator sees the failure.
            let had_errors = outcome
                .errors
                .last()
                .is_some_and(|err| err.starts_with(&format!("{letter}:")));
            if !had_errors {
                outcome.already_absent.push(letter);
            }
        }

        // Tracing breadcrumb so operators grepping `forget` can
        // correlate registry eviction with cache-file deletion.
        // `unix_now_ms()` is sampled here only for the event
        // timestamp; the orchestration above doesn't need a clock
        // read.
        tracing::info!(
            target: "shard.transition",
            drive = %letter,
            freed_bytes = freed,
            ts_ms = unix_now_ms(),
            reason = "operator-forget",
            "forget: drive evicted from registry and cache files unlinked",
        );
    }
}
