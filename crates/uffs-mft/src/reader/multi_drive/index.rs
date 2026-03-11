//! Lean-index multi-drive reader helpers.

use std::sync::Arc;

use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use super::{MultiDriveMftReader, drive_reader_budget};
use crate::cache::{CacheStatus, check_cache_status, save_to_cache};
use crate::error::{MftError, Result};
use crate::index::{IndexHeader, MftIndex};
use crate::platform::VolumeHandle;
use crate::reader::{MftProgress, MftReader};
use crate::usn::{aggregate_changes, query_usn_journal, read_usn_journal};

impl MultiDriveMftReader {
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
    pub async fn read_all_index(&self) -> Result<Vec<MftIndex>> {
        self.read_all_index_internal(None::<fn(char, MftProgress)>)
            .await
    }

    /// Read MFTs from all drives with progress callbacks into lean index.
    ///
    /// The callback receives `(drive_letter, progress)` for each drive.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    pub async fn read_all_index_with_progress<F>(&self, callback: F) -> Result<Vec<MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        self.read_all_index_internal(Some(callback)).await
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
    pub async fn read_all_index_cached(&self, ttl_seconds: u64) -> Result<Vec<MftIndex>> {
        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        let budget = drive_reader_budget(self.drives.len());
        let mut join_set = JoinSet::new();
        let mut pending_drives = self.drives.iter().copied();

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                let ttl = ttl_seconds;

                join_set.spawn(async move {
                    match check_cache_status(drive, ttl) {
                        CacheStatus::Fresh {
                            index,
                            header,
                            age_seconds,
                        } => {
                            info!(
                                drive = %drive,
                                age_seconds,
                                records = index.len(),
                                "📦 Cache HIT - applying USN updates"
                            );
                            Self::apply_usn_updates_to_cached_index(drive, index, header).await
                        }
                        CacheStatus::Stale { age_seconds } => {
                            info!(
                                drive = %drive,
                                age_seconds = ?age_seconds,
                                "🔄 Cache STALE - rebuilding index"
                            );
                            Self::read_and_cache_single_drive(drive).await
                        }
                        CacheStatus::Missing => {
                            info!(drive = %drive, "🆕 Cache MISS - building index");
                            Self::read_and_cache_single_drive(drive).await
                        }
                    }
                });
            }
        }

        let mut indices: Vec<MftIndex> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(index)) => indices.push(index),
                Ok(Err(error)) => errors.push(('?', error)),
                Err(join_err) => {
                    errors.push((
                        '?',
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }

            if let Some(drive) = pending_drives.next() {
                let ttl = ttl_seconds;

                join_set.spawn(async move {
                    match check_cache_status(drive, ttl) {
                        CacheStatus::Fresh {
                            index,
                            header,
                            age_seconds,
                        } => {
                            info!(
                                drive = %drive,
                                age_seconds,
                                records = index.len(),
                                "📦 Cache HIT - applying USN updates"
                            );
                            Self::apply_usn_updates_to_cached_index(drive, index, header).await
                        }
                        CacheStatus::Stale { age_seconds } => {
                            info!(
                                drive = %drive,
                                age_seconds = ?age_seconds,
                                "🔄 Cache STALE - rebuilding index"
                            );
                            Self::read_and_cache_single_drive(drive).await
                        }
                        CacheStatus::Missing => {
                            info!(drive = %drive, "🆕 Cache MISS - building index");
                            Self::read_and_cache_single_drive(drive).await
                        }
                    }
                });
            }
        }

        if indices.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, error)| error)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        Ok(indices)
    }

    /// Internal implementation for concurrent lean index reading.
    async fn read_all_index_internal<F>(&self, callback: Option<F>) -> Result<Vec<MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        let callback = callback.map(Arc::new);
        let budget = drive_reader_budget(self.drives.len());
        let mut join_set = JoinSet::new();
        let mut pending_drives = self.drives.iter().copied();

        for _ in 0..budget {
            if let Some(drive) = pending_drives.next() {
                let cb = callback.clone();
                join_set.spawn(async move { Self::read_single_drive_index(drive, cb).await });
            }
        }

        let mut indices: Vec<MftIndex> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(index)) => indices.push(index),
                Ok(Err(error)) => errors.push(('?', error)),
                Err(join_err) => {
                    errors.push((
                        '?',
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }

            if let Some(drive) = pending_drives.next() {
                let cb = callback.clone();
                join_set.spawn(async move { Self::read_single_drive_index(drive, cb).await });
            }
        }

        if indices.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, error)| error)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        Ok(indices)
    }

    /// Read a single drive into lean index with optional progress callback.
    async fn read_single_drive_index<F>(drive: char, callback: Option<Arc<F>>) -> Result<MftIndex>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static,
    {
        tokio::task::spawn_blocking(move || {
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
        .map_err(|error| MftError::InvalidInput(format!("Task join error: {error}")))?
    }

    /// Read a single drive and save to cache.
    async fn read_and_cache_single_drive(drive: char) -> Result<MftIndex> {
        tokio::task::spawn_blocking(move || Self::read_and_cache_single_drive_sync(drive))
            .await
            .map_err(|error| MftError::InvalidInput(format!("Task join error: {error}")))?
    }

    /// Synchronous implementation of `read_and_cache_single_drive`.
    fn read_and_cache_single_drive_sync(drive: char) -> Result<MftIndex> {
        let reader = MftReader::open(drive)?;
        let index = reader.read_all_index_sync()?;

        let handle = VolumeHandle::open(drive)?;
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
            Ok(info) => (info.journal_id, info.next_usn),
            Err(_) => (0, 0),
        };

        if let Err(error) = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn) {
            info!(drive = %drive, error = %error, "⚠️ Failed to save to cache");
        } else {
            info!(drive = %drive, records = index.len(), "💾 Saved to cache");
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
    async fn apply_usn_updates_to_cached_index(
        drive: char,
        index: MftIndex,
        header: IndexHeader,
    ) -> Result<MftIndex> {
        tokio::task::spawn_blocking(move || {
            Self::apply_usn_updates_to_cached_index_sync(drive, index, header)
        })
        .await
        .map_err(|error| MftError::InvalidInput(format!("Task join error: {error}")))?
    }

    /// Synchronous implementation of `apply_usn_updates_to_cached_index`.
    fn apply_usn_updates_to_cached_index_sync(
        drive: char,
        mut index: MftIndex,
        header: IndexHeader,
    ) -> Result<MftIndex> {
        let current_info = match query_usn_journal(drive) {
            Ok(info) => info,
            Err(error) => {
                warn!(
                    drive = %drive,
                    error = %error,
                    "⚠️ USN Journal unavailable - using cached index as-is"
                );
                return Ok(index);
            }
        };

        if header.usn_journal_id != 0 && current_info.journal_id != header.usn_journal_id {
            info!(
                drive = %drive,
                cached_journal_id = header.usn_journal_id,
                current_journal_id = current_info.journal_id,
                "🔄 USN Journal ID changed - rebuilding index"
            );
            return Self::read_and_cache_single_drive_sync(drive);
        }

        let start_usn = header.next_usn;
        if start_usn < current_info.first_usn {
            info!(
                drive = %drive,
                cached_usn = start_usn,
                first_usn = current_info.first_usn,
                "🔄 USN Journal wrapped - rebuilding index"
            );
            return Self::read_and_cache_single_drive_sync(drive);
        }

        if start_usn >= current_info.next_usn {
            debug!(drive = %drive, usn = start_usn, "✅ Index is already up to date");
            return Ok(index);
        }

        let (records, next_usn) = match read_usn_journal(drive, current_info.journal_id, start_usn)
        {
            Ok(result) => result,
            Err(error) => {
                warn!(
                    drive = %drive,
                    error = %error,
                    "⚠️ Failed to read USN Journal - using cached index as-is"
                );
                return Ok(index);
            }
        };

        if records.is_empty() {
            debug!(drive = %drive, "✅ No USN changes since last cache");
            return Ok(index);
        }

        let changes_map = aggregate_changes(&records);
        let changes: Vec<_> = changes_map.into_values().collect();
        info!(
            drive = %drive,
            usn_records = changes.len(),
            from_usn = start_usn,
            to_usn = next_usn,
            "🔧 Applying USN changes"
        );

        let stats = index.apply_usn_changes(&changes);
        debug!(
            drive = %drive,
            created = stats.created,
            deleted = stats.deleted,
            modified = stats.modified,
            skipped = stats.skipped,
            "📊 USN changes applied"
        );

        debug!(drive = %drive, "🔨 Recomputing tree metrics after USN updates");
        index.compute_tree_metrics();

        let handle = match VolumeHandle::open(drive) {
            Ok(handle) => handle,
            Err(error) => {
                warn!(
                    drive = %drive,
                    error = %error,
                    "⚠️ Failed to open volume for cache update"
                );
                return Ok(index);
            }
        };
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        if let Err(error) = save_to_cache(
            &index,
            drive,
            volume_serial,
            current_info.journal_id,
            next_usn,
        ) {
            warn!(drive = %drive, error = %error, "⚠️ Failed to update cache");
        } else {
            debug!(
                drive = %drive,
                next_usn,
                "💾 Cache updated with new USN checkpoint"
            );
        }

        Ok(index)
    }
}
