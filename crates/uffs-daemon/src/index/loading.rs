// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Drive loading orchestrators for [`IndexManager`].
//!
//! Two startup paths share the same per-drive `JoinSet`-and-collect
//! pattern but differ in MFT source:
//!
//! 1. **`load_from_data_dir`** — Mac/Linux offline mode.  Loads each `*.mft`
//!    file the operator passed via `--data-dir` on its own blocking thread;
//!    when every file has been processed the daemon emits `DaemonReady`.
//! 2. **`load_live_drives`** (Windows-only) — online mode.  Loads each NTFS
//!    volume's live MFT in parallel, capped per-drive by
//!    [`Self::DRIVE_LOAD_TIMEOUT`] so a single stuck drive can't hang the
//!    daemon indefinitely.
//!
//! Both paths funnel successful loads through [`Self::add_drive`]
//! which performs the atomic registry pointer-swap and bumps the
//! aggregate cache's `index_version`.  [`Self::replace_drive`]
//! lives in this module too because it mirrors `add_drive`'s swap
//! semantics — used by the refresh path to update an
//! already-loaded drive in place.
//!
//! The `set_ready` helper transitions the daemon out of the
//! `Loading { drives_loaded, drives_total }` status and records
//! startup duration; called once per startup path.

use alloc::sync::Arc;
use core::sync::atomic::Ordering;
use std::path::PathBuf;

use uffs_client::protocol::response::DaemonStatus;

use super::{IndexManager, StoredDriveTiming, release_allocator_pages};
use crate::cache::unix_now_ms;
use crate::events::DaemonEvent;

impl IndexManager {
    /// Load drives from MFT files — **all files in parallel**.
    ///
    /// Each MFT file is loaded on its own blocking thread via `JoinSet`.
    /// Results are collected as they complete (fastest first).
    pub(crate) async fn load_from_data_dir(&self, mft_files: &[PathBuf], no_cache: bool) {
        let total = mft_files.len();
        *self.status.write().await = DaemonStatus::Loading {
            drives_loaded: 0,
            drives_total: total,
        };

        let mut join_set = Self::spawn_data_dir_loaders(mft_files, no_cache);

        // Drain completions as they finish (fastest first), patching
        // status, events, and timings via a single per-result helper.
        let mut loaded: usize = 0;
        while let Some(join_result) = join_set.join_next().await {
            self.apply_data_dir_load_result(join_result, &mut loaded, total)
                .await;
        }

        // Retune the search-concurrency semaphore to match the loaded
        // drive count before admitting queries (see `tune_concurrency`).
        self.tune_concurrency().await;

        // Mark as ready + record startup duration.
        self.set_ready().await;
        self.emit_data_dir_ready_summary().await;
    }

    /// Spawn one blocking task per MFT file, returning the `JoinSet`
    /// the caller drains for incremental progress.
    fn spawn_data_dir_loaders(
        mft_files: &[PathBuf],
        no_cache: bool,
    ) -> tokio::task::JoinSet<(
        PathBuf,
        anyhow::Result<(
            uffs_core::compact::DriveCompactIndex,
            uffs_core::compact::LoadTiming,
        )>,
    )> {
        let mut join_set = tokio::task::JoinSet::new();
        for mft_path in mft_files {
            let path = mft_path.clone();
            tracing::info!(path = %path.display(), "Loading MFT file (parallel)");
            join_set.spawn_blocking(move || {
                let source = uffs_core::compact::MftSource::File(path.clone(), None);
                let result = uffs_core::compact::load_drive(&source, no_cache);
                (path, result)
            });
        }
        join_set
    }

