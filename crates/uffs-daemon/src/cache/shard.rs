// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-shard wrapper: tier state machine + query stats + body.
//!
//! See [`crate::cache`] module docs for the bigger picture.

use alloc::sync::Arc;
use core::error::Error as StdError;
use core::fmt;
use core::str::FromStr;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use uffs_core::compact::DriveCompactIndex;

/// Lifecycle state of a single drive's shard inside the daemon's
/// in-memory cache.
///
/// The state machine mirrors `docs/refactor/memory-tiering-plan.md`
/// §3.1.  Phase 1 only ever holds shards in [`Self::Warm`]; tier
/// transitions out of `Warm` land in Phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub(crate) enum ShardState {
    /// Just discovered; no body, no bloom, no stats. Pre-load.
    Unknown = 0,
    /// Encrypted cache exists but nothing in RAM. Boot/early-startup.
    Cold = 1,
    /// Bloom + trie loaded; body dropped (Phase 4+).
    Parked = 2,
    /// Body fully loaded and searchable. Phase 1 default.
    #[default]
    Warm = 3,
    /// Body loaded + pre-faulted via `Prefetch::hint`. Recent activity.
    Hot = 4,
    /// Demote in progress. Transient.
    Evicting = 5,
}

impl ShardState {
    /// Returns true iff a transition `self` → `to` is in the legal
    /// graph.
    ///
    /// Legal transitions:
    /// * `Unknown` → `Cold`, `Parked`, `Warm`
    /// * `Cold` → `Parked`, `Warm`
    /// * `Parked` → `Cold`, `Warm`
    /// * `Warm` → `Hot`, `Evicting`
    /// * `Hot` → `Warm`, `Evicting`
    /// * `Evicting` → `Cold`, `Parked`
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 3 consumer (tier-transition demote/promote logic); \
                      exercised by this module's proptest and by the \
                      integration test in `crate::index::tests` \
                      under `cfg(test)`."
        )
    )]
    #[must_use]
    pub(crate) const fn can_transition_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Unknown, Self::Cold | Self::Parked | Self::Warm)
                | (Self::Cold, Self::Parked | Self::Warm)
                | (Self::Parked, Self::Cold | Self::Warm)
                | (Self::Warm, Self::Hot | Self::Evicting)
                | (Self::Hot, Self::Warm | Self::Evicting)
                | (Self::Evicting, Self::Cold | Self::Parked)
        )
    }

    /// Round-trip from atomic storage.  Unknown encodings fall back to
    /// `Warm` (the Phase-1 default) to preserve forward-progress on a
    /// torn read; the caller's CAS will redo the transition cleanly.
    const fn from_repr(repr: u8) -> Self {
        match repr {
            0 => Self::Unknown,
            1 => Self::Cold,
            2 => Self::Parked,
            4 => Self::Hot,
            5 => Self::Evicting,
            _ => Self::Warm,
        }
    }
}

impl fmt::Display for ShardState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unknown => "unknown",
            Self::Cold => "cold",
            Self::Parked => "parked",
            Self::Warm => "warm",
            Self::Hot => "hot",
            Self::Evicting => "evicting",
        })
    }
}

/// Error returned by [`FromStr`] for [`ShardState`] when the input
/// isn't one of the six known state names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseShardStateError(pub String);

impl fmt::Display for ParseShardStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown shard state: {}", self.0)
    }
}

impl StdError for ParseShardStateError {}

impl FromStr for ShardState {
    type Err = ParseShardStateError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unknown" => Ok(Self::Unknown),
            "cold" => Ok(Self::Cold),
            "parked" => Ok(Self::Parked),
            "warm" => Ok(Self::Warm),
            "hot" => Ok(Self::Hot),
            "evicting" => Ok(Self::Evicting),
            other => Err(ParseShardStateError(other.into())),
        }
    }
}

/// Error returned by [`ShardEntry::try_transition`] when the requested
/// transition is outside the legal graph encoded in
/// [`ShardState::can_transition_to`].
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Phase 3 consumer (returned by `ShardEntry::try_transition` \
                  when the demoter attempts an out-of-graph move); \
                  exercised by \
                  `crate::index::tests::shard_entry_try_transition_legal_and_illegal` \
                  under `cfg(test)`."
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IllegalTransition {
    /// State the shard was in when the transition was attempted.
    pub from: ShardState,
    /// State the caller asked to move to.
    pub to: ShardState,
}

impl fmt::Display for IllegalTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "illegal shard state transition: {} -> {}",
            self.from, self.to
        )
    }
}

