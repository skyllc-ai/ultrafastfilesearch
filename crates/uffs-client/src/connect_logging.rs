// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tracing helpers extracted from `connect.rs`.
//!
//! These three helpers exist purely to keep the cognitive-complexity
//! lint happy on [`crate::connect::UffsClient`]'s retry loop.  They
//! live in a sibling module because `connect.rs` is an
//! `async`-feature-gated leaf file; keeping the helpers next to
//! `connect` would have blown through the `check_file_size_policy.sh`
//! 800-LOC ceiling once the v0.5.36 UAC work added the elevation
//! entry points.

use std::ffi::OsString;
use std::path::Path;

/// Log daemon spawn details (exe path, existence, command args).
pub(crate) fn log_spawn_details(uffs_exe: &Path, cmd_args: &[OsString]) {
    tracing::debug!(
        uffs_exe = %uffs_exe.display(),
        uffs_exe_exists = uffs_exe.exists(),
        ?cmd_args,
        "auto_start_daemon: resolved exe, spawning"
    );
}

/// Log a connect retry attempt with socket/PID file status.
pub(crate) fn log_connect_attempt(
    attempt: usize,
    max_attempts: usize,
    delay_ms: u64,
    sock: &Path,
    pid_path: &Path,
) {
    tracing::debug!(
        attempt,
        max_attempts,
        delay_ms,
        sock_exists = sock.exists(),
        pid_exists = pid_path.exists(),
        "connect attempt"
    );
}

/// Log a failed connect attempt (only for first 3 and final attempts
/// to avoid spam).
pub(crate) fn log_connect_error(
    attempt: usize,
    max_attempts: usize,
    err: &crate::error::ClientError,
) {
    if attempt <= 3 || attempt == max_attempts {
        tracing::debug!(attempt, %err, "connect attempt failed");
    }
}
