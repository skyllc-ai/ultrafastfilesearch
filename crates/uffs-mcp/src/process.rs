// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! OS-level process management helpers for `uffsmcp`.
//!
//! Signal delivery, port scanning, PID file parsing, and reload logic.

use anyhow::Result;

/// Build `--mft-file` / `--data-dir` args for daemon auto-start.
///
/// Returns `OsString` entries so a path containing non-UTF-8 / WTF-8 bytes is
/// forwarded to the spawned daemon verbatim, never `to_string_lossy`-mangled
/// (Category 4, WI-4.2). Flag literals are ASCII; only the path values carry
/// the OS-native bytes.
#[must_use]
pub(crate) fn build_daemon_args(
    mft_files: &[std::path::PathBuf],
    data_dir: Option<&std::path::Path>,
) -> Vec<std::ffi::OsString> {
    let mut args = Vec::new();
    if let Some(dir) = data_dir {
        args.push(std::ffi::OsString::from("--data-dir"));
        args.push(dir.as_os_str().to_os_string());
    }
    for path in mft_files {
        args.push(std::ffi::OsString::from("--mft-file"));
        args.push(path.as_os_str().to_os_string());
    }
    args
}

/// Send a signal to a process.
///
/// When `force` is true: SIGKILL (Unix) / `/F` (Windows).
/// When `force` is false: SIGTERM (Unix) / normal taskkill (Windows).
pub(crate) fn signal_pid(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        let sig = if force { "-9" } else { "-15" };
        drop(
            std::process::Command::new("kill")
                .args([sig, &pid.to_string()])
                .output(),
        );
    }
    #[cfg(not(unix))]
    {
        let mut cmd = std::process::Command::new("taskkill");
        if force {
            cmd.arg("/F");
        }
        cmd.args(["/PID", &pid.to_string()]);
        drop(cmd.output());
    }
}

/// Send SIGHUP to a process (Unix only; no-op on Windows).
pub(crate) fn signal_pid_hup(pid: u32) {
    #[cfg(unix)]
    {
        drop(
            std::process::Command::new("kill")
                .args(["-1", &pid.to_string()])
                .output(),
        );
    }
    #[cfg(not(unix))]
    {
        signal_pid(pid, false);
    }
}

/// Try to find and kill any process listening on `port`.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn kill_process_on_port(port: u16, skip_pid: u32) {
    #[cfg(unix)]
    {
        let Ok(output) = std::process::Command::new("lsof")
            .args(["-ti", &format!(":{port}")])
            .output()
        else {
            return;
        };
        // AUDIT-OK(bytes): per-line PID scan of `lsof` output. The actual
        // targeting decision is `line.trim().parse::<u32>()`, which fails
        // closed on any U+FFFD-mangled line (a corrupted digit string does
        // not parse). Whole-buffer strict from_utf8 would instead discard
        // every line if one byte were invalid — a worse, less-robust result.
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Ok(pid) = line.trim().parse::<u32>()
                && pid != skip_pid
                && pid != std::process::id()
            {
                println!("  Also killing stale process on port {port} (PID {pid})...");
                signal_pid(pid, true);
            }
        }
    }
    #[cfg(windows)]
    {
        let Ok(output) = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return;
        };
        // AUDIT-OK(bytes): per-line scan of `netstat` output; the decision is
        // the `LISTENING` substring + PID parse per line. A mangled line
        // simply fails to match; whole-buffer strict decode would drop all
        // lines on a single bad byte.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let port_suffix = format!(":{port}");
        for line in stdout.lines() {
            let trimmed = line.trim();
            if !trimmed.contains("LISTENING") {
                continue;
            }
            let fields: Vec<&str> = trimmed.split_whitespace().collect();
            let (Some(local_addr), Some(pid_field)) = (fields.get(1), fields.get(4)) else {
                continue;
            };
            if !local_addr.ends_with(&port_suffix) {
                continue;
            }
            let Ok(pid) = pid_field.parse::<u32>() else {
                continue;
            };
            if pid != skip_pid && pid != std::process::id() && pid != 0 {
                println!("  Also killing stale process on port {port} (PID {pid})...");
                signal_pid(pid, true);
            }
        }
    }
}

