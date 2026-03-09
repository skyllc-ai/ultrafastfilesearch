//! Index command implementation.

use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::Arc;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;
use uffs_mft::{MftProgress, MftReader};

#[cfg(windows)]
use super::{add_drive_progress, create_multi_progress};

/// Build an index from drive MFT(s).
///
/// Supports both single drive (`--drive C`) and multiple drives (`--drives
/// C,D,E`). When multiple drives are specified, they are read concurrently and
/// merged into a single `DataFrame` with a `drive` column.
///
/// If no drives are specified, indexes ALL available NTFS drives.
#[expect(
    clippy::single_call_fn,
    reason = "public CLI command handler called from main dispatch"
)]
pub async fn index(
    output_path: PathBuf,
    single_drive: Option<char>,
    multi_drives: Option<Vec<char>>,
) -> Result<()> {
    let output = if output_path.extension().is_some() {
        output_path
    } else {
        output_path.with_extension("parquet")
    };

    let drive_list: Vec<char> = match (single_drive, multi_drives) {
        (Some(drv), None) => vec![drv],
        (None, Some(drvs)) => drvs,
        (None, None) => {
            #[cfg(windows)]
            {
                if !uffs_mft::is_elevated() {
                    anyhow::bail!(
                        "Administrator privileges required.\n\n\
                         UFFS reads the NTFS Master File Table directly, which requires elevated access.\n\n\
                         Solutions:\n\
                         1. Run PowerShell/Terminal as Administrator\n\
                         2. Specify a drive explicitly: uffs index --drive C output.parquet"
                    );
                }
                let all_drives = uffs_mft::detect_ntfs_drives();
                if all_drives.is_empty() {
                    anyhow::bail!("No NTFS drives found on this system");
                }
                info!(drives = ?all_drives, count = all_drives.len(), "No drive specified - indexing all NTFS drives");
                all_drives
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!(
                    "No drive specified. Use --drive or --drives to specify which drive(s) to index."
                );
            }
        }
        (Some(_), Some(_)) => anyhow::bail!("Cannot specify both --drive and --drives"),
    };

    if drive_list.is_empty() {
        anyhow::bail!("No drives specified");
    }

    if let Some(&drive_letter) = drive_list.first() {
        if drive_list.len() == 1 {
            info!(drive = %drive_letter, "Indexing drive");

            let reader = MftReader::open(drive_letter)
                .with_context(|| format!("Failed to open drive {drive_letter}:"))?;

            let progress_disabled = std::env::var("UFFS_NO_PROGRESS")
                .is_ok_and(|val| val == "1" || val.eq_ignore_ascii_case("true"));

            let progress_bar: Option<ProgressBar> = if progress_disabled {
                None
            } else {
                let bar = ProgressBar::new(0);
                let template = format!(
                    "{{spinner:.cyan}} [{drive_letter}:] [{{elapsed_precise}}] {{bar:40.cyan/blue}} {{bytes}}/{{total_bytes}} 📖 reading MFT..."
                );
                bar.set_style(
                    ProgressStyle::default_bar()
                        .template(&template)
                        .unwrap_or_else(|_| ProgressStyle::default_bar())
                        .progress_chars("━━╸"),
                );
                Some(bar)
            };

            let mut df = reader.read_with_progress(move |progress: MftProgress| {
                if let Some(bar) = &progress_bar {
                    if let Some(total) = progress.total_records {
                        bar.set_length(progress.bytes_read.max(total));
                    }
                    bar.set_position(progress.bytes_read);
                }
            })?;

            info!(records = df.height(), "Read records");

            MftReader::save_parquet(&mut df, &output)
                .with_context(|| format!("Failed to save index to {}", output.display()))?;

            info!(path = %output.display(), "Index saved");
            return Ok(());
        }
    }

    index_multi_drive(&drive_list, &output).await
}

/// Index multiple drives concurrently.
#[cfg(windows)]
async fn index_multi_drive(drives: &[char], output: &Path) -> Result<()> {
    use uffs_mft::MultiDriveMftReader;

    let drive_str: String = drives
        .iter()
        .map(|c| format!("{c}:"))
        .collect::<Vec<_>>()
        .join(", ");
    info!(drives = %drive_str, "Indexing drives");

    let reader = MultiDriveMftReader::new(drives.to_vec());
    let mp = create_multi_progress();
    let progress_bars: Option<Arc<std::collections::HashMap<char, ProgressBar>>> =
        mp.as_ref().map(|m| {
            let mut pbs = std::collections::HashMap::new();
            for &drive_char in drives {
                pbs.insert(drive_char, add_drive_progress(m, drive_char));
            }
            Arc::new(pbs)
        });

    let pbs = progress_bars.clone();

    let mut df = reader
        .read_with_progress(move |drive, progress| {
            if let Some(ref bars) = pbs {
                if let Some(pb) = bars.get(&drive) {
                    if let Some(total) = progress.total_records {
                        pb.set_length(progress.bytes_read.max(total));
                    }
                    pb.set_position(progress.bytes_read);
                }
            }
        })
        .await
        .context("Failed to read MFTs from drives")?;

    if let Some(ref bars) = progress_bars {
        for pb in bars.values() {
            pb.finish();
        }
    }

    info!(
        records = df.height(),
        drives = drives.len(),
        "Read records from drives"
    );

    MftReader::save_parquet(&mut df, output)
        .with_context(|| format!("Failed to save index to {}", output.display()))?;

    info!(path = %output.display(), "Index saved");
    Ok(())
}

/// Index multiple drives (non-Windows stub).
#[cfg(not(windows))]
#[expect(
    clippy::unused_async,
    reason = "must match async signature of Windows implementation"
)]
#[expect(
    clippy::single_call_fn,
    reason = "platform stub — matches Windows counterpart"
)]
async fn index_multi_drive(_drives: &[char], _output: &Path) -> Result<()> {
    anyhow::bail!("Multi-drive indexing is only supported on Windows")
}
