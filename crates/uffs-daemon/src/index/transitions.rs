// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tier-transition controllers driven by the daemon's background tasks.
//!
//! Three independent controllers, each spawned from
//! [`crate::run_daemon`], share this module:
//!
//! 1. [`Self::demote_idle_shards`] — Phase 3 Commit D idle-tick demote.  Walks
//!    the registry once per 30 s tick, demotes any shard whose `idle_secs`
//!    exceeds its tier's TTL, and calls
//!    [`crate::cache::working_set::WorkingSetTrim::trim`] once per batch. Phase
//!    6 Commit C wired the static-TTL lookup to the adaptive
//!    [`crate::cache::policy::next_state_for_idle_with_thresholds`] helper:
//!    per-shard thresholds are sized from the live queries/min EMA
//!    ([`crate::cache::shard::DriveStats::decay_ema_qpm`]) plus the user's
//!    `daemon.toml` `[tiers]` section, then clamped by any
//!    `[shards.per_drive."X:"].min_tier` floor (plan tasks 6.4, 6.6).  Every
//!    demote evaluation emits a `shard.ttl` tracing event with the chosen TTL,
//!    the live rate, and a structured reason (plan task 6.7).
//! 2. [`Self::cascade_demote_one_step`] (+ [`Self::subscribe_pressure`]) —
//!    Phase 5 task 5.6 pressure-cascade.  Picks the LRU Warm shard, demotes it
//!    Warm → Parked, and trims the working set; the subscriber loop in `lib.rs`
//!    calls this in a tight loop while
//!    [`crate::cache::pressure::PressureLevel`] reports `Critical`, yielding
//!    between calls so the cascade stops as soon as the pressure clears.
//! 3. [`Self::refresh_usn_for_warm_shards`] (+ helper
//!    [`Self::apply_one_refresh_result`]) — Phase 5 (#95) USN journal replay.
//!    Runs every five minutes (overridable via
//!    [`crate::cache::policy::usn_refresh_interval_secs`]) so long-running
//!    daemons fold MFT changes into their cached bodies without the latency of
//!    a full reload.
//!
//! The three controllers all consume the same Arc-swap registry
//! mutation primitives (`demote_letter`, `replace_warm_body`)
//! exposed by [`crate::cache::registry::ShardRegistry`], and all
//! three call [`Self::bump_index_version`] after a successful
//! mutation so the aggregate cache drops stale entries.  Keeping
//! them in one module keeps that contract visible.

use alloc::sync::Arc;
use std::time::Instant;

use super::IndexManager;
use crate::cache::ShardState;
use crate::cache::policy::{TierThresholds, hot_ttl, parked_ttl, warm_ttl};
use crate::cache::shard::ShardEntry;
use crate::config::{Config, TiersConfig};

