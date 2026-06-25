// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Apply-cadence tests — the search-freshness path that decouples the
//! in-memory body patch from the rare compact-cache disk save.
//!
//! Before this split, a newly created / renamed / deleted file stayed
//! invisible to search until a [`super::super::SaveTrigger`] threshold
//! fired (50k events / 5 min): the live USN path buffered changes in
//! `accept` but only patched the searchable body inside `trigger_save`.
//! The [`super::super::ApplyTrigger`] closes that gap — when buffered
//! churn exists and [`super::super::JournalLoopConfig::apply_interval`]
//! has elapsed, the loop fires [`PatchSink::trigger_apply`] to patch
//! the body without the disk write.
//!
//! These tests pin three contracts:
//!
//! 1. **Apply fires without a save** — on the short interval the body is
//!    patched (`trigger_apply`) while no `trigger_save` fires.
//! 2. **A save subsumes the apply** — when a save threshold crosses on the same
//!    tick, `trigger_save` fires and the redundant `trigger_apply` is
//!    suppressed (the save already drained + applied the buffer).
//! 3. **The [`ApplyTrigger`] state machine** — churn-gated, interval-
//!    rate-limited, and reset by a save.
//!
//! [`ApplyTrigger`]: super::super::ApplyTrigger
//! [`PatchSink::trigger_apply`]: super::super::PatchSink::trigger_apply

use alloc::sync::Arc;
use core::time::Duration;

use super::super::{
    ApplyTrigger, JournalLoopConfig, JournalSource, PatchSink, SaveTrigger, process_tick,
    spawn_journal_loop,
};
use super::{
    CONVERGENCE_DEADLINE, FakeJournalSource, RecordingSink, null_cursor_store, one_change, wait_for,
};

/// A [`JournalLoopConfig`] whose apply cadence fires on every tick with
/// churn (`apply_interval == 0`) and whose save thresholds are pinned
/// out of reach, so a `process_tick` exercises the apply path in
/// isolation from any save.
fn apply_only_config() -> JournalLoopConfig {
    JournalLoopConfig {
        save_threshold_events: u64::MAX,
        save_threshold_age: Duration::from_hours(24),
        apply_interval: Duration::ZERO,
        ..JournalLoopConfig::default()
    }
}

#[test]
fn apply_tick_patches_body_without_save() {
    let sink = RecordingSink::new();
    let mut save_trigger = SaveTrigger::new();
    let mut apply_trigger = ApplyTrigger::new();
    let config = apply_only_config();

    let changes = [one_change(10), one_change(11)];
    process_tick(
        &sink as &dyn PatchSink,
        uffs_mft::platform::DriveLetter::C,
        100,
        &changes,
        &mut save_trigger,
        &mut apply_trigger,
        &config,
    );

    // The body was patched via the apply path...
    assert_eq!(
        sink.apply_calls().as_slice(),
        &[uffs_mft::platform::DriveLetter::C],
        "an apply tick with buffered churn must fire trigger_apply exactly once",
    );
    // ...and crucially NOT via a disk save (the whole point of the split).
    assert!(
        sink.save_calls().is_empty(),
        "the apply tick must not fire a compact-cache save; got {:?}",
        sink.save_calls(),
    );
}

#[test]
fn save_tick_suppresses_redundant_apply() {
    let sink = RecordingSink::new();
    let mut save_trigger = SaveTrigger::new();
    let mut apply_trigger = ApplyTrigger::new();
    // Save crosses on the first event; apply would otherwise fire too
    // (interval 0), so this proves the mutual exclusion: the save
    // subsumes the apply (it drained + applied the same buffer).
    let config = JournalLoopConfig {
        save_threshold_events: 1,
        save_threshold_age: Duration::from_hours(24),
        apply_interval: Duration::ZERO,
        ..JournalLoopConfig::default()
    };

    let changes = [one_change(10), one_change(11)];
    process_tick(
        &sink as &dyn PatchSink,
        uffs_mft::platform::DriveLetter::C,
        100,
        &changes,
        &mut save_trigger,
        &mut apply_trigger,
        &config,
    );

    assert_eq!(
        sink.save_calls().len(),
        1,
        "the save threshold crossed, so exactly one save must fire",
    );
    assert!(
        sink.apply_calls().is_empty(),
        "a save subsumes the apply — no redundant trigger_apply on the same tick; got {:?}",
        sink.apply_calls(),
    );
}

