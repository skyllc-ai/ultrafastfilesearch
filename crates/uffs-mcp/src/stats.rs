// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! MCP server runtime statistics.
//!
//! Lightweight, lock-free counters shared across MCP sessions (HTTP gateway)
//! or owned by a single session (stdio).  All updates use [`Ordering::Relaxed`]
//! — exact cross-thread consistency is not required for diagnostics.

use core::sync::atomic::{AtomicU64, Ordering};

/// Runtime statistics for the MCP server layer.
///
/// All fields are [`AtomicU64`] for lock-free concurrent updates.
/// An `Arc<McpStats>` is shared across HTTP sessions; in stdio mode
/// a single session owns its own instance.
#[derive(Debug, Default)]
pub struct McpStats {
    /// Total tool calls dispatched (successful + failed).
    pub total_tool_calls: AtomicU64,
    /// Total tool calls that returned an error.
    pub total_tool_errors: AtomicU64,
    /// Cumulative tool call latency in microseconds.
    pub total_tool_latency_us: AtomicU64,
    /// Total resource reads dispatched.
    pub total_resource_reads: AtomicU64,
    /// Total prompt gets dispatched.
    pub total_prompt_gets: AtomicU64,
    /// Currently active sessions (HTTP only — always 1 for stdio).
    pub active_sessions: AtomicU64,
    /// Total sessions created since server start.
    pub total_sessions: AtomicU64,

    // ── Per-tool counters ──────────────────────────────────────────
    /// `uffs_search` call count.
    pub tool_search: AtomicU64,
    /// `uffs_drives` call count.
    pub tool_drives: AtomicU64,
    /// `uffs_status` call count.
    pub tool_status: AtomicU64,
    /// `uffs_info` call count.
    pub tool_info: AtomicU64,
    /// `uffs_aggregate` call count.
    pub tool_aggregate: AtomicU64,
    /// `uffs_facet_values` call count.
    pub tool_facet_values: AtomicU64,
}

impl McpStats {
    /// Record a successful tool call.
    pub(crate) fn record_tool_call(&self, tool_name: &str, latency_us: u64) {
        self.total_tool_calls.fetch_add(1, Ordering::Relaxed);
        self.total_tool_latency_us
            .fetch_add(latency_us, Ordering::Relaxed);
        self.increment_tool_counter(tool_name);
    }

    /// Record a failed tool call.
    pub(crate) fn record_tool_error(&self, tool_name: &str, latency_us: u64) {
        self.total_tool_calls.fetch_add(1, Ordering::Relaxed);
        self.total_tool_errors.fetch_add(1, Ordering::Relaxed);
        self.total_tool_latency_us
            .fetch_add(latency_us, Ordering::Relaxed);
        self.increment_tool_counter(tool_name);
    }

    /// Record a resource read.
    pub(crate) fn record_resource_read(&self) {
        self.total_resource_reads.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a prompt get.
    pub(crate) fn record_prompt_get(&self) {
        self.total_prompt_gets.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment session counters (HTTP gateway — session created).
    pub(crate) fn session_started(&self) {
        self.active_sessions.fetch_add(1, Ordering::Relaxed);
        self.total_sessions.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement active session counter (HTTP gateway — session ended).
    pub(crate) fn session_ended(&self) {
        self.active_sessions.fetch_sub(1, Ordering::Relaxed);
    }

    /// Average tool call latency in microseconds (0 if no calls).
    ///
    /// Only consumed by the HTTP `/status` endpoint, hence the
    /// `streamable-http` feature gate.
    #[cfg(feature = "streamable-http")]
    #[must_use]
    pub(crate) fn avg_tool_latency_us(&self) -> u64 {
        let total = self.total_tool_calls.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        self.total_tool_latency_us.load(Ordering::Relaxed) / total
    }

    /// Serialize stats to a JSON value for the `/status` endpoint.
    ///
    /// Only consumed by the HTTP `/status` endpoint, hence the
    /// `streamable-http` feature gate.
    #[cfg(feature = "streamable-http")]
    #[must_use]
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tool_calls": self.total_tool_calls.load(Ordering::Relaxed),
            "tool_errors": self.total_tool_errors.load(Ordering::Relaxed),
            "avg_tool_latency_us": self.avg_tool_latency_us(),
            "resource_reads": self.total_resource_reads.load(Ordering::Relaxed),
            "prompt_gets": self.total_prompt_gets.load(Ordering::Relaxed),
            "active_sessions": self.active_sessions.load(Ordering::Relaxed),
            "total_sessions": self.total_sessions.load(Ordering::Relaxed),
            "tools": {
                "search": self.tool_search.load(Ordering::Relaxed),
                "drives": self.tool_drives.load(Ordering::Relaxed),
                "status": self.tool_status.load(Ordering::Relaxed),
                "info": self.tool_info.load(Ordering::Relaxed),
                "aggregate": self.tool_aggregate.load(Ordering::Relaxed),
                "facet_values": self.tool_facet_values.load(Ordering::Relaxed),
            }
        })
    }

    /// Increment per-tool counter by name.
    fn increment_tool_counter(&self, tool_name: &str) {
        match tool_name {
            "uffs_search" => self.tool_search.fetch_add(1, Ordering::Relaxed),
            "uffs_drives" => self.tool_drives.fetch_add(1, Ordering::Relaxed),
            "uffs_status" => self.tool_status.fetch_add(1, Ordering::Relaxed),
            "uffs_info" => self.tool_info.fetch_add(1, Ordering::Relaxed),
            "uffs_aggregate" => self.tool_aggregate.fetch_add(1, Ordering::Relaxed),
            "uffs_facet_values" => self.tool_facet_values.fetch_add(1, Ordering::Relaxed),
            _ => 0, // Unknown tool — don't crash, just skip.
        };
    }
}
