// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! On-demand drive loading for [`IndexManager`].
//!
//! Two distinct entry points cover the runtime "load this drive
//! now" surface:
//!
//! 1. [`Self::load_single_mft_file`] — a path-based hot-load. Used by the `add`
//!    RPC and the file-watcher integration in `crate::lifecycle` when an
//!    operator drops a new `*.mft` snapshot into `data_dir`.  Skips if the
//!    drive is already loaded (no replace).
//! 2. [`Self::hot_load_drive`] — a letter-based hot-load.  Used by the `load`
//!    RPC.  On Windows reads the live MFT directly; on Mac/Linux looks for a
//!    snapshot under `data_dir/drive_X/`.  Replaces an already-loaded drive
//!    (the operator wants a re-read).
//!
//! Both paths share the per-drive blocking-load helper
//! [`Self::blocking_load_drive`] which wraps
//! [`uffs_core::compact::load_drive`] in `spawn_blocking` and
//! reclaims allocator pages on completion.  Auto-discovery from
//! the data directory is provided by
//! [`Self::discover_and_load_drive`] /
//! [`Self::ensure_drives_loaded`] so the search RPC can
//! transparently load drives the user named but didn't
//! pre-mount.

use super::{IndexManager, StoredDriveTiming, release_allocator_pages};
use crate::events::DaemonEvent;

impl IndexManager {
    /// Hot-load a single MFT file if its drive letter is not already loaded.
    ///
    /// Returns `Ok(Some(letter))` if loaded, `Ok(None)` if already present.
    pub(crate) async fn load_single_mft_file(
        &self,
        mft_path: &std::path::Path,
        no_cache: bool,
    ) -> anyhow::Result<Option<char>> {
        let letter = Self::infer_drive_letter(mft_path);

        // Skip if already loaded.
        {
            let snap = self.snapshot().await;
            if snap.drives.iter().any(|dr| dr.letter == letter) {
                tracing::debug!(drive = %letter, "Drive already loaded, skipping");
                return Ok(None);
            }
        }

        tracing::info!(
            drive = %letter,
            path = %mft_path.display(),
            "Hot-loading MFT file"
        );

        let cloned_path = mft_path.to_path_buf();
        let source = uffs_core::compact::MftSource::File(cloned_path, None);
        let result =
            tokio::task::spawn_blocking(move || uffs_core::compact::load_drive(&source, no_cache))
                .await;

        // Reclaim pages freed by MftIndex temporaries during load.
        release_allocator_pages();

        self.apply_hot_load_result(letter, mft_path, result).await
    }

    /// Derive the drive letter from a `.mft` / `.iocp` snapshot path.
    ///
    /// Convention: the first ASCII-alphabetic character of the file
    /// stem (e.g. `G_mft.iocp` → `'G'`).  Falls back to `'X'` for
    /// non-conforming names so the caller still gets a stable handle
    /// to log against rather than an `Option`.
    pub(super) fn infer_drive_letter(mft_path: &std::path::Path) -> char {
        let stem = mft_path.file_name().and_then(|n| n.to_str()).unwrap_or("X");
        stem.chars()
            .next()
            .filter(char::is_ascii_alphabetic)
            .map_or('X', |ch| ch.to_ascii_uppercase())
    }

    /// Fold the `JoinError`/`anyhow::Error` ladder of a hot-load
    /// `spawn_blocking` into a single trace-and-publish step.
    ///
    /// On success: emits `DriveLoaded`, swaps the new drive into the
    /// snapshot, and bumps the search concurrency semaphore.  On
    /// failure: traces the cause and propagates it as `Err` so the
    /// caller can surface it to the RPC layer.
    async fn apply_hot_load_result(
        &self,
        letter: char,
        mft_path: &std::path::Path,
        result: Result<
            anyhow::Result<(
                uffs_core::compact::DriveCompactIndex,
                uffs_core::compact::LoadTiming,
            )>,
            tokio::task::JoinError,
        >,
    ) -> anyhow::Result<Option<char>> {
        match result {
            Ok(Ok((drive_index, timing))) => {
                let records = drive_index.records.len();
                tracing::info!(
                    drive = %letter,
                    records,
                    mft_ms = timing.mft,
                    compact_ms = timing.compact,
                    trigram_ms = timing.trigram,
                    "Drive hot-loaded"
                );
                self.events.emit(DaemonEvent::DriveLoaded {
                    drive: letter,
                    records,
                    mft_ms: timing.mft,
                    compact_ms: timing.compact,
                    trigram_ms: timing.trigram,
                    drives_loaded: 1,
                    drives_total: 1,
                });
                self.add_drive(drive_index).await;
                // Drive count changed — resize the search semaphore.
                self.tune_concurrency().await;
                Ok(Some(letter))
            }
            Ok(Err(load_err)) => {
                tracing::error!(
                    path = %mft_path.display(),
                    error = %load_err,
                    "Failed to hot-load MFT file"
                );
                Err(load_err)
            }
            Err(join_err) => {
                tracing::error!(
                    path = %mft_path.display(),
                    error = %join_err,
                    "Task panicked hot-loading MFT"
                );
                anyhow::bail!("Task panicked: {join_err}")
            }
        }
    }

