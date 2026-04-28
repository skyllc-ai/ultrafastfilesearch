// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Static-TTL placeholder policy for the Phase 3 demote/promote
//! controller.
//!
//! Phase 3 of the memory-tiering work
//! (`docs/refactor/memory-tiering-implementation-plan.md`).
//!
//! Adaptive TTL controllers live in Phase 6; for now every shard
//! shares the same fixed thresholds:
//!
//! * **Hot → Warm** at [`HOT_TO_WARM_IDLE_SECS`] (1 min idle, default).
//! * **Warm → Parked** at [`WARM_TO_PARKED_IDLE_SECS`] (5 min idle, default).
//! * **Parked → Cold** at [`PARKED_TO_COLD_IDLE_SECS`] (24 h idle, default).
//!
//! These defaults give a fast Hot → Parked test cycle (~6 min) so
//! the Phase 4 re-promote path is exercisable during normal dev
//! iteration without dragging in `UFFS_*_IDLE_SECS` env-var
//! overrides on every launch.  Production users who want longer
//! retention windows can still set the overrides; the env-var
//! pathway is unchanged.
//!
//! Each default is overridable at daemon startup via env var.
//! The override is read once and cached; restart the daemon to
//! pick up a change.
//!
//! ```text
//! UFFS_HOT_TO_WARM_IDLE_SECS=60      \
//! UFFS_WARM_TO_PARKED_IDLE_SECS=360  \
//! UFFS_PARKED_TO_COLD_IDLE_SECS=900  \
//!     uffs daemon start --drive C,D,E,F,G,M,S
//! ```
//!
//! [`next_state_for_idle`] is the single decision function consumed by
//! `crate::cache::registry::ShardRegistry::demote_idle_shards`; it
//! returns `None` when the shard is not yet idle past its current
//! tier's TTL or when the tier is already at the bottom.

use std::sync::OnceLock;

use super::shard::ShardState;

/// Default for the `Hot` → `Warm` idle threshold.
///
/// Overridable at daemon startup via [`HOT_TO_WARM_IDLE_ENV`].
/// Phase 4+ extends "no query" to "no bloom-positive query" so cold
/// drives don't re-warm just because their bloom got faulted in by
/// a wildcard scan.
///
/// 60 s default makes the Hot → Warm transition observable in
/// normal dev iteration; the runtime-mmap rebuild on the next
/// query is sub-millisecond so the demotion is essentially free.
///
/// Consumed by [`crate::index::IndexManager::demote_idle_shards`]
/// (Phase 3 Commit D) via [`next_state_for_idle`].
pub(crate) const HOT_TO_WARM_IDLE_SECS: u64 = 60;

/// Default for the `Warm` → `Parked` idle threshold.
///
/// Overridable at daemon startup via [`WARM_TO_PARKED_IDLE_ENV`].
/// `Parked` drops the runtime mmap (the records / names columns
/// are released; bloom + trie persist for Phase 4+ search-skip).
///
/// 5 min default lets dev iteration exercise the Phase 4
/// re-promote path (which #93 parallelised) without waiting the
/// 30 min the original prototype used.  The bloom + trie pre-check
/// from Commit F means a Parked re-promote only re-decrypts the
/// compact body when the query actually hits the drive, so the
/// extra demote/promote churn at this cadence is bounded by query
/// pattern, not by the timer alone.
pub(crate) const WARM_TO_PARKED_IDLE_SECS: u64 = 300;

/// Default for the `Parked` → `Cold` idle threshold.
///
/// Overridable at daemon startup via [`PARKED_TO_COLD_IDLE_ENV`].
/// `Cold` drops bloom + trie too — re-promotion requires a full
/// re-decrypt of the encrypted compact cache, so the threshold is
/// generous (24 h) by default.  Anything shorter risks thrashing
/// under nightly batch processes that scan archives once per day.
pub(crate) const PARKED_TO_COLD_IDLE_SECS: u64 = 86_400;

/// Default cadence for the Phase 5 (#95) background USN refresh
/// controller.  Every `USN_REFRESH_INTERVAL_SECS` (default 5 min)
/// the daemon walks all `Warm` / `Hot` shards and folds live USN
/// journal deltas into their in-memory body so search results
/// reflect the live filesystem state without waiting for the next
/// idle-tier transition.
///
/// 5 min is a deliberate trade-off: short enough that `Warm` shards
/// don't drift more than `WARM_TO_PARKED_IDLE_SECS / 6` between
/// refreshes (so nothing demotes mid-refresh), long enough that the
/// USN journal accumulates a meaningful batch (per-drive replay
/// dominated by fixed setup, not by record count).  Override at
/// daemon startup via [`USN_REFRESH_INTERVAL_ENV`].
pub(crate) const USN_REFRESH_INTERVAL_SECS: u64 = 300;

