#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1"
//! colored = "2"
//! ```
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//!
//! orphan-cleanup.rs — find and kill stale `uffs` / `uffs.exe` processes
//! that were left behind by an aborted CLI invocation, a crashed test
//! harness, or a Ctrl-C'd validation script, while leaving the
//! legitimate long-running daemon (and its parent shell) untouched.
//!
//! The validation suites (`scripts/windows/{api,cli,mcp}-validation.rs`)
//! used to perform this cleanup as their final step.  That coupled
//! "did the test pass?" with "is the host process table tidy?" — two
//! orthogonal concerns — so the cleanup logic now lives here and is
//! invoked by `just orphan` (or directly via `rust-script`) only when
//! the operator asks for it.
//!
//! Usage:
//!   rust-script scripts/dev/orphan-cleanup.rs
//!   rust-script scripts/dev/orphan-cleanup.rs --bin target/release/uffs
//!   rust-script scripts/dev/orphan-cleanup.rs --dry-run
//!
//! How "orphan" is decided:
//!   1. Run `<bin> daemon status` to learn the legitimate daemon PID.
//!   2. Enumerate every running `uffs[.exe]` process on the host.
//!   3. Anything that is **not** the daemon, and **not** this script
//!      itself, is an orphan.
//!
//! Cross-platform:
//!   * Unix : `ps -eo pid,command`              + `kill -9 <pid>`
//!   * Windows : `wmic process ... CommandLine` + `taskkill /F /PID`

use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
use colored::Colorize;

fn main() -> Result<()> {
    let mut args_iter = std::env::args().skip(1);
    let mut bin_override: Option<String> = None;
    let mut dry_run = false;
    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--bin" | "-b" | "--binary" => bin_override = args_iter.next(),
            "--dry-run" | "-n" => dry_run = true,
            "--help" | "-h" => {
                eprintln!("Usage: rust-script scripts/dev/orphan-cleanup.rs [--bin <path>] [--dry-run]");
                eprintln!();
                eprintln!("  --bin <path>   Override the uffs binary used to query daemon PID.");
                eprintln!("                 Default: $HOME/bin/uffs[.exe], target/release/uffs[.exe], or PATH.");
                eprintln!("  --dry-run      List orphan processes without killing them.");
                return Ok(());
            }
            other => {
                eprintln!("Unknown argument: {other}");
                eprintln!("Run with --help for usage.");
                std::process::exit(2);
            }
        }
    }

    let bin = bin_override.unwrap_or_else(default_binary);

    eprintln!();
    eprintln!("┌───────────────────────────────────────────────────────────────┐");
    eprintln!("│  Orphan uffs-process cleanup                                 │");
    eprintln!("└───────────────────────────────────────────────────────────────┘");
    eprintln!("  Binary:  {}", bin.cyan());
    if dry_run {
        eprintln!("  Mode:    {} (no processes will be killed)", "dry-run".yellow().bold());
    }

    // 1. Ask the daemon for its PID via `daemon status` so we always
    //    have the authoritative current PID — even if the on-disk PID
    //    file went stale (different binary, restart race, etc.).
    let daemon_pid = read_daemon_pid_via_status(&bin);
    match daemon_pid {
        Some(pid) => eprintln!("  {} Daemon verified (PID {pid})", "✅".green()),
        None      => eprintln!("  {} Daemon not reachable — every uffs process is an orphan candidate", "⚠️".yellow()),
    }

    // 2. Enumerate every uffs[.exe] process on the host.
    let bin_name = std::path::Path::new(&bin)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("uffs");
    let all_procs = find_uffs_processes(bin_name);

    // 3. Filter: skip self, skip the legitimate daemon.
    let my_pid = std::process::id();
    let orphans: Vec<&(u32, String)> = all_procs
        .iter()
        .filter(|(pid, _)| {
            *pid != my_pid && daemon_pid.map_or(true, |dp| *pid != dp)
        })
        .collect();

    if orphans.is_empty() {
        eprintln!("  {} No orphan uffs processes found.", "✅".green());
        return Ok(());
    }

    eprintln!(
        "  {} {} orphan uffs process(es){}:",
        "⚠️".yellow(),
        orphans.len(),
        if dry_run { "" } else { " — killing" },
    );
    let mut killed = 0_usize;
    for (pid, cmdline) in &orphans {
        eprintln!("    PID {pid}: {cmdline}");
        if !dry_run && kill_process(*pid) {
            killed += 1;
        }
    }
    if dry_run {
        eprintln!("  {} Re-run without --dry-run to actually kill them.", "ℹ".cyan());
    } else {
        eprintln!("  🔪 Killed {killed}/{} orphan process(es).", orphans.len());
    }
    Ok(())
}

