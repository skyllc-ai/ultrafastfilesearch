// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs status` — combined daemon + MCP server status in one view.
//!
//! Shows three sections:
//! - **Daemon**: PID, uptime, drives, queries
//! - **MCP HTTP Gateway**: PID, transport, health, sessions, tool calls
//! - **MCP Stdio Sessions**: active `uffs mcp run` processes (one per AI host)

#[cfg(feature = "mcp-http-probe")]
use anyhow::{Context as _, Result};
use uffs_client::connect_sync::UffsClientSync;
use uffs_client::protocol::response::{DaemonStatus, ShardTier};

/// `uffs status` — show combined system status.
///
/// # Errors
///
/// Returns an error if the operation fails.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn system_status() {
    println!("═══ UFFS System Status ═══");
    println!();
    print_daemon_status();
    println!();
    print_mcp_http_status();
    println!();
    print_mcp_stdio_sessions();
}

/// Print daemon status section.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_daemon_status() {
    println!("── Daemon ──");

    let Ok(mut client) = UffsClientSync::connect_raw() else {
        println!("  Status:      not running");
        let pid_path = uffs_client::daemon_ctl::pid_file_path();
        if pid_path.exists() {
            println!("  PID file:    {} (stale)", pid_path.display());
        }
        return;
    };

    let Ok(status) = client.status() else {
        println!("  Status:      connected but not responding");
        return;
    };

    let uptime = core::time::Duration::from_secs(status.uptime_secs);
    let daemon_stale = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
        .is_some_and(|bin_mtime| {
            let started = std::time::SystemTime::now() - uptime;
            started < bin_mtime
        });
    let stale_tag = if daemon_stale {
        "  ⚠ stale binary"
    } else {
        ""
    };
    println!(
        "  Version:     {}",
        crate::commands::version_summary(&status.version)
    );
    println!("  Status:      running (PID {}){stale_tag}", status.pid);
    println!(
        "  Uptime:      {}",
        uffs_client::format::format_duration(uptime)
    );

    match &status.status {
        DaemonStatus::Loading {
            drives_loaded,
            drives_total,
        } => {
            println!("  State:       Loading ({drives_loaded}/{drives_total} drives)");
        }
        DaemonStatus::Ready => {
            println!("  State:       Ready");
        }
        DaemonStatus::Refreshing { drives } => {
            let drive_list: String = drives
                .iter()
                .map(|letter| format!("{letter}:"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  State:       Refreshing ({drive_list})");
        }
    }
    println!("  Connections: {}", status.connections);

    if let Ok(drives) = client.drives() {
        print_drive_summary(&drives.drives);
    }

    // Performance stats.
    if let Ok(stats) = client.stats() {
        let fmt = uffs_client::format::format_duration;
        let startup = core::time::Duration::from_millis(stats.startup_duration_ms);
        println!("  Startup:     {}", fmt(startup));
        println!("  Queries:     {}", stats.total_queries);
        if stats.total_queries > 0 {
            let avg = core::time::Duration::from_micros(uffs_client::format::f64_to_u64(
                stats.avg_query_time_us,
            ));
            println!("  Avg query:   {}", fmt(avg));
            println!("  Queries/s:   {:.2}", stats.queries_per_second);
        }
    }
}

/// Render the `Drives:` block of `uffs status`.
///
/// Phase 5 task 5.11: enumerate every shard in the registry (Hot /
/// Warm / Parked / Cold) and tag each row with its tier marker.
/// `total_records` reflects only Warm/Hot bodies — Parked/Cold have
/// no body in RAM, so their `records` field is 0 and excluded from
/// the headline count.  Empty registry still renders `(none loaded)`
/// so cold-boot detection in external scripts (`api-validation`,
/// `cli-validation`, `mcp-validation`) keeps working.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_drive_summary(drives: &[uffs_client::protocol::response::DriveInfo]) {
    if drives.is_empty() {
        println!("  Drives:      (none loaded)");
        return;
    }

    let total_records: usize = drives.iter().map(|dr| dr.records).sum();
    let active = drives
        .iter()
        .filter(|dr| matches!(dr.tier, Some(ShardTier::Warm | ShardTier::Hot) | None))
        .count();
    let parked = drives
        .iter()
        .filter(|dr| matches!(dr.tier, Some(ShardTier::Parked)))
        .count();
    let cold = drives
        .iter()
        .filter(|dr| matches!(dr.tier, Some(ShardTier::Cold)))
        .count();
    println!(
        "  Drives:      {} loaded ({} records, {active} active / {parked} parked / {cold} cold)",
        drives.len(),
        uffs_client::format::format_number_commas(total_records as u64),
    );
    for dr in drives {
        let marker = compact_tier_marker(dr.tier);
        match dr.tier {
            Some(ShardTier::Parked) => {
                println!("    {} {}: bloom + trie resident", marker, dr.letter);
            }
            Some(ShardTier::Cold) => {
                println!("    {} {}: cache only (no RAM)", marker, dr.letter);
            }
            Some(ShardTier::Hot | ShardTier::Warm) | None => {
                println!(
                    "    {} {}: {:>10} records",
                    marker,
                    dr.letter,
                    uffs_client::format::format_number_commas(dr.records as u64),
                );
            }
            Some(ShardTier::Evicting | ShardTier::Unknown) => {
                println!("    {} {}: ({})", marker, dr.letter, dr.source);
            }
        }
    }
}