/// Env var that overrides [`HOT_TO_WARM_IDLE_SECS`].
pub(crate) const HOT_TO_WARM_IDLE_ENV: &str = "UFFS_HOT_TO_WARM_IDLE_SECS";

/// Env var that overrides [`WARM_TO_PARKED_IDLE_SECS`].
pub(crate) const WARM_TO_PARKED_IDLE_ENV: &str = "UFFS_WARM_TO_PARKED_IDLE_SECS";

/// Env var that overrides [`PARKED_TO_COLD_IDLE_SECS`].
pub(crate) const PARKED_TO_COLD_IDLE_ENV: &str = "UFFS_PARKED_TO_COLD_IDLE_SECS";

/// Env var that overrides [`USN_REFRESH_INTERVAL_SECS`].
pub(crate) const USN_REFRESH_INTERVAL_ENV: &str = "UFFS_USN_REFRESH_INTERVAL_SECS";

/// Read a positive `u64` seconds value from `env_name`, falling back
/// to `default` on any parse error or non-positive value.  Logs a
/// single startup line per override so the effective policy is
/// observable in production.
fn read_env_secs(env_name: &str, default: u64) -> u64 {
    let Ok(raw) = std::env::var(env_name) else {
        return default;
    };
    let parsed: Option<u64> = raw.trim().parse::<u64>().ok().filter(|&n| n > 0);
    let effective = parsed.unwrap_or(default);
    if parsed.is_some() {
        tracing::info!(
            target: "shard.policy",
            env_var = env_name,
            override_secs = effective,
            default_secs = default,
            "idle-threshold override active",
        );
    } else {
        tracing::warn!(
            target: "shard.policy",
            env_var = env_name,
            raw = %raw,
            default_secs = default,
            "idle-threshold env var unparseable; using default",
        );
    }
    effective
}

/// Effective `Hot` → `Warm` idle threshold (env override or default).
#[must_use]
pub(crate) fn hot_to_warm_idle_secs() -> u64 {
    static CACHED: OnceLock<u64> = OnceLock::new();
    *CACHED.get_or_init(|| read_env_secs(HOT_TO_WARM_IDLE_ENV, HOT_TO_WARM_IDLE_SECS))
}

/// Effective `Warm` → `Parked` idle threshold (env override or default).
#[must_use]
pub(crate) fn warm_to_parked_idle_secs() -> u64 {
    static CACHED: OnceLock<u64> = OnceLock::new();
    *CACHED.get_or_init(|| read_env_secs(WARM_TO_PARKED_IDLE_ENV, WARM_TO_PARKED_IDLE_SECS))
}

/// Effective `Parked` → `Cold` idle threshold (env override or default).
#[must_use]
pub(crate) fn parked_to_cold_idle_secs() -> u64 {
    static CACHED: OnceLock<u64> = OnceLock::new();
    *CACHED.get_or_init(|| read_env_secs(PARKED_TO_COLD_IDLE_ENV, PARKED_TO_COLD_IDLE_SECS))
}

/// Effective USN refresh interval (env override or default), in
/// seconds.  Consumed by the Phase 5 (#95) background USN refresh
/// controller spawned from `crate::lib::run_daemon`.
#[must_use]
pub(crate) fn usn_refresh_interval_secs() -> u64 {
    static CACHED: OnceLock<u64> = OnceLock::new();
    *CACHED.get_or_init(|| read_env_secs(USN_REFRESH_INTERVAL_ENV, USN_REFRESH_INTERVAL_SECS))
}

