// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bounded concurrency via a counting-semaphore "slot pool".
//!
//! [`SlotPool`] hands out a fixed number of *run-tickets*.  A thread calls
//! [`SlotPool::acquire`] which blocks until a ticket is available, does its
//! work, then the returned [`SlotGuard`] automatically returns the ticket on
//! drop — waking the next waiter.
//!
//! ```rust
//! use std::sync::Arc;
//!
//! use uffs_core::SlotPool;
//!
//! let pool = Arc::new(SlotPool::new(2)); // 2 concurrent slots
//!
//! std::thread::scope(|s| {
//!     for i in 0..5 {
//!         let pool = Arc::clone(&pool);
//!         s.spawn(move || {
//!             let _ticket = pool.acquire(); // blocks until a slot opens
//!             // … do work …
//!             // ticket returned automatically when _ticket drops
//!         });
//!     }
//! });
//! ```

use std::sync::{Condvar, Mutex};

/// Recovers from a poisoned `Mutex` by extracting the inner value.
fn unpoison<T>(err: std::sync::PoisonError<T>) -> T {
    err.into_inner()
}

/// A pool of `N` run-tickets.
///
/// At most `N` threads can hold a ticket simultaneously.  Additional
/// callers block in [`acquire`](Self::acquire) until a ticket is
/// returned (either explicitly or via `SlotGuard` drop).
pub struct SlotPool {
    /// `(available, total)` — `available` is decremented on acquire,
    /// incremented on release.
    state: Mutex<(usize, usize)>,
    /// Signalled whenever a slot becomes available.
    available: Condvar,
}

impl SlotPool {
    /// Creates a pool with `slots` tickets.
    ///
    /// # Panics
    ///
    /// Panics if `slots` is 0.
    #[must_use]
    pub fn new(slots: usize) -> Self {
        assert!(slots > 0, "SlotPool requires at least 1 slot");
        Self {
            state: Mutex::new((slots, slots)),
            available: Condvar::new(),
        }
    }

    /// Creates a pool sized to the lesser of `max_slots` and the number
    /// of hardware threads (with a floor of 1).
    #[must_use]
    pub fn hardware_bounded(max_slots: usize) -> Self {
        let hw =
            std::thread::available_parallelism().map_or(max_slots, core::num::NonZeroUsize::get);
        Self::new(hw.min(max_slots).max(1))
    }

    /// Blocks until a slot is available, then returns a `SlotGuard`
    /// that will release the slot on drop.
    #[must_use]
    pub fn acquire(&self) -> SlotGuard<'_> {
        let mut guard = self.state.lock().unwrap_or_else(unpoison);
        while guard.0 == 0 {
            guard = self.available.wait(guard).unwrap_or_else(unpoison);
        }
        guard.0 -= 1;
        drop(guard);
        SlotGuard { pool: self }
    }

    /// Returns one slot to the pool and wakes a single waiter (if any).
    fn release(&self) {
        let mut guard = self.state.lock().unwrap_or_else(unpoison);
        guard.0 += 1;
        drop(guard);
        self.available.notify_one();
    }

    /// Returns the total number of slots (fixed at construction time).
    #[must_use]
    pub fn total(&self) -> usize {
        self.state.lock().unwrap_or_else(unpoison).1
    }
}

/// RAII guard returned by [`SlotPool::acquire`].
///
/// Dropping this guard returns the slot to the pool and wakes the next
/// waiting thread.
pub struct SlotGuard<'pool> {
    /// Back-reference to the owning pool.
    pool: &'pool SlotPool,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.pool.release();
    }
}

impl core::fmt::Debug for SlotPool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.state.lock().unwrap_or_else(unpoison);
        f.debug_struct("SlotPool")
            .field("available", &guard.0)
            .field("total", &guard.1)
            .finish()
    }
}

// ─── Resource-Aware Budget ──────────────────────────────────────────────────

/// Per-drive cost estimate for the budget calculator.
#[derive(Debug, Clone)]
pub struct DriveLoadEstimate {
    /// Drive letter.
    pub letter: char,
    /// Estimated peak memory in bytes for loading this drive.
    pub peak_bytes: u64,
    /// Whether a compact cache hit is expected (much cheaper).
    pub compact_cache_hit: bool,
}

/// Maximum concurrent drive loads (hard ceiling regardless of RAM).
const MAX_CONCURRENT_DRIVES: usize = 8;

/// Fraction of available RAM we're willing to use for drive loading.
const RAM_BUDGET_FRACTION: f64 = 0.60;

/// Default peak memory per drive when we can't estimate (500 MB).
const DEFAULT_PEAK_PER_DRIVE: u64 = 500 * 1024 * 1024;

/// Decompression ratio estimate (zstd level 3 → ~6× expansion).
const DECOMPRESS_RATIO: u64 = 6;

/// When `MftIndex` is loaded (cache miss), peak is `MftIndex` + compact build.
/// Compact ≈ 0.4× `MftIndex`, so peak multiplier is ~1.4.
const CACHE_MISS_PEAK_MULTIPLIER: f64 = 1.4;

