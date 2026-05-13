// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! UFFS Daemon library — reusable daemon entry point.
//!
//! This crate exposes [`run_daemon`] so the daemon logic can be invoked
//! both from the standalone `uffs-daemon` binary and from the embedded
//! `uffs daemon run` subcommand in the CLI.
//!
//! Exception: `file_size_policy` allows this file to exceed 800 LOC.
//! Rationale: `run_daemon` plus the cohesive cluster of background-
//! task spawners (`spawn_load_task`, `spawn_ipc_servers`,
//! `spawn_stats_heartbeat`, `spawn_idle_demote_controller`,
//! `spawn_journal_loops_for_warm_shards`, `spawn_pressure_subscriber`)
//! form the daemon's startup graph; splitting the controllers
//! across sibling modules fragments the shared `DaemonConfig` /
//! `EventSender` wiring and obscures the parent-task lifetime
//! relationships between them.

// Enable unstable Windows Unix domain socket support (Windows 10 1803+).
#![cfg_attr(windows, feature(windows_unix_domain_sockets))]

extern crate alloc;

use alloc::sync::Arc;
use std::path::PathBuf;

// Suppress unused crate warnings for deps used by sub-modules, the binary, or
// behind cfg gates.
use clap as _;
use dirs_next as _;
use mimalloc as _;
use serde as _;
use thiserror as _;
use tracing_appender as _;
use tracing_subscriber as _;
use uffs_mft as _;

/// Broker client — volume handle requests (Windows) / stubs (other).
mod broker_client;
/// Shard-based index cache with per-drive lifecycle.
///
/// Phase 1 of the memory-tiering work — see
/// `docs/refactor/memory-tiering-implementation-plan.md`.
mod cache;
/// `daemon.toml` parser — Phase 6 of memory-tiering.
///
/// Schema mirrors plan §11; defaults match Phase-3 static behavior.
/// The type is named [`config::Config`] (idiomatic
/// `crate::module::Type`) to avoid collision with the existing
/// [`DaemonConfig`] runtime-args wrapper that this file owns.
/// Commit C wires the loader into [`run_daemon`] startup and
/// replaces the env-var-overridable static getters in
/// [`crate::cache::policy`] with config-driven readers.
mod config;
/// Daemon event broadcasting — push notifications to connected clients.
pub mod events;
/// JSON-RPC request handler.
mod handler;
/// Index manager — loads and queries MFT data.
mod index;
/// IPC server — Unix domain socket / named pipe listener.
mod ipc;
/// Lifecycle manager — PID file, idle timer, shutdown coordination.
mod lifecycle;
/// JSON-RPC protocol types.
mod protocol;
/// Phase 2b memory-tiering: runtime-tempfile orphan cleanup at boot.
mod runtime_orphans;
/// Process-level memory and runtime telemetry.
pub(crate) mod telemetry;

/// Default log file location: `<data-local-dir>/uffs/uffsd.log`.
///
/// Falls back to `./uffsd.log` if the platform data directory
/// cannot be determined.
#[must_use]
pub(crate) fn default_log_file() -> PathBuf {
    dirs_next::data_local_dir().map_or_else(
        || PathBuf::from("uffsd.log"),
        |dir| dir.join("uffs").join("uffsd.log"),
    )
}

/// Initialise tracing for the daemon process.
///
/// * `log_file = Some(path)` — write to that file (append mode). A path of
///   `"-"` or empty string uses `default_log_file`.
/// * `log_file = None` **and** the effective log level is `debug` or `trace` —
///   automatically write to `default_log_file` so that diagnostic output is
///   never lost to `/dev/null`.
/// * `log_file = None` with a higher level — write to stdout.
///
/// Returns a guard that **must** be held until the daemon exits —
/// dropping it flushes the non-blocking writer.
#[must_use]
pub fn init_tracing(
    log_spec: &str,
    log_file: Option<&std::path::Path>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = tracing_subscriber::EnvFilter::try_new(log_spec)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Decide whether to use a file writer.
    let is_verbose = {
        let lower = log_spec.to_ascii_lowercase();
        lower.contains("debug") || lower.contains("trace")
    };
    let effective_file: Option<PathBuf> = match log_file {
        Some(path) => {
            let resolved = if path.as_os_str().is_empty() || path == std::path::Path::new("-") {
                default_log_file()
            } else {
                path.to_path_buf()
            };
            Some(resolved)
        }
        None if is_verbose => Some(default_log_file()),
        None => None,
    };

    if let Some(resolved) = effective_file {
        // Compute a *safe* parent directory.
        //
        // `PathBuf::from("uffsd.log").parent()` returns `Some(Path::new(""))`,
        // not `None` — so the defensive `unwrap_or_else(|| Path::new("."))`
        // below used to never fire for a relative file name, and
        // `tracing_appender::rolling::never("", "uffsd.log")` would propagate
        // the empty path through `create_dir_all("")`, which errors on
        // Windows ("The system cannot find the path specified") and then
        // panics via `.expect("initializing rolling file appender failed")`
        // — killing the detached daemon before it ever binds IPC.
        //
        // Coerce both `None` and `Some("")` to the current directory so
        // relative `--log-file` paths work the same everywhere.
        let parent_dir = match resolved.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => std::path::Path::new("."),
        };
        let _mkdir_ignore = std::fs::create_dir_all(parent_dir);

        let file_appender = tracing_appender::rolling::never(
            parent_dir,
            resolved
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("uffsd.log")),
        );
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        // `try_init` — a subscriber may already exist when invoked via
        // the embedded `uffs daemon run` path.
        let _ignore = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_ansi(false)
            .with_writer(non_blocking)
            .try_init();
        Some(guard)
    } else {
        let _ignore = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .try_init();
        None
    }
}