/// Minimal HTTP GET — no external deps needed.
pub(crate) async fn reqwest_lite_get(raw_url: &str) -> Result<String> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let stripped = raw_url.strip_prefix("http://").unwrap_or(raw_url);
    let (host_port, rel_path) = stripped.split_once('/').unwrap_or((stripped, ""));
    let abs_path = format!("/{rel_path}");
    let stream = tokio::net::TcpStream::connect(host_port).await?;
    let (mut reader, mut writer) = stream.into_split();
    let request =
        format!("GET {abs_path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    writer.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    reader.read_to_end(&mut response).await?;
    // AUDIT-OK(bytes): HTTP health-probe response body returned for
    // display/logging to the operator, not used for a trust/targeting
    // decision; lossy decode is acceptable here.
    let text = String::from_utf8_lossy(&response);
    Ok(text
        .split_once("\r\n\r\n")
        .map_or_else(|| text.to_string(), |(_, body)| body.trim().to_owned()))
}

/// `uffsmcp restart` — kill the running MCP server so the AI host respawns it.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn mcp_restart() {
    let Some(pid) = uffs_client::mcp_pid::is_mcp_server_running() else {
        println!("MCP server is not running — nothing to restart.");
        println!("  Start it with: uffsmcp start");
        return;
    };
    println!("Stopping MCP server (PID {pid})...");
    signal_pid(pid, true);
    drop(std::fs::remove_file(
        uffs_client::mcp_pid::mcp_pid_file_path(),
    ));
    println!("MCP server killed.");
    println!("  The AI host will respawn it, or run: uffsmcp start");
    println!("  (The daemon continues running — no re-index needed.)");
}

/// `uffsmcp reload` — reload stale MCP components to pick up a new binary.
#[expect(
    clippy::print_stdout,
    clippy::too_many_lines,
    reason = "CLI output; sequential reload pipeline"
)]
pub(crate) async fn mcp_reload() -> Result<()> {
    use uffs_client::connect::UffsClient;
    use uffs_client::daemon_ctl::{pid_file_path, socket_path};

    let exe_mtime = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok());

    let Some(bin_mtime) = exe_mtime else {
        anyhow::bail!("Cannot determine current binary mtime.");
    };

    println!("Reloading MCP stack...");
    let mut anything_reloaded = false;

    let pid_info = uffs_client::mcp_pid::parse_mcp_pid_file_full();
    let gw_pid = pid_info
        .as_ref()
        .filter(|_info| uffs_client::mcp_pid::is_mcp_server_running().is_some())
        .map(|info| info.pid);

    let gw_config = {
        let from_pid_file = pid_info.as_ref().and_then(|info| {
            let (bind, port) = info.http_addr()?;
            Some(GatewayConfig {
                bind: bind.to_owned(),
                port,
                data_dir: info.data_dir.clone(),
                mft_files: info.mft_files.clone(),
                no_cache: info.no_cache,
            })
        });
        if from_pid_file
            .as_ref()
            .is_some_and(|cfg| cfg.data_dir.is_some() || !cfg.mft_files.is_empty())
        {
            from_pid_file
        } else {
            gw_pid.and_then(read_gateway_config).or(from_pid_file)
        }
    };

    let can_restart_daemon = cfg!(windows)
        || gw_config
            .as_ref()
            .is_some_and(|cfg| cfg.data_dir.is_some() || !cfg.mft_files.is_empty());

    // 1. Daemon
    if let Ok(mut client) = UffsClient::connect_raw().await
        && let Ok(status) = client.status().await
    {
        let uptime = core::time::Duration::from_secs(status.uptime_secs);
        let started = std::time::SystemTime::now() - uptime;
        if started < bin_mtime {
            if can_restart_daemon {
                println!("  ✗ Daemon PID {} is stale — killing...", status.pid);
                signal_pid(status.pid, true);
                drop(std::fs::remove_file(pid_file_path()));
                drop(std::fs::remove_file(socket_path()));
                std::thread::sleep(core::time::Duration::from_millis(300));
                anything_reloaded = true;
            } else {
                println!(
                    "  ✗ Daemon PID {} is stale but no data sources — skipping.",
                    status.pid
                );
            }
        } else {
            println!("  ✓ Daemon PID {} is current.", status.pid);
        }
    }

    // 2. HTTP gateway
    if let (Some(pid), Some(config)) = (gw_pid, &gw_config) {
        let gw_start = process_start_time(pid);
        let is_stale = gw_start.is_none_or(|st| st < bin_mtime);
        if is_stale {
            let has_data =
                cfg!(windows) || config.data_dir.is_some() || !config.mft_files.is_empty();
            if has_data {
                println!("  ✗ HTTP gateway PID {pid} is stale — restarting...");
                signal_pid(pid, true);
                drop(std::fs::remove_file(
                    uffs_client::mcp_pid::mcp_pid_file_path(),
                ));
                std::thread::sleep(core::time::Duration::from_millis(500));
                // Restart by spawning `uffsmcp start`
                let exe = std::env::current_exe().unwrap_or_default();
                let mut cmd = std::process::Command::new(&exe);
                cmd.args([
                    "start",
                    "--bind",
                    &config.bind,
                    "--port",
                    &config.port.to_string(),
                ]);
                if let Some(dir) = &config.data_dir {
                    cmd.arg("--data-dir");
                    cmd.arg(dir.as_os_str());
                }
                for mft in &config.mft_files {
                    cmd.arg("--mft-file");
                    cmd.arg(mft.as_os_str());
                }
                if config.no_cache {
                    cmd.arg("--no-cache");
                }
                drop(cmd.status());
                anything_reloaded = true;
            } else {
                println!(
                    "  ✗ HTTP gateway PID {pid} is stale but no data sources — cannot restart."
                );
            }
        } else {
            println!("  ✓ HTTP gateway PID {pid} is current.");
        }
    } else if gw_pid.is_none() {
        println!("  No HTTP gateway running.");
    }

    // 3. Stdio sessions
    let stdio_pids = find_mcp_run_pids();
    for &proc_pid in &stdio_pids {
        let proc_start = process_start_time(proc_pid);
        let is_stale = proc_start.is_none_or(|st| st < bin_mtime);
        if is_stale {
            let parent = resolve_parent_name(proc_pid);
            let host = parent.as_deref().unwrap_or("unknown");
            println!("  ↻ SIGHUP stale stdio PID {proc_pid} (parent: {host})");
            signal_pid_hup(proc_pid);
            anything_reloaded = true;
        }
    }

    if anything_reloaded {
        println!("Reload complete ✓");
    } else {
        println!("Everything is current — nothing to reload.");
    }
    Ok(())
}

