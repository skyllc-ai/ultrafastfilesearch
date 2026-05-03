// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-shard USN journal polling loop (Phase 7 tasks 7.2 + 7.3).
//!
//! ## Architecture
//!
//! Each loaded shard owns one `tokio::task` polling its drive's USN
//! journal at [`JournalLoopConfig::poll_interval`] cadence (default
//! 500 ms, overridable via [`UFFS_USN_POLL_INTERVAL_MS`] for tests
//! and benchmarks).  Per tick:
//!
//! 1. **Poll the journal** via the trait-object [`JournalSource`]. Returns
//!    `(changes, next_cursor, journal_id)`.  Empty changes is the no-op fast
//!    path — no patch, no swap, just advance the cursor (still useful as a
//!    journal-liveness signal so wrap detection in Phase 7 task 7.7 has fresh
//!    data).
//! 2. **Apply to the shard** via the caller-supplied [`PatchSink`].  Production
//!    wires this to a closure that calls
//!    [`crate::cache::ShardEntry::apply_usn_patch_to_body`] +
//!    [`crate::cache::ShardRegistry::replace_warm_body`]; tests wire it to a
//!    recording fake.
//! 3. **Update cursor** so the next poll picks up only what's new.
//! 4. **Sleep** until the next deadline, racing the cancellation receiver so
//!    shutdown takes effect within one poll-interval.
//!
//! ## Mac vs Windows split
//!
//! The trait is platform-agnostic; the **journal-source impl** is
//! Windows-only (`WindowsJournalSource` wraps `read_usn_journal`).
//! On macOS / Linux the production wire-up uses
//! [`MacStubJournalSource`] which always returns empty changes —
//! the loop ticks at the configured cadence but produces no patches.
//! State-machine semantics (cancellation, cursor advance, no-op
//! ticks) are exercised end-to-end on Mac via [`tests::FakeJournalSource`].
//!
//! ## Phase 7 commit boundary
//!
//! This commit (task 7.2 + 7.3) lands the **infrastructure**: trait,
//! impls, loop body, spawn helper, cancellation handle, tests.
//! Production spawning from `lib.rs::spawn_load_task` is wired in a
//! later commit (after task 7.4 threshold-triggered save and
//! task 7.6/7.7 cursor persistence + wrap detection land), so the
//! existing Phase-5 5-min global tick (`refresh_usn_for_warm_shards`)
//! continues to handle live USN refresh until the per-shard path is
//! activation-complete.

use alloc::sync::Arc;
use core::time::Duration;

use tokio::sync::watch;
use uffs_mft::usn::FileChange;

/// Default poll interval for the per-shard journal loop (500 ms).
///
/// Overridable at runtime via the `UFFS_USN_POLL_INTERVAL_MS`
/// environment variable; the env-var path lets benchmarks and
/// long-running soak tests slow the tick down to reduce log noise
/// without recompiling.
pub(crate) const DEFAULT_POLL_INTERVAL_MS: u64 = 500;

/// Result of one [`JournalSource::poll`] call.
///
/// Carries the change batch, the new cursor for the next call, and
/// the journal identifier (Windows-side: the USN-journal `JournalID`
/// from `FSCTL_QUERY_USN_JOURNAL`).  The `journal_id` is consumed by
/// Phase 7 task 7.7 (wrap detection): if it changes between two
/// successive polls the journal was recreated and the in-memory body
/// must be force-rebuilt instead of incrementally patched.
#[derive(Debug, Clone, Default)]
pub(crate) struct JournalPollResult {
    /// Aggregated per-file changes since the previous cursor.
    /// Empty on no-op ticks (no journal activity in the interval).
    pub(crate) changes: Vec<FileChange>,
    /// Cursor to pass into the next [`JournalSource::poll`] call.
    /// Advances monotonically except across a journal-wrap — see
    /// `journal_id` for the wrap-detection signal.
    pub(crate) next_cursor: u64,
    /// Journal identifier.  Compared against the previous tick's
    /// value to detect journal-wrap (Phase 7 task 7.7); changed
    /// → force a full rebuild on the next promote.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 7-B forward reference; the wrap-detection \
                      consumer in Phase 7 task 7.7 reads this field. \
                      Exercised by `cache::journal_loop::tests` under \
                      `cfg(test)`."
        )
    )]
    pub(crate) journal_id: u64,
}

