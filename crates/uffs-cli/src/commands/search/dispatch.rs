// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Thin-client output dispatch for `search_cli` responses.
//!
//! Extracts format/column/separator settings from raw CLI args and
//! delegates to the output module for formatting.

use std::io::Write as _;

use anyhow::Result;
use uffs_client::format::extract_drive_letter;

use super::super::output::write_native_results;

// ── Thin-client output helpers ─────────────────────────────────────────
//
// Used by the passthrough `search_cli` path where no `SearchConfig` exists.

/// Extract a flag value from raw CLI args.
fn arg_val<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let eq_prefix = format!("{flag}=");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix(eq_prefix.as_str()) {
            return Some(rest);
        }
        if arg == flag {
            return iter.next().map(String::as_str);
        }
    }
    None
}

/// Write search result rows to console using format extracted from raw
/// CLI args.
///
/// The daemon already writes to file when `--out` is set (OPT-4),
/// so this only handles console output.
///
/// # Errors
///
/// Returns an error if writing fails.
pub fn write_rows(rows: &[serde_json::Value], args: &[String]) -> Result<()> {
    let format = arg_val(args, "--format")
        .or_else(|| arg_val(args, "-f"))
        .unwrap_or("csv");
    // --parity-compat implies --columns parity (matches legacy OutputConfig
    // behaviour).
    let parity_compat = args.iter().any(|arg| arg == "--parity-compat");
    let columns = if parity_compat {
        "parity"
    } else {
        arg_val(args, "--columns").unwrap_or("")
    };
    let sep = arg_val(args, "--sep").unwrap_or(",");
    let quotes = arg_val(args, "--quotes").unwrap_or("\"");
    let header = arg_val(args, "--header").is_none_or(|val| val != "false" && val != "0");
    let pos = arg_val(args, "--pos").unwrap_or("1");
    let neg = arg_val(args, "--neg").unwrap_or("0");
    let tz_offset = arg_val(args, "--tz-offset").and_then(|val| val.parse::<i32>().ok());

    // Extract drive targets for footer.
    let drive = arg_val(args, "--drive").or_else(|| arg_val(args, "-d"));
    let drives_str = arg_val(args, "--drives");
    let mft_str = arg_val(args, "--mft-file");
    let mut targets: Vec<uffs_mft::platform::DriveLetter> = Vec::new();
    if let Some(drive_val) = drive {
        if let Some(letter) = drive_val
            .chars()
            .next()
            .and_then(|ch| uffs_mft::platform::DriveLetter::parse(ch).ok())
        {
            targets.push(letter);
        }
    } else if let Some(drives_val) = drives_str {
        for part in drives_val.split(',') {
            let trimmed = part.trim();
            let stripped = trimmed.strip_suffix(':').unwrap_or(trimmed);
            if let Some(letter) = stripped
                .chars()
                .next()
                .and_then(|ch| uffs_mft::platform::DriveLetter::parse(ch).ok())
            {
                targets.push(letter);
            }
        }
    } else if let Some(mft_val) = mft_str {
        for part in mft_val.split(',') {
            if let Some(letter) = extract_drive_letter(part.trim()) {
                targets.push(letter);
            }
        }
    }

    let out = arg_val(args, "--out").unwrap_or("console");
    let pattern = args.first().map_or("*", String::as_str);

    write_native_results(
        rows,
        format,
        out,
        columns,
        sep,
        quotes,
        header,
        pos,
        neg,
        tz_offset,
        &targets,
        core::time::Duration::ZERO,
        pattern,
    )
}

/// Write aggregate results to console.
///
/// # Errors
///
/// Returns an error if writing fails.
pub(crate) fn write_aggregations(
    aggregations: &[serde_json::Value],
    args: &[String],
) -> Result<()> {
    let format = arg_val(args, "--format")
        .or_else(|| arg_val(args, "-f"))
        .unwrap_or("csv");
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(aggregations)?;
            writeln!(std::io::stdout(), "{json}")?;
        }
        "csv" | "tsv" => {
            crate::commands::aggregate::print_csv_results_raw(aggregations, format == "tsv")?;
        }
        _ => {
            crate::commands::aggregate::print_table_results_raw(aggregations)?;
        }
    }
    Ok(())
}
