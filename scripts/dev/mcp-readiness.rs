#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! serde_json = "1.0"
//! ```
// =============================================================================
// scripts/dev/mcp-readiness.rs — UFFS MCP HTTP Server Lifecycle Verification
// =============================================================================
//
// Exercises MCP HTTP server lifecycle flows — startup, health checks,
// shutdown, daemon dependency, PID management, and recovery.
//
//   Scenario A: Clean lifecycle — start HTTP server, health, shutdown
//   Scenario B: Daemon already running → instant MCP HTTP start
//   Scenario C: Daemon NOT running → MCP HTTP auto-starts daemon
//   Scenario D: MCP status / stop / kill CLI commands
//   Scenario E: Double start (idempotent) — gateway ✓ + daemon ✓
//   Scenario F: Hard kill → PID file cleanup → fresh start
//   Scenario G: Daemon killed while MCP HTTP alive → `mcp start` restarts daemon only
//   Scenario H: HTTP /mcp endpoint (MCP initialize via HTTP)
//   Scenario I: MCP HTTP startup timing
//   Scenario J: Stale port occupant → kill and start fresh
//   Scenario K: `mcp kill` cleans up stale port processes
//   Scenario L: Daemon killed → next tool call reconnects
//   Scenario M: Daemon load — hot-load MFT into running daemon
//
// Usage:
//   rust-script scripts/dev/mcp-readiness.rs ~/uffs_data
//   rust-script scripts/dev/mcp-readiness.rs /path/to/C.iocp
//   rust-script scripts/dev/mcp-readiness.rs                  # uses ~/uffs_data

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::{Duration, Instant};
use anyhow::{Context, Result, bail};
use clap::Parser;
use colored::Colorize;
use serde_json::Value;

// ── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "mcp-readiness",
    about = "UFFS MCP HTTP server lifecycle verification",
    after_help = "EXAMPLES:\n  \
        rust-script scripts/dev/mcp-readiness.rs ~/uffs_data\n  \
        rust-script scripts/dev/mcp-readiness.rs /path/to/C_mft.iocp\n  \
        rust-script scripts/dev/mcp-readiness.rs                  # uses ~/uffs_data"
)]
struct Cli {
    /// Path to MFT file or data directory.  Defaults to ~/uffs_data.
    #[arg(value_name = "PATH")]
    path: Option<String>,
    /// Path to the uffs binary.
    #[arg(long)]
    binary: Option<String>,
    /// HTTP port for the MCP server.
    #[arg(long, default_value = "18080")]
    port: u16,
}

// ── Binary / data-source discovery ──────────────────────────────────────────

fn detect_data_source(path: &str) -> Result<(&'static str, String)> {
    let p = std::path::Path::new(path);
    if !p.exists() { bail!("Path does not exist: {path}"); }
    if p.is_file() { Ok(("--mft-file", path.to_owned())) }
    else if p.is_dir() { Ok(("--data-dir", path.to_owned())) }
    else { bail!("Not a file or directory: {path}"); }
}

fn find_workspace_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut dir = cwd.as_path();
    loop {
        if dir.join("Cargo.toml").exists() && dir.join(".cargo").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() { Some(p) => dir = p, None => break }
    }
    cwd
}

fn ensure_fresh_release_build() -> String {
    let ws = find_workspace_root();
    let bin = ws.join("target").join("release").join("uffs");
    eprintln!("╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  Building fresh release binaries (uffs + uffsmcp)...             ║");
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
    eprintln!("  Workspace: {}", ws.display());
    let start = Instant::now();
    // Build both uffs (thin CLI) and uffsmcp (MCP server) — `uffs mcp *`
    // delegates to `uffsmcp` so both must be present.
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "uffs-cli", "-p", "uffs-mcp"])
        .current_dir(&ws).status();
    match status {
        Ok(s) if s.success() => {
            eprintln!("  ✅ Build in {:.1}s → {}\n", start.elapsed().as_secs_f64(), bin.display());
        }
        Ok(s) => { eprintln!("  ❌ Build failed ({s})"); std::process::exit(1); }
        Err(e) => { eprintln!("  ❌ cargo: {e}"); std::process::exit(1); }
    }
    bin.to_string_lossy().into_owned()
}

fn default_binary() -> String {
    if cfg!(windows) {
        if let Ok(h) = std::env::var("USERPROFILE") {
            let d = std::path::PathBuf::from(&h).join("bin").join("uffs.exe");
            if d.exists() { return d.to_string_lossy().into_owned(); }
        }
        "target\\release\\uffs.exe".to_string()
    } else {
        ensure_fresh_release_build()
    }
}

fn default_data_dir() -> Option<String> {
    if cfg!(windows) { return None; }
    let home = std::env::var("HOME").ok()?;
    let dir = std::path::PathBuf::from(home).join("uffs_data");
    if dir.is_dir() { Some(dir.to_string_lossy().into_owned()) } else { None }
}

// ── PID file path (must match uffs-mcp crate) ───────────────────────────────

fn mcp_pid_file_path() -> std::path::PathBuf {
    // macOS: ~/Library/Application Support/uffs/mcp-server.pid
    // Linux: ~/.local/share/uffs/mcp-server.pid
    // Windows: %LOCALAPPDATA%/uffs/mcp-server.pid
    let base = if cfg!(target_os = "macos") {
        std::env::var("HOME").ok()
            .map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
    } else if cfg!(windows) {
        std::env::var("LOCALAPPDATA").ok()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
    } else {
        std::env::var("XDG_DATA_HOME").ok()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var("HOME").ok()
                    .map(|h| std::path::PathBuf::from(h).join(".local/share"))
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            })
    };
    base.join("uffs").join("mcp-server.pid")
}

// ── HTTP helpers (no external HTTP crate — raw TCP) ─────────────────────────

