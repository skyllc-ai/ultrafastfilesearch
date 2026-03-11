//! Streaming multi-drive search helpers.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;
use tracing::info;
use uffs_core::output::OutputConfig;

use super::drive_search::{DriveResult, reorder_drive_column, search_single_drive};
use crate::commands::output::StreamingWriter;
use crate::commands::raw_io::{OwnedQueryFilters, QueryFilters};

/// Streaming search for multi-drive queries.
///
/// Outputs results as each drive completes, providing immediate feedback.
/// Uses CSV or NDJSON format for streaming compatibility.
pub(super) async fn search_streaming(
    multi_drives: Option<Vec<char>>,
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    no_bitmap: bool,
) -> Result<()> {
    let drives = resolve_streaming_drives(multi_drives, single_drive, filters)?;

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout = std::io::stdout();
        search_multi_drive_streaming(&drives, filters, format, stdout, output_config, no_bitmap)
            .await
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let writer = BufWriter::new(file);
        search_multi_drive_streaming(&drives, filters, format, writer, output_config, no_bitmap)
            .await?;
        info!(file = out, "Results written to file");
        Ok(())
    }
}

/// Resolve the set of drives that a streaming search should scan.
fn resolve_streaming_drives(
    multi_drives: Option<Vec<char>>,
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
) -> Result<Vec<char>> {
    if let Some(drives) = multi_drives {
        return Ok(drives);
    }

    if let Some(drive) = single_drive.or_else(|| filters.parsed.drive()) {
        return Ok(vec![drive]);
    }

    if !uffs_mft::is_elevated() {
        bail!(
            "Administrator privileges required.\n\n\
             UFFS reads the NTFS Master File Table directly, which requires elevated access.\n\n\
             Solutions:\n\
             1. Run PowerShell/Terminal as Administrator\n\
             2. Use a pre-built index: uffs search --index <file.parquet> \"*.txt\""
        );
    }

    let all_drives = uffs_mft::detect_ntfs_drives();
    if all_drives.is_empty() {
        bail!("No NTFS drives found on this system");
    }
    info!(drives = ?all_drives, count = all_drives.len(), "Searching all NTFS drives");
    Ok(all_drives)
}

/// Search multiple drives in parallel with streaming output.
///
/// Outputs results as each drive completes, providing immediate feedback.
/// No progress bars - the streaming output is the progress indicator.
#[expect(
    clippy::print_stderr,
    reason = "intentional user-facing streaming errors"
)]
async fn search_multi_drive_streaming<W: Write + Send + 'static>(
    drives: &[char],
    filters: &QueryFilters<'_>,
    format: &str,
    writer: W,
    output_config: &OutputConfig,
    no_bitmap: bool,
) -> Result<()> {
    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    info!(
        count = drives.len(),
        "Streaming search across drives (results appear as each drive completes)"
    );

    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));
    let streaming_writer = Arc::new(StreamingWriter::new(
        writer,
        format,
        filters.limit,
        output_config.clone(),
    ));

    let (tx, mut rx) = mpsc::channel::<DriveResult>(drives.len());

    for &drive_char in drives {
        let tx = tx.clone();
        let filters = Arc::clone(&owned_filters);

        tokio::spawn(async move {
            let result = search_single_drive(drive_char, filters, true, no_bitmap, None).await;
            let _ = tx.send(result).await;
        });
    }

    drop(tx);

    let mut total_matches = 0usize;
    let mut drives_processed = 0usize;

    while let Some(result) = rx.recv().await {
        drives_processed += 1;

        if let Some(error) = result.error {
            eprintln!("[{}:] Error: {}", result.drive, error);
            continue;
        }

        total_matches += result.matches;

        if let Some(ref df) = result.df {
            if let Ok(reordered) = reorder_drive_column(df) {
                if let Err(error) = streaming_writer.write_batch(&reordered) {
                    eprintln!("[{}:] Write error: {}", result.drive, error);
                }
            }
        }

        if streaming_writer.limit_reached() {
            info!(
                limit = filters.limit,
                "Output limit reached, stopping early"
            );
            break;
        }

        info!(
            drive = %result.drive,
            records = result.records_read,
            matches = result.matches,
            progress = format!("{}/{}", drives_processed, drives.len()),
            "Drive completed"
        );
    }

    info!(
        total_matches,
        rows_output = streaming_writer.total_rows(),
        drives = drives.len(),
        "Streaming search complete"
    );

    Ok(())
}
