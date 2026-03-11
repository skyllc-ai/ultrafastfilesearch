//! Per-drive search helpers shared by multi-drive command flows.

use std::sync::Arc;

use anyhow::{Context, Result};
use indicatif::ProgressBar;
use uffs_mft::{IntoLazy, col, lit};

use crate::commands::raw_io::OwnedQueryFilters;

/// Result from a single drive read operation.
pub(super) struct DriveResult {
    /// Drive letter that was read.
    pub(super) drive: char,
    /// Filtered `DataFrame` with matching results (None if no matches or
    /// error).
    ///
    /// Paths are already resolved using the full MFT data when requested.
    pub(super) df: Option<uffs_mft::DataFrame>,
    /// Total records read from the MFT.
    pub(super) records_read: usize,
    /// Number of records matching the filters.
    pub(super) matches: usize,
    /// Error message if the drive read failed.
    pub(super) error: Option<String>,
    /// Whether paths were resolved (for logging).
    pub(super) paths_resolved: bool,
}

/// Load, filter, and decorate results for a single drive.
#[expect(
    clippy::too_many_lines,
    reason = "preserves the established per-drive search pipeline in one place"
)]
pub(super) async fn search_single_drive(
    drive_char: char,
    filters: Arc<OwnedQueryFilters>,
    needs_paths: bool,
    no_bitmap: bool,
    progress: Option<ProgressBar>,
) -> DriveResult {
    let full_df =
        uffs_mft::load_or_build_dataframe_cached(drive_char, uffs_mft::INDEX_TTL_SECONDS).await;

    let full_df = match full_df {
        Ok(df) => df,
        Err(error) => {
            if let Some(pb) = progress.as_ref() {
                pb.finish_with_message(format!("Error: {error}"));
            }
            return drive_error(drive_char, 0, 0, error.to_string(), false);
        }
    };

    _ = no_bitmap;

    let records_read = full_df.height();
    if let Some(pb) = progress.as_ref() {
        pb.finish();
    }

    let path_resolver = if needs_paths {
        match uffs_core::FastPathResolver::build(&full_df, drive_char) {
            Ok(resolver) => Some(resolver),
            Err(error) => {
                return drive_error(
                    drive_char,
                    records_read,
                    0,
                    format!("Failed to build path resolver: {error}"),
                    false,
                );
            }
        }
    } else {
        None
    };

    let filtered = match filters.execute(full_df) {
        Ok(filtered) => filtered,
        Err(error) => {
            return drive_error(drive_char, records_read, 0, error.to_string(), false);
        }
    };

    let matches = filtered.height();

    let with_paths = if let Some(resolver) = &path_resolver {
        match resolver.add_path_column_with_dir_suffix(&filtered) {
            Ok(df) => match uffs_core::add_path_only_column(&df) {
                Ok(df_with_path_only) => {
                    match uffs_core::apply_directory_treesize(&df_with_path_only) {
                        Ok(df_with_treesize) => df_with_treesize,
                        Err(error) => {
                            return drive_error(
                                drive_char,
                                records_read,
                                matches,
                                format!("Failed to apply treesize: {error}"),
                                false,
                            );
                        }
                    }
                }
                Err(error) => {
                    return drive_error(
                        drive_char,
                        records_read,
                        matches,
                        format!("Failed to add path_only: {error}"),
                        false,
                    );
                }
            },
            Err(error) => {
                return drive_error(
                    drive_char,
                    records_read,
                    matches,
                    format!("Failed to add paths: {error}"),
                    false,
                );
            }
        }
    } else {
        match uffs_core::apply_directory_treesize(&filtered) {
            Ok(df) => df,
            Err(error) => {
                return drive_error(
                    drive_char,
                    records_read,
                    matches,
                    format!("Failed to apply treesize: {error}"),
                    false,
                );
            }
        }
    };

    let df_with_drive = if matches > 0 {
        match with_paths
            .lazy()
            .with_column(lit(format!("{drive_char}:")).alias("drive"))
            .collect()
        {
            Ok(df) => Some(df),
            Err(error) => {
                return drive_error(
                    drive_char,
                    records_read,
                    matches,
                    error.to_string(),
                    path_resolver.is_some(),
                );
            }
        }
    } else {
        None
    };

    DriveResult {
        drive: drive_char,
        df: df_with_drive,
        records_read,
        matches,
        error: None,
        paths_resolved: path_resolver.is_some(),
    }
}

/// Reorder a `DataFrame` so the `drive` column appears first.
pub(super) fn reorder_drive_column(df: &uffs_mft::DataFrame) -> Result<uffs_mft::DataFrame> {
    let column_names: Vec<String> = df
        .get_column_names()
        .into_iter()
        .filter(|name| name.as_str() != "drive")
        .map(|name| name.to_string())
        .collect();
    let columns: Vec<_> = std::iter::once("drive".to_string())
        .chain(column_names)
        .map(|name| col(&name))
        .collect();

    df.clone()
        .lazy()
        .select(columns)
        .collect()
        .context("Failed to reorder columns")
}

/// Build a failed per-drive result.
fn drive_error(
    drive: char,
    records_read: usize,
    matches: usize,
    error: String,
    paths_resolved: bool,
) -> DriveResult {
    DriveResult {
        drive,
        df: None,
        records_read,
        matches,
        error: Some(error),
        paths_resolved,
    }
}