impl StdError for IllegalTransition {}

/// Per-drive query rate stats.
///
/// Counters use atomics so [`Self::record_query`] stays lock-free on
/// the search hot path.  [`Self::decay_ema`] reads + writes are not
/// strictly atomic together, but the EMA tolerates skew (it is a rate
/// estimator, not a hard counter).
///
/// Half-life of the EMA is fixed at 60 s in Phase 1; Phase 6 makes it
/// configurable via `daemon.toml`.
#[derive(Debug, Default)]
pub(crate) struct DriveStats {
    /// Total queries served against this shard.
    queries_total: AtomicU64,
    /// EMA of the per-second query rate, stored as fixed-point `× 1e6`
    /// so it round-trips through the `AtomicU64`.
    rate_ema_micro_per_s: AtomicU64,
    /// Unix-millis timestamp of the last [`Self::decay_ema`] call.
    /// Zero means "never decayed" — first call short-circuits the
    /// decay arithmetic to avoid a huge-elapsed spike from epoch 0.
    last_decay_ms: AtomicU64,
}

impl DriveStats {
    /// Construct a fresh, all-zero stats record.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            queries_total: AtomicU64::new(0),
            rate_ema_micro_per_s: AtomicU64::new(0),
            last_decay_ms: AtomicU64::new(0),
        }
    }

    /// Lock-free increment of the total query counter.
    pub(crate) fn record_query(&self) {
        self.queries_total.fetch_add(1, Ordering::Relaxed);
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
    /// Half-life is 60 s, chosen so a burst of activity is "forgotten"
    /// within ~5 minutes (5 half-lives → 1/32 of the original).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 6 consumer (adaptive-TTL controller reads the EMA \
                      to size the demote/promote thresholds); exercised by \
                      the `drivestats_decay_is_non_increasing` proptest in \
                      this module under `cfg(test)`."
        )
    )]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::float_arithmetic,
        clippy::default_numeric_fallback,
        reason = "EMA arithmetic; precision loss is tolerated by the \
                  rate-estimator semantics and bounded by the 60 s half-life. \
                  Float arithmetic and the `1.0e6` literal are core to the \
                  decay formula; suffixing or changing types would obscure \
                  intent without changing behavior."
    )]
    pub(crate) fn decay_ema(&self, now_ms: u64) -> f64 {
        const HALF_LIFE_MS: u64 = 60_000;
        let prev_ms = self.last_decay_ms.swap(now_ms, Ordering::Relaxed);
        let prev_ema_fixed = self.rate_ema_micro_per_s.load(Ordering::Relaxed);
        let prev_ema = prev_ema_fixed as f64 / 1.0e6;
        if prev_ms == 0 {
            return prev_ema;
        }
        let elapsed_ms = now_ms.saturating_sub(prev_ms);
        if elapsed_ms == 0 {
            return prev_ema;
        }
        let decay_factor =
            (-(elapsed_ms as f64) * core::f64::consts::LN_2 / HALF_LIFE_MS as f64).exp();
        let new_ema = prev_ema * decay_factor;
        self.rate_ema_micro_per_s
            .store((new_ema * 1.0e6) as u64, Ordering::Relaxed);
        new_ema
    }
}

/// Test-only direct EMA read for `DriveStats`.
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
fn drive_stats_ema_value(stats: &DriveStats) -> f64 {
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
}

impl From<&DriveStats> for DriveStatsSnapshot {
    fn from(stats: &DriveStats) -> Self {
        Self {
            queries_total: stats.queries_total.load(Ordering::Relaxed),
            rate_ema_micro_per_s: stats.rate_ema_micro_per_s.load(Ordering::Relaxed),
            last_decay_ms: stats.last_decay_ms.load(Ordering::Relaxed),
        }
    }
}

impl From<DriveStatsSnapshot> for DriveStats {
    fn from(snap: DriveStatsSnapshot) -> Self {
        Self {
            queries_total: AtomicU64::new(snap.queries_total),
            rate_ema_micro_per_s: AtomicU64::new(snap.rate_ema_micro_per_s),
            last_decay_ms: AtomicU64::new(snap.last_decay_ms),
        }
    }
}

