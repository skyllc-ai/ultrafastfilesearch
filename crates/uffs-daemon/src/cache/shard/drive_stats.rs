// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-drive query-rate stats — extracted from
//! [`super`](crate::cache::shard) to keep the parent
//! `cache/shard.rs` under the workspace 800-LOC ceiling.
//!
//! This module owns:
//!
//! * [`DriveStats`] — the live atomic counter bundle every
//!   [`super::ShardEntry`] holds via `Arc<DriveStats>` so per-drive counters
//!   survive a tier-transition registry rebuild (the new `ShardEntry` shares
//!   the same `Arc<DriveStats>` so concurrent `mark_query_at` writes from
//!   in-flight searches still land on the canonical counters).
//! * [`DriveStatsSnapshot`] — the `serde`-able plain-`u64` mirror for
//!   persistence (`AtomicU64` does not derive `Serialize`/`Deserialize`).
//! * The two `From` impls that move data between the live atomics and the
//!   snapshot.
//! * The test-only [`drive_stats_ema_value`] reader, which exists so the
//!   production `impl DriveStats` block carries no `#[cfg(test)]`-gated
//!   methods.
//!
//! Re-exported at the parent path
//! [`crate::cache::shard::DriveStats`] so existing call-sites
//! continue to work without churn — the split is mechanical, not
//! a public-API change.

use core::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Per-drive query rate stats.
///
/// Counters use atomics so [`Self::record_query`] and
/// [`Self::mark_query_at`] stay lock-free on the search hot path.
/// [`Self::decay_ema`] reads + writes are not strictly atomic
/// together, but the EMA tolerates skew (it is a rate estimator, not
/// a hard counter).
///
/// Half-life of the EMA is fixed at 60 s in Phase 1; Phase 6 makes it
/// configurable via `daemon.toml`.
///
/// Phase 3 wraps the live `DriveStats` in `Arc<DriveStats>` on the
/// owning [`super::ShardEntry`] so the per-drive counters survive
/// intact when the registry rebuilds a `ShardEntry` for a tier
/// transition — the new entry shares the same `Arc<DriveStats>` and
/// concurrent `mark_query_at` writes from in-flight searches still
/// land on the canonical counters.
#[derive(Debug, Default)]
pub(crate) struct DriveStats {
    /// Total queries served against this shard.
    queries_total: AtomicU64,
    /// EMA of the per-second query rate, stored as fixed-point `× 1e6`
    /// so it round-trips through the `AtomicU64`.
    ///
    /// Visibility: `pub(super)` — the proptest harness in
    /// `cache/shard/tests.rs` seeds this field directly to drive
    /// the `decay_ema` non-monotonicity property test
    /// (`drivestats_decay_is_non_increasing`).  Production code
    /// goes through [`Self::decay_ema`] / [`Self::decay_ema_qpm`]
    /// which mutate this field internally; no other crate-level
    /// caller pokes it.
    pub(super) rate_ema_micro_per_s: AtomicU64,
    /// Unix-millis timestamp of the last [`Self::decay_ema`] call.
    /// Zero means "never decayed" — first call short-circuits the
    /// decay arithmetic to avoid a huge-elapsed spike from epoch 0.
    ///
    /// Visibility: `pub(super)` — same rationale as
    /// [`Self::rate_ema_micro_per_s`].  Test fixtures seed a known
    /// `last_decay_ms` so the elapsed-time arithmetic in
    /// `decay_ema` is deterministic.
    pub(super) last_decay_ms: AtomicU64,
    /// Unix-millis timestamp of the last [`Self::mark_query_at`] call.
    /// Zero means "never queried" — the Phase-3 demote controller in
    /// `crate::cache::registry::ShardRegistry::demote_idle_shards`
    /// treats a zero value as "as old as the daemon" so freshly-loaded
    /// shards aren't immediately demoted on the first 30 s tick.
    ///
    /// Visibility: `pub(super)` — only the snapshot round-trip test
    /// in `cache/shard/tests.rs` seeds this directly; production
    /// code uses [`Self::mark_query_at`] / [`Self::mark_loaded_at`].
    pub(super) last_query_at_ms: AtomicU64,
    /// Cumulative count of `Cold → Hot` promotions for this drive
    /// since daemon start (Phase 9 — `promotions_total` wire field).
    ///
    /// Bumped from
    /// [`crate::cache::registry::ShardRegistry::promote_letter_to_hot`]
    /// only when the pre-promote tier was `Cold` — i.e. the operator
    /// ran `uffs daemon preload <drive>` against a fully-evicted
    /// drive and the daemon had to re-decrypt the encrypted compact
    /// cache from disk.  Already-Warm preload calls (where the body
    /// is in RAM and only the tier marker flips Warm → Hot) do
    /// **not** bump this counter — they are not "Cold → Hot".
    ///
    /// Surfaced via the [`StatusDrivesResponse`] wire format's
    /// `promotions_total` field so operators can count expensive
    /// re-promotes per drive without reconstructing the count from
    /// the `shard.transition` event log.
    ///
    /// [`StatusDrivesResponse`]: uffs_client::protocol::response::StatusDrivesResponse
    promotions_total: AtomicU64,
}

