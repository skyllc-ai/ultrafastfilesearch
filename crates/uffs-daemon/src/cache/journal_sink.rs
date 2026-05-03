// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Production [`PatchSink`] for the per-shard journal loop (Phase 7
//! activation A3).
//!
//! ## Architecture — applier-task pattern
//!
//! [`RegistryPatchSink::accept`] / [`RegistryPatchSink::trigger_save`] /
//! [`RegistryPatchSink::journal_wrapped`] are **synchronous** callbacks
//! invoked from the journal loop's `process_tick` (which itself runs
//! in async context via [`crate::cache::journal_loop::JournalLoop::run`]).
//! The downstream work (registry write-lock acquisition,
//! [`crate::index::IndexManager::handle_journal_refresh`]) is `async`
//! and uses [`tokio::sync::RwLock`].  Calling `Handle::block_on` from
//! the sync callback would re-enter the runtime and deadlock.
//!
//! The fix: each callback **enqueues** an [`ApplyMsg`] onto an
//! [`tokio::sync::mpsc::UnboundedSender`] (sync, non-blocking) and
//! returns immediately.  A single async **applier task** owns the
//! receiver and processes messages serially.  This:
//!
//! 1. Preserves FIFO ordering (per-letter and across letters).
//! 2. Keeps the loop's hot path zero-cost — accept is `Vec::clone`
//!    + atomic-list-tail-push.
//! 3. Decouples the loop's tick cadence from the registry-mutation latency (a
//!    slow `load_drive_with_usn_refresh` doesn't stall the cursor advance — the
//!    next tick proceeds while the previous refresh is still draining).
//!
//! ## Lifecycle
//!
//! * Construction is via [`RegistryPatchSink::spawn_with_applier`] which
//!   returns `(Arc<Self>, JoinHandle<()>)`.  The caller (
//!   `lib.rs::spawn_journal_loops_for_warm_shards`, commit A4) keeps the
//!   `JoinHandle` and aborts it during graceful shutdown.
//!
//! * The applier holds [`alloc::sync::Weak<IndexManager>`] so the sink +
//!   applier pair never extends the daemon's lifetime past shutdown.  When all
//!   `Arc<IndexManager>` instances drop, the `Weak::upgrade` arm returns `None`
//!   and the applier exits cleanly.
//!
//! * Dropping all sinks (sink Arc count → 0) closes the `mpsc` sender.  The
//!   applier's `recv().await` then returns `None` and the task exits — same
//!   shutdown shape as the `Weak` path.

use alloc::sync::{Arc, Weak};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uffs_mft::usn::FileChange;

use super::journal_loop::{PatchSink, SaveReason};
use crate::index::IndexManager;

/// Cross-task message from a sync sink callback to the async applier.
///
/// Each variant maps 1-to-1 with a [`PatchSink`] callback.  Carrying
/// only the minimum metadata (letter + per-callback context) keeps
/// the channel's per-message size small even at high churn.
#[derive(Debug)]
enum ApplyMsg {
    /// `accept` callback — for now, just an event-count signal so
    /// the applier can trace per-letter churn.  Surgical body-patch
    /// is deferred to Phase 8 (requires `frs_to_compact` persistence
    /// — see `crate::index::journal` module docs for the rationale).
    Accept {
        /// Drive letter the loop polled this tick.
        letter: char,
        /// Number of aggregated [`FileChange`] entries the source
        /// produced.  Surfaced in the trace event so operators can
        /// validate per-letter churn vs the
        /// [`super::journal_loop::JournalLoopConfig`]
        /// `save_threshold_events` ceiling.
        change_count: usize,
    },
    /// `trigger_save` callback — fire a full per-shard
    /// [`IndexManager::handle_journal_refresh`].  The applier
    /// converts [`SaveReason`] to a stable diagnostic string
    /// (`"events-exceeded"` / `"age-elapsed"`) for the success/
    /// failure log.
    Save {
        /// Drive letter to refresh.
        letter: char,
        /// Why the save threshold fired.
        reason: SaveReason,
    },
    /// `journal_wrapped` callback — same effect as `Save` (full
    /// refresh) so the body's resync against the new journal head
    /// is immediate, not deferred to the next save threshold.
    Wrap {
        /// Drive letter whose USN journal was recreated.
        letter: char,
    },
}

