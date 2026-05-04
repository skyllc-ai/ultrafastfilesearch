// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! JSON-RPC 2.0 protocol types shared between client and daemon.
//!
//! These types define the wire format for IPC communication. Both
//! `uffsd` (daemon) and `uffs` (CLI) both depend on this module.

pub mod cli_args;
mod cli_args_helpers;
pub mod response;
pub mod response_status;
pub mod response_tiering;
pub mod search_params;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

// ────────────────────────────────────────────────────────────────────────────
// JSON-RPC 2.0 envelope
// ────────────────────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Request ID (correlates request → response). `None` for notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// Method name (e.g. `"search"`, `"drives"`, `"status"`).
    pub method: String,
    /// Method parameters (JSON object or array).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 success response.
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcResponse {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Matching request ID.
    pub id: u64,
    /// Result payload (method-specific).
    pub result: serde_json::Value,
}

/// JSON-RPC 2.0 error response.
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcErrorResponse {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Matching request ID.
    pub id: Option<u64>,
    /// Error details.
    pub error: RpcError,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    /// Error code (standard JSON-RPC or application-specific).
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 notification (no `id`, no response expected).
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcNotification {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Notification method (e.g. `"drive_loaded"`, `"refresh_complete"`).
    pub method: String,
    /// Notification parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

// Standard JSON-RPC error codes
/// Parse error (invalid JSON).
pub const ERR_PARSE: i32 = -32700;
/// Invalid request (missing fields).
pub const ERR_INVALID_REQUEST: i32 = -32600;
/// Method not found.
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
/// Invalid parameters.
pub const ERR_INVALID_PARAMS: i32 = -32602;
/// Internal error.
pub const ERR_INTERNAL: i32 = -32603;

/// Canonical result filter mode for file-vs-directory selection.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchFilterMode {
    /// Return both files and directories.
    All,
    /// Return files only.
    Files,
    /// Return directories only.
    Dirs,
}

/// Canonical sort direction in the daemon wire contract.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchSortDirection {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

/// Canonical sort clause in the daemon wire contract.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SearchSortSpec {
    /// Canonical field name or accepted alias.
    pub field: String,
    /// Explicit direction. When omitted, the daemon applies the field default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<SearchSortDirection>,
}

/// Canonical predicate operator in the daemon wire contract.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchPredicateOp {
    /// Equality comparison.
    Eq,
    /// Inequality comparison.
    Ne,
    /// Strictly less-than comparison.
    Lt,
    /// Less-than-or-equal comparison.
    Lte,
    /// Strictly greater-than comparison.
    Gt,
    /// Greater-than-or-equal comparison.
    Gte,
    /// Membership in a set of values.
    In,
    /// Exclusion from a set of values.
    NotIn,
    /// Field contains all listed values.
    HasAll,
    /// Field contains any listed value.
    HasAny,
    /// Field contains none of the listed values.
    HasNone,
    /// Pattern/glob match.
    Match,
    /// Negated pattern/glob match.
    NotMatch,
    /// Case-insensitive substring containment.
    Contains,
    /// Case-insensitive prefix match.
    StartsWith,
    /// Case-insensitive suffix match.
    EndsWith,
}

/// Canonical predicate value in the daemon wire contract.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SearchPredicateValue {
    /// String scalar.
    String(String),
    /// String list.
    StringList(Vec<String>),
    /// Unsigned integer scalar.
    U64(u64),
    /// Signed integer scalar.
    I64(i64),
    /// Boolean scalar.
    Bool(bool),
}

/// Canonical predicate clause in the daemon wire contract.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SearchPredicate {
    /// Canonical field name or accepted alias.
    pub field: String,
    /// Comparison operator.
    pub op: SearchPredicateOp,
    /// Predicate operand.
    pub value: SearchPredicateValue,
}

/// Canonical response shaping mode for direct daemon callers.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchResponseMode {
    /// Traditional full-row response.
    Rows,
    /// Projected JSON objects keyed by projected field name.
    Json,
}