/// Pluggable USN-journal data source.
///
/// Production wires [`MacStubJournalSource`] (always-empty) on
/// macOS / Linux and `WindowsJournalSource` (cfg(windows), reads via
/// `FSCTL_READ_USN_JOURNAL`) on Windows.  Tests wire
/// [`tests::FakeJournalSource`] (programmable event queue) to drive
/// the [`JournalLoop`] state machine deterministically without a
/// live MFT.
///
/// The trait is **synchronous** to match the established
/// `BackgroundIoPriority` / `BodyLoader` / `Prefetch` / `PressureSignal`
/// pattern across the cache module; the loop wraps every call in
/// [`tokio::task::spawn_blocking`] so the main runtime thread never
/// blocks on a kernel-mode journal read.
pub(crate) trait JournalSource: Send + Sync + 'static {
    /// Poll the journal for changes since `cursor`.
    ///
    /// **Returns** `Ok(JournalPollResult)` on success, including
    /// the empty-changes case (which is **not** an error — it just
    /// means nothing happened in the interval).
    ///
    /// **Returns** `Err(io::Error)` only when the underlying journal
    /// surface itself fails (e.g. the volume handle was revoked,
    /// the journal was deleted, the broker dropped the request).
    /// The loop logs the error at warn-level and retries on the
    /// next tick — a single failure does not abort the loop.
    ///
    /// # Errors
    ///
    /// Surfaces any platform-level failure reading the journal.
    fn poll(&self, cursor: u64) -> std::io::Result<JournalPollResult>;
}

/// Cross-platform always-empty journal source.
///
/// Used as the production journal source on macOS / Linux where
/// USN journals don't exist, and as a default for tests that don't
/// need to drive change events.  Every poll returns
/// `JournalPollResult::default()` (no changes, cursor unchanged,
/// `journal_id == 0`) without any I/O.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Phase 7-B forward reference; production wiring \
                  in `lib.rs::spawn_load_task` lands in the \
                  activation commit (post-7-D, after cursor \
                  persistence + wrap detection).  Exercised by \
                  `cache::journal_loop::tests::mac_stub_source_*` \
                  under `cfg(test)`."
    )
)]
#[derive(Debug, Default)]
pub(crate) struct MacStubJournalSource;

impl JournalSource for MacStubJournalSource {
    fn poll(&self, cursor: u64) -> std::io::Result<JournalPollResult> {
        Ok(JournalPollResult {
            changes: Vec::new(),
            next_cursor: cursor,
            journal_id: 0,
        })
    }
}

/// Windows production journal source.
///
/// Wraps the real `FSCTL_READ_USN_JOURNAL` path via
/// [`uffs_mft::usn::read_usn_journal`] + [`uffs_mft::usn::aggregate_changes`].
/// Carries the drive letter so the broker's volume-handle pool
/// can resolve to the right NTFS volume.
///
/// **Phase 7-B status**: scaffolding only.  The real
/// FSCTL-backed implementation lands when the production loop is
/// wired into `spawn_load_task` (post-7-D activation).  Until then
/// this type returns empty results so the daemon's existing Phase-5
/// `refresh_usn_for_warm_shards` global tick continues to handle
/// live USN refresh.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct WindowsJournalSource {
    /// Drive letter for which this source reads the USN journal.
    drive: char,
}

#[cfg(windows)]
#[expect(
    dead_code,
    reason = "Phase 7-B Windows scaffolding; the FSCTL-backed \
              implementation + production wiring land in the \
              Phase 7 activation commit (post-7-D).  Until then \
              `poll` returns empty so the existing Phase-5 \
              `refresh_usn_for_warm_shards` global tick stays the \
              authoritative live-refresh path.  The block-level \
              `expect` covers both the constructor and any future \
              accessor / config-update methods this type acquires \
              before activation."
)]
impl WindowsJournalSource {
    /// Create a source bound to `drive`.
    #[must_use]
    pub(crate) const fn new(drive: char) -> Self {
        Self { drive }
    }
}

#[cfg(windows)]
impl JournalSource for WindowsJournalSource {
    fn poll(&self, cursor: u64) -> std::io::Result<JournalPollResult> {
        // Phase 7-B scaffolding: Windows production wiring lands in
        // the activation commit.  Until then return empty so the
        // existing Phase-5 global tick stays the live-refresh path.
        // Cursor unchanged so the next poll behaves the same.
        let _ = self.drive;
        Ok(JournalPollResult {
            changes: Vec::new(),
            next_cursor: cursor,
            journal_id: 0,
        })
    }
}