    /// Hot-load a single drive letter into the running daemon.
    ///
    /// On **Windows**, reads the live NTFS MFT directly.
    /// On **non-Windows**, looks in `data_dir` for an offline MFT file.
    ///
    /// If the drive is already loaded, replaces it (re-read).
    ///
    /// Returns `Ok(records)` on success.
    pub(crate) async fn hot_load_drive(
        &self,
        drive_letter: char,
        no_cache: bool,
    ) -> anyhow::Result<usize> {
        let letter = drive_letter.to_ascii_uppercase();

        if self.is_drive_loaded(letter).await {
            tracing::info!(drive = %letter, "Drive already loaded — will hot-swap after re-read");
        }

        let source = self.resolve_drive_source(letter)?;
        tracing::info!(drive = %letter, "Hot-loading drive");

        let (drive_index, timing) = self.blocking_load_drive(source, no_cache).await?;
        let records = drive_index.records.len();

        self.emit_drive_loaded(letter, records, &timing);
        self.store_drive_timing(letter, &timing).await;
        // Atomic swap: old drive (if any) is replaced in a single pointer
        // swap — in-flight queries on the old Arc finish undisturbed, new
        // queries see the fresh data immediately.
        self.replace_drive(letter, drive_index).await;

        Ok(records)
    }

    /// Check whether a drive letter is already in the index.
    async fn is_drive_loaded(&self, letter: char) -> bool {
        let guard = self.index.read().await;
        guard.contains(letter)
    }

