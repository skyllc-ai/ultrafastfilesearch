// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Restore (design §10): relaunch services from the captured snapshot
//! recipe, **providers before consumers** (broker → daemon → MCP).
//!
//! This is also the INV-1 backstop: the apply rollback path and Phase H
//! call it to make sure nothing the run stopped is left down. So it is
//! **best-effort per component** — a failure to restart one is reported
//! but never blocks restarting the others.

use std::process::Command;

use crate::plan::{SnapRunning, Snapshot};
use crate::quiesce::{BROKER_SERVICE, daemon_pid_file, wait_until};

/// How long to wait for a service to come back up.
const START_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(20);

/// How long to wait for the broker's named pipe to begin serving (ms).
const PIPE_READY_TIMEOUT_MS: u32 = 10_000;

/// Restart every component that was running, provider→consumer. Returns
/// the components that failed to restart (empty = all good).
pub(crate) fn restore(snapshot: &Snapshot) -> Vec<String> {
    let mut failed = Vec::new();
    for component in ["broker", "daemon", "mcp"] {
        for running in snapshot
            .running
            .iter()
            .filter(|run| run.component == component)
        {
            if !start_component(component, running) {
                failed.push(component.to_owned());
            }
        }
    }
    failed
}

/// Start one component; `true` on (best-effort) success.
fn start_component(component: &str, running: &SnapRunning) -> bool {
    match component {
        "broker" => start_broker(),
        "daemon" => start_from_command_line(running, || daemon_pid_file().exists()),
        "mcp" => start_from_command_line(running, || true),
        _ => false,
    }
}

/// `sc start` the broker, wait until `sc query` reports RUNNING, **then**
/// wait until the pipe is actually serving (R10, §19.13). Service-RUNNING
/// is necessary but not sufficient: the daemon's warm-up hits
/// `ERROR_PIPE_BUSY` if it connects before the broker's pipe is listening.
fn start_broker() -> bool {
    let _ignore = Command::new("sc.exe")
        .args(["start", BROKER_SERVICE])
        .status();
    wait_until(START_TIMEOUT, || sc_query_running(BROKER_SERVICE))
        && broker_pipe_ready(PIPE_READY_TIMEOUT_MS)
}

/// Wait up to `timeout_ms` for the broker's named pipe to serve via a
/// **non-connecting** `WaitNamedPipe` probe (R10, §19.13). Unlike a
/// connecting open (or `GetFileAttributesW`), it never consumes the single
/// pipe instance — so it cannot itself cause the `ERROR_PIPE_BUSY` it
/// guards against. `true` when the pipe is available within the timeout.
#[cfg(windows)]
pub(crate) fn broker_pipe_ready(timeout_ms: u32) -> bool {
    use uffs_broker_protocol::PIPE_NAME;
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = PIPE_NAME
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 buffer that outlives
    // the call; the timeout is a plain millisecond count. `WaitNamedPipe`
    // only waits for availability — it opens nothing.
    #[expect(unsafe_code, reason = "Win32 FFI — WaitNamedPipeW")]
    let ready = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), timeout_ms) };
    ready.as_bool()
}

/// Non-Windows: there is no broker pipe, so readiness is vacuously true.
#[cfg(not(windows))]
pub(crate) const fn broker_pipe_ready(_timeout_ms: u32) -> bool {
    true
}

/// Relaunch a process from its captured `command_line`, detached, then
/// wait for `ready` (e.g. the daemon's PID file to reappear).
fn start_from_command_line(running: &SnapRunning, ready: impl Fn() -> bool) -> bool {
    let Some(cmd) = running.command_line.as_deref() else {
        return false;
    };
    let Some((program, args)) = parse_command_line(cmd) else {
        return false;
    };
    // Spawn detached: don't wait, so the relaunched service outlives us.
    let spawned = Command::new(&program).args(&args).spawn().is_ok();
    spawned && wait_until(START_TIMEOUT, &ready)
}

/// Split a captured command line into `(program, args)`.
///
/// Naive whitespace split — sufficient for UFFS's switch-style argv
/// (`uffsd --no-retire --drive C,D`). Pure → testable.
pub(crate) fn parse_command_line(cmd: &str) -> Option<(String, Vec<String>)> {
    let mut parts = cmd.split_whitespace().map(ToOwned::to_owned);
    let program = parts.next()?;
    Some((program, parts.collect()))
}

/// `true` when `sc query <service>` reports RUNNING.
fn sc_query_running(service: &str) -> bool {
    let Ok(out) = Command::new("sc.exe").args(["query", service]).output() else {
        return false;
    };
    out.status.success() && sc_state_is_running(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `sc query` stdout for a RUNNING state (pure — testable).
fn sc_state_is_running(sc_output: &str) -> bool {
    sc_output.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("STATE") && trimmed.contains("RUNNING")
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_command_line, sc_state_is_running};

    #[test]
    fn parses_switch_style_command_line() {
        let (program, args) = parse_command_line("uffsd --no-retire --drive C,D").expect("parse");
        assert_eq!(program, "uffsd");
        assert_eq!(args, vec!["--no-retire", "--drive", "C,D"]);
    }

    #[test]
    fn parses_program_only() {
        let (program, args) = parse_command_line("uffsmcp").expect("parse");
        assert_eq!(program, "uffsmcp");
        assert!(args.is_empty());
    }

    #[test]
    fn empty_command_line_is_none() {
        assert!(parse_command_line("   ").is_none());
    }

    #[test]
    fn sc_running_parsing() {
        assert!(sc_state_is_running("    STATE  : 4  RUNNING"));
        assert!(!sc_state_is_running("    STATE  : 1  STOPPED"));
    }
}
