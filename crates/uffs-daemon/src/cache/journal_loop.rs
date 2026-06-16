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
//! This module lands the per-shard infrastructure (tasks 7.2 / 7.3 /
//! 7.4 / 7.6 / 7.7).  Production spawning from `lib.rs::spawn_load_task`
//! waits for the activation commit; until then the existing Phase-5
//! 5-min global tick (`refresh_usn_for_warm_shards`) is the live path.

use alloc::sync::Arc;
use core::time::Duration;
use std::time::Instant;

use tokio::sync::watch;
use uffs_mft::usn::FileChange;

/// Default poll interval for the per-shard journal loop (500 ms).
///
/// Overridable at runtime via the `UFFS_USN_POLL_INTERVAL_MS`
/// environment variable; the env-var path lets benchmarks and
/// long-running soak tests slow the tick down to reduce log noise
/// without recompiling.
pub(crate) const DEFAULT_POLL_INTERVAL_MS: u64 = 500;

/// Default events-since-save threshold for triggering a background
/// compact-cache save (Phase 7 task 7.4).
///
/// Sized to approximate the plan's "5% churn" criterion at the
/// typical 1.3 GB × ~7 M-record drive shape (`50_000` events ≈ 0.7%
/// churn, comfortably below 5%).  Saving more frequently would
/// thrash the disk; less frequently would let the on-disk snapshot
/// drift far enough that a cold-boot replay window grows beyond
/// the cost of an incremental save.
pub(crate) const DEFAULT_SAVE_THRESHOLD_EVENTS: u64 = 50_000;

/// Default time-since-save threshold for triggering a background
/// compact-cache save (Phase 7 task 7.4) — 5 minutes.
///
/// Provides a wall-clock ceiling for how stale the on-disk snapshot
/// can get under low-churn workloads (where the events-threshold
/// would never fire on its own).  Five minutes matches the cadence
/// of the existing Phase-5 `refresh_usn_for_warm_shards` global
/// tick so the persistence guarantee carries over to the per-shard
/// path without changing the operator-visible recovery window.
pub(crate) const DEFAULT_SAVE_THRESHOLD_AGE: Duration = Duration::from_mins(5);

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
/// Tests wire it to a `Mutex<Vec<(uffs_mft::platform::DriveLetter,
/// Vec<FileChange>)>>` recorder so the test can assert which letters saw which
/// changes without touching a real registry.
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
    fn accept(&self, letter: uffs_mft::platform::DriveLetter, changes: &[FileChange]) -> bool;

    /// Trigger a background compact-cache save for `letter`.
    ///
    /// Phase 7 task 7.4 — the loop calls this when the
    /// per-shard [`SaveTrigger`] decides the in-memory body has
    /// drifted far enough from the on-disk snapshot that a
    /// persist is warranted.  Production wires this to
    /// [`uffs_core::compact_cache::save_compact_cache_background`];
    /// tests record the call for assertion.
    ///
    /// The trigger is **fire-and-forget**.  The save runs on a
    /// background thread; the loop does not wait for it.  If
    /// the save fails, the implementor logs but does not
    /// propagate — the next threshold crossing will retry.
    ///
    /// `cursor` is the loop's current read position.  The sink
    /// persists it **in lockstep** with the on-disk compact-cache
    /// body — and only when that body save actually succeeds — so
    /// the persisted cursor never outruns the persisted body (a
    /// parked shard's save is a no-op, so its cursor must not
    /// advance on disk; see `journal_sink`).
    fn trigger_save(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        reason: SaveReason,
        cursor: u64,
    );

    /// Notify the sink that the USN journal for `letter` was
    /// detected to have wrapped (Phase 7 task 7.7).
    ///
    /// A wrap is detected when [`JournalPollResult::journal_id`]
    /// changes between two successive non-zero-id polls — the
    /// journal was deleted + recreated (Windows admins can do this,
    /// or the volume can run out of journal space and rotate the
    /// `$UsnJrnl`).  Incremental patches don't apply across a wrap
    /// because the FRS → `compact_idx` mapping is now stale, so the
    /// production sink must force-rebuild the body on the next
    /// promote (typically by evicting the shard back to Cold and
    /// letting the standard cold-load path re-read the MFT).
    ///
    /// The loop resets its cursor to 0 after this call, so the
    /// next poll starts from the new journal's head.  No patches
    /// are applied for the wrap-tick — the sink's `accept` is
    /// **not** called.
    fn journal_wrapped(&self, letter: uffs_mft::platform::DriveLetter);
}

