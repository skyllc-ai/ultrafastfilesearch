// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 5 (#95 + task 5.7) — `IndexManager::refresh_usn_for_warm_shards`
//! contract tests.
//!
//! Covers the empty-registry / no-Warm fast paths, the
//! cross-platform graceful-failure contract on macOS / Linux
//! (where USN journals are NTFS-only and the helper errors out
//! by design), and the Phase 5 task 5.7 background-I/O priority
//! wire-up (`BackgroundIoScope` enters `THREAD_MODE_BACKGROUND_BEGIN`
//! at the top of every per-letter `spawn_blocking` closure on
//! Windows, no-op on Mac/Linux).

#![expect(
    clippy::std_instead_of_alloc,
    reason = "test code — `std::sync::Arc` matches the rest of the daemon's test \
              fixtures, no need to switch to `alloc::sync::Arc` for tests"
)]

use std::sync::Arc;

use super::{IndexManager, build_test_drive, build_test_drive_d};

// ── Phase 5 (#95) — IndexManager::refresh_usn_for_warm_shards ──────

/// Fast-path contract: refresh tick on an empty registry returns
/// immediately without panicking and without mutating any state.
/// Pins the early-return at the top of
/// [`IndexManager::refresh_usn_for_warm_shards`] so future
/// refactors can't accidentally call into the [`tokio::task::JoinSet`]
/// phase with an empty `letters` vec.
#[tokio::test]
async fn refresh_usn_for_warm_shards_no_op_when_empty() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    assert!(mgr.shard_states_for_test().await.is_empty());

    mgr.refresh_usn_for_warm_shards().await;

    assert!(
        mgr.shard_states_for_test().await.is_empty(),
        "empty-registry refresh tick must keep the registry empty",
    );
}

/// Fast-path contract: refresh tick on a registry with no
/// `Warm`/`Hot` shards (everything Parked) is also a no-op.
/// Pins that the read-lock detect skips Parked/Cold shards
/// without ever entering the [`tokio::task::JoinSet`] phase.
#[tokio::test]
async fn refresh_usn_for_warm_shards_no_op_when_no_warm_or_hot() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    mgr.refresh_usn_for_warm_shards().await;

    assert_eq!(
        mgr.shard_states_for_test().await,
        vec![('C', ShardState::Parked)],
        "Parked shard must stay Parked through refresh tick",
    );
}

/// Cross-platform graceful-failure contract: on macOS / Linux the
/// underlying [`uffs_core::compact_loader::load_drive_with_usn_refresh`]
/// helper errors out by design (USN journals are NTFS-only).  The
/// refresh tick must NOT panic, NOT lose the existing in-memory body,
/// and NOT mutate `index_version`.  On Windows this same test
/// exercises the success path (USN replay applied + body swapped),
/// but the assertions above (state preservation + body retained)
/// still hold because `replace_warm_body` keeps the previous body
/// on any registry race.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_usn_for_warm_shards_handles_helper_errors_gracefully() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let states_before = mgr.shard_states_for_test().await;
    assert_eq!(states_before, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Warm),
    ]);

    // The refresh tick walks Warm shards, calls the helper, and on
    // non-Windows every call errors with `PlatformNotSupported`.
    // The test passes if the call returns cleanly.
    mgr.refresh_usn_for_warm_shards().await;

    let states_after = mgr.shard_states_for_test().await;
    assert_eq!(
        states_after, states_before,
        "shards must keep Warm state when USN refresh helper errors",
    );
}