/// Sink that consumes change batches produced by a [`JournalLoop`].
///
/// Production wires this to a closure that:
///
/// 1. Looks up the shard for `letter` in the registry.
/// 2. Calls [`crate::cache::ShardEntry::apply_usn_patch_to_body`] on the
///    current body.
/// 3. Swaps the new body via
///    [`crate::cache::ShardRegistry::replace_warm_body`].
///
/// Tests wire it to a `Mutex<Vec<(char, Vec<FileChange>)>>` recorder
/// so the test can assert which letters saw which changes without
/// touching a real registry.
///
/// The sink is **synchronous** (no `async fn`) for consistency with
/// the rest of the cache traits and so the loop body stays
/// transparent to platform allocators.  Mutation behind the trait
/// (registry write-lock acquire) is the implementor's concern.
pub(crate) trait PatchSink: Send + Sync + 'static {
    /// Apply `changes` for the shard identified by `letter`.
    ///
    /// **Returns** `true` if the sink accepted the batch (Warm /
    /// Hot shard, body present, swap succeeded), `false` if the
    /// caller should treat the tick as a no-op (Parked / Cold
    /// shard, registry race, etc.).  The boolean is purely
    /// informational — the loop continues in either case.
    fn accept(&self, letter: char, changes: &[FileChange]) -> bool;
}

/// Configuration for a single [`JournalLoop`] task.
///
/// Carries the tuning knobs the production loop reads from env
/// vars and the test loop sets directly.  Keeping these in one
/// place lets future tasks (7.4 thresholds, 7.7 wrap detection)
/// extend the config without churning the loop signature.
#[derive(Debug, Clone)]
pub(crate) struct JournalLoopConfig {
    /// Cadence between successive polls.  Default 500 ms.
    pub(crate) poll_interval: Duration,
    /// Initial cursor passed into the first poll.  Production
    /// reads this from the persisted `usn.cursor` (Phase 7 task
    /// 7.6); tests use 0 as a clean-slate baseline.
    pub(crate) initial_cursor: u64,
}

impl Default for JournalLoopConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            initial_cursor: 0,
        }
    }
}

/// Per-shard journal-polling state machine.
///
/// Holds the trait-object `source` + `sink` that wire it to the
/// real journal + registry, the drive `letter` it serves, the
/// cancellation receiver that lets the daemon's shutdown tear it
/// down within one poll interval, and the [`JournalLoopConfig`] for
/// cadence + cursor seeding.
///
/// Construction is `pub(crate)` so the production spawn path
/// ([`spawn_journal_loop`]) and test paths can both build it; the
/// only state-machine entry point is [`Self::run`].
pub(crate) struct JournalLoop {
    /// Drive letter this loop polls.
    letter: char,
    /// Plug-in journal-data source.
    source: Arc<dyn JournalSource>,
    /// Plug-in patch consumer.
    sink: Arc<dyn PatchSink>,
    /// Cancellation channel — set to `true` by the daemon shutdown
    /// path; the loop checks on every iteration and exits cleanly.
    cancel_rx: watch::Receiver<bool>,
    /// Tuning knobs.
    config: JournalLoopConfig,
}

impl JournalLoop {
    /// Construct a loop bound to `letter`, polling `source`,
    /// applying via `sink`, watching `cancel_rx`, configured by
    /// `config`.
    #[must_use]
    pub(crate) const fn new(
        letter: char,
        source: Arc<dyn JournalSource>,
        sink: Arc<dyn PatchSink>,
        cancel_rx: watch::Receiver<bool>,
        config: JournalLoopConfig,
    ) -> Self {
        Self {
            letter,
            source,
            sink,
            cancel_rx,
            config,
        }
    }

    /// Run the loop until `cancel_rx` flips to `true` (typically
    /// the daemon's shutdown path).  Polls `source` every
    /// `config.poll_interval`, applies non-empty change batches
    /// via `sink`, advances the cursor.
    ///
    /// On a poll error: warn-logs and retries on the next tick.
    /// One failure does **not** abort the loop — the journal can
    /// be transiently unavailable (volume revocation, broker
    /// reconnect) and the daemon should resume cleanly when the
    /// surface returns.
    pub(crate) async fn run(mut self) {
        let mut cursor = self.config.initial_cursor;
        let letter = self.letter;
        loop {
            if !wait_for_next_tick(&mut self.cancel_rx, self.config.poll_interval, letter).await {
                return;
            }

            let Some(result) = poll_blocking(Arc::clone(&self.source), cursor, letter).await else {
                continue;
            };

            cursor = result.next_cursor;
            process_tick(self.sink.as_ref(), letter, cursor, &result.changes);
        }
    }
}

