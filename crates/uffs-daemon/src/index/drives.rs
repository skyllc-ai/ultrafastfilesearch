// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `drives` RPC handler — enumerates every shard in the registry,
//! tagged with its memory-tiering tier marker.
//!
//! Phase 5 task 5.11 (memory-tiering implementation plan §3) split
//! this off [`super::search`] when the combined module pushed past
//! the workspace 800-LOC ceiling.  The pre-Phase-5 implementation
//! filtered through `active_index()` (Warm/Hot only); the dogfood
//! on 2026-04-28 surfaced that as a display bug — a daemon with all
//! shards demoted to `Parked` rendered `Drives: (none loaded)`
//! despite the bloom + trie still being resident.  This module's
//! [`IndexManager::drives`] walks the registry directly so every
//! tier round-trips to the CLI, with a synthetic `source` label
//! ("parked" / "cold") for body-less shards so legacy CLI versions
//! without the new `tier` field still render something readable.

use uffs_client::protocol::response::{DriveInfo, DrivesResponse, ShardTier};

use super::IndexManager;
use crate::cache::ShardState;

/// Map the daemon-internal `ShardState` to the wire-public `ShardTier`.
///
/// Bridges the `pub(crate) ShardState` (used for the atomic state
/// machine) and the `pub ShardTier` (carried over RPC).  Kept tier-
/// for-tier identical so the CLI's `[Hot]` / `[Warm]` / `[Parked]` /
/// `[Cold]` markers reflect exactly what the registry holds.
const fn shard_state_to_tier(state: ShardState) -> ShardTier {
    match state {
        ShardState::Unknown => ShardTier::Unknown,
        ShardState::Cold => ShardTier::Cold,
        ShardState::Parked => ShardTier::Parked,
        ShardState::Warm => ShardTier::Warm,
        ShardState::Hot => ShardTier::Hot,
        ShardState::Evicting => ShardTier::Evicting,
    }
}

/// Synthetic `source` label for body-less shards (`Parked` / `Cold`),
/// returned verbatim in `DriveInfo::source` so legacy CLI versions
/// without the `tier` field still render something operator-readable
/// instead of an empty string.  Used only for shards where
/// `ShardEntry::body()` is `None`.
const fn tier_source_label(tier: ShardTier) -> &'static str {
    match tier {
        ShardTier::Hot => "hot",
        ShardTier::Warm => "warm",
        ShardTier::Parked => "parked",
        ShardTier::Cold => "cold",
        ShardTier::Evicting => "evicting",
        ShardTier::Unknown => "unknown",
    }
}

/// Render a `DriveCompactIndex::source` into the human-readable
/// `DriveInfo::source` string the CLI prints inside the `()` after
/// the records count.  Live MFT reads (path is `"C:"` etc.) collapse
/// to the single word `"live"`; offline file-backed reads expose
/// the path so the operator can tell which `.iocp` snapshot the
/// daemon actually loaded.
fn describe_index_source(source: &uffs_core::compact::IndexSource) -> String {
    match source {
        uffs_core::compact::IndexSource::MftFile(mft_path) => {
            if mft_path.to_string_lossy().len() <= 2 {
                "live".to_owned()
            } else {
                format!("file:{}", mft_path.display())
            }
        }
    }
}

impl IndexManager {
    /// Get loaded drives info — every shard in the registry, tagged
    /// with its tier marker.
    ///
    /// Phase 5 task 5.11 (memory-tiering implementation plan §3)
    /// widens this from "Warm/Hot only" to "every loaded shard",
    /// driven by the 2026-04-28 dogfood: when all 7 drives demoted to
    /// `Parked`, the registry still held them (their bloom + trie are
    /// loaded, ready for re-promote on bloom hit), but the old
    /// `drives()` filtered those out via `active_index()` and the CLI
    /// rendered `Drives: (none loaded)`.
    ///
    /// Now we walk the registry directly: Warm/Hot shards keep their
    /// records count + source from `body`; Parked/Cold shards report
    /// `records: 0` and a synthetic `source` ("parked" / "cold") so
    /// callers can render the tier without poking at internal state.
    /// Every entry carries `tier: Some(_)` so older clients without
    /// the field continue to deserialize, but new clients can drive
    /// the formatter off the tier marker directly.
    pub(crate) async fn drives(&self) -> DrivesResponse {
        // Inner scope so the registry read-lock guard drops before
        // the `DrivesResponse` is constructed —
        // `clippy::significant_drop_tightening` flags the pattern
        // where a guard's scope outlives the block that actually
        // needs it, since concurrent writers (demote / promote)
        // would block longer than necessary.
        let drives: Vec<DriveInfo> = {
            let guard = self.index.read().await;
            guard
                .iter()
                .map(|shard| {
                    let tier = shard_state_to_tier(shard.state());
                    let (records, source) = shard.body().map_or_else(
                        || (0_usize, tier_source_label(tier).to_owned()),
                        |body| (body.records.len(), describe_index_source(&body.source)),
                    );
                    DriveInfo {
                        letter: shard.drive,
                        records,
                        source,
                        tier: Some(tier),
                    }
                })
                .collect()
        };
        DrivesResponse { drives }
    }
}
