// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Mac-deterministic tests for the per-shard USN journal loop
//! (Phase 7 tasks 7.2 + 7.3).
//!
//! These tests drive [`super::JournalLoop`] end-to-end via a
//! programmable [`FakeJournalSource`] + a recording
//! [`RecordingSink`], pinning the state-machine contract:
//!
//! * **Empty ticks** — no `accept()` call when the journal returns no changes.
//! * **Non-empty ticks** — exactly one `accept()` call per tick with the
//!   matching change batch and drive letter.
//! * **Cursor advance** — each poll receives the cursor returned by the
//!   previous poll (monotonic except across journal-wrap, which Phase 7 task
//!   7.7 will pin once activated).
//! * **Cancellation** — `JournalLoopHandle::cancel()` causes the loop to exit
//!   within one poll-interval.
//! * **Source error recovery** — a single `Err` from the source does not abort
//!   the loop; the next tick proceeds normally.
//!
//! No Windows host, no live MFT, no `read_usn_journal` syscall:
//! the entire surface is covered by deterministic time-based
//! tokio runtime control.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::time::Duration;
use std::sync::Mutex;

use uffs_mft::usn::FileChange;

use super::{
    JournalLoopConfig, JournalPollResult, JournalSource, PatchSink, SaveReason, spawn_journal_loop,
};

/// Acquire `mutex`, defusing any poison error by recovering the
/// inner guard.  Test code never deliberately poisons — this
/// helper just satisfies clippy's `expect_used` denial without
/// silencing it across the file.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Programmable journal source.
///
/// Holds a queue of poll results that the test pre-loads.  Each
/// call to `poll` pops the front of the queue (or returns
/// `JournalPollResult::default()` when empty so the loop has a
/// well-defined post-drain steady state).  Polls are also recorded
/// for assertions on cursor monotonicity.
struct FakeJournalSource {
    /// Queued results, popped in FIFO order on `poll()`.
    /// `Result<JournalPollResult, io::Error>` so error-recovery
    /// tests can interleave failures with successful polls.
    queue: Mutex<VecDeque<std::io::Result<JournalPollResult>>>,
    /// Cursor values that were passed into `poll()`, in call order.
    /// Lets tests assert the loop carried each `next_cursor`
    /// forward into the subsequent call.
    cursors_seen: Mutex<Vec<u64>>,
}

impl FakeJournalSource {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            cursors_seen: Mutex::new(Vec::new()),
        }
    }

    /// Enqueue a successful poll result.
    fn enqueue_changes(&self, changes: Vec<FileChange>, next_cursor: u64) {
        lock_or_recover(&self.queue).push_back(Ok(JournalPollResult {
            changes,
            next_cursor,
            journal_id: 1,
        }));
    }

    /// Enqueue an `io::Error` to exercise the source-error retry path.
    ///
    /// Takes a fully-constructed `std::io::Error` (rather than an
    /// `ErrorKind` taxonomy) because `core::io::ErrorKind` is still
    /// nightly-gated (rust-lang/rust#154046) and the workspace lint
    /// `restriction::std_instead_of_core` flags any `std::io::ErrorKind`
    /// reference that happens to be available via `core` on a newer
    /// toolchain.  Passing the constructed error keeps the test
    /// surface stable across both nightly + stable.
    fn enqueue_error(&self, error: std::io::Error) {
        lock_or_recover(&self.queue).push_back(Err(error));
    }

    /// Snapshot of cursor values seen since construction.
    fn cursors_seen(&self) -> Vec<u64> {
        lock_or_recover(&self.cursors_seen).clone()
    }
}

impl JournalSource for FakeJournalSource {
    fn poll(&self, cursor: u64) -> std::io::Result<JournalPollResult> {
        lock_or_recover(&self.cursors_seen).push(cursor);
        let popped = lock_or_recover(&self.queue).pop_front();
        match popped {
            Some(Ok(res)) => Ok(res),
            Some(Err(err)) => Err(err),
            None => Ok(JournalPollResult {
                changes: Vec::new(),
                next_cursor: cursor,
                journal_id: 1,
            }),
        }
    }
}

