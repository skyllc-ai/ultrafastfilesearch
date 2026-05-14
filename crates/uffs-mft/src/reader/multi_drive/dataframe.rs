// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `DataFrame`-backed multi-drive reader helpers.

use alloc::sync::Arc;

use tokio::task::JoinSet;
use uffs_polars::{DataFrame, IntoLazy as _, col, lit};

use super::{DriveReadResult, MultiDriveMftReader, drive_reader_budget};
use crate::error::{MftError, Result};
use crate::reader::{MftProgress, MftReader};

impl MultiDriveMftReader {
    /// Read MFTs from all drives concurrently.
    ///
    /// Returns a merged `DataFrame` with a `drive` column (e.g., "C:", "D:").
    /// If some drives fail, the successful ones are still returned.
    /// Only fails if ALL drives fail.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    pub async fn read_all(&self) -> Result<DataFrame> {
        self.read_all_internal(None::<fn(crate::platform::DriveLetter, MftProgress)>)
            .await
    }

    /// Read MFTs from all drives with per-drive progress callbacks.
    ///
    /// The callback receives `(drive_letter, progress)` for each drive.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called with progress updates for each drive
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    pub async fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(crate::platform::DriveLetter, MftProgress) + Send + Sync + Clone + 'static,
    {
        self.read_all_internal(Some(callback)).await
    }

    /// Internal implementation for concurrent drive reading.
    async fn read_all_internal<F>(&self, callback: Option<F>) -> Result<DataFrame>
    where
        F: Fn(crate::platform::DriveLetter, MftProgress) + Send + Sync + Clone + 'static,
    {
        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        let shared_callback = callback.map(Arc::new);
        let budget = drive_reader_budget(self.drives.len());
        let mut join_set = JoinSet::new();
        let mut pending_drives = self.drives.iter().copied();

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                let cb = shared_callback.clone();

                join_set.spawn(async move {
                    let result = Self::read_single_drive(drive, cb).await;
                    DriveReadResult {
                        drive,
                        dataframe: result.as_ref().ok().cloned(),
                        error: result.err(),
                    }
                });
            }
        }

        let mut dataframes: Vec<DataFrame> = Vec::new();
        let mut errors: Vec<(crate::platform::DriveLetter, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(drive_result) => {
                    if let Some(df) = drive_result.dataframe {
                        let drive_str = format!("{}:", drive_result.drive);
                        let df_with_drive = df
                            .lazy()
                            .with_column(lit(drive_str).alias("drive"))
                            .collect()
                            .map_err(MftError::from)?;
                        dataframes.push(df_with_drive);
                    } else if let Some(err) = drive_result.error {
                        errors.push((drive_result.drive, err));
                    }
                }
                Err(join_err) => {
                    errors.push((
                        crate::platform::DriveLetter::X,
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }

            if let Some(drive) = pending_drives.next() {
                let cb = shared_callback.clone();

                join_set.spawn(async move {
                    let read_result = Self::read_single_drive(drive, cb).await;
                    DriveReadResult {
                        drive,
                        dataframe: read_result.as_ref().ok().cloned(),
                        error: read_result.err(),
                    }
                });
            }
        }

        if dataframes.is_empty() {
            return Err(errors.into_iter().next().map_or_else(
                || MftError::InvalidInput("No drives could be read".into()),
                |(_, error)| error,
            ));
        }

        let mut result = dataframes.remove(0);
        for df in dataframes {
            result = result.vstack(&df).map_err(MftError::from)?;
        }

        let column_names: Vec<String> = result
            .get_column_names()
            .into_iter()
            .filter(|name| name.as_str() != "drive")
            .map(uffs_polars::PlSmallStr::to_string)
            .collect();
        let columns: Vec<_> = core::iter::once("drive".to_owned())
            .chain(column_names)
            .map(|name| col(&name))
            .collect();

        result
            .lazy()
            .select(columns)
            .collect()
            .map_err(MftError::from)
    }

    /// Read a single drive with optional progress callback.
    ///
    /// Uses `spawn_blocking` because `MftReader` contains Windows HANDLEs
    /// which are not `Send`, and the MFT reading is blocking I/O.
    async fn read_single_drive<F>(
        drive: crate::platform::DriveLetter,
        callback: Option<Arc<F>>,
    ) -> Result<DataFrame>
    where
        F: Fn(crate::platform::DriveLetter, MftProgress) + Send + Sync + 'static,
    {
        tokio::task::spawn_blocking(move || {
            let reader = MftReader::open(drive)?;

            callback.map_or_else(
                || reader.read_all(),
                |cb| {
                    reader.read_with_progress(move |progress| {
                        cb(drive, progress);
                    })
                },
            )
        })
        .await
        .map_err(|error| MftError::InvalidInput(format!("Task join error: {error}")))?
    }

    /// Read all drives and return individual results (for detailed error
    /// handling).
    ///
    /// Unlike `read_all()`, this returns results for each drive separately,
    /// allowing the caller to handle partial failures.
    ///
    /// # Errors
    ///
    /// Returns an error only if the operation itself fails (not individual
    /// drives).
    pub async fn read_all_detailed(&self) -> Result<Vec<DriveReadResult>> {
        if self.drives.is_empty() {
            return Ok(Vec::new());
        }

        let budget = drive_reader_budget(self.drives.len());
        let mut join_set = JoinSet::new();
        let mut pending_drives = self.drives.iter().copied();

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                join_set.spawn(async move {
                    let read_result = Self::read_single_drive::<
                        fn(crate::platform::DriveLetter, MftProgress),
                    >(drive, None)
                    .await;
                    DriveReadResult {
                        drive,
                        dataframe: read_result.as_ref().ok().cloned(),
                        error: read_result.err(),
                    }
                });
            }
        }

        let mut results = Vec::with_capacity(self.drives.len());
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(drive_result) => results.push(drive_result),
                Err(join_err) => {
                    results.push(DriveReadResult {
                        drive: crate::platform::DriveLetter::X,
                        dataframe: None,
                        error: Some(MftError::InvalidInput(format!("Task failed: {join_err}"))),
                    });
                }
            }

            if let Some(drive) = pending_drives.next() {
                join_set.spawn(async move {
                    let read_result = Self::read_single_drive::<
                        fn(crate::platform::DriveLetter, MftProgress),
                    >(drive, None)
                    .await;
                    DriveReadResult {
                        drive,
                        dataframe: read_result.as_ref().ok().cloned(),
                        error: read_result.err(),
                    }
                });
            }
        }

        Ok(results)
    }
}
