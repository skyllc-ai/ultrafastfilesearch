// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Drive-refresh path for [`IndexManager`].
//!
//! On a `refresh` RPC the daemon walks the requested drive list
//! sequentially, reloads each drive's MFT (live on Windows or the
//! original `.mft` snapshot on Mac/Linux) on a blocking thread,
//! and atomically swaps the new compact index into the registry
//! via [`IndexManager::replace_drive`].
//!
//! Sequential — not parallel — because the typical refresh tick
//! is operator-driven (`uffs refresh`) and the per-drive cost is
//! bounded by a single MFT read + compact build (~1 s per
//! drive).  A single-flight serial loop keeps the
//! `RefreshStarted`/`RefreshComplete` event pair semantically
//! tight (one tick = one operator action) and avoids the
//! background-IO cascade the [`crate::spawn_journal_loops_for_warm_shards`]
//! controller already covers for incremental updates.
//!
//! The `live_refresh_supported` + `is_live_drive_marker` pair
//! gates the platform-specific branch in
//! [`IndexManager::resolve_refresh_mft_source`]: a 2-char path like
//! `"C:"` is the opaque marker for a live MFT volume that
//! `load_live_drives`
//! installs on Windows, while every other path length is an
//! on-disk `.mft` snapshot reloadable from disk on any platform.

use uffs_client::protocol::response::DaemonStatus;

use super::{IndexManager, release_allocator_pages};
use crate::events::DaemonEvent;

impl IndexManager {
    /// Refresh specific drives (or all if empty).
    pub(crate) async fn refresh(&self, drives: &[uffs_mft::platform::DriveLetter]) {
        let drives_to_refresh: Vec<uffs_mft::platform::DriveLetter> = if drives.is_empty() {
            let snap = self.snapshot().await;
            snap.drives.iter().map(|dr| dr.letter).collect()
        } else {
            drives.to_vec()
        };

        self.events.emit(DaemonEvent::RefreshStarted {
            drives: drives_to_refresh.clone(),
        });

        let mut refresh_status = self.status.write().await;
        *refresh_status = DaemonStatus::Refreshing {
            drives: drives_to_refresh.clone(),
        };
        drop(refresh_status);

        // Refresh each drive sequentially.  Allocator-page reclamation
        // happens inside the helper after every per-drive cycle so a
        // long refresh list doesn't accumulate freed-but-not-decommitted
        // pages.
        for &letter in &drives_to_refresh {
            self.refresh_one_drive(letter).await;
        }

        self.set_ready().await;
        self.events.emit(DaemonEvent::RefreshComplete {
            drives_refreshed: drives_to_refresh.len(),
        });
    }

    /// Refresh a single drive in-place.
    ///
    /// Looks up the drive's `IndexSource` in the current snapshot,
    /// reloads it on a blocking thread, swaps the result into the
    /// shared index on success, and traces the outcome of every arm
    /// of the resulting `Result<Result<_, _>, JoinError>`.  Caller
    /// holds no locks across the await points.
    async fn refresh_one_drive(&self, letter: uffs_mft::platform::DriveLetter) {
        let Some(source) = self.lookup_drive_source(letter).await else {
            tracing::warn!(drive = %letter, "Drive not found for refresh");
            return;
        };

        let result = tokio::task::spawn_blocking(move || match &source {
            uffs_core::compact::IndexSource::MftFile(mft_path) => {
                if Self::is_live_drive_marker(mft_path) && !Self::live_refresh_supported() {
                    return Err(anyhow::anyhow!("Cannot refresh live drive on non-Windows"));
                }
                let mft_source = Self::resolve_refresh_mft_source(mft_path, letter);
                uffs_core::compact::load_drive(&mft_source, false)
            }
        })
        .await;

        self.apply_refresh_result(letter, result).await;

        // Reclaim pages freed by the old drive index and MftIndex temporaries.
        release_allocator_pages();
    }

