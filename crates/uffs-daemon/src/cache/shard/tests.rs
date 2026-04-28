// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Test suite for [`super`] (`crate::cache::shard`).
//!
//! Hosted as a sibling submodule under `cache/shard/` so the
//! production `cache/shard.rs` stays under the workspace 800-LOC
//! file-size policy after Phase 3 Commit A added the
//! `last_query_at_ms` + `mark_query_at` + `new_parked` / `new_cold`
//! coverage.  Module path `crate::cache::shard::tests` is unchanged
//! from the previous inline `#[cfg(test)] mod tests { ... }` block.

#![expect(
    clippy::min_ident_chars,
    clippy::default_numeric_fallback,
    clippy::doc_markdown,
    reason = "test code — short loop counters and doc references like \
              `serde_json` are clearer without the pedantic ceremony."
)]

use alloc::sync::Arc;
use core::sync::atomic::Ordering;

use proptest::prelude::*;

use super::{
    DriveStats, DriveStatsSnapshot, IllegalTransition, ShardEntry, ShardState,
    drive_stats_ema_value,
};

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
/// and through the `From` conversions, including the Phase-3
/// `last_query_at_ms` field.
#[test]
fn drivestats_snapshot_round_trips() {
    let stats = DriveStats::new();
    for _ in 0..7 {
        stats.record_query();
    }
    stats.rate_ema_micro_per_s.store(123_456, Ordering::Relaxed);
    stats.last_decay_ms.store(987_654_321, Ordering::Relaxed);
    stats.last_query_at_ms.store(555_555_555, Ordering::Relaxed);

    let snap = DriveStatsSnapshot::from(&stats);
    let json = serde_json::to_string(&snap).expect("serialize");
    let restored: DriveStatsSnapshot = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(snap, restored);
    assert_eq!(restored.queries_total, 7);
    assert_eq!(restored.rate_ema_micro_per_s, 123_456);
    assert_eq!(restored.last_decay_ms, 987_654_321);
    assert_eq!(restored.last_query_at_ms, 555_555_555);

    let stats2 = DriveStats::from(restored);
    assert_eq!(stats2.queries_total(), 7);
    assert_eq!(stats2.last_query_at_ms(), 555_555_555);
}

/// Phase 3: `mark_query_at` increments `queries_total` and stores
/// `now_ms` in `last_query_at_ms` in a single hot-path call.
#[test]
fn mark_query_at_updates_last_query_at_ms() {
    let stats = DriveStats::new();
    assert_eq!(stats.queries_total(), 0);
    assert_eq!(stats.last_query_at_ms(), 0);

    stats.mark_query_at(1_700_000_000_000);
    assert_eq!(stats.queries_total(), 1);
    assert_eq!(stats.last_query_at_ms(), 1_700_000_000_000);

    stats.mark_query_at(1_700_000_000_500);
    assert_eq!(stats.queries_total(), 2);
    assert_eq!(stats.last_query_at_ms(), 1_700_000_000_500);
}

/// Phase 3: `mark_query_at` overwrites with whatever timestamp it
/// receives — a non-monotonic clock (NTP rewind) doesn't trip an
/// assertion, the demote controller just sees a small idle_secs
/// or temporarily-negative-clamped-to-zero on the next tick.
#[test]
fn mark_query_at_overwrites_with_later_timestamp() {
    let stats = DriveStats::new();
    stats.mark_query_at(2_000);
    assert_eq!(stats.last_query_at_ms(), 2_000);
    // Earlier timestamp wins (caller's clock decision).
    stats.mark_query_at(1_000);
    assert_eq!(stats.last_query_at_ms(), 1_000);
    assert_eq!(stats.queries_total(), 2);
}

/// Phase 3: a freshly-constructed `DriveStats` reports
/// `last_query_at_ms() == 0` so the demote controller can use
/// that as the "never queried" sentinel.
#[test]
fn last_query_at_ms_zero_after_construction() {
    let stats = DriveStats::new();
    assert_eq!(stats.last_query_at_ms(), 0);
    // record_query (the legacy entry point that doesn't take a
    // timestamp) must NOT touch last_query_at_ms; otherwise a
    // legacy caller would synthesise a fake "queried at epoch"
    // marker and the demote controller would compute a 50+ year
    // idle window.
    stats.record_query();
    assert_eq!(stats.last_query_at_ms(), 0);
    assert_eq!(stats.queries_total(), 1);
}

/// Phase 3: legacy `DriveStatsSnapshot` JSON without
/// `last_query_at_ms` (e.g. v0.5.78 persisted state) deserialises
/// with `last_query_at_ms == 0` instead of rejecting the input.
/// Pins the `#[serde(default)]` fallback so the on-disk schema
/// stays forward-compatible without a migration.
#[test]
fn drivestats_snapshot_legacy_json_back_compat() {
    let legacy = r#"{"queries_total":42,"rate_ema_micro_per_s":12345,"last_decay_ms":67890}"#;
    let snap: DriveStatsSnapshot = serde_json::from_str(legacy).expect("legacy parses");
    assert_eq!(snap.queries_total, 42);
    assert_eq!(snap.rate_ema_micro_per_s, 12345);
    assert_eq!(snap.last_decay_ms, 67890);
    assert_eq!(
        snap.last_query_at_ms, 0,
        "legacy omitted field defaults to 0"
    );
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

/// Phase 3: `ShardEntry::new_parked` produces a body-less shard
/// in `ShardState::Parked` that shares the caller-provided
/// `Arc<DriveStats>`.
#[test]
fn new_parked_has_no_body_and_shares_stats() {
    let stats = Arc::new(DriveStats::new());
    stats.record_query();
    stats.record_query();

    let shard = ShardEntry::new_parked('C', Arc::clone(&stats));
    assert_eq!(shard.drive, 'C');
    assert_eq!(shard.state(), ShardState::Parked);
    assert!(shard.body().is_none());

    // The shard's stats Arc points at the same DriveStats — a
    // mutation via the external Arc shows up in the shard's
    // counter, pinning the "shared not snapshotted" contract.
    assert_eq!(shard.stats.queries_total(), 2);
    stats.record_query();
    assert_eq!(shard.stats.queries_total(), 3);
}

/// Phase 3: `ShardEntry::new_cold` produces the same body-less
/// shape but in `ShardState::Cold`.
#[test]
fn new_cold_has_no_body_and_shares_stats() {
    let stats = Arc::new(DriveStats::new());
    let shard = ShardEntry::new_cold('D', Arc::clone(&stats));
    assert_eq!(shard.drive, 'D');
    assert_eq!(shard.state(), ShardState::Cold);
    assert!(shard.body().is_none());

    // Same Arc semantics as new_parked.
    stats.mark_query_at(1_700_000_000_000);
    assert_eq!(shard.stats.queries_total(), 1);
    assert_eq!(shard.stats.last_query_at_ms(), 1_700_000_000_000);
}