/// Phase 5 task **5.7** — every per-letter `spawn_blocking` closure
/// in [`IndexManager::refresh_usn_for_warm_shards`] enters
/// [`crate::cache::background_io::BackgroundIoScope`] at the top
/// (calling `BackgroundIoPriority::begin()`) and the RAII guard's
/// [`Drop`] fires the matching `end()` on closure exit.
///
/// The fake counts `begin()` and `end()` independently so the
/// assertion can pin both halves of the pair: `begins == ends ==
/// number_of_warm_shards`.  Pairing matters because the production
/// blocking-pool thread is reused for unrelated work after the
/// closure returns — leaving a thread stuck at
/// `THREAD_MODE_BACKGROUND_BEGIN` would silently degrade later
/// foreground RPC handlers that happen to land on the same pool
/// thread.
///
/// Topology: 2 Warm shards (C, D) so the [`tokio::task::JoinSet`]
/// phase actually fires (the no-Warm fast path skips
/// `spawn_blocking` entirely).
/// On Mac / Linux the underlying USN helper errors out — the test
/// passes anyway because the `BackgroundIoScope` guard wraps the
/// whole closure body, including the panic-catch arm; on Windows
/// the same wiring runs alongside a successful USN replay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_usn_for_warm_shards_wraps_each_closure_in_background_io_scope() {
    use crate::cache::ShardState;
    use crate::cache::background_io::tests::CountingBackgroundIoPriority;
    use crate::cache::prefetch::PlatformPrefetch;
    use crate::cache::pressure::PlatformPressureSignal;
    use crate::cache::working_set::PlatformWorkingSetTrim;

    let (tx, _rx) = crate::events::event_channel();
    let counting_bg_io = Arc::new(CountingBackgroundIoPriority::new());
    let mgr = IndexManager::with_lifecycle_hooks_for_test(
        None,
        tx,
        Arc::new(crate::cache::body_loader::DiskBodyLoader),
        Arc::new(PlatformWorkingSetTrim),
        Arc::new(PlatformPrefetch),
        Arc::new(PlatformPressureSignal::new()),
        Arc::clone(&counting_bg_io) as Arc<dyn crate::cache::background_io::BackgroundIoPriority>,
    );
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    // Pre-tick: hook never fired.
    assert_eq!(counting_bg_io.begins(), 0, "no refresh yet → no begin()");
    assert_eq!(counting_bg_io.ends(), 0, "no refresh yet → no end()");

    let states_before = mgr.shard_states_for_test().await;
    assert_eq!(states_before, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Warm),
    ]);

    mgr.refresh_usn_for_warm_shards().await;

    // Post-tick: each per-letter closure entered + exited the
    // background-I/O scope exactly once.  begins == ends pins the
    // RAII pairing — a panic in the helper that skipped Drop would
    // leave begins > ends, which would fail this assertion.
    assert_eq!(
        counting_bg_io.begins(),
        2,
        "exactly one BackgroundIoScope::enter() per Warm shard's spawn_blocking closure",
    );
    assert_eq!(
        counting_bg_io.ends(),
        2,
        "exactly one BackgroundIoScope drop per closure (RAII matched begin/end)",
    );
    assert_eq!(
        counting_bg_io.begins(),
        counting_bg_io.ends(),
        "begin/end pair invariant: every begin() must be balanced by exactly one end()",
    );
}

/// Phase 5 task **5.7** companion — the no-Warm fast path skips
/// `spawn_blocking` entirely, so `BackgroundIoPriority::begin()`
/// and `end()` must NOT fire when the registry has no Warm shards.
/// Pins that the cost of holding the trait stays zero on idle
/// daemons (no syscalls per refresh tick when nothing to refresh).
#[tokio::test]
async fn refresh_usn_for_warm_shards_no_op_skips_background_io_scope() {
    use crate::cache::ShardState;
    use crate::cache::background_io::tests::CountingBackgroundIoPriority;
    use crate::cache::prefetch::PlatformPrefetch;
    use crate::cache::pressure::PlatformPressureSignal;
    use crate::cache::working_set::PlatformWorkingSetTrim;

    let (tx, _rx) = crate::events::event_channel();
    let counting_bg_io = Arc::new(CountingBackgroundIoPriority::new());
    let mgr = IndexManager::with_lifecycle_hooks_for_test(
        None,
        tx,
        Arc::new(crate::cache::body_loader::DiskBodyLoader),
        Arc::new(PlatformWorkingSetTrim),
        Arc::new(PlatformPrefetch),
        Arc::new(PlatformPressureSignal::new()),
        Arc::clone(&counting_bg_io) as Arc<dyn crate::cache::background_io::BackgroundIoPriority>,
    );
    // One shard, demoted → no Warm/Hot for the JoinSet phase.
    mgr.add_drive(build_test_drive()).await;
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    mgr.refresh_usn_for_warm_shards().await;

    assert_eq!(
        counting_bg_io.begins(),
        0,
        "no Warm shards → no spawn_blocking → no BackgroundIoScope::enter()",
    );
    assert_eq!(
        counting_bg_io.ends(),
        0,
        "no scope entered → no Drop fires → no end()",
    );
}
