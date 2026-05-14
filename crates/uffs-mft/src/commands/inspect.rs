// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `inspect` (alias `file-info`) command — show stats for a Parquet index file.
//!
//! This is the `uffs-mft` equivalent of the former `uffs info` command.
//! Works cross-platform (no MFT or Windows APIs needed).

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use uffs_mft::reader::MftReader;

use crate::display::{format_bytes, format_number_commas};

/// Inspect a Parquet index file and print summary statistics.
///
/// # Errors
///
/// Returns an error if the file cannot be loaded or stdout write fails.
pub(crate) fn cmd_inspect(path: &Path) -> Result<()> {
    let df = MftReader::load_parquet(path)
        .with_context(|| format!("Failed to load parquet: {}", path.display()))?;

    let abs_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let file_size = std::fs::metadata(path).map_or(0, |meta| meta.len());
    let total = df.height();

    let dir_count = count_bool(&df, "is_directory");
    let file_count = (total as u64).saturating_sub(dir_count);

    let mut out = std::io::stdout().lock();
    let sep = "═══════════════════════════════════════════════════════════════";

    writeln!(out, "{sep}")?;
    writeln!(out, "                       INDEX FILE INFO")?;
    writeln!(out, "{sep}\n")?;

    writeln!(out, "📁 FILE DETAILS")?;
    writeln!(out, "  Path:                 {}", abs_path.display())?;
    writeln!(out, "  File size:            {}", format_bytes(file_size))?;
    writeln!(out, "  Columns:              {}\n", df.width())?;

    writeln!(out, "📊 RECORD STATISTICS")?;
    writeln!(
        out,
        "  Total records:        {}",
        format_number_commas(total as u64)
    )?;
    writeln!(
        out,
        "  Directories:          {}",
        format_number_commas(dir_count)
    )?;
    writeln!(
        out,
        "  Files:                {}\n",
        format_number_commas(file_count)
    )?;

    writeln!(out, "💾 SIZE METRICS")?;
    writeln!(
        out,
        "  Total file size:      {}",
        format_bytes(sum_u64(&df, "size"))
    )?;
    writeln!(
        out,
        "  Total allocated:      {}\n",
        format_bytes(sum_u64(&df, "allocated_size"))
    )?;

    writeln!(out, "🏷️  ATTRIBUTES")?;
    for (label, col) in [
        ("Hidden", "is_hidden"),
        ("System", "is_system"),
        ("Read-only", "is_readonly"),
        ("Archive", "is_archive"),
        ("Compressed", "is_compressed"),
        ("Encrypted", "is_encrypted"),
        ("Sparse", "is_sparse"),
        ("Reparse points", "is_reparse"),
    ] {
        writeln!(
            out,
            "  {label:<20} {}",
            format_number_commas(count_bool(&df, col))
        )?;
    }
    writeln!(out)?;

    writeln!(out, "🔗 ADVANCED")?;
    writeln!(
        out,
        "  Multi-stream files:   {}",
        format_number_commas(count_multi_u16(&df, "stream_count"))
    )?;
    writeln!(
        out,
        "  Multi-name files:     {}\n",
        format_number_commas(count_multi_u16(&df, "name_count"))
    )?;

    writeln!(out, "📋 SCHEMA")?;
    for (name, dtype) in df.schema().iter() {
        writeln!(out, "  {name}: {dtype}")?;
    }

    Ok(())
}

/// Count `true` values in a boolean column (0 if column is missing).
fn count_bool(df: &uffs_polars::DataFrame, col: &str) -> u64 {
    df.column(col)
        .ok()
        .and_then(|series| series.bool().ok())
        .map_or(0, |bools| u64::from(bools.sum().unwrap_or(0)))
}

/// Sum values in a `u64` column (0 if column is missing).
fn sum_u64(df: &uffs_polars::DataFrame, col: &str) -> u64 {
    df.column(col)
        .ok()
        .and_then(|series| series.u64().ok())
        .map_or(0, |arr| arr.iter().flatten().sum())
}

/// Count entries where a `u16` column value exceeds 1.
fn count_multi_u16(df: &uffs_polars::DataFrame, col: &str) -> u64 {
    df.column(col)
        .ok()
        .and_then(|series| series.u16().ok())
        .map_or(0, |arr| {
            arr.iter()
                .filter(|val| val.is_some_and(|num| num > 1))
                .count() as u64
        })
}
