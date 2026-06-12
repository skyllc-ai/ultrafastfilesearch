// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Capture and restore the host's UFFS run-state ("R1": daemon + MCP).
//!
//! The bench kills and restarts the UFFS daemon, scoping it to the negotiated
//! capable drives so index RAM and query routing are confined to the drives
//! under test. To leave the host exactly as found, we snapshot the *original*
//! run-state — which drives the daemon had loaded and whether the MCP HTTP
//! gateway was up — **before the first kill**, and register a restore that
//! replays it at teardown.
//!
//! The snapshot is parsed from `uffs status` (see [`parse_status`]); the
//! restore shells back through the [`Host`] seam using the **resolved** UFFS
//! binary (never a bare `uffs` off `PATH`).
//!
//! Everything (ES) is deliberately **not** handled here: the bench always runs
//! it as a private `-instance` sandbox, never touching the operator's tool.

use crate::error::{BenchError, Result};
use crate::host::Host;
use crate::restore::RunGuard;

/// Snapshot of the host's UFFS run-state at bench start.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunState {
    /// Drive letters the daemon had loaded, or `None` if the daemon was not
    /// running. `Some(vec![])` means running but with no drives loaded.
    pub daemon_drives: Option<Vec<char>>,
    /// Whether the MCP HTTP gateway was running.
    pub mcp_running: bool,
}