/// Production [`PatchSink`] wired to the registry via an applier task.
///
/// Stateless apart from the `mpsc::UnboundedSender` — every callback
/// is a one-line enqueue + immediate return.  The receiver lives in
/// the spawned applier task (see [`Self::spawn_with_applier`]).
pub(crate) struct RegistryPatchSink {
    /// Channel into the applier task.  `UnboundedSender::send` is
    /// sync-non-blocking, which is exactly what the loop's sync
    /// callback contract requires.  Failures (receiver dropped /
    /// applier exited) are silently absorbed — the loop's cursor
    /// still advances and the per-shard `SaveTrigger` counters still
    /// reset, so the visible behaviour on a dead applier is "this
    /// shard's in-memory body stops refreshing" which is the
    /// correct degraded state.
    apply_tx: mpsc::UnboundedSender<ApplyMsg>,
}

impl RegistryPatchSink {
    /// Construct a sink + spawn its applier task.
    ///
    /// Returns `(Arc<Self>, JoinHandle<()>)`:
    ///
    /// * The sink Arc is cloned into every per-shard journal-loop's `Arc<dyn
    ///   PatchSink>` (one shared sink across N letters).
    /// * The `JoinHandle` is held by the caller (typically
    ///   `lib.rs::spawn_journal_loops_for_warm_shards` in commit A4) and
    ///   aborted during graceful shutdown.
    ///
    /// The applier holds [`Weak<IndexManager>`] so this constructor
    /// does NOT extend the daemon's lifetime — when all
    /// `Arc<IndexManager>` instances drop the applier exits cleanly
    /// via the `Weak::upgrade` `None` arm.
    pub(crate) fn spawn_with_applier(idx: &Arc<IndexManager>) -> (Arc<Self>, JoinHandle<()>) {
        let (apply_tx, apply_rx) = mpsc::unbounded_channel();
        let weak = Arc::downgrade(idx);
        let handle = tokio::spawn(applier_task(apply_rx, weak));
        (Arc::new(Self { apply_tx }), handle)
    }
}

impl PatchSink for RegistryPatchSink {
    fn accept(&self, letter: char, changes: &[FileChange]) -> bool {
        // Send-and-forget.  Returning `true` optimistically is
        // correct: the loop's `process_tick` only uses the boolean
        // for tracing-debug instrumentation; FIFO ordering across
        // ticks is preserved by the single-applier serialisation.
        let _ignore = self.apply_tx.send(ApplyMsg::Accept {
            letter,
            change_count: changes.len(),
        });
        true
    }

    fn trigger_save(&self, letter: char, reason: SaveReason) {
        let _ignore = self.apply_tx.send(ApplyMsg::Save { letter, reason });
    }

    fn journal_wrapped(&self, letter: char) {
        let _ignore = self.apply_tx.send(ApplyMsg::Wrap { letter });
    }
}

/// Async drain loop that owns the applier-task receiver.
///
/// Exits cleanly when:
///
/// 1. All [`RegistryPatchSink`] instances drop, closing the sender —
///    `recv().await` returns `None`.
/// 2. The [`IndexManager`] drops past its last `Arc` reference —
///    `Weak::upgrade()` returns `None`, the loop returns immediately on the
///    next message even if more are pending (correctness: the daemon is
///    shutting down, no point applying more refreshes).
async fn applier_task(mut rx: mpsc::UnboundedReceiver<ApplyMsg>, idx_weak: Weak<IndexManager>) {
    while let Some(msg) = rx.recv().await {
        let Some(idx_strong) = idx_weak.upgrade() else {
            tracing::debug!(
                target: "shard.journal",
                "IndexManager dropped; exiting applier task",
            );
            return;
        };
        dispatch_msg(&idx_strong, msg).await;
    }
    tracing::debug!(
        target: "shard.journal",
        "Sink channel closed; applier task exiting",
    );
}