/// Pluggable cursor-persistence surface (Phase 7 task 7.6).
///
/// Production wires [`NullCursorStore`] (always-empty) as a
/// fallback on platforms without a real persisted cursor and the
/// disk-backed implementation lands in the activation commit.
/// Tests wire [`tests::FakeCursorStore`] (in-memory `HashMap`)
/// to drive the load / store path deterministically.
///
/// Both methods are **infallible** at the trait level: any
/// underlying I/O failure must be logged and absorbed by the
/// implementor (cursor persistence is best-effort — a missed
/// store just means the next cold-boot re-replays a few extra
/// seconds of journal entries, which is correct since the body
/// patcher is idempotent on duplicate change records).
pub(crate) trait CursorStore: Send + Sync + 'static {
    /// Load the persisted cursor for `letter`.  Returns 0 (the
    /// "start from journal head" sentinel) when no cursor has
    /// been persisted yet or when the load failed.
    fn load(&self, letter: uffs_mft::platform::DriveLetter) -> u64;

    /// Persist `cursor` for `letter`.  Best-effort — the
    /// implementor logs failures but does not propagate.  The
    /// loop calls this every time it fires a save trigger so
    /// the on-disk cursor advances in lockstep with the on-disk
    /// snapshot.
    fn store(&self, letter: uffs_mft::platform::DriveLetter, cursor: u64);
}

/// Why a [`PatchSink::trigger_save`] call fired.
///
/// Encoded so observability surfaces (logs, metrics) can
/// distinguish heavy-churn-driven saves from time-pressure-driven
/// saves; the production sink also passes this through to the
/// compact-cache writer for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveReason {
    /// `events_since_save >= save_threshold_events` — lots of
    /// churn has accumulated and the on-disk snapshot is
    /// progressively stale.
    EventsExceeded,
    /// `Instant::now() - last_save_at >= save_threshold_age` —
    /// time-pressure path for low-churn drives where the
    /// events threshold would otherwise never fire.
    AgeElapsed,
}

/// Per-shard save-threshold state machine (Phase 7 task 7.4).
///
/// Tracks the wall-clock time of the last save trigger and the
/// number of events accumulated since.  Crossing either the
/// events- or age-threshold (with at least one event pending)
/// produces a [`SaveReason`] and resets both counters.  Held
/// inside the [`JournalLoop`] so each per-shard task carries
/// its own independent counters.
#[derive(Debug)]
struct SaveTrigger {
    /// Wall-clock time of the last save trigger (or, before any
    /// triggers, the loop's spawn time).  Compared against
    /// `Instant::now()` to compute elapsed-since-last-save.
    last_save_at: Instant,
    /// Total events accumulated across [`Self::record`] calls
    /// since the last save trigger.  Compared against
    /// `save_threshold_events` to fire the events-based save.
    events_since_save: u64,
}

impl SaveTrigger {
    /// Construct a fresh trigger with `last_save_at` set to
    /// `Instant::now()` (so the first age-based save can't fire
    /// until at least `save_threshold_age` has elapsed since
    /// loop spawn).
    fn new() -> Self {
        Self {
            last_save_at: Instant::now(),
            events_since_save: 0,
        }
    }

    /// Record `change_count` events accumulating toward the
    /// events-based threshold.  Saturating add so a runaway
    /// drive can't wrap and silently miss the threshold.
    const fn record(&mut self, change_count: u64) {
        self.events_since_save = self.events_since_save.saturating_add(change_count);
    }

    /// Evaluate the thresholds.
    ///
    /// **Returns** `Some(reason)` if a save should fire — and
    /// resets both counters as a side effect (so the next
    /// `evaluate` after a save starts from a clean slate).
    /// Returns `None` when no threshold is crossed *or* when no
    /// events are pending (zero-churn drives never produce
    /// no-op saves).
    fn evaluate(
        &mut self,
        save_threshold_events: u64,
        save_threshold_age: Duration,
    ) -> Option<SaveReason> {
        if self.events_since_save == 0 {
            return None;
        }
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_save_at);
        let reason = if self.events_since_save >= save_threshold_events {
            Some(SaveReason::EventsExceeded)
        } else if elapsed >= save_threshold_age {
            Some(SaveReason::AgeElapsed)
        } else {
            None
        };
        if reason.is_some() {
            self.last_save_at = now;
            self.events_since_save = 0;
        }
        reason
    }
}