/// Decide whether a shard in `state` that has been idle for
/// `idle_secs` seconds should demote to a colder tier.
///
/// Reads the effective tier thresholds via the
/// [`hot_to_warm_idle_secs`] / [`warm_to_parked_idle_secs`] /
/// [`parked_to_cold_idle_secs`] getters, so any
/// `UFFS_*_IDLE_SECS` env-var override set before daemon startup is
/// honoured here too.
///
/// Returns the target state on demote, or `None` when:
///
/// * the shard has not been idle long enough for its current tier, or
/// * the shard is already at the bottom of the tier ladder
///   ([`ShardState::Cold`], [`ShardState::Unknown`], [`ShardState::Evicting`])
///   — only the controller produces those states, the policy never selects them
///   as a *target*.
///
/// `Hot` skips straight to `Warm` rather than `Parked` so a brief
/// idle window doesn't re-cost a runtime-mmap rebuild on the next
/// query.
///
/// Wired into the production demote path by
/// [`crate::index::IndexManager::demote_idle_shards`] (Phase 3
/// Commit D).
#[must_use]
pub(crate) fn next_state_for_idle(state: ShardState, idle_secs: u64) -> Option<ShardState> {
    match state {
        ShardState::Hot if idle_secs >= hot_to_warm_idle_secs() => Some(ShardState::Warm),
        ShardState::Warm if idle_secs >= warm_to_parked_idle_secs() => Some(ShardState::Parked),
        ShardState::Parked if idle_secs >= parked_to_cold_idle_secs() => Some(ShardState::Cold),
        // Two distinct cases collapsed into one arm because they
        // share the same outcome:
        //   * Hot / Warm / Parked: idle window not exceeded for the current tier (the guard above
        //     this arm failed).
        //   * Cold / Unknown / Evicting: floor + controller-only states; the policy never targets
        //     these.
        ShardState::Hot
        | ShardState::Warm
        | ShardState::Parked
        | ShardState::Cold
        | ShardState::Unknown
        | ShardState::Evicting => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_demotes_to_warm_at_threshold() {
        assert_eq!(
            next_state_for_idle(ShardState::Hot, HOT_TO_WARM_IDLE_SECS),
            Some(ShardState::Warm)
        );
        assert_eq!(
            next_state_for_idle(ShardState::Hot, HOT_TO_WARM_IDLE_SECS + 1),
            Some(ShardState::Warm)
        );
    }

    #[test]
    fn hot_does_not_demote_below_threshold() {
        assert_eq!(next_state_for_idle(ShardState::Hot, 0), None);
        assert_eq!(
            next_state_for_idle(ShardState::Hot, HOT_TO_WARM_IDLE_SECS - 1),
            None
        );
    }

    #[test]
    fn warm_demotes_to_parked_at_threshold() {
        assert_eq!(
            next_state_for_idle(ShardState::Warm, WARM_TO_PARKED_IDLE_SECS),
            Some(ShardState::Parked)
        );
    }

    #[test]
    fn warm_does_not_demote_below_threshold() {
        // Crucially below the warm-to-parked threshold even though
        // it's already past the hot-to-warm one — `Warm` shards don't
        // care about the hot threshold.
        assert_eq!(
            next_state_for_idle(ShardState::Warm, HOT_TO_WARM_IDLE_SECS),
            None
        );
        assert_eq!(
            next_state_for_idle(ShardState::Warm, WARM_TO_PARKED_IDLE_SECS - 1),
            None
        );
    }

    #[test]
    fn parked_demotes_to_cold_at_threshold() {
        assert_eq!(
            next_state_for_idle(ShardState::Parked, PARKED_TO_COLD_IDLE_SECS),
            Some(ShardState::Cold)
        );
    }

    #[test]
    fn parked_does_not_demote_below_threshold() {
        assert_eq!(
            next_state_for_idle(ShardState::Parked, WARM_TO_PARKED_IDLE_SECS),
            None
        );
        assert_eq!(
            next_state_for_idle(ShardState::Parked, PARKED_TO_COLD_IDLE_SECS - 1),
            None
        );
    }

    #[test]
    fn floor_states_never_demote() {
        for state in [ShardState::Cold, ShardState::Unknown, ShardState::Evicting] {
            for idle_secs in [
                0,
                HOT_TO_WARM_IDLE_SECS,
                WARM_TO_PARKED_IDLE_SECS,
                PARKED_TO_COLD_IDLE_SECS,
                u64::MAX,
            ] {
                assert_eq!(
                    next_state_for_idle(state, idle_secs),
                    None,
                    "{state:?} at {idle_secs}s should never demote"
                );
            }
        }
    }

    /// Pin the ladder so a future tweak can't accidentally make
    /// `PARKED_TO_COLD_IDLE_SECS` shorter than
    /// `WARM_TO_PARKED_IDLE_SECS`, which would mean a parked shard
    /// could demote to Cold faster than a warm one demotes to
    /// Parked.  Compile-time `const _: ()` so the invariant is
    /// enforced at build time, not at test run.
    const _: () = assert!(HOT_TO_WARM_IDLE_SECS < WARM_TO_PARKED_IDLE_SECS);
    const _: () = assert!(WARM_TO_PARKED_IDLE_SECS < PARKED_TO_COLD_IDLE_SECS);

    /// `read_env_secs` falls back to the supplied default when the
    /// env var is unset.  Pins the contract that a non-overridden
    /// daemon uses the in-source thresholds.  Uses a deliberately
    /// unique env-var name (`UFFS_TEST_ENV_NEVER_SET`) so the test
    /// is independent of any `UFFS_*_IDLE_SECS` value the developer
    /// happens to have exported in their shell.
    #[test]
    fn read_env_secs_falls_back_when_env_unset() {
        let val = read_env_secs("UFFS_TEST_ENV_NEVER_SET_12345", 42);
        assert_eq!(val, 42, "unset env var → default");
    }
}
