// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Lean-index multi-drive reader helpers.

use alloc::sync::Arc;

use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use super::{MultiDriveMftReader, drive_reader_budget};
use crate::cache::{CacheStatus, check_cache_status, save_to_cache};
use crate::error::{MftError, Result};
use crate::index::{IndexHeader, MftIndex};
use crate::platform::VolumeHandle;
use crate::reader::usn_apply::{
    UsnDecision, apply_targeted_usn_reads, classify_usn_state as classify_with_journal_info,
    persist_usn_checkpoint, rebuild_derived_after_usn,
};
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
        self.read_all_index_internal(None::<fn(crate::platform::DriveLetter, MftProgress)>)
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
        F: Fn(crate::platform::DriveLetter, MftProgress) + Send + Sync + Clone + 'static,
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
        let mut errors: Vec<(crate::platform::DriveLetter, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(index)) => indices.push(index),
                Ok(Err(error)) => errors.push((crate::platform::DriveLetter::X, error)),
                Err(join_err) => {
                    errors.push((
                        crate::platform::DriveLetter::X,
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
            return Err(errors.into_iter().next().map_or_else(
                || MftError::InvalidInput("No drives could be read".into()),
                |(_, error)| error,
            ));
        }

        Ok(indices)
    }

    /// Internal implementation for concurrent lean index reading.
    async fn read_all_index_internal<F>(&self, callback: Option<F>) -> Result<Vec<MftIndex>>
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
                join_set.spawn(async move { Self::read_single_drive_index(drive, cb).await });
            }
        }

        let mut indices: Vec<MftIndex> = Vec::new();
        let mut errors: Vec<(crate::platform::DriveLetter, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(index)) => indices.push(index),
                Ok(Err(error)) => errors.push((crate::platform::DriveLetter::X, error)),
                Err(join_err) => {
                    errors.push((
                        crate::platform::DriveLetter::X,
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }

            if let Some(drive) = pending_drives.next() {
                let cb = shared_callback.clone();
                join_set.spawn(async move { Self::read_single_drive_index(drive, cb).await });
            }
        }

        if indices.is_empty() {
            return Err(errors.into_iter().next().map_or_else(
                || MftError::InvalidInput("No drives could be read".into()),
                |(_, error)| error,
            ));
        }

        Ok(indices)
    }

    /// Read a single drive into lean index with optional progress callback.
    async fn read_single_drive_index<F>(
        drive: crate::platform::DriveLetter,
        callback: Option<Arc<F>>,
    ) -> Result<MftIndex>
    where
        F: Fn(crate::platform::DriveLetter, MftProgress) + Send + Sync + 'static,
    {
        tokio::task::spawn_blocking(move || {
            let reader = MftReader::open(drive)?;

            callback.map_or_else(
                || reader.read_all_index_sync(),
                |cb| {
                    reader.read_index_with_progress_sync(move |progress| {
                        cb(drive, progress);
                    })
                },
            )
        })
        .await
        .map_err(|error| MftError::InvalidInput(format!("Task join error: {error}")))?
    }

    /// Read a single drive and save to cache.
    async fn read_and_cache_single_drive(drive: crate::platform::DriveLetter) -> Result<MftIndex> {
        tokio::task::spawn_blocking(move || Self::read_and_cache_single_drive_sync(drive))
            .await
            .map_err(|error| MftError::InvalidInput(format!("Task join error: {error}")))?
    }

    /// Synchronous implementation of `read_and_cache_single_drive`.
    fn read_and_cache_single_drive_sync(drive: crate::platform::DriveLetter) -> Result<MftIndex> {
        let reader = MftReader::open(drive)?;
        let index = reader.read_all_index_sync()?;

        let handle = VolumeHandle::open(drive)?;
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        let (usn_journal_id, next_usn) =
            query_usn_journal(drive).map_or((0, 0), |info| (info.journal_id, info.next_usn));

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
        drive: crate::platform::DriveLetter,
        index: MftIndex,
        header: IndexHeader,
    ) -> Result<MftIndex> {
        tokio::task::spawn_blocking(move || {
            Self::apply_usn_updates_to_cached_index_sync(drive, index, &header)
        })
        .await
        .map_err(|error| MftError::InvalidInput(format!("Task join error: {error}")))?
    }

    /// Synchronous implementation of `apply_usn_updates_to_cached_index`.
    fn apply_usn_updates_to_cached_index_sync(
        drive: crate::platform::DriveLetter,
        index: MftIndex,
        header: &IndexHeader,
    ) -> Result<MftIndex> {
        match Self::classify_usn_state(drive, header) {
            UsnDecision::UseCached => Ok(index),
            UsnDecision::Rebuild => Self::read_and_cache_single_drive_sync(drive),
            UsnDecision::Apply {
                journal_id,
                start_usn,
            } => Ok(Self::apply_or_skip_usn_changes(
                drive, index, journal_id, start_usn,
            )),
        }
    }

    /// Classify what to do with the cached `index` for `drive` based on the
    /// USN-journal state at the moment of inspection.
    ///
    /// Thin adapter over [`crate::reader::usn_apply::classify_usn_state`]
    /// that handles the `query_usn_journal` failure path here: when the
    /// journal is unavailable we fall back to [`UsnDecision::UseCached`]
    /// so the cached index is still served instead of erroring out.
    fn classify_usn_state(
        drive: crate::platform::DriveLetter,
        header: &IndexHeader,
    ) -> UsnDecision {
        match query_usn_journal(drive) {
            Ok(info) => classify_with_journal_info(drive, header, &info),
            Err(error) => {
                warn!(
                    drive = %drive,
                    error = %error,
                    "⚠️ USN Journal unavailable - using cached index as-is"
                );
                UsnDecision::UseCached
            }
        }
    }

    /// Read the USN journal from `start_usn` and either apply the resulting
    /// changes (and persist the index back to cache) or return the index
    /// unchanged when there is nothing to apply.
    ///
    /// All failure paths (USN read errors, no records) gracefully fall back
    /// to returning the original cached `index`; there is no fallible
    /// outcome to surface, so this returns `MftIndex` directly.
    fn apply_or_skip_usn_changes(
        drive: crate::platform::DriveLetter,
        index: MftIndex,
        journal_id: u64,
        start_usn: i64,
    ) -> MftIndex {
        let (records, next_usn) = match read_usn_journal(drive, journal_id, start_usn) {
            Ok(result) => result,
            Err(error) => {
                warn!(
                    drive = %drive,
                    error = %error,
                    "⚠️ Failed to read USN Journal - using cached index as-is"
                );
                return index;
            }
        };

        if records.is_empty() {
            debug!(drive = %drive, "✅ No USN changes since last cache");
            return index;
        }

        Self::apply_usn_changes_and_save(drive, index, &records, journal_id, start_usn, next_usn)
    }

    /// Apply aggregated USN changes to `index` and persist the result.
    ///
    /// Splits into three phases:
    /// 1. Apply deletes (and collect FRSes needing a re-read on Windows).
    /// 2. Targeted MFT reads for non-delete changes.
    /// 3. Rebuild extension index + tree metrics if anything changed, then save
    ///    the updated index back to cache.
    fn apply_usn_changes_and_save(
        drive: crate::platform::DriveLetter,
        mut index: MftIndex,
        records: &[crate::usn::UsnRecord],
        journal_id: u64,
        start_usn: i64,
        next_usn: i64,
    ) -> MftIndex {
        let changes_map = aggregate_changes(records);
        let changes: Vec<_> = changes_map.into_values().collect();
        info!(
            drive = %drive,
            usn_records = changes.len(),
            from_usn = start_usn,
            to_usn = next_usn,
            "🔧 Applying USN changes"
        );

        // Phase 1: apply deletes and collect FRS values for targeted reads.
        // `frs_to_read` is only consumed on Windows (Phase 2 below); discard
        // it on non-Windows builds so the binding stays referenced exactly
        // when it is needed.
        #[cfg(windows)]
        let (mut stats, frs_to_read) = index.apply_usn_deletes(&changes);
        #[cfg(not(windows))]
        let (mut stats, _) = index.apply_usn_deletes(&changes);

        let handle = match VolumeHandle::open(drive) {
            Ok(handle) => handle,
            Err(error) => {
                warn!(
                    drive = %drive,
                    error = %error,
                    "⚠️ Failed to open volume for cache update"
                );
                return index;
            }
        };

        // Phase 2: targeted MFT reads for non-delete changes (Windows only)
        #[cfg(windows)]
        apply_targeted_usn_reads(drive, &handle, &mut index, &frs_to_read, &mut stats);

        // Phase 3: rebuild derived structures + persist
        rebuild_derived_after_usn(drive, &mut index, &stats);
        persist_usn_checkpoint(drive, &handle, &index, journal_id, next_usn);

        index
    }
}
