// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Test-only escape hatches into [`IndexManager`] internals.
//!
//! Each helper is gated behind `#[cfg(test)]` so it never
//! appears in production binaries.  They expose narrowly
//! scoped slices of registry state (`queries_total` per
//! shard, full `ShardState` distribution, in-flight slot
//! count) and one mutator (`demote_letter_for_test`,
//! `backdate_last_query_at_ms_for_test`) that the integration
//! tests in `crate::index::tests` need to seed deterministic
//! preconditions for the Phase 3+ tier-machinery tests.
//!
//! The production code path observes the same fields through
//! the `(snapshot, snapshot_active_letters)` pair on
//! [`IndexManager`] and the public `demote_idle_shards` tick
//! — which would force tests through real-time `tokio::time`
//! manipulation and 5-minute soak windows to exercise tier
//! transitions.  These helpers let the same tests run in
//! sub-millisecond virtual time by writing the underlying
//! state directly.
//!
//! Keeping them in their own module makes the
//! production-vs-test boundary explicit: any reviewer can
//! search for `_for_test` to find every test-only mutation
//! surface, and the file-size policy never sees them mingled
//! with the production methods they exercise.

#![cfg(test)]

use alloc::sync::Arc;

use super::IndexManager;

impl IndexManager {
    /// Per-shard `(drive_letter, queries_total)` snapshot for tests.
    ///
    /// Test-only escape hatch so integration tests in `crate::index::tests`
    /// can verify the [`Self::record_search_dispatch`] wiring without
    /// exposing the registry to production callers.
    pub(crate) async fn shard_query_totals_for_test(&self) -> Vec<(char, u64)> {
        let guard = self.index.read().await;
        guard
            .iter()
            .map(|shard| (shard.drive, shard.stats.queries_total()))
            .collect()
    }

    /// Demote a single shard to `target` for tests.
    ///
    /// Test-only escape hatch used by Commit C tests to seed a
    /// `Parked`/`Cold` shard so [`Self::ensure_warm_for_dispatch`]
    /// has something to promote.  Production code never calls this
    /// directly — the demote-on-idle controller in Commit D uses
    /// `ShardRegistry::demote_letter` from a `tokio::task` instead.
    ///
    /// Returns `true` if the registry was rebuilt (demote was
    /// legal), `false` otherwise (unknown letter or illegal target).
    pub(crate) async fn demote_letter_for_test(
        &self,
        letter: char,
        target: crate::cache::ShardState,
    ) -> bool {
        let mut guard = self.index.write().await;
        guard
            .demote_letter(letter, target)
            .is_some_and(|new_registry| {
                *guard = Arc::new(new_registry);
                drop(guard);
                self.bump_index_version();
                true
            })
    }

    /// Backdate a shard's `last_query_at_ms` for tests.
    ///
    /// Sets the timestamp via [`crate::cache::DriveStats::mark_loaded_at`]
    /// (no `queries_total` bump), so the per-shard query counter
    /// stays a clean count of actual searches dispatched in tests
    /// where that matters.  Returns `true` when the letter was
    /// found, `false` otherwise.
    ///
    /// Used by the Commit-D virtual-time tests to simulate "shard
    /// has been idle for N seconds" by writing a known-old
    /// timestamp directly, then calling
    /// [`Self::demote_idle_shards`] with `now_ms = old_ts + ttl +
    /// epsilon` and asserting the demote happened.
    pub(crate) async fn backdate_last_query_at_ms_for_test(
        &self,
        letter: char,
        ts_ms: u64,
    ) -> bool {
        let guard = self.index.read().await;
        guard
            .iter()
            .find(|shard| shard.drive.eq_ignore_ascii_case(&letter))
            .is_some_and(|shard| {
                shard.stats.mark_loaded_at(ts_ms);
                true
            })
    }

    /// Per-shard `(drive_letter, ShardState)` snapshot for tests.
    ///
    /// Test-only — the production code path observes shard state
    /// only through [`Self::snapshot`] (which filters Warm/Hot).
    /// Commit C tests need to assert the *full* tier distribution
    /// (Parked/Cold) before and after `ensure_warm_for_dispatch`.
    pub(crate) async fn shard_states_for_test(&self) -> Vec<(char, crate::cache::ShardState)> {
        let guard = self.index.read().await;
        guard
            .iter()
            .map(|shard| (shard.drive, shard.state()))
            .collect()
    }

    /// Number of in-flight promote slots currently held in the
    /// dedup map.  Used by the PR-e
    /// `slot_clears_after_completion` test to wait deterministically
    /// for the cleanup task spawned in
    /// [`Self::load_or_join_in_flight`] without relying on real-time
    /// sleeps — see the test for the polling helper.
    pub(crate) fn in_flight_promotes_len_for_test(&self) -> usize {
        self.in_flight_promotes
            .lock()
            .expect("in_flight_promotes lock poisoned — programmer bug")
            .len()
    }
}
