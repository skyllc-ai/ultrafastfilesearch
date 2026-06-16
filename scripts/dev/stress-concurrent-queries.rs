#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! serde = { version = "1.0", features = ["derive"] }
//! serde_json = "1.0"
//! dirs-next = "2.0"
//! uds_windows = "1.1"
//! ```
// =============================================================================
// scripts/dev/stress-concurrent-queries.rs — UFFS Concurrent Query Stress Test
// =============================================================================
//
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// Concurrent query stress test for the UFFS daemon.
//
// Ramps concurrency from 1 → max (default 128), measures per-query
// latency (p50/p95/p99/max), throughput (queries/sec), and detects the
// saturation point where average latency starts climbing.
//
// Requires a running daemon with loaded indices.  Does NOT start/stop it.
//
// Usage:
//   rust-script scripts/dev/stress-concurrent-queries.rs
//   rust-script scripts/dev/stress-concurrent-queries.rs --max-concurrency 64 --queries-per-level 200

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
use anyhow::{Result, bail};
use clap::Parser;
use colored::Colorize;

// ── Cross-platform Unix domain socket ────────────────────────────────
// macOS/Linux: std::os::unix::net::UnixStream  (stable)
// Windows:     uds_windows::UnixStream           (Win10 1803+, stable crate)

#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(windows)]
use uds_windows::UnixStream;

#[derive(Parser)]
#[command(name = "stress-concurrent-queries", about = "Concurrent query stress test")]
struct Cli {
    #[arg(long, default_value = "128")]
    max_concurrency: usize,
    #[arg(long, default_value = "100")]
    queries_per_level: usize,
    #[arg(long, default_value = "*.rs,*.dll,*.exe,*.txt,config")]
    patterns: String,
    #[arg(long, default_value = "50")]
    limit: u32,
    #[arg(long, default_value = "10")]
    warmup: usize,
    /// Data directory for daemon auto-start (macOS/Linux only).
    /// On Windows the daemon auto-discovers NTFS drives.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Path to the uffs binary.  Auto-detected from PATH or ./target/release/.
    #[arg(long)]
    uffs_bin: Option<PathBuf>,
}

fn socket_path() -> PathBuf {
    let base = dirs_next::data_local_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("uffs").join("daemon.sock")
}

/// Find the uffs binary: explicit flag → well-known locations → PATH → local build.
fn find_uffs_bin(explicit: &Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.exists() { return Some(p.clone()); }
    }
    // Well-known locations (matches cli-validation pattern).
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let candidates = [
        format!("{home}\\bin\\uffs.exe"),
        format!("{home}/bin/uffs.exe"),
        format!("{home}/bin/uffs"),
        "target/release/uffs.exe".to_string(),
        "target\\release\\uffs.exe".to_string(),
        "target/release/uffs".to_string(),
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return Some(PathBuf::from(c));
        }
    }
    // Check PATH
    let which_cmd = if cfg!(windows) { "where" } else { "which" };
    if let Ok(output) = std::process::Command::new(which_cmd).arg("uffs").output() {
        if output.status.success() {
            let p = String::from_utf8_lossy(&output.stdout).trim().lines().next().unwrap_or("").to_string();
            if !p.is_empty() { return Some(PathBuf::from(p)); }
        }
    }
    None
}

/// Check if daemon is running via `uffs --daemon status`.
fn daemon_running_via_cli(bin: &PathBuf) -> bool {
    if let Ok(out) = std::process::Command::new(bin).args(["--daemon", "status"]).output() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        stdout.contains("Ready") || stdout.contains("Loading")
    } else {
        false
    }
}

