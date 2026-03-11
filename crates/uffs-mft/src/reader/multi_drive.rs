//! Multi-drive reader orchestration and cache/update helpers.
//! Exception: multi-drive orchestration, cache refresh, and USN update helpers
//! remain co-located pending a dedicated split outside Wave 3C.

#[cfg(windows)]
use std::sync::Arc;

use uffs_polars::DataFrame;

use super::MftProgress;
#[cfg(windows)]
use super::MftReader;
use crate::error::{MftError, Result};

// ============================================================================
// Multi-Drive MFT Reader
// ============================================================================

/// Maximum number of drive-level reader tasks to run at once.
///
/// Each drive reader can already fan out internally via blocking I/O and
/// parsing workers, so we keep cross-drive orchestration conservative to avoid
/// multiplying that parallelism across many volumes at once.
#[cfg(any(windows, test))]
const MAX_CONCURRENT_DRIVE_READERS: usize = 4;

/// Returns the bounded drive-level task budget for multi-drive orchestration.
#[cfg(any(windows, test))]
#[must_use]
fn drive_reader_budget(total_drives: usize) -> usize {
    if total_drives == 0 {
        return 0;
    }

    let hardware_budget = std::thread::available_parallelism()
        .map_or(MAX_CONCURRENT_DRIVE_READERS, core::num::NonZeroUsize::get);

    total_drives
        .min(hardware_budget.max(1))
        .min(MAX_CONCURRENT_DRIVE_READERS)
}

/// Result from reading a single drive.
#[derive(Debug)]
pub struct DriveReadResult {
    /// The drive letter.
    pub drive: char,
    /// The `DataFrame` (if successful).
    pub dataframe: Option<DataFrame>,
    /// The error (if failed).
    pub error: Option<MftError>,
}

/// Reads MFTs from multiple drives concurrently.
///
/// This struct orchestrates parallel reading of MFTs from multiple NTFS
/// volumes, merging the results into a single `DataFrame` with a `drive` column
/// to distinguish the source of each record.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_mft::MultiDriveMftReader;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let reader = MultiDriveMftReader::new(vec!['C', 'D', 'E']);
///     let df = reader.read_all().await?;
///     println!("Found {} files across all drives", df.height());
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct MultiDriveMftReader {
    /// The drive letters to read from.
    drives: Vec<char>,
}

impl MultiDriveMftReader {
    /// Creates a new multi-drive reader.
    ///
    /// # Arguments
    ///
    /// * `drives` - List of drive letters to read (e.g., `vec!['C', 'D', 'E']`)
    #[must_use]
    pub fn new(drives: Vec<char>) -> Self {
        Self {
            drives: drives
                .into_iter()
                .map(|ch| ch.to_ascii_uppercase())
                .collect(),
        }
    }

    /// Returns the list of drives this reader will process.
    #[must_use]
    pub fn drives(&self) -> &[char] {
        &self.drives
    }

    /// Read MFTs from all drives concurrently.
    ///
    /// Returns a merged DataFrame with a `drive` column (e.g., "C:", "D:").
    /// If some drives fail, the successful ones are still returned.
    /// Only fails if ALL drives fail.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    #[tracing::instrument(level = "info", skip(self), fields(drive_count = self.drives.len()))]
    pub async fn read_all(&self) -> Result<DataFrame> {
        self.read_all_internal(None::<fn(char, MftProgress)>).await
    }