/// Minimal HTTP GET via raw TCP.  Returns the response body.
fn http_get(host: &str, port: u16, path: &str) -> Result<String> {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!("connect to {addr}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes())?;
    stream.flush()?;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    if text.is_empty() {
        bail!("empty response from GET {path} at {addr}");
    }
    let (headers, raw_body) = text.split_once("\r\n\r\n")
        .context("no HTTP body in response")?;
    // Decode chunked transfer encoding if present.
    let body = if headers.to_lowercase().contains("transfer-encoding: chunked") {
        decode_chunked(raw_body)
    } else {
        raw_body.trim().to_owned()
    };


    Ok(body)
}

/// Raw HTTP response including status, headers, and body.
struct HttpResponse {
    status: u16,
    headers: String,
    body: String,
}

impl HttpResponse {
    /// Extract a header value (case-insensitive).
    fn header(&self, name: &str) -> Option<String> {
        let lower = self.headers.to_lowercase();
        let key = format!("{}:", name.to_lowercase());
        lower.lines()
            .find(|l| l.starts_with(&key))
            .map(|l| l[key.len()..].trim().to_owned())
    }
}

/// Minimal HTTP POST via raw TCP.  Returns `(status, body)`.
fn http_post(host: &str, port: u16, path: &str, body: &str) -> Result<(u16, String)> {
    let resp = http_post_full(host, port, path, body, &[])?;
    Ok((resp.status, resp.body))
}

/// Minimal HTTP POST via raw TCP with optional extra headers.
fn http_post_full(
    host: &str,
    port: u16,
    path: &str,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> Result<HttpResponse> {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!("connect to {addr}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(body);
    stream.write_all(req.as_bytes())?;
    stream.flush()?;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    // Parse status code from first line: "HTTP/1.1 200 OK"
    let status = text.split_once(' ')
        .and_then(|(_, rest)| rest.split_once(' '))
        .and_then(|(code, _)| code.parse::<u16>().ok())
        .unwrap_or(0);
    let (raw_headers, raw_body) = text.split_once("\r\n\r\n")
        .map(|(h, b)| (h.to_owned(), b.to_owned()))
        .unwrap_or_default();
    // Decode chunked transfer encoding if present.
    let body_decoded = if raw_headers.to_lowercase().contains("transfer-encoding: chunked") {
        decode_chunked(&raw_body)
    } else {
        raw_body.trim().to_owned()
    };
    Ok(HttpResponse { status, headers: raw_headers, body: body_decoded })
}

/// Decode HTTP chunked transfer encoding.
///
/// Format: `{hex_size}\r\n{data}\r\n{hex_size}\r\n{data}\r\n0\r\n\r\n`
fn decode_chunked(raw: &str) -> String {
    let mut result = String::new();
    let mut remaining = raw;
    loop {
        // Skip leading whitespace/newlines.
        remaining = remaining.trim_start();
        // Read chunk size (hex).
        let (size_str, rest) = match remaining.split_once("\r\n") {
            Some(pair) => pair,
            None => break,
        };
        // Chunk size may include extensions after `;` — ignore them.
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let size = match usize::from_str_radix(size_hex, 16) {
            Ok(0) => break, // terminal chunk
            Ok(s) => s,
            Err(_) => break,
        };
        // Extract `size` bytes of data.
        if rest.len() >= size {
            result.push_str(&rest[..size]);
            remaining = &rest[size..];
            // Skip trailing \r\n after chunk data.
            remaining = remaining.strip_prefix("\r\n").unwrap_or(remaining);
        } else {
            // Incomplete chunk — take what we have.
            result.push_str(rest);
            break;
        }
    }
    result
}

/// Check if /health returns "ok".
fn health_ok(host: &str, port: u16) -> bool {
    http_get(host, port, "/health").is_ok_and(|b| b == "ok")
}

/// Check /health and return diagnostic detail.
fn health_check_detail(host: &str, port: u16) -> (bool, String) {
    match http_get(host, port, "/health") {
        Ok(body) if body == "ok" => (true, "ok".to_owned()),
        Ok(body) => (false, format!("unexpected body: {body:?}")),
        Err(err) => (false, format!("{err:#}")),
    }
}

/// Poll /health until ready (up to `timeout`), with periodic diagnostics.
#[allow(dead_code)]
fn wait_for_health(host: &str, port: u16, timeout: Duration) -> bool {
    let start = Instant::now();
    let deadline = start + timeout;
    let mut attempt = 0u32;
    let mut last_detail = String::new();
    while Instant::now() < deadline {
        attempt += 1;
        let (ok, detail) = health_check_detail(host, port);
        if ok { return true; }
        // Print diagnostic every 5 seconds (every 20th poll at 250ms interval).
        if attempt <= 3 || attempt % 20 == 0 || detail != last_detail {
            let elapsed = start.elapsed().as_secs();
            eprintln!(
                "    [health poll #{attempt}, +{elapsed}s] {host}:{port} → {detail}"
            );
            last_detail = detail;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let elapsed = start.elapsed().as_secs();
    eprintln!(
        "    [health TIMEOUT after {attempt} attempts, {elapsed}s] last: {last_detail}"
    );
    false
}

/// Poll until /health stops responding (up to `timeout`).
fn wait_for_shutdown(host: &str, port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !health_ok(host, port) { return true; }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// MCP initialize handshake via HTTP POST /mcp.
fn mcp_initialize_http(host: &str, port: u16) -> Result<Value> {
    let body = serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": { "roots": { "listChanged": true } },
            "clientInfo": { "name": "mcp-readiness-http", "version": "0.1.0" }
        }
    }))?;
    let (status, resp_body) = http_post(host, port, "/mcp", &body)?;
    if status == 0 { bail!("No HTTP response from /mcp"); }
    // The response is SSE (text/event-stream).  Each event is:
    //   data: <optional payload>\n\n
    // There may be multiple events — the MCP initialize response JSON
    // is in a `data: {"jsonrpc":...}` line.  Find the first `data:` line
    // that starts with `{` (i.e. contains JSON).
    let json_str = resp_body.lines()
        .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
        .find(|payload| payload.starts_with('{'))
        .unwrap_or(&resp_body);
    let parsed: Value = serde_json::from_str(json_str)
        .with_context(|| format!("bad JSON from /mcp (status={status}): {resp_body}"))?;
    Ok(parsed)
}

/// Send an MCP `tools/call` request for `uffs.status` via HTTP.
///
/// This exercises the full daemon round-trip: HTTP gateway → MCP handler
/// → daemon RPC → response.  Returns the parsed JSON response.
/// Send an MCP `tools/call` request for `uffs.status` via HTTP.
///
/// This exercises the full daemon round-trip: HTTP gateway → MCP handler
/// → daemon RPC → response.  Performs a proper MCP session handshake:
///   1. POST `initialize` → extract `Mcp-Session-Id` from response header
///   2. POST `notifications/initialized` with session header
///   3. POST `tools/call` with session header → return result
fn mcp_tool_call_status(host: &str, port: u16) -> Result<Value> {
    // Step 1: Initialize — get session ID.
    let init_body = serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "mcp-readiness-tool", "version": "0.1.0" }
        }
    }))?;
    let init_resp = http_post_full(host, port, "/mcp", &init_body, &[])?;
    if init_resp.status == 0 { bail!("No HTTP response from /mcp init"); }
    let session_id = init_resp.header("mcp-session-id")
        .ok_or_else(|| anyhow::anyhow!(
            "No Mcp-Session-Id header in initialize response (status={})\nHeaders:\n{}",
            init_resp.status, init_resp.headers
        ))?;

    // Step 2: Send `initialized` notification.
    let notif_body = serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }))?;
    let _ = http_post_full(
        host, port, "/mcp", &notif_body,
        &[("Mcp-Session-Id", &session_id)],
    )?;

    // Step 3: tools/call with session header.
    let tool_body = serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "uffs_status",
            "arguments": {}
        }
    }))?;
    let tool_resp = http_post_full(
        host, port, "/mcp", &tool_body,
        &[("Mcp-Session-Id", &session_id)],
    )?;
    if tool_resp.status == 0 { bail!("No HTTP response from tools/call"); }
    // Parse the SSE data line containing JSON.
    let json_str = tool_resp.body.lines()
        .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
        .find(|payload| payload.starts_with('{'))
        .unwrap_or(&tool_resp.body);
    let parsed: Value = serde_json::from_str(json_str)
        .with_context(|| format!(
            "bad JSON from tools/call (status={}): {}",
            tool_resp.status, tool_resp.body
        ))?;
    Ok(parsed)
}

