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
use uffs_security as _;

/// Broker client — volume handle requests (Windows) / stubs (other).
mod broker_client;
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
    _drives: &[char],
    lifecycle_mgr: &lifecycle::LifecycleManager,
) -> anyhow::Result<()> {
    let has_data = !mft_files.is_empty() || {
        #[cfg(windows)]
        {
            !_drives.is_empty()
        }
        #[cfg(not(windows))]
        {
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
#[expect(
    clippy::too_many_lines,
    reason = "temporary: extra tracing for daemon debugging"
)]
#[expect(
    clippy::cognitive_complexity,
    reason = "daemon startup with socket, config, and index orchestration"
)]
pub async fn run_daemon(config: DaemonConfig) -> anyhow::Result<()> {
    // ── Catastrophe safety net ──────────────────────────────────────
    // Ensure the daemon process is ALWAYS terminable.  If any thread
    // panics (e.g. inside a blocking MFT read), the default panic hook
    // might hang trying to unwind through kernel I/O.  This hook logs
    // the panic and force-exits so the process never becomes a zombie.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        // Force-exit after the default hook has printed the panic info.
        // This prevents the process from hanging if other threads are
        // stuck in kernel-mode I/O.
        #[expect(clippy::exit, reason = "catastrophe safety net — force-exit on panic")]
        {
            std::process::exit(101);
        }
    }));

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

    // Determine data directory:
    // - lifecycle_dir: always %LOCALAPPDATA%\uffs — PID/socket/lock files
    // - data_dir: user-supplied --data-dir (for MFT file discovery/hot-load)
    let lifecycle_dir = dirs_next::data_local_dir()
        .map_or_else(|| PathBuf::from("/tmp/uffs"), |base| base.join("uffs"));

    // Create event broadcast channel — used for push notifications to clients.
    let (event_tx, _event_rx) = events::event_channel();

    // Emit daemon_starting event
    event_tx.emit(events::DaemonEvent::DaemonStarting {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    });

    // Setup lifecycle manager
    let idle_timeout = if config.no_retire {
        None
    } else {
        Some(core::time::Duration::from_secs(config.idle_timeout))
    };
    let mut lifecycle_mgr =
        lifecycle::LifecycleManager::new(&lifecycle_dir, idle_timeout, event_tx.clone());

    tracing::info!(data_dir = %lifecycle_mgr.data_dir().display(), "Lifecycle data directory");

    // Check for stale PID / another running instance
    if !lifecycle_mgr.check_stale_pid() {
        tracing::error!("Another daemon instance is already running");
        anyhow::bail!("Another daemon instance is already running");
    }

    // Write PID file
    lifecycle_mgr.write_pid_file()?;
    tracing::info!("PID file written");

    // D5.0: clean up stale shmem files from previous daemon sessions.
    uffs_client::shmem::cleanup_stale_shmem_files();

    // Create index manager — uses the user-supplied --data-dir for offline MFT
    // discovery and hot-loading (not the lifecycle directory).
    let idx = Arc::new(index::IndexManager::new(config.data_dir.clone(), event_tx));
    tracing::debug!(index_data_dir = ?idx.data_dir(), "Index manager created");

    // Merge --data-dir discovered files into --mft-file list.
    // When --drive is specified, filter to only those drive letters.
    let mut mft_files = config.mft_files;
    if let Some(dir) = &config.data_dir {
        let discovered = uffs_mft::discovery::discover_mft_files(dir);
        let filtered: Vec<PathBuf> = if config.drives.is_empty() {
            discovered
        } else {
            discovered
                .into_iter()
                .filter(|path| {
                    // Extract drive letter from path: .../drive_c/C_mft.iocp → 'C'
                    path.parent()
                        .and_then(|parent| parent.file_name())
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.strip_prefix("drive_"))
                        .and_then(|suffix| suffix.chars().next())
                        .is_some_and(|letter| {
                            config
                                .drives
                                .iter()
                                .any(|drive| drive.eq_ignore_ascii_case(&letter))
                        })
                })
                .collect()
        };
        tracing::info!(
            data_dir = %dir.display(),
            count = filtered.len(),
            filter = ?config.drives,
            "Discovered MFT files from --data-dir"
        );
        mft_files.extend(filtered);
    }
    let no_cache = config.no_cache;

    // Gather drive letters (Windows only; empty on other platforms).
    //
    // When `--drive C,D` is passed: load ONLY those drives — fast start.
    // When no `--drive` is passed: auto-discover ALL NTFS drives — full index.
    #[cfg(windows)]
    let drives: Vec<char> = {
        let explicit = config.drives;
        if explicit.is_empty() {
            // No drives specified → auto-discover all NTFS drives.
            let auto_drives = uffs_mft::detect_ntfs_drives();
            tracing::info!(
                count = auto_drives.len(),
                drives = ?auto_drives,
                "Auto-discovered NTFS drives (no --drive flag)"
            );
            auto_drives
        } else {
            // Respect the explicit drive list — load only what was asked.
            tracing::info!(
                drives = ?explicit,
                "Loading only requested drives (--drive flag)"
            );
            explicit
        }
    };
    #[cfg(not(windows))]
    let drives: Vec<char> = Vec::new();

    tracing::info!(mft_files = mft_files.len(), drives = ?drives, "Final data sources");

    // Refuse to start with zero data sources — an empty daemon is useless.
    validate_data_sources(&mft_files, &drives, &lifecycle_mgr)?;
    tracing::info!("Data sources validated OK");

    let load_index = Arc::clone(&idx);
    // Load heartbeat handle — the load task calls `record_load_progress`
    // after each drive so the idle timer can detect stalls.
    // Used only on Windows (inside cfg(windows) block below).
    let load_lifecycle = lifecycle_mgr.handle();
    let broker_is_available = broker_client::broker_available();
    let load_task = tokio::spawn(async move {
        tracing::info!(mft_files = mft_files.len(), drives = ?drives, "Load task starting");
        if !mft_files.is_empty() {
            tracing::info!("Loading MFT files from data dir...");
            load_index.load_from_data_dir(&mft_files, no_cache).await;
            tracing::info!("MFT files loaded");
        }
        #[cfg(windows)]
        if !drives.is_empty() {
            if broker_is_available {
                for &drive_letter in &drives {
                    match broker_client::request_volume_handle(drive_letter) {
                        Ok(handle) => {
                            tracing::info!(drive = %drive_letter, handle, "Got broker handle");
                        }
                        Err(broker_err) => {
                            tracing::debug!(drive = %drive_letter, error = %broker_err, "Broker unavailable, using direct access");
                        }
                    }
                }
            }
            tracing::info!(drives = ?drives, "Loading live drives...");
            load_index
                .load_live_drives(&drives, no_cache, &load_lifecycle)
                .await;
            tracing::info!("Live drives loaded");
        }
        if broker_is_available {
            let _handle_result = broker_client::request_volume_handle('C');
        }
        tracing::info!("Load task completed");

        // Zero-drive guard: `validate_data_sources` (above) rejects
        // empty argv, but it cannot predict parse failures.  When
        // every MFT file / live drive fails to load, the Loading →
        // Ready transition fires anyway with an empty index — the
        // "zombie daemon" state where status reports `Ready` but
        // every search returns zero rows.  Catch that here by
        // checking the actual post-load drive count.  If zero,
        // request shutdown so the lifecycle's `select!` takes the
        // process down cleanly; leaving such a daemon alive only
        // wastes the socket slot and the PID file for its next
        // idle timeout.
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
    });

    // Start IPC server
    let ipc_index = Arc::clone(&idx);
    let ipc_lifecycle = lifecycle_mgr.handle();

    tracing::info!("Starting IPC server...");
    let ipc_task = tokio::spawn(async move {
        if let Err(ipc_err) = ipc::run_ipc_server(ipc_index, ipc_lifecycle).await {
            tracing::error!(error = %ipc_err, "IPC server error");
        }
    });
    tracing::info!("IPC server task spawned");

    // On Windows, also listen on a named pipe.  This is the PREFERRED
    // transport (no ws2_32 dependency, ~54 ms faster per CLI launch).
    // The AF_UNIX listener above remains active as a fallback for older
    // clients during the transition — it will be removed in a follow-up
    // once all clients speak pipe.
    #[cfg(windows)]
    let _pipe_task = {
        let pipe_index = Arc::clone(&idx);
        let pipe_lifecycle = lifecycle_mgr.handle();
        tracing::info!("Starting named-pipe IPC server...");
        tokio::spawn(async move {
            if let Err(pipe_err) = ipc::run_pipe_server(pipe_index, pipe_lifecycle).await {
                tracing::error!(error = %pipe_err, "Named-pipe IPC server error");
            }
        })
    };

    // Spawn periodic stats heartbeat — pushes stats to all connected
    // clients every 30 seconds.
    let stats_index = Arc::clone(&idx);
    let stats_lifecycle = lifecycle_mgr.handle();
    let _stats_task = tokio::spawn(async move {
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
    });

    // Run idle timer (blocks until shutdown or timeout)
    lifecycle_mgr.run_idle_timer().await;

    // Graceful shutdown
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

    // ── Force-terminate safety net ───────────────────────────────
    //
    // Spawn a watchdog thread BEFORE calling `process::exit`.
    // If threads are stuck in kernel-mode I/O (raw NTFS volume reads),
    // `process::exit()` may itself hang because the C runtime's atexit
    // handlers try to join threads that are blocked in non-interruptible
    // system calls.
    //
    // The watchdog sleeps for a grace period, then calls
    // `process::abort()` which raises SIGABRT and forces the OS to
    // tear down the process — including all kernel-mode waiters.
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