    /// Read MFTs from all drives (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_all(&self) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
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
    #[cfg(windows)]
    #[tracing::instrument(
        level = "info",
        skip(self, callback),
        fields(drive_count = self.drives.len(), progress_callback = true)
    )]
    pub async fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        self.read_all_internal(Some(callback)).await
    }

    /// Read MFTs with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_with_progress<F>(&self, _callback: F) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for concurrent drive reading.
    #[cfg(windows)]
    #[tracing::instrument(
        level = "info",
        skip(self, callback),
        fields(
            drive_count = self.drives.len(),
            progress_callback = callback.is_some()
        )
    )]
    async fn read_all_internal<F>(&self, callback: Option<F>) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        use std::sync::Arc;

        use tokio::task::JoinSet;
        use uffs_polars::{IntoLazy, col, lit};

        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        // Wrap callback in Arc for sharing across tasks
        let callback = callback.map(Arc::new);

        // Keep only a bounded number of drive tasks in flight at once.
        let budget = drive_reader_budget(self.drives.len());
        tracing::info!(
            drives = ?self.drives,
            drive_count = self.drives.len(),
            budget,
            progress_callback = callback.is_some(),
            wait_strategy = "join_set + spawn_blocking",
            "Starting multi-drive DataFrame read orchestration"
        );
        let mut pending_drives = self.drives.iter().copied();
        let mut join_set = JoinSet::new();
        let mut drives_dispatched = 0usize;
        let mut drives_completed = 0usize;

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                let cb = callback.clone();
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "initial",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued drive DataFrame read"
                );

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

        // Collect results
        let mut dataframes: Vec<DataFrame> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while !join_set.is_empty() {
            tracing::debug!(
                drives_completed,
                drives_dispatched,
                drive_count = self.drives.len(),
                in_flight = drives_dispatched.saturating_sub(drives_completed),
                wait_strategy = "join_next",
                "Waiting for next DataFrame drive read"
            );

            let Some(result) = join_set.join_next().await else {
                break;
            };

            drives_completed += 1;
            match result {
                Ok(drive_result) => {
                    if let Some(df) = drive_result.dataframe {
                        tracing::debug!(
                            drive = %drive_result.drive,
                            rows = df.height(),
                            "Drive DataFrame read completed"
                        );
                        // Add "drive" column
                        let drive_str = format!("{}:", drive_result.drive);
                        let df_with_drive = df
                            .lazy()
                            .with_column(lit(drive_str).alias("drive"))
                            .collect()
                            .map_err(MftError::from)?;
                        dataframes.push(df_with_drive);
                    } else if let Some(err) = drive_result.error {
                        tracing::warn!(
                            drive = %drive_result.drive,
                            error = %err,
                            "Drive DataFrame read failed"
                        );
                        errors.push((drive_result.drive, err));
                    }
                }
                Err(join_err) => {
                    tracing::warn!(
                        error = %join_err,
                        drives_completed,
                        drives_dispatched,
                        "DataFrame drive task join failed"
                    );
                    // Task panicked or was cancelled
                    errors.push(('?', MftError::from_join_error("read_all", &join_err)));
                }
            }

            if let Some(drive) = pending_drives.next() {
                let cb = callback.clone();
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "replenish",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued drive DataFrame read"
                );

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

        // If no DataFrames were collected, return the first error
        if dataframes.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        let successful_drives = dataframes.len();
        let failed_drives = errors.len();

        // Concatenate all DataFrames using vstack
        let mut result = dataframes.remove(0);
        for df in dataframes {
            result = result.vstack(&df).map_err(MftError::from)?;
        }

        // Reorder columns to put "drive" first
        let column_names: Vec<String> = result
            .get_column_names()
            .into_iter()
            .filter(|c| c.as_str() != "drive")
            .map(|c| c.to_string())
            .collect();
        let columns: Vec<_> = std::iter::once("drive".to_string())
            .chain(column_names)
            .map(|s| col(&s))
            .collect();

        let final_result = result
            .lazy()
            .select(columns)
            .collect()
            .map_err(MftError::from)?;

        tracing::info!(
            drive_count = self.drives.len(),
            successful_drives,
            failed_drives,
            rows = final_result.height(),
            "Completed multi-drive DataFrame read orchestration"
        );

        Ok(final_result)
    }

    /// Read a single drive with optional progress callback.
    ///
    /// Uses `spawn_blocking` because `MftReader` contains Windows HANDLEs
    /// which are not `Send`, and the MFT reading is blocking I/O.
    #[cfg(windows)]
    async fn read_single_drive<F>(drive: char, callback: Option<Arc<F>>) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static,
    {
        let progress_callback = callback.is_some();
        tracing::debug!(
            drive = %drive,
            progress_callback,
            wait_strategy = "spawn_blocking",
            "Dispatching DataFrame drive read to blocking pool"
        );

        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        // This avoids blocking the async runtime and prevents nested runtime panics.
        let dataframe = tokio::task::spawn_blocking(move || {
            let reader = MftReader::open(drive)?;

            if let Some(cb) = callback {
                reader.read_with_progress(move |progress| {
                    cb(drive, progress);
                })
            } else {
                reader.read_all()
            }
        })
        .await
        .map_err(|error| MftError::from_join_error("read_single_drive", &error))?;

        tracing::debug!(
            drive = %drive,
            progress_callback,
            rows = dataframe.height(),
            "Blocking DataFrame drive read completed"
        );

        Ok(dataframe)
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
    #[cfg(windows)]
    pub async fn read_all_detailed(&self) -> Result<Vec<DriveReadResult>> {
        use tokio::task::JoinSet;

        if self.drives.is_empty() {
            return Ok(Vec::new());
        }

        let budget = drive_reader_budget(self.drives.len());
        tracing::info!(
            drives = ?self.drives,
            drive_count = self.drives.len(),
            budget,
            wait_strategy = "join_set + spawn_blocking",
            "Starting multi-drive detailed read orchestration"
        );
        let mut pending_drives = self.drives.iter().copied();
        let mut join_set = JoinSet::new();
        let mut drives_dispatched = 0usize;
        let mut drives_completed = 0usize;

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "initial",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued detailed drive read"
                );
                join_set.spawn(async move {
                    let result =
                        Self::read_single_drive::<fn(char, MftProgress)>(drive, None).await;
                    DriveReadResult {
                        drive,
                        dataframe: result.as_ref().ok().cloned(),
                        error: result.err(),
                    }
                });
            }
        }

        let mut results = Vec::with_capacity(self.drives.len());
        while !join_set.is_empty() {
            tracing::debug!(
                drives_completed,
                drives_dispatched,
                drive_count = self.drives.len(),
                in_flight = drives_dispatched.saturating_sub(drives_completed),
                wait_strategy = "join_next",
                "Waiting for next detailed drive read"
            );

            let Some(result) = join_set.join_next().await else {
                break;
            };

            drives_completed += 1;
            match result {
                Ok(drive_result) => results.push(drive_result),
                Err(join_err) => {
                    tracing::warn!(
                        error = %join_err,
                        drives_completed,
                        drives_dispatched,
                        "Detailed drive task join failed"
                    );
                    results.push(DriveReadResult {
                        drive: '?',
                        dataframe: None,
                        error: Some(MftError::from_join_error("read_all_detailed", &join_err)),
                    });
                }
            }

            if let Some(drive) = pending_drives.next() {
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "replenish",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued detailed drive read"
                );
                join_set.spawn(async move {
                    let result =
                        Self::read_single_drive::<fn(char, MftProgress)>(drive, None).await;
                    DriveReadResult {
                        drive,
                        dataframe: result.as_ref().ok().cloned(),
                        error: result.err(),
                    }
                });
            }
        }

        tracing::info!(
            drive_count = self.drives.len(),
            results = results.len(),
            "Completed multi-drive detailed read orchestration"
        );

        Ok(results)
    }

    /// Read all drives detailed (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_all_detailed(&self) -> Result<Vec<DriveReadResult>> {
        Err(MftError::PlatformNotSupported)
    }

    // =========================================================================
    // Lean Index Methods (Optimized Path)
    // =========================================================================

    /// Read MFTs from all drives concurrently into lean `MftIndex` structures.
    ///
    /// This is the optimized path that uses `SlidingIocpInline` with parallel
    /// parsing for maximum performance. Returns a vector of `MftIndex` objects,
    /// one per drive.
    ///
    /// If some drives fail, the successful ones are still returned.
    /// Only fails if ALL drives fail.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all_index(&self) -> Result<Vec<crate::index::MftIndex>> {
        self.read_all_index_internal(None::<fn(char, MftProgress)>)
            .await
    }

    /// Read MFTs from all drives into lean index (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_all_index(&self) -> Result<Vec<crate::index::MftIndex>> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFTs from all drives with progress callbacks into lean index.
    ///
    /// The callback receives `(drive_letter, progress)` for each drive.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all_index_with_progress<F>(
        &self,
        callback: F,
    ) -> Result<Vec<crate::index::MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        self.read_all_index_internal(Some(callback)).await
    }

    /// Read MFTs with progress into lean index (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_all_index_with_progress<F>(
        &self,
        _callback: F,
    ) -> Result<Vec<crate::index::MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFTs from all drives with cache support.
    ///
    /// For each drive:
    /// - If cache is fresh (within TTL), use cached index
    /// - If cache is stale or missing, read from disk and update cache
    ///
    /// This provides the best of both worlds: fast startup when cache is valid,
    /// and automatic refresh when needed.
    ///
    /// # Arguments
    ///
    /// * `ttl_seconds` - Time-to-live for cache entries (use
    ///   `INDEX_TTL_SECONDS` for default)
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all_index_cached(
        &self,
        ttl_seconds: u64,
    ) -> Result<Vec<crate::index::MftIndex>> {
        use tokio::task::JoinSet;

        use crate::cache::{CacheStatus, check_cache_status};

        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        let budget = drive_reader_budget(self.drives.len());
        tracing::info!(
            drives = ?self.drives,
            drive_count = self.drives.len(),
            budget,
            cache_ttl_seconds = ttl_seconds,
            wait_strategy = "join_set + cache_decision + spawn_blocking",
            "Starting multi-drive cached index orchestration"
        );
        let mut pending_drives = self.drives.iter().copied();
        let mut join_set: JoinSet<(char, Result<crate::index::MftIndex>)> = JoinSet::new();
        let mut drives_dispatched = 0usize;
        let mut drives_completed = 0usize;

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                let ttl = ttl_seconds;
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "initial",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued cached index drive read"
                );

                join_set.spawn(async move {
                    // Check cache first
                    let cache_result = check_cache_status(drive, ttl);

                    let read_result = match cache_result {
                        CacheStatus::Fresh {
                            index,
                            header,
                            age_seconds,
                        } => {
                            tracing::info!(
                                drive = %drive,
                                cache_decision = "fresh",
                                age_seconds,
                                records = index.len(),
                                refresh_strategy = "usn_incremental",
                                "Cache fresh; checking USN journal for incremental refresh"
                            );
                            // Apply USN changes to bring index up to date
                            Self::apply_usn_updates_to_cached_index(drive, index, header).await
                        }
                        CacheStatus::Stale { age_seconds } => {
                            tracing::info!(
                                drive = %drive,
                                cache_decision = "stale",
                                age_seconds = ?age_seconds,
                                refresh_strategy = "full_rebuild",
                                "Cache stale; rebuilding index"
                            );
                            Self::read_and_cache_single_drive(drive).await
                        }
                        CacheStatus::Missing => {
                            tracing::info!(
                                drive = %drive,
                                cache_decision = "missing",
                                refresh_strategy = "full_rebuild",
                                "Cache missing; building index"
                            );
                            Self::read_and_cache_single_drive(drive).await
                        }
                    };

                    (drive, read_result)
                });
            }
        }

        // Collect results
        let mut indices: Vec<crate::index::MftIndex> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while !join_set.is_empty() {
            tracing::debug!(
                drives_completed,
                drives_dispatched,
                drive_count = self.drives.len(),
                in_flight = drives_dispatched.saturating_sub(drives_completed),
                wait_strategy = "join_next",
                "Waiting for next cached index drive result"
            );

            let Some(result) = join_set.join_next().await else {
                break;
            };

            drives_completed += 1;
            match result {
                Ok((drive, Ok(index))) => {
                    tracing::debug!(
                        drive = %drive,
                        records = index.len(),
                        "Cached index drive read completed"
                    );
                    indices.push(index);
                }
                Ok((drive, Err(error))) => {
                    tracing::warn!(
                        drive = %drive,
                        error = %error,
                        "Cached index drive read failed"
                    );
                    errors.push((drive, error));
                }
                Err(join_err) => {
                    tracing::warn!(
                        error = %join_err,
                        drives_completed,
                        drives_dispatched,
                        "Cached index drive task join failed"
                    );
                    errors.push((
                        '?',
                        MftError::from_join_error("read_all_index_cached", &join_err),
                    ));
                }
            }

            if let Some(drive) = pending_drives.next() {
                let ttl = ttl_seconds;
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "replenish",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued cached index drive read"
                );

                join_set.spawn(async move {
                    // Check cache first
                    let cache_result = check_cache_status(drive, ttl);

                    let read_result = match cache_result {
                        CacheStatus::Fresh {
                            index,
                            header,
                            age_seconds,
                        } => {
                            tracing::info!(
                                drive = %drive,
                                cache_decision = "fresh",
                                age_seconds,
                                records = index.len(),
                                refresh_strategy = "usn_incremental",
                                "Cache fresh; checking USN journal for incremental refresh"
                            );
                            // Apply USN changes to bring index up to date
                            Self::apply_usn_updates_to_cached_index(drive, index, header).await
                        }
                        CacheStatus::Stale { age_seconds } => {
                            tracing::info!(
                                drive = %drive,
                                cache_decision = "stale",
                                age_seconds = ?age_seconds,
                                refresh_strategy = "full_rebuild",
                                "Cache stale; rebuilding index"
                            );
                            Self::read_and_cache_single_drive(drive).await
                        }
                        CacheStatus::Missing => {
                            tracing::info!(
                                drive = %drive,
                                cache_decision = "missing",
                                refresh_strategy = "full_rebuild",
                                "Cache missing; building index"
                            );
                            Self::read_and_cache_single_drive(drive).await
                        }
                    };

                    (drive, read_result)
                });
            }
        }

        // If no indices were collected, return the first error
        if indices.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        tracing::info!(
            drive_count = self.drives.len(),
            successful_drives = indices.len(),
            failed_drives = errors.len(),
            "Completed multi-drive cached index orchestration"
        );

        Ok(indices)
    }

    /// Read MFTs with cache support (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_all_index_cached(
        &self,
        _ttl_seconds: u64,
    ) -> Result<Vec<crate::index::MftIndex>> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for concurrent lean index reading.
    #[cfg(windows)]
    async fn read_all_index_internal<F>(
        &self,
        callback: Option<F>,
    ) -> Result<Vec<crate::index::MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        use std::sync::Arc;

        use tokio::task::JoinSet;

        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        // Wrap callback in Arc for sharing across tasks
        let callback = callback.map(Arc::new);

        // Keep only a bounded number of drive tasks in flight at once.
        let budget = drive_reader_budget(self.drives.len());
        tracing::info!(
            drives = ?self.drives,
            drive_count = self.drives.len(),
            budget,
            progress_callback = callback.is_some(),
            wait_strategy = "join_set + spawn_blocking",
            "Starting multi-drive index read orchestration"
        );
        let mut pending_drives = self.drives.iter().copied();
        let mut join_set: JoinSet<(char, Result<crate::index::MftIndex>)> = JoinSet::new();
        let mut drives_dispatched = 0usize;
        let mut drives_completed = 0usize;

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                let cb = callback.clone();
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "initial",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued drive index read"
                );

                join_set
                    .spawn(async move { (drive, Self::read_single_drive_index(drive, cb).await) });
            }
        }

        // Collect results
        let mut indices: Vec<crate::index::MftIndex> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while !join_set.is_empty() {
            tracing::debug!(
                drives_completed,
                drives_dispatched,
                drive_count = self.drives.len(),
                in_flight = drives_dispatched.saturating_sub(drives_completed),
                wait_strategy = "join_next",
                "Waiting for next index drive read"
            );

            let Some(result) = join_set.join_next().await else {
                break;
            };

            drives_completed += 1;
            match result {
                Ok((drive, Ok(index))) => {
                    tracing::debug!(
                        drive = %drive,
                        records = index.len(),
                        "Drive index read completed"
                    );
                    indices.push(index);
                }
                Ok((drive, Err(error))) => {
                    tracing::warn!(
                        drive = %drive,
                        error = %error,
                        "Drive index read failed"
                    );
                    errors.push((drive, error));
                }
                Err(join_err) => {
                    tracing::warn!(
                        error = %join_err,
                        drives_completed,
                        drives_dispatched,
                        "Index drive task join failed"
                    );
                    errors.push((
                        '?',
                        MftError::from_join_error("read_all_index_with_progress", &join_err),
                    ));
                }
            }

            if let Some(drive) = pending_drives.next() {
                let cb = callback.clone();
                drives_dispatched += 1;
                tracing::debug!(
                    drive = %drive,
                    dispatch_reason = "replenish",
                    drives_dispatched,
                    drive_count = self.drives.len(),
                    "Queued drive index read"
                );

                join_set
                    .spawn(async move { (drive, Self::read_single_drive_index(drive, cb).await) });
            }
        }

        // If no indices were collected, return the first error
        if indices.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        tracing::info!(
            drive_count = self.drives.len(),
            successful_drives = indices.len(),
            failed_drives = errors.len(),
            "Completed multi-drive index read orchestration"
        );

        Ok(indices)
    }

    /// Read a single drive into lean index with optional progress callback.
    #[cfg(windows)]
    async fn read_single_drive_index<F>(
        drive: char,
        callback: Option<Arc<F>>,
    ) -> Result<crate::index::MftIndex>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static,
    {
        let progress_callback = callback.is_some();
        tracing::debug!(
            drive = %drive,
            progress_callback,
            wait_strategy = "spawn_blocking",
            "Dispatching index drive read to blocking pool"
        );

        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        let index = tokio::task::spawn_blocking(move || {
            let reader = MftReader::open(drive)?;

            if let Some(cb) = callback {
                reader.read_index_with_progress_sync(move |progress| {
                    cb(drive, progress);
                })
            } else {
                reader.read_all_index_sync()
            }
        })
        .await
        .map_err(|error| MftError::from_join_error("read_single_drive_index", &error))?;

        tracing::debug!(
            drive = %drive,
            progress_callback,
            records = index.len(),
            "Blocking index drive read completed"
        );

        Ok(index)
    }

    /// Read a single drive and save to cache.
    #[cfg(windows)]
    async fn read_and_cache_single_drive(drive: char) -> Result<crate::index::MftIndex> {
        tracing::debug!(
            drive = %drive,
            refresh_strategy = "full_rebuild",
            wait_strategy = "spawn_blocking",
            "Dispatching full index rebuild to blocking pool"
        );

        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        let index =
            tokio::task::spawn_blocking(move || Self::read_and_cache_single_drive_sync(drive))
                .await
                .map_err(|error| {
                    MftError::from_join_error("read_and_cache_single_drive", &error)
                })?;

        tracing::debug!(
            drive = %drive,
            records = index.len(),
            refresh_strategy = "full_rebuild",
            "Blocking full index rebuild completed"
        );

        Ok(index)
    }

    /// Synchronous implementation of read_and_cache_single_drive.
    #[cfg(windows)]
    fn read_and_cache_single_drive_sync(drive: char) -> Result<crate::index::MftIndex> {
        use crate::cache::save_to_cache;
        use crate::platform::VolumeHandle;
        use crate::usn::query_usn_journal;

        tracing::info!(
            drive = %drive,
            refresh_strategy = "full_rebuild",
            cache_write = true,
            "Reading full index for cache refresh"
        );

        let reader = MftReader::open(drive)?;
        let index = reader.read_all_index_sync()?;

        // Get volume info for caching
        let handle = VolumeHandle::open(drive)?;
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
            Ok(info) => (info.journal_id, info.next_usn),
            Err(error) => {
                tracing::debug!(
                    drive = %drive,
                    error = %error,
                    fallback = "zeroed_usn_checkpoint",
                    "USN journal metadata unavailable while preparing cache header"
                );
                (0, 0)
            }
        };

        // Save to cache
        if let Err(e) = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn) {
            // Log but don't fail - caching is optional
            tracing::info!(drive = %drive, error = %e, "⚠️ Failed to save to cache");
        } else {
            tracing::info!(drive = %drive, records = index.len(), "💾 Saved to cache");
        }

        Ok(index)
    }

    /// Apply USN Journal updates to a cached index to bring it up to date.
    ///
    /// This reads changes from the USN Journal since the cached checkpoint,
    /// applies them to the index, and saves the updated index back to cache.
    ///
    /// If USN Journal is unavailable or the journal has wrapped, falls back
    /// to a full rebuild.
    #[cfg(windows)]
    async fn apply_usn_updates_to_cached_index(
        drive: char,
        index: crate::index::MftIndex,
        header: crate::index::IndexHeader,
    ) -> Result<crate::index::MftIndex> {
        tracing::debug!(
            drive = %drive,
            cached_records = index.len(),
            cached_next_usn = header.next_usn,
            cached_journal_id = header.usn_journal_id,
            refresh_strategy = "usn_incremental",
            wait_strategy = "spawn_blocking",
            "Dispatching cached index USN refresh to blocking pool"
        );

        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        let refreshed_index = tokio::task::spawn_blocking(move || {
            Self::apply_usn_updates_to_cached_index_sync(drive, index, header)
        })
        .await
        .map_err(|error| MftError::from_join_error("apply_usn_updates_to_cached_index", &error))?;

        tracing::debug!(
            drive = %drive,
            records = refreshed_index.len(),
            refresh_strategy = "usn_incremental",
            "Blocking cached index USN refresh completed"
        );

        Ok(refreshed_index)
    }

    /// Synchronous implementation of apply_usn_updates_to_cached_index.
    #[cfg(windows)]
    fn apply_usn_updates_to_cached_index_sync(
        drive: char,
        mut index: crate::index::MftIndex,
        header: crate::index::IndexHeader,
    ) -> Result<crate::index::MftIndex> {
        use crate::cache::save_to_cache;
        use crate::platform::VolumeHandle;
        use crate::usn::{aggregate_changes, query_usn_journal, read_usn_journal};

        tracing::info!(
            drive = %drive,
            cached_records = index.len(),
            cached_next_usn = header.next_usn,
            cached_journal_id = header.usn_journal_id,
            refresh_strategy = "usn_incremental",
            "Refreshing cached index from USN journal"
        );

        // Query current USN Journal state
        let current_info = match query_usn_journal(drive) {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!(
                    drive = %drive,
                    error = %e,
                    fallback = "use_cached_index_as_is",
                    "⚠️ USN Journal unavailable - using cached index as-is"
                );
                return Ok(index);
            }
        };

        // Check if journal ID matches (journal may have been recreated)
        if header.usn_journal_id != 0 && current_info.journal_id != header.usn_journal_id {
            tracing::info!(
                drive = %drive,
                cached_journal_id = header.usn_journal_id,
                current_journal_id = current_info.journal_id,
                refresh_strategy = "full_rebuild",
                reason = "journal_id_changed",
                "🔄 USN Journal ID changed - rebuilding index"
            );
            // Journal was recreated, need full rebuild
            return Self::read_and_cache_single_drive_sync(drive);
        }

        // Check if our checkpoint is still valid (not before first_usn)
        let start_usn = header.next_usn;
        if start_usn < current_info.first_usn {
            tracing::info!(
                drive = %drive,
                cached_usn = start_usn,
                first_usn = current_info.first_usn,
                refresh_strategy = "full_rebuild",
                reason = "journal_wrapped",
                "🔄 USN Journal wrapped - rebuilding index"
            );
            // Journal wrapped, need full rebuild
            return Self::read_and_cache_single_drive_sync(drive);
        }

        // If we're already at the latest USN, no changes needed
        if start_usn >= current_info.next_usn {
            tracing::debug!(
                drive = %drive,
                usn = start_usn,
                refresh_strategy = "no_op",
                "✅ Index is already up to date"
            );
            return Ok(index);
        }

        // Read USN changes since our checkpoint
        let (records, next_usn) = match read_usn_journal(drive, current_info.journal_id, start_usn)
        {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(
                    drive = %drive,
                    error = %e,
                    fallback = "use_cached_index_as_is",
                    "⚠️ Failed to read USN Journal - using cached index as-is"
                );
                return Ok(index);
            }
        };

        if records.is_empty() {
            tracing::debug!(
                drive = %drive,
                refresh_strategy = "no_op",
                "✅ No USN changes since last cache"
            );
            return Ok(index);
        }

        // Aggregate changes (deduplicate by FRS)
        let changes_map = aggregate_changes(&records);
        let changes: Vec<_> = changes_map.into_values().collect();
        tracing::info!(
            drive = %drive,
            usn_records = changes.len(),
            from_usn = start_usn,
            to_usn = next_usn,
            refresh_strategy = "usn_incremental",
            "🔧 Applying USN changes"
        );

        // Apply changes to index
        let stats = index.apply_usn_changes(&changes);
        tracing::debug!(
            drive = %drive,
            created = stats.created,
            deleted = stats.deleted,
            modified = stats.modified,
            skipped = stats.skipped,
            "📊 USN changes applied"
        );

        // Recompute tree metrics after structural changes
        tracing::debug!(drive = %drive, "🔨 Recomputing tree metrics after USN updates");
        index.compute_tree_metrics();

        // Save updated index to cache with new checkpoint
        let handle = match VolumeHandle::open(drive) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(
                    drive = %drive,
                    error = %e,
                    fallback = "return_updated_index_without_cache_write",
                    "⚠️ Failed to open volume for cache update"
                );
                return Ok(index);
            }
        };
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        if let Err(e) = save_to_cache(
            &index,
            drive,
            volume_serial,
            current_info.journal_id,
            next_usn,
        ) {
            tracing::warn!(
                drive = %drive,
                error = %e,
                "⚠️ Failed to update cache"
            );
        } else {
            tracing::debug!(
                drive = %drive,
                next_usn,
                "💾 Cache updated with new USN checkpoint"
            );
        }

        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONCURRENT_DRIVE_READERS, drive_reader_budget};

    #[test]
    fn drive_reader_budget_handles_empty_input() {
        assert_eq!(drive_reader_budget(0), 0);
    }

    #[test]
    fn drive_reader_budget_never_exceeds_drive_count() {
        assert_eq!(drive_reader_budget(1), 1);
        assert!(drive_reader_budget(3) <= 3);
    }

    #[test]
    fn drive_reader_budget_caps_drive_fan_out() {
        assert!(
            drive_reader_budget(MAX_CONCURRENT_DRIVE_READERS + 8) <= MAX_CONCURRENT_DRIVE_READERS
        );
    }
}
