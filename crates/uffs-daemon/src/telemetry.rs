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

/// Sanity cap for `current_commit`: any value above this is treated
/// as evidence of a `usize` underflow in mimalloc's commit accounting.
///
/// Observed on macOS where `mi_process_info` reports values close to
/// `u64::MAX` (`2^64 − small_number`) after heavy allocation churn.
/// 1 TiB is well above any reasonable workstation total committed; a
/// daemon that genuinely commits > 1 TiB wants its own dedicated
/// telemetry pipeline anyway.
const MAX_PLAUSIBLE_COMMIT_BYTES: u64 = 1 << 40;

/// Drop implausible `current_commit` readings from `mi_process_info`.
///
/// Returns `None` when the raw value:
/// * exceeds [`MAX_PLAUSIBLE_COMMIT_BYTES`] (absolute cap), or
/// * exceeds 8× the reported RSS (relative cap, only applied when `rss_bytes >
///   0`).
///
/// Otherwise returns `Some(raw_committed)`.  The 8× ratio is generous
/// (mimalloc typically reports `committed ≈ 1.0–1.5× RSS`) so a
/// legitimate large reservation is never clamped to `None`; in
/// practice the underflow values observed are `≈ 2^64 / RSS` ≈ `10^9×`,
/// many orders of magnitude above any plausible ratio.
///
/// Extracted as a free function so it can be unit-tested without the
/// mimalloc FFI in the loop.
const fn sanity_clamp_committed(rss_bytes: u64, raw_committed: u64) -> Option<u64> {
    if raw_committed > MAX_PLAUSIBLE_COMMIT_BYTES {
        return None;
    }
    if rss_bytes > 0 && raw_committed > rss_bytes.saturating_mul(8) {
        return None;
    }
    Some(raw_committed)
}

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
    /// Mimalloc allocator committed bytes (`current_commit`), if a
    /// plausible reading is available.
    ///
    /// What the allocator has actually paged in from the OS.  After
    /// `mi_collect(true)` this number drops as freed segments are
    /// decommitted; comparing it to `rss_bytes` shows how much of the
    /// daemon's RSS is allocator-managed versus everything else.
    ///
    /// `None` when [`sanity_clamp_committed`] rejects the raw FFI
    /// reading — currently observed only on macOS where mimalloc's
    /// commit accounting can underflow under heavy allocation churn.
    pub(crate) mimalloc_committed_bytes: Option<u64>,
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
    let rss = current_rss as u64;
    let snap = MemSnapshot {
        rss_bytes: rss,
        mimalloc_committed_bytes: sanity_clamp_committed(rss, current_commit as u64),
    };

    if snap.rss_bytes == 0 && snap.mimalloc_committed_bytes.is_none() {
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
            let Some(snap) = mem_snapshot() else {
                // `mi_process_info` returned all zeros — log a debug
                // line so the absence is visible without spamming
                // info-level output.
                tracing::debug!(
                    target: "mem.snapshot",
                    logical_heap_bytes = logical_heap,
                    "memory snapshot (mimalloc reports zero)",
                );
                continue;
            };
            if let Some(committed) = snap.mimalloc_committed_bytes {
                tracing::info!(
                    target: "mem.snapshot",
                    logical_heap_bytes = logical_heap,
                    mimalloc_committed_bytes = committed,
                    rss_bytes = snap.rss_bytes,
                    "memory snapshot",
                );
            } else {
                tracing::info!(
                    target: "mem.snapshot",
                    logical_heap_bytes = logical_heap,
                    rss_bytes = snap.rss_bytes,
                    "memory snapshot (mimalloc committed unavailable)",
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
        // committed may legitimately be `None` on macOS where the
        // raw mimalloc reading underflows; if `Some`, the clamp has
        // already vetted the upper bound, but reasserting documents
        // the invariant for future readers.
        if let Some(committed) = snap.mimalloc_committed_bytes {
            assert!(
                committed < u64::MAX / 2,
                "committed_bytes looks like an underflow: {committed}"
            );
        }
    }

    /// `MemSnapshot` defaults are zero / `None` — used as the
    /// "telemetry unavailable" sentinel value.
    #[test]
    fn mem_snapshot_default_is_zero() {
        let zero = MemSnapshot::default();
        assert_eq!(zero.rss_bytes, 0);
        assert_eq!(zero.mimalloc_committed_bytes, None);
    }

    /// `MemSnapshot` is `Copy`, so callers can hand it around without
    /// thinking about ownership; locking that in regression-test form.
    #[test]
    fn mem_snapshot_is_copy() {
        let original = MemSnapshot {
            rss_bytes: 1,
            mimalloc_committed_bytes: Some(2),
        };
        let copied = original;
        // Both reads are valid because of `Copy`.
        assert_eq!(original.rss_bytes, 1);
        assert_eq!(copied.rss_bytes, 1);
    }

    /// `sanity_clamp_committed` drops the underflow value observed on
    /// macOS during the v0.5.77 baseline capture (`Mimalloc:
    /// 17592186035942 MB`).
    #[test]
    fn sanity_clamp_drops_macos_underflow() {
        // Reproduce the exact byte value rendered as `17592186035942 MB`
        // in the field report (`u64::MAX` minus a small offset — the
        // signature of a `commit_inc − commit_dec` underflow).
        let underflow = u64::MAX - 7_886_233_600;
        let mac_rss = 12_005_u64 * 1024 * 1024;
        assert_eq!(sanity_clamp_committed(mac_rss, underflow), None);
    }

    /// `sanity_clamp_committed` accepts the Windows v0.5.77 baseline
    /// reading unchanged (committed > RSS but well under the 8× cap).
    #[test]
    fn sanity_clamp_accepts_windows_baseline() {
        let rss = 5_110_u64 * 1024 * 1024;
        let committed = 5_886_u64 * 1024 * 1024;
        assert_eq!(sanity_clamp_committed(rss, committed), Some(committed));
    }

    /// `sanity_clamp_committed` enforces the absolute 1 TiB cap even
    /// when RSS is zero (cold-start path with no live process info).
    #[test]
    fn sanity_clamp_enforces_absolute_cap() {
        assert_eq!(
            sanity_clamp_committed(0, MAX_PLAUSIBLE_COMMIT_BYTES + 1),
            None
        );
        // Just under the cap with rss=0 still passes — lets the cold
        // boot path through without over-clamping.
        assert_eq!(
            sanity_clamp_committed(0, MAX_PLAUSIBLE_COMMIT_BYTES - 1),
            Some(MAX_PLAUSIBLE_COMMIT_BYTES - 1)
        );
    }

    /// `sanity_clamp_committed` enforces the relative `8 × RSS` cap so
    /// underflow values that happen to fall under 1 TiB are still
    /// rejected when RSS is also abnormal.
    #[test]
    fn sanity_clamp_enforces_relative_cap() {
        // 1 GiB RSS, 9 GiB committed — above the 8× ratio.
        let rss = 1_u64 << 30_u32;
        let committed = 9_u64 << 30_u32;
        assert_eq!(sanity_clamp_committed(rss, committed), None);
        // Same RSS, exactly 8 GiB — right at the boundary, still
        // accepted (clamp uses strict greater-than).
        let committed_at_boundary = 8_u64 << 30_u32;
        assert_eq!(
            sanity_clamp_committed(rss, committed_at_boundary),
            Some(committed_at_boundary)
        );
    }
}