/// Recording sink that captures every `accept()` invocation.
struct RecordingSink {
    /// One entry per `accept()` call: `(letter, change_count)`.
    /// Storing only the count (not the full `Vec<FileChange>`)
    /// keeps the Mutex contention minimal and the assertions
    /// focused on the loop semantics rather than payload shape.
    calls: Mutex<Vec<(char, usize)>>,
    /// One entry per `trigger_save()` call: `(letter, reason)`.
    /// Phase 7-C surface — lets tests assert the threshold state
    /// machine fires the right reason at the right time.
    save_calls: Mutex<Vec<(char, SaveReason)>>,
    /// Boolean to return from `accept()`.  Tests flip this to
    /// `false` to exercise the registry-race / Parked-shard path
    /// (loop must continue cleanly when accept returns false).
    accept_outcome: Mutex<bool>,
}

impl RecordingSink {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            save_calls: Mutex::new(Vec::new()),
            accept_outcome: Mutex::new(true),
        }
    }

    fn calls(&self) -> Vec<(char, usize)> {
        lock_or_recover(&self.calls).clone()
    }

    fn save_calls(&self) -> Vec<(char, SaveReason)> {
        lock_or_recover(&self.save_calls).clone()
    }
}

impl PatchSink for RecordingSink {
    fn accept(&self, letter: char, changes: &[FileChange]) -> bool {
        lock_or_recover(&self.calls).push((letter, changes.len()));
        *lock_or_recover(&self.accept_outcome)
    }

    fn trigger_save(&self, letter: char, reason: SaveReason) {
        lock_or_recover(&self.save_calls).push((letter, reason));
    }
}

/// Helper: small fake change so the queue is exercising a
/// non-trivial batch shape (not just `vec![]`).
fn one_change(frs: u64) -> FileChange {
    FileChange {
        frs,
        deleted: true,
        ..FileChange::default()
    }
}

/// Helper: build a [`JournalLoopConfig`] with a fast-tick
/// `poll_interval` so the loop fires multiple ticks in a
/// hundred-millisecond test budget without flaking on slower
/// CI runners.  5 ms is well below any realistic CI scheduler
/// jitter while still being long enough that `spawn_blocking`
/// work can complete between ticks.  Save thresholds are set
/// generously so the tick / cancel / cursor tests don't
/// accidentally trigger saves; threshold tests override.
fn fast_config() -> JournalLoopConfig {
    JournalLoopConfig {
        poll_interval: Duration::from_millis(5),
        initial_cursor: 0,
        save_threshold_events: u64::MAX,
        save_threshold_age: Duration::from_hours(24),
    }
}

/// Deadline for polling assertions — the maximum wall-clock time
/// the test will wait for the loop to converge on the expected
/// state.  At a 5 ms tick interval this gives the loop ~50
/// chances to fire, well above what every test below actually
/// needs.  Mirrors the `lifecycle_hooks.rs::CASCADE_DEADLINE`
/// idiom of "give the runtime a generous wall-clock budget
/// before declaring the convergence missed".
const CONVERGENCE_DEADLINE: Duration = Duration::from_millis(250);

/// Yield to the runtime + sleep a small amount so the journal
/// loop has a chance to fire one tick.  Used by every assertion
/// helper below.
async fn yield_one_tick() {
    tokio::time::sleep(Duration::from_millis(10)).await;
}

/// Drive the runtime forward until `predicate()` returns `true`
/// or [`CONVERGENCE_DEADLINE`] elapses.  Returns `true` on
/// convergence, `false` on timeout — the caller asserts.
async fn wait_for<F: Fn() -> bool>(predicate: F) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < CONVERGENCE_DEADLINE {
        if predicate() {
            return true;
        }
        yield_one_tick().await;
    }
    predicate()
}

// ── Empty-tick contract: no accept() call when source has no changes ─

#[tokio::test]
async fn empty_tick_does_not_call_accept() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Empty queue → poll() returns default (empty changes).
    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        fast_config(),
    );

    // Let several ticks fire — every one returns empty changes.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        sink.calls().is_empty(),
        "no accept() call should fire when every poll returns empty changes"
    );
    // The source must still have been polled — prove the loop
    // wasn't simply stuck before reaching the source.
    assert!(
        !source.cursors_seen().is_empty(),
        "loop must have polled the source at least once"
    );
}

// ── Non-empty tick contract: exactly one accept() with matching batch ─

#[tokio::test]
async fn non_empty_tick_invokes_accept_once() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    source.enqueue_changes(vec![one_change(10), one_change(11)], 100);

    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        fast_config(),
    );

    // Wait until the recording sink sees exactly one accept().
    let sink_for_pred = Arc::clone(&sink);
    let converged = wait_for(move || sink_for_pred.calls().len() == 1).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        converged,
        "loop did not invoke accept() within {CONVERGENCE_DEADLINE:?}"
    );
    let calls = sink.calls();
    assert_eq!(calls.len(), 1, "exactly one accept() call expected");
    assert_eq!(
        calls.first().copied(),
        Some(('C', 2_usize)),
        "letter + change-count must round-trip"
    );
}

