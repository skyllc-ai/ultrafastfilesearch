// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! UFFS daemon control helpers for the bench orchestrator.
//!
//! The daemon is always killed and restarted with only the negotiated capable
//! drives before measurement begins, so index RAM and query routing are
//! confined to the drives under test.

use crate::error::{BenchError, Result};
use crate::host::Host;

/// Maximum poll attempts waiting for the UFFS daemon to reach `Status: Ready`.
///
/// 90 attempts × 2 s = 3 minutes maximum — matches the user-facing timeout
/// promise.
pub(super) const DAEMON_READY_POLL_ATTEMPTS: u32 = 90;

/// Milliseconds between UFFS daemon readiness polls.
pub(super) const DAEMON_READY_POLL_INTERVAL_MS: u64 = 2_000;

/// Kill the running UFFS daemon (hard stop) and start a fresh instance
/// with **no `--drive` restrictions** so the daemon self-discovers every
/// available drive.
///
/// Called at the start of preflight so the first drive-negotiation probe
/// sees the full set of drives the host can index.  Returns immediately
/// after the start command — the caller must call [`ensure_daemon_ready`]
/// before issuing preflight queries.
pub(super) fn kill_and_restart_all_drives(host: &dyn Host, uffs_exe: &str) {
    host.out("[uffs-daemon] killing daemon to restart with all drives …");
    if let Err(err) = host.run(uffs_exe, &["--daemon", "kill"]) {
        host.out(&format!(
            "[uffs-daemon] WARNING: kill returned error (may not have been running): {err}"
        ));
    }
    // Brief pause to allow the OS to release sockets / named-pipe handles
    // before we immediately re-launch.
    host.sleep_ms(1_500);
    host.out(&format!("[uffs-daemon] spawn: {uffs_exe} daemon start"));
    if let Err(err) = host.run(uffs_exe, &["--daemon", "start"]) {
        host.out(&format!(
            "[uffs-daemon] WARNING: could not restart UFFS daemon: {err}"
        ));
    }
}

/// Kill the running UFFS daemon (hard stop) and start a fresh instance
/// restricted to `capable_drives`.
///
/// Fires `uffs --daemon kill` first, waits briefly for the process to exit,
/// then calls `uffs --daemon start --drive X --drive Y …`.  Returns immediately
/// after the start command — the caller must call [`ensure_daemon_ready`] to
/// poll until the index is loaded.
pub(super) fn kill_and_restart_with_drives(
    host: &dyn Host,
    uffs_exe: &str,
    capable_drives: &[char],
) {
    host.out(&format!(
        "[uffs-daemon] killing daemon to restart with drives: {} …",
        capable_drives
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if let Err(err) = host.run(uffs_exe, &["--daemon", "kill"]) {
        host.out(&format!(
            "[uffs-daemon] WARNING: kill returned error (may not have been running): {err}"
        ));
    }
    // Brief pause to allow the OS to release sockets / named-pipe handles
    // before we immediately re-launch.
    host.sleep_ms(1_500);

    let drive_strs: Vec<String> = capable_drives.iter().map(char::to_string).collect();
    let mut args: Vec<&str> = vec!["--daemon", "start"];
    for drive_s in &drive_strs {
        args.push("--drive");
        args.push(drive_s.as_str());
    }
    host.out(&format!(
        "[uffs-daemon] spawn: {uffs_exe} {}",
        args.join(" ")
    ));
    if let Err(err) = host.run(uffs_exe, &args) {
        host.out(&format!(
            "[uffs-daemon] WARNING: could not restart UFFS daemon: {err}"
        ));
    }
}

/// Poll `uffs --daemon status` until `Status:        Ready` appears, printing a
/// progress line on each attempt so the operator knows the tool is waiting.
///
/// # Errors
/// Returns [`BenchError::Command`] if the daemon does not become ready within
/// [`DAEMON_READY_POLL_ATTEMPTS`] × [`DAEMON_READY_POLL_INTERVAL_MS`] ms
/// (~3 minutes).
pub(super) fn ensure_daemon_ready(host: &dyn Host, uffs_exe: &str) -> Result<()> {
    for attempt in 1..=DAEMON_READY_POLL_ATTEMPTS {
        match host.run(uffs_exe, &["--daemon", "status"]) {
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
         check `uffs --daemon status` and re-run"
            .to_owned(),
    ))
}
