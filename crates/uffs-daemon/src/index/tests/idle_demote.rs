// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 3 Commit D & E + Commit E tracing-event contract tests.
//!
//! Covers:
//!
//! * `mark_loaded_at` seeding the freshly-mounted shard's idle clock.
//! * `demote_idle_shards` no-op / Warm→Parked / Parked→Cold / batch-multiple /
//!   round-trip query stats.
//! * Plan tasks 3.7 / 3.8 — virtual-time multi-drive demote tests.
//! * Plan task 3.9 — `shard.transition` tracing events with the `letter` /
//!   `from` / `to` / `reason` / `freed_mb` / `restored_mb` field contract; the
//!   `EventLog` / `CapturedEvent` / `FieldCapture` capture scaffold lives at
//!   the bottom of this file.

#![expect(
    clippy::indexing_slicing,
    clippy::min_ident_chars,
    clippy::std_instead_of_alloc,
    reason = "test code — assertions index into known-shape vectors, use short \
              drive-letter idents like `c`/`d`, and pull `Arc` from `std` to \
              match the rest of the daemon's test fixtures"
)]

use std::sync::Arc;

use super::{
    FixedBodyLoader, IndexManager, build_test_drive, build_test_drive_d, build_test_drive_e,
};

// ── Phase 3 Commit D — IndexManager::demote_idle_shards ────────────

/// `add_drive` calls `DriveStats::mark_loaded_at(now_ms)` on the
/// freshly mounted shard so the demote-controller's idle clock
/// starts ticking from load time, not from epoch zero.  Without
/// this seed, every freshly loaded shard would demote on the
/// first idle tick because `last_query_at_ms == 0` would compute
/// `idle_secs ≈ now_ms / 1000` (≈ billions of seconds since
/// 1970-01-01).
#[tokio::test]
async fn mark_loaded_at_seeds_freshly_added_drive() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    // Read the shard's last_query_at_ms via a search-ish path; we
    // only have access from inside the manager, so drive a
    // demote-controller call with `now_ms = load_ts + 0` and assert
    // the shard didn't demote — that proves last_query_at_ms is
    // recent (within `WARM_TO_PARKED_IDLE_SECS` of now).
    //
    // More directly: the timestamp must be non-zero.  We test that
    // by attempting to demote at a now that's *exactly* the load
    // time + 1 ms; an unseeded shard would have `idle_ms = now`
    // (huge) and demote, but a seeded shard sees `idle_ms = 1` ms
    // (basically 0 s) and stays Warm.
    let states_before = mgr.shard_states_for_test().await;
    assert_eq!(states_before, vec![('C', crate::cache::ShardState::Warm)]);

    // Synthetic now_ms a billion ms in the future would catch
    // unseeded `last_query_at_ms == 0`.  But a seeded shard has
    // last_query_at_ms ≈ unix_now_ms() (when add_drive was just
    // called), so calling demote_idle_shards with the same now
    // gives idle_secs ≈ 0.
    let now_ms = crate::cache::unix_now_ms();
    mgr.demote_idle_shards(now_ms).await;

    let states_after = mgr.shard_states_for_test().await;
    assert_eq!(
        states_after, states_before,
        "freshly loaded shard must NOT demote on the first tick — \
         mark_loaded_at must have seeded last_query_at_ms",
    );
}

/// Fast-path contract: `demote_idle_shards` on a registry where
/// every shard has been queried recently must complete with no
/// state mutation and no `index_version` bump.
#[tokio::test]
async fn demote_idle_shards_no_op_when_all_fresh() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    // Pretend every shard was queried at t=10_000_000_000 ms.
    let load_ts = 10_000_000_000_u64;
    assert!(mgr.backdate_last_query_at_ms_for_test('C', load_ts).await);
    assert!(mgr.backdate_last_query_at_ms_for_test('D', load_ts).await);

    // `now_ms` only 1 ms after load → idle_secs = 0 → no demote.
    mgr.demote_idle_shards(load_ts + 1).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Warm)
    ]);
}

/// Warm shard idle past `WARM_TO_PARKED_IDLE_SECS` demotes to
/// Parked on the next `demote_idle_shards` call.
#[tokio::test]
async fn demote_idle_shards_warm_to_parked_at_ttl() {
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    // Backdate C's last_query_at_ms to t=1_000_000_000 ms.
    let last_query_ms = 1_000_000_000_u64;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', last_query_ms)
            .await
    );

    // now_ms = last_query + WARM_TO_PARKED_IDLE_SECS * 1000 (exact
    // boundary; `next_state_for_idle` uses `>=`).
    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000;
    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', crate::cache::ShardState::Parked)]);
}

