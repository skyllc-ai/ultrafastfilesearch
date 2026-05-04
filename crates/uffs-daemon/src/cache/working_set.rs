// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Process-level working-set trim hook (Phase 5 task 5.1).
//!
//! After a demote we want the kernel to reclaim pages the daemon no
//! longer needs.  On Windows that's a single `EmptyWorkingSet` call —
//! the OS scans the process's working set and trims everything that
//! isn't pinned, then queues writebacks for any dirty pages.  On
//! macOS / Linux there's no process-level equivalent that respects
//! mmap'd file backing (Linux has `madvise(MADV_DONTNEED)`, but it's
//! per-region and the demote path already releases those Arcs); the
//! kernel's own page-reclaim runs the show.  So Mac/Linux ship a
//! no-op stub and the trait is purely a Windows hook.
//!
//! The trait is held by [`crate::index::IndexManager`] as
//! `Arc<dyn WorkingSetTrim>` so production wires the platform impl
//! and the Phase 5 unit tests inject a counting fake (see
//! [`tests::CountingWorkingSetTrim`]) to assert the hook fires
//! exactly once per demote batch.

use std::io;

/// Process-level working-set trim hook.
///
/// Implementations are held as `Arc<dyn WorkingSetTrim>` on
/// [`crate::index::IndexManager`].  Called at the end of every
/// demote batch in [`crate::index::IndexManager::demote_idle_shards`]
/// (Phase 5 task 5.4).  The call is process-wide, not per-shard:
/// on Windows `EmptyWorkingSet` operates on the current process,
/// so coalescing all demotes in a batch into a single trim
/// avoids redundant syscalls and pager work.
///
/// Implementors must be `Send + Sync + 'static` to satisfy the
/// `Arc<dyn ...>` bound; the function is `&self` so concurrent
/// calls from the demote-controller and the (future) pressure
/// subscriber are both safe.
pub(crate) trait WorkingSetTrim: Send + Sync + 'static {
    /// Trim the daemon's working set.  Best-effort; any I/O error
    /// is logged at the call site and the daemon continues.  On
    /// Mac/Linux this returns `Ok(())` immediately (no-op).
    fn trim(&self) -> io::Result<()>;
}

/// Production working-set trim implementation.
///
/// On Windows: thin wrapper around the Win32 `EmptyWorkingSet`
/// API.  On Mac/Linux: no-op (the per-region `madvise` calls the
/// demote path's `Arc::drop` triggers handle reclaim; there's no
/// process-level equivalent that's safe to call mid-flight).
///
/// Phase 5 task 5.1 — paired with the Phase-5 dogfood gate
/// "working set drops in Task Manager within 5 s of demote".
pub(crate) struct PlatformWorkingSetTrim;

impl WorkingSetTrim for PlatformWorkingSetTrim {
    #[cfg(target_os = "windows")]
    fn trim(&self) -> io::Result<()> {
        use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows::Win32::System::Threading::GetCurrentProcess;

        #[expect(
            unsafe_code,
            reason = "GetCurrentProcess Win32 FFI returning a process pseudo-handle"
        )]
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle that is
        // always valid for the lifetime of the process; the
        // pseudo-handle does not need closing.
        let process = unsafe { GetCurrentProcess() };
        #[expect(
            unsafe_code,
            reason = "EmptyWorkingSet Win32 FFI on the current process's pseudo-handle"
        )]
        // SAFETY: `process` was just obtained from `GetCurrentProcess`
        // above and is valid for the process lifetime.  The call is
        // sync, takes no ownership, and the windows-rs `Result<()>`
        // already wraps the underlying `BOOL` / `GetLastError`
        // protocol.  We translate any windows-rs error into
        // `io::Error::other` so the demote controller's
        // `tracing::warn!` line stays platform-agnostic.
        let result = unsafe { EmptyWorkingSet(process) };
        result.map_err(|err| io::Error::other(err.to_string()))
    }

    #[cfg(not(target_os = "windows"))]
    fn trim(&self) -> io::Result<()> {
        // Mac/Linux: no-op stub.  See module docs for rationale.
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::WorkingSetTrim;

    /// Phase 5 task 5.8 fake.  Counts every `trim()` invocation so
    /// the demote-batch test can assert the hook fires exactly once
    /// per batch (not once per shard, not zero).
    pub(crate) struct CountingWorkingSetTrim {
        calls: AtomicUsize,
    }

    impl CountingWorkingSetTrim {
        pub(crate) const fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }

        pub(crate) fn calls(&self) -> usize {
            self.calls.load(Ordering::Acquire)
        }
    }

    impl WorkingSetTrim for CountingWorkingSetTrim {
        fn trim(&self) -> std::io::Result<()> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    /// Smoke-test the production no-op stub on Mac/Linux: returns
    /// `Ok(())` immediately, no panic, no I/O.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn platform_working_set_trim_is_noop_on_unix() {
        let trim = super::PlatformWorkingSetTrim;
        for _ in 0_i32..32_i32 {
            trim.trim().expect("Mac/Linux trim() never errors");
        }
    }

    /// Counting fake increments exactly once per call and returns
    /// `Ok(())`.  Pinned here (not in `index/tests.rs`) so the
    /// fake's contract is colocated with its definition.
    #[test]
    fn counting_working_set_trim_increments_atomically() {
        let trim = CountingWorkingSetTrim::new();
        assert_eq!(trim.calls(), 0);

        for expected in 1..=5 {
            trim.trim().expect("counting trim never errors");
            assert_eq!(trim.calls(), expected);
        }
    }
}
