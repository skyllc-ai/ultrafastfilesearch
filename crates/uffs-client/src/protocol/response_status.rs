// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Daemon-state wire types: `drives`, `status`, and `stats` RPC responses.
//!
//! Split out of [`super::response`] so the search/payload subset of the
//! protocol surface stays under the workspace 800-LOC policy ceiling
//! without a file-size exemption.  Functionally these types form one
//! cohesive group — they describe the *daemon's own state* (loaded
//! drives, runtime telemetry, lifecycle phase, perf counters) — as
//! opposed to query results, which live alongside `SearchResponse` in
//! `response.rs`.
//!
//! The original module re-exports each type via `pub use`, so external
//! callers continue to import from `uffs_client::protocol::response::*`
//! exactly as before — the split is internal layout, not a breaking
//! change to the public protocol surface.

use serde::{Deserialize, Serialize};

/// Response for the `drives` method.
#[derive(Debug, Serialize, Deserialize)]
pub struct DrivesResponse {
    /// Loaded drives with record counts.
    pub drives: Vec<DriveInfo>,
}

/// Information about a loaded drive.
#[derive(Debug, Serialize, Deserialize)]
pub struct DriveInfo {
    /// Drive letter.
    pub letter: char,
    /// Number of records in the compact index.
    pub records: usize,
    /// Source (e.g. `"cache"`, `"live"`, `"mft_file"`).
    pub source: String,
}

/// Response for the `status` method.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Current daemon status.
    pub status: DaemonStatus,
    /// Daemon uptime in seconds.
    pub uptime_secs: u64,
    /// Number of active connections.
    pub connections: usize,
    /// Daemon process ID.
    pub pid: u32,
    /// Compile-time version of the daemon binary (`env!("CARGO_PKG_VERSION")`).
    ///
    /// Surfaced to the CLI so `uffs daemon status` / `uffs status` can
    /// flag a daemon-vs-CLI version mismatch when the user has an old
    /// long-running daemon and an upgraded CLI binary (or vice versa).
    /// Defaults to `""` for back-compat with pre-0.5.79 daemons that
    /// did not populate this field; the CLI renders that as `<unknown>`.
    #[serde(default)]
    pub version: String,
    /// Process RSS (resident set size) in bytes, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    /// Calculated heap footprint of all loaded indices (bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_heap_bytes: Option<u64>,
    /// Mimalloc allocator committed bytes (bytes paged in from the OS).
    ///
    /// Reported by `mi_process_info`; equals or exceeds `index_heap_bytes`
    /// because the allocator carries page-level overhead and free-but-
    /// not-yet-decommitted segments.  Comparing this to `rss_bytes` shows
    /// how much of the daemon's RSS is allocator-managed.  Phase 0 of the
    /// memory-tiering work surfaces this so subsequent phases can be
    /// measured against a stable allocator-committed baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimalloc_committed_bytes: Option<u64>,
    /// Per-drive memory breakdown (drive letter → heap bytes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drive_memory: Vec<DriveMemoryInfo>,
}

/// Per-drive memory breakdown for status reporting.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DriveMemoryInfo {
    /// Drive letter.
    pub drive: char,
    /// Number of records in this drive's index.
    pub records: usize,
    /// Calculated heap footprint in bytes.
    pub heap_bytes: u64,
    /// Breakdown: records Vec.
    pub records_bytes: u64,
    /// Breakdown: names Vec.
    pub names_bytes: u64,
    /// Breakdown: trigram index.
    pub trigram_bytes: u64,
    /// Breakdown: children index.
    pub children_bytes: u64,
    /// Breakdown: extension index.
    pub ext_index_bytes: u64,
}

/// Response for the `stats` method — daemon performance metrics.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatsResponse {
    /// Compile-time version of the daemon binary (`env!("CARGO_PKG_VERSION")`).
    ///
    /// Mirrors [`StatusResponse::version`].  Both responses include it
    /// so neither RPC has to chain to the other purely to obtain the
    /// daemon's version string.  Defaults to `""` for pre-0.5.79
    /// daemons (back-compat).
    #[serde(default)]
    pub version: String,
    /// Total search queries served since startup.
    pub total_queries: u64,
    /// Cumulative search time in microseconds.
    pub total_query_time_us: u64,
    /// Average query time in microseconds.
    pub avg_query_time_us: f64,
    /// Time from daemon start to `Ready` in milliseconds.
    pub startup_duration_ms: u64,
    /// Daemon uptime in seconds.
    pub uptime_secs: u64,
    /// Total records across all loaded drives.
    pub total_records: usize,
    /// Queries per second (over daemon lifetime).
    pub queries_per_second: f64,
    /// Aggregate-cache hit count (lifetime, since daemon start).
    ///
    /// Defaults to `0` when the daemon is older than this field
    /// (forward compatibility with pre-0.5.44 daemons).
    #[serde(default)]
    pub agg_cache_hits: u64,
    /// Aggregate-cache miss count (lifetime, includes stale/expired).
    #[serde(default)]
    pub agg_cache_misses: u64,
    /// Number of entries currently in the aggregate cache.
    #[serde(default)]
    pub agg_cache_entries: u64,
}

/// Daemon operational status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state")]
pub enum DaemonStatus {
    /// Daemon is loading indices.
    #[serde(rename = "loading")]
    Loading {
        /// Drives loaded so far.
        drives_loaded: usize,
        /// Total drives to load.
        drives_total: usize,
    },
    /// Daemon is ready to serve queries.
    #[serde(rename = "ready")]
    Ready,
    /// Daemon is refreshing one or more drives.
    #[serde(rename = "refreshing")]
    Refreshing {
        /// Drives being refreshed.
        drives: Vec<char>,
    },
}