/// Warm shard idle just below `WARM_TO_PARKED_IDLE_SECS` stays
/// Warm — pin the off-by-one that `>=` vs `>` would expose.
#[tokio::test]
async fn demote_idle_shards_below_ttl_keeps_warm() {
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    let last_query_ms = 1_000_000_000_u64;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', last_query_ms)
            .await
    );

    // 1 ms before the boundary — idle_secs computed by
    // `(now - last) / 1000` is `WARM_TO_PARKED_IDLE_SECS - 1`,
    // strictly below the threshold.
    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000 - 1;
    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', crate::cache::ShardState::Warm)]);
}

/// Parked shard idle past `PARKED_TO_COLD_IDLE_SECS` demotes to
/// Cold on the next `demote_idle_shards` call.
///
/// Pins the multi-step ladder: a single tick can see both
/// `Warm → Parked` and `Parked → Cold` transitions if a Parked
/// shard's `last_query_at_ms` is old enough, but the policy only
/// returns one demote target per call so each tick advances each
/// shard at most one tier.  This test seeds a Parked shard
/// directly to keep the assertion focused.
#[tokio::test]
async fn demote_idle_shards_parked_to_cold_at_ttl() {
    use crate::cache::ShardState;
    use crate::cache::policy::PARKED_TO_COLD_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    // Seed C as Parked via the test escape hatch.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    // Backdate so the Parked shard has been idle past its TTL.
    let last_query_ms = 1_000_000_000_u64;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', last_query_ms)
            .await
    );

    let now_ms = last_query_ms + PARKED_TO_COLD_IDLE_SECS * 1000;
    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', ShardState::Cold)]);
}

/// `demote_idle_shards` batches multiple demotes inside a single
/// write-lock window.  Pin the contract by demoting three shards
/// in one call.
#[tokio::test]
async fn demote_idle_shards_batches_multiple_demotes() {
    use crate::cache::ShardState;
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let last_query_ms = 1_000_000_000_u64;
    for letter in ['C', 'D'] {
        assert!(
            mgr.backdate_last_query_at_ms_for_test(letter, last_query_ms)
                .await
        );
    }

    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000;
    let version_before = mgr
        .index_version
        .load(core::sync::atomic::Ordering::Relaxed);
    mgr.demote_idle_shards(now_ms).await;
    let version_after = mgr
        .index_version
        .load(core::sync::atomic::Ordering::Relaxed);

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![('C', ShardState::Parked), ('D', ShardState::Parked)],
        "all backdated Warm shards must demote in a single batch call"
    );
    assert_eq!(
        version_after - version_before,
        1,
        "batch must bump index_version exactly once for the whole batch, \
         not once per demoted shard"
    );
}

/// End-to-end: demote → promote → demote → promote preserves
/// query stats across every rebuild.  Pins the
/// `Arc<DriveStats>`-sharing contract under repeated transitions.
#[test]
fn demote_then_promote_round_trips_query_stats() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let mut reg = ShardRegistry::new().add(Arc::clone(&body_c));

    // Each transition adds queries to verify the canonical stats
    // Arc is what the new shard's `.stats` points at.
    for round in 0_u64..3_u64 {
        reg.iter()
            .find(|s| s.drive == 'C')
            .unwrap()
            .stats
            .mark_query_at(1_000 + round);
        reg = reg.demote_letter('C', ShardState::Parked).expect("demote");
        reg.iter()
            .find(|s| s.drive == 'C')
            .unwrap()
            .stats
            .mark_query_at(2_000 + round);
        reg = reg
            .promote_letter('C', Arc::clone(&body_c))
            .expect("promote");
    }

    let final_c = reg.iter().find(|s| s.drive == 'C').unwrap();
    // 6 mark_query_at calls total across 3 rounds (3 pre-demote +
    // 3 post-demote-pre-promote).
    assert_eq!(final_c.stats.queries_total(), 6);
    // Last mark_query_at was during round 2 with `now_ms = 2_002`.
    assert_eq!(final_c.stats.last_query_at_ms(), 2_002);
}

// ── Phase 3 Commit E — virtual-time multi-drive demote tests ───────