/// Configuration for [`run_daemon`].
pub struct DaemonConfig {
    /// MFT files to load.
    pub mft_files: Vec<PathBuf>,
    /// Data directory containing `drive_*` subdirectories.
    pub data_dir: Option<PathBuf>,
    /// Explicit drive letters (Windows only).
    pub drives: Vec<char>,
    /// Idle timeout in seconds (0 = use default 7200s / 2 hours).
    pub idle_timeout: u64,
    /// Disable auto-retire.
    pub no_retire: bool,
    /// Skip cache.
    pub no_cache: bool,
    /// Log level string (e.g. "info", "debug").
    pub log_level: String,
    /// Optional log file path.  When set, daemon tracing output is
    /// written to this file instead of stdout.  If the value is empty
    /// or `"-"`, the daemon defaults to `./uffs_daemon.log` in the
    /// current working directory.
    pub log_file: Option<PathBuf>,
}

/// Bail if the daemon has nothing to serve.
fn validate_data_sources(
    mft_files: &[PathBuf],
    drives: &[char],
    lifecycle_mgr: &lifecycle::LifecycleManager,
) -> anyhow::Result<()> {
    let has_data = !mft_files.is_empty() || {
        #[cfg(windows)]
        {
            !drives.is_empty()
        }
        #[cfg(not(windows))]
        {
            // `drives` is Windows-only; no auto-discovery on macOS/Linux.
            // The explicit type pins the annotation clippy expects on
            // discarded bindings.
            let _: &[char] = drives;
            false
        }
    };
    if !has_data {
        tracing::error!(
            "No data sources provided. On macOS/Linux pass --mft-file; \
             on Windows, NTFS drives are auto-discovered."
        );
        lifecycle_mgr.remove_pid_file();
        anyhow::bail!(
            "Daemon has no data sources to load. \
             Provide --mft-file <path> (or --data-dir when launching via CLI)."
        );
    }
    Ok(())
}

/// Run the UFFS daemon with the given configuration.
///
/// This is the main entry point shared by both the standalone
/// `uffs-daemon` binary and the embedded `uffs daemon run` subcommand.
///
/// **Does not return** until the daemon shuts down (idle timeout,
/// RPC shutdown, or signal).
///
/// # Errors
///
/// Returns an error if another daemon is already running, data sources
/// are missing, or the IPC server fails to bind.
pub async fn run_daemon(config: DaemonConfig) -> anyhow::Result<()> {
    install_catastrophe_panic_hook();
    log_daemon_starting(&config);

    let (event_tx, _event_rx) = events::event_channel();
    emit_daemon_starting_event(&event_tx);

    let lifecycle_mgr = bootstrap_lifecycle_manager(&config, event_tx.clone())?;

    // D5.0: clean up stale shmem files from previous daemon sessions.
    uffs_client::shmem::cleanup_stale_shmem_files();

    // Phase 2b: wipe runtime-tempfile leftovers from dead daemon PIDs
    // before our own PID-scoped subdir gets created by the first
    // `load_compact_cache` call.  The cross-platform `cleanup_orphans`
    // contract makes this safe even when other live daemons share the
    // root directory.
    runtime_orphans::sweep_runtime_tempfile_orphans();

    // Phase 6 Commit C task 6.5 — resolve `daemon.toml` from the
    // platform-default location.  Factored out to keep
    // `run_daemon`'s cognitive complexity under the workspace's
    // strict-clippy ceiling.
    let daemon_config = load_daemon_config()?;

    // Create index manager — uses the user-supplied --data-dir for offline MFT
    // discovery and hot-loading (not the lifecycle directory).
    let idx = Arc::new(index::IndexManager::new(
        config.data_dir.clone(),
        event_tx,
        Arc::clone(&daemon_config),
    ));
    tracing::debug!(index_data_dir = ?idx.data_dir(), "Index manager created");

    let mft_files = gather_mft_files(&config);
    let drives = resolve_drive_list(&config);
    tracing::info!(mft_files = mft_files.len(), drives = ?drives, "Final data sources");

    // Refuse to start with zero data sources — an empty daemon is useless.
    validate_data_sources(&mft_files, &drives, &lifecycle_mgr)?;
    tracing::info!("Data sources validated OK");

    let load_task = spawn_load_task(
        Arc::clone(&idx),
        mft_files,
        drives,
        config.no_cache,
        lifecycle_mgr.handle(),
    );

    let ipc_task = spawn_ipc_servers(&idx, &lifecycle_mgr);
    let _stats_task = spawn_stats_heartbeat(Arc::clone(&idx), lifecycle_mgr.handle());
    let _mem_snapshot_task = telemetry::spawn_mem_snapshot_task(
        Arc::clone(&idx),
        telemetry::DEFAULT_MEM_SNAPSHOT_INTERVAL,
    );
    // Phase 3 Commit D — periodic shard idle-demote sweep.
    let _idle_demote_task = spawn_idle_demote_controller(Arc::clone(&idx));

    // Phase 7 activation: per-shard journal loops are spawned
    // inside `spawn_load_task` after `record_load_complete()`.  They
    // replace the deleted Phase-5 `spawn_usn_refresh_controller`
    // 5-min global tick with per-letter event-driven refresh — see
    // `spawn_journal_loops_for_warm_shards` below.

    // Phase 5 task 5.6 — memory-pressure subscriber.  Cascade-
    // demotes LRU Warm shards on `Low` transitions until pressure
    // clears (`High`) or no Warm shards remain.  No-op on Mac/Linux
    // (the platform `PressureSignal` never fires).
    let _pressure_task = spawn_pressure_subscriber(Arc::clone(&idx));

    // Run idle timer (blocks until shutdown or timeout) then tear
    // everything down.  Returns `!` so `force_exit_with_watchdog`
    // covers the post-await tail.
    await_shutdown_then_force_exit(lifecycle_mgr, ipc_task, load_task).await
}

