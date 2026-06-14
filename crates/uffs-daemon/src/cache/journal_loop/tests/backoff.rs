// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! FU-2a unit tests for the journal-poll backoff schedule.
//!
//! The journal can be persistently unavailable (a non-elevated daemon whose USN
//! handle isn't brokered yet) — polling every `poll_interval` in that state
//! storms the log and the blocking pool.  [`super::super::PollBackoff`] doubles
//! the cadence from `base` toward `cap` on consecutive failures and snaps back
//! to `base` on the first success.  These tests pin that schedule
//! deterministically (no async, no clock).

use core::time::Duration;

use super::super::{MAX_POLL_BACKOFF, PollBackoff};

/// Cadence doubles on each failure and saturates at `cap`.
#[test]
fn doubles_then_caps() {
    let base = Duration::from_millis(500);
    let cap = Duration::from_secs(30);
    let mut backoff = PollBackoff::new(base, cap);

    assert_eq!(backoff.current(), base, "starts at base");

    // 500ms -> 1s -> 2s -> 4s -> 8s -> 16s -> (32s capped to) 30s.
    let expected = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
        Duration::from_secs(8),
        Duration::from_secs(16),
        cap,
    ];
    for (tick, want) in expected.into_iter().enumerate() {
        let streak = backoff.on_failure();
        assert_eq!(
            u32::try_from(tick).expect("tick fits u32") + 1,
            streak,
            "streak is 1-based and monotonic"
        );
        assert_eq!(backoff.current(), want, "cadence after failure {streak}");
    }

    // Further failures stay pinned at the cap.
    for _ in 0_u32..5 {
        backoff.on_failure();
        assert_eq!(backoff.current(), cap, "stays capped");
    }
}

/// A success resets the cadence to `base` and reports the recovery once.
#[test]
fn resets_on_success() {
    let base = Duration::from_millis(500);
    let mut backoff = PollBackoff::new(base, Duration::from_secs(30));

    backoff.on_failure();
    backoff.on_failure();
    assert!(backoff.current() > base, "backed off after failures");

    assert!(
        backoff.on_success(),
        "first success after failures reports recovery"
    );
    assert_eq!(backoff.current(), base, "snaps back to base");

    assert!(
        !backoff.on_success(),
        "a success while already healthy is not a recovery"
    );
    assert_eq!(backoff.current(), base, "stays at base");
}

/// The failure streak restarts after a success — so the next outage logs its
/// first failure loudly again (`streak == 1`).
#[test]
fn streak_restarts_after_success() {
    let mut backoff = PollBackoff::new(Duration::from_millis(500), Duration::from_secs(30));

    assert_eq!(backoff.on_failure(), 1);
    assert_eq!(backoff.on_failure(), 2);
    backoff.on_success();
    assert_eq!(backoff.on_failure(), 1, "streak restarts after recovery");
}

/// The production cap is a sane, finite ceiling (sanity-pins the const so a
/// future edit to `MAX_POLL_BACKOFF` is a deliberate, reviewed change).
#[test]
fn production_cap_is_bounded() {
    assert_eq!(MAX_POLL_BACKOFF, Duration::from_secs(30));
}
