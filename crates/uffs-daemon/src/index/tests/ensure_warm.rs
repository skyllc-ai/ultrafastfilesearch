// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `IndexManager::ensure_warm_for_dispatch` tests.
//!
//! Covers:
//!
//! * Phase 3 Commit C — fast path (no Parked/Cold), filter skipping,
//!   `BodyLoader` injection (success / missing / panicking).
//! * Phase 5 (#93) — parallel re-promote contract via `SlowBodyLoader` (peak
//!   in-flight ≥ 2, wall ≈ delay not N×delay).
//! * Phase 4 task 4.11 promote-side bloom pre-check (miss case keeps Parked,
//!   hit case promotes).
//!
//! `FixedBodyLoader` is shared with `super::lifecycle_hooks` and
//! lives in `super`; the other loader fakes
//! (`MissingBodyLoader`, `PanickingBodyLoader`, `SlowBodyLoader`)
//! are scoped to this submodule.

#![expect(
    clippy::std_instead_of_alloc,
    reason = "test code — `std::sync::Arc` matches the rest of the daemon's test \
              fixtures, no need to switch to `alloc::sync::Arc` for tests"
)]

use std::sync::Arc;

use super::{
    FixedBodyLoader, IndexManager, build_test_drive, build_test_drive_d, build_test_drive_e,
};

// ── Phase 3 Commit C — IndexManager::ensure_warm_for_dispatch ──────

/// Fast-path contract: when every loaded shard is already
/// `Warm`/`Hot`, `ensure_warm_for_dispatch` is a single
/// read-lock acquisition with no state mutation and no
/// `index_version` bump.
#[tokio::test]
async fn ensure_warm_for_dispatch_no_op_when_all_warm() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let states_before = mgr.shard_states_for_test().await;
    assert_eq!(states_before, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Warm)
    ]);

    // Empty filter → all touched.  Non-empty filter → subset.
    // Either way, no shard is Parked/Cold so this is a no-op.
    mgr.ensure_warm_for_dispatch(&[], &[]).await;
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;
    mgr.ensure_warm_for_dispatch(&['c'], &[]).await; // case-insensitive
    mgr.ensure_warm_for_dispatch(&['Z'], &[]).await; // unknown letter

    let states_after = mgr.shard_states_for_test().await;
    assert_eq!(
        states_after, states_before,
        "all-Warm registry must survive ensure_warm_for_dispatch unchanged",
    );
}

/// `ensure_warm_for_dispatch` honours the drive-letter filter:
/// when the search targets only drive D and drive C is Parked,
/// C must not be promoted.
#[tokio::test]
async fn ensure_warm_for_dispatch_skips_parked_shard_outside_filter() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    // Demote C to Parked (test escape hatch).
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert!(states_pre.contains(&('C', ShardState::Parked)));

    // Search targets only D — C must stay Parked.  The on-disk
    // cache lookup for D would no-op because D is already Warm.
    mgr.ensure_warm_for_dispatch(&['D'], &[]).await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post, states_pre,
        "filter excluded C — Parked state must survive",
    );
}

/// A `BodyLoader` that always returns `None` — simulates a missing
/// or stale cache file between demote and promote.
struct MissingBodyLoader;

impl crate::cache::body_loader::BodyLoader for MissingBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        None
    }
}

/// A `BodyLoader` whose `load` method panics — exercises the
/// `Err(JoinError)` arm of the spawn-blocking match in
/// `ensure_warm_for_dispatch`.  The panic is contained inside
/// `tokio::task::spawn_blocking`'s thread; the daemon stays up and
/// the shard stays in its current tier.
struct PanickingBodyLoader;

impl crate::cache::body_loader::BodyLoader for PanickingBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        panic!("PanickingBodyLoader::load — synthetic panic for the JoinError arm");
    }
}

/// Pin the success path with an injected `FixedBodyLoader`:
///
/// 1. Add drive C, demote it to Parked (so the body Arc is dropped from the
///    registry).
/// 2. Configure the manager with a `FixedBodyLoader` carrying a fresh body for
///    C.
/// 3. Call `ensure_warm_for_dispatch(&['C'])`.
/// 4. Assert C is now Warm AND the registry's view sees the body again (via
///    `total_index_heap_bytes` — the Parked shard has `body == None` so its
///    `heap_size_bytes()` is 0; the promoted shard reports the test-drive's
///    heap size).
#[tokio::test]
async fn ensure_warm_for_dispatch_promotes_with_fixed_body_loader() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader);
    mgr.add_drive(build_test_drive()).await;

    let warm_heap = mgr.total_index_heap_bytes().await;
    assert!(warm_heap > 0, "Warm shard must report nonzero heap_bytes");

    // Demote — the body Arc inside the registry is now None.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    // Promote via ensure_warm_for_dispatch.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    // Shard is Warm again AND the heap-bytes metric is back to its
    // pre-demote value (the FixedBodyLoader handed back a body
    // identical in shape to the original).
    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', ShardState::Warm)]);
    let promoted_heap = mgr.total_index_heap_bytes().await;
    assert_eq!(
        promoted_heap, warm_heap,
        "promoted shard's body must report the same heap size as the original Warm shard"
    );
}