/// Emit the startup `tracing::info!` line with every config field
/// the operator might want to grep for.  Extracted so the orchestrator
/// stays under clippy's `cognitive_complexity` budget.
fn log_daemon_starting(config: &DaemonConfig) {
    tracing::info!(
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        broker_available = broker_client::broker_available(),
        mft_files = ?config.mft_files,
        drives = ?config.drives,
        data_dir = ?config.data_dir,
        no_cache = config.no_cache,
        no_retire = config.no_retire,
        "uffsd starting"
    );
}

/// Publish the [`DaemonEvent::DaemonStarting`] notification so any
/// pre-IPC subscriber (e.g. the embedded MCP server) sees the
/// transition.
fn emit_daemon_starting_event(event_tx: &events::EventSender) {
    event_tx.emit(events::DaemonEvent::DaemonStarting {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    });
}

/// Wait for the idle timer / shutdown signal, then run the graceful
/// shutdown sequence: abort the IPC task, timeout-join the load task,
/// drop the lifecycle manager (which cleans up PID + socket files),
/// and finally force-exit via the watchdog.
///
/// Returns `!` because both legitimate exits (clean shutdown, watchdog
/// abort) terminate the process.
async fn await_shutdown_then_force_exit(
    mut lifecycle_mgr: lifecycle::LifecycleManager,
    ipc_task: tokio::task::JoinHandle<()>,
    load_task: tokio::task::JoinHandle<()>,
) -> ! {
    lifecycle_mgr.run_idle_timer().await;

    tracing::info!("Daemon shutting down");
    ipc_task.abort();
    // Give the load task a brief window to finish, then abandon it.
    // Stuck kernel-mode I/O threads cannot be cancelled, so we don't
    // wait indefinitely — process::exit at the bottom will clean up.
    let shutdown_deadline = tokio::time::timeout(core::time::Duration::from_secs(3), load_task);
    let _ignore = shutdown_deadline.await;
    tracing::info!("Daemon stopped");

    // Clean up PID + socket files before exiting.
    drop(lifecycle_mgr);

    force_exit_with_watchdog()
}

/// Install a panic hook that runs the existing default hook (so the
/// usual stack trace + payload still print) and then force-exits.
///
/// Without this, a panic on any blocking I/O thread can leave the
/// daemon in a zombie state — the default hook tries to unwind through
/// kernel-mode I/O which may never return.  Force-exiting with code
/// `101` matches Rust's standard panic exit code so process supervisors
/// don't see a "clean" 0-exit on a panic.
fn install_catastrophe_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        #[expect(clippy::exit, reason = "catastrophe safety net — force-exit on panic")]
        {
            std::process::exit(101);
        }
    }));
}

