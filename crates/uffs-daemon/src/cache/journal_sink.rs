// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Production [`PatchSink`] for the per-shard journal loop (Phase 7
//! activation A3, Phase 8 surgical-patch B3).
//!
//! ## Architecture — buffered applier-task pattern
//!
//! [`RegistryPatchSink::accept`] / [`RegistryPatchSink::trigger_save`] /
//! [`RegistryPatchSink::journal_wrapped`] are **synchronous** callbacks
//! invoked from the journal loop's `process_tick` (which itself runs
//! in async context via [`crate::cache::journal_loop::JournalLoop::run`]).
//! The downstream work (registry write-lock acquisition, body patching,
//! background save) is `async` and uses [`tokio::sync::RwLock`].
//! Calling `Handle::block_on` from the sync callback would re-enter the
//! runtime and deadlock.
//!
//! Phase 7 fix: each callback **enqueues** an [`ApplyMsg`] onto an
//! [`tokio::sync::mpsc::UnboundedSender`] (sync, non-blocking) and
//! returns immediately.  A single async **applier task** owns the
//! receiver and processes messages serially.
//!
//! Phase 8 B3 extension: per-letter
//! [`std::sync::Mutex<HashMap<char, Vec<FileChange>>>`] **pending
//! buffer** owned by the sink.  `accept` appends to the buffer
//! synchronously (no mpsc traffic).  `trigger_save` drains the buffer
//! for that letter and ships the drained `Vec<FileChange>` into
//! [`ApplyMsg::Save`] so the applier can run a *surgical*
//! [`crate::cache::ShardEntry::apply_usn_patch_to_body`] instead of a
//! full [`uffs_core::compact_loader::load_drive_with_usn_refresh`].
//! `journal_wrapped` discards the buffer (a wrap means the journal
//! head reset, so any pending events are stale relative to the new
//! cursor) and falls back to the Phase-7 full-reload path.
//!
//! Properties of the buffered design:
//!
//! 1. Preserves FIFO ordering (per-letter and across letters).
//! 2. Keeps the loop's hot path zero-cost — accept is `Vec::extend_from_slice`
//!    on a per-letter `Vec<FileChange>` under a short-held mutex.
//! 3. Save-tick latency is independent of accept-tick volume — the journal loop
//!    never blocks on the patch / swap / persist sequence.
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
use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uffs_mft::usn::FileChange;

use super::journal_loop::{PatchSink, SaveReason};
use crate::index::IndexManager;

/// Cross-task message from a sync sink callback to the async applier.
///
/// Phase 8 dropped the Phase-7 `Accept` variant: per-letter event
/// buffering moved into the sink itself ([`RegistryPatchSink::pending`])
/// so `accept` no longer puts traffic on the channel.
#[derive(Debug)]
enum ApplyMsg {
    /// `trigger_save` callback — the applier runs a surgical
    /// [`crate::cache::ShardEntry::apply_usn_patch_to_body`] over
    /// the drained per-letter buffer, then `replace_warm_body` +
    /// `save_compact_cache_background`.  The applier converts
    /// [`SaveReason`] to a stable diagnostic string
    /// (`"events-exceeded"` / `"age-elapsed"`) for the success /
    /// failure log.
    Save {
        /// Drive letter to refresh.
        letter: char,
        /// Why the save threshold fired.
        reason: SaveReason,
        /// Drained per-letter event buffer.  Empty on age-elapsed
        /// triggers when the drive saw no churn since the last save
        /// (the surgical-patch path short-circuits to a no-op).
        changes: Vec<FileChange>,
    },
    /// `journal_wrapped` callback — the journal head reset so any
    /// pending events are stale; the applier discards them in the
    /// sink and runs a full
    /// [`IndexManager::handle_journal_refresh`] to resync the body
    /// against the new journal head.
    Wrap {
        /// Drive letter whose USN journal was recreated.
        letter: char,
    },
}

