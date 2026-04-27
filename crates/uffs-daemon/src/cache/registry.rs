// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! [`ShardRegistry`] — top-level container that replaces the old
//! `Arc<DriveIndex>` field on [`crate::index::IndexManager`].
//!
//! See [`crate::cache`] module docs for the bigger picture.

use alloc::sync::Arc;

use uffs_core::compact::DriveCompactIndex;
use uffs_core::search::backend::DriveIndex;

use super::shard::{ShardEntry, ShardState};

/// Registry of per-drive shards plus a cached `Arc<DriveIndex>` over
/// the active subset (Warm/Hot tiers).
///
/// The cached `active_index` makes [`Self::active_index`] an
/// `Arc::clone` — search dispatch reads it under the daemon's
/// `RwLock<Arc<ShardRegistry>>` and never builds a `Vec` per query.
/// Mutations (`add` / `replace` / `remove`) return a *new* registry
/// with the rebuilt active subset; the daemon swaps the outer `Arc`.
///
/// Phase 1 keeps every shard in [`ShardState::Warm`] so the active
/// subset always equals the full registry.  Phase 3 starts demoting,
/// at which point the active subset shrinks below the full shard list.
pub(crate) struct ShardRegistry {
    /// Every loaded shard, in load order.  `Arc<ShardEntry>` so a
    /// registry rebuild is a Vec clone of pointers, not bodies.
    shards: Vec<Arc<ShardEntry>>,
    /// Pre-computed `DriveIndex` over shards in [`ShardState::Warm`]
    /// or [`ShardState::Hot`].  Cheap to clone for search dispatch.
    active_index: Arc<DriveIndex>,
}

impl ShardRegistry {
    /// Empty registry — no shards, empty active index.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            shards: Vec::new(),
            active_index: Arc::new(DriveIndex::new()),
        }
    }

    /// Build a registry from an explicit shard vector, recomputing the
    /// active subset.
    ///
    /// Used internally by [`Self::add`], [`Self::replace`], and
    /// [`Self::remove`]; exposed for tests that want to seed a
    /// registry directly.
    #[must_use]
    pub(crate) fn from_shards(shards: Vec<Arc<ShardEntry>>) -> Self {
        let drives: Vec<Arc<DriveCompactIndex>> = shards
            .iter()
            .filter(|shard| matches!(shard.state(), ShardState::Warm | ShardState::Hot))
            .map(|shard| shard.body())
            .collect();
        Self {
            shards,
            active_index: Arc::new(DriveIndex { drives }),
        }
    }

    /// Insert a fresh `Warm` shard for `body.letter` and return the
    /// rebuilt registry.  The previous registry is left untouched.
    ///
    /// The shard's identity is `body.letter` so callers don't have to
    /// thread the letter separately and can't accidentally store a
    /// shard whose letter disagrees with its body.
    #[must_use]
    pub(crate) fn add(&self, body: Arc<DriveCompactIndex>) -> Self {
        let letter = body.letter;
        let mut shards = self.shards.clone();
        shards.push(Arc::new(ShardEntry::new_warm(letter, body)));
        Self::from_shards(shards)
    }

    /// Replace the shard whose drive letter case-insensitively
    /// matches `match_letter` (if any) with a fresh `Warm` entry, and
    /// return the rebuilt registry.
    ///
    /// The new shard's identity is `body.letter` (canonical case from
    /// the index payload), preserving the pre-Phase-1 contract where
    /// `DriveIndex { drives: vec![Arc::new(new_drive)] }` always
    /// identified the new entry by `new_drive.letter`.  When no
    /// existing shard matches `match_letter`, this is equivalent to
    /// [`Self::add`].
    ///
    /// The match is **case-insensitive** to preserve the behavior of
    /// the pre-Phase-1 `IndexManager::replace_drive`, which used
    /// `eq_ignore_ascii_case` to handle drive letters that flow
    /// through the daemon in either case.
    #[must_use]
    pub(crate) fn replace(&self, match_letter: char, body: Arc<DriveCompactIndex>) -> Self {
        let new_letter = body.letter;
        let mut shards: Vec<Arc<ShardEntry>> = self
            .shards
            .iter()
            .filter(|shard| !shard.drive.eq_ignore_ascii_case(&match_letter))
            .cloned()
            .collect();
        shards.push(Arc::new(ShardEntry::new_warm(new_letter, body)));
        Self::from_shards(shards)
    }

    /// Drop the shard for `drive` (if any) and return the rebuilt
    /// registry.  No-op when `drive` is not in the registry.
    ///
    /// Match is case-insensitive — see [`Self::replace`] for the
    /// rationale.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 3 consumer (tier-transition cleanup); exercised \
                      by `cache::registry::tests::remove_missing_is_noop` \
                      under `cfg(test)`."
        )
    )]
    #[must_use]
    pub(crate) fn remove(&self, drive: char) -> Self {
        let shards: Vec<Arc<ShardEntry>> = self
            .shards
            .iter()
            .filter(|shard| !shard.drive.eq_ignore_ascii_case(&drive))
            .cloned()
            .collect();
        Self::from_shards(shards)
    }

    /// Iterate over every shard in load order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Arc<ShardEntry>> {
        self.shards.iter()
    }

    /// Snapshot of the active (`Warm`/`Hot`) subset as an
    /// `Arc<DriveIndex>` for the search backend.  Cheap clone.
    #[must_use]
    pub(crate) fn active_index(&self) -> Arc<DriveIndex> {
        Arc::clone(&self.active_index)
    }

    /// Total number of loaded shards (active + demoted).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Phase 3 consumer (status renderer with active vs demoted \
                      shard counts); exercised by registry tests under \
                      `cfg(test)`."
        )
    )]
    #[must_use]
    pub(crate) const fn len(&self) -> usize {
        self.shards.len()
    }

    /// `true` iff the registry has no shards at all.
    #[must_use]
    pub(crate) const fn is_empty(&self) -> bool {
        self.shards.is_empty()
    }

    /// `true` iff any shard exists for `drive` (regardless of tier).
    #[must_use]
    pub(crate) fn contains(&self, drive: char) -> bool {
        self.shards.iter().any(|shard| shard.drive == drive)
    }

    /// Drive letters of every loaded shard in load order.
    #[must_use]
    pub(crate) fn loaded_letters(&self) -> Vec<char> {
        self.shards.iter().map(|shard| shard.drive).collect()
    }
}

