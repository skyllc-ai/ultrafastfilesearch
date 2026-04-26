// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Process-level memory and runtime telemetry.
//!
//! Phase 0 of the memory-tiering work
//! ([`docs/refactor/memory-tiering-implementation-plan.md`]).
//!
//! Provides:
//!
//! * [`mem_snapshot`] — a cross-platform helper that returns the daemon's
//!   current resident-set size and mimalloc-committed bytes.
//! * [`spawn_mem_snapshot_task`] — spawns a background tokio task that logs
//!   that snapshot at a configurable interval as a `mem.snapshot` tracing event
//!   so a long-running daemon produces the time-series the tiering work needs
//!   to measure its impact.
//!
//! The numbers come from mimalloc's `mi_process_info`, which is
//! implemented uniformly on Mac, Linux and Windows; this lets the
//! daemon emit consistent telemetry without pulling in `sysinfo`,
//! `psapi` or platform-specific `mach`/`procfs` crates.
//!
//! [`docs/refactor/memory-tiering-implementation-plan.md`]: https://github.com/skyllc-ai/UltraFastFileSearch/blob/main/docs/refactor/memory-tiering-implementation-plan.md

use alloc::sync::Arc;
use core::time::Duration;

use crate::index::IndexManager;

/// Default cadence for the background `mem.snapshot` tracing event.
///
/// One hour is enough granularity for a 24 h soak run while staying
/// well below any reasonable log-volume budget.
pub(crate) const DEFAULT_MEM_SNAPSHOT_INTERVAL: Duration = Duration::from_hours(1);

/// Process-level memory snapshot in bytes.
///
/// Snapshot is a single point-in-time read; no smoothing, no peak
/// tracking.  Consumers that want trend lines layer EMA / max-window
/// logic on top.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MemSnapshot {
    /// Process resident-set size (RSS) in bytes.
    ///
    /// What the OS reports as the process's committed-and-paged-in
    /// memory.  Includes mimalloc's working pages, stack, executable
    /// images and any non-allocator mappings.
    pub(crate) rss_bytes: u64,
    /// Mimalloc allocator committed bytes (`current_commit`).
    ///
    /// What the allocator has actually paged in from the OS.  After
    /// `mi_collect(true)` this number drops as freed segments are
    /// decommitted; comparing it to `rss_bytes` shows how much of the
    /// daemon's RSS is allocator-managed versus everything else.
    pub(crate) mimalloc_committed_bytes: u64,
}

/// Read the daemon's current memory snapshot.
///
/// Cross-platform via mimalloc's `mi_process_info` (enabled by the
/// `extended` feature on `libmimalloc-sys`, already pinned in
/// `crates/uffs-daemon/Cargo.toml`).
///
/// Returns `None` if `mi_process_info` reports zero for both RSS and
/// committed bytes — that should not happen on Mac, Linux or Windows
/// but it's the right "telemetry unavailable" signal if the function
/// is ever stubbed out for a future platform.
#[must_use]
pub(crate) fn mem_snapshot() -> Option<MemSnapshot> {
    // Eight `usize` slots, one per `mi_process_info` out-pointer.
    // Only `current_rss` and `current_commit` are surfaced today; the
    // rest are filled to satisfy the FFI contract and ignored.
    let mut elapsed_msecs: usize = 0;
    let mut user_msecs: usize = 0;
    let mut system_msecs: usize = 0;
    let mut current_rss: usize = 0;
    let mut peak_rss: usize = 0;
    let mut current_commit: usize = 0;
    let mut peak_commit: usize = 0;
    let mut page_faults: usize = 0;

    // SAFETY: `mi_process_info` reads internal mimalloc counters and
    // OS process info into the eight stack-local `usize` slots.  Each
    // pointer is exclusive to a local binding (no aliasing); the
    // function is documented as thread-safe; mimalloc is initialised
    // because it is the global allocator (`crates/uffs-daemon/src/main.rs`).
    #[expect(unsafe_code, reason = "FFI to libmimalloc-sys::mi_process_info")]
    unsafe {
        libmimalloc_sys::mi_process_info(
            &raw mut elapsed_msecs,
            &raw mut user_msecs,
            &raw mut system_msecs,
            &raw mut current_rss,
            &raw mut peak_rss,
            &raw mut current_commit,
            &raw mut peak_commit,
            &raw mut page_faults,
        );
    };

    // `usize as u64` is loss-free on every platform UFFS supports
    // (64-bit no-op; widening on hypothetical 32-bit) so no bound
    // check is needed.
    let snap = MemSnapshot {
        rss_bytes: current_rss as u64,
        mimalloc_committed_bytes: current_commit as u64,
    };

    if snap.rss_bytes == 0 && snap.mimalloc_committed_bytes == 0 {
        None
    } else {
        Some(snap)
    }
}

