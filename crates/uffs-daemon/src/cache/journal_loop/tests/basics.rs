// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 7-B basic loop-lifecycle tests for the per-shard USN
//! journal loop.
//!
//! Pins the state-machine contract for the foundational tasks 7.2
//! + 7.3:
//!
//! * **Empty ticks** — no `accept()` call when the journal returns no changes.
//! * **Non-empty ticks** — exactly one `accept()` call per tick with the
//!   matching change batch and drive letter.
//! * **Cursor advance** — each poll receives the cursor returned by the
//!   previous poll.
//! * **Cancellation** — `JournalLoopHandle::cancel()` causes the loop to exit
//!   within one poll-interval.
//! * **Source error recovery** — a single `Err` from the source does not abort
//!   the loop; the next tick proceeds normally.
//! * **`MacStubJournalSource` always-empty fallback** — production journal
//!   source on macOS / Linux returns `(empty, cursor, 0)` without I/O.

use alloc::sync::Arc;

use super::super::sources::MacStubJournalSource;
use super::super::{JournalSource, PatchSink, spawn_journal_loop};
use super::{
    CONVERGENCE_DEADLINE, FakeJournalSource, RecordingSink, fast_config, null_cursor_store,
    one_change, wait_for,
};

#[tokio::test]
async fn empty_tick_does_not_call_accept() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Empty queue → poll() returns default (empty changes).
    let handle = spawn_journal_loop(
        uffs_mft::platform::DriveLetter::C,
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        null_cursor_store(),
        fast_config(),
    );

    // Deterministic synchronisation (issue #208): wait until the
    // loop has actually polled the source at least 3 times — well
    // above any first-tick startup race — then assert the empty-tick
    // contract held across those ticks.  Replaces the previous
    // wall-clock `sleep(50ms)` which raced on slow CI runners and
    // produced `FLKY-FL` retries (see issue #208).  Uses the same
    // [`wait_for`] deadline-driven helper every other liveness test
    // in this module already uses.
    let source_for_pred = Arc::clone(&source);
    let polled = wait_for(move || source_for_pred.cursors_seen().len() >= 3).await;

    let join = handle.cancel();
    drop(tokio::time::timeout(CONVERGENCE_DEADLINE, join).await);

    assert!(
        polled,
        "loop must have polled the source ≥ 3 times within \
         {CONVERGENCE_DEADLINE:?} so the empty-tick contract is \
         exercised across multiple ticks; got {} polls",
        source.cursors_seen().len()
    );
    assert!(
        sink.calls().is_empty(),
        "no accept() call should fire when every poll returns empty changes"
    );
}

#[tokio::test]
async fn non_empty_tick_invokes_accept_once() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    source.enqueue_changes(vec![one_change(10), one_change(11)], 100);

    let handle = spawn_journal_loop(
        uffs_mft::platform::DriveLetter::C,
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        null_cursor_store(),
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
        Some((uffs_mft::platform::DriveLetter::C, 2_usize)),
        "letter + change-count must round-trip"
    );
}

#[tokio::test]
async fn cursor_advances_monotonically_across_ticks() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Queue three successful polls with increasing next_cursor.
    source.enqueue_changes(vec![one_change(10)], 100);
    source.enqueue_changes(vec![one_change(11)], 200);
    source.enqueue_changes(vec![one_change(12)], 300);

    let handle = spawn_journal_loop(
        uffs_mft::platform::DriveLetter::C,
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        null_cursor_store(),
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

#[tokio::test]
async fn source_error_does_not_abort_loop() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    // Pattern: Err → Ok with changes.  Loop must skip the Err
    // and apply the next batch on the following tick.
    source.enqueue_error(std::io::Error::other("fake source error"));
    source.enqueue_changes(vec![one_change(10)], 100);

    let handle = spawn_journal_loop(
        uffs_mft::platform::DriveLetter::C,
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        null_cursor_store(),
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

#[tokio::test]
async fn cancel_exits_within_convergence_deadline() {
    let source = Arc::new(FakeJournalSource::new());
    let sink = Arc::new(RecordingSink::new());

    let handle = spawn_journal_loop(
        uffs_mft::platform::DriveLetter::C,
        Arc::clone(&source) as Arc<dyn JournalSource>,
        Arc::clone(&sink) as Arc<dyn PatchSink>,
        null_cursor_store(),
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

#[test]
fn mac_stub_source_returns_empty_with_unchanged_cursor() -> std::io::Result<()> {
    let stub = MacStubJournalSource;
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