/// Compact tier marker for `uffs status`'s drive list — fixed-width
/// bracket label per tier so the per-drive lines align in the
/// combined system view.  Phase 5 task 5.11.
const fn compact_tier_marker(tier: Option<ShardTier>) -> &'static str {
    match tier {
        Some(ShardTier::Hot) => "[H]",
        Some(ShardTier::Warm) => "[W]",
        Some(ShardTier::Parked) => "[P]",
        Some(ShardTier::Cold) => "[C]",
        Some(ShardTier::Evicting) => "[E]",
        Some(ShardTier::Unknown) => "[?]",
        None => "[ ]",
    }
}

/// Print MCP HTTP gateway status section.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_mcp_http_status() {
    println!("── MCP HTTP Gateway ──");

    // Read the PID file.  Only show HTTP-transport entries here;
    // stdio entries are handled by `print_mcp_stdio_sessions()`.
    let info = match uffs_client::mcp_pid::parse_mcp_pid_file_full() {
        Some(info) if info.http_addr().is_some() => info,
        _ => {
            println!("  Status:      not running");
            return;
        }
    };

    let alive = uffs_client::mcp_pid::is_mcp_server_running().is_some();
    if !alive {
        println!(
            "  Status:      not running (stale PID file, PID {})",
            info.pid
        );
        return;
    }

    let uptime_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |dur| dur.as_secs().saturating_sub(info.start_ts));
    let uptime = core::time::Duration::from_secs(uptime_secs);

    // Stale binary check: was the gateway started before the current
    // binary was last modified?
    let gw_stale = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
        .is_some_and(|bin_mtime| {
            let gw_started = std::time::UNIX_EPOCH + core::time::Duration::from_secs(info.start_ts);
            gw_started < bin_mtime
        });
    let stale_tag = if gw_stale { "  ⚠ stale binary" } else { "" };

    println!("  Status:      running (PID {}){stale_tag}", info.pid);
    println!(
        "  Uptime:      {}",
        uffs_client::format::format_duration(uptime)
    );

    // Probe HTTP /status endpoint for health + stats.
    //
    // Gated behind the `mcp-http-probe` feature: enabling it pulls in
    // `std::net::TcpStream`, which on Windows unconditionally links
    // `ws2_32.dll` and adds measurable process-launch overhead.  When
    // the feature is off we still report the configured bind address,
    // just without actively probing it.
    if let Some((bind, port)) = info.http_addr() {
        #[cfg(feature = "mcp-http-probe")]
        match http_get_json(bind, port, "/status") {
            Ok(json) => {
                println!("  Health:      ✓ (http://{bind}:{port}/health)");
                println!("  Endpoint:    http://{bind}:{port}/mcp");

                // Display MCP stats from the /status response.
                if let Some(stats) = json.get("mcp_stats") {
                    print_mcp_stats(stats);
                }
            }
            Err(err) => {
                println!("  Health:      ✗ unreachable ({err})");
                println!("  Endpoint:    http://{bind}:{port}/mcp");
            }
        }
        #[cfg(not(feature = "mcp-http-probe"))]
        {
            println!("  Endpoint:    http://{bind}:{port}/mcp");
            println!(
                "  Health:      (probe disabled — rebuild with `--features mcp-http-probe` to enable)"
            );
        }
    }
    if gw_stale {
        println!("  Run `uffs mcp reload` to restart with the current binary.");
    }
}