/// Spawn the background `mem.snapshot` emitter.
///
/// On every tick the task asks `IndexManager` for the logical heap
/// total (sum of `DriveCompactIndex::heap_size_bytes()` across all
/// loaded drives), reads `mem_snapshot()`, and emits a single
/// `tracing::info!` event with target `mem.snapshot` carrying:
///
/// * `logical_heap_bytes` — sum of per-drive logical heap sizes
/// * `mimalloc_committed_bytes` — what mimalloc has paged in
/// * `rss_bytes` — process RSS
///
/// The tracing event taxonomy is documented in
/// `docs/refactor/memory-tiering-implementation-plan.md` §4.2.
///
/// The returned `JoinHandle` is held by the caller for orderly
/// cancellation; in the daemon it is dropped at process exit, the
/// runtime tears the task down with everything else.
pub(crate) fn spawn_mem_snapshot_task(
    idx: Arc<IndexManager>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // The first tick fires immediately so we capture a baseline as
        // soon as the daemon is up; subsequent ticks fire on the
        // configured cadence.
        loop {
            ticker.tick().await;
            let logical_heap = idx.total_index_heap_bytes().await;
            if let Some(snap) = mem_snapshot() {
                tracing::info!(
                    target: "mem.snapshot",
                    logical_heap_bytes = logical_heap,
                    mimalloc_committed_bytes = snap.mimalloc_committed_bytes,
                    rss_bytes = snap.rss_bytes,
                    "memory snapshot",
                );
            } else {
                // `mi_process_info` returned all zeros — log a debug
                // line so the absence is visible without spamming
                // info-level output.
                tracing::debug!(
                    target: "mem.snapshot",
                    logical_heap_bytes = logical_heap,
                    "memory snapshot (mimalloc reports zero)",
                );
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mem_snapshot()` returns `Some` with non-zero RSS when called
    /// from a live process.  Covers Mac / Linux / Windows since
    /// `mi_process_info` is implemented on all three.
    #[test]
    fn mem_snapshot_returns_some_with_nonzero_rss() {
        let snap = mem_snapshot().expect("mimalloc returns a snapshot");
        assert!(
            snap.rss_bytes > 0,
            "RSS must be positive in a live test process; got {}",
            snap.rss_bytes
        );
        // committed_bytes is allowed to be zero on a very cold start,
        // but the upper bound rules out an underflow / sign-bit bug.
        assert!(
            snap.mimalloc_committed_bytes < u64::MAX / 2,
            "committed_bytes looks like an underflow: {}",
            snap.mimalloc_committed_bytes
        );
    }

    /// `MemSnapshot` defaults are zero on every field — used as the
    /// "telemetry unavailable" sentinel value.
    #[test]
    fn mem_snapshot_default_is_zero() {
        let zero = MemSnapshot::default();
        assert_eq!(zero.rss_bytes, 0);
        assert_eq!(zero.mimalloc_committed_bytes, 0);
    }

    /// `MemSnapshot` is `Copy`, so callers can hand it around without
    /// thinking about ownership; locking that in regression-test form.
    #[test]
    fn mem_snapshot_is_copy() {
        let original = MemSnapshot {
            rss_bytes: 1,
            mimalloc_committed_bytes: 2,
        };
        let copied = original;
        // Both reads are valid because of `Copy`.
        assert_eq!(original.rss_bytes, 1);
        assert_eq!(copied.rss_bytes, 1);
    }
}
