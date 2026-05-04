// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Operator-driven `forget` (Phase 8-D) and `status_drives`
//! (Phase 8-E) tests for [`super::IndexManager`].
//!
//! `forget` tests pin the eviction guard (busy-without-force vs.
//! force-auto-hibernate), the registry-eviction step, the
//! cache-cleaner side effect (verified via the
//! [`crate::cache::cache_cleaner::CountingCacheCleaner`] fake), and
//! the per-drive classification (`forgotten` vs. `already_absent`).
//!
//! `status_drives` tests pin the per-drive row builder: tier
//! mapping, resident-bytes calculation across the
//! Hot/Warm/Parked/Cold ladder, pin-expiry surfacing, and the
//! deterministic ascending sort.
//!
//! Every test uses [`super::IndexManager::with_lifecycle_hooks_for_test`]
//! to inject a [`CountingCacheCleaner`] (and a [`FixedBodyLoader`]
//! for the preload-then-forget sequences) so the host's real cache
//! directory is **never** touched — a "forget drive C" call against
//! the platform paths would be catastrophic in CI.

#![expect(
    clippy::std_instead_of_alloc,
    reason = "test fixtures — `std::sync::Arc` matches the rest of the daemon's \
              test fixtures, no need to switch to `alloc::sync::Arc` for tests"
)]

use std::sync::Arc;

use super::{FixedBodyLoader, build_test_drive, build_test_drive_d, build_test_drive_e};
use crate::cache::ShardState;
use crate::cache::cache_cleaner::CountingCacheCleaner;
use crate::index::IndexManager;
use crate::index::constructors::LifecycleHooks;
use crate::index::forget_drive::ForgetOutcomeOrBusy;

/// Build an `IndexManager` with the supplied `cache_cleaner` and
/// (optional) `body_loader` injected, threading
/// [`LifecycleHooks::production`] for every other hook.
///
/// Centralised so the per-test boilerplate stays focused on the
/// behaviour under test rather than the hook bundle assembly.
fn make_manager(
    cleaner: Arc<CountingCacheCleaner>,
    body_loader: Option<Arc<dyn crate::cache::body_loader::BodyLoader>>,
) -> IndexManager {
    let (tx, _rx) = crate::events::event_channel();
    let mut hooks = LifecycleHooks::production();
    hooks.cache_cleaner = cleaner as Arc<dyn crate::cache::cache_cleaner::CacheCleaner>;
    if let Some(loader) = body_loader {
        hooks.body_loader = loader;
    }
    IndexManager::with_lifecycle_hooks_for_test(
        None,
        tx,
        hooks,
        Arc::new(crate::config::Config::default()),
    )
}

// ── 8-D forget ──────────────────────────────────────────────────────

/// Phase 8-D — forgetting a `Cold` drive evicts it from the registry,
/// invokes the cache cleaner, and reports `forgotten` with the freed
/// byte count.
#[tokio::test]
async fn forget_cold_drive_evicts_and_unlinks() {
    let cleaner = Arc::new(CountingCacheCleaner::new(1_024));
    let mgr = make_manager(Arc::clone(&cleaner), None);
    mgr.add_drive(build_test_drive()).await;
    assert!(mgr.demote_letter_for_test('C', ShardState::Cold).await);

    let outcome = mgr.forget_drives(&['C'], false).await;

    let ForgetOutcomeOrBusy::Ok(out) = outcome else {
        panic!("expected Ok outcome on Cold-drive forget; got {outcome:?}");
    };
    assert_eq!(out.forgotten, vec!['C']);
    assert!(out.already_absent.is_empty());
    assert_eq!(out.freed_bytes, 1_024);
    assert!(out.errors.is_empty());

    assert_eq!(
        cleaner.calls(),
        vec!['C'],
        "cache cleaner must have been invoked exactly once for C"
    );
    let states = mgr.shard_states_for_test().await;
    assert!(
        states.is_empty(),
        "registry must be empty after forget; got {states:?}"
    );
}