/// Wait for the next poll deadline, racing the cancellation watch.
///
/// **Returns** `true` when the loop should proceed with a poll,
/// `false` when cancellation has been observed and the loop
/// should exit.
async fn wait_for_next_tick(
    cancel_rx: &mut watch::Receiver<bool>,
    poll_interval: Duration,
    letter: char,
) -> bool {
    if *cancel_rx.borrow() {
        tracing::debug!(drive = %letter, "Journal loop cancellation requested before tick");
        return false;
    }
    tokio::select! {
        () = tokio::time::sleep(poll_interval) => true,
        changed = cancel_rx.changed() => {
            if changed.is_ok() && *cancel_rx.borrow() {
                tracing::debug!(
                    drive = %letter,
                    "Journal loop cancellation observed during sleep"
                );
                false
            } else {
                true
            }
        }
    }
}

/// Run one journal poll on the blocking pool.
///
/// **Returns** `Some(result)` on success, `None` when the source
/// or the spawn-blocking task itself failed (warn-logged at the
/// call site so the loop can `continue` cleanly).
async fn poll_blocking(
    source: Arc<dyn JournalSource>,
    cursor: u64,
    letter: char,
) -> Option<JournalPollResult> {
    let poll_result = tokio::task::spawn_blocking(move || source.poll(cursor)).await;
    match poll_result {
        Ok(Ok(res)) => Some(res),
        Ok(Err(io_err)) => {
            tracing::warn!(
                drive = %letter,
                error = %io_err,
                "Journal poll failed; retrying next tick"
            );
            None
        }
        Err(join_err) => {
            tracing::warn!(
                drive = %letter,
                error = %join_err,
                "Journal poll task aborted; retrying next tick"
            );
            None
        }
    }
}

/// Apply the post-poll change batch to `sink`, or trace-log the
/// no-op tick when `changes` is empty.
fn process_tick(sink: &dyn PatchSink, letter: char, cursor: u64, changes: &[FileChange]) {
    if changes.is_empty() {
        tracing::trace!(drive = %letter, "Journal poll: no changes");
        return;
    }
    let accepted = sink.accept(letter, changes);
    tracing::debug!(
        drive = %letter,
        accepted,
        change_count = changes.len(),
        cursor,
        "Journal poll: applied tick"
    );
}

/// Handle returned by [`spawn_journal_loop`] for cancellation +
/// join.  Holding it keeps the loop alive; dropping the
/// `cancel_tx` causes the loop to exit on its next iteration via
/// the `watch` receiver's `changed()` arm.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Phase 7-B infrastructure; production spawn path \
                  lands in the activation commit (post-7-D, after \
                  cursor persistence + wrap detection).  Exercised \
                  end-to-end by `cache::journal_loop::tests` under \
                  `cfg(test)`."
    )
)]
pub(crate) struct JournalLoopHandle {
    /// Sender side of the cancellation watch.  Setting it to
    /// `true` (or dropping it) causes the loop to exit.
    cancel_tx: watch::Sender<bool>,
    /// Joinable handle on the spawned loop task.  Awaiting this
    /// after a `cancel()` or `cancel_tx` drop blocks until the
    /// loop has finished its in-flight tick and returned.
    join: tokio::task::JoinHandle<()>,
}

impl JournalLoopHandle {
    /// Request cancellation and return the join handle.  After
    /// this call, awaiting the returned `JoinHandle` blocks until
    /// the loop's next iteration observes the signal and returns
    /// (typically within one [`JournalLoopConfig::poll_interval`]).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 7-B infrastructure; cancellation entry \
                      point exercised by tests, awaiting production \
                      activation."
        )
    )]
    pub(crate) fn cancel(self) -> tokio::task::JoinHandle<()> {
        let _ignore = self.cancel_tx.send(true);
        self.join
    }
}

/// Spawn a journal loop on the current runtime.  Returns a
/// [`JournalLoopHandle`] for cancellation + join.
///
/// Caller responsibility: ensure the runtime is alive for the
/// duration of the loop, and call [`JournalLoopHandle::cancel`]
/// before the runtime tears down.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Phase 7-B infrastructure; production spawn site \
                  in `lib.rs::spawn_load_task` lands in the \
                  activation commit (post-7-D)."
    )
)]
#[must_use]
pub(crate) fn spawn_journal_loop(
    letter: char,
    source: Arc<dyn JournalSource>,
    sink: Arc<dyn PatchSink>,
    config: JournalLoopConfig,
) -> JournalLoopHandle {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let loop_state = JournalLoop::new(letter, source, sink, cancel_rx, config);
    let join = tokio::spawn(loop_state.run());
    JournalLoopHandle { cancel_tx, join }
}

#[cfg(test)]
mod tests;