/// Locate an existing uffs binary; do **not** auto-build.
///
/// Mirrors `scripts/windows/{api,cli,mcp}-validation.rs::default_binary`
/// — same search order so the cleanup runs against whatever artifact
/// the validation suites would also use.
///
/// Search order (cross-platform):
///   1. `$HOME/bin/uffs[.exe]`           — `just use` install location
///   2. `target/release/uffs[.exe]`      — `cargo build --release` output
///   3. Bare `uffs[.exe]`                — falls through to PATH lookup
fn default_binary() -> String {
    let bin_name = if cfg!(windows) { "uffs.exe" } else { "uffs" };
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(home_var).unwrap_or_else(|_| ".".to_string());
    let candidates = [
        PathBuf::from(&home).join("bin").join(bin_name),
        PathBuf::from("target").join("release").join(bin_name),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    bin_name.to_string()
}

/// Run `<bin> daemon status` and extract the `Daemon PID:` value.
///
/// Returns `None` when the daemon is unreachable (no socket, no
/// listener, no `Daemon PID:` line in the output).  Same parser
/// pattern as the validation scripts' `capture_daemon_version`,
/// targeting a different label.
fn read_daemon_pid_via_status(bin: &str) -> Option<u32> {
    let out = Command::new(bin).args(["--daemon", "status"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Daemon PID:") {
            return rest.trim().parse::<u32>().ok();
        }
    }
    None
}

/// Returns `Vec<(pid, command line)>` for every running `<bin_name>`
/// process visible to the current user.
fn find_uffs_processes(bin_name: &str) -> Vec<(u32, String)> {
    let my_pid = std::process::id();

    #[cfg(unix)]
    {
        // `ps -eo pid,command` lists all visible processes with their
        // full command line.  Filter for our binary name, skip
        // helper processes like `ps`, `grep`, and `rust-script`.
        let output = Command::new("ps").args(["-eo", "pid,command"]).output().ok();
        let stdout = output
            .as_ref()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        stdout
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if !line.contains(bin_name) {
                    return None;
                }
                if line.contains("ps -eo") || line.contains("grep") || line.contains("rust-script") {
                    return None;
                }
                let mut parts = line.splitn(2, char::is_whitespace);
                let pid: u32 = parts.next()?.trim().parse().ok()?;
                let cmd = parts.next().unwrap_or("").trim().to_string();
                if pid == my_pid {
                    return None;
                }
                Some((pid, cmd))
            })
            .collect()
    }

    #[cfg(windows)]
    {
        // WMIC carries the full command line; `tasklist` does not.
        let output = Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("name like '%{bin_name}%'"),
                "get",
                "ProcessId,CommandLine",
                "/format:csv",
            ])
            .output()
            .ok();
        let stdout = output
            .as_ref()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        stdout
            .lines()
            .skip(1) // CSV header
            .filter_map(|line| {
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() < 3 {
                    return None;
                }
                let cmd = fields[1].trim().to_string();
                let pid: u32 = fields[2].trim().parse().ok()?;
                if pid == my_pid || cmd.is_empty() {
                    return None;
                }
                Some((pid, cmd))
            })
            .collect()
    }
}

/// Send SIGKILL (Unix) or `taskkill /F` (Windows) to the given PID.
fn kill_process(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
