// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Process-level memory-pressure signal pipeline (Phase 5 task 5.3).
//!
//! Cooperates with Windows's
//! [`MEMORY_RESOURCE_NOTIFICATION_TYPE`][win32-mem]:
//! `LowMemoryResourceNotification` fires when free RAM drops below the
//! kernel's threshold; `HighMemoryResourceNotification` fires when it
//! rises back above.  The daemon's subscriber loop translates `Low`
//! into a cascade demote of LRU Warm shards
//! (see [`crate::index::IndexManager::cascade_demote_one_step`])
//! until either the registry has no Warm shards left or `High` arrives.
//!
//! Mac/Linux ship a never-fires stub — there is no portable
//! process-wide notification API on those targets and the kernel
//! handles reclaim itself; demotions are TTL-driven via
//! [`crate::index::IndexManager::demote_idle_shards`].
//!
//! ## Wire shape
//!
//! [`IndexManager`][im] holds the trait as
//! `Arc<dyn PressureSignal>`.  Production wires
//! [`PlatformPressureSignal`]; the Phase 5 unit tests inject
//! [`tests::ControllablePressureSignal`] so the test can `set(Low)` /
//! `set(High)` and assert the cascade behaviour deterministically
//! without any real OS pressure.
//!
//! The signal is delivered as a [`tokio::sync::watch::Receiver`]
//! returned by [`PressureSignal::subscribe`].  `watch` is the right
//! primitive here: it carries the *latest* value (so a subscriber
//! that joined late still sees the current pressure level) and
//! `changed().await` only wakes on transitions.  Multiple subscribers
//! are supported but the production daemon only needs one (in
//! `lib.rs::spawn_pressure_subscriber`).
//!
//! [im]: crate::index::IndexManager
//! [win32-mem]: https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-creatememoryresourcenotification

use tokio::sync::watch;

/// Memory-pressure level reported by [`PressureSignal::subscribe`].
///
/// Production translates Windows's
/// `LowMemoryResourceNotification` / `HighMemoryResourceNotification`
/// into [`Self::Low`] / [`Self::High`]; [`Self::Normal`] is the
/// initial value before any notification arrives and the steady
/// state on Mac/Linux.
///
/// `Low` and `High` are *pattern-matched* in production
/// (`lib.rs::spawn_pressure_subscriber`) but only *constructed* by
/// the `tests::ControllablePressureSignal` fake until the Win32
/// watcher thread lands in a follow-up commit on this Phase 5
/// branch.  The targeted `#[expect(dead_code, …)]` attributes on
/// `Low` / `High` will *fail to compile* once the watcher thread
/// starts constructing them — exactly the right tripwire to ensure
/// the suppressions get removed when they're no longer needed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PressureLevel {
    /// No pressure signal yet, or steady state.  Subscriber takes no
    /// action.
    Normal,
    /// Free RAM has fallen below the kernel's low-memory threshold.
    /// Subscriber cascade-demotes LRU Warm shards.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Constructed by the Win32 watcher thread (Phase 5 follow-up commit \
                      on this branch) and by tests::ControllablePressureSignal; \
                      production pattern-matches in lib.rs::spawn_pressure_subscriber \
                      but never constructs in non-test code paths until the watcher \
                      lands — at which point this attribute will fail to compile and \
                      force its own removal."
        )
    )]
    Low,
    /// Free RAM has risen back above the kernel's high-memory
    /// threshold; pressure cleared.  Subscriber stops the cascade.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Constructed by the Win32 watcher thread (Phase 5 follow-up commit \
                      on this branch) and by tests::ControllablePressureSignal; \
                      production pattern-matches in lib.rs::spawn_pressure_subscriber \
                      but never constructs in non-test code paths until the watcher \
                      lands — at which point this attribute will fail to compile and \
                      force its own removal."
        )
    )]
    High,
}

/// Process-level memory-pressure subscriber.
///
/// Implementations are held as `Arc<dyn PressureSignal>` on
/// [`crate::index::IndexManager`].  The daemon's
/// `spawn_pressure_subscriber` calls [`Self::subscribe`] to get a
/// [`watch::Receiver`] and reacts to transitions.
///
/// Implementors must be `Send + Sync + 'static` to satisfy the
/// `Arc<dyn ...>` bound.
pub(crate) trait PressureSignal: Send + Sync + 'static {
    /// Subscribe to pressure-level transitions.
    ///
    /// The returned receiver carries the *current* pressure value
    /// (so a late subscriber still sees the right state) and
    /// `changed().await` wakes on every transition.  Multiple
    /// subscribers are supported.
    fn subscribe(&self) -> watch::Receiver<PressureLevel>;
}

/// Production pressure-signal implementation.
///
/// On Windows: future commit (Phase 5 task 5.6 wire-up) will spawn
/// a thread that calls `WaitForMultipleObjects(low_event, high_event)`
/// on handles from `CreateMemoryResourceNotification` and translates
/// the OS notifications into [`PressureLevel::Low`] /
/// [`PressureLevel::High`] sends on the inner [`watch::Sender`].
/// On Mac/Linux: never-fires sender held internally — receivers
/// always see [`PressureLevel::Normal`] and `changed()` never returns.
///
/// Phase 5 task 5.3 — paired with the Phase 5 dogfood gate
/// "stress test … daemon log shows `cache.pressure { level: \"Low\" }`
/// and demotion cascade".
pub(crate) struct PlatformPressureSignal {
    /// The watch sender held internally.  Mac/Linux never `send`s on
    /// this; Windows's future watcher thread will.  Holding the
    /// sender keeps the channel open for as long as the
    /// `IndexManager` lives, even when no subscriber is currently
    /// attached (e.g. during the brief startup window before
    /// `spawn_pressure_subscriber` runs).
    sender: watch::Sender<PressureLevel>,
}