// ── Test runner ─────────────────────────────────────────────────────────────

struct Runner {
    binary: String,
    source_flag: Option<&'static str>,
    source_path: String,
    port: u16,
    host: String,
    passed: u32,
    failed: u32,
    timings: Vec<(String, u128)>,
}

impl Runner {
    fn new(binary: String, flag: Option<&'static str>, path: String, port: u16) -> Self {
        Self {
            binary, source_flag: flag, source_path: path,
            port, host: "127.0.0.1".to_owned(),
            passed: 0, failed: 0, timings: vec![],
        }
    }

    fn source_args(&self) -> Vec<&str> {
        match self.source_flag {
            Some(f) => vec![f, &self.source_path],
            None => vec![],
        }
    }

    /// Start the MCP HTTP server via `uffs mcp start`.
    ///
    /// `uffs mcp start` already handles:
    ///   1. Auto-starting the daemon if needed
    ///   2. Spawning `uffs mcp serve` as a background process
    ///   3. Polling /health until the server is ready
    ///   4. Exiting with code 0 on success, non-zero on failure
    ///
    /// We simply run it with `.status()` (inherits console stdio — no
    /// pipe handle inheritance issues on Windows) and check the exit code.
    fn mcp_start(&self) -> Result<String> {
        let port_str = self.port.to_string();
        let mut args: Vec<&str> = vec!["mcp", "start", "--port", &port_str, "--bind", &self.host];
        args.extend(self.source_args());

        eprintln!("    [mcp_start] {} {}", self.binary, args.join(" "));

        let status = Command::new(&self.binary)
            .args(&args)
            .status()
            .with_context(|| format!("exec: {} {}", self.binary, args.join(" ")))?;

        if !status.success() {
            bail!(
                "`uffs mcp start` exited with {status}\n\
                 Run manually with logging:\n  \
                 UFFS_LOG=debug UFFS_LOG_FILE=/tmp/mcp.log {} {}",
                self.binary, args.join(" ")
            );
        }

        // Quick sanity: confirm /health is actually up.
        let (ok, detail) = health_check_detail(&self.host, self.port);
        if !ok {
            bail!(
                "`uffs mcp start` exited OK but /health check failed: {detail}"
            );
        }

        Ok(format!("healthy on {}:{}", self.host, self.port))
    }

    fn step(&mut self, name: &str, f: impl FnOnce(&mut Self) -> Result<String>) {
        if self.failed > 0 { return; }
        println!("  {name}");
        let t0 = Instant::now();
        match f(self) {
            Ok(detail) => {
                let ms = t0.elapsed().as_millis();
                let tag = if detail.is_empty() { String::new() } else { format!(" — {detail}") };
                println!("    ↳ {} ({ms}ms){tag}", "PASSED".green().bold());
                self.passed += 1;
                self.timings.push((name.to_owned(), ms));
            }
            Err(err) => {
                println!("    ↳ {}: {err:#}", "FAILED".red().bold());
                self.failed += 1;
            }
        }
    }

    /// Run a `uffs` CLI command, return stdout.
    fn run_ok(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(&self.binary).args(args).output()
            .with_context(|| format!("exec: {} {}", self.binary, args.join(" ")))?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Run a `uffs` CLI command, return combined stdout+stderr and exit code.
    fn run_full(&self, args: &[&str]) -> Result<(String, bool)> {
        let out = Command::new(&self.binary).args(args).output()
            .with_context(|| format!("exec: {} {}", self.binary, args.join(" ")))?;
        let mut combined = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.is_empty() {
            combined.push('\n');
            combined.push_str(&stderr);
        }
        Ok((combined, out.status.success()))
    }

    fn ensure_daemon_stopped(&self) {
        let _ = self.run_ok(&["daemon", "kill"]);
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(500));
            let status = self.run_ok(&["daemon", "status"]).unwrap_or_default().to_lowercase();
            if status.contains("not running") { return; }
        }
    }

    fn ensure_daemon_running(&self) -> Result<String> {
        let mut args: Vec<&str> = vec!["daemon", "start"];
        args.extend(self.source_args());
        self.run_ok(&args)
    }

    fn mcp_status(&self) -> Result<String> { self.run_ok(&["mcp", "status"]) }
    fn mcp_stop(&self) -> Result<String> { self.run_ok(&["mcp", "stop"]) }
    fn mcp_kill(&self) -> Result<String> {
        let port_str = self.port.to_string();
        self.run_ok(&["mcp", "kill", "--port", &port_str, "--bind", &self.host])
    }
    fn daemon_kill(&self) -> Result<String> { self.run_ok(&["daemon", "kill"]) }

    fn daemon_status_text(&self) -> String {
        self.run_ok(&["daemon", "status"]).unwrap_or_default()
    }

    fn is_daemon_running(&self) -> bool {
        let out = self.daemon_status_text().to_lowercase();
        // `daemon status` prints "Daemon PID: ..." when running,
        // or "Daemon is not running." when not.
        (out.contains("daemon pid") || out.contains("ready") || out.contains("loading"))
            && !out.contains("not running")
    }

    /// Run `uffs daemon load` with given args, return stdout.
    fn daemon_load(&self, extra_args: &[&str]) -> Result<String> {
        let mut args = vec!["daemon", "load"];
        args.extend_from_slice(extra_args);
        self.run_ok(&args)
    }

    /// Kill everything — daemon + MCP gateway.
    fn kill_all(&self) {
        let _ = self.mcp_kill();
        let _ = self.daemon_kill();
        self.ensure_daemon_stopped();
    }
}

