// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Poll scheduling + failure backoff for the per-shard [`super::JournalLoop`].
//!
//! Houses the cancellation-aware inter-poll wait, the one-shot blocking poll
//! of the [`JournalSource`], and the exponential backoff that keeps a failing
//! drive (an `os 995` re-warm abort, a parked volume) from storming the log
//! and the blocking pool.  Extracted from `journal_loop.rs` to keep that file
//! under the workspace 800-LOC policy while keeping the poll-timing concern as
//! one auditable unit.

use alloc::sync::Arc;
use core::time::Duration;

use tokio::sync::watch;

use super::{JournalPollResult, JournalSource};

/// Wait for the next poll deadline, racing the cancellation watch.
///
/// **Returns** `true` when the loop should proceed with a poll,
/// `false` when cancellation has been observed and the loop
/// should exit.
pub(super) async fn wait_for_next_tick(
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
pub(crate) const MAX_POLL_BACKOFF: Duration = Duration::from_secs(30);

/// Why a journal poll tick produced no result.
pub(super) struct PollFailure {
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
pub(crate) struct PollBackoff {
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
    pub(crate) const fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            current: base,
            consecutive_failures: 0,
        }
    }

    /// Cadence the next tick should wait.
    pub(crate) const fn current(&self) -> Duration {
        self.current
    }

    /// Record a successful poll: reset to `base`.  Returns `true` when the loop
    /// was previously backed off, so the caller can log a one-shot recovery.
    pub(crate) const fn on_success(&mut self) -> bool {
        let was_backed_off = self.consecutive_failures > 0;
        self.consecutive_failures = 0;
        self.current = self.base;
        was_backed_off
    }

    /// Record a failed poll: double the cadence (saturating at `cap`).  Returns
    /// the 1-based failure count in the current streak so the caller can log
    /// the first failure loudly and demote the rest.
    pub(crate) fn on_failure(&mut self) -> u32 {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.current = self.current.saturating_mul(2).min(self.cap);
        self.consecutive_failures
    }
}

/// Run one journal poll on the blocking pool.
///
/// **Returns** `Ok(result)` on success, or `Err(PollFailure)` describing the
/// cause — the caller logs it (with backoff-aware severity) and `continue`s.
pub(super) async fn poll_blocking(
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
pub(super) fn log_poll_failure(
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
