// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Thread-level background-I/O priority hook (Phase 5 task 5.7).
//!
//! Cooperates with Windows's
//! [`SetThreadPriority`][win32-setthreadpriority] +
//! `THREAD_MODE_BACKGROUND_BEGIN` / `_END` flags.  When a thread
//! enters `THREAD_MODE_BACKGROUND_BEGIN`, the kernel:
//!
//! * lowers the thread's I/O priority to "very low" (background) so physical
//!   disk reads/writes from this thread yield to any foreground I/O,
//! * lowers memory-allocation priority so the thread's working set gets trimmed
//!   first under pressure,
//! * keeps CPU scheduling priority unchanged (the daemon's foreground RPC
//!   handlers stay responsive).
//!
//! The pairing `THREAD_MODE_BACKGROUND_END` reverses both effects.
//! Crucially this is a **per-thread** API — `SetPriorityClass` with
//! `PROCESS_MODE_BACKGROUND_BEGIN` would lower the entire daemon
//! and starve user-driven RPC handlers, which we do **not** want.
//!
//! `tokio::task::spawn_blocking` runs each closure on a thread from
//! the blocking pool, where the same thread is reused for later
//! work after the closure returns.  Without RAII cleanup, a panic
//! between `begin()` and `end()` would leave the pool thread stuck
//! at background priority for unrelated future tasks — so this
//! module ships [`BackgroundIoScope`], a guard whose [`Drop`] always
//! restores normal priority even on unwind.
//!
//! Mac / Linux ship a never-fires stub.  Linux has
//! `ioprio_set(IOPRIO_CLASS_IDLE)` and `setpriority(PRIO_PROCESS, ..)`
//! but the per-thread semantics differ from Windows
//! `THREAD_MODE_BACKGROUND_BEGIN` (no atomic memory-priority
//! lowering, no automatic working-set trim ordering) and Phase 5
//! deliberately scopes background-priority cooperation to Windows
//! where the "Modern Standby"-style throttling behaviour is the
//! reference contract.  Daemons on Mac / Linux already let the
//! kernel arbitrate between foreground RPCs and the periodic
//! housekeeping tick via the standard CFS scheduler — the absent
//! syscall is the right answer there.
//!
//! ## Wire shape
//!
//! [`crate::index::IndexManager`] holds the trait as
//! `Arc<dyn BackgroundIoPriority>`.  Production wires
//! [`PlatformBackgroundIoPriority`]; the Phase 5 unit tests inject
//! [`tests::CountingBackgroundIoPriority`] so the test can assert
//! `begin()` + `end()` pair exactly once per
//! `tokio::task::spawn_blocking` closure that runs the periodic
//! USN refresh tick.
//!
//! Production hooks the guard at the top of every per-letter
//! closure spawned by
//! [`crate::index::IndexManager::refresh_usn_for_warm_shards`] —
//! the periodic 5-min housekeeping tick that:
//!
//! * reads each Warm/Hot drive's USN journal (background read),
//! * applies the deltas to a fresh `MftIndex` (CPU + heap),
//! * persists the rebuilt body via
//!   [`uffs_core::compact_cache::save_compact_cache_background`] (background
//!   write).
//!
//! User-driven re-promote in
//! [`crate::index::IndexManager::ensure_warm_for_dispatch`] is
//! **not** wrapped — the user is actively waiting for results, so
//! that path stays at normal priority.
//!
//! [win32-setthreadpriority]: https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadpriority

use alloc::sync::Arc;
use std::io;

/// Thread-level background-I/O priority hook.
///
/// Implementations are held as `Arc<dyn BackgroundIoPriority>` on
/// [`crate::index::IndexManager`].  Called from inside the
/// `tokio::task::spawn_blocking` closures of
/// [`crate::index::IndexManager::refresh_usn_for_warm_shards`]
/// (Phase 5 task 5.7).  Both methods operate on the **calling
/// thread**, not the process, so concurrent USN refreshes for
/// multiple drives each enter / leave background mode independently
/// without affecting the foreground RPC handler threads.
///
/// Implementors must be `Send + Sync + 'static` to satisfy the
/// `Arc<dyn ...>` bound.  Both methods are `&self` — the trait
/// itself holds no per-thread state; that lives in the OS thread's
/// priority register, which the windows-rs syscall manipulates
/// directly.
pub(crate) trait BackgroundIoPriority: Send + Sync + 'static {
    /// Mark the calling thread as background-I/O priority.
    ///
    /// Best-effort: any I/O error is returned to the caller (and
    /// the [`BackgroundIoScope`] guard logs + ignores it).  On
    /// Mac/Linux this returns `Ok(())` immediately (no-op).
    fn begin(&self) -> io::Result<()>;

    /// Restore the calling thread to normal priority.
    ///
    /// Idempotent — safe to call even if `begin()` was never
    /// invoked or already ended (the Win32 syscall is a no-op when
    /// the thread isn't in background mode).
    fn end(&self) -> io::Result<()>;
}