impl IndexManager {
    /// Demote any shards whose idle time exceeds the static-TTL
    /// threshold for their current tier.
    ///
    /// Phase 3 Commit D — driven from a 30 s tick task in `lib.rs`
    /// (see `spawn_idle_demote_controller`).  The tick cadence is
    /// shorter than every TTL by design: a Hot shard idle for 1
    /// minute can race past the `HOT_TO_WARM_IDLE_SECS`
    /// (60 s) boundary at most one tick (30 s) before the
    /// controller observes it.  See `cache::policy` for the
    /// per-tier thresholds.
    ///
    /// Three-phase orchestration:
    ///
    /// 1. **Read-lock detect.**  Single `self.index.read()` to enumerate
    ///    `(letter, target)` pairs where the shard's `idle_secs = (now_ms -
    ///    last_query_at_ms) / 1000` reaches its tier's TTL.  Common case (no
    ///    shard past its TTL) exits with one read-lock acquisition.
    /// 2. **Write-lock atomic batch.**  Apply every demote in a single
    ///    write-lock window — N demotes → N registry rebuilds inside one lock
    ///    acquisition, vs N separate write-lock acquisitions.  Each rebuild is
    ///    O(shards) and sub-microsecond at the project's max ≤ 26 drives.
    /// 3. **One `bump_index_version` for the batch.**  The aggregate cache only
    ///    needs to invalidate once even when multiple shards moved.
    ///
    /// `now_ms` is threaded as a parameter so tests can pass
    /// deterministic timestamps (no `tokio::time::pause`
    /// dependency at this layer) and so the spawn task in
    /// `lib.rs` controls when "now" is sampled (once per tick,
    /// shared across every shard's idle computation).
    ///
    /// Race-resilient: if a search promoted the shard between our
    /// detect and the corresponding swap, `demote_letter` returns
    /// `None` for the now-Warm shard, that demote is skipped, the
    /// rest of the batch proceeds, and the next idle-tick
    /// re-evaluates.
    pub(crate) async fn demote_idle_shards(&self, now_ms: u64) {
        // ── Phase 1: read-lock detect ──────────────────────────────
        // Each shard's adaptive thresholds are derived from its
        // live queries/min EMA in [`evaluate_idle_demote`] below;
        // [`min_tier_for_drive`] applies the per-drive
        // `min_tier` floor.  Both helpers also emit `shard.ttl`
        // tracing events so operators can observe the controller's
        // decisions without re-deriving the formulas (plan task
        // 6.7).
        let config = Arc::clone(&self.config);
        let demotes: Vec<(char, ShardState)> = {
            let guard = self.index.read().await;
            guard
                .iter()
                .filter_map(|shard| {
                    evaluate_idle_demote(shard, now_ms, &config).map(|target| (shard.drive, target))
                })
                .collect()
        };
        if demotes.is_empty() {
            return;
        }

        // ── Phase 2: write-lock atomic batch ───────────────────────
        let mut guard = self.index.write().await;
        let mut applied = 0_usize;
        for (letter, target) in demotes {
            if let Some(new_registry) = guard.demote_letter(letter, target) {
                *guard = Arc::new(new_registry);
                applied += 1;
            }
        }
        drop(guard);

        // ── Phase 3: single index-version bump for the batch ───────
        if applied > 0 {
            self.bump_index_version();

            // ── Phase 4: working-set trim (Phase 5 task 5.4) ─────
            // Process-level hook called once per batch (not once
            // per shard) — on Windows `EmptyWorkingSet` is process-
            // wide so coalescing matters; on Mac/Linux the call is
            // a no-op stub.  Best-effort: any I/O error is logged
            // and the daemon continues.  Runs after the index-
            // version bump so the demote is fully observable to
            // searches before we ask the kernel to reclaim pages.
            if let Err(err) = self.working_set_trim.trim() {
                tracing::warn!(
                    target: "shard.transition",
                    error = %err,
                    applied,
                    reason = "demote-batch",
                    "WorkingSetTrim::trim failed; daemon continues",
                );
            }
        }
    }