/// Phase 8-D — forgetting a non-`Cold` drive without `force = true`
/// is a top-level refusal that lists the busy drive's tier and
/// leaves the registry untouched.
#[tokio::test]
async fn forget_warm_without_force_refuses_with_busy_listing() {
    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let mgr = make_manager(Arc::clone(&cleaner), None);
    mgr.add_drive(build_test_drive()).await; // C lands in Warm.

    let outcome = mgr.forget_drives(&['C'], false).await;

    let ForgetOutcomeOrBusy::Busy(busy) = outcome else {
        panic!("expected Busy refusal for Warm drive without force; got {outcome:?}");
    };
    assert_eq!(busy, vec![('C', ShardState::Warm)]);
    assert!(
        cleaner.calls().is_empty(),
        "cache cleaner must NOT have been invoked on the refused path"
    );
    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![('C', ShardState::Warm)],
        "registry must be untouched on refusal"
    );
}

/// Phase 8-D — `force = true` auto-hibernates non-`Cold` drives
/// before evicting + cleaning, and the cache cleaner is invoked
/// exactly once per requested drive.
#[tokio::test]
async fn forget_warm_with_force_auto_hibernates_then_evicts() {
    let cleaner = Arc::new(CountingCacheCleaner::new(2_048));
    let mgr = make_manager(Arc::clone(&cleaner), None);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let outcome = mgr.forget_drives(&['C', 'D'], true).await;

    let ForgetOutcomeOrBusy::Ok(out) = outcome else {
        panic!("expected Ok outcome with force; got {outcome:?}");
    };
    assert_eq!(out.forgotten, vec!['C', 'D']);
    assert_eq!(out.freed_bytes, 4_096, "2 drives × 2_048 bytes each");
    assert!(out.already_absent.is_empty());
    assert!(out.errors.is_empty());

    assert_eq!(
        cleaner.calls(),
        vec!['C', 'D'],
        "cache cleaner invoked once per drive in input order"
    );
    let states = mgr.shard_states_for_test().await;
    assert!(states.is_empty(), "registry must be empty");
}

/// Phase 8-D — forgetting an unknown drive is idempotent: the
/// cache cleaner runs (so any stale on-disk file from a previous
/// daemon instance gets cleaned up), and the drive lands in
/// `already_absent` when the cleaner reports zero freed bytes.
#[tokio::test]
async fn forget_unknown_drive_is_idempotent_already_absent() {
    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let mgr = make_manager(Arc::clone(&cleaner), None);
    mgr.add_drive(build_test_drive()).await; // C only.

    let outcome = mgr.forget_drives(&['Z'], false).await;

    let ForgetOutcomeOrBusy::Ok(out) = outcome else {
        panic!("expected Ok for unknown drive; got {outcome:?}");
    };
    assert!(out.forgotten.is_empty());
    assert_eq!(out.already_absent, vec!['Z']);
    assert_eq!(out.freed_bytes, 0);
    assert!(out.errors.is_empty());

    assert_eq!(
        cleaner.calls(),
        vec!['Z'],
        "cleaner still runs for unknown drives so stale on-disk files are purged"
    );
    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![('C', ShardState::Warm)],
        "C must remain Warm — the unknown-drive forget call must not touch other drives"
    );
}

