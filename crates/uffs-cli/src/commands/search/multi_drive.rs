//! Parallel multi-drive DataFrame search helpers.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use indicatif::ProgressBar;
use tokio::task::JoinSet;
use tracing::info;
use uffs_mft::IntoLazy;

use super::drive_search::{DriveResult, reorder_drive_column, search_single_drive};
use crate::commands::raw_io::{OwnedQueryFilters, QueryFilters};
use crate::commands::{add_drive_progress, create_multi_progress};

/// Search multiple drives in parallel with per-drive filtering.
///
/// This approach spawns all drive reads concurrently using tokio tasks,
/// then collects and merges results as they complete. This maximizes I/O
/// parallelism across multiple drives.
///
/// # Path Resolution
///
/// When `needs_paths` is true, builds a `FastPathResolver` from the full MFT
/// data before filtering. This ensures parent directories are available for
/// path resolution, fixing the `<unknown>` path bug.
///
/// # Arguments
///
/// * `no_bitmap` - If true, disables MFT bitmap optimization (reads all
///   records).
pub(super) async fn search_multi_drive_filtered(
    drives: &[char],
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    no_bitmap: bool,
) -> Result<uffs_mft::DataFrame> {
    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    let budget = super::search_drive_task_budget(drives.len());

    info!(
        count = drives.len(),
        budget, needs_paths, "Searching drives in PARALLEL (blazing fast mode)"
    );

    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));
    let multi_progress = create_multi_progress();

    let progress_bars: Option<Arc<HashMap<char, ProgressBar>>> =
        multi_progress.as_ref().map(|mp| {
            let mut pbs = HashMap::new();
            for &drive_char in drives {
                pbs.insert(drive_char, add_drive_progress(mp, drive_char));
            }
            Arc::new(pbs)
        });

    let mut pending_drives = drives.iter().copied();
    let mut join_set = JoinSet::new();

    for _ in 0..budget {
        if let Some(drive_char) = pending_drives.next() {
            let filters = Arc::clone(&owned_filters);
            let pbs = progress_bars.clone();

            join_set.spawn(async move {
                let pb = pbs
                    .as_ref()
                    .and_then(|progress_bars| progress_bars.get(&drive_char).cloned());
                search_single_drive(drive_char, filters, needs_paths, no_bitmap, pb).await
            });
        }
    }

    let mut filtered_results: Vec<uffs_mft::DataFrame> = Vec::new();
    let mut total_matches = 0usize;
    let mut drives_processed = 0usize;

    while let Some(join_result) = join_set.join_next().await {
        drives_processed += 1;

        let result = match join_result {
            Ok(result) => result,
            Err(error) => {
                info!(error = %error, "Drive search task failed");

                if let Some(drive_char) = pending_drives.next() {
                    let filters = Arc::clone(&owned_filters);
                    let pbs = progress_bars.clone();

                    join_set.spawn(async move {
                        let pb = pbs
                            .as_ref()
                            .and_then(|progress_bars| progress_bars.get(&drive_char).cloned());
                        search_single_drive(drive_char, filters, needs_paths, no_bitmap, pb).await
                    });
                }

                continue;
            }
        };

        if let Some(error) = result.error {
            info!(drive = %result.drive, error = %error, "Drive failed");
        } else {
            total_matches += result.matches;

            info!(
                drive = %result.drive,
                records = result.records_read,
                matches = result.matches,
                paths_resolved = result.paths_resolved,
                progress = format!("{}/{}", drives_processed, drives.len()),
                "Drive completed"
            );

            if let Some(df) = result.df {
                filtered_results.push(df);
            }
        }

        if let Some(drive_char) = pending_drives.next() {
            let filters = Arc::clone(&owned_filters);
            let pbs = progress_bars.clone();

            join_set.spawn(async move {
                let pb = pbs
                    .as_ref()
                    .and_then(|progress_bars| progress_bars.get(&drive_char).cloned());
                search_single_drive(drive_char, filters, needs_paths, no_bitmap, pb).await
            });
        }
    }

    if filtered_results.is_empty() {
        bail!("No matching files found across {} drives", drives.len());
    }

    let mut merged = filtered_results.remove(0);
    for df in filtered_results {
        merged = merged.vstack(&df).context("Failed to merge results")?;
    }

    let reordered = reorder_drive_column(&merged)?;
    let result = if filters.limit > 0 {
        reordered
            .lazy()
            .limit(filters.limit)
            .collect()
            .context("Failed to apply result limit")?
    } else {
        reordered
    };

    info!(
        total_matches,
        drives = drives.len(),
        "Parallel multi-drive search complete"
    );

    Ok(result)
}
