// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Operator-driven memory-tiering operations for [`IndexManager`].
//!
//! Phase 8 commit-level decomposition (sub-phases 8-B / 8-C):
//!
//! * [`Self::hibernate_shards`] — sub-phase 8-B.  Walks every shard in the
//!   registry (or a caller-supplied subset) and demotes each non-`Cold` shard
//!   to `Cold` in a single write-lock batch. Mirrors the orchestration shape of
//!   [`Self::demote_idle_shards`] (Phase 3 Commit D) — read-lock detect →
//!   write-lock atomic batch → single `bump_index_version`. Hibernate
//!   explicitly clears tier pins by virtue of rebuilding the shard as `Cold`
//!   (the new `ShardEntry` starts with `pin_until_ms = 0`).
//!
//! * [`Self::preload_drive`] — sub-phase 8-C.  Promotes a single drive to `Hot`
//!   via the existing per-letter single-flight body-load + `Prefetch::hint`
//!   machinery ([`Self::load_or_join_in_flight`] from [`super::dispatch`]) and
//!   arms the tier pin for `pin_minutes` minutes.  Source state can be `Cold`
//!   (loads body from encrypted compact cache), `Parked` (loads body, dropping
//!   the parked bloom + trie), `Warm` (clones the existing body), or `Hot`
//!   (skips the rebuild and atomically extends the pin).
//!
//! Why a sibling file (instead of folding into
//! [`super::transitions`]): the two background controllers in
//! `transitions.rs` (`demote_idle_shards`, `cascade_demote_one_step`)
//! are policy-driven daemon-internal decisions, while these two
//! methods are operator-driven entry points reached over the wire.
//! Keeping the two clusters separate makes the audit boundary
//! obvious — operator overrides go here, controller automation goes
//! there — and keeps `transitions.rs` from growing past the 800-LOC
//! ceiling.

use alloc::sync::Arc;

use super::IndexManager;
use crate::cache::registry::DemoteReason;
use crate::cache::{ShardState, unix_now_ms};

/// Outcome of a [`IndexManager::hibernate_shards`] call.
///
/// Each `Vec<uffs_mft::platform::DriveLetter>` lists the drives whose pre-call
/// tier matched the field name and that are now `Cold` (or, for `already_cold`,
/// were already there).  The handler maps this 1:1 onto
/// [`uffs_client::protocol::response::HibernateResponse`]; the
/// internal type exists so the RPC layer can do the serialisation
/// without re-walking the registry.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct HibernateOutcome {
    /// Drives whose pre-call tier was `Hot`.
    pub hot_demoted: Vec<uffs_mft::platform::DriveLetter>,
    /// Drives whose pre-call tier was `Warm`.
    pub warm_demoted: Vec<uffs_mft::platform::DriveLetter>,
    /// Drives whose pre-call tier was `Parked`.
    pub parked_demoted: Vec<uffs_mft::platform::DriveLetter>,
    /// Drives that were already `Cold` (or whose state was
    /// `Unknown` / `Evicting`, which the operator path cannot
    /// pre-empt).  No registry rebuild for these.
    pub already_cold: Vec<uffs_mft::platform::DriveLetter>,
}

/// Outcome of a [`IndexManager::preload_drive`] call for one drive.
///
/// Mirrors the per-drive shape the
/// [`uffs_client::protocol::response::PreloadResponse`] handler
/// aggregates over a multi-drive request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreloadOutcome {
    /// The drive transitioned to `Hot` from `from_state` and the
    /// pin was armed.
    Promoted {
        /// Tier the drive was in before the preload call.
        from_state: ShardState,
        /// Unix-millis pin expiry.
        pin_until_ms: u64,
    },
    /// The drive was already `Hot`; only the pin was extended.
    AlreadyHot {
        /// Unix-millis pin expiry.
        pin_until_ms: u64,
    },
    /// The drive letter is not registered with the daemon (no
    /// shard exists for it).
    UnknownDrive,
    /// The drive was registered but the body load failed (missing
    /// cache file, decrypt error, panic in the loader, etc.).
    /// Mirrors the `None` return from
    /// [`super::IndexManager::load_or_join_in_flight`].
    LoadFailed,
    /// The drive's pre-call state was `Unknown` or `Evicting` —
    /// controller-only states the operator path cannot pre-empt.
    /// Caller should retry once the state machine settles.
    Busy {
        /// Tier the drive was in when the preload call observed it.
        from_state: ShardState,
    },
}