/// Phase 8-D — forgetting a `Hot`-pinned drive with `force = true`
/// clears the pin (via the registry rebuild) and proceeds to
/// evict + clean.  The pin is implicitly cleared because
/// `demote_letter_with_reason` rebuilds the shard with a fresh
/// `ShardEntry` whose `pin_until_ms` starts at `0`.
#[tokio::test]
async fn forget_pinned_hot_drive_with_force_clears_pin() {
    use crate::index::tiering_ops::PreloadOutcome;

    let cleaner = Arc::new(CountingCacheCleaner::new(512));
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = make_manager(
        Arc::clone(&cleaner),
        Some(loader as Arc<dyn crate::cache::body_loader::BodyLoader>),
    );
    mgr.add_drive(build_test_drive()).await;
    assert!(mgr.demote_letter_for_test('C', ShardState::Cold).await);

    // Preload C → Hot + pin.
    let preload = mgr.preload_drive('C', 30).await;
    assert!(matches!(preload, PreloadOutcome::Promoted { .. }));

    // Pre-condition assertion: C is Hot and pinned.
    let pre_states = mgr.shard_states_for_test().await;
    assert_eq!(pre_states, vec![('C', ShardState::Hot)]);

    // Force-forget must succeed despite the pin.
    let outcome = mgr.forget_drives(&['C'], true).await;
    let ForgetOutcomeOrBusy::Ok(out) = outcome else {
        panic!("expected Ok with force; got {outcome:?}");
    };
    assert_eq!(out.forgotten, vec!['C']);
    assert_eq!(out.freed_bytes, 512);

    let post_states = mgr.shard_states_for_test().await;
    assert!(post_states.is_empty());
}

/// Phase 8-D — multi-drive forget where one drive is busy and
/// `force` is `false` refuses the entire request (all-or-nothing).
/// The non-busy drives stay loaded; the cleaner is never invoked.
#[tokio::test]
async fn forget_mixed_request_refuses_when_any_drive_busy_without_force() {
    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let mgr = make_manager(Arc::clone(&cleaner), None);
    mgr.add_drive(build_test_drive()).await; // C: Warm
    mgr.add_drive(build_test_drive_d()).await; // D: Warm
    assert!(mgr.demote_letter_for_test('D', ShardState::Cold).await);
    // Now: C is Warm, D is Cold.

    let outcome = mgr.forget_drives(&['C', 'D'], false).await;

    let ForgetOutcomeOrBusy::Busy(busy) = outcome else {
        panic!("expected all-or-nothing Busy refusal; got {outcome:?}");
    };
    assert_eq!(
        busy,
        vec![('C', ShardState::Warm)],
        "only C should be reported as busy; D is Cold and would be safe"
    );
    assert!(
        cleaner.calls().is_empty(),
        "cleaner must NOT have run on the refused path"
    );

    // Both shards must still be present.
    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Cold)
    ]);
}

// ── 8-E status_drives ───────────────────────────────────────────────

/// Phase 8-E — empty registry produces an empty `drives` vector.
/// Mirrors the "no drives loaded" CLI hint.
#[tokio::test]
async fn status_drives_empty_registry_returns_empty_drives() {
    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let mgr = make_manager(cleaner, None);

    let response = mgr.status_drives().await;

    assert!(response.drives.is_empty());
}

/// Phase 8-E — a single Warm shard surfaces `tier = "warm"`,
/// `resident_bytes > 0` (the body's heap footprint), and zero
/// values for never-queried + never-pinned counters.
#[tokio::test]
async fn status_drives_single_warm_shard_full_snapshot() {
    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let mgr = make_manager(cleaner, None);
    mgr.add_drive(build_test_drive()).await;

    let response = mgr.status_drives().await;

    let [row] = response.drives.as_slice() else {
        panic!(
            "expected exactly 1 drive in status_drives response; got {}",
            response.drives.len()
        );
    };
    assert_eq!(row.letter, 'C');
    assert_eq!(row.tier, "warm");
    assert!(
        row.resident_bytes > 0,
        "Warm shard must report nonzero resident_bytes (body heap footprint); got {}",
        row.resident_bytes
    );
    assert_eq!(row.pin_until_unix_ms, 0, "no preload ⇒ no pin");
    assert_eq!(row.promotions_total, 0, "Phase 9 placeholder, always 0");
    assert!(
        row.last_query_at_ms > 0,
        "add_drive seeds last_query_at_ms via mark_loaded_at; got {}",
        row.last_query_at_ms
    );
}

