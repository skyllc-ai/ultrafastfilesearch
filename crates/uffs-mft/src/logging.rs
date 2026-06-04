// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Logging initialization for the `uffs-mft` binary.

use std::io;

use tracing_appender::non_blocking::NonBlocking;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::{EnvFilter, Layer as _};

/// Initialize logging with terminal + file support.
///
/// If `verbose` is true and `RUST_LOG` is not set, uses `debug` level for
/// terminal. Otherwise, terminal logging is controlled by `RUST_LOG` (default:
/// `info`). File logging is controlled by `RUST_LOG_FILE` (default: `info`).
/// Log directory is the shared per-platform native location resolved by
/// [`uffs_security::log_dir::log_dir`] (overridable via `UFFS_LOG_DIR`).
#[expect(
    clippy::single_call_fn,
    reason = "logical separation of logging initialization"
)]
#[expect(
    clippy::print_stderr,
    reason = "WI-6.2: log-dir create failure must surface before tracing is up; stderr is the only honest channel"
)]
pub(crate) fn init_logging(verbose: bool) -> tracing_appender::non_blocking::WorkerGuard {
    use std::fs;

    // Shared native log dir (macOS ~/Library/Logs/uffs, Windows
    // %LOCALAPPDATA%\uffs\logs, Linux $XDG_STATE_HOME/uffs/logs),
    // honoring the UFFS_LOG_DIR override.
    let log_dir = uffs_security::log_dir::log_dir();

    // Create log directory if it doesn't exist. Logging isn't up yet, so a
    // failure can only be surfaced on stderr — say so once and degrade to
    // terminal-only logging rather than vanishing silently (WI-6.2).
    if let Err(err) = fs::create_dir_all(&log_dir) {
        eprintln!(
            "uffs-mft: could not create log dir {} ({err}); file logging may be disabled",
            log_dir.display()
        );
    }

    // Create rolling file appender (daily rotation).
    // Use the builder API which returns Result instead of panicking, and retry
    // briefly to handle transient Windows file-lock races (e.g. previous process
    // still releasing the log file handle).
    let max_attempts = 4_u32;
    let mut file_log_err: Option<String> = None;
    let mut file_log_attempt = 0_u32;
    let (non_blocking, guard): (NonBlocking, _) = {
        let mut last_err = None;
        let mut appender = None;
        for attempt in 0..max_attempts {
            if attempt > 0 {
                std::thread::sleep(core::time::Duration::from_millis(250));
            }
            match RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix("uffs_mft_log_")
                .build(&log_dir)
            {
                Ok(file_appender) => {
                    file_log_attempt = attempt;
                    appender = Some(file_appender);
                    break;
                }
                Err(init_err) => last_err = Some(init_err),
            }
        }
        appender.map_or_else(
            || {
                file_log_err = Some(
                    last_err
                        .as_ref()
                        .map_or_else(|| "unknown error".to_owned(), ToString::to_string),
                );
                NonBlocking::new(io::sink())
            },
            NonBlocking::new,
        )
    };

    // Terminal filter: -v sets debug if RUST_LOG not explicitly set
    let terminal_default = if verbose { "debug" } else { "info" };
    let terminal_filter =
        EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| terminal_default.to_owned()));

    // File filter (default: info)
    let file_filter =
        EnvFilter::new(std::env::var("RUST_LOG_FILE").unwrap_or_else(|_| "info".to_owned()));

    // Timer format
    let timer = UtcTime::rfc_3339();

    // Terminal layer (to stderr to avoid corrupting output when redirecting stdout,
    // with ANSI colors, file/line info, thread IDs)
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_timer(timer.clone())
        .with_ansi(true)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .with_filter(terminal_filter);

    // File layer (no ANSI colors, but with full context)
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_timer(timer)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .with_filter(file_filter);

    // Combine layers
    let subscriber = Registry::default().with(terminal_layer).with(file_layer);

    #[expect(
        clippy::expect_used,
        reason = "global subscriber must be set or the program cannot continue"
    )]
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global tracing subscriber");

    // Post-init diagnostics: surface file-appender issues through tracing now
    // that the subscriber is active.
    if let Some(err_msg) = &file_log_err {
        tracing::error!(
            log_dir = %log_dir.display(),
            attempts = max_attempts,
            error = %err_msg,
            "File logging DISABLED — log file could not be opened after all retries. \
             All tracing output is terminal-only for this session."
        );
    } else if file_log_attempt > 0 {
        tracing::warn!(
            log_dir = %log_dir.display(),
            retries = file_log_attempt,
            "Log file opened after {file_log_attempt} retries — \
             previous process may have been slow to release the file handle"
        );
    }

    guard
}