    /// Trace + dispatch the `Result<Result<_, _>, JoinError>` returned
    /// by [`crate::index::IndexManager::refresh_one_drive`]'s `spawn_blocking`.
    /// On success defers
    /// to [`crate::index::IndexManager::apply_refresh_success`]; on either
    /// error arm emits the matching error trace.
    async fn apply_refresh_result(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        result: Result<
            anyhow::Result<(
                uffs_core::compact::DriveCompactIndex,
                uffs_core::compact::LoadTiming,
            )>,
            tokio::task::JoinError,
        >,
    ) {
        match result {
            Ok(Ok((new_drive, timing))) => {
                self.apply_refresh_success(letter, new_drive, &timing).await;
            }
            Ok(Err(refresh_err)) => {
                tracing::error!(drive = %letter, error = %refresh_err, "Failed to refresh drive");
            }
            Err(join_err) => {
                tracing::error!(drive = %letter, error = %join_err, "Task panicked during refresh");
            }
        }
    }

    /// Snapshot-bounded lookup of a drive's recorded `IndexSource`.
    ///
    /// Returned by clone so the caller can hand the source to
    /// `spawn_blocking` without keeping the read guard alive across
    /// the await.
    async fn lookup_drive_source(
        &self,
        letter: uffs_mft::platform::DriveLetter,
    ) -> Option<uffs_core::compact::IndexSource> {
        let snap = self.snapshot().await;
        snap.drives
            .iter()
            .find(|dr| dr.letter == letter)
            .map(|dr| dr.source.clone())
    }

    /// Successful-refresh fanout: hot-swap the drive in the shared
    /// snapshot, trace the new record count + per-stage timings, and
    /// emit `DriveRefreshed` so subscribers (TUI, daemon-events RPC)
    /// stay in lockstep with the index.
    async fn apply_refresh_success(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        new_drive: uffs_core::compact::DriveCompactIndex,
        timing: &uffs_core::compact::LoadTiming,
    ) {
        let records = new_drive.records.len();
        self.replace_drive(letter, new_drive).await;
        tracing::info!(
            drive = %letter,
            records,
            mft_ms = timing.mft,
            compact_ms = timing.compact,
            trigram_ms = timing.trigram,
            "Drive refreshed"
        );
        self.events.emit(DaemonEvent::DriveRefreshed {
            drive: letter,
            records,
            mft_ms: timing.mft,
            compact_ms: timing.compact,
            trigram_ms: timing.trigram,
        });
    }

    /// Map a cached drive's recorded MFT source path back to a
    /// reloadable [`MftSource`].
    ///
    /// A path like `"C:"` (length ≤ 2) is an opaque marker for a
    /// live MFT scan — valid on Windows, rejected at the
    /// [`IndexManager::refresh_one_drive`] call site on every other platform
    /// via [`IndexManager::live_refresh_supported`].  Anything longer is an
    /// on-disk `.mft` snapshot reloadable from disk on any platform.
    ///
    /// [`MftSource`]: uffs_core::compact::MftSource
    fn resolve_refresh_mft_source(
        mft_path: &std::path::Path,
        letter: uffs_mft::platform::DriveLetter,
    ) -> uffs_core::compact::MftSource {
        if Self::is_live_drive_marker(mft_path) {
            #[cfg(windows)]
            {
                uffs_core::compact::MftSource::Live(letter)
            }
            #[cfg(not(windows))]
            {
                // Caller (`live_refresh_supported`) gates this branch so
                // we only reach it on Windows; the non-Windows
                // construction here is unreachable but kept so the
                // function remains total without a `Result` wrapper.
                uffs_core::compact::MftSource::File(mft_path.to_path_buf(), Some(letter))
            }
        } else {
            uffs_core::compact::MftSource::File(mft_path.to_path_buf(), Some(letter))
        }
    }

    /// Path-shape test: a cached source whose stringified length is
    /// ≤ 2 (e.g. `"C:"`) was originally a live MFT scan rather than
    /// an on-disk snapshot.
    pub(super) fn is_live_drive_marker(mft_path: &std::path::Path) -> bool {
        mft_path.to_string_lossy().len() <= 2
    }

    /// Returns `true` when refreshing a live-drive marker is
    /// supported on the current target.  Always `true` on Windows;
    /// always `false` elsewhere because live MFT scanning needs
    /// `\\.\<letter>:` raw-volume access that only Windows provides.
    #[cfg(windows)]
    const fn live_refresh_supported() -> bool {
        true
    }

    /// Non-Windows stub: live MFT scanning is unsupported on this
    /// target, so callers must reject the live-drive marker before
    /// reaching `resolve_refresh_mft_source`.
    #[cfg(not(windows))]
    const fn live_refresh_supported() -> bool {
        false
    }
}
