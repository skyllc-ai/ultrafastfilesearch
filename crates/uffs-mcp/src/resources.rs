// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Static and live MCP resource implementations.
//!
//! Schema resources are compile-time metadata — they do not require a daemon
//! connection.  Live resources (`uffs://drives`, `uffs://status`) are handled
//! directly in [`crate::handler`].

use serde::Serialize;
use uffs_client::schema::{FieldAccess, FieldId, FieldType};

// Re-export cookbook_json so handler can use
// `crate::resources::cookbook_json()`. cookbook_json lives in crate::cookbook —
// import from there directly.

// ── uffs://schema/fields ─────────────────────────────────────────────

/// A single entry in the field catalog resource.
///
/// The four capability bools are genuinely independent flags (not a state
/// machine or correlated options), so the `excessive_bools` lint does not
/// apply here.
#[derive(Debug, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "schema description struct — bool fields are independent metadata flags"
)]
struct FieldEntry {
    /// Canonical field name.
    name: &'static str,
    /// Human-readable display name.
    display_name: &'static str,
    /// Data type (string, u64, i64, bool, timestamp, enum).
    field_type: &'static str,
    /// Access tier: hot (compact index), derived, cold (extra lookup).
    access: &'static str,
    /// Accepted aliases during parsing.
    aliases: &'static [&'static str],
    /// Whether the field can be used in sort clauses.
    sortable: bool,
    /// Whether the field can be used in filter predicates.
    filterable: bool,
    /// Whether the field can appear in result projections.
    projectable: bool,
    /// Whether the field supports aggregation grouping.
    groupable: bool,
}

/// Build the `uffs://schema/fields` JSON string.
#[must_use]
pub(crate) fn field_catalog_json() -> String {
    let entries: Vec<FieldEntry> = FieldId::ALL
        .iter()
        .map(|id| {
            let meta = id.metadata();
            FieldEntry {
                name: meta.canonical_name,
                display_name: meta.display_name,
                field_type: match meta.field_type {
                    FieldType::String => "string",
                    FieldType::Numeric => "numeric",
                    FieldType::Bool => "bool",
                    FieldType::Timestamp => "timestamp",
                    FieldType::Enum => "enum",
                    FieldType::Bitmask => "bitmask",
                },
                access: match meta.access {
                    FieldAccess::Hot => "hot",
                    FieldAccess::Derived => "derived",
                    FieldAccess::Cold => "cold",
                },
                aliases: meta.aliases,
                sortable: meta.sortable,
                filterable: meta.filterable,
                projectable: meta.projectable,
                groupable: meta.aggregate.groupable,
            }
        })
        .collect();

    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_owned())
}

// ── uffs://schema/search ─────────────────────────────────────────────

/// Build the `uffs://schema/search` JSON string from the schemars schema.
#[must_use]
pub(crate) fn search_schema_json() -> String {
    let schema = schemars::schema_for!(crate::tools::search::SearchArgs);
    serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".to_owned())
}

// ── uffs://schema/aggregate ──────────────────────────────────────────

/// Build the `uffs://schema/aggregate` JSON string from the schemars schema.
#[must_use]
pub(crate) fn aggregate_schema_json() -> String {
    let schema = schemars::schema_for!(crate::tools::aggregate::AggregateArgs);
    serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".to_owned())
}

// ── uffs://presets/aggregate ─────────────────────────────────────────

/// A single preset entry.
#[derive(Debug, Serialize)]
struct PresetEntry {
    /// Preset name (e.g. `"overview"`).
    name: &'static str,
    /// Human-readable description of what the preset computes.
    description: &'static str,
}

/// Build the `uffs://presets/aggregate` JSON string.
#[must_use]
pub(crate) fn aggregate_presets_json() -> String {
    let presets = vec![
        PresetEntry {
            name: "overview",
            description: "Full filesystem overview: total count, files vs dirs, size stats, \
                          type facet, drive facet, monthly modified histogram",
        },
        PresetEntry {
            name: "by_type",
            description: "Breakdown by semantic file type (documents, images, video, audio, \
                          archives, code, etc.) with size and waste metrics",
        },
        PresetEntry {
            name: "by_extension",
            description: "Top file extensions by count and total size",
        },
        PresetEntry {
            name: "by_drive",
            description: "Per-drive totals: file count, total size, allocated size",
        },
        PresetEntry {
            name: "by_size",
            description: "Size distribution histogram (tiny, small, medium, large, huge, giant)",
        },
        PresetEntry {
            name: "by_age",
            description: "Age distribution by last modification time",
        },
        PresetEntry {
            name: "storage",
            description: "Storage analysis: waste ratio, allocated vs logical, per-drive breakdown",
        },
        PresetEntry {
            name: "activity",
            description: "Activity analysis: recently created, modified, and accessed files",
        },
        PresetEntry {
            name: "top_folders",
            description: "Top-level folders ranked by subtree size",
        },
        PresetEntry {
            name: "duplicates",
            description: "Potential duplicate detection: files grouped by name+size with count > 1",
        },
        PresetEntry {
            name: "media",
            description: "Media file analysis: images, video, audio breakdown with size stats",
        },
        PresetEntry {
            name: "cleanup",
            description: "Cleanup candidates: zero-byte files, temp files, no-extension files, \
                          cache directories",
        },
    ];

    serde_json::to_string_pretty(&presets).unwrap_or_else(|_| "[]".to_owned())
}
