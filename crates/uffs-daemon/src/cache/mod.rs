// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shard-based index cache with per-drive lifecycle.
//!
//! Phase 1 of the memory-tiering work
//! (`docs/refactor/memory-tiering-implementation-plan.md`).
//!
//! The cache layer wraps each loaded `DriveCompactIndex` in a
//! [`ShardEntry`] that carries:
//!
//! * a tier state ([`ShardState`]) — Phase 1 pins everything to
//!   [`ShardState::Warm`]; Phase 3 wires real transitions.
//! * per-drive query stats ([`DriveStats`]) — atomic counters plus an
//!   exponentially-weighted moving average rate; consumed by the adaptive-TTL
//!   formulas in Phase 6.
//! * the in-memory body, an `Arc<DriveCompactIndex>` cloned cheaply into the
//!   per-search snapshot.
//!
//! [`ShardRegistry`] is the top-level container that the daemon swaps
//! under `RwLock<Arc<...>>` in place of the old
//! `Arc<uffs_core::search::backend::DriveIndex>`.  It maintains a
//! cached `Arc<DriveIndex>` over the active (Warm/Hot) subset so the
//! search hot path stays an `Arc::clone` away from a usable backend.
//!
//! Phase 1 keeps every shard in `Warm` so the active subset always
//! matches the full registry; Phase 3 starts demoting and
//! [`ShardRegistry::active_index`] begins to diverge from the full
//! shard list.

pub(crate) mod registry;
pub(crate) mod shard;

// Phase 1 surface: only `ShardRegistry` and `ShardState` are referenced
// from outside this module (in `crate::index`).  The other types
// (`ShardEntry`, `DriveStats`, `DriveStatsSnapshot`,
// `IllegalTransition`) stay accessible via the explicit `shard::` /
// `registry::` paths so the proptest harness in `shard.rs` and the
// integration tests in `crate::index::tests` can construct them
// without going through this re-export.  Phase 3+ widens the
// re-export list as more callers join.
pub(crate) use registry::ShardRegistry;
pub(crate) use shard::ShardState;