    /// Subscribe to memory-pressure transitions (Phase 5 task 5.6).
    ///
    /// Returns a [`tokio::sync::watch::Receiver`] carrying the
    /// current [`PressureLevel`] and waking on every transition.
    /// The daemon's `spawn_pressure_subscriber` (in `lib.rs`) is
    /// the sole production consumer; the Phase 5 task 5.10 test
    /// uses [`Self::cascade_demote_one_step`] directly without
    /// going through the watch channel.
    ///
    /// [`PressureLevel`]: crate::cache::pressure::PressureLevel
    pub(crate) fn subscribe_pressure(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::cache::pressure::PressureLevel> {
        self.pressure.subscribe()
    }

    /// Cascade-demote one LRU Warm shard to Parked (Phase 5 task 5.6).
    ///
    /// Picks the Warm shard with the **oldest**
    /// `DriveStats::last_query_at_ms` and demotes it one tier
    /// (Warm → Parked).  Returns `Some((letter, ShardState::Parked))`
    /// when work was done, `None` when no Warm shards remain (the
    /// caller stops the cascade).
    ///
    /// **LRU contract** (closes the deferred Phase 3 task 3.6).  The
    /// per-shard `last_query_at_ms` already exists from Phase 3; the
    /// "LRU bookkeeping" task 3.6 alluded to is just a sort at
    /// demote-time — no separate ordering data structure is needed,
    /// since the cascade fires rarely (only on Windows pressure
    /// transitions) and the Warm subset is small (one shard per
    /// loaded drive, capped at the indexed-drive count).
    ///
    /// **Working-set trim**.  Each cascade step calls
    /// [`WorkingSetTrim::trim`] once.  Unlike the idle-demote
    /// batch — where one trim per batch coalesces N shards — the
    /// cascade is one shard per call by design (the subscriber
    /// loop yields between calls so a `High` transition can stop
    /// the cascade promptly), so there's no batch to coalesce.
    ///
    /// [`WorkingSetTrim::trim`]: crate::cache::working_set::WorkingSetTrim::trim
    pub(crate) async fn cascade_demote_one_step(&self) -> Option<(char, ShardState)> {
        // ── Phase 1: read-lock detect (LRU pick) ────────────────────
        // Enumerate Warm shards and keep the one with the oldest
        // `last_query_at_ms`.  `min_by_key` returns `None` when no
        // Warm shards exist; the caller stops the cascade.
        let pick: Option<(char, u64)> = {
            let guard = self.index.read().await;
            guard
                .iter()
                .filter(|shard| shard.state() == ShardState::Warm)
                .map(|shard| (shard.drive, shard.stats.last_query_at_ms()))
                .min_by_key(|&(_, ts)| ts)
        };
        let (letter, _last_query_at_ms) = pick?;

        // ── Phase 2: write-lock atomic single-shard demote ─────────
        // Re-check inside the write lock — a concurrent promote
        // could have moved the picked shard back to Hot/Warm
        // between the read-lock and the write-lock acquisition.
        // `demote_letter_with_reason` returns `None` for an illegal
        // transition, in which case we skip and the next cascade
        // tick re-picks.  The `PressureCascade` reason flows into
        // the canonical `shard.transition` event so operators can
        // distinguish cascade demotes from TTL idle demotes by
        // grepping `reason="pressure-cascade"` (Phase 5 G4
        // follow-up — the cascade no longer emits its own
        // duplicate event of the same demote).
        let target = ShardState::Parked;
        let mut guard = self.index.write().await;
        let new_registry = guard.demote_letter_with_reason(
            letter,
            target,
            crate::cache::registry::DemoteReason::PressureCascade,
        )?;
        *guard = Arc::new(new_registry);
        drop(guard);
        self.bump_index_version();

        // ── Phase 3: working-set trim (Phase 5 task 5.4 reuse) ────
        // One trim per cascade step (vs once per idle-demote batch)
        // — see method-level docs for the rationale.  Best-effort:
        // any I/O error is logged at `target: "shard.transition"`
        // and the daemon continues.
        if let Err(err) = self.working_set_trim.trim() {
            tracing::warn!(
                target: "shard.transition",
                drive = %letter,
                error = %err,
                reason = "pressure-cascade",
                "WorkingSetTrim::trim failed; daemon continues",
            );
        }

        Some((letter, target))
    }

    /// Phase 5 (#95) — fold live USN journal deltas into every
    /// `Warm` / `Hot` shard's in-memory body and persist a fresher
    /// compact cache to disk.
    ///
    /// Driven from a periodic tick task in `lib.rs`
    /// (`spawn_usn_refresh_controller`); the cadence defaults to
    /// `cache::policy::USN_REFRESH_INTERVAL_SECS` (5 min) and is
    /// overridable via `UFFS_USN_REFRESH_INTERVAL_SECS` for tests
    /// and benchmarks.
    ///
    /// Three-phase like [`Self::ensure_warm_for_dispatch`] (Phase 4
    /// re-promote in #93):
    ///
    /// 1. **Read-lock detect** — collect Warm/Hot drive letters.
    /// 2. **Parallel USN refresh** — fan out into the blocking pool via
    ///    [`tokio::task::JoinSet`] so one slow drive doesn't serialise the
    ///    others (mirrors the #93 pattern).  Each closure is
    ///    `catch_unwind`-wrapped so a panicking USN apply on one drive doesn't
    ///    lose the letter identifier in the [`tokio::task::JoinSet`] error arm.
    /// 3. **Per-letter write-lock swap** — drain results as they complete and
    ///    `replace_warm_body` the registry; sub-µs Arc-swap per letter,
    ///    cumulative contention < 10 µs even at N=7 drives.
    ///
    /// **Failure handling**: per-drive USN refresh failures (cache
    /// missing, journal unavailable, drive G `error 1179`) are
    /// warn-logged at `target: "shard.refresh"` and the shard's
    /// previous body stays in place.  Aggregate counters are
    /// emitted at info-level on completion so production telemetry
    /// can monitor the refresh success rate.
    ///
    /// **Non-Windows behaviour**: the underlying
    /// [`uffs_core::compact_loader::load_drive_with_usn_refresh`]
    /// helper errors out by design (USN journals are NTFS-only),
    /// so this loop becomes a no-op refresh tick that just walks
    /// the registry and logs the per-drive errors.  The structure
    /// is exercised on macOS / Linux for testing parity.
    pub(crate) async fn refresh_usn_for_warm_shards(&self) {
        // ── Phase 1: read-lock detect Warm/Hot letters ─────────────
        let letters: Vec<char> = {
            let guard = self.index.read().await;
            guard
                .iter()
                .filter(|shard| matches!(shard.state(), ShardState::Warm | ShardState::Hot))
                .map(|shard| shard.drive)
                .collect()
        };
        if letters.is_empty() {
            return;
        }

        let total_start = Instant::now();
        let total = letters.len();
        tracing::info!(
            target: "shard.refresh",
            count = total,
            interval_secs = crate::cache::policy::usn_refresh_interval_secs(),
            "USN refresh tick starting",
        );

        // ── Phase 2: parallel USN refresh via JoinSet ──────────────
        // Each per-letter closure enters
        // [`crate::cache::background_io::BackgroundIoScope`]
        // (Phase 5 task 5.7) at the top so the USN-journal read +
        // delta apply + `save_compact_cache_background` write all
        // run at Windows `THREAD_MODE_BACKGROUND_BEGIN` priority,
        // yielding to any foreground RPC handler under disk
        // contention.  RAII via `_bg_scope` ensures the matching
        // `_END` fires even if the inner closure panics — the
        // blocking pool thread returns to normal priority before
        // it gets reused for unrelated work.  No-op on Mac/Linux.
        let mut load_set: tokio::task::JoinSet<(
            char,
            anyhow::Result<Arc<uffs_core::compact::DriveCompactIndex>>,
        )> = tokio::task::JoinSet::new();
        for letter in letters {
            let background_io = Arc::clone(&self.background_io);
            load_set.spawn_blocking(move || {
                let _bg_scope =
                    crate::cache::background_io::BackgroundIoScope::enter(background_io);
                // `catch_unwind` mirrors the #93 pattern: convert a
                // panicking refresh closure into a typed error so
                // the JoinSet's error arm doesn't lose the letter.
                let result = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
                    uffs_core::compact_loader::load_drive_with_usn_refresh(letter)
                        .map(|(body, _timing)| Arc::new(body))
                }))
                .unwrap_or_else(|_payload| {
                    Err(anyhow::anyhow!(
                        "panic in USN refresh blocking closure for drive {letter}"
                    ))
                });
                (letter, result)
            });
        }

        // ── Phase 3: drain + per-letter write-lock swap ────────────
        let mut refreshed = 0_usize;
        let mut failed = 0_usize;
        while let Some(joined) = load_set.join_next().await {
            if self.apply_one_refresh_result(joined).await {
                refreshed += 1;
            } else {
                failed += 1;
            }
        }

        tracing::info!(
            target: "shard.refresh",
            refreshed,
            failed,
            total,
            total_ms = total_start.elapsed().as_millis(),
            "USN refresh tick complete",
        );
    }

    /// Apply a single drained [`tokio::task::JoinSet::join_next`]
    /// result from the Phase 5 (#95) USN refresh fan-out.
    ///
    /// Returns `true` when the body was successfully Arc-swapped
    /// into the registry; `false` on any failure (panicked closure,
    /// USN refresh helper error, registry race where the shard
    /// demoted between detect and swap).  The caller (
    /// [`Self::refresh_usn_for_warm_shards`]) accumulates the
    /// boolean into per-tick success/failure counters.
    ///
    /// Extracted from the parent so the parent stays under
    /// clippy's strict-gate cognitive-complexity ceiling.
    async fn apply_one_refresh_result(
        &self,
        joined: Result<
            (
                char,
                anyhow::Result<Arc<uffs_core::compact::DriveCompactIndex>>,
            ),
            tokio::task::JoinError,
        >,
    ) -> bool {
        let (letter, result) = match joined {
            Ok(pair) => pair,
            Err(join_err) => {
                tracing::warn!(
                    target: "shard.refresh",
                    error = %join_err,
                    "blocking-task aborted before completion; shard kept previous body",
                );
                return false;
            }
        };
        let body = match result {
            Ok(body) => body,
            Err(err) => {
                tracing::warn!(
                    target: "shard.refresh",
                    drive = %letter,
                    error = %err,
                    "USN refresh failed; shard kept previous body",
                );
                return false;
            }
        };
        let mut guard = self.index.write().await;
        let Some(new_registry) = guard.replace_warm_body(letter, body) else {
            // Race: the shard demoted between Phase 1 detect and
            // the swap.  No-op; the next promote will USN-refresh
            // via DiskBodyLoader.
            return false;
        };
        *guard = Arc::new(new_registry);
        drop(guard);
        self.bump_index_version();
        true
    }
}

