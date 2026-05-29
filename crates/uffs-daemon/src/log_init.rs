// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tracing-subscriber bootstrap for the daemon.
//!
//! Extracted from `lib.rs` so the daemon's startup graph + `spawn_*`
//! cluster can stay focused on lifecycle wiring without the log-file
//! parent-directory normalisation noise.  No collision risk with the
//! `tracing` crate because the module is named `log_init`.

use std::path::PathBuf;

/// Default log file location: `<native-log-dir>/uffsd.log`.
///
/// The directory is the shared per-platform native log location resolved
/// by [`uffs_security::log_dir::log_dir`] (macOS `~/Library/Logs/uffs`,
/// Windows `%LOCALAPPDATA%\uffs\logs`, Linux `$XDG_STATE_HOME/uffs/logs`),
/// overridable via `UFFS_LOG_DIR`.  When that resolution falls back to a
/// relative `logs` (no home dir), the parent-dir normalisation in
/// [`init_tracing`] still produces a usable `logs/uffsd.log`.
#[must_use]
pub(crate) fn default_log_file() -> PathBuf {
    uffs_security::log_dir::log_dir().join("uffsd.log")
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
