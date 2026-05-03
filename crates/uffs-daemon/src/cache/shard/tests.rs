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
use std::path::PathBuf;

use proptest::prelude::*;
use uffs_core::CaseFold;
use uffs_core::bloom::Bloom;
use uffs_core::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex, IndexSource,
};
use uffs_core::compact_cache::ParkedBody;
use uffs_core::compact_storage::ColumnStorage;
use uffs_core::path_trie::PathTrie;
use uffs_core::trigram::TrigramIndex;
use uffs_mft::usn::FileChange;

use super::{
    DriveStats, DriveStatsSnapshot, IllegalTransition, ShardEntry, ShardState,
    drive_stats_ema_value,
};

/// Build a minimal 2-record `DriveCompactIndex` for shard-body
/// fixture purposes — root directory + one file under it.
///
/// Sufficient to exercise [`ShardEntry::apply_usn_patch_to_body`]'s
/// Arc-clone + Arc-swap contract without dragging in the
/// `index/tests/mod.rs::build_test_drive` fixture (which builds a
/// 7-record drive from a real `MftIndex` and is overkill for the
/// patch-method shape tests below).
fn make_test_body(letter: char) -> DriveCompactIndex {
    // Names blob: letter + "/" + "f.txt".
    let names = vec![letter as u8, b'f', b'.', b't', b'x', b't'];
    let records = vec![
        CompactRecord {
            name_offset: 0,
            flags: 0x10, // directory
            parent_idx: u32::MAX,
            name_len: 1,
            name_first_byte: letter as u8,
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 1,
            parent_idx: 0,
            name_len: 5,
            name_first_byte: b'f',
            ..CompactRecord::default()
        },
    ];
    let fold = CaseFold::default_table();
    let trigram = TrigramIndex::build(&records, &names, fold);
    let children = ChildrenIndex::build(&records);
    let ext_index = ExtensionIndex::build(&records);
    DriveCompactIndex {
        letter,
        records: ColumnStorage::from_vec(records),
        names: ColumnStorage::from_vec(names),
        trigram,
        children,
        ext_index,
        fold,
        ext_names: vec![Box::from("")],
        source: IndexSource::MftFile(PathBuf::from(format!("{letter}:"))),
        source_epoch: 1,
        bloom: None,
        path_trie: None,
    }
}

/// Build a minimal `ParkedBody` for shard-construction tests — a
/// 64-bit bloom + an empty path trie + the default fold table.
/// Sufficient to exercise `ShardEntry::new_parked` and the
/// `parked_body()` accessor without pulling in fixture-builder
/// machinery from `crate::index::tests`.
fn make_test_parked_body(letter: char, source_epoch: u64) -> ParkedBody {
    let bloom = Bloom::with_size_and_k(64, 7);
    let path_trie = PathTrie::build(&[], &[]);
    ParkedBody {
        letter,
        source_epoch,
        bloom,
        path_trie,
        fold: CaseFold::default_table(),
    }
}

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