/// Plan task 3.7 — three drives loaded; only C is queried; advance
/// past `WARM_TO_PARKED_IDLE_SECS` and verify D + E demote to Parked
/// while C stays Warm.
///
/// Models the steady-state pattern of a developer using their
/// project drive (C) actively while archive drives (D, E) sit idle.
/// Pins the per-shard idle-clock contract: each shard's
/// `last_query_at_ms` is independent, so the demote controller
/// only acts on the ones that have actually been idle.
///
/// `now_ms` threading lets the test simulate "31 minutes later"
/// deterministically — no `tokio::time::pause` needed because
/// `demote_idle_shards(now_ms)` reads the timestamp from its
/// argument, not from a clock.
#[tokio::test]
async fn demote_idle_shards_warm_only_for_unqueried_drives() {
    use crate::cache::ShardState;
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    let load_ts = 1_000_000_000_u64;
    // Seed all three to the load timestamp.
    for letter in ['C', 'D', 'E'] {
        assert!(
            mgr.backdate_last_query_at_ms_for_test(letter, load_ts)
                .await
        );
    }

    // C is queried 30 minutes after load (last query at
    // load_ts + 30min).  D and E remain at load_ts.
    let c_last_query_ms = load_ts + 30 * 60 * 1000;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', c_last_query_ms)
            .await
    );

    // now_ms = load_ts + 31 minutes.
    let now_ms = load_ts + 31 * 60 * 1000;

    // Sanity: 31 min ≥ WARM_TO_PARKED_IDLE_SECS for D, E.
    let d_e_idle_secs = (now_ms - load_ts) / 1000;
    assert!(d_e_idle_secs >= WARM_TO_PARKED_IDLE_SECS);
    // Sanity: 1 min < WARM_TO_PARKED_IDLE_SECS for C.
    let c_idle_secs = (now_ms - c_last_query_ms) / 1000;
    assert!(c_idle_secs < WARM_TO_PARKED_IDLE_SECS);

    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![
            ('C', ShardState::Warm),
            ('D', ShardState::Parked),
            ('E', ShardState::Parked),
        ],
        "C must stay Warm (recently queried); D and E must demote to Parked"
    );
}

/// Plan task 3.8 — three Parked drives, advance past
/// `PARKED_TO_COLD_IDLE_SECS`, verify all three demote to Cold.
///
/// Pins the bottom rung of the static-TTL ladder.  The Parked tier
/// is the first that drops bloom + trie (Phase 4+); for Phase 3
/// "Parked" already means "no body", so the only difference is the
/// state label.  Cold means "needs a full re-decrypt to re-promote",
/// captured by the policy via the longer 24 h threshold.
#[tokio::test]
async fn demote_idle_shards_parked_drives_demote_to_cold_past_threshold() {
    use crate::cache::ShardState;
    use crate::cache::policy::PARKED_TO_COLD_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    // Seed every drive's last_query to load_ts and demote each to
    // Parked via the test escape hatch.  Order: backdate first so
    // the demote controller doesn't trip on the seeding tick.
    let load_ts = 1_000_000_000_u64;
    for letter in ['C', 'D', 'E'] {
        assert!(
            mgr.backdate_last_query_at_ms_for_test(letter, load_ts)
                .await
        );
        assert!(mgr.demote_letter_for_test(letter, ShardState::Parked).await);
    }

    let pre_states = mgr.shard_states_for_test().await;
    assert_eq!(pre_states, vec![
        ('C', ShardState::Parked),
        ('D', ShardState::Parked),
        ('E', ShardState::Parked),
    ],);

    // now_ms = load_ts + 25 hours (≥ PARKED_TO_COLD_IDLE_SECS = 24h).
    let now_ms = load_ts + 25 * 60 * 60 * 1000;
    let idle_secs = (now_ms - load_ts) / 1000;
    assert!(idle_secs >= PARKED_TO_COLD_IDLE_SECS);

    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![
            ('C', ShardState::Cold),
            ('D', ShardState::Cold),
            ('E', ShardState::Cold),
        ],
        "all three Parked shards past the cold-tier TTL must demote to Cold"
    );
}

// ── Phase 3 Commit E — tracing-event contract (plan task 3.9) ──────