/// Display MCP stats from the `/status` JSON response.
///
/// Only compiled when [`http_get_json`] is available (i.e. the
/// `mcp-http-probe` feature is enabled).
#[cfg(feature = "mcp-http-probe")]
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_mcp_stats(stats: &serde_json::Value) {
    let tool_calls = stats["tool_calls"].as_u64().unwrap_or(0);
    let tool_errors = stats["tool_errors"].as_u64().unwrap_or(0);
    let avg_latency = stats["avg_tool_latency_us"].as_u64().unwrap_or(0);
    let sessions = stats["active_sessions"].as_u64().unwrap_or(0);
    let total_sessions = stats["total_sessions"].as_u64().unwrap_or(0);
    let resource_reads = stats["resource_reads"].as_u64().unwrap_or(0);
    let prompt_gets = stats["prompt_gets"].as_u64().unwrap_or(0);

    println!("  Sessions:    {sessions} active / {total_sessions} total");
    println!("  Tool calls:  {tool_calls} ({tool_errors} errors)");
    if tool_calls > 0 {
        let avg = core::time::Duration::from_micros(avg_latency);
        println!(
            "  Avg latency: {}",
            uffs_client::format::format_duration(avg)
        );
    }
    if resource_reads > 0 || prompt_gets > 0 {
        println!("  Resources:   {resource_reads} reads, {prompt_gets} prompts");
    }

    // Per-tool breakdown (only if there are calls).
    if tool_calls > 0
        && let Some(tools) = stats.get("tools").and_then(|val| val.as_object())
    {
        let mut tool_list: Vec<_> = tools
            .iter()
            .filter(|(_, cnt)| cnt.as_u64().unwrap_or(0) > 0)
            .collect();
        tool_list.sort_by(|lhs, rhs| {
            rhs.1
                .as_u64()
                .unwrap_or(0)
                .cmp(&lhs.1.as_u64().unwrap_or(0))
        });
        if !tool_list.is_empty() {
            println!("  By tool:");
            for (name, count) in &tool_list {
                println!("    {name:.<20} {}", count.as_u64().unwrap_or(0));
            }
        }
    }
}

/// Print MCP stdio session list.
///
/// Scans for running `uffs mcp run` processes.  Each one is an AI-host
/// (Augment, Claude Desktop, Cursor, etc.) connected via stdio transport.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_mcp_stdio_sessions() {
    println!("── MCP Stdio Sessions ──");

    let sessions = find_mcp_stdio_processes();
    if sessions.is_empty() {
        println!("  (none)");
        return;
    }

    let mut any_stale = false;
    for (idx, session) in sessions.iter().enumerate() {
        let num = idx + 1;
        let ppid_info = session
            .parent_name
            .as_deref()
            .map_or(String::new(), |name| format!("  (parent: {name})"));
        let stale_tag = if session.is_stale {
            any_stale = true;
            "  ⚠ stale binary"
        } else {
            ""
        };
        println!(
            "  {num}. PID {pid:<8} uptime: {uptime}{ppid_info}{stale_tag}",
            pid = session.pid,
            uptime = uffs_client::format::format_duration(session.uptime),
        );
    }
    if any_stale {
        println!("  Run `uffs mcp reload` to restart stale sessions.");
    }
}

/// Information about a running MCP stdio process.
struct StdioSession {
    /// Process ID.
    pid: u32,
    /// How long the process has been running.
    uptime: core::time::Duration,
    /// Name of the parent process (the AI host), if available.
    parent_name: Option<String>,
    /// True if the process's binary is older than the current binary.
    is_stale: bool,
}