#[test]
fn idle_tick_fires_neither_apply_nor_save() {
    let sink = RecordingSink::new();
    let mut save_trigger = SaveTrigger::new();
    let mut apply_trigger = ApplyTrigger::new();
    let config = apply_only_config();

    // Empty change batch — a no-op poll on a quiescent drive.
    process_tick(
        &sink as &dyn PatchSink,
        uffs_mft::platform::DriveLetter::C,
        100,
        &[],
        &mut save_trigger,
        &mut apply_trigger,
        &config,
    );

    assert!(sink.apply_calls().is_empty(), "idle tick must not apply");
    assert!(sink.save_calls().is_empty(), "idle tick must not save");
}

#[tokio::test]
async fn loop_applies_body_near_live_without_saving() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // A single create-shaped batch.  With a sub-tick apply interval and
    // out-of-reach save thresholds, the loop must patch the body (apply)
    // within a couple of ticks while never firing a disk save.
    source.enqueue_changes(vec![one_change(42)], 100);

    let config = JournalLoopConfig {
        poll_interval: Duration::from_millis(5),
        save_threshold_events: u64::MAX,
        save_threshold_age: Duration::from_hours(24),
        apply_interval: Duration::ZERO,
        ..JournalLoopConfig::default()
    };
    let handle = spawn_journal_loop(
        uffs_mft::platform::DriveLetter::C,
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        null_cursor_store(),
        config,
    );

    let sink_for_pred = Arc::clone(&sink);
    let applied = wait_for(move || !sink_for_pred.apply_calls().is_empty()).await;
    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        applied,
        "the loop must fire a near-live apply within the convergence deadline",
    );
    assert!(
        sink.save_calls().is_empty(),
        "near-live apply must not trigger a compact-cache save; got {:?}",
        sink.save_calls(),
    );
}

#[test]
fn apply_trigger_requires_churn() {
    let mut trigger = ApplyTrigger::new();
    // No events recorded — even a zero interval must not fire.
    assert!(
        !trigger.evaluate(Duration::ZERO),
        "an apply must not fire without buffered churn",
    );
}

#[test]
fn apply_trigger_fires_once_per_interval_then_resets() {
    let mut trigger = ApplyTrigger::new();
    trigger.record(3);
    assert!(
        trigger.evaluate(Duration::ZERO),
        "churn present + interval elapsed must fire",
    );
    // The fire reset the churn counter, so a second evaluate with no
    // new events must not fire again.
    assert!(
        !trigger.evaluate(Duration::ZERO),
        "evaluate must reset the churn counter after firing",
    );
}

#[test]
fn apply_trigger_respects_interval() {
    let mut trigger = ApplyTrigger::new();
    trigger.record(3);
    // Interval far in the future — churn exists but the rate limit
    // holds the apply back.
    assert!(
        !trigger.evaluate(Duration::from_hours(1)),
        "an apply must wait for the interval even with churn pending",
    );
    // The churn is retained (no reset on a held-back tick), so once the
    // interval is satisfied the apply fires.
    assert!(
        trigger.evaluate(Duration::ZERO),
        "retained churn must fire once the interval is satisfied",
    );
}

#[test]
fn reset_after_save_clears_pending_churn() {
    let mut trigger = ApplyTrigger::new();
    trigger.record(5);
    // A save tick drained + applied the buffer; the apply trigger must
    // forget the churn so it doesn't redundantly re-apply.
    trigger.reset_after_save();
    assert!(
        !trigger.evaluate(Duration::ZERO),
        "reset_after_save must clear the churn guard",
    );
}