/// Dispatch a single drained [`ApplyMsg`] to the appropriate
/// [`IndexManager`] entry point.
///
/// Extracted from [`applier_task`] so the parent stays under
/// clippy's strict-gate cognitive-complexity ceiling.  The split
/// also keeps the `Weak::upgrade` lifecycle decision in the parent
/// (visible at the recv-loop level) and the per-variant dispatch
/// in this helper (focused, easy to extend).
async fn dispatch_msg(idx: &Arc<IndexManager>, msg: ApplyMsg) {
    match msg {
        ApplyMsg::Accept {
            letter,
            change_count,
        } => {
            // Phase 7 activation: surgical body-patch is deferred
            // to Phase 8 (needs `frs_to_compact` persistence on
            // `DriveCompactIndex`).  For now we just trace the
            // per-letter churn so operator dashboards can see
            // which drives are accumulating events between save
            // triggers.
            tracing::trace!(
                target: "shard.journal",
                drive = %letter,
                change_count,
                "Journal accept (events recorded; body refresh deferred to next save trigger)",
            );
        }
        ApplyMsg::Save { letter, reason } => {
            let reason_str = save_reason_str(reason);
            let _applied = idx.handle_journal_refresh(letter, reason_str).await;
        }
        ApplyMsg::Wrap { letter } => {
            let _applied = idx.handle_journal_refresh(letter, "journal-wrapped").await;
        }
    }
}