// ── Phase 6 Commit C — adaptive idle-demote helpers ──────────────────────

/// Build a [`TierThresholds`] from a live queries/min EMA plus the
/// `[tiers]` section of the daemon's parsed [`Config`].
///
/// Plan §5.2 / task 6.4: `hot_ttl_secs = (base + 60·log2(rate))`
/// clamped to `[base, cap]`; `warm_ttl_secs = (base +
/// 600·log2(rate))` clamped to `[base, cap]`; `parked_ttl_secs`
/// has no rate dependence.  The three formulas are implemented in
/// [`crate::cache::policy::hot_ttl`] /
/// [`crate::cache::policy::warm_ttl`] /
/// [`crate::cache::policy::parked_ttl`] — this helper composes
/// them into a single [`TierThresholds`] the demote controller
/// can pass straight to
/// [`crate::cache::policy::next_state_for_idle_with_thresholds`].
fn build_thresholds(rate_qpm: f64, tiers: &TiersConfig) -> TierThresholds {
    TierThresholds {
        hot_to_warm_secs: hot_ttl(rate_qpm, tiers.hot_ttl_base_secs, tiers.hot_ttl_cap_secs)
            .as_secs(),
        warm_to_parked_secs: warm_ttl(rate_qpm, tiers.warm_ttl_base_secs, tiers.warm_ttl_cap_secs)
            .as_secs(),
        parked_to_cold_secs: parked_ttl(tiers.parked_ttl_secs).as_secs(),
    }
}