// ── Scenario A: Clean HTTP lifecycle ───────────────────────────────────────

fn scenario_a(r: &mut Runner) {
    println!("\n{}", "── Scenario A: Clean HTTP lifecycle ──".cyan().bold());

    r.step("A1  Kill stale MCP + daemon", |r| {
        r.kill_all();
        Ok(String::new())
    });

    r.step("A2  Start daemon", |r| { r.ensure_daemon_running()?; Ok(String::new()) });

    r.step("A3  Start MCP HTTP server", |r| {
        r.mcp_start()?;
        Ok(format!("listening on {}:{}", r.host, r.port))
    });

    r.step("A4  /health returns 'ok'", |r| {
        let body = http_get(&r.host, r.port, "/health")?;
        if body != "ok" { bail!("expected 'ok', got: {body}"); }
        Ok(String::new())
    });

    r.step("A5  /status returns JSON", |r| {
        let body = http_get(&r.host, r.port, "/status")?;
        let json: Value = serde_json::from_str(&body)
            .with_context(|| format!("bad JSON from /status: {body}"))?;
        if json["status"] != "running" { bail!("expected status=running: {json}"); }
        Ok(format!("uptime={}s", json["uptime_secs"]))
    });

    r.step("A6  MCP initialize via HTTP POST /mcp", |r| {
        let resp = mcp_initialize_http(&r.host, r.port)?;
        let version = resp.get("result")
            .and_then(|r| r.get("protocolVersion"))
            .and_then(Value::as_str)
            .unwrap_or("?");
        if version != "2024-11-05" { bail!("unexpected protocol: {version}"); }
        Ok(format!("protocol={version}"))
    });

    r.step("A7  Stop MCP HTTP server", |r| {
        r.mcp_stop()?;
        if !wait_for_shutdown(&r.host, r.port, Duration::from_secs(10)) {
            bail!("server still responding after stop");
        }
        Ok(String::new())
    });
}

// ── Scenario B: Daemon already running → fast HTTP start ─────────────────