impl Default for ShardRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::min_ident_chars,
        reason = "test code — short names like `a`/`b` are clearer than \
                  long names in equality assertions."
    )]

    use super::*;

    /// Empty registry has zero shards and an empty active index.
    #[test]
    fn empty_registry_is_empty() {
        let reg = ShardRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert_eq!(reg.active_index().drives.len(), 0);
        assert_eq!(reg.loaded_letters(), Vec::<char>::new());
        assert!(!reg.contains('C'));
    }

    /// `Default` matches `new()`.
    #[test]
    fn default_matches_new() {
        let a = ShardRegistry::default();
        let b = ShardRegistry::new();
        assert_eq!(a.len(), b.len());
        assert_eq!(a.is_empty(), b.is_empty());
    }

    /// `from_shards(empty)` produces the empty registry.
    #[test]
    fn from_shards_empty_is_empty() {
        let reg = ShardRegistry::from_shards(Vec::new());
        assert!(reg.is_empty());
        assert_eq!(reg.active_index().drives.len(), 0);
    }

    /// `remove` on a missing letter is a no-op that still produces a
    /// valid empty registry — pins the no-op contract that
    /// `IndexManager::replace_drive` relies on for fresh inserts.
    #[test]
    fn remove_missing_is_noop() {
        let reg = ShardRegistry::new().remove('Z');
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert_eq!(reg.active_index().drives.len(), 0);
    }

    // Tests that exercise mutation paths with real `DriveCompactIndex`
    // bodies live in `crate::index::tests` next to the existing
    // `build_test_drive` helper — see
    // `shard_registry_add_replace_remove_round_trip` and
    // `shard_registry_records_query_per_search`.
}