/// Phase 8-E — mixed-tier registry produces one row per shard, with
/// per-tier `resident_bytes` reflecting the source: Warm reads
/// `body.heap_size_bytes().total`, Cold reads `0`.
#[tokio::test]
async fn status_drives_mixed_tier_distribution_one_row_per_shard() {
    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let mgr = make_manager(cleaner, None);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    // C stays Warm; D demotes to Parked; E demotes to Cold.
    assert!(mgr.demote_letter_for_test('D', ShardState::Parked).await);
    assert!(mgr.demote_letter_for_test('E', ShardState::Cold).await);

    let response = mgr.status_drives().await;

    let [c_row, d_row, e_row] = response.drives.as_slice() else {
        panic!(
            "expected exactly 3 drives in status_drives response; got {}",
            response.drives.len()
        );
    };

    // Sorted ascending by letter.
    assert_eq!(c_row.letter, 'C');
    assert_eq!(c_row.tier, "warm");
    assert!(
        c_row.resident_bytes > 0,
        "Warm shard reports nonzero resident_bytes (body heap)"
    );

    assert_eq!(d_row.letter, 'D');
    assert_eq!(d_row.tier, "parked");
    // Parked shards report parked_body.size_bytes() (bloom + trie).
    // The tiny synthetic test fixture builds a non-empty trie + bloom,
    // so resident_bytes should be > 0.
    assert!(
        d_row.resident_bytes > 0,
        "Parked shard reports nonzero resident_bytes (bloom + trie); got {}",
        d_row.resident_bytes
    );

    assert_eq!(e_row.letter, 'E');
    assert_eq!(e_row.tier, "cold");
    assert_eq!(
        e_row.resident_bytes, 0,
        "Cold shard reports zero resident_bytes (encrypted cache only on disk)"
    );
}

/// Phase 8-E — preloaded (Hot + pinned) drive surfaces both
/// `tier = "hot"` and `pin_until_unix_ms > 0`, so the CLI's pin
/// column has a non-empty value to render.
#[tokio::test]
async fn status_drives_preloaded_hot_drive_surfaces_pin_expiry() {
    use crate::index::tiering_ops::PreloadOutcome;

    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = make_manager(
        cleaner,
        Some(loader as Arc<dyn crate::cache::body_loader::BodyLoader>),
    );
    mgr.add_drive(build_test_drive()).await;
    assert!(mgr.demote_letter_for_test('C', ShardState::Cold).await);

    let preload = mgr.preload_drive('C', 30).await;
    assert!(matches!(preload, PreloadOutcome::Promoted { .. }));

    let response = mgr.status_drives().await;

    let [row] = response.drives.as_slice() else {
        panic!(
            "expected exactly 1 drive in status_drives response; got {}",
            response.drives.len()
        );
    };
    assert_eq!(row.tier, "hot");
    assert!(
        row.pin_until_unix_ms > 0,
        "preloaded shard must surface pin_until_unix_ms > 0; got {}",
        row.pin_until_unix_ms
    );
}

/// Phase 8-E — output is sorted by drive letter (ASCII ascending),
/// even when shards were loaded in a different order.  Stable order
/// across re-runs is part of the operator-facing contract.
#[tokio::test]
async fn status_drives_sorts_rows_by_letter_ascending() {
    let cleaner = Arc::new(CountingCacheCleaner::new(0));
    let mgr = make_manager(cleaner, None);

    // Load in a deliberately scrambled order.
    mgr.add_drive(build_test_drive_e()).await; // 'E' first
    mgr.add_drive(build_test_drive()).await; // 'C' second
    mgr.add_drive(build_test_drive_d()).await; // 'D' third

    let response = mgr.status_drives().await;

    let letters: Vec<char> = response.drives.iter().map(|drive| drive.letter).collect();
    assert_eq!(
        letters,
        vec!['C', 'D', 'E'],
        "rows must be sorted by drive letter ascending regardless of load order"
    );
}