// Application error codes (daemon-specific)
/// Daemon is still loading indices.
pub const ERR_NOT_READY: i32 = -1;
/// Search pattern compilation failed (bad regex).
pub const ERR_BAD_PATTERN: i32 = -2;
/// Method is wired into the protocol surface but has no handler
/// implementation yet.
///
/// Used by the Phase 8-A scaffolding stage of the memory-tiering
/// rollout: `hibernate` / `preload` / `forget` / `status_drives` are
/// reachable in the dispatcher and serialise round-trip cleanly,
/// but each handler returns this error until the corresponding
/// follow-up sub-phase (8-B / 8-C / 8-D / 8-E) fills in the logic.
pub const ERR_NOT_IMPLEMENTED: i32 = -3;
/// Drive is non-`Cold` and cannot be modified by a destructive
/// operation that requires it to be at rest.
///
/// Reserved for the `forget` method (Phase 8-D) which refuses to
/// delete cache files for a drive whose shard is still warm in RAM.
/// Operators must `hibernate` the drive first or pass `force = true`.
pub const ERR_DRIVE_BUSY: i32 = -4;

// ────────────────────────────────────────────────────────────────────────────
// Method parameters
// ────────────────────────────────────────────────────────────────────────────

/// Parameters for the `search` method.
///
/// All filter fields mirror the CLI surface; see
/// `uffs_core::search::filters::SearchFilters` for semantics.
/// Every field is optional — omitted fields impose no constraint.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "JSON-RPC wire type — boolean fields are the natural JSON encoding for on/off flags"
)]
pub struct SearchParams {
    // ── Core ────────────────────────────────────────────────────────
    /// Search pattern (glob, regex with `>` prefix, or substring).
    pub pattern: String,
    /// Case-sensitive matching.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Whole-word matching.
    #[serde(default)]
    pub whole_word: bool,
    /// Match pattern against the full path (not just the filename).
    ///
    /// When true, directory records whose name matches the pattern will also
    /// contribute all their descendants to the result set.  Default (`false`)
    /// matches filename-only, consistent with Everything's default behaviour.
    #[serde(default)]
    pub match_path: bool,

    // ── Sort ────────────────────────────────────────────────────────
    /// Sort column name (e.g. `"modified"`, `"size"`, `"name"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// Canonical ordered sort clauses. Preferred over legacy `sort`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sorts: Vec<SearchSortSpec>,
    /// Sort direction: `true` = descending.
    #[serde(default)]
    pub sort_desc: bool,

    // ── Limit ───────────────────────────────────────────────────────
    /// Maximum results to return (`None` = unlimited).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,

    // ── Filter mode ────────────────────────────────────────────────
    /// Filter mode: `"all"` (default), `"files"`, `"dirs"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Canonical filter mode. Preferred over legacy `filter`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_mode: Option<SearchFilterMode>,
    /// Canonical predicates. Preferred over legacy filter fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicates: Vec<SearchPredicate>,
    /// Specific drives to search (empty = all loaded).
    #[serde(default)]
    pub drives: Vec<char>,
    /// Requested projection fields in canonical order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projection: Vec<String>,
    /// Requested response shaping mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mode: Option<SearchResponseMode>,

    // ── Size filters ───────────────────────────────────────────────
    /// Minimum file size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_size: Option<u64>,
    /// Maximum file size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_size: Option<u64>,

    // ── Descendant filters ─────────────────────────────────────────
    /// Minimum descendant count (inclusive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_descendants: Option<u32>,
    /// Maximum descendant count (inclusive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_descendants: Option<u32>,

    // ── Time filters ───────────────────────────────────────────────
    /// Modified-time lower bound (e.g. `"7d"`, `"24h"`, `"2026-01-15"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newer: Option<String>,
    /// Modified-time upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older: Option<String>,
    /// Created-time lower bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newer_created: Option<String>,
    /// Created-time upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_created: Option<String>,
    /// Accessed-time lower bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newer_accessed: Option<String>,
    /// Accessed-time upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_accessed: Option<String>,

    // ── Attribute filter ───────────────────────────────────────────
    /// Attribute filter spec (e.g. `"hidden,compressed,!system"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,

    // ── Extension filter ───────────────────────────────────────────
    /// Comma-separated extension filter (e.g. `"rs,toml,md"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,

    // ── Exclude ────────────────────────────────────────────────────
    /// Exclude glob pattern (e.g. `"backup*"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<String>,
    /// Directory-path pattern (glob). Only matches against the directory
    /// portion of the path, not the filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_contains: Option<String>,
    /// File type/category filter (e.g. `"code"`, `"document"`, `"picture"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_filter: Option<String>,
    /// Minimum bulkiness percentage (100 = perfectly packed, >100 = wasteful).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_bulkiness: Option<u64>,
    /// Maximum bulkiness percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bulkiness: Option<u64>,