// ── Cursor monotonicity: each poll sees the previous next_cursor ─────

#[tokio::test]
async fn cursor_advances_monotonically_across_ticks() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Queue three successful polls with increasing next_cursor.
    source.enqueue_changes(vec![one_change(10)], 100);
    source.enqueue_changes(vec![one_change(11)], 200);
    source.enqueue_changes(vec![one_change(12)], 300);

    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        fast_config(),
    );

    // Wait until at least 3 polls have been observed.
    let source_for_pred = Arc::clone(&source);
    let converged = wait_for(move || source_for_pred.cursors_seen().len() >= 3).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        converged,
        "loop did not produce 3 polls within {CONVERGENCE_DEADLINE:?}"
    );
    let cursors = source.cursors_seen();
    // First poll uses the initial cursor (0).  Subsequent polls
    // see the previous next_cursor (100, then 200).
    assert_eq!(
        cursors.first().copied(),
        Some(0),
        "first poll must use initial cursor"
    );
    assert_eq!(
        cursors.get(1).copied(),
        Some(100),
        "second poll must carry next_cursor=100"
    );
    assert_eq!(
        cursors.get(2).copied(),
        Some(200),
        "third poll must carry next_cursor=200"
    );
}

// ── Source-error retry: one Err does not abort the loop ──────────────

#[tokio::test]
async fn source_error_does_not_abort_loop() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Pattern: Err → Ok with changes.  Loop must skip the Err
    // and apply the next batch on the following tick.
    source.enqueue_error(std::io::Error::other("fake source error"));
    source.enqueue_changes(vec![one_change(10)], 100);

    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        fast_config(),
    );

    // Wait for accept() to fire — proves the loop survived the Err.
    let sink_for_pred = Arc::clone(&sink);
    let converged = wait_for(move || !sink_for_pred.calls().is_empty()).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        converged,
        "loop must have continued past the Err and applied the subsequent batch within {CONVERGENCE_DEADLINE:?}; got 0 accept() calls"
    );
}

// ── Cancellation contract: cancel() exits within CONVERGENCE_DEADLINE ─

#[tokio::test]
async fn cancel_exits_within_convergence_deadline() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        fast_config(),
    );

    // Cancel immediately and assert the join handle resolves
    // within the convergence deadline.
    let join = handle.cancel();
    let result = tokio::time::timeout(CONVERGENCE_DEADLINE, join).await;
    assert!(
        result.is_ok(),
        "cancellation must drive the loop to exit within {CONVERGENCE_DEADLINE:?}; timed out instead"
    );
}

// ── Phase 7-C: events-based save threshold fires after enough churn ──

#[tokio::test]
async fn events_threshold_triggers_save() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Two batches of 3 + 4 = 7 events; threshold of 5 means the
    // second batch must trigger an EventsExceeded save.
    source.enqueue_changes(vec![one_change(10), one_change(11), one_change(12)], 100);
    source.enqueue_changes(
        vec![
            one_change(13),
            one_change(14),
            one_change(15),
            one_change(16),
        ],
        200,
    );

    let config = JournalLoopConfig {
        poll_interval: Duration::from_millis(5),
        initial_cursor: 0,
        save_threshold_events: 5,
        save_threshold_age: Duration::from_hours(24),
    };
    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        config,
    );

    // Wait for both accept()s + the save trigger.
    let sink_for_pred = Arc::clone(&sink);
    let converged = wait_for(move || !sink_for_pred.save_calls().is_empty()).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        converged,
        "events threshold did not fire trigger_save within {CONVERGENCE_DEADLINE:?}"
    );
    let saves = sink.save_calls();
    assert_eq!(
        saves.len(),
        1,
        "exactly one EventsExceeded save expected; got {saves:?}"
    );
    assert_eq!(
        saves.first().copied(),
        Some(('C', SaveReason::EventsExceeded)),
        "save reason must be EventsExceeded"
    );
}

// ── Phase 7-C: age-based save threshold fires after time elapses ─────

