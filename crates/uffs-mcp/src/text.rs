// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Human-readable text formatting for MCP tool responses.
//!
//! These formatters produce compact summaries alongside structured JSON,
//! giving LLMs context without requiring them to parse raw data.

use core::fmt::Write as _;

/// Format the scan-stats header line shared by the aggregate and facet
/// tools, e.g. `"(12815626 scanned in 3ms)"`.
///
/// Mirrors the phrasing of the `uffs_search` text header so every
/// daemon-query tool reports its measured query time the same way —
/// agents can quote it directly instead of digging it out of the JSON
/// block.
#[must_use]
pub fn format_scan_header(records_scanned: usize, duration_ms: u64) -> String {
    format!("({records_scanned} scanned in {duration_ms}ms)")
}

/// Format aggregate results as a compact human-readable summary.
#[must_use]
pub fn format_aggregate_summary(results: &[uffs_client::protocol::AggregateResultWire]) -> String {
    let mut out = String::new();

    for result in results {
        let label = result.label.as_deref().unwrap_or(&result.kind);

        match result.kind.as_str() {
            "count" => {
                let val = result.value.unwrap_or(0);
                _ = writeln!(out, "• {label}: {val}");
            }
            "missing" => {
                let val = result.value.unwrap_or(0);
                _ = writeln!(out, "• {label}: {val} records with missing value");
            }
            "distinct" => {
                let val = result.value.unwrap_or(0);
                _ = writeln!(out, "• {label}: {val} distinct values");
            }
            "stats" => {
                if let Some(stats) = &result.stats {
                    _ = writeln!(
                        out,
                        "• {label}: count={} sum={} min={} max={} avg={:.1}",
                        stats.count, stats.sum, stats.min, stats.max, stats.avg
                    );
                    if stats.waste_bytes > 0 {
                        _ = writeln!(
                            out,
                            "  waste: {} bytes ({:.1}%)",
                            stats.waste_bytes, stats.waste_pct
                        );
                    }
                }
            }
            "buckets" | "terms" | "rollup" | "duplicates" => {
                format_bucket_summary(&mut out, label, result);
            }
            _ => {
                _ = writeln!(
                    out,
                    "• {label}: (kind={}, {} buckets)",
                    result.kind,
                    result.buckets.len()
                );
            }
        }
    }

    if out.is_empty() {
        out.push_str("No aggregate results.");
    }

    out
}

/// Format bucket-style results (terms, rollup, duplicates) into `out`.
fn format_bucket_summary(
    out: &mut String,
    label: &str,
    result: &uffs_client::protocol::AggregateResultWire,
) {
    _ = writeln!(out, "• {label} ({} buckets):", result.buckets.len());
    for bucket in result.buckets.iter().take(10) {
        _ = writeln!(
            out,
            "    {:<30} count={:<8} bytes={}",
            bucket.key, bucket.count, bucket.total_bytes
        );
        // Sample rows (top-hits), max 3 per bucket.
        let max_samples = 3;
        for sr in bucket.sample_rows.iter().take(max_samples) {
            let name = sr.fields.get("name").map_or("?", |val| val.as_str());
            let size = sr
                .fields
                .get("size")
                .and_then(|val| val.parse::<u64>().ok())
                .map_or(String::new(), |n| format!(" ({n} B)"));
            _ = writeln!(out, "      → {name}{size}");
        }
        let remaining = bucket.sample_rows.len().saturating_sub(max_samples);
        if remaining > 0 {
            _ = writeln!(out, "      ... and {remaining} more");
        }
        // Nested sub-aggregation buckets.
        for sub in bucket.sub_buckets.iter().take(5) {
            _ = writeln!(
                out,
                "      ├─ {:<26} count={:<8} bytes={}",
                sub.key, sub.count, sub.total_bytes
            );
        }
        let sub_rest = bucket.sub_buckets.len().saturating_sub(5);
        if sub_rest > 0 {
            _ = writeln!(out, "      ... and {sub_rest} more sub-buckets");
        }
    }
    if result.buckets.len() > 10 {
        _ = writeln!(out, "    ... and {} more", result.buckets.len() - 10);
    }
    if let Some(other) = result.other_count
        && other > 0
    {
        _ = writeln!(out, "    (+ {other} in other groups)");
    }
    if result.values_complete == Some(false) {
        _ = writeln!(out, "    [truncated — not all values shown]");
    }
    if result.exact == Some(false) {
        _ = writeln!(out, "    [approximate — not all records scanned]");
    }
    if let Some(cursor) = &result.next_cursor {
        _ = writeln!(out, "    [next_cursor: {cursor}]");
    }
}

/// Format a search result row as a markdown table line.
///
/// Columns: `| Name | Ext | Type | Size | Modified | Path |`
#[must_use]
pub fn format_search_row(row: &uffs_client::protocol::response::SearchRow) -> String {
    let ext = match row.name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    };
    let kind = if row.is_directory { "dir" } else { "file" };
    format!(
        "| {} | {} | {} | {} | {} | {} |",
        row.name,
        ext,
        kind,
        uffs_client::protocol::response::format_size(row.size),
        uffs_client::protocol::response::format_time(row.modified),
        row.path,
    )
}