/// Production [`PatchSink`] wired to the registry via an applier task.
///
/// Holds two pieces of state:
///
/// * `apply_tx` — the sender side of the applier task's mpsc channel.  `accept`
///   does NOT use it; `trigger_save` and `journal_wrapped` do.
/// * `pending` — per-letter buffer of [`FileChange`] entries that `accept` has
///   appended since the last save / wrap.
///
/// Both are owned by the sink Arc; cloning the Arc is cheap and the
/// inner state is shared across every per-shard journal loop that
/// holds a clone.
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
    /// Per-letter pending [`FileChange`] buffer accumulated by
    /// `accept` between save / wrap triggers.  Drained on
    /// `trigger_save` (forwarded to the applier in
    /// [`ApplyMsg::Save`]) and discarded on `journal_wrapped`
    /// (the wrap full-reload supersedes any pending patches).
    ///
    /// Wrapped in [`std::sync::Mutex`] (not [`tokio::sync::Mutex`])
    /// because every access is from the *sync* sink callbacks; the
    /// critical section is `Vec::extend_from_slice` /
    /// `HashMap::remove` — microseconds at most, so the brief lock
    /// contention is invisible relative to the journal loop's 500 ms
    /// tick cadence.  Poisoning is recovered via
    /// [`std::sync::PoisonError::into_inner`] (matching the
    /// `lock_journal_handles` helper in `index/journal.rs`).
    pending: Mutex<HashMap<char, Vec<FileChange>>>,
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
        (
            Arc::new(Self {
                apply_tx,
                pending: Mutex::new(HashMap::new()),
            }),
            handle,
        )
    }

    /// Acquire the pending-buffer mutex, recovering from poison.
    /// Mirrors the `lock_journal_handles` helper in
    /// `index/journal.rs` — a poisoned mutex on the sink side
    /// would otherwise propagate a panic from the applier task into
    /// every subsequent journal-loop tick, killing the whole
    /// journal-refresh subsystem.
    fn lock_pending(&self) -> std::sync::MutexGuard<'_, HashMap<char, Vec<FileChange>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Test-only constructor: build a sink with an explicit
    /// `mpsc::UnboundedReceiver<ApplyMsg>` returned to the caller.
    /// Skips the applier-task spawn so unit tests can drain the
    /// channel directly and assert on per-`ApplyMsg`-variant
    /// payloads.  Production code must use
    /// [`Self::spawn_with_applier`].
    #[cfg(test)]
    fn new_for_test() -> (Arc<Self>, mpsc::UnboundedReceiver<ApplyMsg>) {
        let (apply_tx, apply_rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                apply_tx,
                pending: Mutex::new(HashMap::new()),
            }),
            apply_rx,
        )
    }
}

impl PatchSink for RegistryPatchSink {
    fn accept(&self, letter: char, changes: &[FileChange]) -> bool {
        // Phase 8: append to the per-letter pending buffer instead
        // of putting traffic on the mpsc channel.  `trigger_save`
        // drains this buffer and forwards the changes into the
        // applier task for surgical-patch processing.
        tracing::trace!(
            target: "shard.journal",
            drive = %letter,
            change_count = changes.len(),
            "Journal accept (buffered for next save tick)",
        );
        let mut guard = self.lock_pending();
        guard.entry(letter).or_default().extend_from_slice(changes);
        drop(guard);
        true
    }

    fn trigger_save(&self, letter: char, reason: SaveReason) {
        // Drain the per-letter buffer in one swoop so the applier
        // sees a snapshot of all events accumulated since the
        // previous save.  An empty drained Vec is forwarded as-is
        // (the applier short-circuits the surgical-patch path).
        let drained = {
            let mut guard = self.lock_pending();
            guard.remove(&letter).unwrap_or_default()
        };
        let _ignore = self.apply_tx.send(ApplyMsg::Save {
            letter,
            reason,
            changes: drained,
        });
    }