    /// Process a single [`tokio::task::JoinSet`] completion: trace the
    /// outcome, install the drive on success, bump the `loaded`
    /// counter regardless of arm, reclaim allocator pages, and update
    /// the `Loading` progress status.
    async fn apply_data_dir_load_result(
        &self,
        join_result: Result<
            (
                PathBuf,
                anyhow::Result<(
                    uffs_core::compact::DriveCompactIndex,
                    uffs_core::compact::LoadTiming,
                )>,
            ),
            tokio::task::JoinError,
        >,
        loaded: &mut usize,
        total: usize,
    ) {
        *loaded = loaded.saturating_add(1);
        match join_result {
            Ok((_path, Ok((drive_index, timing)))) => {
                self.install_data_dir_drive(drive_index, &timing, *loaded, total)
                    .await;
            }
            Ok((path, Err(load_err))) => {
                tracing::error!(path = %path.display(), error = %load_err, "Failed to load MFT file");
            }
            Err(join_err) => {
                tracing::error!(error = %join_err, "Task panicked loading MFT");
            }
        }

        // Return freed pages to the OS after each drive load.
        release_allocator_pages();

        let mut progress = self.status.write().await;
        *progress = DaemonStatus::Loading {
            drives_loaded: *loaded,
            drives_total: total,
        };
        drop(progress);
    }

    /// Successful-load fanout: trace the per-stage timings, emit the
    /// `DriveLoaded` event subscribers consume, persist the timing
    /// for `--profile` reporting, and add the drive to the snapshot.
    async fn install_data_dir_drive(
        &self,
        drive_index: uffs_core::compact::DriveCompactIndex,
        timing: &uffs_core::compact::LoadTiming,
        loaded: usize,
        total: usize,
    ) {
        let letter = drive_index.letter;
        let records = drive_index.records.len();
        tracing::info!(
            drive = %letter,
            records,
            mft_ms = timing.mft,
            compact_ms = timing.compact,
            trigram_ms = timing.trigram,
            loaded,
            total,
            "Drive loaded"
        );
        self.events.emit(DaemonEvent::DriveLoaded {
            drive: letter,
            records,
            mft_ms: timing.mft,
            compact_ms: timing.compact,
            trigram_ms: timing.trigram,
            drives_loaded: loaded,
            drives_total: total,
        });
        self.drive_timings
            .write()
            .await
            .insert(letter, StoredDriveTiming {
                cache: timing.cache,
                mft: timing.mft,
                compact: timing.compact,
                trigram: timing.trigram,
            });
        self.add_drive(drive_index).await;
    }

    /// Emit the final `DaemonReady` event + summary trace once every
    /// drive in the data-dir set has been processed.
    async fn emit_data_dir_ready_summary(&self) {
        let snap = self.snapshot().await;
        let drive_count = snap.drives.len();
        let total_records = snap.total_records();
        drop(snap);
        tracing::info!(
            drives = drive_count,
            total_records,
            "All drives loaded — daemon ready"
        );
        self.events.emit(DaemonEvent::DaemonReady {
            drives: drive_count,
            total_records,
            startup_ms: self.start_time.elapsed().as_millis(),
        });
    }

    /// Per-drive load timeout.  If a single drive's MFT read takes
    /// longer than this, we skip it rather than blocking the entire
    /// daemon.  Raw NTFS volume reads can hang indefinitely when a
    /// drive is unresponsive (bad sectors, sleep, USB disconnect).
    #[cfg(windows)]
    const DRIVE_LOAD_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(300);

