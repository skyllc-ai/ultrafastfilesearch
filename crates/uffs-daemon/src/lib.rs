// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! UFFS Daemon library — reusable daemon entry point.
//!
//! This crate exposes [`run_daemon`] so the daemon logic can be invoked
//! both from the standalone `uffs-daemon` binary and from the embedded
//! `uffs daemon run` subcommand in the CLI.

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
pub fn default_log_file() -> PathBuf {
    dirs_next::data_local_dir().map_or_else(
        || PathBuf::from("uffsd.log"),
        |dir| dir.join("uffs").join("uffsd.log"),
    )
}

/// Initialise tracing for the daemon process.
///
/// * `log_file = Some(path)` — write to that file (append mode). A path of
///   `"-"` or empty string uses [`default_log_file`].
/// * `log_file = None` **and** the effective log level is `debug` or `trace` —
///   automatically write to [`default_log_file`] so that diagnostic output is
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

    // Create index manager — uses the user-supplied --data-dir for offline MFT
    // discovery and hot-loading (not the lifecycle directory).
    let idx = Arc::new(index::IndexManager::new(config.data_dir.clone(), event_tx));
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

/// Final shutdown: spawn a 5 s watchdog thread that calls
/// [`std::process::abort`] if `process::exit` itself hangs (kernel
/// I/O can wedge atexit handlers), then force-exit.
///
/// Returns `!` because both arms terminate the process.
fn force_exit_with_watchdog() -> ! {
    tracing::info!("Spawning shutdown watchdog (5s grace period)");
    std::thread::Builder::new()
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
        })
        .ok(); // best-effort; if thread spawn fails, exit may still work

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
