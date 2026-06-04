// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Auto-start daemon helpers for [`crate::connect_sync::UffsClientSync`].
//!
//! Extracted from `connect_sync.rs` to keep that file under the workspace
//! 800-LOC policy ceiling.  The three helpers below are tightly coupled to
//! one another (PID-file freshness → process liveness → daemon-identity
//! verification) and only invoked by the sync client's reconnect loop.

use crate::daemon_ctl::{find_daemon_exe, pid_file_path, socket_path};
use crate::daemon_spawn::{ElevationPolicy, spawn_daemon};
use crate::error::ClientError;

/// Spawn the daemon binary if not already running.
///
/// Returns:
/// * `Ok(Some(handle))` when this call spawned a fresh daemon — the handle lets
///   the caller's retry loop poll for unexpected early exit.
/// * `Ok(None)` when an existing daemon was already alive and no spawn
///   happened, so there is nothing to poll.
///
/// # Errors
///
/// Propagates spawn failures from [`spawn_daemon`].
pub(crate) fn auto_start_daemon(
    spawn_args: &[String],
    policy: ElevationPolicy,
) -> Result<Option<crate::daemon_child::DaemonChildHandle>, ClientError> {
    let pid_path = pid_file_path();

    // Check if daemon is already alive via PID file.
    if pid_path.exists()
        && crate::daemon_ctl::parse_pid_file(&pid_path)
            .is_some_and(|(pid, _ts, _hash, _nonce)| is_process_alive(pid))
    {
        return Ok(None);
    }
    if pid_path.exists() {
        // Stale PID file — clean up.
        drop(std::fs::remove_file(&pid_path));
        let sock = socket_path();
        drop(std::fs::remove_file(&sock));
    }

    let daemon_exe = find_daemon_exe();
    let str_args: Vec<&str> = spawn_args.iter().map(String::as_str).collect();
    let handle = spawn_daemon(&daemon_exe, &str_args, policy)?;
    Ok(Some(handle))
}

/// Check if a process is alive **and** is actually a `uffsd` daemon.
///
/// A bare `kill(pid, 0)` / `tasklist` check is insufficient because the OS
/// can recycle PIDs.  A stale PID file whose PID was reused by an unrelated
/// process would make us think the daemon is running, so we'd skip the spawn
/// and then fail to connect (the socket never appears).
///
/// On **macOS** we use `ps -p <pid> -o comm=` to verify the process name.
/// On **Linux** we read `/proc/<pid>/comm`.
/// On **Windows** we check `tasklist` output for `uffsd`.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Cheap signal-zero check.  Unix PIDs fit in i32 by spec, so the
        // saturating `try_from` fallback is unreachable in practice.
        let pid_i32 = i32::try_from(pid).unwrap_or(i32::MAX);
        // SAFETY: kill(pid, 0) only checks if a signal *could* be sent
        // to the given PID — it does not actually deliver any signal.
        // The pid comes from our own PID file (trusted input).
        #[expect(unsafe_code, reason = "kill(pid, 0) is a safe existence check")]
        let alive = unsafe { libc::kill(pid_i32, 0) } == 0;
        if !alive {
            return false;
        }

        // Process exists — verify it is actually uffsd.
        is_daemon_process(pid)
    }
    #[cfg(not(unix))]
    {
        // Windows: `tasklist` filtered to our PID.
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|output| {
                // AUDIT-OK(bytes): daemon-identity probe via substring match; a lossy
                // decode can only FAIL the match → treat as 'not the daemon' (fail-safe
                // reconnect), never a false positive. (WI-4.3 follow-up)
                let text = String::from_utf8_lossy(&output.stdout);
                // tasklist prints  "uffsd.exe  <PID> ..." when the process matches.
                // Verify both the PID and the executable name.
                text.contains(&pid.to_string()) && text.contains("uffsd")
            })
    }
}

/// Verify that the process with `pid` is actually a `uffsd` daemon.
#[cfg(unix)]
fn is_daemon_process(pid: u32) -> bool {
    // Linux: read /proc/<pid>/comm (fastest, no fork).
    #[cfg(target_os = "linux")]
    {
        let comm_path = format!("/proc/{pid}/comm");
        if let Ok(comm) = std::fs::read_to_string(&comm_path) {
            return comm.trim() == "uffsd";
        }
    }

    // macOS / fallback: use `ps -p <pid> -o comm=`.
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| {
            // AUDIT-OK(bytes): daemon-identity probe via substring match; a lossy
            // decode can only FAIL the match → treat as 'not the daemon' (fail-safe
            // reconnect), never a false positive. (WI-4.3 follow-up)
            let comm = String::from_utf8_lossy(&output.stdout);
            // `ps -o comm=` prints the executable path or basename.
            // Match if any path component is "uffsd".
            comm.trim()
                .rsplit('/')
                .next()
                .is_some_and(|name| name == "uffsd")
        })
}
