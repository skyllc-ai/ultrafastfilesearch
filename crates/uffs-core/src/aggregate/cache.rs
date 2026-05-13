// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Aggregate result cache.
//!
//! Caches `AggregateOutput` values keyed by a spec + filter hash and an
//! index-version token.  The first call for a given query pays the full
//! scan cost; subsequent callers within the TTL window (and against the
//! same index version) get a cheap `clone` instead of a 5-second rayon
//! fan-out over millions of records.
//!
//! Key properties:
//! - **Hit cost**: `Mutex::lock` + `HashMap::get` + `AggregateOutput::clone` —
//!   microseconds, independent of drive size.
//! - **Invalidation**: automatic when [`AggregateCache::set_index_version`] is
//!   called with a new version number (drive load / refresh) or when an entry's
//!   TTL expires.
//! - **Key scope**: opaque `u64` hash supplied by the caller.  The core library
//!   does not prescribe the hash function — callers are expected to mix in
//!   every input that affects `AggregateOutput` shape (specs, pattern, drive
//!   filter, record filter, query predicates).  The helper [`hash_specs`]
//!   computes a stable [`std::collections::hash_map::DefaultHasher`] digest
//!   over any `Hash`-friendly string; composite keys can be assembled by
//!   callers via `format!` or `std::fmt::Write`.
//!
//! Observability: [`AggregateCache::stats`] returns live hit/miss/entry
//! counters so the daemon can surface them via the `stats` RPC for tuning
//! the TTL.

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use super::AggregateOutput;

/// A time-limited aggregate cache.
///
/// Entries are keyed by a spec hash and automatically expire after a
/// configurable TTL. The cache is invalidated when the drive index
/// version changes.
#[derive(Debug)]
pub struct AggregateCache {
    /// Cache entries keyed by spec hash.
    entries: Mutex<HashMap<u64, CacheEntry>>,
    /// Time-to-live for cache entries.
    ttl: Duration,
    /// Drive index version at cache time (for invalidation).
    index_version: Mutex<u64>,
    /// Lifetime count of cache hits.
    hits: AtomicU64,
    /// Lifetime count of cache misses (includes stale/expired).
    misses: AtomicU64,
}

/// A single cache entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// The cached aggregate output (response + scan counters).
    output: AggregateOutput,
    /// When this entry was created.
    created: Instant,
    /// Drive index version when this was computed.
    index_version: u64,
}

/// Snapshot of cache performance counters.
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    /// Lifetime hit count.
    pub hits: u64,
    /// Lifetime miss count (misses, expiries, and version-stale reads).
    pub misses: u64,
    /// Entries currently in the cache.
    pub entries: usize,
}

impl AggregateCache {
    /// Create a new cache with the specified TTL.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            index_version: Mutex::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Create a cache with the default 60-second TTL.
    #[must_use]
    pub fn default_ttl() -> Self {
        Self::new(Duration::from_mins(1))
    }

    /// Set the current drive index version.
    ///
    /// When this changes, all existing cache entries are invalidated.
    pub fn set_index_version(&self, version: u64) {
        let needs_invalidation = {
            let mut current = self
                .index_version
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *current == version {
                false
            } else {
                *current = version;
                true
            }
        };
        if needs_invalidation {
            // Invalidate all entries.
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.clear();
        }
    }

    /// Look up a cached result.
    ///
    /// Returns `None` if the entry is missing, expired, or belongs
    /// to a different index version.  All three paths count as a
    /// miss in [`Self::stats`].
    #[must_use]
    pub fn get(&self, spec_hash: u64) -> Option<AggregateOutput> {
        // Snapshot current index version first (single-lock scope), then
        // examine entries (separate single-lock scope).  Tighter drops
        // and consistent lock ordering with `set_index_version`.
        let current_version = *self
            .index_version
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let cached = {
            let entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.get(&spec_hash).and_then(|entry| {
                if entry.created.elapsed() > self.ttl || entry.index_version != current_version {
                    None
                } else {
                    Some(entry.output.clone())
                }
            })
        };

        if cached.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        cached
    }

    /// Insert a result into the cache.
    ///
    /// Silently drops the entry when the cache's current index
    /// version has advanced beyond the caller's — this protects
    /// against races where a drive reload bumps the version between
    /// [`Self::get`] and [`Self::put`].
    pub fn put(&self, spec_hash: u64, output: AggregateOutput) {
        let current_version = *self
            .index_version
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = CacheEntry {
            output,
            created: Instant::now(),
            index_version: current_version,
        };
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Evict expired entries first.
        entries.retain(|_, existing| existing.created.elapsed() <= self.ttl);

        entries.insert(spec_hash, entry);
    }

    /// Clear all cached entries.
    pub fn clear(&self) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.clear();
    }

    /// Number of entries currently in cache.
    #[must_use]
    pub fn len(&self) -> usize {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot of cache performance counters.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries: self.len(),
        }
    }

    /// Reset hit/miss counters (entries preserved).
    ///
    /// Useful for on-demand diagnostic sessions that want a fresh
    /// baseline without dropping cached values.
    pub fn reset_counters(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }
}

/// Compute a hash for a set of aggregate spec labels + kinds.
///
/// This is a simple hash function for cache keying — not cryptographic.
#[must_use]
pub fn hash_specs(specs_key: &str) -> u64 {
    use core::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    specs_key.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::finalize::AggregateResponse;

    fn empty_output() -> AggregateOutput {
        AggregateOutput {
            response: AggregateResponse { results: vec![] },
            records_scanned: 0,
            records_matched: 0,
            execution_us: 0,
        }
    }

    #[test]
    fn cache_put_and_get() {
        let cache = AggregateCache::default_ttl();
        let hash = hash_specs("test_key");

        cache.put(hash, empty_output());
        let cached = cache.get(hash);
        assert!(cached.is_some());
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.entries, 1);
    }

    #[test]
    fn cache_miss_after_version_change() {
        let cache = AggregateCache::default_ttl();
        let hash = hash_specs("test_key");

        cache.put(hash, empty_output());
        cache.set_index_version(1);

        let cached = cache.get(hash);
        assert!(cached.is_none());
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn cache_miss_counter_increments_on_unknown_key() {
        let cache = AggregateCache::default_ttl();
        assert!(cache.get(hash_specs("missing")).is_none());
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);
    }

    #[test]
    fn cache_clear() {
        let cache = AggregateCache::default_ttl();
        cache.put(hash_specs("a"), empty_output());
        cache.put(hash_specs("b"), empty_output());
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn reset_counters_preserves_entries() {
        let cache = AggregateCache::default_ttl();
        let hash = hash_specs("k");
        cache.put(hash, empty_output());
        let _first: Option<AggregateOutput> = cache.get(hash);
        let _second: Option<AggregateOutput> = cache.get(hash);
        assert_eq!(cache.stats().hits, 2);

        cache.reset_counters();
        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.entries, 1);
    }
}
