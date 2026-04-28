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
//! * **Hot → Warm** at [`HOT_TO_WARM_IDLE_SECS`] (5 min idle).
//! * **Warm → Parked** at [`WARM_TO_PARKED_IDLE_SECS`] (30 min idle).
//! * **Parked → Cold** at [`PARKED_TO_COLD_IDLE_SECS`] (24 h idle).
//!
//! [`next_state_for_idle`] is the single decision function consumed by
//! `crate::cache::registry::ShardRegistry::demote_idle_shards`; it
//! returns `None` when the shard is not yet idle past its current
//! tier's TTL or when the tier is already at the bottom.

use super::shard::ShardState;

/// After this many seconds without a query a `Hot` shard demotes to
/// `Warm`.  Phase 4+ extends "no query" to "no bloom-positive query"
/// so cold drives don't re-warm just because their bloom got faulted
/// in by a wildcard scan.
///
/// Consumed by [`crate::index::IndexManager::demote_idle_shards`]
/// (Phase 3 Commit D) via [`next_state_for_idle`].
pub(crate) const HOT_TO_WARM_IDLE_SECS: u64 = 300;

/// After this many seconds without a query a `Warm` shard demotes to
/// `Parked`, dropping the runtime mmap (the records / names columns
/// are released; bloom + trie persist in Phase 4+).
pub(crate) const WARM_TO_PARKED_IDLE_SECS: u64 = 1800;

/// After this many seconds without a query a `Parked` shard demotes
/// to `Cold`, dropping bloom + trie too.  A `Cold` shard requires a
/// full re-decrypt of the encrypted compact cache to re-promote, so
/// the threshold is generous (24 h) — anything shorter risks
/// thrashing under nightly batch processes that scan archives once
/// per day.
pub(crate) const PARKED_TO_COLD_IDLE_SECS: u64 = 86_400;

/// Decide whether a shard in `state` that has been idle for
/// `idle_secs` seconds should demote to a colder tier.
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
pub(crate) const fn next_state_for_idle(state: ShardState, idle_secs: u64) -> Option<ShardState> {
    match state {
        ShardState::Hot if idle_secs >= HOT_TO_WARM_IDLE_SECS => Some(ShardState::Warm),
        ShardState::Warm if idle_secs >= WARM_TO_PARKED_IDLE_SECS => Some(ShardState::Parked),
        ShardState::Parked if idle_secs >= PARKED_TO_COLD_IDLE_SECS => Some(ShardState::Cold),
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
}
