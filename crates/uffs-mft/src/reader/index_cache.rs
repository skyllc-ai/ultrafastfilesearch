//! Cached lean-index read helpers.

use super::MftReader;
#[cfg(not(windows))]
use crate::error::MftError;
use crate::error::Result;

impl MftReader {
    /// Read MFT into lean `MftIndex` with automatic caching.
    ///
    /// This is the **recommended primary method** for CLI usage. It:
    /// 1. Checks if a fresh cache exists (within TTL)
    /// 2. If fresh, loads from cache and applies USN Journal updates
    /// 3. If stale/missing, reads MFT fresh and saves to cache
    ///
    /// Use `read_all_index()` directly only when you need to bypass caching.
    ///
    /// # Arguments
    ///
    /// * `ttl_seconds` - Cache TTL in seconds (use `INDEX_TTL_SECONDS` for
    ///   default)
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[tracing::instrument(
        level = "info",
        skip(self),
        fields(
            volume = %self.volume,
            ttl_seconds,
            mode = %self.mode,
            use_bitmap = self.use_bitmap,
            merge_extensions = self.merge_extensions,
            expand_links = self.expand_links
        )
    )]
    pub async fn read_index_cached(&self, ttl_seconds: u64) -> Result<crate::index::MftIndex> {
        use tracing::{debug, info, warn};

        use crate::cache::{CacheStatus, check_cache_status, save_to_cache};
        use crate::platform::VolumeHandle;
        use crate::usn::{aggregate_changes, query_usn_journal, read_usn_journal};

        let drive = self.volume;
        tracing::debug!(drive = %drive, ttl_seconds, "[TRIP] reader::read_index_cached ENTER");

        // Fast path for read-only volumes: cache can never go stale since
        // nothing can change on the drive.  Skip TTL, skip USN, skip
        // VolumeHandle — just load from disk and return.
        let read_only = crate::platform::is_volume_read_only(drive);
        if read_only {
            if let Some((index, _header)) = crate::cache::load_cached_index(drive, u64::MAX) {
                info!(
                    drive = %drive,
                    records = index.len(),
                    "📦 Read-only volume — using cached index (no TTL)"
                );
                return Ok(index);
            }
            // No cache yet — fall through to build one.
            info!(drive = %drive, "🆕 Read-only volume — no cache, building index");
            return self.read_and_cache_index().await;
        }

        // Check cache status
        match check_cache_status(drive, ttl_seconds) {
            CacheStatus::Fresh {
                mut index,
                header,
                age_seconds,
            } => {
                tracing::debug!(drive = %drive, age_seconds, "[TRIP] reader::read_index_cached -> CACHE_HIT path");
                info!(
                    drive = %drive,
                    age_seconds,
                    records = index.len(),
                    "📦 Cache HIT - checking for USN updates"
                );

                // Restore reserved_allocated_bytes from live volume data.
                // This field is not serialized in the cache; it is needed for
                // correct root tree_allocated when tree metrics are recomputed
                // (e.g. after USN updates).
                if let Ok(handle) = VolumeHandle::open(drive) {
                    index.reserved_allocated_bytes =
                        handle.volume_data().reserved_allocated_bytes();
                }

                // Apply USN Journal updates to bring index up to date
                let current_info = match query_usn_journal(drive) {
                    Ok(info) => info,
                    Err(e) => {
                        warn!(
                            drive = %drive,
                            error = %e,
                            "⚠️ USN Journal unavailable - using cached index as-is"
                        );
                        return Ok(index);
                    }
                };

                // Check if journal ID matches (journal may have been recreated)
                if header.usn_journal_id != 0 && current_info.journal_id != header.usn_journal_id {
                    info!(
                        drive = %drive,
                        cached_journal_id = header.usn_journal_id,
                        current_journal_id = current_info.journal_id,
                        "🔄 USN Journal ID changed - rebuilding index"
                    );
                    return self.read_and_cache_index().await;
                }

                // Check if our checkpoint is still valid
                let start_usn = header.next_usn;
                if start_usn < current_info.first_usn {
                    info!(
                        drive = %drive,
                        cached_usn = start_usn,
                        first_usn = current_info.first_usn,
                        "🔄 USN Journal wrapped - rebuilding index"
                    );
                    return self.read_and_cache_index().await;
                }

                // If already at latest USN, no changes needed
                if start_usn >= current_info.next_usn {
                    debug!(
                        drive = %drive,
                        usn = start_usn,
                        "✅ Index is already up to date"
                    );
                    return Ok(index);
                }

                // Read USN changes since checkpoint
                let (records, next_usn) =
                    match read_usn_journal(drive, current_info.journal_id, start_usn) {
                        Ok(result) => result,
                        Err(e) => {
                            warn!(
                                drive = %drive,
                                error = %e,
                                "⚠️ Failed to read USN Journal - using cached index as-is"
                            );
                            return Ok(index);
                        }
                    };

                if records.is_empty() {
                    debug!(drive = %drive, "✅ No USN changes since last cache");
                    return Ok(index);
                }

                // Aggregate and apply changes
                let changes_map = aggregate_changes(&records);
                let changes: Vec<_> = changes_map.into_values().collect();
                info!(
                    drive = %drive,
                    usn_records = changes.len(),
                    "📝 Applying USN updates to cached index"
                );

                let stats = index.apply_usn_changes(&changes);
                info!(
                    drive = %drive,
                    created = stats.created,
                    modified = stats.modified,
                    deleted = stats.deleted,
                    skipped = stats.skipped,
                    "✅ USN updates applied"
                );

                // Save updated index back to cache
                let handle = VolumeHandle::open(drive)?;
                let volume_serial = handle.volume_data().volume_serial_number;
                if let Err(e) = save_to_cache(
                    &index,
                    drive,
                    volume_serial,
                    current_info.journal_id,
                    next_usn,
                ) {
                    warn!(drive = %drive, error = %e, "⚠️ Failed to update cache");
                }

                Ok(index)
            }
            CacheStatus::Stale { age_seconds } => {
                tracing::debug!(drive = %drive, "[TRIP] reader::read_index_cached -> CACHE_STALE path");
                info!(
                    drive = %drive,
                    age_seconds = ?age_seconds,
                    "🔄 Cache STALE - rebuilding index"
                );
                self.read_and_cache_index().await
            }
            CacheStatus::Missing => {
                tracing::debug!(drive = %drive, "[TRIP] reader::read_index_cached -> CACHE_MISS path");
                info!(drive = %drive, "🆕 Cache MISS - building index");
                self.read_and_cache_index().await
            }
        }
    }

