// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shared per-platform log-directory resolution for all UFFS binaries.
//!
//! UFFS is a shipped end-user product, so logs go where each OS expects
//! them rather than into a single `~/.uffs` dotfile, the cache dir, or
//! `~/bin` (a binaries dir — writing `logs/` under the installed `uffs`
//! file there fails with `Not a directory`).
//!
//! # Resolution order
//!
//! 1. `UFFS_LOG_DIR` env var, if set to a **non-empty** value — used verbatim.
//! 2. The per-platform native location
//!    ([`native_log_dir`](crate::log_dir::native_log_dir)):
//!    - macOS:   `~/Library/Logs/uffs`            (read by Console.app)
//!    - Windows: `%LOCALAPPDATA%\uffs\logs`        (== `dirs data_local_dir`)
//!    - Linux:   `$XDG_STATE_HOME/uffs/logs`, else `~/.local/state/uffs/logs`
//! 3. `./logs` — final fallback, only when the home dir cannot be determined.
//!
//! All UFFS binaries share **one** base directory; each keeps its own
//! distinct filename within it (`uffsd.log`, `uffs_mcp.log`,
//! `mcp-gateway.log`, `uffs_mft_log_*`), so Console.app / journald show
//! every UFFS log in one place.
//!
//! # Environment
//!
//! | Env var | Type | Default | Notes |
//! |---|---|---|---|
//! | `UFFS_LOG_DIR` | path | (native per-OS dir) | Overrides the log directory for every UFFS binary.  STANDARD semver class. |

use std::path::PathBuf;

/// Env var that overrides the log directory for every UFFS binary.
pub const LOG_DIR_ENV: &str = "UFFS_LOG_DIR";

/// Resolve the UFFS log directory, honoring the `UFFS_LOG_DIR` override.
///
/// See the [module docs](self) for the full resolution order.  The
/// returned path is **not** created — callers are responsible for
/// `create_dir_all` (they already do, and need to handle the error in
/// their own logging-init style).
#[must_use]
pub fn log_dir() -> PathBuf {
    match std::env::var_os(LOG_DIR_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => native_log_dir(),
    }
}

/// The per-platform native log directory, ignoring any env override.
///
/// Exposed for callers that want the OS-native location regardless of
/// `UFFS_LOG_DIR` (e.g. diagnostics that report "where logs *would*
/// land natively").  Most code should call [`log_dir`] instead.
#[must_use]
pub fn native_log_dir() -> PathBuf {
    // Bind the per-platform value rather than early-returning from each
    // cfg arm, so clippy's `needless_return` does not fire on the
    // single-expression branches.
    #[cfg(target_os = "macos")]
    let dir = macos_log_dir();

    #[cfg(target_os = "windows")]
    let dir = windows_log_dir();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let dir = linux_log_dir();

    dir
}

/// macOS: `~/Library/Logs/uffs` (the location Console.app reads).
///
/// Falls back to `./logs` if the home directory cannot be determined.
#[cfg(target_os = "macos")]
fn macos_log_dir() -> PathBuf {
    dirs_next::home_dir().map_or_else(
        || PathBuf::from("logs"),
        |home| home.join("Library").join("Logs").join("uffs"),
    )
}

/// Windows: `%LOCALAPPDATA%\uffs\logs` (== `dirs_next::data_local_dir`).
///
/// Falls back to `./logs` if the local-app-data directory cannot be
/// determined.
#[cfg(target_os = "windows")]
fn windows_log_dir() -> PathBuf {
    dirs_next::data_local_dir().map_or_else(
        || PathBuf::from("logs"),
        |dir| dir.join("uffs").join("logs"),
    )
}

/// Linux / other Unix: `$XDG_STATE_HOME/uffs/logs`, falling back to
/// `~/.local/state/uffs/logs`.
///
/// `dirs_next` has no `state_dir()` helper, so the XDG state home is
/// resolved by hand per the XDG Base Directory spec: honor
/// `XDG_STATE_HOME` only when it is an **absolute** path (the spec
/// requires relative values to be ignored), else `~/.local/state`.
/// Final fallback is `./logs` if the home directory is also unknown.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn linux_log_dir() -> PathBuf {
    let state_home = xdg_state_home(std::env::var_os("XDG_STATE_HOME"), dirs_next::home_dir());
    state_home.join("uffs").join("logs")
}

/// Resolve the XDG state home from a raw `XDG_STATE_HOME` value and the
/// detected home dir, applying the XDG spec's absolute-path rule.
///
/// Honors `xdg_state_home_raw` only when it is an **absolute** path (the
/// spec requires relative values to be ignored), else `~/.local/state`.
/// Returns `.` (current dir) only when neither input yields a path — the
/// caller then lands on `./uffs/logs`, close enough to the documented
/// `./logs` last-resort fallback for the genuinely-headless case.
///
/// Split out as a pure function (no direct `std::env` read) so it is
/// unit-testable without mutating the process-global environment, which
/// is `unsafe` under the Rust 2024 edition.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn xdg_state_home(
    xdg_state_home_raw: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> PathBuf {
    if let Some(value) = xdg_state_home_raw {
        let candidate = PathBuf::from(&value);
        if candidate.is_absolute() {
            return candidate;
        }
    }
    home.map_or_else(
        || PathBuf::from("."),
        |home_dir| home_dir.join(".local").join("state"),
    )
}

#[cfg(test)]
mod tests;