impl IndexManager {
    /// Phase 8-B — demote every (or a caller-selected subset of)
    /// loaded shard down to `Cold` in a single write-lock batch.
    ///
    /// Empty `drives` ⇒ every shard in the registry.  Non-empty
    /// `drives` ⇒ only shards whose drive letter case-insensitively
    /// matches an entry; entries that don't match any registered
    /// drive are dropped silently (the caller's audit is on the
    /// returned `HibernateOutcome`, which only lists drives the
    /// daemon actually knew about).
    ///
    /// Three-phase orchestration mirroring
    /// [`Self::demote_idle_shards`]:
    ///
    /// 1. **Read-lock detect.**  Single `self.index.read()` to enumerate the
    ///    (letter, from-state) tuples for every shard the call will touch. Cold
    ///    shards record into [`HibernateOutcome::already_cold`] and skip the
    ///    write-lock work.
    /// 2. **Write-lock atomic batch.**  Apply every demote in a single
    ///    write-lock window: each `demote_letter` rebuild is O(shards) and
    ///    sub-µs at the project's max ≤ 26 drives.
    /// 3. **One `bump_index_version` for the batch.**  The aggregate cache
    ///    invalidates once even when N shards moved.  A single
    ///    [`crate::cache::working_set::WorkingSetTrim::trim`] call runs at the
    ///    end (Mac/Linux: no-op; Windows: process-wide `EmptyWorkingSet`
    ///    reclaim).
    ///
    /// The `OperatorHibernate` reason discriminator flows into the
    /// canonical `shard.transition` event so operators can grep
    /// `reason="operator-hibernate"` to distinguish manual
    /// hibernation from idle-tick or pressure-cascade demotes.
    pub(crate) async fn hibernate_shards(
        &self,
        drives: &[uffs_mft::platform::DriveLetter],
    ) -> HibernateOutcome {
        // ── Phase 1: read-lock detect ──────────────────────────────
        let mut outcome = HibernateOutcome::default();
        let demotes: Vec<(uffs_mft::platform::DriveLetter, ShardState)> = {
            let guard = self.index.read().await;
            let mut to_demote: Vec<(uffs_mft::platform::DriveLetter, ShardState)> = Vec::new();
            for shard in guard.iter() {
                let drive = shard.drive;
                if !drives.is_empty() && !drives.contains(&drive) {
                    continue;
                }
                let from_state = shard.state();
                match from_state {
                    ShardState::Hot => {
                        outcome.hot_demoted.push(drive);
                        to_demote.push((drive, from_state));
                    }
                    ShardState::Warm => {
                        outcome.warm_demoted.push(drive);
                        to_demote.push((drive, from_state));
                    }
                    ShardState::Parked => {
                        outcome.parked_demoted.push(drive);
                        to_demote.push((drive, from_state));
                    }
                    // `Cold` — already at the bottom; no-op.
                    // `Unknown` / `Evicting` — controller-only
                    // transient states that hibernate must not
                    // pre-empt (the controller will land them in
                    // a stable tier, and the next operator
                    // hibernate call will catch them).  We surface
                    // them under `already_cold` so the operator
                    // sees they were observed; the wire shape's
                    // `already_cold` field is documented as
                    // "already Cold (or unknown to the registry)"
                    // for exactly this case.
                    ShardState::Cold | ShardState::Unknown | ShardState::Evicting => {
                        outcome.already_cold.push(drive);
                    }
                }
            }
            // Explicit early drop so the read lock is released
            // before the `Vec<(uffs_mft::platform::DriveLetter, ShardState)>` is returned
            // and the surrounding block exits.  Tightens the read-lock
            // hold time for any concurrent dispatcher while keeping
            // clippy::significant_drop_tightening satisfied.
            drop(guard);
            to_demote
        };

        if demotes.is_empty() {
            return outcome;
        }

        // ── Phase 2: write-lock atomic batch ───────────────────────
        let mut guard = self.index.write().await;
        let mut applied = 0_usize;
        for (letter, _from_state) in demotes {
            if let Some(new_registry) = guard.demote_letter_with_reason(
                letter,
                ShardState::Cold,
                DemoteReason::OperatorHibernate,
            ) {
                *guard = Arc::new(new_registry);
                applied = applied.saturating_add(1);
            }
        }
        drop(guard);

        // ── Phase 3: single index-version bump + trim ──────────────
        if applied > 0 {
            self.bump_index_version();
            if let Err(err) = self.working_set_trim.trim() {
                tracing::warn!(
                    target: "shard.transition",
                    error = %err,
                    applied,
                    reason = "operator-hibernate",
                    "WorkingSetTrim::trim failed; daemon continues",
                );
            }
        }

        outcome
    }