    // ── Length filters ─────────────────────────────────────────────
    /// Minimum filename length in characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_name_len: Option<u16>,
    /// Maximum filename length in characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_name_len: Option<u16>,
    /// Minimum full-path length in characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_path_len: Option<u16>,
    /// Maximum full-path length in characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_path_len: Option<u16>,

    // ── Size-on-disk filters ──────────────────────────────────────
    /// Minimum allocated (on-disk) size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_allocated: Option<u64>,
    /// Maximum allocated (on-disk) size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_allocated: Option<u64>,

    // ── Tree metric filters ────────────────────────────────────────
    /// Minimum subtree logical size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_treesize: Option<u64>,
    /// Maximum subtree logical size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_treesize: Option<u64>,
    /// Minimum subtree allocated (on-disk) size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_tree_allocated: Option<u64>,
    /// Maximum subtree allocated (on-disk) size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tree_allocated: Option<u64>,

    // ── Month-of-year filter ──────────────────────────────────────
    /// Allowed month numbers (1-12).  Empty = no month filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_months: Vec<u32>,

    // ── Misc ───────────────────────────────────────────────────────
    /// Hide system meta-files (names starting with `$`).
    #[serde(default)]
    pub hide_system: bool,
    /// Hide NTFS Alternate Data Streams from results.
    #[serde(default)]
    pub hide_ads: bool,

    // ── Profiling ──────────────────────────────────────────────────
    /// Request detailed timing breakdown from the daemon.
    #[serde(default)]
    pub profile: bool,

    // ── Aggregation ────────────────────────────────────────────────
    /// Aggregation specs to compute alongside or instead of rows.
    ///
    /// When non-empty, the daemon runs the aggregation engine in addition
    /// to (or instead of) returning rows, depending on `include_rows`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aggregations: Vec<AggregateSpecWire>,
    /// Whether to include result rows in the response.
    ///
    /// Defaults to `true`. Set to `false` for aggregate-only queries
    /// (equivalent to `--count` or `--aggregate` without `--rows`).
    #[serde(default = "default_true")]
    pub include_rows: bool,

    // ── Aggregation pagination ─────────────────────────────────────
    /// Opaque cursor token from a previous response's `next_cursor`.
    ///
    /// When set, the daemon resumes pagination from the encoded position.
    /// The cursor encodes `result_index:offset:page_size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agg_cursor: Option<String>,
    /// Maximum buckets per aggregation result page.
    ///
    /// When set (and `agg_cursor` is absent), the daemon returns at most
    /// this many buckets per result, with a `next_cursor` on any result
    /// that has more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agg_page_size: Option<u16>,

    // ── Direct file output ────────────────────────────────────────
    /// When set, the daemon writes results directly to this file path
    /// instead of sending rows through IPC.  The response contains only
    /// metadata (`rows_written`, timing).  This eliminates serialization
    /// and IPC overhead for bulk exports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    /// Output config: column separator (default: `","`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_separator: Option<String>,
    /// Output config: quote character for strings (default: `"\""`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_quote: Option<String>,
    /// Output config: include header row (default: `true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_header: Option<bool>,
    /// Output config: representation for active boolean attributes (default:
    /// `"1"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_pos: Option<String>,
    /// Output config: representation for inactive boolean attributes (default:
    /// `"0"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_neg: Option<String>,
    /// Output config: columns to output (default: all).
    /// Comma-separated column names like `"path,name,size"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_columns: Option<String>,
    /// Output config: enable parity-compat formatting (trailing `\` on
    /// directory paths, empty `Name` for dirs, `treesize` for dir `Size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_parity_compat: Option<bool>,
    /// Output config: timezone offset in hours from UTC for timestamp
    /// formatting (overrides auto-detected local timezone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tz_offset_hours: Option<i32>,
    /// Output config: CLI-layer `--format` value (e.g. `"csv"`, `"json"`,
    /// `"custom"`, `"table"`).
    ///
    /// The daemon uses this field to decide how to pre-format rows
    /// server-side into a
    /// [`SearchPayload::InlineBlob`](crate::protocol::response::SearchPayload::InlineBlob)
    /// / [`SearchPayload::ShmemBlob`](crate::protocol::response::SearchPayload::ShmemBlob).
    /// As of Phase 3:
    ///
    /// - `Some("csv")` (or absent — CLI default is `csv`) → pre-format through
    ///   [`uffs_format::write_rows`], emitting the canonical CSV bytes.
    /// - `Some("custom")` → pre-format the CSV body, then append the legacy
    ///   `Drives? … / MMMmmm …` footer via
    ///   [`uffs_format::write_legacy_drive_footer`] (gated on
    ///   `output_drive_targets` being non-empty).
    /// - `Some("json")` / `Some("table")` → skip pre-format.  The CLI keeps
    ///   ownership of those structural formats.
    ///
    /// Byte-parity between the CLI slow path and the daemon fast path
    /// is pinned by
    /// `uffs_cli::commands::output::tests::{parity_byte_parity_*,
    /// columnar_byte_parity_*}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,