/// Plan task 3.9 — every demote / promote transition emits exactly
/// one `tracing::event!(target: "shard.transition", ...)` event.
///
/// Pins the operator-facing observability contract: the tracing
/// fields (`letter`, `from`, `to`, `reason`, `freed_mb` /
/// `restored_mb`) are part of the public log surface, so a refactor
/// that silently drops or renames them would break dashboards and
/// alerting.  This test captures every event during a
/// demote-then-promote round-trip and asserts on the field values.
///
/// `tokio::test` defaults to a `current_thread` runtime, so the
/// thread-local `tracing::subscriber::set_default` we install at
/// the top of the test captures every event emitted from inside
/// the test future — including the events from `demote_letter` /
/// `promote_letter` running on the same thread.
#[tokio::test]
async fn shard_transition_events_emitted_on_demote_and_promote() {
    use crate::cache::ShardState;

    let log = EventLog::default();
    // Hold a second registered `Dispatch` for the duration of the test
    // body so `tracing-core` keeps `Dispatchers::has_just_one` at
    // `false`, which forces every callsite-interest rebuild to walk
    // the global `LOCKED_DISPATCHERS` list (containing OUR subscriber)
    // instead of taking the `JustOne` fast path.
    //
    // The `JustOne` fast path (`tracing-core::callsite::Dispatchers::
    // rebuilder` at v0.1.36 line 544–549) calls
    // `dispatcher::get_default(f)`, which is THREAD-LOCAL.  When a
    // sibling test on a different thread fires a `shard.transition`
    // callsite for the first time (registering it against
    // `NoSubscriber` because that thread never called `set_default`),
    // the per-callsite `Interest` cache gets pinned to `never` GLOBALLY,
    // and OUR thread-local subscriber never gets a chance to vote
    // — even if we later call `rebuild_interest_cache`, that rebuild
    // also takes the `JustOne` path and asks the wrong (sibling)
    // thread's default.
    //
    // Holding a dummy `NoSubscriber` Dispatch alive flips
    // `has_just_one` to `false`, so the rebuilder switches to
    // `Rebuilder::Read(LOCKED_DISPATCHERS)` which walks every
    // registered dispatcher (ours + the dummy).  Our subscriber's
    // `register_callsite` is then consulted on every callsite,
    // and the `Interest` cache reflects our `Interest::always`
    // independently of which thread first encountered the callsite.
    let _interest_rebuild_dummy =
        tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let _guard = tracing::subscriber::set_default(log.clone());
    // Force every existing callsite to re-register against the dispatchers
    // we just expanded with the dummy + our subscriber, in case sibling
    // tests already pinned cache entries to `never` before this test ran.
    tracing::callsite::rebuild_interest_cache();

    let (tx, _rx) = crate::events::event_channel();
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader);
    mgr.add_drive(build_test_drive()).await;

    // Demote → expect one demote event.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    // Promote via ensure_warm_for_dispatch → expect one promote event.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    let events = log.events();
    // Filter to the operator-facing observability contract: the
    // INFO-level `shard.transition` events with a tier-transition
    // `reason` (`demote`, `promote`, `usn-refresh`).  Other levels
    // on this target are best-effort observability noise that may
    // legitimately fire on a tier transition without violating the
    // contract — e.g. the Phase 5 `Prefetch::hint failed` warn,
    // which can fire on Linux for synthetic small heap regions
    // even when the promote itself succeeds (and is documented at
    // `crates/uffs-daemon/src/cache/prefetch.rs` to be best-effort
    // with warn-level logging on failure).  Pinning by level +
    // reason makes this test robust to the runtime-topology
    // detail of *which thread* the prefetch hint runs on (the
    // PR-e refactor moved the hint from a `spawn_blocking` closure
    // into the surrounding async task — both paths are observably
    // correct, but only the latter is captured by the
    // `set_default` thread-local subscriber on a `current_thread`
    // runtime).
    let transitions: Vec<&CapturedEvent> = events
        .iter()
        .filter(|event| {
            event.target == "shard.transition"
                && event.level == tracing::Level::INFO
                && matches!(
                    event.field("reason"),
                    Some("demote" | "promote" | "usn-refresh")
                )
        })
        .collect();

    assert_eq!(
        transitions.len(),
        2,
        "expected exactly two INFO `shard.transition` events with reason in \
         {{demote, promote, usn-refresh}} (one demote + one promote), got {}: {:#?}",
        transitions.len(),
        transitions
    );

    // `ShardState`'s `Display` impl emits lowercase variant names
    // (`warm`, `parked`, `cold`, …) — that's the wire contract this
    // test pins.  See `impl fmt::Display for ShardState` in
    // `cache/shard.rs`.
    let demote = transitions[0];
    assert_eq!(demote.level, tracing::Level::INFO);
    assert_eq!(demote.field("reason"), Some("demote"));
    assert_eq!(demote.field("from"), Some("warm"));
    assert_eq!(demote.field("to"), Some("parked"));
    assert_eq!(demote.field("letter"), Some("C"));
    assert!(
        demote.has_field("freed_mb"),
        "demote event must carry freed_mb field for resident-delta accounting"
    );

    let promote = transitions[1];
    assert_eq!(promote.level, tracing::Level::INFO);
    assert_eq!(promote.field("reason"), Some("promote"));
    assert_eq!(promote.field("from"), Some("parked"));
    assert_eq!(promote.field("to"), Some("warm"));
    assert_eq!(promote.field("letter"), Some("C"));
    assert!(
        promote.has_field("restored_mb"),
        "promote event must carry restored_mb field for resident-delta accounting"
    );
}

