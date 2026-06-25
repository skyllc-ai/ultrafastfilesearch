// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-shard cadence state machines for the journal loop.
//!
//! Two independent counters govern when the loop drains its pending
//! change buffer, deliberately decoupled so search freshness and disk
//! persistence run on their own clocks:
//!
//! * [`SaveTrigger`] — the rare, expensive **disk save** (default 50k events /
//!   5 min).  Crossing either threshold patches the body AND persists the
//!   compact cache + cursor.
//! * [`ApplyTrigger`] — the more frequent, disk-free **in-memory apply**
//!   (default 30 s).  Buffered churn plus an elapsed interval patches the body
//!   so a freshly created / renamed / deleted file becomes searchable, without
//!   touching disk.
//!
//! A save subsumes an apply (it drains + applies the same buffer), so
//! the loop fires at most one of the two per tick; see
//! [`super::process_tick`].  Extracted from `journal_loop.rs` to keep
//! that file under the workspace file-size policy while keeping the two
//! cadence machines (and the threshold constants they default from)
//! together as one auditable unit.

use core::time::Duration;
use std::time::Instant;

/// Default events-since-save threshold for triggering a background
/// compact-cache save (Phase 7 task 7.4).
///
/// Sized to approximate the plan's "5% churn" criterion at the
/// typical 1.3 GB × ~7 M-record drive shape (`50_000` events ≈ 0.7%
/// churn, comfortably below 5%).  Saving more frequently would
/// thrash the disk; less frequently would let the on-disk snapshot
/// drift far enough that a cold-boot replay window grows beyond
/// the cost of an incremental save.
pub(crate) const DEFAULT_SAVE_THRESHOLD_EVENTS: u64 = 50_000;

/// Default time-since-save threshold for triggering a background
/// compact-cache save (Phase 7 task 7.4) — 5 minutes.
///
/// Provides a wall-clock ceiling for how stale the on-disk snapshot
/// can get under low-churn workloads (where the events-threshold
/// would never fire on its own).  Five minutes matches the cadence
/// of the existing Phase-5 `refresh_usn_for_warm_shards` global
/// tick so the persistence guarantee carries over to the per-shard
/// path without changing the operator-visible recovery window.
pub(crate) const DEFAULT_SAVE_THRESHOLD_AGE: Duration = Duration::from_mins(5);

/// Default apply interval for the per-shard journal loop — 30 seconds.
///
/// This is the **search-freshness** knob, decoupled from the much
/// rarer disk-save cadence above.  When buffered changes exist and at
/// least this long has elapsed since the last apply / save, the loop
/// patches the in-memory body (via [`super::PatchSink::trigger_apply`])
/// so a freshly created / renamed / deleted file becomes searchable —
/// instead of waiting up to [`DEFAULT_SAVE_THRESHOLD_AGE`] (5 min) for a
/// disk-save tick to also apply it.
///
/// Thirty seconds is tuned for the per-apply rebuild cost: each apply
/// clones the body and rebuilds the children / trigram / extension
/// indexes (~600 ms on a 7M-record drive, **independent of batch
/// size**).  On a filesystem with constant churn that throttles the
/// rebuild to background noise (~600 ms / 30 s ≈ 2 % of one core per
/// active drive) instead of a continuous drag.  Crucially it does *not*
/// blunt the common case: because the trigger fires as soon as the
/// interval has elapsed *since the last apply*, the first change after
/// any quiet period is applied within a poll or two — only sustained,
/// back-to-back churn is batched onto the 30 s cadence.  On an idle
/// drive no apply fires at all (the event counter stays at zero).
///
/// Overridable at runtime via the `UFFS_USN_APPLY_INTERVAL_MS`
/// environment variable, mirroring `UFFS_USN_POLL_INTERVAL_MS`, so soak
/// tests and latency-sensitive setups can dial freshness up or down
/// without recompiling.
pub(crate) const DEFAULT_APPLY_INTERVAL_MS: u64 = 30_000;

/// Why a [`super::PatchSink::trigger_save`] call fired.
///
/// Encoded so observability surfaces (logs, metrics) can
/// distinguish heavy-churn-driven saves from time-pressure-driven
/// saves; the production sink also passes this through to the
/// compact-cache writer for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveReason {
    /// `events_since_save >= save_threshold_events` — lots of
    /// churn has accumulated and the on-disk snapshot is
    /// progressively stale.
    EventsExceeded,
    /// `Instant::now() - last_save_at >= save_threshold_age` —
    /// time-pressure path for low-churn drives where the
    /// events threshold would otherwise never fire.
    AgeElapsed,
}

/// Per-shard save-threshold state machine (Phase 7 task 7.4).
///
/// Tracks the wall-clock time of the last save trigger and the
/// number of events accumulated since.  Crossing either the
/// events- or age-threshold (with at least one event pending)
/// produces a [`SaveReason`] and resets both counters.  Held
/// inside the [`super::JournalLoop`] so each per-shard task carries
/// its own independent counters.
#[derive(Debug)]
pub(crate) struct SaveTrigger {
    /// Wall-clock time of the last save trigger (or, before any
    /// triggers, the loop's spawn time).  Compared against
    /// `Instant::now()` to compute elapsed-since-last-save.
    last_save_at: Instant,
    /// Total events accumulated across [`Self::record`] calls
    /// since the last save trigger.  Compared against
    /// `save_threshold_events` to fire the events-based save.
    events_since_save: u64,
}