/// Resolve the `[shards.per_drive."<letter>:"].min_tier` override
/// for `letter`, lifted into a [`ShardState`] so the demote
/// clamp can compare it against a proposed target tier.
///
/// Returns `None` when the user hasn't set a per-drive `min_tier`
/// for the given letter — meaning the demote ladder bottoms at
/// [`ShardState::Cold`] like the Phase 3 static behavior.
///
/// Lookup is case-insensitive on the letter and tolerates either
/// `"C"` or `"C:"` styled keys: callers tend to mix conventions
/// (the plan §11 example uses `"C:"`; the env-var-style convention
/// just uses `"C"`), and forcing one form would silently swallow
/// the user's intent.
fn min_tier_for_drive(letter: char, config: &Config) -> Option<ShardState> {
    let letter_upper = letter.to_ascii_uppercase();
    config
        .shards
        .per_drive
        .iter()
        .find(|(key, _)| {
            let trimmed = key.trim_end_matches(':');
            trimmed.len() == 1
                && trimmed
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.eq_ignore_ascii_case(&letter_upper))
        })
        .and_then(|(_, per_drive)| per_drive.min_tier)
        .map(crate::config::TierLevel::to_state)
}

/// Tier residency rank — higher ⇒ more resident.
///
/// Used to compare a proposed demote target against a per-drive
/// `min_tier` floor: `target_rank < floor_rank` means the demote
/// would push the shard below the user's configured floor and
/// must be suppressed.
///
/// `Unknown` and `Evicting` are mid-transition states and never
/// surface as a demote target — they get rank 0 so any comparison
/// against them is conservatively a no-op.
const fn tier_rank(state: ShardState) -> u8 {
    match state {
        ShardState::Hot => 4,
        ShardState::Warm => 3,
        ShardState::Parked => 2,
        ShardState::Cold => 1,
        ShardState::Unknown | ShardState::Evicting => 0,
    }
}