/// Configuration for a single [`JournalLoop`] task.
///
/// Carries the tuning knobs the production loop reads from env
/// vars and the test loop sets directly.  Keeping these in one
/// place lets future tasks extend the config without churning
/// the loop signature.
#[derive(Debug, Clone)]
pub(crate) struct JournalLoopConfig {
    /// Cadence between successive polls.  Default 500 ms.
    pub(crate) poll_interval: Duration,
    /// Fallback cursor used when the [`CursorStore`] returns 0
    /// (no persisted cursor for this letter yet).  Tests use 0
    /// as a clean-slate baseline; production keeps it 0 because
    /// real cursor seeding flows through the cursor store.
    pub(crate) initial_cursor: u64,
    /// Events-since-last-save ceiling (Phase 7 task 7.4).
    /// Crossing this triggers a [`SaveReason::EventsExceeded`]
    /// save.  Default [`DEFAULT_SAVE_THRESHOLD_EVENTS`] (50K).
    pub(crate) save_threshold_events: u64,
    /// Time-since-last-save ceiling (Phase 7 task 7.4).
    /// Crossing this triggers a [`SaveReason::AgeElapsed`] save
    /// when at least one event is pending.  Default
    /// [`DEFAULT_SAVE_THRESHOLD_AGE`] (5 min).
    pub(crate) save_threshold_age: Duration,
}

impl Default for JournalLoopConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            initial_cursor: 0,
            save_threshold_events: DEFAULT_SAVE_THRESHOLD_EVENTS,
            save_threshold_age: DEFAULT_SAVE_THRESHOLD_AGE,
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
    letter: uffs_mft::platform::DriveLetter,
    /// Plug-in journal-data source.
    source: Arc<dyn JournalSource>,
    /// Plug-in patch consumer.
    sink: Arc<dyn PatchSink>,
    /// Plug-in cursor-persistence surface (Phase 7 task 7.6).
    /// Loaded once at start of `run` to seed `cursor`; stored at
    /// the same time as each `trigger_save` call so on-disk cursor
    /// and on-disk body advance together.
    cursor_store: Arc<dyn CursorStore>,
    /// Cancellation channel — set to `true` by the daemon shutdown
    /// path; the loop checks on every iteration and exits cleanly.
    cancel_rx: watch::Receiver<bool>,
    /// Tuning knobs.
    config: JournalLoopConfig,
    /// Per-shard save-threshold state (Phase 7 task 7.4).
    /// Mutated on every non-empty tick by [`SaveTrigger::record`]
    /// + [`SaveTrigger::evaluate`].
    save_trigger: SaveTrigger,
    /// Last `journal_id` observed from a non-zero-id poll
    /// (Phase 7 task 7.7 wrap detection).  `None` until the first
    /// non-zero `journal_id` is observed; transitions to
    /// `Some(id)` on the first such poll, then any subsequent
    /// poll with `journal_id != id` (and `journal_id != 0`) fires
    /// `sink.journal_wrapped` and resets the cursor.
    last_journal_id: Option<u64>,
}