/// One shard's runtime state + stats + body.
///
/// Phase 1 holds the body unconditionally as `Arc<DriveCompactIndex>`.
/// Phase 2 introduces an `Option`-wrapped variant for demoted shards;
/// Phase-1 callers can rely on [`Self::body`] always returning the
/// index.
pub(crate) struct ShardEntry {
    /// Drive letter (`'C'`, `'D'`, …). Capital ASCII per existing
    /// daemon convention.
    pub(crate) drive: char,
    /// Tier state. Read on every search via [`Self::state`]; mutated
    /// only by tier-transition methods (`try_transition`).
    state: AtomicU8,
    /// Per-drive query stats.
    pub(crate) stats: DriveStats,
    /// In-memory compact index. Cloned cheaply (Arc bump) into
    /// [`crate::cache::ShardRegistry::active_index`] on rebuild.
    body: Arc<DriveCompactIndex>,
}

impl ShardEntry {
    /// Construct a shard wrapping `body` and pinning it in
    /// [`ShardState::Warm`].
    ///
    /// Phase 1 only ever creates shards in this constructor; Phase 3
    /// adds `new_cold` / `new_parked` for boot-time partial loads.
    #[must_use]
    pub(crate) const fn new_warm(drive: char, body: Arc<DriveCompactIndex>) -> Self {
        Self {
            drive,
            state: AtomicU8::new(ShardState::Warm as u8),
            stats: DriveStats::new(),
            body,
        }
    }

    /// Read the current tier state without locking.
    #[must_use]
    pub(crate) fn state(&self) -> ShardState {
        ShardState::from_repr(self.state.load(Ordering::Acquire))
    }

    /// Cheap clone of the in-memory body.
    #[must_use]
    pub(crate) fn body(&self) -> Arc<DriveCompactIndex> {
        Arc::clone(&self.body)
    }