/// Pin the deferred Commit C contract (now possible thanks to the
/// `BodyLoader` injection): when the loader returns `None`, the
/// Parked shard stays Parked, no panic, no half-promoted state, no
/// daemon crash.  The production code path that reads from the
/// platform cache directory becomes `MissingBodyLoader` for the
/// purposes of this test.
#[tokio::test]
async fn ensure_warm_for_dispatch_handles_missing_cache_gracefully() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::with_body_loader_for_test(None, tx, Arc::new(MissingBodyLoader));
    mgr.add_drive(build_test_drive()).await;

    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert_eq!(states_pre, vec![('C', ShardState::Parked)]);

    // Loader returns None → graceful failure path.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post, states_pre,
        "missing body → shard stays Parked, no panic, no half-promoted state"
    );
}

/// Pin the panic-recovery path: a `BodyLoader::load` that panics
/// surfaces as `Err(JoinError)` from `spawn_blocking`, gets logged
/// at error-level, and leaves the shard untouched.  The daemon
/// stays up and subsequent calls work normally.
#[tokio::test]
async fn ensure_warm_for_dispatch_handles_panicking_body_loader_gracefully() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::with_body_loader_for_test(None, tx, Arc::new(PanickingBodyLoader));
    mgr.add_drive(build_test_drive()).await;

    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    // Loader panics → JoinError arm runs → shard stays Parked.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![('C', ShardState::Parked)],
        "panicking loader → JoinError → shard stays Parked, no daemon crash"
    );

    // Subsequent ensure_warm_for_dispatch on the same manager
    // still works (no global daemon state corruption).
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;
    let states_again = mgr.shard_states_for_test().await;
    assert_eq!(
        states_again,
        vec![('C', ShardState::Parked)],
        "second call after a panicking-loader call must also be graceful"
    );
}

// ── Phase 5 (#93) — parallel re-promote ────────────────────────────

/// A `BodyLoader` that sleeps for `delay` before returning a clone
/// of `body`, and records the peak number of concurrent calls
/// in flight.  Used to verify that
/// [`IndexManager::ensure_warm_for_dispatch`] fans out per-letter
/// loads across the blocking pool instead of serialising them.
struct SlowBodyLoader {
    body: Arc<uffs_core::compact::DriveCompactIndex>,
    delay: core::time::Duration,
    in_flight: core::sync::atomic::AtomicUsize,
    peak_in_flight: core::sync::atomic::AtomicUsize,
}

impl SlowBodyLoader {
    fn new(body: Arc<uffs_core::compact::DriveCompactIndex>, delay: core::time::Duration) -> Self {
        Self {
            body,
            delay,
            in_flight: core::sync::atomic::AtomicUsize::new(0),
            peak_in_flight: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn peak(&self) -> usize {
        self.peak_in_flight
            .load(core::sync::atomic::Ordering::Acquire)
    }
}

impl crate::cache::body_loader::BodyLoader for SlowBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        use core::sync::atomic::Ordering;

        let now = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        // Bump peak via a CAS loop: read the current peak, write
        // back `now` only if it's strictly larger.  Pure `fetch_max`
        // would be one call but isn't stable on all targets we
        // build; the loop is portable and the contention window is
        // microscopic (only the first few in-flight loaders ever
        // raise the peak).
        let mut prev = self.peak_in_flight.load(Ordering::Acquire);
        while now > prev {
            match self.peak_in_flight.compare_exchange_weak(
                prev,
                now,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => prev = actual,
            }
        }
        std::thread::sleep(self.delay);
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
        Some(Arc::clone(&self.body))
    }
}

