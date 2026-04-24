// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Cached `DataFrame` loading (Windows-only).
//!
//! Extracted from `cache.rs` for file-size policy compliance.
//! These functions combine MFT reading with cache management to produce
//! Polars `DataFrame`s.

#[cfg(windows)]
use super::{load_cached_index, save_to_cache_background};

/// Loads a cached index and converts to `DataFrame`, or builds fresh if cache
/// miss.
///
/// This is the recommended way to get a `DataFrame` with caching support.
/// It uses the lean `MftIndex` cache format and converts to `DataFrame` on
/// demand.
///
/// # Arguments
///
/// * `drive` - Drive letter to read
/// * `ttl_seconds` - TTL in seconds for cache freshness
///
/// # Returns
///
/// A `DataFrame` with all MFT records for the drive.
///
/// # Errors
///
/// Returns an error if MFT reading or `DataFrame` conversion fails.
///
/// # Note
///
/// This function uses `spawn_blocking` internally to avoid nested tokio runtime
/// issues. Polars uses tokio internally for some operations, and calling polars
/// from within a tokio async context can cause "Cannot start a runtime from
/// within a runtime" panics. By running all MFT reading and polars operations
/// on a dedicated blocking thread, we avoid this issue.
#[cfg(windows)]
pub async fn load_or_build_dataframe_cached(
    drive: char,
    ttl_seconds: u64,
) -> crate::Result<uffs_polars::DataFrame> {
    tracing::debug!(drive = %drive, ttl_seconds, "Entering cached DataFrame load/build");
    let result = tokio::task::spawn_blocking(move || {
        tracing::debug!(
            drive = %drive,
            ttl_seconds,
            "Running cached DataFrame load/build on blocking thread"
        );
        load_or_build_dataframe_cached_sync(drive, ttl_seconds)
    })
    .await
    .map_err(|error| crate::MftError::from_join_error("load_or_build_dataframe_cached", &error))?;
    tracing::debug!(
        drive = %drive,
        ttl_seconds,
        success = result.is_ok(),
        "Completed cached DataFrame load/build"
    );
    result
}

/// Synchronous version of [`load_or_build_dataframe_cached`].
///
/// This is the actual implementation that runs on a blocking thread.
/// It performs all MFT reading and polars operations synchronously.
#[cfg(windows)]
fn load_or_build_dataframe_cached_sync(
    drive: char,
    ttl_seconds: u64,
) -> crate::Result<uffs_polars::DataFrame> {
    use crate::VolumeHandle;
    use crate::reader::MftReader;
    use crate::usn::query_usn_journal;

    tracing::debug!(
        drive = %drive,
        ttl_seconds,
        "Entering synchronous cached DataFrame load/build"
    );

    // Try to load from cache first
    if let Some((index, _header)) = load_cached_index(drive, ttl_seconds) {
        tracing::info!(drive = %drive, records = index.records.len(), "📦 Cache hit - converting to DataFrame");
        let df = index.to_dataframe();
        match &df {
            Ok(dataframe) => tracing::debug!(
                drive = %drive,
                rows = dataframe.height(),
                columns = dataframe.width(),
                "Converted cached index to DataFrame"
            ),
            Err(error) => tracing::debug!(
                drive = %drive,
                error = %error,
                "Cached index DataFrame conversion failed"
            ),
        }
        return df;
    }

    // Cache miss - read fresh
    tracing::info!(drive = %drive, "📖 Cache miss - reading MFT fresh");
    let reader = MftReader::open(drive)?;
    tracing::debug!(drive = %drive, "Opened MFT reader; reading fresh index");
    let index = reader.read_all_index_sync()?;
    tracing::debug!(drive = %drive, records = index.len(), "Completed fresh index read");

    // Save to cache for next time
    let handle = VolumeHandle::open(drive)?;
    let volume_serial = handle.volume_data().volume_serial_number;
    let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
        Ok(info) => (info.journal_id, info.next_usn),
        Err(_) => (0, 0),
    };

    // Background save: serialize sync, compress/encrypt/write in bg thread.
    if let Err(err) =
        save_to_cache_background(&index, drive, volume_serial, usn_journal_id, next_usn)
    {
        tracing::warn!(drive = %drive, error = %err, "Failed to start cache save");
    }

    // Convert to DataFrame
    tracing::debug!(drive = %drive, records = index.len(), "Converting fresh index to DataFrame");
    let df = index.to_dataframe();
    match &df {
        Ok(dataframe) => tracing::debug!(
            drive = %drive,
            rows = dataframe.height(),
            columns = dataframe.width(),
            "Converted fresh index to DataFrame"
        ),
        Err(error) => tracing::debug!(
            drive = %drive,
            error = %error,
            "Fresh index DataFrame conversion failed"
        ),
    }
    df
}