impl JournalLoop {
    /// Construct a loop bound to `letter`, polling `source`,
    /// applying via `sink`, persisting cursor via `cursor_store`,
    /// watching `cancel_rx`, configured by `config`.
    #[must_use]
    pub(crate) fn new(
        letter: uffs_mft::platform::DriveLetter,
        source: Arc<dyn JournalSource>,
        sink: Arc<dyn PatchSink>,
        cursor_store: Arc<dyn CursorStore>,
        cancel_rx: watch::Receiver<bool>,
        config: JournalLoopConfig,
    ) -> Self {
        Self {
            letter,
            source,
            sink,
            cursor_store,
            cancel_rx,
            config,
            save_trigger: SaveTrigger::new(),
            last_journal_id: None,
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
        let letter = self.letter;
        // Seed cursor from the persistence store; fall back to
        // the config's initial_cursor when the store returns 0
        // (no persisted cursor for this letter yet).
        let mut cursor = match self.cursor_store.load(letter) {
            0 => self.config.initial_cursor,
            persisted => persisted,
        };
        let mut backoff = PollBackoff::new(self.config.poll_interval, MAX_POLL_BACKOFF);
        loop {
            if !wait_for_next_tick(&mut self.cancel_rx, backoff.current(), letter).await {
                return;
            }

            let result = match poll_blocking(Arc::clone(&self.source), cursor).await {
                Ok(result) => {
                    if backoff.on_success() {
                        tracing::info!(
                            drive = %letter,
                            "Journal poll recovered; resuming normal cadence"
                        );
                    }
                    result
                }
                Err(failure) => {
                    let streak = backoff.on_failure();
                    log_poll_failure(letter, &failure, streak, backoff.current());
                    continue;
                }
            };

            // Wrap-detection (Phase 7 task 7.7): if the journal_id
            // changed between two successive non-zero-id polls, the
            // journal was recreated.  Notify the sink, reset cursor
            // to the new journal's head, skip the patch.
            if let Some(prev_id) = self.last_journal_id
                && result.journal_id != 0
                && result.journal_id != prev_id
            {
                tracing::warn!(
                    drive = %letter,
                    prev_journal_id = prev_id,
                    new_journal_id = result.journal_id,
                    "Journal wrap detected; sink must force-rebuild body"
                );
                self.sink.journal_wrapped(letter);
                cursor = 0;
                self.last_journal_id = Some(result.journal_id);
                self.cursor_store.store(letter, cursor);
                continue;
            }
            if result.journal_id != 0 {
                self.last_journal_id = Some(result.journal_id);
            }

            cursor = result.next_cursor;
            // The cursor is persisted by the sink in lockstep with the
            // compact-cache body save (and only when that save actually
            // happens), so the loop no longer persists it here — doing
            // so would let a parked shard's on-disk cursor outrun its
            // frozen on-disk body.  See `PatchSink::trigger_save`.
            process_tick(
                self.sink.as_ref(),
                letter,
                cursor,
                &result.changes,
                &mut self.save_trigger,
                self.config.save_threshold_events,
                self.config.save_threshold_age,
            );
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
    letter: uffs_mft::platform::DriveLetter,
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

/// Upper bound on the journal-poll backoff cadence.
///
/// When the journal is unavailable the loop backs its cadence off geometrically
/// (see [`PollBackoff`]) up to this ceiling, so a persistently unavailable
/// journal — e.g. a non-elevated daemon whose USN handle isn't brokered yet
/// (FU-2b) — polls at most this often instead of every `poll_interval`.  Small
/// enough that a recovered journal is picked up promptly; large enough that an
/// unavailable one stops flooding the log and the blocking pool.
const MAX_POLL_BACKOFF: Duration = Duration::from_secs(30);

/// Why a journal poll tick produced no result.
struct PollFailure {
    /// Human-readable cause for the log line.
    cause: String,
    /// `true` when the `spawn_blocking` task itself failed (panicked /
    /// cancelled) rather than the source returning an I/O error.
    aborted: bool,
}

/// Geometric backoff for the journal poll cadence.
///
/// The journal can be transiently unavailable (volume revocation, broker
/// reconnect) or — for a non-elevated daemon without a brokered USN handle —
/// persistently access-denied.  Polling every `base` interval in that state
/// floods the log with one WARN per tick (~2/s) and burns a `spawn_blocking`
/// plus an FSCTL per tick for nothing.  This doubles the cadence from `base`
/// toward `cap` on each consecutive failure and snaps back to `base` on the
/// first success, so a healthy journal keeps its tight cadence while an
/// unavailable one goes quiet.
struct PollBackoff {
    /// Healthy cadence (the configured `poll_interval`).
    base: Duration,
    /// Maximum backed-off cadence.
    cap: Duration,
    /// Cadence the next tick will wait.
    current: Duration,
    /// Consecutive failures since the last success.
    consecutive_failures: u32,
}

impl PollBackoff {
    /// Start at the healthy `base` cadence, backing off no slower than `cap`.
    const fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            current: base,
            consecutive_failures: 0,
        }
    }

    /// Cadence the next tick should wait.
    const fn current(&self) -> Duration {
        self.current
    }

    /// Record a successful poll: reset to `base`.  Returns `true` when the loop
    /// was previously backed off, so the caller can log a one-shot recovery.
    const fn on_success(&mut self) -> bool {
        let was_backed_off = self.consecutive_failures > 0;
        self.consecutive_failures = 0;
        self.current = self.base;
        was_backed_off
    }

    /// Record a failed poll: double the cadence (saturating at `cap`).  Returns
    /// the 1-based failure count in the current streak so the caller can log
    /// the first failure loudly and demote the rest.
    fn on_failure(&mut self) -> u32 {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.current = self.current.saturating_mul(2).min(self.cap);
        self.consecutive_failures
    }
}

/// Run one journal poll on the blocking pool.
///
/// **Returns** `Ok(result)` on success, or `Err(PollFailure)` describing the
/// cause — the caller logs it (with backoff-aware severity) and `continue`s.
async fn poll_blocking(
    source: Arc<dyn JournalSource>,
    cursor: u64,
) -> Result<JournalPollResult, PollFailure> {
    match tokio::task::spawn_blocking(move || source.poll(cursor)).await {
        Ok(Ok(res)) => Ok(res),
        Ok(Err(io_err)) => Err(PollFailure {
            cause: io_err.to_string(),
            aborted: false,
        }),
        Err(join_err) => Err(PollFailure {
            cause: join_err.to_string(),
            aborted: true,
        }),
    }
}

/// Log a journal poll failure with backoff-aware severity: the **first**
/// failure of a streak is a WARN (the operator should see the journal went
/// away), every subsequent tick is DEBUG so an unavailable journal doesn't
/// storm the log.
fn log_poll_failure(
    letter: uffs_mft::platform::DriveLetter,
    failure: &PollFailure,
    streak: u32,
    next_interval: Duration,
) {
    let next_ms = u64::try_from(next_interval.as_millis()).unwrap_or(u64::MAX);
    let what = if failure.aborted {
        "Journal poll task aborted"
    } else {
        "Journal poll failed"
    };
    if streak <= 1 {
        tracing::warn!(
            drive = %letter,
            error = %failure.cause,
            next_poll_ms = next_ms,
            "{what}; backing off until the journal recovers"
        );
    } else {
        tracing::debug!(
            drive = %letter,
            error = %failure.cause,
            streak,
            next_poll_ms = next_ms,
            "{what}; still backed off"
        );
    }
}

/// Apply the post-poll change batch to `sink`, or trace-log the
/// no-op tick when `changes` is empty.
///
/// On a non-empty tick, also: (a) records the event count into
/// `save_trigger`, (b) evaluates the save thresholds, and (c)
/// fires [`PatchSink::trigger_save`] (passing `cursor` so the sink can
/// persist it in lockstep with the body save) when a threshold crosses.
fn process_tick(
    sink: &dyn PatchSink,
    letter: uffs_mft::platform::DriveLetter,
    cursor: u64,
    changes: &[FileChange],
    save_trigger: &mut SaveTrigger,
    save_threshold_events: u64,
    save_threshold_age: Duration,
) {
    if changes.is_empty() {
        tracing::trace!(drive = %letter, "Journal poll: no changes");
        return;
    }
    let accepted = sink.accept(letter, changes);
    save_trigger.record(changes.len() as u64);
    if let Some(reason) = save_trigger.evaluate(save_threshold_events, save_threshold_age) {
        sink.trigger_save(letter, reason, cursor);
        tracing::info!(
            drive = %letter,
            ?reason,
            cursor,
            "Journal poll: triggered background compact-cache save"
        );
    }
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
#[must_use]
pub(crate) fn spawn_journal_loop(
    letter: uffs_mft::platform::DriveLetter,
    source: Arc<dyn JournalSource>,
    sink: Arc<dyn PatchSink>,
    cursor_store: Arc<dyn CursorStore>,
    config: JournalLoopConfig,
) -> JournalLoopHandle {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let loop_state = JournalLoop::new(letter, source, sink, cursor_store, cancel_rx, config);
    let join = tokio::spawn(loop_state.run());
    JournalLoopHandle { cancel_tx, join }
}

pub(crate) mod sources;

#[cfg(test)]
mod tests;