/// Estimates per-drive memory cost from cache files on disk.
///
/// Checks:
/// 1. Compact cache file → if fresh, cost = file\_size × `DECOMPRESS_RATIO`
/// 2. `MftIndex` cache file → if present, cost = file\_size × ratio × 1.4
///    (transient `MftIndex` + compact build)
/// 3. Neither → conservative default
#[must_use]
pub fn estimate_drive_cost(drive_letter: char) -> DriveLoadEstimate {
    let compact_path = crate::compact_cache::compact_cache_path(drive_letter);
    let mft_path = uffs_mft::cache::cache_file_path(drive_letter);

    // Check compact cache first — if it exists and is fresh, it's a cheap hit
    if let Ok(meta) = std::fs::metadata(&compact_path) {
        let age_ok = meta
            .modified()
            .ok()
            .and_then(|mtime| mtime.elapsed().ok())
            .is_some_and(|age| age.as_secs() < crate::compact::INDEX_TTL_SECONDS);

        if age_ok {
            let peak = meta.len() * DECOMPRESS_RATIO;
            return DriveLoadEstimate {
                letter: drive_letter,
                peak_bytes: peak,
                compact_cache_hit: true,
            };
        }
    }

    // Cache miss path — check MftIndex file size for cost estimate
    if let Ok(meta) = std::fs::metadata(&mft_path) {
        let decompressed = meta.len() * DECOMPRESS_RATIO;
        #[expect(clippy::float_arithmetic, reason = "cost estimation arithmetic")]
        let peak =
            uffs_mft::f64_to_u64(uffs_mft::u64_to_f64(decompressed) * CACHE_MISS_PEAK_MULTIPLIER);
        return DriveLoadEstimate {
            letter: drive_letter,
            peak_bytes: peak,
            compact_cache_hit: false,
        };
    }

    // No cache at all — use conservative default
    DriveLoadEstimate {
        letter: drive_letter,
        peak_bytes: DEFAULT_PEAK_PER_DRIVE,
        compact_cache_hit: false,
    }
}

/// Computes the optimal number of concurrent drive-load slots based on
/// available system memory and per-drive cost estimates.
///
/// Algorithm:
/// 1. Query available system RAM
/// 2. Compute memory budget = available × 60%
/// 3. Find the most expensive (cache-miss) drive as the cost unit
/// 4. Slots = budget ÷ max\_cost, clamped to `[1, min(num_drives, 8)]`
///
/// On a 64 GB system at 10% utilisation (57 GB available):
/// - Budget = 57 × 0.60 = 34 GB
/// - If largest drive costs 2 GB → 17 slots → clamped to min(drives, 8)
/// - All drives run in parallel
///
/// On a 8 GB system at 70% utilisation (2.4 GB available):
/// - Budget = 2.4 × 0.60 = 1.4 GB
/// - If largest drive costs 1.5 GB → 0.9 → clamped to 1
/// - Drives load one at a time
#[must_use]
#[expect(
    clippy::float_arithmetic,
    reason = "Budget estimation uses approximate floating-point arithmetic by design"
)]
pub fn compute_load_budget(drives: &[char]) -> SlotPool {
    if drives.is_empty() {
        return SlotPool::new(1);
    }

    let mem = uffs_mft::query_system_memory();
    let budget =
        uffs_mft::f64_to_u64(uffs_mft::u64_to_f64(mem.available_bytes) * RAM_BUDGET_FRACTION);

    let estimates: Vec<DriveLoadEstimate> = drives
        .iter()
        .map(|&drive| estimate_drive_cost(drive))
        .collect();

    // Use the worst-case (most expensive) drive as the denominator
    let max_cost = estimates
        .iter()
        .map(|est| est.peak_bytes)
        .max()
        .unwrap_or(DEFAULT_PEAK_PER_DRIVE)
        .max(1); // avoid division by zero

    let computed = uffs_mft::frs_to_usize(budget / max_cost);
    let ceiling = drives.len().min(MAX_CONCURRENT_DRIVES);
    let slots = computed.clamp(1, ceiling);

    let gib = 1024_u64 * 1024 * 1024;
    let mib = 1024_u64 * 1024;
    tracing::info!(
        total_ram_gb = mem.total_bytes / gib,
        available_ram_gb = mem.available_bytes / gib,
        available_pct = format_args!("{:.0}%", mem.available_fraction() * 100.0_f64),
        budget_gb = budget / gib,
        max_drive_cost_mb = max_cost / mib,
        cache_hits = estimates.iter().filter(|est| est.compact_cache_hit).count(),
        cache_misses = estimates
            .iter()
            .filter(|est| !est.compact_cache_hit)
            .count(),
        slots,
        "🎫 Drive load budget computed"
    );

    SlotPool::new(slots)
}

#[cfg(test)]
#[expect(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::default_numeric_fallback,
    clippy::shadow_reuse,
    reason = "test module — relaxed linting"
)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn basic_acquire_release() {
        let pool = SlotPool::new(2);
        let guard_a = pool.acquire();
        let guard_b = pool.acquire();
        assert_eq!(pool.total(), 2);
        drop(guard_a);
        drop(guard_b);
    }

    #[test]
    fn concurrency_bounded() {
        let pool = Arc::new(SlotPool::new(2));
        let peak = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|scope| {
            for _ in 0..8 {
                let pool = Arc::clone(&pool);
                let peak = Arc::clone(&peak);
                let active = Arc::clone(&active);
                scope.spawn(move || {
                    let _ticket = pool.acquire();
                    let cur = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(cur, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });

        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak concurrency exceeded 2 slots"
        );
    }

    #[test]
    fn compute_budget_returns_at_least_one() {
        let pool = compute_load_budget(&['Z', 'Y', 'X']);
        assert!(pool.total() >= 1, "budget must be at least 1 slot");
        assert!(pool.total() <= 3, "budget can't exceed num drives");
    }

    #[test]
    fn compute_budget_empty_drives() {
        let pool = compute_load_budget(&[]);
        assert_eq!(pool.total(), 1);
    }

    #[test]
    fn estimate_unknown_drive_uses_default() {
        let est = estimate_drive_cost('Z');
        assert!(!est.compact_cache_hit);
        assert_eq!(est.peak_bytes, DEFAULT_PEAK_PER_DRIVE);
    }
}