#[tokio::test]
async fn age_threshold_triggers_save_with_pending_events() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // First tick lands one event (under the events-threshold,
    // and well within the age-threshold from spawn time).
    source.enqueue_changes(vec![one_change(10)], 100);

    let config = JournalLoopConfig {
        poll_interval: Duration::from_millis(5),
        initial_cursor: 0,
        save_threshold_events: u64::MAX,
        save_threshold_age: Duration::from_millis(30),
    };
    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        config,
    );

    // Wait for the first batch to land via accept() — proves
    // events_since_save is now > 0.
    let sink_for_accept_pred = Arc::clone(&sink);
    let first_accept_landed = wait_for(move || !sink_for_accept_pred.calls().is_empty()).await;
    assert!(
        first_accept_landed,
        "first batch must land before age can be tested"
    );

    // Sleep past the 30 ms age threshold while events stay pending
    // (no save fires on empty ticks — those skip the threshold
    // evaluation per Phase 7 task 7.4 design).
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Now enqueue the second batch.  Its tick will observe
    // elapsed >= 30 ms with events_since_save > 0, firing AgeElapsed.
    source.enqueue_changes(vec![one_change(11)], 200);

    // Wait for the save to fire.
    let sink_for_save_pred = Arc::clone(&sink);
    let converged = wait_for(move || !sink_for_save_pred.save_calls().is_empty()).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        converged,
        "age threshold did not fire trigger_save within {CONVERGENCE_DEADLINE:?}"
    );
    let saves = sink.save_calls();
    assert_eq!(
        saves.first().copied(),
        Some(('C', SaveReason::AgeElapsed)),
        "save reason must be AgeElapsed; got {saves:?}"
    );
}

// ── Phase 7-C: zero-event drive never triggers a save ─────────────────

#[tokio::test]
async fn zero_events_drive_does_not_trigger_save() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Empty queue + tight age threshold.  The age threshold
    // SHOULD NOT fire because no events have ever been recorded
    // — Phase 7 task 7.4 contract: zero-churn drives produce
    // no wasteful saves.
    let config = JournalLoopConfig {
        poll_interval: Duration::from_millis(5),
        initial_cursor: 0,
        save_threshold_events: 1,
        save_threshold_age: Duration::from_millis(20),
    };
    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        config,
    );

    // Let several ticks fire well past the age threshold.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    let saves = sink.save_calls();
    assert!(
        saves.is_empty(),
        "zero-event drive must not produce any save triggers; got {saves:?}"
    );
    // Sanity: the loop did poll the source.
    assert!(
        !source.cursors_seen().is_empty(),
        "loop must have polled the source"
    );
}

// ── Phase 7-C: counter resets after save — no double-fire on next tick ──

#[tokio::test]
async fn counter_resets_after_save() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // First batch of 5 events crosses the events_threshold = 5
    // → first save fires.  Second batch of 1 event must NOT
    // trigger another save (counter reset; 1 < 5).
    source.enqueue_changes(
        vec![
            one_change(10),
            one_change(11),
            one_change(12),
            one_change(13),
            one_change(14),
        ],
        100,
    );
    source.enqueue_changes(vec![one_change(15)], 200);

    let config = JournalLoopConfig {
        poll_interval: Duration::from_millis(5),
        initial_cursor: 0,
        save_threshold_events: 5,
        save_threshold_age: Duration::from_hours(24),
    };
    let handle = spawn_journal_loop(
        'C',
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        config,
    );

    // Wait until both accept() calls land + first save fires.
    let sink_for_pred = Arc::clone(&sink);
    let converged = wait_for(move || {
        sink_for_pred.calls().len() >= 2 && !sink_for_pred.save_calls().is_empty()
    })
    .await;

    // Give the loop one extra tick window to potentially fire
    // a wrong second save.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(converged, "loop did not converge within deadline");
    let saves = sink.save_calls();
    assert_eq!(
        saves.len(),
        1,
        "exactly one save expected (counter reset after first); got {saves:?}"
    );
}

// ── MacStubJournalSource production fallback: always-empty contract ──

#[test]
fn mac_stub_source_returns_empty_with_unchanged_cursor() -> std::io::Result<()> {
    let stub = super::MacStubJournalSource;
    let res = stub.poll(42)?;
    assert!(
        res.changes.is_empty(),
        "MacStub must always return empty changes"
    );
    assert_eq!(
        res.next_cursor, 42,
        "MacStub must keep the cursor unchanged"
    );
    assert_eq!(res.journal_id, 0, "MacStub journal_id is the zero sentinel");
    Ok(())
}