/// Start the daemon if not already running, wait for it to be ready.
fn ensure_daemon(sock: &PathBuf, cli: &Cli) -> Result<()> {
    let bin = find_uffs_bin(&cli.uffs_bin);

    // Already running? Check socket probe first, then CLI fallback.
    if sock.exists() {
        let probe = send_search(sock, 0, "*.txt", 1);
        if probe.ok { return Ok(()); }
        // Socket exists but probe failed — try CLI check (may be socket
        // type mismatch between uds_windows and std::os::windows::net).
        if let Some(ref b) = bin {
            if daemon_running_via_cli(b) {
                println!("  daemon:       {} (verified via CLI)\n", "RUNNING".green().bold());
                return Ok(());
            }
        }
    }

    let bin = bin.ok_or_else(|| anyhow::anyhow!(
        "Cannot find uffs binary.\n\
         Provide --uffs-bin <path>, add uffs to PATH, or build with: cargo build --release -p uffs-cli"
    ))?;
    println!("  uffs binary:  {}", bin.display());

    // Build args: `uffs --daemon start` blocks until "Daemon started and ready."
    let mut args = vec!["--daemon", "start"];
    let data_dir_str;
    if let Some(ref dir) = cli.data_dir {
        args.push("--data-dir");
        data_dir_str = dir.to_string_lossy().into_owned();
        args.push(&data_dir_str);
    }

    println!("  starting:     {} {}", bin.display(), args.join(" "));

    let t0 = Instant::now();
    let output = std::process::Command::new(&bin)
        .args(&args)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `uffs --daemon start`: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let elapsed = t0.elapsed();

    if !output.status.success() {
        bail!(
            "`uffs --daemon start` failed (exit {}):\nstdout: {}\nstderr: {}",
            output.status.code().unwrap_or(-1), stdout.trim(), stderr.trim()
        );
    }

    println!("  daemon:       {} ({:.1}s)\n", "READY".green().bold(), elapsed.as_secs_f64());
    Ok(())
}

#[derive(Debug, Clone)]
struct QueryResult {
    latency: Duration,
    rows: usize,
    ok: bool,
    error: Option<String>,
}

fn qerr(start: Instant, msg: String) -> QueryResult {
    QueryResult { latency: start.elapsed(), rows: 0, ok: false, error: Some(msg) }
}

fn send_search(sock_path: &PathBuf, id: u64, pattern: &str, limit: u32) -> QueryResult {
    let start = Instant::now();
    let mut stream = match UnixStream::connect(sock_path) {
        Ok(s) => s,
        Err(e) => return qerr(start, format!("connect: {e}")),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "search",
        "params": { "pattern": pattern, "limit": limit, "case_sensitive": false }
    });
    let mut msg = serde_json::to_string(&req).unwrap();
    msg.push('\n');
    if let Err(e) = stream.write_all(msg.as_bytes()) {
        return qerr(start, format!("write: {e}"));
    }
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Err(e) => return qerr(start, format!("read: {e}")),
            Ok(0) => return qerr(start, "eof".into()),
            Ok(_) => {}
        }
        if line.trim().is_empty() { continue; }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if v.get("id").is_some() {
                if let Some(err) = v.get("error") {
                    return qerr(start, format!("rpc: {err}"));
                }
                let r = v.get("result").cloned().unwrap_or(serde_json::Value::Null);
                let rows = r.get("rows").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                return QueryResult { latency: start.elapsed(), rows, ok: true, error: None };
            }
            // notification — read next line
        } else {
            return qerr(start, format!("json: {}", line.trim()));
        }
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() { return Duration::ZERO; }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[derive(Debug, Clone)]
struct LevelStats {
    concurrency: usize,
    successful: usize,
    failed: usize,
    p50: Duration,
    p95: Duration,
    p99: Duration,
    max: Duration,
    min: Duration,
    mean: Duration,
    wall_time: Duration,
    throughput_qps: f64,
}

fn compute_stats(c: usize, results: &[QueryResult], wall: Duration) -> LevelStats {
    let ok: Vec<_> = results.iter().filter(|r| r.ok).collect();
    let failed = results.len() - ok.len();
    let mut lat: Vec<Duration> = ok.iter().map(|r| r.latency).collect();
    lat.sort();
    let total: Duration = lat.iter().sum();
    let mean = if lat.is_empty() { Duration::ZERO } else { total / lat.len() as u32 };
    let qps = if wall.as_secs_f64() > 0.0 { ok.len() as f64 / wall.as_secs_f64() } else { 0.0 };
    LevelStats {
        concurrency: c, successful: ok.len(), failed,
        p50: percentile(&lat, 0.50), p95: percentile(&lat, 0.95),
        p99: percentile(&lat, 0.99), max: *lat.last().unwrap_or(&Duration::ZERO),
        min: *lat.first().unwrap_or(&Duration::ZERO), mean, wall_time: wall, throughput_qps: qps,
    }
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0)
}