impl SaveTrigger {
    /// Construct a fresh trigger with `last_save_at` set to
    /// `Instant::now()` (so the first age-based save can't fire
    /// until at least `save_threshold_age` has elapsed since
    /// loop spawn).
    pub(super) fn new() -> Self {
        Self {
            last_save_at: Instant::now(),
            events_since_save: 0,
        }
    }

    /// Record `change_count` events accumulating toward the
    /// events-based threshold.  Saturating add so a runaway
    /// drive can't wrap and silently miss the threshold.
    pub(super) const fn record(&mut self, change_count: u64) {
        self.events_since_save = self.events_since_save.saturating_add(change_count);
    }

    /// Evaluate the thresholds.
    ///
    /// **Returns** `Some(reason)` if a save should fire — and
    /// resets both counters as a side effect (so the next
    /// `evaluate` after a save starts from a clean slate).
    /// Returns `None` when no threshold is crossed *or* when no
    /// events are pending (zero-churn drives never produce
    /// no-op saves).
    pub(super) fn evaluate(
        &mut self,
        save_threshold_events: u64,
        save_threshold_age: Duration,
    ) -> Option<SaveReason> {
        if self.events_since_save == 0 {
            return None;
        }
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_save_at);
        let reason = if self.events_since_save >= save_threshold_events {
            Some(SaveReason::EventsExceeded)
        } else if elapsed >= save_threshold_age {
            Some(SaveReason::AgeElapsed)
        } else {
            None
        };
        if reason.is_some() {
            self.last_save_at = now;
            self.events_since_save = 0;
        }
        reason
    }
}

/// Per-shard apply-cadence state machine — the search-freshness
/// counterpart to [`SaveTrigger`].
///
/// Where `SaveTrigger` governs the rare, expensive disk save (50k
/// events / 5 min), this governs the more frequent, in-memory body
/// patch (default 30 s, [`DEFAULT_APPLY_INTERVAL_MS`]).  Decoupling the
/// two is the whole point: a created / renamed / deleted file must
/// become searchable quickly, but the compact-cache disk write should
/// stay rare.
///
/// The trigger is purely time-gated with a "has churn" guard — it
/// fires when at least one event has been recorded since the last
/// apply / save **and** [`Self::evaluate`]'s `apply_interval` has
/// elapsed.  There is intentionally no event-count fast-path: a huge
/// burst is already caught by `SaveTrigger`'s 50k threshold (a save
/// applies too), so the apply tick only needs to bound *latency*, not
/// volume.
#[derive(Debug)]
pub(crate) struct ApplyTrigger {
    /// Wall-clock time of the last apply (or save, which also applies)
    /// — or the loop spawn time before any fire.  Compared against
    /// `Instant::now()` to rate-limit applies to one per
    /// `apply_interval`.
    last_apply_at: Instant,
    /// Events accumulated since the last apply / save.  Non-zero is
    /// the "there is something to apply" guard; the exact count does
    /// not matter (no volume threshold here).
    events_since_apply: u64,
}

impl ApplyTrigger {
    /// Construct a fresh trigger with `last_apply_at` set to
    /// `Instant::now()` so the first apply can't fire until
    /// `apply_interval` has elapsed since loop spawn.
    pub(super) fn new() -> Self {
        Self {
            last_apply_at: Instant::now(),
            events_since_apply: 0,
        }
    }

    /// Record `change_count` events toward the "has churn" guard.
    /// Saturating so a runaway drive can't wrap the counter back to
    /// zero and suppress an apply.
    pub(super) const fn record(&mut self, change_count: u64) {
        self.events_since_apply = self.events_since_apply.saturating_add(change_count);
    }

    /// Evaluate the apply cadence.
    ///
    /// **Returns** `true` (and resets the counters) when there is
    /// buffered churn and at least `apply_interval` has elapsed since
    /// the last apply / save.  Returns `false` — without resetting —
    /// otherwise, so a not-yet-due tick keeps accumulating.
    pub(super) fn evaluate(&mut self, apply_interval: Duration) -> bool {
        if self.events_since_apply == 0 {
            return false;
        }
        let now = Instant::now();
        if now.saturating_duration_since(self.last_apply_at) < apply_interval {
            return false;
        }
        self.last_apply_at = now;
        self.events_since_apply = 0;
        true
    }

    /// Reset the trigger because a **save** tick just drained + applied
    /// the buffer (a save subsumes an apply).  Clears the churn guard
    /// and restarts the interval clock so the loop doesn't fire a
    /// redundant apply on the same buffer right after a save.
    pub(super) fn reset_after_save(&mut self) {
        self.last_apply_at = Instant::now();
        self.events_since_apply = 0;
    }
}