/// Resolve the operator's `daemon.toml` from the platform-default
/// location and emit a structured `tracing::info!` event with the
/// resolved path.
///
/// Phase 6 Commit C task 6.5 helper.  A missing file is **not** an
/// error: [`config::Config::load_default`] returns the
/// Phase-3-equivalent defaults so every existing deployment boots
/// with the same observable behavior (plan task 6.8).  A malformed
/// file propagates as a startup error so a typo doesn't silently
/// fall back to defaults — the operator gets a precise parser error
/// with line and column.
///
/// Returned as `Arc<Config>` so the index manager and any future
/// background controller can share a single read-only view without
/// cloning the BTreeMap-bearing `[shards.per_drive]` table.
fn load_daemon_config() -> anyhow::Result<Arc<config::Config>> {
    let cfg = config::Config::load_default()
        .map_err(|err| anyhow::anyhow!("Failed to load daemon.toml from default path: {err}"))?;
    tracing::info!(
        daemon_config_path = ?config::Config::default_path(),
        "daemon.toml resolved (or defaults used when missing)",
    );
    Ok(Arc::new(cfg))
}

/// Build the [`LifecycleManager`], gate against another running
/// instance via the PID file, and write a fresh PID file.
///
/// Returns the manager ready for use, or bails when another daemon is
/// already alive.
fn bootstrap_lifecycle_manager(
    config: &DaemonConfig,
    event_tx: events::EventSender,
) -> anyhow::Result<lifecycle::LifecycleManager> {
    // Determine data directory:
    // - lifecycle_dir: always %LOCALAPPDATA%\uffs — PID/socket/lock files
    // - data_dir: user-supplied --data-dir (for MFT file discovery/hot-load)
    let lifecycle_dir = dirs_next::data_local_dir()
        .map_or_else(|| PathBuf::from("/tmp/uffs"), |base| base.join("uffs"));

    let idle_timeout = if config.no_retire {
        None
    } else {
        Some(core::time::Duration::from_secs(config.idle_timeout))
    };
    let mut lifecycle_mgr =
        lifecycle::LifecycleManager::new(&lifecycle_dir, idle_timeout, event_tx);

    tracing::info!(data_dir = %lifecycle_mgr.data_dir().display(), "Lifecycle data directory");

    if !lifecycle_mgr.check_stale_pid() {
        tracing::error!("Another daemon instance is already running");
        anyhow::bail!("Another daemon instance is already running");
    }

    lifecycle_mgr.write_pid_file()?;
    tracing::info!("PID file written");
    Ok(lifecycle_mgr)
}

/// Merge `--mft-file` arguments with files discovered under
/// `--data-dir`, applying the `--drive` filter when present.
fn gather_mft_files(config: &DaemonConfig) -> Vec<PathBuf> {
    let mut mft_files = config.mft_files.clone();
    let Some(dir) = config.data_dir.as_ref() else {
        return mft_files;
    };

    let discovered = uffs_mft::discovery::discover_mft_files(dir);
    let filtered: Vec<PathBuf> = if config.drives.is_empty() {
        discovered
    } else {
        discovered
            .into_iter()
            .filter(|path| drive_letter_matches(path, &config.drives))
            .collect()
    };
    tracing::info!(
        data_dir = %dir.display(),
        count = filtered.len(),
        filter = ?config.drives,
        "Discovered MFT files from --data-dir"
    );
    mft_files.extend(filtered);
    mft_files
}

/// Returns `true` when `path`'s parent directory carries a
/// `drive_<letter>` prefix that matches one of `wanted` (case-
/// insensitive).
fn drive_letter_matches(path: &std::path::Path, wanted: &[char]) -> bool {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("drive_"))
        .and_then(|suffix| suffix.chars().next())
        .is_some_and(|letter| {
            wanted
                .iter()
                .any(|drive| drive.eq_ignore_ascii_case(&letter))
        })
}

/// Resolve the drive list to scan.
///
/// On Windows, an empty `--drive` triggers auto-discovery; non-empty
/// respects the explicit list.  Always empty on non-Windows since
/// live MFT scanning is Windows-only.
#[cfg(windows)]
fn resolve_drive_list(config: &DaemonConfig) -> Vec<char> {
    let explicit = config.drives.clone();
    if explicit.is_empty() {
        let auto_drives = uffs_mft::detect_ntfs_drives();
        tracing::info!(
            count = auto_drives.len(),
            drives = ?auto_drives,
            "Auto-discovered NTFS drives (no --drive flag)"
        );
        auto_drives
    } else {
        tracing::info!(
            drives = ?explicit,
            "Loading only requested drives (--drive flag)"
        );
        explicit
    }
}

/// Non-Windows variant: live MFT scanning is unsupported, so the
/// drive list is always empty regardless of `config`.
#[cfg(not(windows))]
const fn resolve_drive_list(_config: &DaemonConfig) -> Vec<char> {
    Vec::new()
}

