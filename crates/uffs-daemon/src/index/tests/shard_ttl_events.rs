// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 6 fix (2026-05-07 24-h soak finding) — `shard.ttl` event
//! shape contract tests.
//!
//! Extracted as a sibling module of [`super::idle_demote`] to keep
//! that file under the workspace's 800-LOC file-size policy
//! (`scripts/ci/check_file_size_policy.sh`).  Companion path
//! coverage on the pure-function helpers (`build_thresholds`,
//! `chosen_ttl_for_state`) lives in the `commit_c_tests` module
//! inside `crate::index::transitions`; this module is the contract
//! pin for the live demote controller.
//!
//! The shared `tracing::Subscriber` capture scaffold (`EventLog` /
//! `CapturedEvent`) lives in [`super::tracing_capture`].

#![expect(
    clippy::indexing_slicing,
    clippy::std_instead_of_alloc,
    reason = "test code — assertions index into known-shape vectors, and pull \
              `Arc` from `std` to match the rest of the daemon's test fixtures"
)]

use std::sync::Arc;

use super::tracing_capture::{CapturedEvent, EventLog};
use super::{IndexManager, build_test_drive};

/// Phase 6 24-h soak finding: the `shard.ttl` event used to emit
/// only `chosen_ttl_sec` (the outgoing edge of the drive's *current*
/// tier).  Drives in different tiers therefore reported different
/// fields, so a log-driven peer-vs-target audit could not compare
/// like-with-like — the original `chosen_ttl_sec exceeds peers`
/// assertion in the soak validator was structurally impossible to
/// pass under the default Phase 6 ladder (warm cap < parked base).
///
/// Post-fix: every `shard.ttl` event carries **all four** TTL
/// fields — `chosen_ttl_sec` (preserved for back-compat),
/// `hot_ttl_sec`, `warm_ttl_sec`, `parked_ttl_sec` — so consumers
/// can pick a single edge (`warm_ttl_sec` is the most rate-sensitive)
/// and compare across drives regardless of their current tier.
///
/// This test pins the new shape on the **idle-demote path** —
/// the canonical path the 24-h soak validator scrapes.  Companion
/// path coverage (clamp / below-ttl) lives in the `commit_c_tests`
/// module on the pure-function helpers; this integration test is
/// the contract pin for the live demote controller.
#[tokio::test]
async fn shard_ttl_event_emits_all_three_thresholds() {
    // All `use`s up-front to satisfy clippy's `items_after_statements`
    // pedantic gate.
    use crate::cache::ShardState;
    use crate::cache::policy::{
        HOT_TO_WARM_IDLE_SECS, PARKED_TO_COLD_IDLE_SECS, WARM_TO_PARKED_IDLE_SECS,
    };

    // Same dummy-Dispatch + thread-local-default + interest-rebuild
    // dance as `shard_transition_events_emitted_on_demote_and_promote`
    // — see that test's docstring for the rationale.
    let log = EventLog::default();
    let _interest_rebuild_dummy =
        tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let _guard = tracing::subscriber::set_default(log.clone());
    tracing::callsite::rebuild_interest_cache();

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx, Arc::new(crate::config::Config::default()));
    mgr.add_drive(build_test_drive()).await;

    // Backdate so the next idle-demote tick fires the Warm→Parked
    // path (emits the canonical `idle-demote` shard.ttl event).
    let last_query_ms = 1_000_000_000_u64;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', last_query_ms)
            .await
    );
    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000;
    mgr.demote_idle_shards(now_ms).await;

    // Verify the Warm→Parked transition actually happened.
    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', ShardState::Parked)]);

    // Find the `idle-demote`-reason `shard.ttl` event for drive C.
    let events = log.events();
    let ttl_events: Vec<&CapturedEvent> = events
        .iter()
        .filter(|event| {
            event.target == "shard.ttl"
                && event.field("drive") == Some("C")
                && event.field("reason") == Some("idle-demote")
        })
        .collect();
    assert_eq!(
        ttl_events.len(),
        1,
        "expected exactly one `idle-demote` shard.ttl event for C, got {}: {:#?}",
        ttl_events.len(),
        ttl_events,
    );
    let event = ttl_events[0];

    // Phase 6 fix contract: all four TTL fields present.
    // `chosen_ttl_sec` stays for back-compat, plus the three new
    // fields that make the log self-describing across tier states.
    assert!(
        event.has_field("chosen_ttl_sec"),
        "shard.ttl must keep emitting chosen_ttl_sec for back-compat \
         (existing dashboards / log greps depend on it)",
    );
    assert!(
        event.has_field("hot_ttl_sec"),
        "shard.ttl must emit hot_ttl_sec — Phase 6 audit contract",
    );
    assert!(
        event.has_field("warm_ttl_sec"),
        "shard.ttl must emit warm_ttl_sec — Phase 6 audit contract",
    );
    assert!(
        event.has_field("parked_ttl_sec"),
        "shard.ttl must emit parked_ttl_sec — Phase 6 audit contract",
    );

    // The default ladder + zero rate produces the documented base
    // values (the bonus formula collapses to 0 at rate_qpm = 0).
    // Pin the per-tier values so a future formula tweak that
    // accidentally wires the wrong threshold to the wrong field
    // would fail decisively.
    assert_eq!(
        event.field("hot_ttl_sec"),
        Some(HOT_TO_WARM_IDLE_SECS.to_string().as_str()),
    );
    assert_eq!(
        event.field("warm_ttl_sec"),
        Some(WARM_TO_PARKED_IDLE_SECS.to_string().as_str()),
    );
    assert_eq!(
        event.field("parked_ttl_sec"),
        Some(PARKED_TO_COLD_IDLE_SECS.to_string().as_str()),
    );
    // chosen_ttl_sec on a Warm-tier drive is the warm→parked edge.
    assert_eq!(
        event.field("chosen_ttl_sec"),
        Some(WARM_TO_PARKED_IDLE_SECS.to_string().as_str()),
        "Warm-tier drive's chosen_ttl_sec must be the warm→parked edge",
    );
}