/// Config extracted from a running gateway process.
pub(crate) struct GatewayConfig {
    /// `--bind` value.
    pub(crate) bind: String,
    /// `--port` value.
    pub(crate) port: u16,
    /// `--data-dir` value (if any).
    pub(crate) data_dir: Option<std::path::PathBuf>,
    /// `--mft-file` values.
    pub(crate) mft_files: Vec<std::path::PathBuf>,
    /// `--no-cache` flag.
    pub(crate) no_cache: bool,
}

/// Read gateway config from a running process's command line.
#[must_use]
pub(crate) fn read_gateway_config(pid: u32) -> Option<GatewayConfig> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    // Strict decode: the command line is parsed for bind/port below, so a
    // lossy decode must fail closed rather than mis-parse a corrupted arg.
    // (WI-4.3)
    let cmdline = core::str::from_utf8(&output.stdout).ok()?.trim().to_owned();
    if cmdline.is_empty() {
        return None;
    }
    let args: Vec<&str> = cmdline.split_whitespace().collect();
    let mut bind = "127.0.0.1".to_owned();
    let mut port: u16 = 8080;
    let mut data_dir: Option<std::path::PathBuf> = None;
    let mut mft_files: Vec<std::path::PathBuf> = Vec::new();
    let mut no_cache = false;
    let mut i = 0;
    while i < args.len() {
        match args.get(i).copied() {
            Some("--bind") => {
                if let Some(&val) = args.get(i + 1) {
                    val.clone_into(&mut bind);
                    i += 1;
                }
            }
            Some("--port") => {
                if let Some(&val) = args.get(i + 1) {
                    port = val.parse().unwrap_or(8080);
                    i += 1;
                }
            }
            Some("--data-dir") => {
                if let Some(&val) = args.get(i + 1) {
                    data_dir = Some(std::path::PathBuf::from(val));
                    i += 1;
                }
            }
            Some("--mft-file") => {
                if let Some(&val) = args.get(i + 1) {
                    for part in val.split(',') {
                        mft_files.push(std::path::PathBuf::from(part));
                    }
                    i += 1;
                }
            }
            Some("--no-cache") => {
                no_cache = true;
            }
            _ => {}
        }
        i += 1;
    }
    Some(GatewayConfig {
        bind,
        port,
        data_dir,
        mft_files,
        no_cache,
    })
}