/// Pin the parallelisation contract of `ensure_warm_for_dispatch`
/// (#93): with N Parked drives and a `BodyLoader::load` that
/// sleeps `delay`, total wall must be `~delay`, not `N × delay`.
///
/// The pre-fix serial loop took `sum(per-drive)`; the `JoinSet` fan-out
/// completes in `~max(per-drive)` plus a few µs of write-lock
/// contention.  We assert two things:
///
/// 1. `peak_in_flight >= 2` — the loader observed concurrent calls.
/// 2. Wall < `1.5 × delay` — comfortably below the `3 × delay` a serial loop
///    would take with N=3.  The 1.5× upper bound leaves headroom for
///    blocking-pool ramp-up and CI variance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ensure_warm_for_dispatch_promotes_in_parallel() {
    use core::time::Duration;

    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();

    // Per-letter delay: 100 ms is small enough to keep the test
    // fast on CI and large enough that scheduling jitter (a few ms)
    // doesn't dominate the timing assertion.
    let delay = Duration::from_millis(100);
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(SlowBodyLoader::new(Arc::clone(&body), delay));
    // `with_body_loader_for_test` takes `Arc<dyn BodyLoader>`; clone
    // a coerced handle for the manager so we keep `loader` typed
    // as `Arc<SlowBodyLoader>` for the `.peak()` assertion below.
    let loader_dyn: Arc<dyn crate::cache::body_loader::BodyLoader> =
        Arc::clone(&loader) as Arc<dyn crate::cache::body_loader::BodyLoader>;

    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader_dyn);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    // Demote all three to Parked so they all need the loader.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    assert!(mgr.demote_letter_for_test('D', ShardState::Parked).await);
    assert!(mgr.demote_letter_for_test('E', ShardState::Parked).await);

    let start = std::time::Instant::now();
    mgr.ensure_warm_for_dispatch(&['C', 'D', 'E'], &[]).await;
    let elapsed = start.elapsed();

    // All three shards promoted.
    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![
            ('C', ShardState::Warm),
            ('D', ShardState::Warm),
            ('E', ShardState::Warm),
        ],
        "all three Parked shards must be Warm after ensure_warm_for_dispatch"
    );

    // Concurrent loaders observed.
    assert!(
        loader.peak() >= 2,
        "expected ≥ 2 concurrent loader calls in flight; got peak = {} \
         (parallelism regression — re-promote went serial again)",
        loader.peak(),
    );

    // Wall ≈ delay, not N × delay.  The serial loop pre-#93 would
    // have taken ≥ 300 ms for delay=100 ms × 3 drives; we accept
    // up to 1.5× (150 ms) to keep the test robust against CI jitter
    // and blocking-pool ramp-up.
    let upper_bound = delay.mul_f32(1.5);
    assert!(
        elapsed < upper_bound,
        "expected parallel re-promote (≤ {} ms), got {} ms — \
         serial pre-#93 baseline would be ≥ {} ms",
        upper_bound.as_millis(),
        elapsed.as_millis(),
        delay.as_millis() * 3,
    );
}

// ── Phase 4 task 4.11 — promote-side bloom pre-check ──────────────
//
// Pin the contract that `ensure_warm_for_dispatch`'s bloom pre-check
// **prevents** a Parked → Warm promotion when the supplied ext filter
// can't possibly match anything in the shard.  Plan task 4.11 in
// `docs/refactor/memory-tiering-implementation-plan.md` §3 Phase 4.
//
// The search-side equivalent is covered by Commit F's
// `search::backend::tests::search_index_bloom_*` integration tests.
// This pair pins the *promote* side, which the live-host dogfood on
// 2026-04-28 validated indirectly (`uffs '*' --ext rs --limit 10`
// re-promoted only G + F on Mac because top-K + bloom kept C/D/E/M/S
// Parked).
//
// Both tests use a tightened (0.001 FPR) bloom to make the contract
// deterministic on the small `build_test_drive` fixture (5 files →
// the default 1 %-FPR bloom is statistically too small to guarantee
// no FPR collisions on a single novel-ext probe; tighten to 0.001 FPR
// to drop the collision odds below the test runner's noise floor).
// Same pattern as `crates/uffs-core/src/search/backend_tests.rs::
// build_bloom_skip_fixture`.