    /// Read MFT with caching (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_index_cached(&self, _ttl_seconds: u64) -> Result<crate::index::MftIndex> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal helper: read MFT fresh and save to cache.
    #[cfg(windows)]
    #[tracing::instrument(
        level = "info",
        skip(self),
        fields(
            volume = %self.volume,
            mode = %self.mode,
            use_bitmap = self.use_bitmap,
            merge_extensions = self.merge_extensions,
            expand_links = self.expand_links
        )
    )]
    async fn read_and_cache_index(&self) -> Result<crate::index::MftIndex> {
        use tracing::info;

        use crate::platform::VolumeHandle;
        use crate::usn::query_usn_journal;

        let drive = self.volume;
        tracing::debug!(drive = %drive, "[TRIP] reader::read_and_cache_index ENTER");
        let index = self.read_all_index().await?;
        tracing::debug!(drive = %drive, records = index.len(), "[TRIP] reader::read_and_cache_index -> read_all_index done");

        // Get volume info for caching (quick syscalls, fine on async thread).
        let volume_serial = VolumeHandle::open(drive)
            .map(|h| h.volume_data().volume_serial_number)
            .unwrap_or(0);

        let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
            Ok(info) => (info.journal_id, info.next_usn),
            Err(_) => (0, 0),
        };

        // Serialize the index on the current thread (CPU-bound, no I/O yet)
        // so we can hand ownership of the index back to the caller immediately.
        // Only the byte buffer is sent to the blocking pool for the disk write.
        let cache_bytes = index.serialize(volume_serial, usn_journal_id, next_usn);

        tokio::task::spawn_blocking(move || {
            use crate::cache::{cache_dir, cache_file_path};
            let dir = cache_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                info!(drive = %drive, error = %e, "⚠️ Failed to create cache dir");
                return;
            }
            let path = cache_file_path(drive);
            if let Err(e) = std::fs::write(&path, &cache_bytes) {
                info!(drive = %drive, error = %e, "⚠️ Failed to write cache (non-fatal)");
            } else {
                info!(drive = %drive, bytes = cache_bytes.len(), "💾 Saved to cache");
            }
        });

        tracing::debug!(drive = %drive, "[TRIP] reader::read_and_cache_index EXIT");
        Ok(index)
    }
}
