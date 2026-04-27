// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Body source for the Phase 3 demote / promote orchestrator.
//!
//! Phase 3 of the memory-tiering work
//! (`docs/refactor/memory-tiering-implementation-plan.md`).
//!
//! [`IndexManager::ensure_warm_for_dispatch`] needs to materialise a
//! `DriveCompactIndex` for any `Parked` / `Cold` shard the search
//! hot path touches.  The production source is the encrypted compact
//! cache on disk
//! (`<cache_dir>/<letter>_compact.uffs`); tests inject deterministic
//! fakes (fixed body, missing body, panicking loader) without
//! touching the platform cache directory.
//!
//! Both flavours implement [`BodyLoader`].  The trait is **sync** —
//! the caller in `IndexManager` wraps every call in
//! `tokio::task::spawn_blocking` since the production loader does
//! file I/O + decryption.  Test fakes are cheap and could be sync,
//! but the same `spawn_blocking` wrapper applies uniformly to keep
//! the call site simple.
//!
//! [`IndexManager::ensure_warm_for_dispatch`]:
//!     crate::index::IndexManager::ensure_warm_for_dispatch

use alloc::sync::Arc;

use uffs_core::compact::DriveCompactIndex;

/// Source for `Parked` / `Cold` shard bodies.
///
/// Implementations are held by [`crate::index::IndexManager`] as an
/// `Arc<dyn BodyLoader>` so the manager can be constructed with the
/// production [`DiskBodyLoader`] for normal daemon operation, or
/// with a test fake for the Commit-E integration tests.
///
/// The single method is **synchronous** — the demote / promote
/// orchestrator wraps every call in
/// `tokio::task::spawn_blocking`, so implementations can do file
/// I/O, decryption, allocation, or whatever they need without
/// blocking the async runtime.  Async work inside the loader is
/// not supported and would deadlock the spawn-blocking thread
/// pool.
///
/// Returning `None` is a graceful failure: the orchestrator emits
/// a `target: "shard.transition"` warning and leaves the shard in
/// its current tier.  A panic is also recovered by the
/// `JoinError` arm in the orchestrator and emits an error-level
/// `shard.transition` event; the shard stays put.
pub(crate) trait BodyLoader: Send + Sync + 'static {
    /// Materialise the body for `letter`, or return `None` if the
    /// underlying source is missing / stale / corrupted.
    fn load(&self, letter: char) -> Option<Arc<DriveCompactIndex>>;
}

/// Production loader: reads
/// `<cache_dir>/<letter>_compact.uffs`, validates the header,
/// decrypts the encrypted body, and assembles a fresh
/// `DriveCompactIndex` (heap or runtime-mmap variant per
/// `compact_cache`'s own selection logic).
///
/// `ttl_seconds = u64::MAX` skips the freshness check — the
/// demote / promote path doesn't care if the cache is "old", only
/// that it is a valid serialisation of the drive that *was* loaded.
/// Phase 4+ may add an explicit `mft_build_epoch` check to detect
/// out-of-band MFT churn between demote and promote; for Phase 3
/// the cache is trusted unconditionally.
pub(crate) struct DiskBodyLoader;

impl BodyLoader for DiskBodyLoader {
    fn load(&self, letter: char) -> Option<Arc<DriveCompactIndex>> {
        uffs_core::compact_cache::load_compact_cache(letter, u64::MAX, 0, true).map(Arc::new)
    }
}