/// Phase 3 + Phase 4 Commit F: `ShardEntry::new_parked` produces a
/// body-less shard in `ShardState::Parked` that shares the
/// caller-provided `Arc<DriveStats>` and carries the bloom + trie
/// payload in `parked_body`.
#[test]
fn new_parked_has_no_body_and_shares_stats_and_parked_body() {
    let stats = Arc::new(DriveStats::new());
    stats.record_query();
    stats.record_query();

    let parked_body = Arc::new(make_test_parked_body('C', 99));
    let shard = ShardEntry::new_parked('C', Arc::clone(&stats), Arc::clone(&parked_body));
    assert_eq!(shard.drive, 'C');
    assert_eq!(shard.state(), ShardState::Parked);
    assert!(shard.body().is_none());

    // The shard's stats Arc points at the same DriveStats — a
    // mutation via the external Arc shows up in the shard's
    // counter, pinning the "shared not snapshotted" contract.
    assert_eq!(shard.stats.queries_total(), 2);
    stats.record_query();
    assert_eq!(shard.stats.queries_total(), 3);

    // Phase 4 Commit F — `parked_body()` returns a clone of the
    // same Arc, so the bloom / trie / epoch round-trip without copy.
    let from_shard = shard.parked_body().expect("parked shard has body");
    assert!(Arc::ptr_eq(&from_shard, &parked_body));
    assert_eq!(from_shard.letter, 'C');
    assert_eq!(from_shard.source_epoch, 99);
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

// ── Phase 7 task 7.1 — ShardEntry::apply_usn_patch_to_body ─────────────

/// Warm-shard happy path: the method returns
/// `Some((new_arc, stats))` and the new Arc is **not**
/// `Arc::ptr_eq` against the original body Arc — the registry can
/// swap it in atomically without tearing concurrent reads of the
/// previous body.
#[test]
fn apply_usn_patch_to_body_returns_new_arc_on_warm() {
    let body = Arc::new(make_test_body('C'));
    let shard = ShardEntry::new_warm('C', Arc::clone(&body));

    // Empty change batch — the method must still produce a fresh
    // Arc so the caller's swap path is exercised even on no-op ticks.
    let frs_to_compact: Vec<u32> = vec![u32::MAX; 16];
    let result = shard.apply_usn_patch_to_body(&[], &frs_to_compact);

    let (new_body, stats) = result.expect("Warm shard must yield Some");
    assert!(
        !Arc::ptr_eq(&new_body, &body),
        "patched body must be a fresh Arc, not the same allocation"
    );
    assert_eq!(stats.deleted, 0);
    assert_eq!(stats.created, 0);
    assert_eq!(stats.renamed, 0);
    assert_eq!(stats.skipped, 0);
    // Record count preserved on the empty-batch path.
    assert_eq!(new_body.records.len(), body.records.len());
}

/// Parked-shard contract: no in-memory body → `None`, even with a
/// non-empty change batch.  The caller must re-promote first via
/// `ensure_warm_for_dispatch` and let the disk-replay path apply
/// the deltas there.
#[test]
fn apply_usn_patch_to_body_returns_none_on_parked() {
    let stats = Arc::new(DriveStats::new());
    let parked_body = Arc::new(make_test_parked_body('C', 1));
    let shard = ShardEntry::new_parked('C', stats, parked_body);

    let changes = vec![FileChange {
        frs: 10,
        deleted: true,
        ..FileChange::default()
    }];
    let frs_to_compact: Vec<u32> = vec![u32::MAX; 16];

    let result = shard.apply_usn_patch_to_body(&changes, &frs_to_compact);
    assert!(
        result.is_none(),
        "Parked shard has no in-memory body — must return None"
    );
}

/// Cold-shard contract: same as Parked — no body, no patch.
/// Pinned separately so a future tier-state regression that lets
/// Cold shards return an empty body Arc (instead of `None`) is
/// caught immediately.
#[test]
fn apply_usn_patch_to_body_returns_none_on_cold() {
    let stats = Arc::new(DriveStats::new());
    let shard = ShardEntry::new_cold('C', stats);

    let frs_to_compact: Vec<u32> = vec![u32::MAX; 16];
    let result = shard.apply_usn_patch_to_body(&[], &frs_to_compact);
    assert!(
        result.is_none(),
        "Cold shard has no in-memory body — must return None"
    );
}

/// End-to-end smoke against a non-empty change batch: a single
/// delete on a Warm shard's root child lands on the new Arc with
/// `stats.deleted == 1` and the deleted record's `name_len`
/// zeroed.  Pins that the daemon-side wrapper preserves the
/// platform-agnostic patch contract from
/// `uffs_core::compact_loader::apply_usn_patch`.
#[test]
fn apply_usn_patch_to_body_lands_delete_on_new_arc() {
    let body = Arc::new(make_test_body('C'));
    let shard = ShardEntry::new_warm('C', Arc::clone(&body));

    // FRS 10 → compact_idx 1 (the file under the root in the
    // 2-record fixture).  All other FRS map to the sentinel.
    // Iterator-collect form sidesteps `clippy::indexing_slicing`.
    let frs_to_compact: Vec<u32> = (0_usize..16)
        .map(|frs| if frs == 10 { 1 } else { u32::MAX })
        .collect();

    let changes = vec![FileChange {
        frs: 10,
        deleted: true,
        ..FileChange::default()
    }];

    let (new_body, stats) = shard
        .apply_usn_patch_to_body(&changes, &frs_to_compact)
        .expect("Warm shard yields Some");

    assert_eq!(stats.deleted, 1, "exactly one delete should land");
    let deleted_record = new_body
        .records
        .as_slice()
        .get(1)
        .expect("two-record fixture has compact_idx 1");
    assert_eq!(
        deleted_record.name_len, 0,
        "deleted record's name_len must be zeroed"
    );
    assert_eq!(
        deleted_record.parent_idx,
        u32::MAX,
        "deleted record's parent_idx must be u32::MAX"
    );

    // The original body Arc is unchanged — concurrent readers see
    // the previous record_count + 1 child unaffected.
    let original_record = body
        .records
        .as_slice()
        .get(1)
        .expect("original fixture still has compact_idx 1");
    assert_eq!(
        original_record.name_len, 5,
        "original body Arc must be unaffected by patch on the clone"
    );
}