    /// Drive letters the search targeted, echoed back into the legacy
    /// drive footer when `output_format == Some("custom")`.
    ///
    /// Matches the CLI's local `targets` computation: populated from
    /// `--drive` / `--drives` (and, in the thin-client passthrough
    /// path, `--mft-file` by extracting drive letters from the file
    /// paths).  Empty → footer omitted entirely, matching
    /// `uffs_format::write_legacy_drive_footer`'s "no drives, no
    /// footer" rule.
    ///
    /// The field is intentionally separate from [`Self::drives`]
    /// because "which drives to search" and "which drives to show in
    /// the footer" are semantically different — e.g.  `--mft-file
    /// C.mft` targets drive C for the footer but leaves `drives`
    /// empty because the MFT path is a separate wire selector.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_drive_targets: Vec<char>,
}

/// Default-true helper for serde.
const fn default_true() -> bool {
    true
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            case_sensitive: false,
            whole_word: false,
            match_path: false,
            sort: None,
            sorts: vec![],
            sort_desc: false,
            limit: None,
            filter: None,
            filter_mode: None,
            predicates: vec![],
            drives: vec![],
            projection: vec![],
            response_mode: None,
            min_size: None,
            max_size: None,
            min_descendants: None,
            max_descendants: None,
            newer: None,
            older: None,
            newer_created: None,
            older_created: None,
            newer_accessed: None,
            older_accessed: None,
            attr: None,
            ext: None,
            exclude: None,
            path_contains: None,
            type_filter: None,
            min_bulkiness: None,
            max_bulkiness: None,
            min_name_len: None,
            max_name_len: None,
            min_path_len: None,
            max_path_len: None,
            min_allocated: None,
            max_allocated: None,
            min_treesize: None,
            max_treesize: None,
            min_tree_allocated: None,
            max_tree_allocated: None,
            allowed_months: vec![],
            hide_system: false,
            hide_ads: false,
            profile: false,
            aggregations: vec![],
            include_rows: true,
            agg_cursor: None,
            agg_page_size: None,
            output_file: None,
            output_separator: None,
            output_quote: None,
            output_header: None,
            output_pos: None,
            output_neg: None,
            output_columns: None,
            output_parity_compat: None,
            output_tz_offset_hours: None,
            output_format: None,
            output_drive_targets: Vec::new(),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Aggregation wire types
// ────────────────────────────────────────────────────────────────────────────

/// Wire format for a single aggregation specification.
///
/// This is the JSON-serializable form of
/// `uffs_core::aggregate::spec::AggregateSpec`. It uses tagged-enum style for
/// `kind` to make JSON schemas self-describing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregateSpecWire {
    /// The aggregation kind (e.g. `"count"`, `"terms"`, `"stats"`).
    pub kind: String,
    /// Optional label for this aggregation in the output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Field to aggregate on (required for most kinds except `"count"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Maximum groups for terms aggregation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<u16>,
    /// Bucket interval for histogram aggregation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<u64>,
    /// Calendar interval for date histogram (e.g. `"month"`, `"day"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
    /// Range boundaries for range aggregation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boundaries: Vec<u64>,
    /// Metrics to compute per bucket/group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metrics: Vec<String>,
    /// Preset name (when `kind` is `"preset"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Number of sample rows (top-hits) to attach per bucket.
    /// `None` or absent means no samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<u8>,
    /// Sort field for sample rows (e.g. `"size"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_sort: Option<String>,
    /// Sort direction for sample rows.  `true` = descending (largest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_desc: Option<bool>,
    /// Duplicate verification mode: `"first_bytes"`, `"sha256"`, or
    /// absent/`"none"`.
    ///
    /// Only meaningful when `kind` is `"duplicates"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    /// Byte count for `verify=first_bytes` mode (default: 4096).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_bytes: Option<u32>,
}