fn run_level(
    c: usize,
    total: usize,
    patterns: &[String],
    limit: u32,
    warmup: usize,
    sock: &PathBuf,
) -> LevelStats {
    let sock = sock.clone();
    let pats = Arc::new(patterns.to_vec());
    // Warmup (serial).
    for i in 0..warmup {
        send_search(&sock, i as u64, &pats[i % pats.len()], limit);
    }
    let per = total / c;
    let rem = total % c;
    let barrier = Arc::new(Barrier::new(c));
    let t0 = Instant::now();
    let handles: Vec<_> = (0..c)
        .map(|w| {
            let s = sock.clone();
            let p = Arc::clone(&pats);
            let b = Arc::clone(&barrier);
            let n = per + if w < rem { 1 } else { 0 };
            std::thread::spawn(move || {
                b.wait();
                (0..n)
                    .map(|q| {
                        send_search(&s, (w * 10000 + q) as u64, &p[(w + q) % p.len()], limit)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut all = Vec::with_capacity(total);
    for h in handles {
        all.extend(h.join().unwrap_or_default());
    }
    compute_stats(c, &all, t0.elapsed())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sock = socket_path();
    let patterns: Vec<String> = cli.patterns.split(',').map(|s| s.trim().to_owned()).collect();

    println!("{}", "═══ UFFS Concurrent Query Stress Test ═══".bold());
    println!("  socket:      {}", sock.display());
    println!("  patterns:    {:?}", patterns);
    println!("  limit:       {}", cli.limit);
    println!("  per level:   {} queries ({} warmup)", cli.queries_per_level, cli.warmup);

    // Auto-start daemon if not running.
    ensure_daemon(&sock, &cli)?;

    let probe = send_search(&sock, 0, &patterns[0], cli.limit);
    if !probe.ok {
        bail!(
            "Daemon is running but search failed: {:?}",
            probe.error
        );
    }
    println!("  probe:       {} ({} rows)\n", "OK".green().bold(), probe.rows);

    // Concurrency levels: 1,2,4,8,16,32,64,128...
    let mut levels = Vec::new();
    let mut c = 1;
    while c <= cli.max_concurrency {
        levels.push(c);
        c *= 2;
    }
    if *levels.last().unwrap_or(&0) != cli.max_concurrency && cli.max_concurrency > 1 {
        levels.push(cli.max_concurrency);
    }

    let mut all_stats: Vec<LevelStats> = Vec::new();

    // Header
    println!(
        "{}",
        "─── Results ────────────────────────────────────────────────────────────────────────────────────────"
            .bold()
    );
    println!(
        "{:>5}  {:>6}  {:>6}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>6}",
        "conc", "ok", "fail", "min", "p50", "p95", "p99", "max", "mean", "qps", "wall"
    );
    println!("{}", "─".repeat(106));

    for &c in &levels {
        let stats = run_level(c, cli.queries_per_level, &patterns, cli.limit, cli.warmup, &sock);

        let qps_str = format!("{:.1}", stats.throughput_qps);
        let qps_colored = if stats.throughput_qps > 100.0 {
            qps_str.green().to_string()
        } else if stats.throughput_qps > 20.0 {
            qps_str.yellow().to_string()
        } else {
            qps_str.red().to_string()
        };

        println!(
            "{:>5}  {:>6}  {:>6}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>6}",
            c,
            stats.successful,
            stats.failed,
            fmt_ms(stats.min),
            fmt_ms(stats.p50),
            fmt_ms(stats.p95),
            fmt_ms(stats.p99),
            fmt_ms(stats.max),
            fmt_ms(stats.mean),
            qps_colored,
            format!("{:.1}s", stats.wall_time.as_secs_f64()),
        );
        all_stats.push(stats);
    }

    // ── Analysis ──────────────────────────────────────────────────────
    println!(
        "\n{}",
        "─── Analysis ────────────────────────────────────────────────────────────".bold()
    );

    // Peak throughput.
    if let Some(p) = all_stats
        .iter()
        .max_by(|a, b| a.throughput_qps.partial_cmp(&b.throughput_qps).unwrap())
    {
        println!(
            "  {} throughput:  {:.1} qps at concurrency {}",
            "Peak".green().bold(),
            p.throughput_qps,
            p.concurrency
        );
    }

    // Saturation point (mean > 2x baseline).
    let baseline_mean = all_stats.first().map(|s| s.mean).unwrap_or(Duration::ZERO);
    let saturation = all_stats
        .iter()
        .find(|s| s.mean > baseline_mean * 2 && s.concurrency > 1);
    match saturation {
        Some(s) => println!(
            "  {} point:  concurrency {} (mean {}ms, baseline {}ms)",
            "Saturation".yellow().bold(),
            s.concurrency,
            fmt_ms(s.mean),
            fmt_ms(baseline_mean)
        ),
        None => println!(
            "  {} point:  not reached (mean stays within 2x baseline)",
            "Saturation".green().bold()
        ),
    }

    // Degradation ratio.
    if all_stats.len() >= 2 {
        let first = &all_stats[0];
        let last = all_stats.last().unwrap();
        let deg = last.mean.as_secs_f64() / first.mean.as_secs_f64().max(0.0001);
        let label = if deg < 2.0 {
            "Excellent".green().bold()
        } else if deg < 5.0 {
            "Acceptable".yellow().bold()
        } else {
            "Poor".red().bold()
        };
        println!(
            "  Latency degradation (1→{}):  {:.1}x — {}",
            last.concurrency, deg, label
        );
    }

    // Error summary.
    let total_fail: usize = all_stats.iter().map(|s| s.failed).sum();
    if total_fail > 0 {
        println!(
            "  {} total failures across all levels",
            format!("{total_fail}").red().bold()
        );
    } else {
        println!(
            "  {}:  0 failures across all levels",
            "Reliability".green().bold()
        );
    }

    // ── Throughput curve (ASCII) ──────────────────────────────────────
    println!(
        "\n{}",
        "─── Throughput Curve ─────────────────────────────────────────────────────".bold()
    );
    let max_qps = all_stats
        .iter()
        .map(|s| s.throughput_qps)
        .fold(0.0_f64, f64::max);
    let bar_width = 50;
    for s in &all_stats {
        let filled = ((s.throughput_qps / max_qps.max(1.0)) * bar_width as f64) as usize;
        let bar = "█".repeat(filled);
        let colored = if filled > bar_width / 2 {
            bar.green().to_string()
        } else {
            bar.yellow().to_string()
        };
        println!(
            "  c={:<4} {:>8.1} qps │{}│",
            s.concurrency, s.throughput_qps, colored
        );
    }

    // ── Latency curve (ASCII) ────────────────────────────────────────
    println!(
        "\n{}",
        "─── Latency Curve (p50) ─────────────────────────────────────────────────".bold()
    );
    let max_p50 = all_stats
        .iter()
        .map(|s| s.p50.as_secs_f64())
        .fold(0.0_f64, f64::max);
    for s in &all_stats {
        let filled = ((s.p50.as_secs_f64() / max_p50.max(0.0001)) * bar_width as f64) as usize;
        let bar = "█".repeat(filled);
        let colored = if filled < bar_width / 3 {
            bar.green().to_string()
        } else if filled < 2 * bar_width / 3 {
            bar.yellow().to_string()
        } else {
            bar.red().to_string()
        };
        println!(
            "  c={:<4} {:>8} ms │{}│",
            s.concurrency,
            fmt_ms(s.p50),
            colored
        );
    }

    println!("\n{}", "═══ Done ═══".bold());
    Ok(())
}