impl PlatformPressureSignal {
    /// Create a new platform pressure signal.
    ///
    /// Initialises the watch channel at [`PressureLevel::Normal`] —
    /// the steady state.  Mac/Linux daemons never observe a
    /// transition; Windows daemons will see Low/High once the
    /// future Win32 watcher thread is wired in (currently unwired
    /// — landing in this same Phase 5 PR's follow-up commit so
    /// the pipeline can be tested end-to-end on Mac first).
    #[must_use]
    pub(crate) fn new() -> Self {
        let (sender, _initial_rx) = watch::channel(PressureLevel::Normal);
        // Drop the initial receiver immediately — subscribers attach
        // via [`Self::subscribe`].  `watch::Sender::send` returns
        // `Err` if no receivers exist; that's fine on Mac where we
        // never call `send`.  On Windows the watcher thread will
        // ignore that error during the (vanishingly small) startup
        // window before `spawn_pressure_subscriber` runs.
        Self { sender }
    }
}

impl Default for PlatformPressureSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl PressureSignal for PlatformPressureSignal {
    fn subscribe(&self) -> watch::Receiver<PressureLevel> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{PressureLevel, PressureSignal, watch};

    /// Phase 5 task 5.10 fake.  Holds the watch sender so tests can
    /// broadcast pressure transitions deterministically and assert
    /// the daemon's cascade-demote behaviour without any real OS
    /// pressure.
    ///
    /// `set(level)` is a thin wrapper over [`watch::Sender::send_replace`]
    /// (rather than [`watch::Sender::send`]) so the stored value is
    /// **always** updated even when no subscribers are attached.
    /// `send` returns `Err` and leaves the slot untouched when
    /// `receiver_count() == 0`, which would silently drop the
    /// transition for late subscribers — defeating the late-attach
    /// contract that this fake is designed to model.
    pub(crate) struct ControllablePressureSignal {
        sender: watch::Sender<PressureLevel>,
    }

    impl ControllablePressureSignal {
        pub(crate) fn new() -> Self {
            let (sender, _initial_rx) = watch::channel(PressureLevel::Normal);
            Self { sender }
        }

        /// Broadcast a new pressure level.  Always stores the value
        /// (so a future subscriber will see it on first
        /// `borrow_and_update`).  Returns `true` when at least one
        /// subscriber was attached at the time of the call (and
        /// therefore observed the transition); `false` otherwise.
        ///
        /// The `receiver_count` read is racy under concurrent
        /// subscribe/drop, but tests using this fake drive the
        /// timeline serially so the read is sufficient.
        pub(crate) fn set(&self, level: PressureLevel) -> bool {
            let had_receivers = self.sender.receiver_count() > 0;
            // `send_replace` never fails — it stores the value
            // unconditionally and notifies any receivers in place.
            // We discard the previous value (the test fake doesn't
            // need it; production never calls `set`).
            let _previous = self.sender.send_replace(level);
            had_receivers
        }

        /// Number of currently attached subscribers.  Used by the
        /// 5.10 test to spin until the daemon's
        /// `spawn_pressure_subscriber` has called `subscribe()`
        /// before broadcasting the first `Low` transition.
        pub(crate) fn receiver_count(&self) -> usize {
            self.sender.receiver_count()
        }
    }

    impl PressureSignal for ControllablePressureSignal {
        fn subscribe(&self) -> watch::Receiver<PressureLevel> {
            self.sender.subscribe()
        }
    }

    /// Smoke-test the production stub on Mac/Linux: a fresh
    /// signal is at `Normal`, `subscribe()` returns a usable
    /// receiver, and `changed().await` would never fire (we
    /// don't actually await it here — that would hang the test).
    #[test]
    fn platform_pressure_signal_initial_value_is_normal() {
        let signal = super::PlatformPressureSignal::new();
        let rx = signal.subscribe();
        assert_eq!(*rx.borrow(), PressureLevel::Normal);
    }

    /// Controllable fake delivers transitions to live subscribers.
    /// Pinned here (not in `index/tests.rs`) so the fake's contract
    /// is colocated with its definition.
    #[tokio::test]
    async fn controllable_pressure_signal_delivers_transitions() {
        let signal = ControllablePressureSignal::new();
        let mut rx = signal.subscribe();

        assert_eq!(*rx.borrow(), PressureLevel::Normal);
        assert_eq!(signal.receiver_count(), 1);

        assert!(signal.set(PressureLevel::Low));
        rx.changed()
            .await
            .expect("watch::Sender still alive — changed() must succeed");
        assert_eq!(*rx.borrow_and_update(), PressureLevel::Low);

        assert!(signal.set(PressureLevel::High));
        rx.changed().await.expect("changed() must succeed");
        assert_eq!(*rx.borrow_and_update(), PressureLevel::High);
    }

    /// Setting a level with no subscribers attached returns `false`
    /// but the latest value is preserved on the channel — a future
    /// subscriber will see it on first `borrow_and_update`.
    #[tokio::test]
    async fn controllable_pressure_signal_preserves_value_for_late_subscribers() {
        let signal = ControllablePressureSignal::new();

        assert_eq!(signal.receiver_count(), 0);
        assert!(
            !signal.set(PressureLevel::Low),
            "no subscribers — send returns false",
        );

        let rx = signal.subscribe();
        assert_eq!(
            *rx.borrow(),
            PressureLevel::Low,
            "late subscriber still sees the most recent value",
        );
    }
}