    /// Phase 8-C — promote `letter` to `Hot` and pin the tier for
    /// `pin_minutes` minutes.
    ///
    /// Source-state dispatch:
    ///
    /// * `Cold` / `Parked`: drives the existing per-letter single-flight
    ///   body-load + `Prefetch::hint` machinery via
    ///   [`Self::load_or_join_in_flight_for_preload`]; rebuilds the registry
    ///   with a `Hot` `ShardEntry` via
    ///   [`crate::cache::ShardRegistry::promote_letter_to_hot`]; atomically
    ///   arms the pin on the new shard.
    /// * `Warm`: clones the live body, calls `Prefetch::hint` to pre-fault its
    ///   records + names regions, rebuilds the registry with a `Hot` shard,
    ///   arms the pin.
    /// * `Hot`: skips the registry rebuild entirely and atomically extends the
    ///   pin via [`crate::cache::shard::ShardEntry::pin_until`] on the live
    ///   `Arc<ShardEntry>`.  Returns [`PreloadOutcome::AlreadyHot`].
    /// * `Unknown` / `Evicting`: refuses with [`PreloadOutcome::Busy`] — the
    ///   operator must wait for the controller to settle the state machine.
    /// * Drive not registered: [`PreloadOutcome::UnknownDrive`].
    /// * Body-load failure: [`PreloadOutcome::LoadFailed`].
    ///
    /// `pin_minutes` is the operator-supplied (or
    /// [`uffs_client::protocol::response::DEFAULT_PRELOAD_PIN_MINUTES`]
    /// default) duration in minutes; the absolute pin timestamp
    /// returned in
    /// [`PreloadOutcome::Promoted::pin_until_ms`] /
    /// [`PreloadOutcome::AlreadyHot::pin_until_ms`] is computed at
    /// the moment the pin is armed (Unix-millis).  Pre-existing
    /// pins are overwritten — every `preload` call resets the pin
    /// window, even for already-pinned drives.
    pub(crate) async fn preload_drive(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        pin_minutes: u32,
    ) -> PreloadOutcome {
        // ── Phase 1: read-lock detect (state + body) ───────────────
        let snapshot = {
            let guard = self.index.read().await;
            guard
                .iter()
                .find(|shard| shard.drive == letter)
                .map(|shard| (shard.drive, shard.state(), shard.body(), Arc::clone(shard)))
        };
        let Some((drive, from_state, current_body, current_arc)) = snapshot else {
            return PreloadOutcome::UnknownDrive;
        };

        // Pin expiry helper — sampled per call so the AlreadyHot
        // path and the Promoted path both produce the same
        // arithmetic shape.
        let compute_pin_until = || {
            let now_ms = unix_now_ms();
            now_ms.saturating_add(u64::from(pin_minutes).saturating_mul(60_000))
        };

        match from_state {
            ShardState::Hot => {
                // ── Already Hot — atomic pin extension only ───────
                let pin_until_ms = compute_pin_until();
                current_arc.pin_until(pin_until_ms);
                tracing::info!(
                    target: "shard.transition",
                    letter = %drive,
                    pin_until_ms,
                    pin_minutes,
                    reason = "preload-pin-extend",
                );
                PreloadOutcome::AlreadyHot { pin_until_ms }
            }
            ShardState::Unknown | ShardState::Evicting => PreloadOutcome::Busy { from_state },
            ShardState::Warm => {
                // ── Warm → Hot — body already in memory ───────────
                // Body is an Arc bump; pre-faulting it is a fresh
                // Prefetch::hint call so the records + names
                // regions are paged in even though the underlying
                // allocation is reused.
                let Some(body) = current_body else {
                    // Defensive: a Warm shard without a body would
                    // be a structural bug; treat it as a transient
                    // load failure rather than panicking.
                    return PreloadOutcome::LoadFailed;
                };
                Self::prefault_body(&self.prefetch, drive, &body);
                self.swap_in_hot_with_pin(drive, body, compute_pin_until())
                    .await
            }
            ShardState::Cold | ShardState::Parked => {
                // ── Cold/Parked → Hot — single-flight body load ───
                let in_flight = Arc::clone(&self.in_flight_promotes);
                let loader = Arc::clone(&self.body_loader);
                let prefetch = Arc::clone(&self.prefetch);
                let Some(body) =
                    Self::load_or_join_in_flight(in_flight, loader, prefetch, drive).await
                else {
                    return PreloadOutcome::LoadFailed;
                };
                self.swap_in_hot_with_pin(drive, body, compute_pin_until())
                    .await
            }
        }
    }