impl DriveStats {
    /// Construct a fresh, all-zero stats record.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            queries_total: AtomicU64::new(0),
            rate_ema_micro_per_s: AtomicU64::new(0),
            last_decay_ms: AtomicU64::new(0),
            last_query_at_ms: AtomicU64::new(0),
            promotions_total: AtomicU64::new(0),
        }
    }

    /// Lock-free increment of the total query counter.
    ///
    /// Phase 3 prefer [`Self::mark_query_at`] which also bumps
    /// `last_query_at_ms` so the demote controller has a fresh idle
    /// timestamp.  This bare counter remains for callers that have no
    /// clock available (e.g. legacy tests).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 3 migrated production `record_search_dispatch` to \
                      `mark_query_at(now_ms)`; this clock-free entry point is \
                      retained for tests that don't need timestamp wiring."
        )
    )]
    pub(crate) fn record_query(&self) {
        self.queries_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Lock-free increment of the total query counter that **also**
    /// stores `now_ms` in [`Self::last_query_at_ms`].
    ///
    /// Two relaxed atomics; not synchronised together — the demote
    /// controller tolerates a one-tick (30 s) lag on a shard whose
    /// `last_query_at_ms` write was reordered after the
    /// `queries_total` increment.
    pub(crate) fn mark_query_at(&self, now_ms: u64) {
        self.queries_total.fetch_add(1, Ordering::Relaxed);
        self.last_query_at_ms.store(now_ms, Ordering::Relaxed);
    }

    /// Set [`Self::last_query_at_ms`] to `now_ms` **without** bumping
    /// the query counter.
    ///
    /// Phase 3 Commit D — called by `IndexManager::add_drive` and
    /// `IndexManager::replace_drive` once per shard the moment the
    /// drive is mounted, so the demote-controller's idle clock
    /// starts ticking from the load time rather than from epoch zero.
    /// Without this seed, a freshly loaded shard's
    /// `last_query_at_ms == 0` would compute `idle_secs ≈ now_ms /
    /// 1000` and trigger an immediate demote on the first 30 s tick.
    ///
    /// Distinct from `mark_query_at` so the per-shard `queries_total`
    /// metric stays a clean count of actual searches dispatched, not
    /// "searches plus one extra at load".
    pub(crate) fn mark_loaded_at(&self, now_ms: u64) {
        self.last_query_at_ms.store(now_ms, Ordering::Relaxed);
    }

    /// Lock-free increment of the `Cold → Hot` promotion counter
    /// (Phase 9).
    ///
    /// Called by
    /// [`crate::cache::registry::ShardRegistry::promote_letter_to_hot`]
    /// after a successful Cold-source promote — i.e. the registry
    /// rebuild that landed an `Arc<ShardEntry>` in `Hot` tier
    /// observed `from_state == Cold` before the rebuild.
    ///
    /// Distinct from [`Self::mark_query_at`] / [`Self::record_query`]
    /// (the per-search counters) so an operator can decompose the
    /// shard's lifecycle into "search load" vs "explicit re-promote
    /// cost" without re-walking the `shard.transition` event log.
    pub(crate) fn record_cold_to_hot_promote(&self) {
        self.promotions_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Read the cumulative `Cold → Hot` promotion count.
    ///
    /// Surfaced via the
    /// [`uffs_client::protocol::response::StatusDrivesResponse`] wire
    /// format's `promotions_total` field by
    /// [`crate::index::status_drives::IndexManager::status_drives`].
    #[must_use]
    pub(crate) fn promotions_total(&self) -> u64 {
        self.promotions_total.load(Ordering::Relaxed)
    }

    /// Read the last activity timestamp (Unix millis).
    ///
    /// Updated by [`Self::mark_query_at`] (search dispatch) and
    /// [`Self::mark_loaded_at`] (drive load).  Returns `0` only on
    /// the snapshot-deserialisation / test paths that go through
    /// the legacy [`Self::new`] constructor without ever calling a
    /// setter.
    ///
    /// Read by [`crate::index::IndexManager::demote_idle_shards`]
    /// (Phase 3 Commit D) to compute `idle_secs` against the
    /// per-tier TTL ladder.
    #[must_use]
    pub(crate) fn last_query_at_ms(&self) -> u64 {
        self.last_query_at_ms.load(Ordering::Relaxed)
    }

    /// Total queries served against this shard since construction.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 6 consumer (status renderer / Prometheus telemetry); \
                      read by `IndexManager::shard_query_totals_for_test` \
                      under `cfg(test)`."
        )
    )]
    #[must_use]
    pub(crate) fn queries_total(&self) -> u64 {
        self.queries_total.load(Ordering::Relaxed)
    }

    /// Apply exponential decay to the EMA based on elapsed time since
    /// the last call and return the new EMA in queries/sec.
    ///
    /// First call after construction returns the stored value as-is
    /// (no elapsed-time signal to decay against).
    ///
    /// The EMA half-life is 60 s — every 60 s without any new query
    /// the rate halves.  Bumps from `mark_query_at` are not directly
    /// integrated here; instead the controller (Phase 6) calls this
    /// once per tick, then if `queries_total` increased since the
    /// last call, computes the per-second rate and feeds it into the
    /// EMA via a separate path.  This split keeps the search hot
    /// path branch-free.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::float_arithmetic,
        reason = "EMA arithmetic is intentionally floating-point and lossy; \
                  the values cast through u64 are fixed-point µ/s representations \
                  whose precision loss in f64 round-trip is bounded by the \
                  decay formula; suffixing or changing types would obscure \
                  intent without changing behavior."
    )]
    pub(crate) fn decay_ema(&self, now_ms: u64) -> f64 {
        const HALF_LIFE_MS: u64 = 60_000;
        let prev_ms = self.last_decay_ms.swap(now_ms, Ordering::Relaxed);
        let prev_ema_fixed = self.rate_ema_micro_per_s.load(Ordering::Relaxed);
        if prev_ms == 0 || prev_ema_fixed == 0 {
            // Convert micro/s fixed-point to /s float.
            return prev_ema_fixed as f64 / 1.0e6;
        }
        let elapsed_ms = now_ms.saturating_sub(prev_ms);
        // Half-life formula: new = old * 0.5^(elapsed / HL).
        let half_lives = elapsed_ms as f64 / HALF_LIFE_MS as f64;
        let decay_factor = (-half_lives * core::f64::consts::LN_2).exp();
        let new_ema = (prev_ema_fixed as f64 / 1.0e6_f64) * decay_factor;
        // Store back the decayed value as fixed-point µ/s.
        let new_fixed = (new_ema * 1.0e6_f64) as u64;
        self.rate_ema_micro_per_s
            .store(new_fixed, Ordering::Relaxed);
        new_ema
    }

    /// Convenience: [`Self::decay_ema`] in queries/min instead of
    /// queries/sec.  The EMA's underlying storage is per-second, but
    /// the Phase 6 adaptive-TTL formula in
    /// [`crate::cache::policy`] is sized in q/min so the human-
    /// scale numbers in `daemon.toml` (e.g. `30` q/min) match the
    /// observed EMA without a per-call multiply at every threshold
    /// computation.
    ///
    /// Side-effect (inherited from `decay_ema`): the EMA's stored value
    /// is mutated in place to reflect the elapsed-time decay.
    /// Callers should sample once per controller tick rather than
    /// per shard if they want a coherent batch view.
    #[expect(
        clippy::float_arithmetic,
        reason = "single multiply-by-60 to convert q/s to q/min — same \
                  precision posture as `decay_ema` itself."
    )]
    pub(crate) fn decay_ema_qpm(&self, now_ms: u64) -> f64 {
        self.decay_ema(now_ms) * 60.0
    }
}