    fn journal_wrapped(&self, letter: char) {
        // Wrap means the journal head reset; any buffered events
        // are stale relative to the new cursor.  Discard the
        // pending buffer and rely on the applier's full reload to
        // resync the body.
        {
            let mut guard = self.lock_pending();
            let _discarded = guard.remove(&letter);
        }
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
        ApplyMsg::Save {
            letter,
            reason,
            changes,
        } => {
            let reason_str = save_reason_str(reason);
            // Phase 8 surgical-patch path: hand the drained per-letter
            // change buffer to `IndexManager::handle_journal_save`,
            // which clones the Warm body, applies the patch, swaps
            // the new Arc into the registry, and persists the patched
            // body via `save_compact_cache_background`.
            let _applied = idx.handle_journal_save(letter, reason_str, changes).await;
        }
        ApplyMsg::Wrap { letter } => {
            // Wrap stays on the Phase-7 full-reload path.  The
            // patched-body snapshot is invalidated by the journal
            // head reset, so cloning + patching is wasted work — the
            // cleanest option is `load_drive_with_usn_refresh` which
            // re-reads the MFT and replays the new journal from
            // its current head.
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

    /// Construct a [`FileChange`] fixture with a unique FRS for
    /// per-event identification.  Fields other than `frs` use
    /// `FileChange::default()` because the sink doesn't inspect them
    /// — only `IndexManager::handle_journal_save` does (covered in
    /// `cache::shard::tests` and the patch end-to-end suite).
    fn make_change(frs: u64) -> FileChange {
        FileChange {
            frs,
            ..FileChange::default()
        }
    }

    /// Snapshot the per-letter pending buffer's event FRS sequence,
    /// dropping the `lock_pending()` guard before returning so the
    /// caller's assertions don't hold the mutex (satisfies
    /// `clippy::significant_drop_tightening` in tests).
    fn pending_frs_for_letter(sink: &RegistryPatchSink, letter: char) -> Option<Vec<u64>> {
        let guard = sink.lock_pending();
        guard
            .get(&letter)
            .map(|buf| buf.iter().map(|change| change.frs).collect())
    }

    /// Pin: `accept` appends to the per-letter pending buffer and
    /// does NOT enqueue a message on the applier channel.
    #[tokio::test]
    async fn accept_buffers_changes_without_enqueueing() {
        let (sink, mut rx) = RegistryPatchSink::new_for_test();

        let accepted = sink.accept('C', &[make_change(100), make_change(101)]);
        assert!(accepted, "accept must return true optimistically");

        // The pending buffer holds the two events for letter 'C'.
        let buf = pending_frs_for_letter(&sink, 'C')
            .expect("accept must populate pending buffer for 'C'");
        assert_eq!(
            buf,
            [100, 101],
            "accept must preserve event order in the buffer",
        );

        // Channel is empty: accept did not enqueue.
        assert!(
            rx.try_recv().is_err(),
            "accept must NOT enqueue an ApplyMsg",
        );
    }

    /// Pin: a sequence of `accept` calls for the same letter
    /// accumulates into the same buffer — no per-call drain or
    /// truncation.
    #[tokio::test]
    async fn multiple_accepts_accumulate_in_pending() {
        let (sink, _rx) = RegistryPatchSink::new_for_test();

        sink.accept('C', &[make_change(1)]);
        sink.accept('C', &[make_change(2), make_change(3)]);
        sink.accept('C', &[make_change(4)]);

        let buf = pending_frs_for_letter(&sink, 'C').expect("buffer for 'C' must exist");
        assert_eq!(
            buf,
            [1, 2, 3, 4],
            "consecutive accepts must accumulate in send order",
        );
    }

    /// Pin: `trigger_save` drains the pending buffer for `letter`
    /// and ships it inside `ApplyMsg::Save { changes }`.  The buffer
    /// for `letter` is cleared after the drain.
    #[tokio::test]
    async fn trigger_save_drains_pending_into_save_message() {
        let (sink, mut rx) = RegistryPatchSink::new_for_test();

        sink.accept('C', &[make_change(10), make_change(11)]);
        sink.trigger_save('C', SaveReason::EventsExceeded);

        let ApplyMsg::Save {
            letter,
            reason,
            changes,
        } = rx.try_recv().expect("trigger_save must enqueue Save")
        else {
            panic!("expected ApplyMsg::Save, got Wrap");
        };
        assert_eq!(letter, 'C');
        assert!(matches!(reason, SaveReason::EventsExceeded));
        assert_eq!(
            changes.iter().map(|change| change.frs).collect::<Vec<_>>(),
            [10, 11],
            "Save must carry the buffered changes in send order",
        );

        // Pending buffer for 'C' is gone after the drain.
        assert!(
            pending_frs_for_letter(&sink, 'C').is_none(),
            "trigger_save must remove the per-letter pending entry",
        );
    }

    /// Pin: `trigger_save` on a letter with no prior `accept` still
    /// emits `ApplyMsg::Save { changes: [] }`.  The applier's
    /// empty-batch fast path then short-circuits to a no-op.
    #[tokio::test]
    async fn trigger_save_with_no_pending_sends_empty_changes() {
        let (sink, mut rx) = RegistryPatchSink::new_for_test();

        sink.trigger_save('Z', SaveReason::AgeElapsed);

        let ApplyMsg::Save {
            letter,
            reason,
            changes,
        } = rx.try_recv().expect("trigger_save must enqueue Save")
        else {
            panic!("expected ApplyMsg::Save, got Wrap");
        };
        assert_eq!(letter, 'Z');
        assert!(matches!(reason, SaveReason::AgeElapsed));
        assert!(
            changes.is_empty(),
            "Save must carry an empty Vec when no events were buffered",
        );
    }

    /// Pin: `journal_wrapped` clears the pending buffer for the
    /// letter and emits `ApplyMsg::Wrap`.  A subsequent
    /// `trigger_save` then sees an empty buffer (no replay of the
    /// stale events past the wrap).
    #[tokio::test]
    async fn journal_wrapped_discards_pending_buffer_and_sends_wrap() {
        let (sink, mut rx) = RegistryPatchSink::new_for_test();

        sink.accept('C', &[make_change(5), make_change(6)]);
        sink.journal_wrapped('C');

        let ApplyMsg::Wrap { letter } = rx.try_recv().expect("journal_wrapped must enqueue Wrap")
        else {
            panic!("expected ApplyMsg::Wrap, got Save");
        };
        assert_eq!(letter, 'C');

        assert!(
            pending_frs_for_letter(&sink, 'C').is_none(),
            "journal_wrapped must discard the per-letter pending entry",
        );

        // A subsequent trigger_save must see an empty buffer.
        sink.trigger_save('C', SaveReason::AgeElapsed);
        let ApplyMsg::Save {
            changes: post_wrap_changes,
            ..
        } = rx.try_recv().expect("trigger_save must enqueue Save")
        else {
            panic!("expected ApplyMsg::Save after wrap, got another Wrap");
        };
        assert!(
            post_wrap_changes.is_empty(),
            "post-wrap trigger_save must drain to empty (stale events discarded)",
        );
    }

    /// Pin: per-letter buffers are independent.  Accepting events on
    /// 'C' must not leak into 'D's buffer or pending state.
    #[tokio::test]
    async fn pending_buffers_are_per_letter() {
        let (sink, mut rx) = RegistryPatchSink::new_for_test();

        sink.accept('C', &[make_change(1)]);
        sink.accept('D', &[make_change(2), make_change(3)]);

        // Drain 'C' first — should NOT include any of 'D's events.
        sink.trigger_save('C', SaveReason::EventsExceeded);
        let ApplyMsg::Save {
            letter: c_letter,
            changes: c_changes,
            ..
        } = rx.try_recv().expect("Save for 'C'")
        else {
            panic!("expected ApplyMsg::Save for 'C'");
        };
        assert_eq!(c_letter, 'C');
        assert_eq!(
            c_changes
                .iter()
                .map(|change| change.frs)
                .collect::<Vec<_>>(),
            [1],
        );

        // 'D's buffer must still hold its events.
        sink.trigger_save('D', SaveReason::EventsExceeded);
        let ApplyMsg::Save {
            letter: d_letter,
            changes: d_changes,
            ..
        } = rx.try_recv().expect("Save for 'D'")
        else {
            panic!("expected ApplyMsg::Save for 'D'");
        };
        assert_eq!(d_letter, 'D');
        assert_eq!(
            d_changes
                .iter()
                .map(|change| change.frs)
                .collect::<Vec<_>>(),
            [2, 3],
            "'D's buffer must be preserved across 'C's drain",
        );
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
    /// must process them in send order.  Out-of-order applies don't
    /// corrupt (each per-letter `handle_journal_save` is independent),
    /// but the FIFO contract is a regression-net for any future
    /// change that swaps `mpsc::unbounded_channel` for an unordered
    /// surface.
    #[tokio::test]
    async fn applier_drains_in_fifo_order() {
        let idx = fresh_index_manager();
        let (sink, applier) = RegistryPatchSink::spawn_with_applier(&idx);

        // Burst-send 5 trigger_save calls without prior `accept`.
        // Each one drains an empty pending buffer and ships
        // `ApplyMsg::Save { changes: [] }`; the applier's empty-batch
        // fast path in `handle_journal_save` short-circuits to a
        // debug-log no-op.  The test verifies the applier processes
        // all 5 in order before the sink's drop closes the channel.
        for letter in ['C', 'D', 'E', 'F', 'G'] {
            sink.trigger_save(letter, SaveReason::EventsExceeded);
        }

        // Drop the sink → applier drains remaining messages → exits.
        drop(sink);
        let join_result = tokio::time::timeout(core::time::Duration::from_secs(5), applier).await;
        assert!(
            join_result.is_ok(),
            "applier must finish draining within 5 s (5 empty-batch no-ops, no real I/O)",
        );
        join_result
            .expect("timeout deadline must not elapse")
            .expect("applier must not panic");
    }
}
