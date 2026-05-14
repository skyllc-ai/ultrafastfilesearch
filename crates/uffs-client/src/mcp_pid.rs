// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! MCP server PID file management.
//!
//! Pure file I/O utilities for reading and writing the MCP server PID file.
//! Lives in `uffs-client` so that both `uffs` (CLI) and `uffsmcp` can use
//! it without creating a circular dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// Path to the MCP server PID file.
///
/// Separate from the daemon PID file (`daemon.pid`).
#[must_use]
pub fn mcp_pid_file_path() -> std::path::PathBuf {
    let base = dirs_next::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("uffs").join("mcp-server.pid")
}

/// Write the MCP server PID file with explicit transport info.
///
/// Only called by the HTTP gateway to record `http:bind:port`.
/// Stdio servers do **not** write PID files (multiple can coexist).
pub fn write_mcp_pid_file_with_transport(transport: &str) {
    write_mcp_pid_file_full(transport, None, &[], false);
}

/// Write the MCP server PID file with transport **and** data source info.
///
/// Called by `mcp start` / `mcp serve` so that `mcp reload` can recover
/// data sources even when the gateway process is already dead.
///
/// # PID file format
///
/// ```text
/// {pid}
/// {unix_ts}
/// {transport}         e.g. "http:127.0.0.1:8080"
/// data-dir={path}     optional — persisted so `reload` can recover
/// mft-file={path}     optional, repeatable
/// no-cache            optional flag
/// ```
#[expect(
    clippy::format_push_string,
    reason = "write! requires fmt::Write import; push_str+format is fine here"
)]
pub fn write_mcp_pid_file_full(
    transport: &str,
    data_dir: Option<&std::path::Path>,
    mft_files: &[std::path::PathBuf],
    no_cache: bool,
) {
    let path = mcp_pid_file_path();
    if let Some(parent) = path.parent() {
        drop(std::fs::create_dir_all(parent));
    }
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |dur| dur.as_secs());
    let mut content = format!("{pid}\n{ts}\n{transport}\n");
    if let Some(dir) = data_dir {
        content.push_str(&format!("data-dir={}\n", dir.display()));
    }
    for mft in mft_files {
        content.push_str(&format!("mft-file={}\n", mft.display()));
    }
    if no_cache {
        content.push_str("no-cache\n");
    }
    if let Err(err) = std::fs::write(&path, content) {
        tracing::warn!(path = %path.display(), %err, "Failed to write MCP PID file");
    } else {
        tracing::info!(pid, path = %path.display(), transport, "Wrote MCP server PID file");
    }
}

/// Remove the MCP server PID file (best-effort).
pub fn remove_mcp_pid_file() {
    let path = mcp_pid_file_path();
    if path.exists() {
        drop(std::fs::remove_file(&path));
        tracing::info!(path = %path.display(), "Removed MCP server PID file");
    }
}

/// Parsed MCP server PID file.
///
/// # Field discipline (Phase 3b §3.4)
///
/// All fields are `pub` because this is a **parsed-data DTO** returned
/// by [`parse_mcp_pid_file_full`] — callers read fields directly to
/// render `uffs mcp status` output and to decide whether the running
/// MCP server matches the requested transport / data sources.  No
/// invariants are protected (the parse function is the validator).
///
/// # `#[non_exhaustive]` decision (Phase 3b §3.6)
///
/// **Kept exhaustive.**  The PID-file shape evolves only in lockstep
/// with the writer functions ([`write_mcp_pid_file_with_transport`],
/// [`write_mcp_pid_file_full`]) in this same module — there is no
/// scenario where an external consumer reads a future version of the
/// file that has fields the current `McpPidInfo` cannot represent.
#[derive(Debug, Clone)]
pub struct McpPidInfo {
    /// Process ID.
    pub pid: u32,
    /// Start timestamp (Unix epoch seconds).
    pub start_ts: u64,
    /// Transport: `"stdio"` or `"http:bind:port"`.
    pub transport: String,
    /// Data directory (if persisted).
    pub data_dir: Option<std::path::PathBuf>,
    /// MFT file paths (if persisted).
    pub mft_files: Vec<std::path::PathBuf>,
    /// Whether `--no-cache` was set.
    pub no_cache: bool,
}

impl McpPidInfo {
    /// If HTTP transport, extract `(bind, port)`.
    #[must_use]
    pub fn http_addr(&self) -> Option<(&str, u16)> {
        let rest = self.transport.strip_prefix("http:")?;
        let (bind, port_str) = rest.rsplit_once(':')?;
        let port: u16 = port_str.parse().ok()?;
        Some((bind, port))
    }

    /// Returns `true` if data sources were persisted in the PID file.
    #[must_use]
    pub const fn has_data_sources(&self) -> bool {
        self.data_dir.is_some() || !self.mft_files.is_empty()
    }
}

/// Parse the MCP server PID file.  Returns `(pid, start_timestamp)`.
#[must_use]
pub fn parse_mcp_pid_file() -> Option<(u32, u64)> {
    let info = parse_mcp_pid_file_full()?;
    Some((info.pid, info.start_ts))
}

/// Parse the full MCP server PID file (pid, timestamp, transport, data
/// sources).
#[must_use]
pub fn parse_mcp_pid_file_full() -> Option<McpPidInfo> {
    let content = std::fs::read_to_string(mcp_pid_file_path()).ok()?;
    let mut lines = content.lines();
    let pid: u32 = lines.next()?.parse().ok()?;
    let ts: u64 = lines.next()?.parse().ok()?;
    let transport = lines.next().unwrap_or("stdio").to_owned();

    let mut data_dir: Option<std::path::PathBuf> = None;
    let mut mft_files: Vec<std::path::PathBuf> = Vec::new();
    let mut no_cache = false;

    for line in lines {
        if let Some(dir) = line.strip_prefix("data-dir=") {
            data_dir = Some(std::path::PathBuf::from(dir));
        } else if let Some(mft) = line.strip_prefix("mft-file=") {
            mft_files.push(std::path::PathBuf::from(mft));
        } else if line == "no-cache" {
            no_cache = true;
        }
    }

    Some(McpPidInfo {
        pid,
        start_ts: ts,
        transport,
        data_dir,
        mft_files,
        no_cache,
    })
}

/// Check if the MCP server process (from the PID file) is still alive.
#[must_use]
pub fn is_mcp_server_running() -> Option<u32> {
    let (pid, _ts) = parse_mcp_pid_file()?;
    is_process_alive(pid).then_some(pid)
}

/// Check if a process is alive (platform-specific).
#[must_use]
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
    }
}