/// Plan task 6.4 + 6.7 — evaluate one shard for adaptive idle
/// demotion and emit the `shard.ttl` tracing event.
///
/// Returns `Some(target)` when the shard should demote to the
/// returned tier; `None` when it stays put.  The decision flow:
///
/// 1. Sample the shard's queries/min EMA via
///    [`crate::cache::shard::DriveStats::decay_ema_qpm`] (also advances the
///    EMA's decay clock — fine because the controller samples once per shard
///    per tick, never re-reading without decay).
/// 2. Build per-shard [`TierThresholds`] from `[tiers]` config + the rate via
///    [`build_thresholds`].
/// 3. Run [`crate::cache::policy::next_state_for_idle_with_thresholds`] to get
///    a proposed target tier (or `None` if not yet idle).
/// 4. Apply the per-drive [`min_tier_for_drive`] clamp (plan task 6.6) — if the
///    proposed target is below the user's `min_tier` floor, suppress the demote
///    and emit a `shard.ttl` event with `reason = "min-tier-clamp"`.
/// 5. Emit a `shard.ttl` debug event with the chosen TTL, the live rate, and
///    the structured reason — matches §4.2 of the implementation plan: `target
///    = "shard.ttl"; fields = drive, chosen_ttl_sec, reason`.
fn evaluate_idle_demote(shard: &ShardEntry, now_ms: u64, config: &Config) -> Option<ShardState> {
    let last = shard.stats.last_query_at_ms();
    let idle_ms = now_ms.saturating_sub(last);
    let idle_secs = idle_ms / 1000;
    let rate_qpm = shard.stats.decay_ema_qpm(now_ms);
    let thresholds = build_thresholds(rate_qpm, &config.tiers);
    let current = shard.state();
    let chosen_ttl_sec = chosen_ttl_for_state(current, &thresholds);

    let target =
        crate::cache::policy::next_state_for_idle_with_thresholds(current, idle_secs, &thresholds);

    match (target, min_tier_for_drive(shard.drive, config)) {
        (Some(proposed), Some(floor)) if tier_rank(proposed) < tier_rank(floor) => {
            tracing::debug!(
                target: "shard.ttl",
                drive = %shard.drive,
                from = ?current,
                proposed = ?proposed,
                min_tier = ?floor,
                chosen_ttl_sec,
                rate_qpm,
                reason = "min-tier-clamp",
                "Demote target clamped by per-drive min_tier",
            );
            None
        }
        (Some(proposed), _) => {
            tracing::debug!(
                target: "shard.ttl",
                drive = %shard.drive,
                from = ?current,
                to = ?proposed,
                chosen_ttl_sec,
                rate_qpm,
                reason = "idle-demote",
                "Adaptive idle-demote evaluation produced demote target",
            );
            Some(proposed)
        }
        (None, _) => {
            tracing::trace!(
                target: "shard.ttl",
                drive = %shard.drive,
                from = ?current,
                idle_secs,
                chosen_ttl_sec,
                rate_qpm,
                reason = "below-ttl",
                "Adaptive idle-demote evaluation: not yet idle past TTL",
            );
            None
        }
    }
}

/// Pick the threshold that gates the current tier's outgoing
/// edge — the value the user's config would need to raise to keep
/// the shard at its current tier longer.  The Cold / Unknown /
/// Evicting tiers don't demote further so the helper returns 0
/// for them; the tracing event still records the value so a log
/// reader can disambiguate "no edge" from "edge at zero".
const fn chosen_ttl_for_state(state: ShardState, thresholds: &TierThresholds) -> u64 {
    match state {
        ShardState::Hot => thresholds.hot_to_warm_secs,
        ShardState::Warm => thresholds.warm_to_parked_secs,
        ShardState::Parked => thresholds.parked_to_cold_secs,
        ShardState::Cold | ShardState::Unknown | ShardState::Evicting => 0,
    }
}

#[cfg(test)]
mod commit_c_tests {
    use super::*;
    use crate::config::{PerDriveConfig, ShardsConfig, TierLevel};

    /// Plan task 6.6 — `[shards.per_drive."C:"].min_tier = "WARM"`
    /// must prevent the demote controller from dropping C below
    /// `Warm`, even when `idle_secs` would otherwise demote it
    /// further (e.g. Warm → Parked).
    ///
    /// Pure-function test: exercises [`min_tier_for_drive`] +
    /// [`tier_rank`] without spinning up an `IndexManager`.
    #[test]
    fn per_drive_min_tier_clamps_proposed_demote_below_floor() {
        let mut per_drive = alloc::collections::BTreeMap::new();
        per_drive.insert(String::from("C:"), PerDriveConfig {
            min_tier: Some(TierLevel::Warm),
            max_tier: None,
        });
        let config = Config {
            shards: ShardsConfig {
                per_drive,
                ..ShardsConfig::default()
            },
            ..Config::default()
        };

        // C: has min_tier = Warm.  The Warm → Parked proposal is
        // below the floor and must be suppressed.
        let floor = min_tier_for_drive('C', &config).expect("min_tier resolved for C");
        assert_eq!(floor, ShardState::Warm);
        assert!(tier_rank(ShardState::Parked) < tier_rank(floor));
        // The Hot → Warm proposal is at the floor and is allowed.
        assert!(tier_rank(ShardState::Warm) >= tier_rank(floor));
    }

