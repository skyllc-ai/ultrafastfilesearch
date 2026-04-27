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
/// owning [`ShardEntry`] so the per-drive counters survive intact
/// when the registry rebuilds a `ShardEntry` for a tier transition
/// — the new entry shares the same `Arc<DriveStats>` and concurrent
/// `mark_query_at` writes from in-flight searches still land on the
/// canonical counters.
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
    /// Unix-millis timestamp of the last [`Self::mark_query_at`] call.
    /// Zero means "never queried" — the Phase-3 demote controller in
    /// `crate::cache::registry::ShardRegistry::demote_idle_shards`
    /// treats a zero value as "as old as the daemon" so freshly-loaded
    /// shards aren't immediately demoted on the first 30 s tick.
    last_query_at_ms: AtomicU64,
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
    /// Last query timestamp. See [`DriveStats::last_query_at_ms`].
    /// Defaults to `0` so legacy snapshots without this field
    /// deserialise as "never queried" rather than rejecting.
    #[serde(default)]
    pub last_query_at_ms: u64,
}

impl From<&DriveStats> for DriveStatsSnapshot {
    fn from(stats: &DriveStats) -> Self {
        Self {
            queries_total: stats.queries_total.load(Ordering::Relaxed),
            rate_ema_micro_per_s: stats.rate_ema_micro_per_s.load(Ordering::Relaxed),
            last_decay_ms: stats.last_decay_ms.load(Ordering::Relaxed),
            last_query_at_ms: stats.last_query_at_ms.load(Ordering::Relaxed),
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
        }
    }
}

/// One shard's runtime state + stats + body.
///
/// Phase 1 held the body unconditionally as `Arc<DriveCompactIndex>`.
/// Phase 3 makes the body optional so demoted (`Parked` / `Cold`)
/// shards can drop their runtime mmap and bloom/trie payload.
///
/// `stats` is wrapped in `Arc<DriveStats>` so a tier-transition rebuild
/// (which replaces this `ShardEntry` with a fresh one inside the
/// registry's `Vec<Arc<ShardEntry>>`) preserves the per-drive
/// counters — the new entry shares the same `Arc<DriveStats>` so
/// concurrent `mark_query_at` writes from in-flight searches still
/// land on the canonical counters.
pub(crate) struct ShardEntry {
    /// Drive letter (`'C'`, `'D'`, …). Capital ASCII per existing
    /// daemon convention.
    pub(crate) drive: char,
    /// Tier state. Read on every search via [`Self::state`]; mutated
    /// only by [`Self::try_transition`] (test-only) or by the
    /// registry's tier-transition rebuilds (production path).
    state: AtomicU8,
    /// Per-drive query stats.  Wrapped in `Arc` so tier transitions
    /// preserve them across `ShardEntry` rebuilds.
    pub(crate) stats: Arc<DriveStats>,
    /// In-memory compact index, present only for `Warm` / `Hot`
    /// tiers.  Cloned cheaply (Arc bump) into
    /// [`crate::cache::ShardRegistry::active_index`] on rebuild for
    /// shards in those states; absent (`None`) for `Parked` / `Cold`
    /// where the runtime mmap has been released.
    body: Option<Arc<DriveCompactIndex>>,
}

impl ShardEntry {
    /// Construct a shard wrapping `body` and pinning it in
    /// [`ShardState::Warm`] with a fresh, all-zero `DriveStats`.
    ///
    /// Used for the boot-time happy path — `IndexManager::add_drive`
    /// and `IndexManager::replace_drive` both flow through this
    /// constructor.  Phase 3 adds [`Self::new_parked`] /
    /// [`Self::new_cold`] for tier-transition rebuilds.
    #[must_use]
    pub(crate) fn new_warm(drive: char, body: Arc<DriveCompactIndex>) -> Self {
        Self {
            drive,
            state: AtomicU8::new(ShardState::Warm as u8),
            stats: Arc::new(DriveStats::new()),
            body: Some(body),
        }
    }

    /// Construct a `Warm` shard wrapping `body` and sharing an
    /// existing `Arc<DriveStats>`.  Mirror of [`Self::new_warm`] for
    /// the promote path: a `Parked` / `Cold` shard's `Arc<DriveStats>`
    /// is lifted into the new `Warm` `ShardEntry` so the per-drive
    /// query counters survive the round-trip through demote-and-back.
    #[must_use]
    pub(crate) const fn new_warm_with_stats(
        drive: char,
        body: Arc<DriveCompactIndex>,
        stats: Arc<DriveStats>,
    ) -> Self {
        Self {
            drive,
            state: AtomicU8::new(ShardState::Warm as u8),
            stats,
            body: Some(body),
        }
    }

    /// Construct a `Parked` shard sharing an existing
    /// `Arc<DriveStats>` (typically lifted off the previous
    /// `Warm` / `Hot` `ShardEntry` for this drive during a tier
    /// transition rebuild).  No body — the runtime mmap has been
    /// released.
    ///
    /// Reached from production via
    /// [`crate::index::IndexManager::demote_idle_shards`] →
    /// [`crate::cache::ShardRegistry::demote_letter`] (Phase 3
    /// Commit D).
    #[must_use]
    pub(crate) const fn new_parked(drive: char, stats: Arc<DriveStats>) -> Self {
        Self {
            drive,
            state: AtomicU8::new(ShardState::Parked as u8),
            stats,
            body: None,
        }
    }

    /// Construct a `Cold` shard sharing an existing
    /// `Arc<DriveStats>`.  No body, no bloom, no trie — a `Cold`
    /// shard is recovered only by re-decrypting the encrypted compact
    /// cache.
    ///
    /// Reached from production via
    /// [`crate::index::IndexManager::demote_idle_shards`] →
    /// [`crate::cache::ShardRegistry::demote_letter`] (Phase 3
    /// Commit D, when a `Parked` shard's idle time exceeds
    /// `PARKED_TO_COLD_IDLE_SECS`).
    #[must_use]
    pub(crate) const fn new_cold(drive: char, stats: Arc<DriveStats>) -> Self {
        Self {
            drive,
            state: AtomicU8::new(ShardState::Cold as u8),
            stats,
            body: None,
        }
    }

    /// Read the current tier state without locking.
    #[must_use]
    pub(crate) fn state(&self) -> ShardState {
        ShardState::from_repr(self.state.load(Ordering::Acquire))
    }

    /// Cheap clone of the in-memory body, present only for
    /// `Warm` / `Hot` shards.  Returns `None` for `Parked` / `Cold` /
    /// `Unknown` / `Evicting`.
    #[must_use]
    pub(crate) fn body(&self) -> Option<Arc<DriveCompactIndex>> {
        self.body.as_ref().map(Arc::clone)
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

// Test suite hosted in the sibling `shard/tests.rs` so this
// production file stays under the workspace 800-LOC cap.  Module
// path `crate::cache::shard::tests` is preserved for any downstream
// consumer that imported individual helpers via that path.
#[cfg(test)]
mod tests;