/// Check for stale stdio MCP sessions and reload them.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn reload_stale_stdio_sessions() {
    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    let current_mtime = std::fs::metadata(&current_exe)
        .and_then(|meta| meta.modified())
        .ok();
    let stdio_pids = find_mcp_run_pids();
    if stdio_pids.is_empty() {
        return;
    }
    let mut reloaded: u32 = 0;
    for proc_pid in stdio_pids {
        let proc_start = process_start_time(proc_pid);
        let is_stale = match (&current_mtime, proc_start) {
            (Some(bin_mtime), Some(started)) => started < *bin_mtime,
            _ => true,
        };
        if is_stale {
            let parent = resolve_parent_name(proc_pid);
            let host = parent.as_deref().unwrap_or("unknown");
            println!("  Reloading stale stdio session PID {proc_pid} (parent: {host})...");
            signal_pid_hup(proc_pid);
            reloaded += 1;
        }
    }
    if reloaded > 0 {
        println!("  Sent SIGHUP to {reloaded} stale stdio session(s) — hosts will respawn.");
    }
}

/// Get the start time of a process.
#[must_use]
pub(crate) fn process_start_time(pid: u32) -> Option<std::time::SystemTime> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "etime="])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    // Strict decode: etime is parsed into a duration used for the process
    // start-time decision, so invalid UTF-8 fails closed. (WI-4.3)
    let etime_str = core::str::from_utf8(&output.stdout).ok()?.trim().to_owned();
    let uptime = parse_ps_etime(&etime_str);
    Some(std::time::SystemTime::now() - uptime)
}

/// Parse `ps` elapsed time format: `[[dd-]hh:]mm:ss`.
#[must_use]
fn parse_ps_etime(etime: &str) -> core::time::Duration {
    let mut total_secs: u64 = 0;
    let (days_part, time_part) = if let Some((days, rest)) = etime.split_once('-') {
        (days.parse::<u64>().unwrap_or(0), rest)
    } else {
        (0, etime)
    };
    total_secs += days_part * 86400;
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

/// Find PIDs of running `uffs mcp run` / `uffsmcp run` processes.
#[must_use]
pub(crate) fn find_mcp_run_pids() -> Vec<u32> {
    let Ok(raw_output) = std::process::Command::new("ps")
        .args(["-eo", "pid,args"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    // AUDIT-OK(bytes): per-line `pid,args` scan; each line's PID is parsed
    // (fails closed on a mangled line) and args matched by substring. A
    // single bad byte must not discard the whole process list, so per-line
    // lossy decode is the correct, more-robust choice here.
    let text = String::from_utf8_lossy(&raw_output.stdout);
    let my_pid = std::process::id();
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let proc_pid: u32 = fields.next()?.parse().ok()?;
            if proc_pid == my_pid {
                return None;
            }
            let cmdline: String = fields.collect::<Vec<_>>().join(" ");
            let is_mcp_run = cmdline.contains("mcp")
                && cmdline.contains("run")
                && !cmdline.contains("serve")
                && !cmdline.contains("start")
                && !cmdline.contains("kill");
            is_mcp_run.then_some(proc_pid)
        })
        .collect()
}

/// Resolve the name of a parent process by PID.
#[must_use]
fn resolve_parent_name(child_pid: u32) -> Option<String> {
    let ppid_output = std::process::Command::new("ps")
        .args(["-p", &child_pid.to_string(), "-o", "ppid="])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    // Strict parse: this PID targets a follow-up `ps` call, so a lossy
    // U+FFFD-mangled digit string must fail closed (return None), never a
    // wrong PID. (WI-4.3)
    let ppid: u32 = core::str::from_utf8(&ppid_output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if ppid == 0 {
        return None;
    }
    let comm_output = std::process::Command::new("ps")
        .args(["-p", &ppid.to_string(), "-o", "comm="])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    // Strict decode: the name feeds a comparison/targeting decision, so
    // invalid UTF-8 fails closed rather than producing a U+FFFD-corrupted
    // match. (WI-4.3)
    let name = core::str::from_utf8(&comm_output.stdout)
        .ok()?
        .trim()
        .to_owned();
    if name.is_empty() {
        return None;
    }
    Some(name.rsplit('/').next().unwrap_or(&name).to_owned())
}