    /// Determine the [`MftSource`] for a drive letter.
    ///
    /// [`MftSource`]: uffs_core::compact::MftSource
    // Note: cannot be `const fn` — the non-Windows branch uses `?` on `Result`
    // and calls non-const helpers (`find_best_mft_file`).  `cargo xwin clippy`
    // only sees the Windows branch and incorrectly suggests `const`, so the
    // expect is gated on `cfg(windows)` to avoid an unfulfilled-lint-expectation
    // on macOS where the lint legitimately doesn't fire.
    #[cfg_attr(
        windows,
        expect(
            clippy::missing_const_for_fn,
            reason = "non-Windows branch uses `?` on Result and calls non-const helpers; cannot be const"
        )
    )]
    #[cfg_attr(
        windows,
        expect(
            clippy::unused_self,
            clippy::unnecessary_wraps,
            reason = "Windows branch collapses to a tuple-only construction; \
                      non-Windows path needs &self.data_dir and propagates Result"
        )
    )]
    fn resolve_drive_source(&self, letter: char) -> anyhow::Result<uffs_core::compact::MftSource> {
        #[cfg(windows)]
        {
            Ok(uffs_core::compact::MftSource::Live(letter))
        }
        #[cfg(not(windows))]
        {
            let data_dir = self.data_dir.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "No data_dir configured — cannot load drive {letter}: on non-Windows"
                )
            })?;
            let drive_subdir = data_dir.join(format!("drive_{}", letter.to_ascii_lowercase()));
            let mft_path =
                uffs_mft::discovery::find_best_mft_file(&drive_subdir).ok_or_else(|| {
                    anyhow::anyhow!("No MFT file found in {}", drive_subdir.display())
                })?;
            Ok(uffs_core::compact::MftSource::File(mft_path, Some(letter)))
        }
    }

    /// Run `load_drive` on a blocking thread and release allocator pages.
    async fn blocking_load_drive(
        &self,
        source: uffs_core::compact::MftSource,
        no_cache: bool,
    ) -> anyhow::Result<(
        uffs_core::compact::DriveCompactIndex,
        uffs_core::compact::LoadTiming,
    )> {
        let result =
            tokio::task::spawn_blocking(move || uffs_core::compact::load_drive(&source, no_cache))
                .await;

        release_allocator_pages();

        match result {
            Ok(Ok(pair)) => Ok(pair),
            Ok(Err(load_err)) => Err(load_err),
            Err(join_err) => anyhow::bail!("Load task panicked: {join_err}"),
        }
    }

    /// Emit a `DriveLoaded` event for a single hot-loaded drive.
    fn emit_drive_loaded(
        &self,
        letter: char,
        records: usize,
        timing: &uffs_core::compact::LoadTiming,
    ) {
        tracing::info!(
            drive = %letter, records,
            mft_ms = timing.mft, compact_ms = timing.compact, trigram_ms = timing.trigram,
            "Drive hot-loaded"
        );
        self.events.emit(DaemonEvent::DriveLoaded {
            drive: letter,
            records,
            mft_ms: timing.mft,
            compact_ms: timing.compact,
            trigram_ms: timing.trigram,
            drives_loaded: 1,
            drives_total: 1,
        });
    }

    /// Persist per-drive load timing for `--profile` reporting.
    async fn store_drive_timing(&self, letter: char, timing: &uffs_core::compact::LoadTiming) {
        self.drive_timings
            .write()
            .await
            .insert(letter, StoredDriveTiming {
                cache: timing.cache,
                mft: timing.mft,
                compact: timing.compact,
                trigram: timing.trigram,
            });
    }

    /// Discover and load a missing drive from the data directory.
    ///
    /// Returns `Ok(true)` if the drive was discovered and loaded,
    /// `Ok(false)` if no MFT file was found for it, or an error.
    pub(crate) async fn discover_and_load_drive(
        &self,
        drive_letter: char,
        no_cache: bool,
    ) -> anyhow::Result<bool> {
        let Some(data_dir) = &self.data_dir else {
            return Ok(false);
        };

        let drive_lower = drive_letter.to_ascii_lowercase();
        let drive_subdir = data_dir.join(format!("drive_{drive_lower}"));

        if !drive_subdir.is_dir() {
            tracing::debug!(
                drive = %drive_letter,
                path = %drive_subdir.display(),
                "No drive_X directory found in data_dir"
            );
            return Ok(false);
        }

        let Some(mft_path) = uffs_mft::discovery::find_best_mft_file(&drive_subdir) else {
            tracing::debug!(
                drive = %drive_letter,
                path = %drive_subdir.display(),
                "No MFT file found in drive directory"
            );
            return Ok(false);
        };

        // Whether Some (freshly loaded) or None (already present), the
        // drive is now available.
        let _loaded = self.load_single_mft_file(&mft_path, no_cache).await?;
        Ok(true)
    }

    /// Ensure all requested drives are loaded, auto-discovering from
    /// `data_dir` if available.
    ///
    /// Returns a list of drive letters that could NOT be loaded (no data
    /// source found).
    pub(crate) async fn ensure_drives_loaded(&self, drives: &[char], no_cache: bool) -> Vec<char> {
        if drives.is_empty() {
            return Vec::new();
        }

        let loaded = self.loaded_drive_letters().await;
        let mut missing: Vec<char> = Vec::new();

        for &letter in drives {
            let upper = letter.to_ascii_uppercase();
            if loaded.contains(&upper) {
                continue;
            }
            if !self.try_auto_discover_drive(upper, no_cache).await {
                missing.push(upper);
            }
        }

        missing
    }

    /// Auto-discover and load a single drive from `data_dir`.
    ///
    /// Returns `true` when the drive ended up loaded (cache hit or
    /// fresh discovery), `false` when no data source was found or the
    /// load failed.  Each branch is traced at its appropriate level so
    /// callers can stay flat.
    async fn try_auto_discover_drive(&self, letter: char, no_cache: bool) -> bool {
        match self.discover_and_load_drive(letter, no_cache).await {
            Ok(true) => {
                tracing::info!(drive = %letter, "Auto-discovered and loaded missing drive");
                true
            }
            Ok(false) => {
                tracing::warn!(
                    drive = %letter,
                    "Drive not loaded and not discoverable from data_dir"
                );
                false
            }
            Err(load_err) => {
                tracing::error!(
                    drive = %letter,
                    error = %load_err,
                    "Failed to auto-load drive"
                );
                false
            }
        }
    }
}