/// Spawn the parallel load task that reads `mft_files` from disk and
/// (on Windows) the live drives via the broker / direct path.  The
/// task also enforces the zero-drive shutdown guard so a daemon that
/// fails every load doesn't linger in `Ready` with an empty index.
fn spawn_load_task(
    load_index: Arc<index::IndexManager>,
    mft_files: Vec<PathBuf>,
    drives: Vec<char>,
    no_cache: bool,
    load_lifecycle: lifecycle::LifecycleHandle,
) -> tokio::task::JoinHandle<()> {
    let broker_is_available = broker_client::broker_available();
    tokio::spawn(async move {
        tracing::info!(mft_files = mft_files.len(), drives = ?drives, "Load task starting");
        if !mft_files.is_empty() {
            tracing::info!("Loading MFT files from data dir...");
            load_index.load_from_data_dir(&mft_files, no_cache).await;
            tracing::info!("MFT files loaded");
        }
        // Live-MFT scanning is Windows-only; on every other platform the
        // load task is fully covered by `load_from_data_dir` above.
        #[cfg(windows)]
        load_live_drives_if_windows(
            &load_index,
            &drives,
            no_cache,
            &load_lifecycle,
            broker_is_available,
        )
        .await;
        if broker_is_available {
            let _handle_result = broker_client::request_volume_handle('C');
        }
        tracing::info!("Load task completed");

        // Latch the load phase as complete: from this point on, a daemon
        // serving zero queries against fully-loaded drives is legitimately
        // idle (not stalled), so the load-stall force-retire guard is
        // permanently disarmed.  Per-drive stuck-load detection remains
        // active via `DRIVE_LOAD_TIMEOUT` inside the load loop itself.
        // Closes the Phase 5 G4 finding (LOG/uffsd-G4-bonus.log line 104,
        // 2 h 11 min force-retire on a healthy idle daemon).
        load_lifecycle.record_load_complete();

        // Phase 7 activation: spawn one per-shard journal loop for
        // every loaded letter.  Replaces the deleted Phase-5 global
        // 5-min `refresh_usn_for_warm_shards` tick.  The applier
        // task's JoinHandle is fire-and-forget here — it stays alive
        // for the daemon's lifetime via the `Arc<RegistryPatchSink>`
        // captured by the per-loop sink clones; on shutdown the
        // applier exits cleanly when the last sink Arc drops.
        let _journal_applier = spawn_journal_loops_for_warm_shards(&load_index).await;

        zero_drive_shutdown_guard(&load_index, &load_lifecycle).await;
    })
}

/// Windows live-drive load step.  Walks the broker for an elevated
/// handle per drive (best-effort), then drives `load_live_drives` to
/// scan their MFTs.
#[cfg(windows)]
async fn load_live_drives_if_windows(
    load_index: &Arc<index::IndexManager>,
    drives: &[char],
    no_cache: bool,
    load_lifecycle: &lifecycle::LifecycleHandle,
    broker_is_available: bool,
) {
    if drives.is_empty() {
        return;
    }
    if broker_is_available {
        warm_up_broker_handles(drives);
    }
    tracing::info!(drives = ?drives, "Loading live drives...");
    load_index
        .load_live_drives(drives, no_cache, load_lifecycle)
        .await;
    tracing::info!("Live drives loaded");
}

/// Best-effort broker pre-warm: ask the elevated broker for a volume
/// handle per drive so the subsequent `load_live_drives` skips the
/// per-drive elevation prompt.  Failures are debug-traced and
/// ignored — the direct-open path takes over transparently.
#[cfg(windows)]
fn warm_up_broker_handles(drives: &[char]) {
    for &drive_letter in drives {
        match broker_client::request_volume_handle(drive_letter) {
            Ok(handle) => {
                tracing::info!(drive = %drive_letter, handle, "Got broker handle");
            }
            Err(broker_err) => {
                tracing::debug!(
                    drive = %drive_letter,
                    error = %broker_err,
                    "Broker unavailable, using direct access"
                );
            }
        }
    }
}

/// Catch the "every load failed but `Ready` fired anyway" zombie
/// state.  Triggers an explicit shutdown request when the post-load
/// drive count is zero so the lifecycle's `select!` tears the daemon
/// down cleanly instead of leaving it queryable-but-empty.
async fn zero_drive_shutdown_guard(
    load_index: &Arc<index::IndexManager>,
    load_lifecycle: &lifecycle::LifecycleHandle,
) {
    let loaded_drives = load_index.loaded_drive_letters().await;
    if loaded_drives.is_empty() {
        tracing::error!(
            "Daemon loaded zero drives even though data sources were provided — every \
             parse attempt failed.  Shutting down to avoid the Ready-with-no-data state. \
             Check the load errors above; common causes: missing/corrupt .iocp files, \
             insufficient privileges on live drives, or a data_dir that contains no MFT \
             captures."
        );
        load_lifecycle.request_shutdown();
    }
}

