// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tool and prompt definitions for `tools/list` and `prompts/list`.
//!
//! Also includes JSON Schema sanitisation helpers that strip non-standard
//! `"format"` annotations emitted by `schemars` for Rust integer types.

extern crate alloc;

use alloc::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::Value;

use crate::tools;

/// Standard JSON Schema format values recognised by AJV and most validators.
/// Non-standard formats (e.g. `uint64`, `uint32`, `uint` emitted by
/// `schemars` for Rust integer types) cause noisy warnings in MCP clients
/// that use AJV (Augment, `OpenCode`, Gemini CLI).
const STANDARD_FORMATS: &[&str] = &[
    // JSON Schema built-in string formats
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

/// Recursively strip non-standard `"format"` values from a JSON Schema tree.
///
/// `schemars` emits `"format": "uint64"` for `u64`, `"format": "uint32"` for
/// `u32`, `"format": "uint"` for `usize`, etc.  These are valid Rust-specific
/// annotations but not standard JSON Schema, and AJV-based clients warn on
/// every one.  This walk removes them while preserving standard formats like
/// `"date-time"` and `"uuid"`.
fn strip_nonstandard_formats(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(fmt)) = map.get("format")
                && !STANDARD_FORMATS.contains(&fmt.as_str())
            {
                map.remove("format");
            }
            for val in map.values_mut() {
                strip_nonstandard_formats(val);
            }
        }
        Value::Array(arr) => {
            for val in arr.iter_mut() {
                strip_nonstandard_formats(val);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Convert a schemars `Schema` to the rmcp `InputSchema` type
/// (`Arc<Map<String, Value>>`), stripping non-standard format annotations.
fn schema_to_input(schema: schemars::Schema) -> Arc<serde_json::Map<String, Value>> {
    let mut value = serde_json::to_value(schema).unwrap_or_default();
    strip_nonstandard_formats(&mut value);
    if let Value::Object(map) = value {
        Arc::new(map)
    } else {
        Arc::new(serde_json::Map::new())
    }
}

/// Like [`Tool::with_output_schema`] but strips non-standard format values
/// from the generated schema before attaching it.
fn with_clean_output_schema<T: schemars::JsonSchema + 'static>(tool: Tool) -> Tool {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(schema).unwrap_or_default();
    strip_nonstandard_formats(&mut value);
    let mut result = tool;
    if let Value::Object(map) = value {
        result.output_schema = Some(Arc::new(map));
    }
    result
}

/// Known tool names — used for early rejection before daemon dispatch.
pub(crate) const KNOWN_TOOLS: &[&str] = &[
    "uffs_search",
    "uffs_drives",
    "uffs_status",
    "uffs_info",
    "uffs_aggregate",
    "uffs_facet_values",
];

/// Check if a tool name is known.
#[must_use]
pub(crate) fn is_known_tool(name: &str) -> bool {
    KNOWN_TOOLS.contains(&name)
}

/// Build the static list of tool definitions for `tools/list`.
#[must_use]
pub(crate) fn tool_definitions() -> Vec<Tool> {
    use crate::schemas::{DrivesOutput, InfoOutput, StatusOutput};

    let read_only = ToolAnnotations::from_raw(
        None,        // title
        Some(true),  // read_only_hint
        Some(false), // destructive_hint
        Some(true),  // idempotent_hint
        Some(false), // open_world_hint
    );

    // Empty object schema for tools that take no arguments.
    // Uses rmcp's own helper — produces `{"type": "object", "properties": {}}`.
    let empty_schema = rmcp::handler::server::common::schema_for_empty_input();

    vec![
        Tool::new(
            "uffs_search",
            "Search files across all indexed NTFS drives. Supports glob patterns \
             (*.rs), regex (prefix with >), and substring matching. Returns \
             file name, size, modification time, and full path.",
            schema_to_input(schemars::schema_for!(tools::search::SearchArgs)),
        )
        // NOTE: outputSchema removed — structuredContent is disabled for
        // search (see search.rs).  Augment enforces the contract: if you
        // declare an outputSchema you MUST return structuredContent.
        .with_annotations(read_only.clone()),
        with_clean_output_schema::<InfoOutput>(Tool::new(
            "uffs_info",
            "Look up detailed information about a specific file or directory by its \
             full path. Returns size, timestamps, MFT attributes, and parent info.",
            schema_to_input(schemars::schema_for!(tools::info::InfoArgs)),
        ))
        .with_annotations(read_only.clone()),
        with_clean_output_schema::<DrivesOutput>(Tool::new(
            "uffs_drives",
            "List all NTFS drives currently indexed by the UFFS daemon. Returns \
             drive letter, record count, and index source (MFT or cached).",
            Arc::clone(&empty_schema),
        ))
        .with_annotations(read_only.clone()),
        with_clean_output_schema::<StatusOutput>(Tool::new(
            "uffs_status",
            "Get the current health and loading progress of the UFFS daemon. Returns \
             daemon state, uptime, memory usage, connection count, and PID.",
            empty_schema,
        ))
        .with_annotations(read_only.clone()),
        Tool::new(
            "uffs_aggregate",
            "Run server-side aggregations over the file index. Supports presets \
             (overview, by_type, by_extension, storage, cleanup, duplicates) and \
             custom specs. Returns counts, stats, terms, rollups, and histograms.",
            schema_to_input(schemars::schema_for!(tools::aggregate::AggregateArgs)),
        )
        // NOTE: outputSchema removed — structuredContent disabled.
        .with_annotations(read_only.clone()),
        Tool::new(
            "uffs_facet_values",
            "Explore distinct values of a field (extension, type, drive, etc.). \
             Returns top-N values with counts and byte totals. Useful for \
             understanding the composition of files before searching.",
            schema_to_input(schemars::schema_for!(tools::facet_values::FacetValuesArgs)),
        )
        // NOTE: outputSchema removed — structuredContent disabled.
        .with_annotations(read_only),
    ]
}

/// Build the static list of prompt definitions for `prompts/list`.
#[must_use]
pub(crate) fn prompt_definitions() -> Vec<rmcp::model::Prompt> {
    use rmcp::model::{Prompt, PromptArgument};

    vec![
        Prompt::new(
            "find_large_files",
            Some("Find the largest files across all drives, sorted by size descending"),
            Some(vec![
                PromptArgument::new("limit")
                    .with_description("Number of results (default: 50)")
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            "recent_changes",
            Some("Find files modified in the last N days"),
            Some(vec![
                PromptArgument::new("days")
                    .with_description("Number of days to look back (default: 1)")
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            "find_by_extension",
            Some("Find all files with a specific extension"),
            Some(vec![
                PromptArgument::new("extension")
                    .with_description("File extension without dot (e.g., 'rs', 'pdf', 'jpg')")
                    .with_required(true),
                PromptArgument::new("limit")
                    .with_description("Number of results (default: 100)")
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            "find_duplicates_by_name",
            Some("Search for files with the same name across all drives"),
            Some(vec![
                PromptArgument::new("filename")
                    .with_description("Exact filename to search for")
                    .with_required(true),
            ]),
        ),
        Prompt::new(
            "disk_usage_report",
            Some("Generate a comprehensive disk usage report with type/extension/size breakdown"),
            Some(vec![
                PromptArgument::new("drive")
                    .with_description("Optional drive letter to scope report (e.g., 'C')")
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            "cleanup_report",
            Some("Identify cleanup candidates: large files, temp files, caches, and duplicates"),
            Some(vec![
                PromptArgument::new("min_size_mb")
                    .with_description("Minimum file size in MB to flag (default: 100)")
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            "duplicate_investigation",
            Some("Investigate duplicate files across all drives with size and path details"),
            Some(vec![
                PromptArgument::new("extension")
                    .with_description("Optional extension filter (e.g., 'pdf', 'jpg')")
                    .with_required(false),
            ]),
        ),
    ]
}

/// Percent-decode a URI path component back to a plain string.
///
/// Handles `%XX` sequences (e.g. `%20` → space, `%5C` → backslash).
#[must_use]
pub(crate) fn percent_decode_path(encoded: &str) -> String {
    let mut decoded = Vec::with_capacity(encoded.len());
    let bytes = encoded.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        // Collapse the outer bounds-check and the inner `from_str_radix` into
        // a single `if let` chain to satisfy `collapsible_if`.  We use `.get()`
        // instead of direct indexing to avoid `indexing_slicing` on byte arrays
        // and `string_slice` on the `&str`.
        if bytes.get(idx).copied() == Some(b'%')
            && idx + 2 < bytes.len()
            && let Some(hex_pair) = encoded.get(idx + 1..idx + 3)
            && let Ok(byte) = u8::from_str_radix(hex_pair, 16)
        {
            decoded.push(byte);
            idx += 3;
            continue;
        }
        if let Some(&raw) = bytes.get(idx) {
            decoded.push(raw);
        }
        idx += 1;
    }
    // AUDIT-OK(bytes): final decode of a locally-assembled buffer for display.
    // (WI-4.3 follow-up)
    String::from_utf8_lossy(&decoded).into_owned()
}