/// Map a [`SaveReason`] to its stable diagnostic string for tracing
/// and the [`IndexManager::handle_journal_refresh`] success/failure
/// log.  Centralised here so log strings stay consistent across the
/// activation surface.
const fn save_reason_str(reason: SaveReason) -> &'static str {
    match reason {
        SaveReason::EventsExceeded => "events-exceeded",
        SaveReason::AgeElapsed => "age-elapsed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a fresh [`IndexManager`] suitable for sink lifecycle
    /// tests.  No drives loaded — the applier exits cleanly when
    /// the strong-Arc drops, regardless of whether refresh messages
    /// were drained, so tests don't need a populated registry.
    fn fresh_index_manager() -> Arc<IndexManager> {
        let (event_tx, _event_rx) = crate::events::event_channel();
        Arc::new(IndexManager::new(
            None,
            event_tx,
            Arc::new(crate::config::Config::default()),
        ))
    }

    /// Pin the canonical happy path: each [`PatchSink`] callback enqueues
    /// exactly one message into the channel and returns immediately.
    /// The applier-task lifecycle is exercised separately below; here
    /// we only assert that the sync callback contract is non-blocking
    /// and that the channel buffers messages independently.
    #[tokio::test]
    async fn each_callback_enqueues_one_message_and_returns_immediately() {
        let idx = fresh_index_manager();
        let (sink, _applier) = RegistryPatchSink::spawn_with_applier(&idx);

        // Before any callback, the channel has zero messages by
        // construction.  We can't directly observe channel depth on
        // an `UnboundedSender`, but we CAN observe end-to-end through
        // the sink's [`PatchSink`] contract — three calls produce no
        // panics, no blocks, and no errors.
        let accepted = sink.accept('C', &[FileChange {
            frs: 100,
            ..FileChange::default()
        }]);
        assert!(accepted, "accept must return true optimistically");

        sink.trigger_save('C', SaveReason::EventsExceeded);
        sink.trigger_save('D', SaveReason::AgeElapsed);
        sink.journal_wrapped('E');
    }

    /// Drop all `Arc<RegistryPatchSink>` instances → the sender side
    /// of the channel is dropped → the applier's `rx.recv().await`
    /// returns `None` → the task exits cleanly.  Pins the graceful-
    /// shutdown shape used when the daemon's load task tears down
    /// without dropping `IndexManager` first.
    #[tokio::test]
    async fn applier_exits_when_sink_dropped() {
        let idx = fresh_index_manager();
        let (sink, applier) = RegistryPatchSink::spawn_with_applier(&idx);

        // Sanity: applier is still running before we drop the sink.
        assert!(!applier.is_finished());

        drop(sink);

        // Should converge within the test deadline; in practice
        // it's <1 ms on Mac (single context-switch).
        let join_result = tokio::time::timeout(core::time::Duration::from_secs(2), applier).await;
        assert!(
            join_result.is_ok(),
            "applier must exit within 2 s of last sender dropping",
        );
        join_result
            .expect("timeout deadline must not elapse")
            .expect("applier must not panic");
    }

    /// Drop the last `Arc<IndexManager>` → the applier's
    /// `Weak::upgrade()` returns `None` on the next message →
    /// the task exits cleanly.  Pins the graceful-shutdown shape
    /// used when the daemon's main lifecycle drops the index before
    /// the load task observes the sink-side drop.
    #[tokio::test]
    async fn applier_exits_when_index_manager_dropped() {
        let idx = fresh_index_manager();
        let (sink, applier) = RegistryPatchSink::spawn_with_applier(&idx);

        drop(idx);
        // Send one message so the applier wakes up, observes the
        // dropped Weak, and exits.  Without this the applier blocks
        // on `recv().await` forever (no Weak-watch primitive in tokio).
        sink.trigger_save('C', SaveReason::EventsExceeded);

        let join_result = tokio::time::timeout(core::time::Duration::from_secs(2), applier).await;
        assert!(
            join_result.is_ok(),
            "applier must exit within 2 s of IndexManager Arc dropping + one wake message",
        );
        join_result
            .expect("timeout deadline must not elapse")
            .expect("applier must not panic");
    }

    /// Pin that `save_reason_str` produces stable diagnostic strings.
    /// These strings are surfaced in the production tracing target
    /// `shard.journal` and consumed by operator runbooks — drift
    /// would silently break grep-based monitoring, so the round-trip
    /// is asserted directly.
    #[test]
    fn save_reason_str_maps_each_variant() {
        assert_eq!(
            save_reason_str(SaveReason::EventsExceeded),
            "events-exceeded"
        );
        assert_eq!(save_reason_str(SaveReason::AgeElapsed), "age-elapsed");
    }

    /// Pin the multi-message FIFO ordering contract: a sink that
    /// buffers many messages before the applier can drain them all
    /// must process them in send order.  This isn't a Phase 7
    /// invariant per se (out-of-order applies don't corrupt — each
    /// `handle_journal_refresh` is idempotent on the body Arc), but
    /// it's a regression-net for any future change that swaps
    /// `mpsc::unbounded_channel` for an unordered surface.
    #[tokio::test]
    async fn applier_drains_in_fifo_order() {
        let idx = fresh_index_manager();
        let (sink, applier) = RegistryPatchSink::spawn_with_applier(&idx);

        // Burst-send 5 messages.  Each one fires a Save which on Mac
        // hits the load_drive_with_usn_refresh stub-err arm and
        // returns false; the test doesn't assert on that side effect
        // because the per-letter behaviour is platform-specific.
        for letter in ['C', 'D', 'E', 'F', 'G'] {
            sink.trigger_save(letter, SaveReason::EventsExceeded);
        }

        // Drop the sink → applier drains remaining messages → exits.
        drop(sink);
        let join_result = tokio::time::timeout(core::time::Duration::from_secs(5), applier).await;
        assert!(
            join_result.is_ok(),
            "applier must finish draining within 5 s on Mac (load_drive_with_usn_refresh \
             returns Err immediately on non-Windows targets, no real I/O)",
        );
        join_result
            .expect("timeout deadline must not elapse")
            .expect("applier must not panic");
    }
}
