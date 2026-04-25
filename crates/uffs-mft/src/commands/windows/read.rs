// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Live MFT read/export command handlers.
//!
//! These commands print human-readable progress to stdout and convert
//! `usize` byte counters into `f64` for MB/s rate display; the lint
//! exemptions below capture those CLI-specific patterns.
#![expect(
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::default_numeric_fallback,
    reason = "byte / rate calculations convert integer counters into f64 for human-readable display"
)]

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::{info, warn};
use uffs_mft::MftReader;

use crate::display::{format_bytes, format_duration, format_number_commas};
use crate::progress::spinner;

/// `read` CLI command — read the MFT from `drive` and write a parsed
/// dataframe to `output`.
///
/// `mode_str` selects an `MftReadMode` (`bulk-iocp`, `sliding-window`, ...);
/// `full`, `unique`, `info_only`, `build_index`, `debug_tree`, and
/// `forensic` toggle output sections per the CLI flags.
pub(crate) async fn cmd_read(
    drive: char,
    output: PathBuf,
    mode_str: &str,
    full: bool,
    unique: bool,
    forensic: bool,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::MftReadMode;

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    warn_unsupported_forensic(forensic);

    let mode: MftReadMode = mode_str
        .parse()
        .map_err(|err: String| anyhow::anyhow!(err))?;
    log_read_startup(drive_upper, &output, mode, full, unique);

    let pb = spinner("Opening volume...");
    let reader = open_reader_with_logging(drive, drive_upper, mode, full, unique)?;

    pb.set_message("Reading MFT records...");
    let (mut df, record_count) = read_records_with_logging(&reader)?;

    pb.set_message("Saving to Parquet...");
    let file_size = save_dataframe_with_logging(&mut df, &output)?;

    let total_elapsed = start_time.elapsed();
    let file_size_mb = file_size as f64 / (1024.0 * 1024.0);
    info!(
        drive = %drive_upper,
        records = record_count,
        total_elapsed_ms = total_elapsed.as_millis(),
        output_size_mb = format!("{:.2}", file_size_mb),
        "🎉 MFT export complete"
    );

    pb.finish_with_message(format!(
        "✅ Exported {} records to {} ({}) in {}",
        format_number_commas(record_count as u64),
        output.display(),
        format_bytes(file_size),
        format_duration(total_elapsed)
    ));

    Ok(())
}

/// Emit the up-front warning about forensic mode being unsupported on
/// live reads.  Pulled out so [`cmd_read`] doesn't carry the conditional
/// directly in its cognitive-complexity budget.
fn warn_unsupported_forensic(forensic: bool) {
    if !forensic {
        return;
    }
    warn!("⚠️ Forensic mode (--forensic) is not yet supported for live reads.");
    warn!("   Use 'uffs_mft save' to save the MFT, then 'uffs_mft load --forensic' to analyze.");
    warn!("   Proceeding with normal mode...");
}

/// Single startup `info!` for the live-read pipeline; encapsulates the
/// branch on `unique` so the caller stays linear.
fn log_read_startup(
    drive_upper: char,
    output: &std::path::Path,
    mode: uffs_mft::MftReadMode,
    full: bool,
    unique: bool,
) {
    let suffix = if unique {
        " (unique FRS mode)"
    } else {
        " (expanding hard links)"
    };
    info!(
        drive = %drive_upper,
        output = %output.display(),
        mode = %mode,
        full,
        unique,
        "📂 Starting MFT read operation{}",
        suffix
    );
}

/// Open the volume handle, configure the reader, and trace the open
/// duration.  Returns the prepared [`MftReader`] for the read phase.
fn open_reader_with_logging(
    drive: char,
    drive_upper: char,
    mode: uffs_mft::MftReadMode,
    full: bool,
    unique: bool,
) -> Result<MftReader> {
    use std::time::Instant;

    use tracing::debug;

    debug!(drive = %drive_upper, "🔓 Opening volume handle");
    let open_start = Instant::now();
    let reader = MftReader::open(drive)
        .with_context(|| format!("Failed to open drive {drive}:"))?
        .with_mode(mode)
        .with_merge_extensions(full)
        .with_expand_links(!unique);

    info!(
        drive = %drive_upper,
        elapsed_ms = open_start.elapsed().as_millis(),
        "✅ Volume opened successfully"
    );

    Ok(reader)
}

/// Drive [`MftReader::read_all`], compute the records-per-second rate,
/// and trace the read summary.  Returns the assembled [`DataFrame`]
/// alongside its row count.
fn read_records_with_logging(reader: &MftReader) -> Result<(uffs_polars::DataFrame, usize)> {
    use std::time::Instant;

    use tracing::debug;

    debug!("📖 Starting MFT record enumeration");
    let read_start = Instant::now();
    let df = reader.read_all().with_context(|| "Failed to read MFT")?;

    let record_count = df.height();
    let read_elapsed = read_start.elapsed();
    let records_per_sec = if read_elapsed.as_secs_f64() > 0.0 {
        record_count as f64 / read_elapsed.as_secs_f64()
    } else {
        0.0
    };

    info!(
        records = record_count,
        elapsed_ms = read_elapsed.as_millis(),
        records_per_sec = format!("{:.0}", records_per_sec),
        "✅ MFT read complete"
    );

    Ok((df, record_count))
}

/// Persist `df` to `output` and emit the corresponding tracing line;
/// returns the on-disk file size for the final summary.
fn save_dataframe_with_logging(
    df: &mut uffs_polars::DataFrame,
    output: &std::path::Path,
) -> Result<u64> {
    use std::time::Instant;

    use tracing::debug;

    debug!(output = %output.display(), "💾 Writing Parquet file");
    let save_start = Instant::now();
    MftReader::save_parquet(df, output).with_context(|| "Failed to save Parquet")?;

    let file_size = std::fs::metadata(output).map_or(0, |metadata| metadata.len());
    let file_size_mb = file_size as f64 / (1024.0 * 1024.0);
    info!(
        output = %output.display(),
        file_size_mb = format!("{:.2}", file_size_mb),
        elapsed_ms = save_start.elapsed().as_millis(),
        "✅ Parquet file saved"
    );
    Ok(file_size)
}