    /// Load live Windows drives — **all drives in parallel**.
    ///
    /// Each drive's MFT read runs on its own blocking thread. Results are
    /// collected via `JoinSet` as they complete (fastest drive first), giving
    /// accurate incremental progress and cutting total wall time from
    /// `sum(per-drive)` to `max(per-drive)`.
    ///
    /// Each drive has a [`Self::DRIVE_LOAD_TIMEOUT`] — if exceeded the drive
    /// is skipped and an error is logged.  This prevents a single stuck
    /// volume from making the daemon unkillable.
    #[cfg(windows)]
    #[expect(
        clippy::print_stderr,
        reason = "[diag] diagnostic tracing — remove after D: drive issue is resolved"
    )]
    #[expect(
        clippy::use_debug,
        reason = "[diag] diagnostic tracing — remove after D: drive issue is resolved"
    )]
    pub(crate) async fn load_live_drives(
        &self,
        drives: &[uffs_mft::platform::DriveLetter],
        no_cache: bool,
        lifecycle: &crate::lifecycle::LifecycleHandle,
    ) {
        let total = drives.len();
        eprintln!("[diag] load_live_drives: starting — drives={drives:?}  no_cache={no_cache}");
        self.set_loading_progress(0, total).await;

        let join_set = Self::spawn_drive_loaders(drives, no_cache);
        self.collect_drive_load_results(join_set, total, lifecycle)
            .await;

        // Final allocator purge after all drives are loaded.
        release_allocator_pages();

        // Retune the search-concurrency semaphore to match the loaded
        // drive count before admitting queries (see `tune_concurrency`).
        self.tune_concurrency().await;

        self.set_ready().await;
        self.emit_daemon_ready_summary().await;
    }

    /// Update the daemon's [`DaemonStatus`] to reflect ongoing load
    /// progress.  Wraps the brief `RwLock` window in a helper so callers
    /// don't smear lock-guard scope across the orchestrator.
    #[cfg(windows)]
    async fn set_loading_progress(&self, loaded: usize, total: usize) {
        let mut status = self.status.write().await;
        *status = DaemonStatus::Loading {
            drives_loaded: loaded,
            drives_total: total,
        };
    }

    /// Spawn one blocking task per drive against the global blocking pool.
    /// Each task returns `(letter, Result<(DriveCompactIndex, LoadTiming)>)`.
    #[cfg(windows)]
    #[expect(
        clippy::print_stderr,
        reason = "[diag] diagnostic tracing — remove after D: drive issue is resolved"
    )]
    fn spawn_drive_loaders(
        drives: &[uffs_mft::platform::DriveLetter],
        no_cache: bool,
    ) -> tokio::task::JoinSet<(
        uffs_mft::platform::DriveLetter,
        anyhow::Result<(
            uffs_core::compact::DriveCompactIndex,
            uffs_core::compact::LoadTiming,
        )>,
    )> {
        let mut join_set = tokio::task::JoinSet::new();
        for &letter in drives {
            tracing::info!(drive = %letter, "Loading live drive (parallel)");
            eprintln!("[diag] load_live_drives: spawning thread for drive={letter}");
            join_set.spawn_blocking(move || {
                // Guarded warm load: serve the on-disk compact cache fast
                // when the background USN journal loop can converge the
                // bounded delta, falling back to a synchronous rebuild
                // only when it cannot (see `cache::guarded_load`).
                let result = crate::cache::guarded_load::load_live_drive(letter, no_cache);
                (letter, result)
            });
        }
        join_set
    }

    /// Drain `join_set` until every drive task finishes or any single
    /// task overruns [`Self::DRIVE_LOAD_TIMEOUT`].  Each completion
    /// updates the daemon status so clients see incremental progress.
    #[cfg(windows)]
    #[expect(
        clippy::print_stderr,
        reason = "[diag] diagnostic tracing — remove after D: drive issue is resolved"
    )]
    async fn collect_drive_load_results(
        &self,
        mut join_set: tokio::task::JoinSet<(
            uffs_mft::platform::DriveLetter,
            anyhow::Result<(
                uffs_core::compact::DriveCompactIndex,
                uffs_core::compact::LoadTiming,
            )>,
        )>,
        total: usize,
        lifecycle: &crate::lifecycle::LifecycleHandle,
    ) {
        let mut loaded: usize = 0;
        loop {
            let next = tokio::time::timeout(Self::DRIVE_LOAD_TIMEOUT, join_set.join_next()).await;
            match next {
                Ok(Some(join_result)) => {
                    self.handle_drive_load_result(join_result, &mut loaded, total)
                        .await;

                    // Return freed pages to the OS after each drive load.
                    // The MftIndex (~3 GB for large drives) was dropped
                    // inside load_drive(), but the system allocator
                    // retains those pages as committed virtual memory.
                    release_allocator_pages();

                    // Heartbeat — tells the idle timer we're still
                    // making progress, preventing a false stall-timeout.
                    lifecycle.record_load_progress();
                    self.set_loading_progress(loaded, total).await;
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    let remaining = total.saturating_sub(loaded);
                    eprintln!(
                        "[diag] load_live_drives: TIMEOUT — {remaining} drive(s) stuck after {}s",
                        Self::DRIVE_LOAD_TIMEOUT.as_secs()
                    );
                    tracing::error!(
                        remaining,
                        timeout_secs = Self::DRIVE_LOAD_TIMEOUT.as_secs(),
                        "Drive load timed out — skipping remaining drives"
                    );
                    // Abort the remaining stuck tasks (best-effort;
                    // kernel-mode I/O may not be interruptible, but
                    // process::exit at daemon shutdown will clean up).
                    //
                    // We intentionally do NOT update `loaded` here: the
                    // surrounding loop is about to `break`, and
                    // `set_ready()` transitions the status out of
                    // `Loading` regardless — so any write to `loaded`
                    // would be dead.  Stuck-drive observability is
                    // carried by the `remaining` count logged above.
                    join_set.abort_all();
                    break;
                }
            }
        }
    }

    /// Apply one [`tokio::task::JoinSet::join_next`] outcome: install a
    /// successful drive into the index, log a partial failure, or log a
    /// task panic — and bump `loaded` exactly once per outcome.
    #[cfg(windows)]
    #[expect(
        clippy::print_stderr,
        reason = "[diag] diagnostic tracing — remove after D: drive issue is resolved"
    )]
    async fn handle_drive_load_result(
        &self,
        join_result: Result<
            (
                uffs_mft::platform::DriveLetter,
                anyhow::Result<(
                    uffs_core::compact::DriveCompactIndex,
                    uffs_core::compact::LoadTiming,
                )>,
            ),
            tokio::task::JoinError,
        >,
        loaded: &mut usize,
        total: usize,
    ) {
        match join_result {
            Ok((letter, Ok((drive_index, timing)))) => {
                *loaded += 1;
                self.install_loaded_drive(letter, drive_index, timing, *loaded, total)
                    .await;
            }
            Ok((letter, Err(err))) => {
                *loaded += 1;
                eprintln!("[diag] load_live_drives: FAILED drive={letter}  error={err:#}");
                tracing::error!(drive = %letter, error = %err, "Failed to load live drive");
            }
            Err(err) => {
                *loaded += 1;
                eprintln!("[diag] load_live_drives: PANIC in task  error={err}");
                tracing::error!(error = %err, "Task panicked loading drive");
            }
        }
    }

    /// Persist a successfully-loaded drive: emit a `DriveLoaded` event,
    /// stash its timing for profile reporting, and add the compact
    /// index to the live snapshot.
    #[cfg(windows)]
    async fn install_loaded_drive(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        drive_index: uffs_core::compact::DriveCompactIndex,
        timing: uffs_core::compact::LoadTiming,
        loaded: usize,
        total: usize,
    ) {
        let records = drive_index.records.len();
        tracing::info!(
            drive = %letter,
            records,
            mft_ms = timing.mft,
            compact_ms = timing.compact,
            trigram_ms = timing.trigram,
            loaded,
            total,
            "Live drive loaded"
        );
        self.events.emit(DaemonEvent::DriveLoaded {
            drive: letter,
            records,
            mft_ms: timing.mft,
            compact_ms: timing.compact,
            trigram_ms: timing.trigram,
            drives_loaded: loaded,
            drives_total: total,
        });
        self.drive_timings
            .write()
            .await
            .insert(letter, StoredDriveTiming {
                cache: timing.cache,
                mft: timing.mft,
                compact: timing.compact,
                trigram: timing.trigram,
            });
        self.add_drive(drive_index).await;
    }

    /// Emit the post-load `DaemonReady` event and the cumulative heap
    /// summary.  Extracted to keep [`Self::load_live_drives`] flat.
    #[cfg(windows)]
    async fn emit_daemon_ready_summary(&self) {
        let snap = self.snapshot().await;
        let drive_count = snap.drives.len();
        let total_records = snap.total_records();

        let mut total_heap: u64 = 0;
        for dr in &snap.drives {
            dr.log_heap_report();
            total_heap += dr.heap_size_bytes().total as u64;
        }
        let heap_mb = total_heap / (1024 * 1024);
        tracing::info!(
            drives = drive_count,
            total_records,
            index_heap_mb = heap_mb,
            "[MEM] All drives loaded: index heap = {} MB",
            heap_mb,
        );
        drop(snap);

        self.events.emit(DaemonEvent::DaemonReady {
            drives: drive_count,
            total_records,
            startup_ms: self.start_time.elapsed().as_millis(),
        });
    }

    /// Transition to `Ready` and record startup duration (idempotent).
    pub(super) async fn set_ready(&self) {
        let mut status = self.status.write().await;
        *status = DaemonStatus::Ready;
        drop(status);
        // Record only the first transition.
        let elapsed_us = u64::try_from(self.start_time.elapsed().as_micros()).unwrap_or(u64::MAX);
        // Only record the first transition; ignore the result.
        let _already_set = self.startup_duration_us.compare_exchange(
            0,
            elapsed_us,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    // ── Atomic drive mutations ─────────────────────────────────────

    /// Add a drive to the shared index via atomic pointer swap.
    ///
    /// Clones the `Vec` of `Arc` pointers (< 100 bytes), appends the new
    /// drive, and swaps.  In-flight queries keep the old snapshot.
    /// Bumps `index_version` and invalidates the aggregate cache so
    /// cached results from the previous snapshot can't leak into the
    /// new one.
    pub(super) async fn add_drive(&self, drive: uffs_core::compact::DriveCompactIndex) {
        let body = Arc::new(drive);
        let letter = body.letter;
        let now_ms = unix_now_ms();
        let mut guard = self.index.write().await;
        // ShardRegistry::add identifies the new shard by `body.letter`
        // (its canonical case from the index payload) — callers don't
        // thread the letter separately so it can't drift.
        let new_registry = guard.add(body);
        // Phase 3 Commit D — seed the load timestamp on the freshly
        // mounted shard so the demote-controller's idle clock starts
        // ticking from now, not from epoch zero.  See
        // `DriveStats::mark_loaded_at` doc.
        if let Some(shard) = new_registry.iter().find(|shard| shard.drive == letter) {
            shard.stats.mark_loaded_at(now_ms);
        }
        *guard = Arc::new(new_registry);
        drop(guard);
        self.bump_index_version();
    }

    /// Replace a drive by letter (for refresh) via atomic pointer swap.
    ///
    /// Builds a new snapshot with the old drive removed and the new one
    /// appended.  Write lock held for < 1 μs (pointer swap only).
    /// Bumps `index_version` so the aggregate cache drops entries
    /// computed against the pre-refresh snapshot.
    pub(super) async fn replace_drive(
        &self,
        letter: uffs_mft::platform::DriveLetter,
        new_drive: uffs_core::compact::DriveCompactIndex,
    ) {
        let body = Arc::new(new_drive);
        let canonical = body.letter;
        let now_ms = unix_now_ms();
        let mut guard = self.index.write().await;
        // `ShardRegistry::replace` matches case-insensitively, mirroring
        // the previous `eq_ignore_ascii_case` filter on `DriveIndex`.
        let new_registry = guard.replace(letter, body);
        // Phase 3 Commit D — same load-timestamp seeding as add_drive.
        // The replaced shard gets a fresh `Arc<DriveStats>` (replace
        // builds a new ShardEntry), so we don't need to preserve any
        // older counters here.
        if let Some(shard) = new_registry.iter().find(|shard| shard.drive == canonical) {
            shard.stats.mark_loaded_at(now_ms);
        }
        *guard = Arc::new(new_registry);
        drop(guard);
        self.bump_index_version();
    }
}