/// Production background-I/O priority implementation.
///
/// On Windows: pairs
/// [`SetThreadPriority`][st] with
/// `THREAD_MODE_BACKGROUND_BEGIN` (in [`Self::begin`]) and
/// `THREAD_MODE_BACKGROUND_END` (in [`Self::end`]).  On Mac/Linux:
/// no-op (see module docs for rationale).
///
/// Phase 5 task 5.7 — paired with the Phase 5 dogfood gate
/// "during USN catch-up, Task Manager I/O priority on `uffsd.exe`
/// shows 'Low'".
///
/// [st]: https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadpriority
pub(crate) struct PlatformBackgroundIoPriority;

impl BackgroundIoPriority for PlatformBackgroundIoPriority {
    #[cfg(target_os = "windows")]
    fn begin(&self) -> io::Result<()> {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
        };

        #[expect(
            unsafe_code,
            reason = "GetCurrentThread Win32 FFI returning a thread pseudo-handle"
        )]
        // SAFETY: `GetCurrentThread` returns a pseudo-handle that is
        // always valid for the lifetime of the calling thread; the
        // pseudo-handle does not need closing.
        let thread = unsafe { GetCurrentThread() };
        #[expect(
            unsafe_code,
            reason = "SetThreadPriority Win32 FFI on the current thread's pseudo-handle"
        )]
        // SAFETY: `thread` was just obtained from `GetCurrentThread`
        // above and is valid for the calling thread's lifetime.
        // `THREAD_MODE_BACKGROUND_BEGIN` is the documented Win32 flag
        // for transitioning the calling thread to background-I/O
        // priority; failures translate to `io::Error::other` so the
        // `BackgroundIoScope` guard's `tracing::debug!` line stays
        // platform-agnostic.
        let result = unsafe { SetThreadPriority(thread, THREAD_MODE_BACKGROUND_BEGIN) };
        result.map_err(|err| io::Error::other(err.to_string()))
    }

    #[cfg(target_os = "windows")]
    fn end(&self) -> io::Result<()> {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_END,
        };

        #[expect(
            unsafe_code,
            reason = "GetCurrentThread Win32 FFI returning a thread pseudo-handle"
        )]
        // SAFETY: `GetCurrentThread` returns a pseudo-handle that is
        // always valid for the lifetime of the calling thread; the
        // pseudo-handle does not need closing.
        let thread = unsafe { GetCurrentThread() };
        #[expect(
            unsafe_code,
            reason = "SetThreadPriority Win32 FFI on the current thread's pseudo-handle"
        )]
        // SAFETY: `thread` was just obtained from `GetCurrentThread`
        // above; the `_END` flag is the documented pair to `_BEGIN`
        // and the kernel treats the call as a no-op when the thread
        // isn't currently in background mode (so calling `end`
        // without a matching `begin` is harmless).
        let result = unsafe { SetThreadPriority(thread, THREAD_MODE_BACKGROUND_END) };
        result.map_err(|err| io::Error::other(err.to_string()))
    }

    #[cfg(not(target_os = "windows"))]
    fn begin(&self) -> io::Result<()> {
        // Mac/Linux: no-op stub.  See module docs for rationale.
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    fn end(&self) -> io::Result<()> {
        // Mac/Linux: no-op stub — symmetric with `begin`.
        Ok(())
    }
}

/// RAII guard that pairs [`BackgroundIoPriority::begin`] on creation
/// with [`BackgroundIoPriority::end`] on [`Drop`].
///
/// Constructed via [`Self::enter`].  Holding the guard keeps the
/// calling thread in background-I/O priority mode; dropping it
/// restores normal priority — even on unwind across a panic, since
/// `Drop` runs during stack unwinding.
///
/// `begin()` failures are logged at `target: "shard.refresh"` (debug
/// level) and the guard stays inactive: [`Drop`] becomes a no-op
/// rather than calling `end()` on a thread that never entered
/// background mode.
///
/// `Send` so the guard can outlive the `move`-in to a
/// `tokio::task::spawn_blocking` closure (the `Arc` inside is
/// already `Send + Sync`).  Not `Sync` — the underlying
/// `SetThreadPriority` syscall is per-thread, and the guard models
/// "this **specific thread** is in background mode"; passing the
/// guard to another thread would not transfer the priority change.
pub(crate) struct BackgroundIoScope {
    /// The trait object keeps the implementation alive for the
    /// duration of the scope; `Arc` is the canonical way to share
    /// the platform impl with `IndexManager`'s holder.
    priority: Arc<dyn BackgroundIoPriority>,
    /// `true` when `begin()` succeeded — the only state that
    /// triggers the matching `end()` on drop.  `false` after a
    /// `begin()` failure (we logged + continued at normal priority,
    /// so there's nothing to undo).
    active: bool,
}