/// Test-only direct EMA read for [`DriveStats`].
///
/// Free function (rather than a `#[cfg(test)]` method on
/// `impl DriveStats`) so the production block carries no test-only
/// methods.  All test-specific lint ceremony stays attached to this
/// helper.
#[cfg(test)]
#[expect(
    clippy::cast_precision_loss,
    clippy::float_arithmetic,
    reason = "test-only EMA read: float divide on a fixed-point store; \
              the precision loss is tolerated by the rate-estimator \
              semantics."
)]
pub(crate) fn drive_stats_ema_value(stats: &DriveStats) -> f64 {
    stats.rate_ema_micro_per_s.load(Ordering::Relaxed) as f64 / 1.0e6
}

/// Serializable snapshot of a [`DriveStats`].
///
/// `AtomicU64` doesn't derive `Serialize`/`Deserialize` so persistence
/// goes through this plain-`u64` mirror.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DriveStatsSnapshot {
    /// Total queries served. See [`DriveStats::queries_total`].
    pub queries_total: u64,
    /// EMA fixed-point. See [`DriveStats::rate_ema_micro_per_s`].
    pub rate_ema_micro_per_s: u64,
    /// Last decay timestamp. See [`DriveStats::last_decay_ms`].
    pub last_decay_ms: u64,
    /// Last query timestamp. See [`DriveStats::last_query_at_ms`].
    /// Defaults to `0` so legacy snapshots without this field
    /// deserialise as "never queried" rather than rejecting.
    #[serde(default)]
    pub last_query_at_ms: u64,
    /// Cumulative `Cold → Hot` promotions.
    /// See [`DriveStats::promotions_total`].  Defaults to `0` so
    /// pre-Phase-9 snapshots that don't carry the field deserialise
    /// as "never promoted from Cold" rather than rejecting — this
    /// preserves backward compat with on-disk persisted stats.
    #[serde(default)]
    pub promotions_total: u64,
}

impl From<&DriveStats> for DriveStatsSnapshot {
    fn from(stats: &DriveStats) -> Self {
        Self {
            queries_total: stats.queries_total.load(Ordering::Relaxed),
            rate_ema_micro_per_s: stats.rate_ema_micro_per_s.load(Ordering::Relaxed),
            last_decay_ms: stats.last_decay_ms.load(Ordering::Relaxed),
            last_query_at_ms: stats.last_query_at_ms.load(Ordering::Relaxed),
            promotions_total: stats.promotions_total.load(Ordering::Relaxed),
        }
    }
}

impl From<DriveStatsSnapshot> for DriveStats {
    fn from(snap: DriveStatsSnapshot) -> Self {
        Self {
            queries_total: AtomicU64::new(snap.queries_total),
            rate_ema_micro_per_s: AtomicU64::new(snap.rate_ema_micro_per_s),
            last_decay_ms: AtomicU64::new(snap.last_decay_ms),
            last_query_at_ms: AtomicU64::new(snap.last_query_at_ms),
            promotions_total: AtomicU64::new(snap.promotions_total),
        }
    }
}
