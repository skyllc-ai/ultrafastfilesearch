// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Output schemas for MCP tool `outputSchema` and `structuredContent`.
//!
//! Each struct derives [`serde::Serialize`] and [`schemars::JsonSchema`] so
//! it can be used with [`rmcp::model::Tool::with_output_schema`] and
//! serialized into [`rmcp::model::CallToolResult::structured_content`].

use schemars::JsonSchema;
use serde::Serialize;

// ── uffs_search ─────────────────────────────────────────────────────

/// Structured output for `uffs_search`.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SearchOutput {
    /// Number of matching rows returned in this page.
    pub returned: usize,
    /// Total matching records (before limit/pagination).
    pub total_count: u64,
    /// Total records scanned across all drives.
    pub records_scanned: usize,
    /// Query execution time in milliseconds.
    pub duration_ms: u64,
    /// Whether more results exist beyond this page.
    pub truncated: bool,
    /// Opaque cursor for fetching the next page (null when no more pages).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub next_cursor: Option<String>,
    /// Warnings about adjusted parameters (e.g. limit was capped).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub warnings: Vec<String>,
    /// Matching file/directory rows.
    pub rows: Vec<SearchRowOutput>,
}

/// A single search result row (structured).
///
/// Mirrors every field from [`uffs_client::protocol::response::SearchRow`] so
/// `structuredContent` exposes 100% of the data the CLI/API returns.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SearchRowOutput {
    /// Drive letter.
    pub drive: char,
    /// Filename.
    pub name: String,
    /// File extension (lowercase, without leading dot). Empty for directories
    /// and files without an extension.
    pub ext: String,
    /// Entry type: `"file"` or `"dir"`.
    pub r#type: String,
    /// File size in bytes.
    pub size: u64,
    /// Allocated size on disk in bytes.
    pub allocated: u64,
    /// Last modified time (Unix microseconds).
    pub modified: i64,
    /// Creation time (Unix microseconds).
    pub created: i64,
    /// Last access time (Unix microseconds).
    pub accessed: i64,
    /// Raw NTFS `FILE_ATTRIBUTE_*` flags.
    pub flags: u32,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// Descendant count (directories only, 0 for files).
    pub descendants: u32,
    /// Sum of logical file sizes in entire subtree (directories only).
    pub treesize: u64,
    /// Sum of allocated sizes in entire subtree (directories only).
    pub tree_allocated: u64,
    /// Full resolved path.
    pub path: String,
}

// ── uffs_info ───────────────────────────────────────────────────────

/// Structured output for `uffs_info`.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct InfoOutput {
    /// Whether the path was found in the index.
    pub found: bool,
    /// Detailed file record (all NTFS columns).
    /// Null when `found` is false.
    #[schemars(default)]
    pub record: Option<serde_json::Value>,
}

// ── uffs_drives ─────────────────────────────────────────────────────

/// Structured output for `uffs_drives`.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct DrivesOutput {
    /// Number of loaded drives.
    pub count: usize,
    /// Per-drive details.
    pub drives: Vec<DriveOutput>,
}

/// A single drive entry (structured).
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct DriveOutput {
    /// Drive letter (e.g. 'C').
    pub letter: char,
    /// Number of records in the compact index.
    pub records: usize,
    /// Data source (`"cache"`, `"live"`, `"mft_file"`).
    pub source: String,
}

// ── uffs_status ─────────────────────────────────────────────────────

/// Structured output for `uffs_status`.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct StatusOutput {
    /// Current daemon status object.
    pub status: serde_json::Value,
    /// Daemon uptime in seconds.
    pub uptime_secs: u64,
    /// Number of active connections.
    pub connections: usize,
    /// Daemon process ID.
    pub pid: u32,
}

// ── uffs_aggregate ──────────────────────────────────────────────────

/// Structured output for `uffs_aggregate`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AggregateOutput {
    /// Total records scanned.
    pub records_scanned: usize,
    /// Query execution time in milliseconds.
    pub duration_ms: u64,
    /// Aggregation result buckets (raw daemon wire format).
    pub aggregations: serde_json::Value,
    /// Opaque cursor for fetching the next page of buckets (null when no
    /// more pages).  Only present when `page_size` was set in the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub next_cursor: Option<String>,
}

// ── uffs_facet_values ───────────────────────────────────────────────

/// Structured output for `uffs_facet_values`.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct FacetValuesOutput {
    /// The field that was faceted.
    pub field: String,
    /// Aggregation result buckets.
    pub aggregations: serde_json::Value,
    /// Opaque cursor for fetching the next page of facet values (null when no
    /// more pages).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub next_cursor: Option<String>,
}