impl RunState {
    /// One-line human description for the run log.
    #[must_use]
    pub fn describe(&self) -> String {
        let daemon = match &self.daemon_drives {
            None => "daemon stopped".to_owned(),
            Some(drives) if drives.is_empty() => "daemon running (no drives)".to_owned(),
            Some(drives) => format!(
                "daemon on {}",
                drives
                    .iter()
                    .map(char::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        };
        let mcp = if self.mcp_running {
            "mcp up"
        } else {
            "mcp down"
        };
        format!("{daemon}; {mcp}")
    }
}

/// Whether a `Status:` line's value reads as "running".
///
/// `"running (PID …)"` → `true`; `"not running"` / `"connected but not
/// responding"` → `false`. `"not running"` is checked first because it
/// contains the substring `"running"`.
fn status_is_running(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    !lower.contains("not running") && !lower.contains("not responding") && lower.contains("running")
}

/// Extract a drive letter from a `uffs status` drive line such as
/// `"[W] G:     15,162 records"`. Returns `None` for any other line.
fn drive_letter_from_line(line: &str) -> Option<char> {
    let after_bracket = line.strip_prefix('[')?.split_once(']')?.1.trim_start();
    let mut chars = after_bracket.chars();
    let letter = chars.next()?;
    (letter.is_ascii_alphabetic() && chars.next() == Some(':')).then(|| letter.to_ascii_uppercase())
}

/// Section of `uffs status` output currently being parsed.
#[derive(PartialEq, Eq)]
enum Section {
    /// Before the first `──` header.
    None,
    /// `── Daemon ──`.
    Daemon,
    /// `── MCP HTTP Gateway ──`.
    McpHttp,
    /// Any other section (MCP stdio, …) — ignored.
    Other,
}

/// Parse `uffs status` stdout into a [`RunState`].
///
/// Scopes the `Status:` line and the `[T] L:` drive lines to the `── Daemon ──`
/// section, and the MCP gateway `Status:` to `── MCP HTTP Gateway ──`.
#[must_use]
pub fn parse_status(stdout: &str) -> RunState {
    let mut section = Section::None;
    let mut daemon_running = false;
    let mut daemon_seen = false;
    let mut drives: Vec<char> = Vec::new();
    let mut mcp_running = false;

    for raw in stdout.lines() {
        let line = raw.trim();
        if line.contains("── Daemon") {
            section = Section::Daemon;
            daemon_seen = true;
            continue;
        }
        if line.contains("MCP HTTP Gateway") {
            section = Section::McpHttp;
            continue;
        }
        if line.contains("MCP Stdio") {
            section = Section::Other;
            continue;
        }
        match section {
            Section::Daemon => {
                if let Some(rest) = line.strip_prefix("Status:") {
                    daemon_running = status_is_running(rest);
                } else if let Some(letter) = drive_letter_from_line(line) {
                    drives.push(letter);
                }
            }
            Section::McpHttp => {
                if let Some(rest) = line.strip_prefix("Status:") {
                    mcp_running = status_is_running(rest);
                }
            }
            Section::None | Section::Other => {}
        }
    }

    RunState {
        daemon_drives: (daemon_seen && daemon_running).then_some(drives),
        mcp_running,
    }
}

/// Capture the current run-state by running `<uffs_exe> status`.
///
/// Best-effort: a missing/erroring binary reads as "nothing running" so the
/// restore is a no-op rather than fabricating state.
#[must_use]
pub fn capture(host: &dyn Host, uffs_exe: &str) -> RunState {
    match host.run(uffs_exe, &["status"]) {
        Ok(out) => parse_status(&out.stdout),
        Err(_) => RunState::default(),
    }
}

/// Register the teardown restore that returns the daemon + MCP to `state`.
///
/// Registered on the [`RunGuard`] **before** the bench mutates the daemon, so
/// it drains last (LIFO) at teardown — after every stage's own undo.
pub fn register_restore(guard: &mut RunGuard<'_>, uffs_exe: &str, state: RunState) {
    let exe = uffs_exe.to_owned();
    guard.register("daemon + mcp run-state", move |host| {
        restore(host, &exe, &state)
    });
}

/// Replay `state`: restart the daemon with its original drive set (or leave it
/// stopped), and bring the MCP gateway back up if it was up.
///
/// # Errors
/// Returns [`BenchError::Command`] if restarting the daemon with the captured
/// drives fails. Daemon-kill and MCP commands are best-effort (ignored).
fn restore(host: &dyn Host, uffs_exe: &str, state: &RunState) -> Result<()> {
    // Tear down whatever the bench left, then rebuild the as-found state.
    if let Err(err) = host.run(uffs_exe, &["daemon", "kill"]) {
        host.out(&format!(
            "[run-state] daemon kill before restore failed (ok if already stopped): {err}"
        ));
    }
    host.sleep_ms(1_500);

    match &state.daemon_drives {
        None => {
            // Daemon was not running before the bench — leave it stopped.
            host.out("[run-state] daemon was stopped at start — leaving it stopped");
        }
        Some(drives) if drives.is_empty() => {
            host.out("[run-state] restarting daemon (full discovery — none recorded)");
            host.run(uffs_exe, &["daemon", "start"])
                .map(|_out| ())
                .map_err(|err| BenchError::Command(format!("restore daemon: {err}")))?;
        }
        Some(drives) => {
            let drive_strs: Vec<String> = drives.iter().map(char::to_string).collect();
            let mut args: Vec<&str> = vec!["daemon", "start"];
            for drive_s in &drive_strs {
                args.push("--drive");
                args.push(drive_s.as_str());
            }
            host.out(&format!(
                "[run-state] restarting daemon on as-found drives: {}",
                drive_strs.join(",")
            ));
            host.run(uffs_exe, &args)
                .map(|_out| ())
                .map_err(|err| BenchError::Command(format!("restore daemon drives: {err}")))?;
        }
    }

    // MCP: only ever bring it *back up* if it was up. The bench never starts
    // MCP, so a "was down" state needs no action — and we must not kill an MCP
    // the operator may have launched mid-run. (The MCP-killing scripts do their
    // own restore.) `mcp start` is idempotent enough to ignore when already up.
    if state.mcp_running {
        host.out("[run-state] restarting MCP gateway (was up at start)");
        if let Err(err) = host.run(uffs_exe, &["mcp", "start"]) {
            host.out(&format!("[run-state] mcp start failed: {err}"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `uffs status` output (7 drives loaded, MCP up) — the operator's
    /// reported state. Drive letters must round-trip in listed order.
    const STATUS_7_DRIVES_MCP_UP: &str = "\
═══ UFFS System Status ═══

── Daemon ──
  Version:     0.5.120
  Status:      running (PID 62036)
  Uptime:       44 s    0 ms
  State:       Ready
  Connections: 1
  Drives:      7 loaded (25,925,871 records, 7 active / 0 parked / 0 cold)
    [W] G:     15,162 records
    [W] M:  1,908,812 records
    [W] F:  2,221,343 records
    [W] C:  3,506,664 records
    [W] E:  2,929,746 records
    [W] D:  7,066,038 records
    [W] S:  8,278,106 records
  Startup:       5 s  343 ms
  Queries:     0

── MCP HTTP Gateway ──
  Status:      running (PID 60016)
  Uptime:        2 s    0 ms
  Endpoint:    http://127.0.0.1:8080/mcp

── MCP Stdio Sessions ──
  (none)
";

    #[test]
    fn parses_seven_drives_and_mcp_up() {
        let state = parse_status(STATUS_7_DRIVES_MCP_UP);
        assert_eq!(
            state.daemon_drives,
            Some(vec!['G', 'M', 'F', 'C', 'E', 'D', 'S'])
        );
        assert!(state.mcp_running);
    }

    #[test]
    fn daemon_stopped_reads_as_none() {
        let out = "\
── Daemon ──
  Status:      not running

── MCP HTTP Gateway ──
  Status:      not running
";
        let state = parse_status(out);
        assert_eq!(state.daemon_drives, None);
        assert!(!state.mcp_running);
    }

    #[test]
    fn mcp_status_scoped_to_its_section() {
        // The daemon is running; MCP is down. A naive `contains("running")`
        // over the whole blob would wrongly flag MCP as up.
        let out = "\
── Daemon ──
  Status:      running (PID 1)
  Drives:      (none loaded)

── MCP HTTP Gateway ──
  Status:      not running
";
        let state = parse_status(out);
        assert_eq!(state.daemon_drives, Some(vec![]));
        assert!(!state.mcp_running);
    }

    #[test]
    fn drive_line_parser_rejects_non_drive_lines() {
        assert_eq!(
            drive_letter_from_line("[W] G:     15,162 records"),
            Some('G')
        );
        assert_eq!(drive_letter_from_line("[H] c: 1 records"), Some('C'));
        assert_eq!(drive_letter_from_line("Drives:      7 loaded"), None);
        assert_eq!(drive_letter_from_line("Status:      running"), None);
        assert_eq!(drive_letter_from_line("[W] (none)"), None);
    }

    #[test]
    fn status_running_handles_not_running_substring() {
        assert!(status_is_running("      running (PID 62036)"));
        assert!(!status_is_running("      not running"));
        assert!(!status_is_running("      connected but not responding"));
    }

    #[test]
    fn describe_is_readable() {
        let state = RunState {
            daemon_drives: Some(vec!['C', 'D']),
            mcp_running: true,
        };
        assert_eq!(state.describe(), "daemon on C,D; mcp up");
        assert_eq!(RunState::default().describe(), "daemon stopped; mcp down");
    }
}
