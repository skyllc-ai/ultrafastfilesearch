// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! UFFS daemon readiness helpers for the bench orchestrator.
//!
//! The bench tool kicks off the daemon as the very first action in Stage 0 so
//! the index load runs in parallel with env capture and the tool-selection
//! gate. Readiness is only polled once the drive-inventory table is needed,
//! giving the daemon the maximum possible warm-up time before the hard wait.

use crate::error::{BenchError, Result};
use crate::host::Host;

/// Maximum poll attempts waiting for the UFFS daemon to reach `Status: Ready`.
///
/// 90 attempts × 2 s = 3 minutes maximum — matches the user-facing timeout
/// promise. The daemon is kicked off at the very start of `capture()`, so the
/// index load runs in parallel with env capture and the tool-selection gate.
/// Polling only starts once those steps are done and the drive table is needed.
pub(super) const DAEMON_READY_POLL_ATTEMPTS: u32 = 90;

/// Milliseconds between UFFS daemon readiness polls.
pub(super) const DAEMON_READY_POLL_INTERVAL_MS: u64 = 2_000;

/// Check if the UFFS daemon is already running and ready.
///
/// Returns `true` when `uffs daemon status` exits 0 and its output contains
/// `Status:        Ready` (the literal string the daemon emits).
fn daemon_is_ready(host: &dyn Host, uffs_exe: &str) -> bool {
    host.run(uffs_exe, &["daemon", "status"])
        .is_ok_and(|out| out.stdout.contains("Status:        Ready"))
}

/// Start the UFFS daemon if it is not already running or ready.
///
/// Fires `uffs daemon start` and returns immediately — the caller does not
/// wait for the index to finish loading. Any start error is printed as a
/// warning and swallowed; a subsequent [`ensure_daemon_ready`] call will
/// catch the failure with a clear message.
pub(super) fn daemon_start_if_needed(host: &dyn Host, uffs_exe: &str) {
    if daemon_is_ready(host, uffs_exe) {
        return; // already up
    }
    host.out("[preflight] UFFS daemon not ready — starting …");
    if let Err(err) = host.run(uffs_exe, &["daemon", "start"]) {
        host.out(&format!(
            "[preflight] WARNING: could not start UFFS daemon: {err}"
        ));
    }
}

/// Poll `uffs daemon status` until `Status:        Ready` appears, printing a
/// progress line on each attempt so the operator knows the tool is waiting.
///
/// # Errors
/// Returns [`BenchError::Command`] if the daemon does not become ready within
/// [`DAEMON_READY_POLL_ATTEMPTS`] × [`DAEMON_READY_POLL_INTERVAL_MS`] ms
/// (~3 minutes).
pub(super) fn ensure_daemon_ready(host: &dyn Host, uffs_exe: &str) -> Result<()> {
    for attempt in 1..=DAEMON_READY_POLL_ATTEMPTS {
        match host.run(uffs_exe, &["daemon", "status"]) {
            Ok(out) if out.stdout.contains("Status:        Ready") => {
                host.out("[preflight] UFFS daemon is Ready — proceeding");
                return Ok(());
            }
            Ok(out) => {
                let status_line = out
                    .stdout
                    .lines()
                    .find(|line| line.trim_start().starts_with("Status:"))
                    .unwrap_or("Status: unknown")
                    .trim()
                    .to_owned();
                host.out(&format!(
                    "[preflight] Waiting for UFFS daemon … {status_line} \
                     (attempt {attempt}/{DAEMON_READY_POLL_ATTEMPTS})"
                ));
            }
            Err(err) => {
                host.out(&format!(
                    "[preflight] Waiting for UFFS daemon … not responding: {err} \
                     (attempt {attempt}/{DAEMON_READY_POLL_ATTEMPTS})"
                ));
            }
        }
        host.sleep_ms(DAEMON_READY_POLL_INTERVAL_MS);
    }
    Err(BenchError::Command(
        "UFFS daemon did not reach Ready within 3 minutes — \
         check `uffs daemon status` and re-run"
            .to_owned(),
    ))
}