impl BackgroundIoScope {
    /// Enter background-I/O priority for the calling thread.
    ///
    /// Always returns a guard, even when `begin()` errors — that
    /// way the caller can `let _scope = ...;` without an `if let`
    /// dance.  An inactive guard's [`Drop`] is a no-op.
    pub(crate) fn enter(priority: Arc<dyn BackgroundIoPriority>) -> Self {
        let active = match priority.begin() {
            Ok(()) => true,
            Err(err) => {
                tracing::debug!(
                    target: "shard.refresh",
                    error = %err,
                    "BackgroundIoPriority::begin failed; continuing at normal priority",
                );
                false
            }
        };
        Self { priority, active }
    }
}

impl Drop for BackgroundIoScope {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Err(err) = self.priority.end() {
            tracing::debug!(
                target: "shard.refresh",
                error = %err,
                "BackgroundIoPriority::end failed; thread may stay in background mode",
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::{BackgroundIoPriority, BackgroundIoScope};

    /// Phase 5 task 5.7 fake.  Counts every `begin()` / `end()`
    /// invocation so the USN-refresh test can assert the hook
    /// fires exactly once per `spawn_blocking` closure (paired
    /// `begin` + `end`).
    ///
    /// The two counters are independent atomics so a missing
    /// `end()` (e.g. a `Drop`-skipped guard from a `mem::forget`)
    /// shows up as `begins() != ends()` instead of silently
    /// passing.
    pub(crate) struct CountingBackgroundIoPriority {
        begins: AtomicUsize,
        ends: AtomicUsize,
    }

    impl CountingBackgroundIoPriority {
        pub(crate) const fn new() -> Self {
            Self {
                begins: AtomicUsize::new(0),
                ends: AtomicUsize::new(0),
            }
        }

        pub(crate) fn begins(&self) -> usize {
            self.begins.load(Ordering::Acquire)
        }

        pub(crate) fn ends(&self) -> usize {
            self.ends.load(Ordering::Acquire)
        }
    }

    impl BackgroundIoPriority for CountingBackgroundIoPriority {
        fn begin(&self) -> std::io::Result<()> {
            self.begins.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        fn end(&self) -> std::io::Result<()> {
            self.ends.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    /// Smoke-test the production no-op stub on Mac/Linux: returns
    /// `Ok(())` immediately, no panic, no I/O.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn platform_background_io_priority_is_noop_on_unix() {
        let priority = super::PlatformBackgroundIoPriority;
        for _ in 0_i32..32_i32 {
            priority.begin().expect("Mac/Linux begin() never errors");
            priority.end().expect("Mac/Linux end() never errors");
        }
    }

    /// Counting fake increments `begins` and `ends` independently
    /// so the test can pin each side.
    #[test]
    fn counting_background_io_priority_increments_independently() {
        let priority = CountingBackgroundIoPriority::new();
        assert_eq!(priority.begins(), 0);
        assert_eq!(priority.ends(), 0);

        priority.begin().expect("counting begin never errors");
        assert_eq!(priority.begins(), 1);
        assert_eq!(priority.ends(), 0);

        priority.end().expect("counting end never errors");
        assert_eq!(priority.begins(), 1);
        assert_eq!(priority.ends(), 1);
    }

    /// `BackgroundIoScope::enter` calls `begin()` on creation and
    /// `end()` on `Drop` — the canonical RAII contract that the
    /// USN-refresh wire-up relies on for panic safety.
    #[test]
    fn background_io_scope_pairs_begin_and_end_on_drop() {
        let priority = Arc::new(CountingBackgroundIoPriority::new());

        // Hold the guard in a named binding so an explicit `drop`
        // (rather than a nested `{ ... }` lexical scope) is the
        // signal that the matching `end()` should fire.  Avoids the
        // `clippy::semicolon_inside_block` ambiguity that the
        // bare `{ let _scope = ...; ... }` form triggers, and is
        // arguably clearer about *when* the RAII pair completes.
        let scope =
            BackgroundIoScope::enter(Arc::clone(&priority) as Arc<dyn BackgroundIoPriority>);
        assert_eq!(priority.begins(), 1);
        assert_eq!(priority.ends(), 0, "end() not yet called inside scope");

        drop(scope);

        assert_eq!(
            priority.ends(),
            1,
            "Drop must call end() to balance begin()",
        );
        assert_eq!(priority.begins(), 1, "begin() not called twice");
    }

    /// Multiple sequential scopes each pair `begin` + `end` exactly
    /// once — pins the contract that the per-letter `spawn_blocking`
    /// closures in the USN refresh tick can each enter + leave
    /// background mode independently without sharing state.
    #[test]
    fn background_io_scope_pairs_each_invocation() {
        let priority = Arc::new(CountingBackgroundIoPriority::new());

        for expected in 1_usize..=5_usize {
            let _scope =
                BackgroundIoScope::enter(Arc::clone(&priority) as Arc<dyn BackgroundIoPriority>);
            assert_eq!(priority.begins(), expected);
            // _scope drops here, calling end()
        }

        assert_eq!(priority.begins(), 5);
        assert_eq!(priority.ends(), 5);
    }
}