    /// Attempt to transition the shard to `to`.
    ///
    /// On success returns the previous state.  On failure returns
    /// [`IllegalTransition`] without mutating the shard.
    ///
    /// Internally uses a CAS loop so concurrent transition attempts
    /// linearise without lost updates.
    ///
    /// # Errors
    ///
    /// Returns [`IllegalTransition`] when the requested move is
    /// outside the graph encoded in [`ShardState::can_transition_to`].
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 3 consumer (tier-transition CAS used by the \
                      demoter / promoter); exercised by \
                      `crate::index::tests::shard_entry_try_transition_legal_and_illegal` \
                      under `cfg(test)`."
        )
    )]
    pub(crate) fn try_transition(&self, to: ShardState) -> Result<ShardState, IllegalTransition> {
        loop {
            let prev_repr = self.state.load(Ordering::Acquire);
            let prev = ShardState::from_repr(prev_repr);
            if !prev.can_transition_to(to) {
                return Err(IllegalTransition { from: prev, to });
            }
            // CAS loop: on success return the prior state; on failure
            // (concurrent transition raced us) fall through and retry.
            if let Ok(_prev) = self.state.compare_exchange(
                prev_repr,
                to as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                return Ok(prev);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::min_ident_chars,
        clippy::default_numeric_fallback,
        clippy::doc_markdown,
        reason = "test code — short loop counters and doc references like \
                  `serde_json` are clearer without the pedantic ceremony."
    )]

    use proptest::prelude::*;

    use super::*;

    fn arb_state() -> impl Strategy<Value = ShardState> {
        prop_oneof![
            Just(ShardState::Unknown),
            Just(ShardState::Cold),
            Just(ShardState::Parked),
            Just(ShardState::Warm),
            Just(ShardState::Hot),
            Just(ShardState::Evicting),
        ]
    }

    proptest! {
        /// Task 1.6: `decay_ema` is non-increasing between consecutive
        /// calls without an intervening `record_query` (decay only
        /// shrinks the EMA, never grows it).
        #[test]
        fn drivestats_decay_is_non_increasing(
            seed_ema_micro in 0_u64..1_000_000_000_u64,
            gap_ms in 1_u64..100_000_u64,
        ) {
            let stats = DriveStats::new();
            stats.rate_ema_micro_per_s.store(seed_ema_micro, Ordering::Relaxed);
            stats.last_decay_ms.store(1_000_000, Ordering::Relaxed);
            let before = drive_stats_ema_value(&stats);
            let after = stats.decay_ema(1_000_000_u64.saturating_add(gap_ms));
            prop_assert!(
                after <= before,
                "after {} > before {}",
                after,
                before,
            );
            prop_assert!(after >= 0.0);
        }

        /// Task 1.7: every (from, to) pair outside the legal graph is
        /// rejected by `can_transition_to`, and the inverse holds for
        /// the listed legal pairs.
        #[test]
        fn shardstate_legal_graph_is_consistent(from in arb_state(), to in arb_state()) {
            // The legal graph is hand-listed in `can_transition_to`;
            // here we duplicate it as a set of pairs and check
            // bidirectional agreement.
            let legal: &[(ShardState, ShardState)] = &[
                (ShardState::Unknown, ShardState::Cold),
                (ShardState::Unknown, ShardState::Parked),
                (ShardState::Unknown, ShardState::Warm),
                (ShardState::Cold, ShardState::Parked),
                (ShardState::Cold, ShardState::Warm),
                (ShardState::Parked, ShardState::Cold),
                (ShardState::Parked, ShardState::Warm),
                (ShardState::Warm, ShardState::Hot),
                (ShardState::Warm, ShardState::Evicting),
                (ShardState::Hot, ShardState::Warm),
                (ShardState::Hot, ShardState::Evicting),
                (ShardState::Evicting, ShardState::Cold),
                (ShardState::Evicting, ShardState::Parked),
            ];
            let in_graph = legal.iter().any(|&(a, b)| a == from && b == to);
            let actual = from.can_transition_to(to);
            prop_assert_eq!(
                in_graph,
                actual,
                "{} -> {}: graph says {}, can_transition_to says {}",
                from,
                to,
                in_graph,
                actual,
            );
        }
    }

    /// Task 1.8: `DriveStatsSnapshot` round-trips through serde_json
    /// and through the `From` conversions.
    #[test]
    fn drivestats_snapshot_round_trips() {
        let stats = DriveStats::new();
        for _ in 0..7 {
            stats.record_query();
        }
        stats.rate_ema_micro_per_s.store(123_456, Ordering::Relaxed);
        stats.last_decay_ms.store(987_654_321, Ordering::Relaxed);

        let snap = DriveStatsSnapshot::from(&stats);
        let json = serde_json::to_string(&snap).expect("serialize");
        let restored: DriveStatsSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(snap, restored);
        assert_eq!(restored.queries_total, 7);
        assert_eq!(restored.rate_ema_micro_per_s, 123_456);
        assert_eq!(restored.last_decay_ms, 987_654_321);

        let stats2 = DriveStats::from(restored);
        assert_eq!(stats2.queries_total(), 7);
    }

    /// `record_query` is monotone — N increments yields total of N.
    #[test]
    fn record_query_is_monotone() {
        let stats = DriveStats::new();
        for _ in 0..10 {
            stats.record_query();
        }
        assert_eq!(stats.queries_total(), 10);
    }

    /// First `decay_ema` call returns the stored value without decaying
    /// (no elapsed signal yet).
    #[test]
    fn decay_ema_first_call_returns_stored_value() {
        let stats = DriveStats::new();
        stats
            .rate_ema_micro_per_s
            .store(5_000_000, Ordering::Relaxed);
        // last_decay_ms == 0 means "never decayed".
        let v = stats.decay_ema(1_000_000);
        assert!((v - 5.0).abs() < 1e-9, "first call returned {v}");
    }

    /// `ShardState::FromStr` accepts every `Display` form and rejects
    /// unknown input.
    #[test]
    fn shardstate_fromstr_round_trips() {
        for state in [
            ShardState::Unknown,
            ShardState::Cold,
            ShardState::Parked,
            ShardState::Warm,
            ShardState::Hot,
            ShardState::Evicting,
        ] {
            let s = state.to_string();
            let parsed: ShardState = s.parse().expect("parse round-trip");
            assert_eq!(state, parsed, "{s} did not round-trip");
        }
        let err = "foobar".parse::<ShardState>().unwrap_err();
        assert_eq!(err.0, "foobar");
        assert!(format!("{err}").contains("unknown shard state"));
    }

    /// `ShardState` serializes through serde with lowercase names.
    #[test]
    fn shardstate_serde_lowercase() {
        let json = serde_json::to_string(&ShardState::Warm).unwrap();
        assert_eq!(json, r#""warm""#);
        let back: ShardState = serde_json::from_str(r#""parked""#).unwrap();
        assert_eq!(back, ShardState::Parked);
    }

    /// `ShardState::default()` is `Warm` (Phase-1 invariant).
    #[test]
    fn shardstate_default_is_warm() {
        assert_eq!(ShardState::default(), ShardState::Warm);
    }

    /// `IllegalTransition` Display matches the documented format.
    #[test]
    fn illegal_transition_display() {
        let err = IllegalTransition {
            from: ShardState::Cold,
            to: ShardState::Hot,
        };
        assert_eq!(
            format!("{err}"),
            "illegal shard state transition: cold -> hot"
        );
    }
}