/// Spawn the IPC server task(s).
///
/// Always spawns the `AF_UNIX` listener (the cross-platform fallback);
/// on Windows additionally spawns the named-pipe listener which is
/// the preferred transport (no `ws2_32` dependency, ~54 ms faster per
/// CLI launch).  Returns the `AF_UNIX` `JoinHandle` so the caller can
/// `.abort()` it during graceful shutdown; the named-pipe task is
/// fire-and-forget — the watchdog will reap it.
fn spawn_ipc_servers(
    idx: &Arc<index::IndexManager>,
    lifecycle_mgr: &lifecycle::LifecycleManager,
) -> tokio::task::JoinHandle<()> {
    let ipc_index = Arc::clone(idx);
    let ipc_lifecycle = lifecycle_mgr.handle();

    tracing::info!("Starting IPC server...");
    let ipc_task = tokio::spawn(async move {
        if let Err(ipc_err) = ipc::run_ipc_server(ipc_index, ipc_lifecycle).await {
            tracing::error!(error = %ipc_err, "IPC server error");
        }
    });
    tracing::info!("IPC server task spawned");

    #[cfg(windows)]
    let _pipe_task = {
        let pipe_index = Arc::clone(idx);
        let pipe_lifecycle = lifecycle_mgr.handle();
        tracing::info!("Starting named-pipe IPC server...");
        tokio::spawn(async move {
            if let Err(pipe_err) = ipc::run_pipe_server(pipe_index, pipe_lifecycle).await {
                tracing::error!(error = %pipe_err, "Named-pipe IPC server error");
            }
        })
    };

    ipc_task
}

/// Spawn the periodic stats heartbeat — pushes `StatsHeartbeat`
/// events to all connected clients every 30 seconds.
fn spawn_stats_heartbeat(
    stats_index: Arc<index::IndexManager>,
    stats_lifecycle: lifecycle::LifecycleHandle,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(core::time::Duration::from_secs(30));
        // Skip the first tick (fires immediately).
        interval.tick().await;
        loop {
            interval.tick().await;
            let total_records = stats_index.total_records().await;
            let stats = stats_index.stats().await;
            stats_index
                .event_sender()
                .emit(events::DaemonEvent::StatsHeartbeat {
                    total_queries: stats.total_queries,
                    uptime_secs: stats.uptime_secs,
                    total_records,
                    connections: stats_lifecycle.active_connections(),
                });
        }
    })
}

/// Spawn the Phase 3 idle-demote controller — every 30 s, ask
/// `IndexManager::demote_idle_shards` to walk the registry and
/// demote any shard whose idle time exceeds its tier's TTL (see
/// `cache::policy`).
///
/// The 30 s cadence is shorter than the shortest TTL (300 s
/// Hot→Warm) by an order of magnitude, so a freshly idle Hot
/// shard demotes within at most one tick of crossing its
/// boundary.  Sampling `unix_now_ms()` once at the top of each
/// tick (and threading it into the manager) keeps every shard's
/// idle-secs computation referenced to the same baseline — a
/// slow read-lock acquisition can't push later shards in the
/// same batch over a TTL boundary mid-walk.
fn spawn_idle_demote_controller(idx: Arc<index::IndexManager>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(core::time::Duration::from_secs(30));
        // Skip the first tick (fires immediately at task spawn).
        // We want the controller to take its first reading 30 s
        // after startup, not at t=0 when no shard could possibly
        // be idle yet.
        interval.tick().await;
        loop {
            interval.tick().await;
            idx.demote_idle_shards(cache::unix_now_ms()).await;
        }
    })
}