fn scenario_b(r: &mut Runner) {
    println!("\n{}", "── Scenario B: Daemon already running → fast start ──".cyan().bold());

    r.step("B0  Ensure daemon running", |r| { r.ensure_daemon_running()?; Ok(String::new()) });

    r.step("B1  Start MCP HTTP (daemon warm)", |r| {
        let t0 = Instant::now();
        r.mcp_start()?;
        let ms = t0.elapsed().as_millis();
        Ok(format!("started in {ms}ms"))
    });

    r.step("B2  /health responsive", |r| {
        if !health_ok(&r.host, r.port) { bail!("/health not ok"); }
        Ok(String::new())
    });

    r.step("B3  Stop", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario C: Daemon NOT running → auto-start ─────────────────────────

fn scenario_c(r: &mut Runner) {
    println!("\n{}", "── Scenario C: Daemon NOT running → auto-start ──".cyan().bold());

    r.step("C1  Kill daemon + MCP", |r| {
        r.kill_all();
        if r.is_daemon_running() { bail!("daemon still running"); }
        Ok(String::new())
    });

    r.step("C2  Start MCP HTTP (should auto-start daemon)", |r| {
        let t0 = Instant::now();
        r.mcp_start()?;
        let ms = t0.elapsed().as_millis();
        Ok(format!("started in {ms}ms (includes daemon auto-start)"))
    });

    r.step("C3  MCP initialize works (proves daemon alive)", |r| {
        let resp = mcp_initialize_http(&r.host, r.port)?;
        let has_result = resp.get("result").is_some();
        if !has_result { bail!("no result in initialize response: {resp}"); }
        Ok(String::new())
    });

    r.step("C4  Verify daemon now running", |r| {
        let status = r.run_ok(&["daemon", "status"]).unwrap_or_default();
        if !status.to_lowercase().contains("ready")
            && !status.to_lowercase().contains("running")
            && !status.to_lowercase().contains("pid") {
            bail!("daemon not running after MCP auto-start: {status}");
        }
        Ok(String::new())
    });

    r.step("C5  Stop MCP", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario D: MCP status / stop / kill CLI ────────────────────────────────

fn scenario_d(r: &mut Runner) {
    println!("\n{}", "── Scenario D: MCP management CLI ──".cyan().bold());

    r.step("D1  `mcp status` when not running", |r| {
        let _ = r.mcp_kill();
        std::thread::sleep(Duration::from_millis(300));
        let out = r.mcp_status()?;
        if !out.to_lowercase().contains("not running") { bail!("expected 'not running': {out}"); }
        Ok(String::new())
    });

    r.step("D2  Start → `mcp status` shows running + transport", |r| {
        r.mcp_start()?;
        let out = r.mcp_status()?;
        let lower = out.to_lowercase();
        if !lower.contains("running") { bail!("expected running: {out}"); }
        if !lower.contains("http") { bail!("expected http transport: {out}"); }
        Ok(out.trim().to_owned())
    });

    r.step("D3  `mcp status` shows health ✓", |r| {
        let out = r.mcp_status()?;
        if !out.contains("✓") && !out.to_lowercase().contains("health") {
            bail!("expected health check in status: {out}");
        }
        Ok(String::new())
    });

    r.step("D4  `mcp stop`", |r| {
        r.mcp_stop()?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(String::new())
    });

    r.step("D5  `mcp kill` when not running", |r| {
        let out = r.mcp_kill()?;
        if out.to_lowercase().contains("error") { bail!("unexpected error: {out}"); }
        Ok(String::new())
    });
}

// ── Scenario E: Double start (idempotent) — gateway ✓ + daemon ✓ ────────────

fn scenario_e(r: &mut Runner) {
    println!("\n{}", "── Scenario E: Double start (idempotent) — gateway ✓ + daemon ✓ ──".cyan().bold());

    r.step("E1  Start MCP HTTP", |r| {
        r.mcp_start()?;
        Ok(String::new())
    });

    r.step("E2  Second `mcp start` → already running (gateway ✓, daemon ✓)", |r| {
        let port_str = r.port.to_string();
        let mut args: Vec<&str> = vec!["mcp", "start", "--port", &port_str, "--bind", &r.host];
        args.extend(r.source_args());
        let out = r.run_ok(&args)?;
        let lower = out.to_lowercase();
        if !lower.contains("already running") { bail!("expected 'already running': {out}"); }
        // Should report both gateway ✓ and daemon ✓.
        if !out.contains("gateway") || !out.contains("daemon") {
            bail!("expected gateway ✓ + daemon ✓ in output: {out}");
        }
        Ok(out.trim().to_owned())
    });

    r.step("E3  Server still healthy", |r| {
        if !health_ok(&r.host, r.port) { bail!("/health failed after double start"); }
        Ok(String::new())
    });

    r.step("E4  Daemon untouched (still running)", |r| {
        if !r.is_daemon_running() { bail!("daemon should still be running"); }
        Ok(String::new())
    });

    r.step("E5  Stop", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario F: Hard kill → recovery ────────────────────────────────────────

fn scenario_f(r: &mut Runner) {
    println!("\n{}", "── Scenario F: Hard kill → recovery ──".cyan().bold());

    r.step("F1  Start MCP HTTP", |r| { r.mcp_start()?; Ok(String::new()) });

    r.step("F2  `mcp kill`", |r| {
        r.mcp_kill()?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(String::new())
    });

    r.step("F3  Status → not running", |r| {
        let out = r.mcp_status()?;
        if !out.to_lowercase().contains("not running") { bail!("expected not running: {out}"); }
        Ok(String::new())
    });

    r.step("F4  Restart MCP HTTP after kill", |r| {
        r.mcp_start()?;
        if !health_ok(&r.host, r.port) { bail!("/health failed"); }
        Ok(String::new())
    });

    r.step("F5  Stop", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario G: Daemon killed → `mcp start` restarts daemon only ────────────

fn scenario_g(r: &mut Runner) {
    println!("\n{}", "── Scenario G: Daemon killed → mcp start restarts daemon only ──".cyan().bold());

    r.step("G1  Ensure daemon + MCP HTTP running", |r| {
        r.ensure_daemon_running()?;
        r.mcp_start()?;
        if !health_ok(&r.host, r.port) { bail!("/health not ok"); }
        Ok(String::new())
    });

    r.step("G2  Kill daemon (leave gateway alive)", |r| {
        r.daemon_kill()?;
        r.ensure_daemon_stopped();
        if !health_ok(&r.host, r.port) { bail!("gateway died when daemon was killed"); }
        Ok("gateway still alive, daemon dead".to_owned())
    });

    r.step("G3  `mcp start` detects daemon dead → restarts daemon only", |r| {
        let port_str = r.port.to_string();
        let mut args: Vec<&str> = vec!["mcp", "start", "--port", &port_str, "--bind", &r.host];
        args.extend(r.source_args());
        let out = r.run_ok(&args)?;
        let lower = out.to_lowercase();
        // Should NOT say "killing" or "recycling" the gateway — only daemon restart.
        if lower.contains("killing") { bail!("gateway was killed — should only restart daemon: {out}"); }
        if !lower.contains("daemon") { bail!("expected daemon restart message: {out}"); }
        Ok(out.trim().to_owned())
    });

    r.step("G4  Gateway still healthy on same port", |r| {
        if !health_ok(&r.host, r.port) { bail!("/health failed"); }
        Ok(String::new())
    });

    r.step("G5  Daemon now running", |r| {
        if !r.is_daemon_running() { bail!("daemon not running after restart"); }
        Ok(String::new())
    });

    r.step("G6  MCP initialize works (proves full stack healthy)", |r| {
        let resp = mcp_initialize_http(&r.host, r.port)?;
        if resp.get("result").is_none() { bail!("no result: {resp}"); }
        Ok(String::new())
    });

    r.step("G7  Stop", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario H: HTTP /mcp endpoint (MCP initialize via HTTP) ────────────────

fn scenario_h(r: &mut Runner) {
    println!("\n{}", "── Scenario H: HTTP /mcp endpoint ──".cyan().bold());

    r.step("H1  Ensure MCP HTTP running", |r| { r.mcp_start()?; Ok(String::new()) });

    r.step("H2  POST /mcp → MCP initialize", |r| {
        let resp = mcp_initialize_http(&r.host, r.port)?;
        let version = resp.get("result")
            .and_then(|r| r.get("protocolVersion"))
            .and_then(Value::as_str);
        Ok(format!("protocol={}", version.unwrap_or("?")))
    });

    r.step("H3  POST /mcp → invalid JSON → error response", |r| {
        let (status, _body) = http_post(&r.host, r.port, "/mcp", "not json")?;
        if status == 200 { bail!("expected error status, got 200"); }
        Ok(format!("status={status}"))
    });

    r.step("H4  Stop", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario I: MCP HTTP startup timing ──────────────────────────────────────

fn scenario_i(r: &mut Runner) {
    println!("\n{}", "── Scenario I: MCP HTTP startup timing ──".cyan().bold());

    // Ensure daemon is warm.
    let _ = r.mcp_kill();
    let _ = r.ensure_daemon_running();
    std::thread::sleep(Duration::from_secs(1));

    r.step("I1  Cold HTTP start (daemon warm)", |r| {
        let t0 = Instant::now();
        r.mcp_start()?;
        let ms = t0.elapsed().as_millis();
        r.mcp_stop()?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(format!("{ms}ms"))
    });

    r.step("I2  Second HTTP start", |r| {
        let t0 = Instant::now();
        r.mcp_start()?;
        let ms = t0.elapsed().as_millis();
        r.mcp_stop()?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(format!("{ms}ms"))
    });

    r.step("I3  Third HTTP start", |r| {
        let t0 = Instant::now();
        r.mcp_start()?;
        let ms = t0.elapsed().as_millis();
        r.mcp_stop()?;
        Ok(format!("{ms}ms"))
    });
}

// ── Scenario J: Stale port occupant → kill and start fresh ──────────────────

fn scenario_j(r: &mut Runner) {
    println!("\n{}", "── Scenario J: Stale port occupant → kill and start fresh ──".cyan().bold());

    r.step("J1  Start MCP HTTP normally", |r| {
        r.kill_all();
        r.mcp_start()?;
        Ok(String::new())
    });

    r.step("J2  Remove PID file (simulate stale process)", |r| {
        // Delete the PID file but leave the gateway process alive.
        let _ = std::fs::remove_file(&mcp_pid_file_path());
        // Verify gateway is still alive (port occupied, no PID file).
        if !health_ok(&r.host, r.port) { bail!("gateway died when PID file was removed"); }
        let status = r.mcp_status()?;
        if !status.to_lowercase().contains("not running") {
            bail!("expected 'not running' with PID file gone: {status}");
        }
        Ok("gateway alive, PID file gone".to_owned())
    });

    r.step("J3  `mcp start` sees healthy stack → reports already running", |r| {
        let port_str = r.port.to_string();
        let mut args: Vec<&str> = vec!["mcp", "start", "--port", &port_str, "--bind", &r.host];
        args.extend(r.source_args());
        let out = r.run_ok(&args)?;
        // Stack is healthy (gateway ✓ + daemon ✓), even without PID file.
        if !out.to_lowercase().contains("already running") {
            bail!("expected 'already running': {out}");
        }
        if !health_ok(&r.host, r.port) { bail!("/health failed"); }
        Ok(out.trim().to_owned())
    });

    r.step("J4  Full stack healthy", |r| {
        if !health_ok(&r.host, r.port) { bail!("/health not ok"); }
        if !r.is_daemon_running() { bail!("daemon not running"); }
        Ok(String::new())
    });

    r.step("J5  Cleanup (kill_all since no PID file)", |r| {
        r.kill_all();
        Ok(String::new())
    });
}

// ── Scenario K: `mcp kill` cleans up stale port processes ───────────────────

fn scenario_k(r: &mut Runner) {
    println!("\n{}", "── Scenario K: mcp kill cleans up stale port processes ──".cyan().bold());

    r.step("K1  Start MCP HTTP (clean)", |r| {
        r.kill_all();
        r.mcp_start()?;
        // Verify PID file exists so mcp_kill can find it.
        if !mcp_pid_file_path().exists() { bail!("PID file not created"); }
        Ok(String::new())
    });

    r.step("K2  `mcp kill` stops gateway", |r| {
        r.mcp_kill()?;
        std::thread::sleep(Duration::from_millis(500));
        if health_ok(&r.host, r.port) { bail!("gateway still alive after mcp kill"); }
        Ok(String::new())
    });

    r.step("K3  Port is free", |r| {
        let addr = format!("{}:{}", r.host, r.port);
        if TcpStream::connect(&addr).is_ok() { bail!("port still occupied"); }
        Ok(String::new())
    });

    r.step("K4  Can start fresh after kill", |r| {
        r.mcp_start()?;
        if !health_ok(&r.host, r.port) { bail!("/health not ok"); }
        Ok(String::new())
    });

    r.step("K5  Stop", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario L: Daemon killed → tool call auto-reconnects ───────────────────

fn scenario_l(r: &mut Runner) {
    println!("\n{}", "── Scenario L: Daemon killed → tool call auto-reconnects ──".cyan().bold());

    r.step("L1  Start full stack", |r| {
        r.kill_all();
        r.mcp_start()?;
        if !r.is_daemon_running() { bail!("daemon not running"); }
        Ok(String::new())
    });

    r.step("L2  MCP tool call works (warm daemon)", |r| {
        let resp = mcp_tool_call_status(&r.host, r.port)?;
        // A successful tool call returns {"result": {"content": [...]}}
        // or an error.  Any valid JSON response proves the round-trip.
        if resp.get("error").is_some() { bail!("tool call error: {resp}"); }
        Ok(String::new())
    });

    r.step("L3  Kill daemon (leave gateway alive)", |r| {
        r.daemon_kill()?;
        r.ensure_daemon_stopped();
        if !health_ok(&r.host, r.port) { bail!("gateway died when daemon was killed"); }
        Ok(String::new())
    });

    r.step("L4  Tool call auto-reconnects (daemon restarted by handler)", |r| {
        // The MCP handler's lazy `ClientSlot` should detect the broken
        // pipe, clear its cached connection, reconnect (auto-starting
        // the daemon), and retry the tool call — all transparently.
        let resp = mcp_tool_call_status(&r.host, r.port)?;
        if resp.get("error").is_some() {
            bail!("tool call should have auto-reconnected: {resp}");
        }
        Ok(String::new())
    });

    r.step("L5  Daemon is now running again", |r| {
        if !r.is_daemon_running() { bail!("daemon not restarted by tool call"); }
        Ok(String::new())
    });

    r.step("L6  `mcp start` sees healthy stack", |r| {
        let port_str = r.port.to_string();
        let mut args: Vec<&str> = vec!["mcp", "start", "--port", &port_str, "--bind", &r.host];
        args.extend(r.source_args());
        let out = r.run_ok(&args)?;
        if !out.to_lowercase().contains("already running") {
            bail!("expected 'already running': {out}");
        }
        Ok(out.trim().to_owned())
    });

    r.step("L7  Stop", |r| { r.mcp_stop()?; Ok(String::new()) });
}

// ── Scenario M: Daemon load — hot-load MFT into running daemon ─────────────

/// Discover `drive_*` subdirectories in a data-dir, returning
/// `(letter, mft_file_path)` pairs sorted by letter.
fn discover_drive_mft_files(data_dir: &str) -> Vec<(char, String)> {
    let dir = std::path::Path::new(data_dir);
    if !dir.is_dir() { return vec![]; }

    const PRIORITY: &[&str] = &["iocp", "uffs", "bin", "raw", "mft"];

    let mut results: Vec<(char, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let letter = name.strip_prefix("drive_")?.chars().next()?;
            if !letter.is_ascii_alphabetic() || !entry.path().is_dir() {
                return None;
            }
            // Find best MFT file by extension priority.
            let files: Vec<std::path::PathBuf> = std::fs::read_dir(entry.path())
                .ok()?
                .flatten()
                .map(|fe| fe.path())
                .filter(|fp| fp.is_file())
                .collect();
            for ext in PRIORITY {
                if let Some(path) = files.iter().find(|fp| {
                    fp.extension()
                        .and_then(|os| os.to_str())
                        .is_some_and(|fe| fe.eq_ignore_ascii_case(ext))
                }) {
                    return Some((
                        letter.to_ascii_uppercase(),
                        path.to_string_lossy().into_owned(),
                    ));
                }
            }
            None
        })
        .collect();
    results.sort_by_key(|(letter, _)| *letter);
    results
}

/// Parse drive count and individual drive letters from `uffs daemon status`.
///
/// The status output format is:
/// ```text
///   Drives:      7 loaded (25,846,853 records)
///     C:  3,428,455 records
///     D:  7,065,539 records
/// ```
///
/// Returns `(count_from_header, vec_of_drive_letters)`.
fn parse_drives_from_status(status_output: &str) -> (usize, Vec<char>) {
    let mut header_count: usize = 0;
    let mut letters: Vec<char> = Vec::new();

    for line in status_output.lines() {
        let trimmed = line.trim();

        // Parse "Drives:      N loaded ..." header.
        if let Some(rest) = trimmed.strip_prefix("Drives:") {
            let rest = rest.trim();
            if let Some(num_str) = rest.split_whitespace().next() {
                if let Ok(count) = num_str.parse::<usize>() {
                    header_count = count;
                }
            }
            continue;
        }

        // Parse individual drive lines like "  C: —  3,428,455 records".
        // After trimming: starts with a single letter, then `:`.
        if trimmed.len() >= 2 {
            let mut chars = trimmed.chars();
            if let Some(letter) = chars.next() {
                if letter.is_ascii_alphabetic() && chars.next() == Some(':') {
                    letters.push(letter.to_ascii_uppercase());
                }
            }
        }
    }

    // Prefer the counted letters; fall back to header count.
    if letters.is_empty() && header_count > 0 {
        (header_count, letters)
    } else {
        (letters.len(), letters)
    }
}

fn scenario_m(r: &mut Runner) {
    println!("\n{}", "── Scenario M: Daemon load — hot-load MFT ──".cyan().bold());

    // Discover available drives for incremental load testing.
    let drive_mft_files = if r.source_flag == Some("--data-dir") {
        discover_drive_mft_files(&r.source_path)
    } else {
        vec![]
    };
    let can_test_incremental = drive_mft_files.len() >= 2;

    if can_test_incremental {
        let (first, first_mft) = &drive_mft_files[0];
        let (second, second_mft) = &drive_mft_files[1];
        let first = *first;
        let second = *second;
        let first_mft = first_mft.clone();
        let second_mft = second_mft.clone();

        println!(
            "    (found {} drives in data-dir: {} — will test incremental load: {} then {})",
            drive_mft_files.len(),
            drive_mft_files.iter().map(|(ch, _)| format!("{ch}:")).collect::<Vec<_>>().join(", "),
            first, second,
        );

        // M1: Kill everything, start daemon with FIRST drive only (explicit --mft-file).
        r.step("M1  Start daemon with single drive", |r| {
            r.kill_all();
            let args = vec!["daemon", "start", "--mft-file", &first_mft];
            r.run_ok(&args)?;
            if !r.is_daemon_running() { bail!("daemon not running"); }
            Ok(format!("started with {first}: via --mft-file"))
        });

        // M2: Verify exactly 1 drive loaded.
        r.step("M2  Verify exactly 1 drive loaded", |r| {
            let status = r.daemon_status_text();
            let (count, letters) = parse_drives_from_status(&status);
            if count != 1 {
                bail!(
                    "expected exactly 1 drive, got {count} (letters: {})\n{status}",
                    letters.iter().map(|ch| format!("{ch}:")).collect::<Vec<_>>().join(", ")
                );
            }
            if !letters.is_empty() && !letters.contains(&first) {
                bail!("expected drive {first}, found {:?}: {status}", letters);
            }
            if letters.contains(&second) {
                bail!("drive {second} already loaded — expected only {first}: {status}");
            }
            Ok(format!("exactly 1 drive: {first}:"))
        });

        // M3: Hot-load second drive via --mft-file.
        r.step("M3  Hot-load second drive", |r| {
            let out = r.daemon_load(&["--mft-file", &second_mft])?;
            let lower = out.to_lowercase();
            if lower.contains("error") && !lower.contains("already") {
                bail!("load failed: {out}");
            }
            // Should report the second drive as newly loaded (not "already loaded").
            if lower.contains("already loaded") {
                bail!("second drive was already loaded — should be new: {out}");
            }
            if !lower.contains("loaded") {
                bail!("expected load confirmation: {out}");
            }
            Ok(format!("loaded drive {second}"))
        });

        // M4: Verify exactly 2 drives now loaded.
        r.step("M4  Verify exactly 2 drives loaded", |r| {
            let status = r.daemon_status_text();
            let (count, letters) = parse_drives_from_status(&status);
            if count != 2 {
                bail!(
                    "expected 2 drives, got {count} (letters: {})\n{status}",
                    letters.iter().map(|ch| format!("{ch}:")).collect::<Vec<_>>().join(", ")
                );
            }
            if !letters.contains(&first) {
                bail!("status missing drive {first}: {status}");
            }
            if !letters.contains(&second) {
                bail!("status missing drive {second}: {status}");
            }
            Ok(format!("exactly 2 drives: {first}: + {second}:"))
        });

        // M5: Search across both drives to prove unified index.
        r.step("M5  Search spans both drives", |r| {
            let out = r.run_ok(&["*", "--limit", "5"])?;
            if out.trim().is_empty() { bail!("search returned nothing"); }
            Ok(format!("{} result line(s)", out.lines().count()))
        });

        // M6: Reload second drive → "already loaded" (idempotent).
        r.step("M6  Reload same drive → already loaded", |r| {
            let out = r.daemon_load(&["--mft-file", &second_mft])?;
            let lower = out.to_lowercase();
            if !lower.contains("already loaded") && !lower.contains("skipped") {
                bail!("expected 'already loaded': {out}");
            }
            Ok(String::new())
        });
    } else if r.source_flag == Some("--mft-file") {
        // Single MFT file: test idempotent reload.
        println!("    (source is single MFT file — testing idempotent reload only)");

        r.step("M1  Start daemon with MFT file", |r| {
            r.kill_all();
            r.ensure_daemon_running()?;
            if !r.is_daemon_running() { bail!("daemon not running"); }
            Ok(String::new())
        });

        r.step("M2  Reload same MFT → already loaded", |r| {
            let out = r.daemon_load(&r.source_args())?;
            let lower = out.to_lowercase();
            if lower.contains("error") && !lower.contains("already") {
                bail!("unexpected error: {out}");
            }
            if !lower.contains("already loaded") && !lower.contains("skipped")
                && !lower.contains("loaded")
            {
                bail!("expected load confirmation: {out}");
            }
            Ok(out.lines().last().unwrap_or("").trim().to_owned())
        });
    } else {
        // Live NTFS (no --data-dir, no --mft-file): `daemon load` only supports
        // file-based sources, so skip idempotent reload and go straight to
        // error handling tests.
        println!("    (live NTFS mode — no file source to reload, testing error handling only)");

        r.step("M1  Start daemon (live drives)", |r| {
            r.kill_all();
            r.ensure_daemon_running()?;
            if !r.is_daemon_running() { bail!("daemon not running"); }
            Ok(String::new())
        });
    }

    // Error handling tests — always run regardless of incremental capability.

    // M7: `daemon load` with no args → informative error.
    r.step("M7  `daemon load` no args → error message", |r| {
        let (out, success) = r.run_full(&["daemon", "load"])?;
        let lower = out.to_lowercase();
        if success {
            bail!("expected non-zero exit, but command succeeded: {out}");
        }
        if !lower.contains("nothing to load") && !lower.contains("provide")
            && !lower.contains("mft-file") {
            bail!("expected usage hint: {out}");
        }
        Ok(String::new())
    });

    // M8: `daemon load --mft-file <bogus>` → error reported (not a crash).
    r.step("M8  `daemon load --mft-file /nonexistent` → daemon survives", |r| {
        let (out, _success) = r.run_full(&["daemon", "load", "--mft-file", "/nonexistent/fake.bin"])?;
        if !r.is_daemon_running() {
            bail!("daemon crashed after bad load: {out}");
        }
        Ok(out.lines().filter(|l| !l.is_empty()).last().unwrap_or("").trim().to_owned())
    });

    // M9: Final health check — search still works.
    r.step("M9  Daemon still healthy after all loads", |r| {
        if !r.is_daemon_running() { bail!("daemon not running"); }
        let out = r.run_ok(&["*", "--limit", "1"])?;
        if out.trim().is_empty() { bail!("search returned nothing — index may be broken"); }
        Ok(String::new())
    });

    // M10: Cleanup.
    r.step("M10 Stop daemon", |r| {
        r.daemon_kill()?;
        r.ensure_daemon_stopped();
        Ok(String::new())
    });
}

// ── main ────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    let binary = cli.binary.unwrap_or_else(|| default_binary());
    let port = cli.port;

    println!("{}", "═══ UFFS MCP HTTP Server Lifecycle Verification ═══".bold());

    let (source_flag, source_path): (Option<&'static str>, String) = match &cli.path {
        Some(path) => {
            let (flag, val) = detect_data_source(path)?;
            (Some(flag), val)
        }
        None if cfg!(windows) => (None, String::new()),
        None => match default_data_dir() {
            Some(dir) => (Some("--data-dir"), dir),
            None => bail!(
                "PATH is required on non-Windows platforms.\n\n\
                 On macOS/Linux, provide a data directory or MFT file:\n  \
                 rust-script scripts/dev/mcp-readiness.rs ~/uffs_data\n  \
                 rust-script scripts/dev/mcp-readiness.rs /path/to/C_mft.iocp\n\n\
                 On Windows, omit PATH to auto-discover live NTFS drives."
            ),
        },
    };

    // ── Preflight: verify companion binaries exist ──────────────────
    // `uffs mcp *` delegates to the standalone `uffsmcp` binary.
    // Fail immediately with a clear message instead of waiting 3.5 min
    // for a health timeout.
    {
        let uffs_path = std::path::Path::new(&binary);
        let mcp_name = if cfg!(windows) { "uffsmcp.exe" } else { "uffsmcp" };
        let mcp_path = uffs_path.parent()
            .map(|dir| dir.join(mcp_name))
            .unwrap_or_else(|| std::path::PathBuf::from(mcp_name));
        if !mcp_path.exists() {
            bail!(
                "Companion binary `{mcp_name}` not found at {}\n\
                 `uffs mcp *` delegates to `{mcp_name}` which must sit alongside `uffs`.\n\
                 Rebuild and deploy: just build-local   (or: just use-local)",
                mcp_path.display(),
            );
        }
    }

    println!("  binary:    {}", binary);
    println!("  port:      {}", port);
    match source_flag {
        Some(flag) => println!("  source:    {} {}", flag, source_path),
        None => println!("  source:    live NTFS drives (auto-discover)"),
    }

    let mut r = Runner::new(binary, source_flag, source_path, port);

    scenario_a(&mut r);
    scenario_b(&mut r);
    scenario_c(&mut r);
    scenario_d(&mut r);
    scenario_e(&mut r);
    scenario_f(&mut r);
    scenario_g(&mut r);
    scenario_h(&mut r);
    scenario_i(&mut r);
    scenario_j(&mut r);
    scenario_k(&mut r);
    scenario_l(&mut r);
    scenario_m(&mut r);

    // Cleanup.
    r.kill_all();

    // Summary.
    println!("\n{}", "── Summary ──".cyan().bold());
    println!(
        "  {} passed, {} failed",
        r.passed.to_string().green().bold(),
        r.failed.to_string().red().bold(),
    );
    if !r.timings.is_empty() {
        let slowest = r.timings.iter().max_by_key(|(_, ms)| *ms).unwrap();
        println!("  slowest: {} ({}ms)", slowest.0.dimmed(), slowest.1);
    }

    if r.failed > 0 {
        println!("\n{}", "MCP HTTP lifecycle check FAILED.".red().bold());
        std::process::exit(1);
    }
    println!("\n{}", "MCP HTTP server lifecycle OK. ✓".green().bold());
    Ok(())
}