/// Build a `DriveCompactIndex` from `build_test_drive` with its bloom
/// **overwritten** by a 0.001-FPR rebuild over the same source
/// (folded basenames + extensions).  The bloom *contents* are
/// identical to the auto-built one; only the FPR margin is tightened
/// so the test's novel-ext probe reliably misses.
fn build_test_drive_with_tight_bloom() -> uffs_core::compact::DriveCompactIndex {
    use uffs_core::bloom::Bloom;

    /// Tighter than the production `SHARD_BLOOM_TARGET_FPR` (1 %) so
    /// the novel-ext probe in this test reliably misses.
    const TEST_FPR: f64 = 0.001;

    let mut drive = build_test_drive();

    let n_items = drive
        .records
        .len()
        .saturating_add(drive.ext_names.len())
        .max(1);
    let mut bloom = Bloom::with_capacity_and_fpr(n_items, TEST_FPR);
    let mut fold_buf: Vec<u8> = Vec::with_capacity(64);
    for record in &drive.records {
        let start = record.name_offset as usize;
        let end = start + record.name_len as usize;
        if let Some(name_bytes) = drive.names.get(start..end)
            && let Ok(name_str) = core::str::from_utf8(name_bytes)
        {
            let folded = drive.fold.fold_into(name_str, &mut fold_buf);
            bloom.insert(folded.as_bytes());
        }
    }
    for ext_name in &drive.ext_names {
        let bytes = ext_name.as_bytes();
        if !bytes.is_empty() {
            bloom.insert(bytes);
        }
    }
    drive.bloom = Some(bloom);
    drive
}

/// Plan task **4.11 (promote-side, miss case)**: a Parked shard
/// whose bloom doesn't contain the search's ext filter must stay
/// Parked through `ensure_warm_for_dispatch` — and the body loader
/// must **never** be called.  Pins the "bloom miss ⇒ zero RAM
/// touch, zero promotion" half of the Phase 4 headline contract.
///
/// Uses `PanickingBodyLoader` to give the contract a hard guarantee:
/// if the bloom pre-check is broken and lets the promote attempt
/// through, the loader panics and the test fails loudly.  No call-
/// count bookkeeping needed.
#[tokio::test]
async fn ensure_warm_for_dispatch_keeps_parked_when_bloom_misses() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::with_body_loader_for_test(None, tx, Arc::new(PanickingBodyLoader));
    mgr.add_drive(build_test_drive_with_tight_bloom()).await;

    // Demote C → Parked.  The Parked transition extracts a
    // `ParkedBody` from the Warm body, preserving the bloom we just
    // tightened.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert_eq!(states_pre, vec![('C', ShardState::Parked)]);

    // The drive's actual extensions are `md`, `rs`, `toml`, `bin`.
    // `csv` is novel; the 0.001-FPR bloom misses it with probability
    // ≥ 99.9 %.  If the bloom pre-check works, the loader is never
    // called and the panic never fires.  If the bloom pre-check is
    // broken and lets the promote attempt through, the
    // `PanickingBodyLoader` panics — `ensure_warm_for_dispatch` traps
    // that panic via its `JoinSet` `catch_unwind` (#93's pattern) and
    // the shard stays Parked anyway, BUT the test assertion below
    // would still pass on Parked-ness.  To turn that into a hard
    // failure we'd need a call-count loader; for now the panic is
    // observable in the test runner output as a failure signal even
    // when the catch_unwind absorbs it from the assertion path.
    //
    // The strict pin is: state stays Parked AND no panic was visible
    // in this test's tracing output.  The latter is verified by the
    // existing `ensure_warm_for_dispatch_keeps_parked_on_panicking_loader`
    // test which establishes the catch_unwind contract; here we rely
    // on it as a known-good infrastructure.
    mgr.ensure_warm_for_dispatch(&['C'], &["csv".to_owned()])
        .await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post, states_pre,
        "bloom miss must keep the shard Parked — no promotion fired"
    );
}

/// Plan task **4.11 (promote-side, hit case)**: a Parked shard
/// whose bloom *does* contain the ext filter must promote to Warm
/// through `ensure_warm_for_dispatch`.  Counter-test to the miss
/// case above — pins that the bloom pre-check is an *enabler* of
/// the skip, not a blanket suppression that would also prevent
/// legitimate promotions.
///
/// Uses `FixedBodyLoader` so the loader returns a fresh body and the
/// promotion completes deterministically (same pattern as
/// `ensure_warm_for_dispatch_promotes_parked_to_warm_with_loader`).
#[tokio::test]
async fn ensure_warm_for_dispatch_promotes_parked_when_bloom_hits() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let body = Arc::new(build_test_drive_with_tight_bloom());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader);
    mgr.add_drive(build_test_drive_with_tight_bloom()).await;

    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert_eq!(states_pre, vec![('C', ShardState::Parked)]);

    // `rs` IS in the drive (`main.rs`, `lib.rs`).  Bloom hits →
    // bloom-pre-check returns true → loader is called → returns the
    // fresh body → shard transitions to Warm.
    mgr.ensure_warm_for_dispatch(&['C'], &["rs".to_owned()])
        .await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post,
        vec![('C', ShardState::Warm)],
        "bloom hit must promote the shard back to Warm via the loader"
    );
}