/// Phase 7 activation: spawn one [`crate::cache::journal_loop::JournalLoop`]
/// per loaded drive letter.
///
/// Replaces the deleted Phase-5 `spawn_usn_refresh_controller`
/// global 5-min tick with per-shard event-driven refresh.  Each
/// loop polls its drive's NTFS USN journal at the configured
/// cadence (default 500 ms via
/// [`crate::cache::journal_loop::JournalLoopConfig`]) and fires save triggers
/// when the per-shard `SaveTrigger`'s events-threshold (50K events) or
/// age-threshold (5 min) crosses. Each save trigger drives a full
/// [`crate::index::IndexManager::handle_journal_refresh`] which
/// reuses the Phase-5
/// [`uffs_core::compact_loader::load_drive_with_usn_refresh`] infrastructure
/// for the body refresh + compact-cache write.
///
/// All loops share a single [`crate::cache::journal_sink::RegistryPatchSink`]
/// applier task: the sink's mpsc channel preserves FIFO ordering
/// across letters and avoids re-entering the runtime from the
/// loop's sync `accept` callback — see the `journal_sink` module
/// docs for the full rationale.
///
/// **Platform split**:
/// * **Windows**: each letter gets a
///   [`crate::cache::journal_loop::sources::WindowsJournalSource`] (real
///   `FSCTL_QUERY_USN_JOURNAL` + `FSCTL_READ_USN_JOURNAL`) and a
///   [`crate::cache::cursor_store::DiskCursorStore`] rooted at
///   `uffs_mft::cache::cache_dir()`.
/// * **macOS / Linux**: each letter gets a
///   [`crate::cache::journal_loop::sources::MacStubJournalSource`]
///   (always-empty polls) and a
///   [`crate::cache::journal_loop::sources::NullCursorStore`] (no-op).  The
///   loop ticks at the configured cadence but produces no patches and triggers
///   no saves — there's no live USN journal to drive against.
///
/// Returns the [`tokio::task::JoinHandle`] for the applier task.
/// The caller (`run_daemon`) keeps it alive for the daemon's
/// lifetime; per-letter loop handles are stored on `IndexManager`
/// via [`crate::index::IndexManager::attach_journal_handle`].  On
/// daemon shutdown the per-loop `watch::Sender`s are dropped with
/// the `IndexManager`, signalling every loop's cancellation arm
/// (matching the existing fire-and-forget pattern used by
/// `spawn_idle_demote_controller` / `spawn_pressure_subscriber`).
async fn spawn_journal_loops_for_warm_shards(
    idx: &Arc<index::IndexManager>,
) -> tokio::task::JoinHandle<()> {
    use cache::cursor_store::DiskCursorStore;
    use cache::journal_loop::sources::NullCursorStore;
    use cache::journal_loop::{
        CursorStore, JournalLoopConfig, JournalSource, PatchSink, spawn_journal_loop,
    };
    use cache::journal_sink::RegistryPatchSink;

    let (sink, applier_handle) = RegistryPatchSink::spawn_with_applier(idx);
    let sink_dyn: Arc<dyn PatchSink> = sink;

    // Cursor-store choice: Windows persists per-drive cursors next
    // to the compact cache; Mac/Linux uses the always-zero
    // NullCursorStore (no journal → no cursor → fall back to
    // "start from journal head" semantics, which is the correct
    // no-op on those platforms).
    let cursor_store: Arc<dyn CursorStore> = if cfg!(windows) {
        Arc::new(DiskCursorStore::new(uffs_mft::cache::cache_dir()))
    } else {
        Arc::new(NullCursorStore)
    };

    let config = JournalLoopConfig::default();
    let letters = idx.loaded_drive_letters().await;
    tracing::info!(
        target: "shard.journal",
        count = letters.len(),
        letters = ?letters,
        poll_interval_ms = config.poll_interval.as_millis(),
        save_threshold_events = config.save_threshold_events,
        save_threshold_age_secs = config.save_threshold_age.as_secs(),
        "Spawning per-shard journal loops",
    );
    for letter in letters {
        let source: Arc<dyn JournalSource> = make_journal_source(letter);
        let handle = spawn_journal_loop(
            letter,
            source,
            Arc::clone(&sink_dyn),
            Arc::clone(&cursor_store),
            config.clone(),
        );
        idx.attach_journal_handle(letter, handle);
    }

    applier_handle
}

/// Construct the platform-correct [`crate::cache::journal_loop::JournalSource`]
/// for `letter`.
///
/// Extracted from [`spawn_journal_loops_for_warm_shards`] as two
/// cfg-gated definitions so the unused-`letter` parameter on
/// Mac/Linux uses the canonical `_letter` rename rather than a
/// `let _ = letter;` body workaround that clippy flags.
#[cfg(windows)]
fn make_journal_source(letter: char) -> Arc<dyn cache::journal_loop::JournalSource> {
    Arc::new(cache::journal_loop::sources::WindowsJournalSource::new(
        letter,
    ))
}

/// Mac/Linux variant of [`make_journal_source`] — always returns
/// the [`crate::cache::journal_loop::sources::MacStubJournalSource`]
/// stub (no NTFS USN journal on these platforms).
#[cfg(not(windows))]
fn make_journal_source(_letter: char) -> Arc<dyn cache::journal_loop::JournalSource> {
    Arc::new(cache::journal_loop::sources::MacStubJournalSource)
}