/// Wire format for an aggregate result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateResultWire {
    /// Label for this result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The result kind (mirrors the spec kind).
    pub kind: String,
    /// Field name (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Scalar value (for count/missing/distinct).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<u64>,
    /// Scalar statistics (for stats kind).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<StatsWire>,
    /// Bucket rows (for `terms`/`histogram`/`date_histogram`/`range`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<BucketWire>,
    /// Count of records beyond top-N (for terms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_count: Option<u64>,
    /// Total groups before truncation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_groups: Option<usize>,
    /// Cursor token for the next page of buckets.
    ///
    /// Present only when the request included `page_size` and more
    /// buckets remain beyond the current page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Whether the bucket values are exact (not approximate).
    ///
    /// `true` for all current implementations — reserved for future
    /// sampling-based approximation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<bool>,
    /// Whether the result contains all distinct values.
    ///
    /// `true` when every group fits within `top` (i.e. `other_count == 0`).
    /// `false` when the result was truncated and `other_count > 0`.
    /// Absent for non-bucket results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values_complete: Option<bool>,
}

/// Wire format for scalar statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsWire {
    /// Record count.
    pub count: u64,
    /// Sum of values.
    pub sum: u64,
    /// Minimum value.
    pub min: u64,
    /// Maximum value.
    pub max: u64,
    /// Average value.
    pub avg: f64,
    /// Waste bytes.
    pub waste_bytes: u64,
    /// Waste percentage.
    pub waste_pct: f64,
}

/// Wire format for a single bucket row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BucketWire {
    /// Bucket key (display string).
    pub key: String,
    /// Record count in this bucket.
    pub count: u64,
    /// Total bytes in this bucket.
    pub total_bytes: u64,
    /// Total allocated bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_allocated: Option<u64>,
    /// Average file size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_size: Option<f64>,
    /// Share of total count (percentage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_count: Option<f64>,
    /// Share of total bytes (percentage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_bytes: Option<f64>,
    /// Sample rows (top-hits) — representative records from this bucket.
    /// Empty when no `sample` was requested in the spec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_rows: Vec<SampleRowWire>,
    /// Drill-down predicates for re-querying this bucket's records.
    /// Includes both the original query predicates and the bucket key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drilldown: Vec<DrilldownWire>,
    /// Nested sub-aggregation bucket results (populated by nested rollups).
    ///
    /// When a rollup spec has a `sub` aggregation, each top-level bucket
    /// contains the sub-aggregation's buckets here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_buckets: Vec<Self>,
    /// Whether this duplicate group has been content-verified.
    ///
    /// Only present for `kind="duplicates"` results with `verify != "none"`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verified: bool,
}

/// Wire format for a sample row (top-hit) within a bucket.
///
/// Each entry represents one record from the bucket, projected onto a
/// set of display fields (e.g. `name`, `size`, `modified`).  The daemon
/// populates this from `uffs_core::aggregate::SampleRow` during
/// `BucketRow → BucketWire` conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleRowWire {
    /// Projected fields as key-value pairs (e.g. `"name" → "foo.rs"`).
    pub fields: std::collections::HashMap<String, String>,
    /// Sort key used for ordering (e.g. file size).  Absent when the
    /// sample was collected without an explicit sort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<i64>,
}

/// Wire format for a drill-down predicate.
///
/// A client can re-issue a row-level search using the predicates
/// attached to a bucket to retrieve the records behind it.  The
/// `value` is a [`serde_json::Value`] so it naturally maps to JSON
/// without an extra enum on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrilldownWire {
    /// Canonical field name (e.g. `"extension"`, `"drive"`, `"type"`).
    pub field: String,
    /// Comparison operator (e.g. `"eq"`, `"gte"`, `"in"`).
    pub op: String,
    /// Predicate value — string, number, or boolean.
    pub value: serde_json::Value,
}