    /// Atomic write-lock swap: rebuild the registry with `letter`
    /// in `Hot` carrying `body`, then arm the pin on the new shard.
    ///
    /// Factored out of [`Self::preload_drive`] so the Warm-source
    /// and Cold/Parked-source code paths converge on a single
    /// rebuild + pin sequence — keeps the per-source-state logic
    /// readable above and the swap mechanics auditable here.
    async fn swap_in_hot_with_pin(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        body: Arc<uffs_core::compact::DriveCompactIndex>,
        pin_until_ms: u64,
    ) -> PreloadOutcome {
        let mut guard = self.index.write().await;

        // Capture the pre-swap source tier from the existing
        // registry before the rebuild call, so the eventual
        // `Promoted { from_state }` accurately reports the
        // observed transition (`Cold → Hot`, `Parked → Hot`, or
        // `Warm → Hot`) even when a concurrent controller race
        // forces the rebuild to fail.
        let prev_state = guard
            .iter()
            .find(|shard| shard.drive == letter)
            .map_or(ShardState::Unknown, |shard| shard.state());

        let Some(new_registry) = guard.promote_letter_to_hot(letter, body) else {
            // Race: state moved out from under us between the
            // read-lock snapshot and the write-lock acquisition
            // (e.g. the controller demoted to Evicting concurrently,
            // or another preload promoted to Hot first).  The
            // pre-swap `prev_state` is the most accurate signal
            // for the caller's retry logic.
            return PreloadOutcome::Busy {
                from_state: prev_state,
            };
        };

        // Seed the idle clock on the freshly-installed Hot shard
        // and arm the pin atomically before the registry swap.
        // `find` short-circuit is defensive — the drive must
        // exist in the rebuilt registry by construction.
        let now_ms = unix_now_ms();
        if let Some(new_shard) = new_registry.iter().find(|shard| shard.drive == letter) {
            new_shard.stats.mark_loaded_at(now_ms);
            new_shard.pin_until(pin_until_ms);
        }
        *guard = Arc::new(new_registry);
        drop(guard);
        self.bump_index_version();

        PreloadOutcome::Promoted {
            from_state: prev_state,
            pin_until_ms,
        }
    }

    /// Best-effort `Prefetch::hint` call for a body that's already
    /// in memory.
    ///
    /// Mirrors the prefault block inside
    /// [`Self::build_load_future`] but for the Warm-source preload
    /// path where the body is reused rather than freshly loaded.
    /// Fire-and-forget: any I/O error is logged at
    /// `target: "shard.transition"` and the preload continues.
    fn prefault_body(
        prefetch: &Arc<dyn crate::cache::prefetch::Prefetch>,
        letter: uffs_mft::platform::DriveLetter,
        body: &Arc<uffs_core::compact::DriveCompactIndex>,
    ) {
        // Build the records + names regions inline, mirroring the
        // shape used by [`super::dispatch::IndexManager::build_load_future`]
        // for the search-driven promote path.  No `prefault_regions()`
        // accessor is exposed by `DriveCompactIndex` today; both call
        // sites produce the same two-element array.
        let regions = [
            crate::cache::prefetch::PrefetchRegion {
                ptr: body.records.as_slice().as_ptr().cast::<u8>(),
                len: size_of_val(body.records.as_slice()),
            },
            crate::cache::prefetch::PrefetchRegion {
                ptr: body.names.as_slice().as_ptr(),
                len: body.names.as_slice().len(),
            },
        ];
        if let Err(err) = prefetch.hint(&regions) {
            tracing::warn!(
                target: "shard.transition",
                drive = %letter,
                error = %err,
                reason = "preload-prefault",
                "Prefetch::hint failed for warm-source preload; daemon continues",
            );
        }
    }
}