/// Spawn the Phase 5 task 5.6 memory-pressure subscriber.
///
/// Subscribes to [`IndexManager::subscribe_pressure`] and reacts to
/// transitions:
///
/// * `Low` — enters cascade mode: calls
///   [`IndexManager::cascade_demote_one_step`] in a loop until either no Warm
///   shards remain (the `None` return) or a `High` / `Normal` transition
///   arrives.  After every step we `tokio::task::yield_now` and check
///   `rx.has_changed()` so a `High` transition can preempt promptly without
///   waiting for the next demote to finish.
/// * `High` / `Normal` — no-op; the loop returns to `rx.changed().await` for
///   the next transition.
///
/// The cascade decision is made via
/// [`PressureLevel::requires_cascade_demote`] rather than direct pattern
/// matching on `PressureLevel::Low`, because the `Low` and `High` variants
/// are platform-conditional — they only exist on Windows and under
/// `cfg(test)` (the targets where the watcher / test fake actually
/// constructs them).  The method ships a `false` branch on Mac/Linux
/// production builds so this loop body compiles cleanly on every host.
///
/// On Mac/Linux the platform [`PressureSignal`] never fires, so this
/// task blocks on `rx.changed().await` forever — TTL-driven demotion
/// via `spawn_idle_demote_controller` is the only demote driver on
/// those targets by design.
///
/// Returns the [`tokio::task::JoinHandle`] so the caller can `.abort()`
/// it during graceful shutdown.  When the [`watch::Sender`] inside
/// `IndexManager::pressure` is dropped the receiver's `changed()`
/// returns `Err`; the loop breaks cleanly without any extra signal.
///
/// `pub(crate)` so the Phase 5 end-to-end integration test in
/// `crate::index::tests::lifecycle_hooks` can drive the full
/// subscribe → cascade → preempt loop against a `ControllablePressureSignal`
/// fake without re-implementing the loop body in test code.  Production
/// callers stay limited to [`run_daemon`] which is the only place this
/// runs in the live daemon.
///
/// [`PressureLevel::requires_cascade_demote`]: crate::cache::pressure::PressureLevel::requires_cascade_demote
/// [`PressureSignal`]: crate::cache::pressure::PressureSignal
/// [`IndexManager::subscribe_pressure`]: crate::index::IndexManager::subscribe_pressure
/// [`IndexManager::cascade_demote_one_step`]: crate::index::IndexManager::cascade_demote_one_step
/// [`watch::Sender`]: tokio::sync::watch::Sender
pub(crate) fn spawn_pressure_subscriber(
    idx: Arc<index::IndexManager>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = idx.subscribe_pressure();
    tokio::spawn(async move {
        loop {
            // Wait for a pressure transition.  `changed()` returns
            // `Err` when the watch sender is dropped, which happens
            // when `IndexManager` (the only owner) is dropped at
            // daemon shutdown.
            if rx.changed().await.is_err() {
                tracing::debug!(
                    target: "cache.pressure",
                    "Pressure-signal sender dropped; subscriber loop exiting",
                );
                break;
            }
            let level = *rx.borrow_and_update();
            tracing::info!(
                target: "cache.pressure",
                ?level,
                "Pressure transition observed",
            );
            if !level.requires_cascade_demote() {
                continue;
            }
            // Cascade-demote until we run out of Warm shards or
            // pressure clears.  `cascade_demote_one_step` returns
            // `None` when no Warm shards remain.
            loop {
                let Some(_demoted) = idx.cascade_demote_one_step().await else {
                    break; // no more Warm shards; cascade exhausted
                };
                // Yield so the runtime can deliver a pending
                // pressure-clearing transition before we loop.
                tokio::task::yield_now().await;
                if rx.has_changed().unwrap_or(false) {
                    let new_level = *rx.borrow_and_update();
                    if !new_level.requires_cascade_demote() {
                        tracing::info!(
                            target: "cache.pressure",
                            ?new_level,
                            "Cascade preempted by transition out of Low",
                        );
                        break;
                    }
                }
            }
        }
    })
}

/// Final shutdown: spawn a 5 s watchdog thread that calls
/// [`std::process::abort`] if `process::exit` itself hangs (kernel
/// I/O can wedge atexit handlers), then force-exit.
///
/// Returns `!` because both arms terminate the process.
fn force_exit_with_watchdog() -> ! {
    tracing::info!("Spawning shutdown watchdog (5s grace period)");
    _ = std::thread::Builder::new()
        .name("shutdown-watchdog".into())
        .spawn(|| {
            std::thread::sleep(core::time::Duration::from_secs(5));
            // process::exit did not complete in 5 s — threads are stuck
            // in kernel I/O.  Force-terminate via abort().
            //
            // Use eprintln! as a last-resort — tracing may not flush
            // before abort().  print_stderr is intentional here: this is
            // a catastrophe path where the structured logging subsystem
            // may be unavailable.
            let msg = "Shutdown watchdog: process::exit stuck for 5s — calling abort()";
            tracing::error!("{msg}");
            #[expect(
                clippy::print_stderr,
                reason = "catastrophe path — tracing may be dead"
            )]
            let _: () = eprintln!("[CATASTROPHE] {msg}");
            std::process::abort();
        }); // best-effort; if thread spawn fails, exit may still work

    // Force-exit the process.  The Windows IPC server uses
    // `std::os::windows::net::UnixListener` with `spawn_blocking(accept)`
    // and per-connection `std::thread::spawn` bridge threads.  These
    // blocking std threads cannot be cancelled by `ipc_task.abort()` and
    // will keep the process alive indefinitely after the daemon logic has
    // finished, turning it into a multi-GB zombie.  `process::exit(0)` is
    // the standard pattern for daemons with uncancellable blocking threads.
    #[expect(
        clippy::exit,
        reason = "daemon has orphaned blocking threads that prevent normal exit"
    )]
    {
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests;