    /// `min_tier_for_drive` lookup is case-insensitive on the
    /// letter and tolerates the `"C"`-without-colon convention.
    /// Pin both forms so the user can write either in
    /// `daemon.toml` without surprises.
    #[test]
    fn min_tier_for_drive_case_insensitive_and_colon_optional() {
        let mut per_drive = alloc::collections::BTreeMap::new();
        per_drive.insert(String::from("c"), PerDriveConfig {
            min_tier: Some(TierLevel::Warm),
            max_tier: None,
        });
        per_drive.insert(String::from("D:"), PerDriveConfig {
            min_tier: Some(TierLevel::Hot),
            max_tier: None,
        });
        let config = Config {
            shards: ShardsConfig {
                per_drive,
                ..ShardsConfig::default()
            },
            ..Config::default()
        };

        assert_eq!(min_tier_for_drive('C', &config), Some(ShardState::Warm));
        assert_eq!(min_tier_for_drive('c', &config), Some(ShardState::Warm));
        assert_eq!(min_tier_for_drive('D', &config), Some(ShardState::Hot));
        assert_eq!(min_tier_for_drive('d', &config), Some(ShardState::Hot));
        // Untouched letter falls through to the static-Cold ladder.
        assert_eq!(min_tier_for_drive('E', &config), None);
    }

    /// Plan task 6.4 — at `rate_qpm = 0` the adaptive thresholds
    /// collapse to the `*_base_secs` floors, matching the Phase 3
    /// static ladder.  Pin the boundary so a future formula tweak
    /// can't silently drift idle drives away from the documented
    /// defaults.
    #[test]
    fn build_thresholds_at_zero_rate_matches_base_secs() {
        let tiers = TiersConfig::default();
        let thresholds = build_thresholds(0.0, &tiers);
        assert_eq!(thresholds.hot_to_warm_secs, tiers.hot_ttl_base_secs);
        assert_eq!(thresholds.warm_to_parked_secs, tiers.warm_ttl_base_secs);
        assert_eq!(thresholds.parked_to_cold_secs, tiers.parked_ttl_secs);
    }

    /// Plan task 6.4 reference point — `rate_qpm = 20` produces
    /// the documented bonus on the Hot edge: `hot_ttl_secs ≈
    /// 60 + 60·log2(20) ≈ 319 s` (formula verified in
    /// `crate::cache::policy::tests::hot_ttl_at_rate_20_qpm_matches_formula`).
    /// Pin the integration here so the controller path can't drift
    /// from the policy module's contract.
    #[test]
    fn build_thresholds_at_rate_20_extends_hot_edge() {
        let tiers = TiersConfig::default();
        let thresholds = build_thresholds(20.0, &tiers);
        assert!(
            thresholds.hot_to_warm_secs > tiers.hot_ttl_base_secs,
            "hot edge must be extended above the base at rate=20",
        );
        // Bound it loosely so a `log2` precision fix won't break
        // the test; the tight pin lives in the policy module.
        assert!(
            thresholds.hot_to_warm_secs <= tiers.hot_ttl_cap_secs,
            "hot edge must remain at or below the cap",
        );
    }

    /// Plan task 6.7 — the `chosen_ttl_for_state` helper returns
    /// the threshold gating the current tier's demote edge so the
    /// `shard.ttl` tracing event carries a meaningful value for
    /// log readers.  Pin the per-tier mapping.
    #[test]
    fn chosen_ttl_for_state_picks_outgoing_edge() {
        let thresholds = TierThresholds {
            hot_to_warm_secs: 11,
            warm_to_parked_secs: 22,
            parked_to_cold_secs: 33,
        };
        assert_eq!(chosen_ttl_for_state(ShardState::Hot, &thresholds), 11);
        assert_eq!(chosen_ttl_for_state(ShardState::Warm, &thresholds), 22);
        assert_eq!(chosen_ttl_for_state(ShardState::Parked, &thresholds), 33);
        assert_eq!(chosen_ttl_for_state(ShardState::Cold, &thresholds), 0);
    }
}