// ── Tracing-event capture helpers ──────────────────────────────────
//
// Mini scaffold for the Commit E tracing contract test.  Implements
// `tracing_subscriber::Layer` so a registry-based subscriber can
// push every event into a thread-safe `Vec<CapturedEvent>`.  The
// helpers are intentionally minimal — only the fields and methods
// the contract test asserts on are surfaced.

/// One captured tracing event.
#[derive(Debug, Clone)]
struct CapturedEvent {
    target: String,
    level: tracing::Level,
    /// `(field_name, stringified_value)` pairs.
    fields: Vec<(String, String)>,
}

impl CapturedEvent {
    /// String value of `field_name`, or `None` when the field was
    /// not present on this event.  Returns `&str` (not owned) so the
    /// test's `assert_eq!` reads naturally.
    fn field(&self, field_name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == field_name)
            .map(|(_, value)| value.as_str())
    }

    /// `true` iff the event carries a field named `field_name`,
    /// regardless of its value.  Used for fields whose value is
    /// dynamic (e.g. `freed_mb` / `restored_mb`) and the test only
    /// pins the *presence*, not the magnitude.
    fn has_field(&self, field_name: &str) -> bool {
        self.fields.iter().any(|(name, _)| name == field_name)
    }
}

/// Thread-safe in-memory event log.  Cloned into the
/// `tracing_subscriber::Layer` and the test asserts against the
/// shared `Arc<Mutex<...>>`.
#[derive(Default, Clone)]
struct EventLog(Arc<std::sync::Mutex<Vec<CapturedEvent>>>);

impl EventLog {
    fn events(&self) -> Vec<CapturedEvent> {
        self.0.lock().unwrap().clone()
    }
}

/// Implements [`tracing::Subscriber`] *directly* (no
/// `tracing-subscriber::Layer` wrapping) so the parallel-test interaction with
/// `tracing`'s global callsite-interest cache is deterministic:
///
/// * `register_callsite` returns `Interest::always` so the cache pins the
///   callsite as "always interested" once we've registered it.
/// * `enabled` returns `true` for every metadata so no filtering happens below
///   the macro level (the `Interest::always` already implies this).
/// * `max_level_hint` returns `LevelFilter::TRACE` so the static
///   `LevelFilter::current()` consulted at the macro level *before* dispatch
///   can never be lower than `TRACE` while this subscriber is the thread-local
///   default — preventing another subscriber's lower hint from silently
///   dropping `INFO`-level events.
///
/// The previous `tracing_subscriber::Layered<EventLog, Registry>`
/// implementation hit a race in parallel test runs: the inner
/// `Registry::register_callsite` returned `Interest::sometimes()`,
/// the outer `Layer::register_callsite` override didn't propagate
/// (`Layered` AND-combines them as `sometimes`), and sibling tests on
/// other threads racing through `tracing::info!` callsites pinned the
/// global per-callsite cache to `never` before we could rebuild it.
/// The direct `Subscriber` impl plus the dummy second `Dispatch` held
/// in the test body together pin the cache to `always` deterministically.
impl tracing::Subscriber for EventLog {
    fn register_callsite(
        &self,
        _metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }

    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        // Span IDs are not inspected by the test; return a stable
        // non-zero placeholder so `tracing` is happy.
        tracing::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let metadata = event.metadata();
        let mut visitor = FieldCapture::default();
        event.record(&mut visitor);
        self.0.lock().unwrap().push(CapturedEvent {
            target: metadata.target().to_owned(),
            level: *metadata.level(),
            fields: visitor.fields,
        });
    }

    fn enter(&self, _span: &tracing::Id) {}

    fn exit(&self, _span: &tracing::Id) {}
}

/// `tracing::field::Visit` impl that converts every recorded field
/// into a `(name, stringified_value)` pair.
#[derive(Default)]
struct FieldCapture {
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for FieldCapture {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .push((field.name().to_owned(), value.to_owned()));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn core::fmt::Debug) {
        // The `tracing::info!(letter = %x.to_ascii_uppercase(), ...)`
        // form goes through `record_debug` because `%` selects the
        // `Display` adapter and the underlying `Field` is recorded
        // via `Debug`.  We strip the surrounding quotes that
        // `Debug` adds for strings so the test asserts read
        // naturally.
        let raw = format!("{value:?}");
        let stripped = raw
            .strip_prefix('"')
            .and_then(|tail| tail.strip_suffix('"'))
            .map(str::to_owned)
            .unwrap_or(raw);
        self.fields.push((field.name().to_owned(), stripped));
    }
}
