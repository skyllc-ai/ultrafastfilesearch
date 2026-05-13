// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! CSV and JSON export for aggregate results.
//!
//! Converts finalized aggregate results into tabular export formats.

use std::io::Write;

use super::finalize::{AggregateResponse, AggregateResultData, BucketRow};

/// Export format for aggregate results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Comma-separated values.
    Csv,
    /// Tab-separated values.
    Tsv,
    /// JSON (one array per result).
    Json,
}

impl ExportFormat {
    /// Parse from a string.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        match input.to_ascii_lowercase().as_str() {
            "csv" => Some(Self::Csv),
            "tsv" => Some(Self::Tsv),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Write aggregate results in the specified format.
///
/// # Errors
///
/// Returns an I/O error if writing fails.
pub fn export_results<W: Write>(
    response: &AggregateResponse,
    format: ExportFormat,
    writer: &mut W,
) -> std::io::Result<()> {
    match format {
        ExportFormat::Csv => export_csv(response, writer, b','),
        ExportFormat::Tsv => export_csv(response, writer, b'\t'),
        ExportFormat::Json => export_json(response, writer),
    }
}

/// Write as CSV/TSV.
fn export_csv<W: Write>(
    response: &AggregateResponse,
    writer: &mut W,
    sep: u8,
) -> std::io::Result<()> {
    let sep_char = char::from(sep);

    for result in &response.results {
        let label = result.label.as_deref().unwrap_or("result");

        match &result.data {
            AggregateResultData::Count { value } => {
                writeln!(writer, "# {label}")?;
                writeln!(writer, "count")?;
                writeln!(writer, "{value}")?;
            }

            AggregateResultData::Stats { field, stats } => {
                writeln!(writer, "# {label}")?;
                writeln!(
                    writer,
                    "field{sep_char}count{sep_char}sum{sep_char}min{sep_char}max{sep_char}avg{sep_char}waste_bytes{sep_char}waste_pct"
                )?;
                writeln!(
                    writer,
                    "{field}{sep_char}{}{sep_char}{}{sep_char}{}{sep_char}{}{sep_char}{:.2}{sep_char}{}{sep_char}{:.2}",
                    stats.count,
                    stats.sum,
                    stats.min,
                    stats.max,
                    stats.avg,
                    stats.waste_bytes,
                    stats.waste_pct
                )?;
            }

            AggregateResultData::Buckets { rows, .. }
            | AggregateResultData::Rollup { rows, .. } => {
                writeln!(writer, "# {label}")?;
                write_bucket_csv(writer, rows, sep_char)?;
            }

            AggregateResultData::Missing { field, count } => {
                writeln!(writer, "# {label}")?;
                writeln!(writer, "field{sep_char}missing_count")?;
                writeln!(writer, "{field}{sep_char}{count}")?;
            }

            AggregateResultData::Distinct { field, count } => {
                writeln!(writer, "# {label}")?;
                writeln!(writer, "field{sep_char}distinct_count")?;
                writeln!(writer, "{field}{sep_char}{count}")?;
            }

            AggregateResultData::Duplicates { result: dup_result } => {
                writeln!(writer, "# {label}")?;
                writeln!(
                    writer,
                    "key{sep_char}count{sep_char}file_size{sep_char}total_bytes{sep_char}reclaimable"
                )?;
                for group in &dup_result.groups {
                    writeln!(
                        writer,
                        "{}x{}{sep_char}{}{sep_char}{}{sep_char}{}{sep_char}{}",
                        group.count,
                        group.file_size,
                        group.count,
                        group.file_size,
                        group.total_bytes,
                        group.reclaimable_bytes
                    )?;
                }
            }
        }
        writeln!(writer)?; // Blank line between results
    }

    Ok(())
}

/// Write bucket rows as CSV/TSV.
fn write_bucket_csv<W: Write>(
    writer: &mut W,
    rows: &[BucketRow],
    sep: char,
) -> std::io::Result<()> {
    writeln!(
        writer,
        "key{sep}count{sep}total_bytes{sep}total_allocated{sep}avg_size{sep}waste_bytes{sep}waste_pct{sep}share_count{sep}share_bytes"
    )?;
    for row in rows {
        writeln!(
            writer,
            "{}{sep}{}{sep}{}{sep}{}{sep}{:.2}{sep}{}{sep}{:.2}{sep}{:.2}{sep}{:.2}",
            row.key,
            row.count,
            row.total_bytes,
            row.total_allocated,
            row.avg_size,
            row.waste_bytes,
            row.waste_pct,
            row.share_of_total_count,
            row.share_of_total_bytes
        )?;
    }
    Ok(())
}

/// Write as JSON.
fn export_json<W: Write>(_response: &AggregateResponse, writer: &mut W) -> std::io::Result<()> {
    // JSON export is handled by serde at the protocol layer.
    // This is a lightweight fallback for non-daemon usage.
    writeln!(
        writer,
        "{{\"note\": \"Use --format json on the CLI for full JSON output\"}}"
    )
}