/// Find running `uffs mcp run` processes via `ps`.
///
/// Also detects stale binaries by comparing the on-disk mtime of each
/// process's executable against the current running binary.
fn find_mcp_stdio_processes() -> Vec<StdioSession> {
    let Ok(raw_output) = std::process::Command::new("ps")
        .args(["-eo", "pid,ppid,etime,args"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&raw_output.stdout);
    let my_pid = std::process::id();

    // Mtime of the current binary — used to detect stale sessions.
    let current_mtime = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok());

    let mut sessions = Vec::new();
    for line in text.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let Some(pid_str) = fields.next() else {
            continue;
        };
        let Ok(proc_pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if proc_pid == my_pid {
            continue;
        }
        let parent_pid: u32 = fields.next().and_then(|val| val.parse().ok()).unwrap_or(0);
        let Some(elapsed_time) = fields.next() else {
            continue;
        };
        let cmdline: String = fields.collect::<Vec<_>>().join(" ");

        // Match `uffs mcp run` in any path.
        if !cmdline.contains("mcp") || !cmdline.contains("run") {
            continue;
        }
        // Exclude `mcp serve` (HTTP gateway) and helper processes.
        if cmdline.contains("serve") || cmdline.contains("start") || cmdline.contains("kill") {
            continue;
        }

        let uptime = parse_ps_etime(elapsed_time);
        let parent_name = resolve_parent_name(parent_pid);

        // Detect stale binary: the process started before the current
        // binary was last modified (i.e. a rebuild happened after the
        // process was spawned).
        let is_stale = current_mtime.is_some_and(|bin_mtime| {
            let proc_started = std::time::SystemTime::now() - uptime;
            proc_started < bin_mtime
        });

        sessions.push(StdioSession {
            pid: proc_pid,
            uptime,
            parent_name,
            is_stale,
        });
    }
    sessions
}

/// Parse `ps` elapsed time format: `[[dd-]hh:]mm:ss`.
fn parse_ps_etime(etime: &str) -> core::time::Duration {
    let mut total_secs: u64 = 0;
    let (days_part, time_part) = if let Some((days, rest)) = etime.split_once('-') {
        (days.parse::<u64>().unwrap_or(0), rest)
    } else {
        (0, etime)
    };
    total_secs += days_part * 86400;
    // Split into parts and iterate from right: ss, mm, [hh].
    let mut parts = time_part.rsplit(':');
    if let Some(ss) = parts.next() {
        total_secs += ss.parse::<u64>().unwrap_or(0);
    }
    if let Some(mm) = parts.next() {
        total_secs += mm.parse::<u64>().unwrap_or(0) * 60;
    }
    if let Some(hh) = parts.next() {
        total_secs += hh.parse::<u64>().unwrap_or(0) * 3600;
    }
    core::time::Duration::from_secs(total_secs)
}

/// Resolve the name of a parent process by PID.
fn resolve_parent_name(ppid: u32) -> Option<String> {
    if ppid == 0 {
        return None;
    }
    let output = std::process::Command::new("ps")
        .args(["-p", &ppid.to_string(), "-o", "comm="])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if name.is_empty() {
        return None;
    }
    // Extract just the binary name from the path.
    let short = name.rsplit('/').next().unwrap_or(&name).to_owned();
    Some(short)
}

/// HTTP GET returning parsed JSON body (blocking).
///
/// Gated behind the `mcp-http-probe` feature: it is the sole user of
/// `std::net::TcpStream` in the CLI, and keeping it out of the default
/// build drops `ws2_32.dll` from the Windows CLI binary.
#[cfg(feature = "mcp-http-probe")]
fn http_get_json(bind: &str, port: u16, path: &str) -> Result<serde_json::Value> {
    use std::io::{Read as _, Write as _};

    let addr = format!("{bind}:{port}");
    let mut stream =
        std::net::TcpStream::connect(&addr).with_context(|| format!("connect to {addr}"))?;
    _ = stream.set_read_timeout(Some(core::time::Duration::from_secs(5)));

    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;

    let text = String::from_utf8_lossy(&response);
    let body = text
        .split_once("\r\n\r\n")
        .map_or("", |(_, resp_body)| resp_body.trim());
    serde_json::from_str(body).with_context(|| format!("bad JSON from {path}: {body}"))
}
