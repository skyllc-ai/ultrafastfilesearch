#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! UFFS Performance Profiler — 3-phase per-drive timing using `--profile`.
//!
//! For each discovered NTFS drive, runs three caching levels:
//!
//!   COLD       — no daemon, no cache files  (full MFT read + index build)
//!   WARM CACHE — no daemon, cache files exist  (daemon auto-starts from cache)
//!   HOT        — daemon already running  (pure in-memory search)
//!
//! Uses `--profile --limit 100` so output doesn't dominate but we still
//! exercise the full search + path-resolution + serialization pipeline.
//!
//! # Usage (Windows, elevated)
//!
//! ```powershell
//! rust-script scripts\windows\profile.rs
//! rust-script scripts\windows\profile.rs --drives C,D
//! rust-script scripts\windows\profile.rs --bin C:\tools\uffs.exe
//! ```
//!
//! ```cargo
//! [dependencies]
//! ```

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct ProfileTiming {
    total_ms: u64,
    connect_ms: u64,
    ready_ms: u64,
    ipc_ms: u64,
    daemon_search_ms: u64,
    startup_ms: u64,
    records_scanned: String,
    profile_lines: Vec<String>,
}

struct RunResult {
    drive: String,
    phase: String,
    timing: ProfileTiming,
    success: bool,
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn flush() { std::io::stderr().flush().ok(); }

fn kill_daemon(bin: &PathBuf) {
    let _ = Command::new(bin).args(["--daemon", "kill"])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
    std::thread::sleep(Duration::from_secs(2));
}

fn delete_cache() {
    if let Ok(local) = env::var("LOCALAPPDATA") {
        let p = PathBuf::from(&local).join("uffs").join("cache");
        if p.exists() { let _ = std::fs::remove_dir_all(&p); }
    }
    if let Ok(tmp) = env::var("TEMP") {
        let p = PathBuf::from(&tmp).join("uffs_index_cache");
        if p.exists() { let _ = std::fs::remove_dir_all(&p); }
    }
}

fn discover_drives(bin: &PathBuf) -> Vec<String> {
    let _ = Command::new(bin).args(["*", "--limit", "1"])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
    std::thread::sleep(Duration::from_secs(1));
    let output = Command::new(bin).args(["--daemon", "status"])
        .stderr(Stdio::null()).output().ok();
    let stdout = output.map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let mut drives = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if t.contains("records") {
            if let Some(ch) = t.chars().next() {
                if ch.is_ascii_uppercase() { drives.push(ch.to_string()); }
            }
        }
    }
    drives.sort();
    drives
}

fn extract_ms(line: &str, prefix: &str) -> Option<u64> {
    let idx = line.find(prefix)?;
    let after = &line[idx + prefix.len()..];
    let num_start = after.find(|c: char| c.is_ascii_digit())?;
    let num_end = after[num_start..].find(|c: char| !c.is_ascii_digit())
        .map_or(after.len(), |e| num_start + e);
    after[num_start..num_end].parse().ok()
}

fn parse_profile(stderr_lines: &[String]) -> ProfileTiming {
    let mut t = ProfileTiming::default();
    for line in stderr_lines {
        let s = line.trim();
        // Keep profile lines for display.
        if s.starts_with("===") || s.starts_with("Connect:") || s.starts_with("Await")
            || s.starts_with("Search (IPC)") || s.starts_with("Convert")
            || s.starts_with("Uptime") || s.starts_with("Startup")
            || s.starts_with("Lock") || s.starts_with("Search:")
            || s.starts_with("Row build") || s.starts_with("Shmem")
            || s.starts_with("Output") || s.starts_with("Drive")
            || s.starts_with("SUM") || s.contains("Cache")
        {
            t.profile_lines.push(line.clone());
        }
        if let Some(v) = extract_ms(s, "Connect:") { t.connect_ms = v; }
        if let Some(v) = extract_ms(s, "Await ready:") { t.ready_ms = v; }
        if let Some(v) = extract_ms(s, "Search (IPC):") { t.ipc_ms = v; }
        if s.starts_with("Search:") {
            if let Some(v) = extract_ms(s, "Search:") { t.daemon_search_ms = v; }
            if let (Some(a), Some(b)) = (s.find('('), s.find(" records")) {
                t.records_scanned = s[a+1..b].to_string();
            }
        }
        if let Some(v) = extract_ms(s, "Startup:") { t.startup_ms = v; }
        if s.starts_with("=== TOTAL:") {
            if let Some(v) = extract_ms(s, "TOTAL:") { t.total_ms = v; }
        }
    }
    t
}

fn run_profile(bin: &PathBuf, drive: &str, phase: &str) -> RunResult {
    let args = if drive == "ALL" {
        vec!["*", "--profile", "--limit", "100"]
    } else {
        vec!["*", "--profile", "--drive", drive, "--limit", "100"]
    };
    let output = Command::new(bin)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let lines: Vec<String> = stderr.lines().map(String::from).collect();
            let timing = parse_profile(&lines);
            RunResult { drive: drive.to_string(), phase: phase.to_string(), timing, success: out.status.success() }
        }
        Err(err) => {
            eprintln!("    ERROR: {err}");
            RunResult { drive: drive.to_string(), phase: phase.to_string(),
                        timing: ProfileTiming::default(), success: false }
        }
    }
}

fn parse_args() -> (PathBuf, Vec<String>) {
    let args: Vec<String> = env::args().collect();
    let mut bin = env::var("USERPROFILE")
        .map(|h| PathBuf::from(h).join("bin").join("uffs.exe"))
        .unwrap_or_else(|_| PathBuf::from("uffs.exe"));
    let mut drives: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--drives" | "-d" => {
                i += 1;
                if i < args.len() {
                    drives = args[i].split(',').map(|s| s.trim().to_uppercase()).collect();
                }
            }
            "--bin" => { i += 1; if i < args.len() { bin = PathBuf::from(&args[i]); } }
            "--help" | "-h" => {
                eprintln!("UFFS Performance Profiler (3-phase per-drive)");
                eprintln!("Usage: rust-script scripts\\windows\\profile.rs [OPTIONS]");
                eprintln!("  --drives, -d C,D,E   Drives to profile (default: auto-discover)");
                eprintln!("  --bin PATH            Path to uffs.exe");
                std::process::exit(0);
            }
            other => { eprintln!("Unknown argument: {other}"); std::process::exit(1); }
        }
        i += 1;
    }
    (bin, drives)
}

// ─── Formatting helpers ─────────────────────────────────────────────────────
//
// Mirrors the style from crates/uffs-mft/src/display.rs:
//   numbers right-aligned, units left-aligned, fixed 11-char width.

/// Format milliseconds as a human-readable duration (fixed 11 chars).
///
/// Layout matches `crates/uffs-mft/src/display.rs` — right-aligned numbers,
/// left-aligned units:
///
/// - `< 1 s`:  `      543 ms` — `{:>8} ms`  (ms unit at pos 9–10)
/// - `1–60 s`: ` 7 s 792 ms` — `{:>2} s {:>3} ms` (ms unit at pos 9–10)
/// - `≥ 60 s`: ` 1 m  05 s ` — `{:>2} m  {:>2} s `
fn fmt_dur(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms:>8} ms")
    } else if ms < 60_000 {
        let s = ms / 1000;
        let frac = ms % 1000;
        format!("{s:>2} s {frac:>3} ms")
    } else {
        let total_s = (ms + 500) / 1000;
        let m = total_s / 60;
        let s = total_s % 60;
        format!("{m:>2} m  {s:02} s ")
    }
}

/// Format a number with comma separators (right-justified, 14 chars).
fn fmt_num(s: &str) -> String {
    // The records_scanned field is already a formatted string from the daemon.
    // If it contains only digits, add commas; otherwise pass through.
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return format!("{s:>14}");
    }
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    let formatted: String = result.chars().rev().collect();
    format!("{formatted:>14}")
}

// ─── Summary ────────────────────────────────────────────────────────────────

fn print_summary(results: &[RunResult]) {
    // Inner width: 1+5 + 1+12 + 6*(1+11) + 1+14 + 1+2 + 1 = 113.
    const W: usize = 113;
    let bar_top    = format!("╔{:═<W$}╗", "");
    let bar_mid    = format!("╠{:═<W$}╣", "");
    let bar_div    = format!("╟{:─<W$}╢", "");
    let bar_bot    = format!("╚{:═<W$}╝", "");

    eprintln!();
    eprintln!("{bar_top}");
    eprintln!("║{:^W$}║", "PERFORMANCE SUMMARY (--profile)");
    eprintln!("{bar_mid}");
    eprintln!("║ {:<5} {:<12} {:>11} {:>11} {:>11} {:>11} {:>11} {:>11} {:>14} {:>2} ║",
        "Drive", "Phase", "Total", "Connct", "Ready", "Startup", "Search", "IPC", "Records", "");
    eprintln!("{bar_mid}");
    let mut prev_drive = String::new();
    for r in results {
        if !prev_drive.is_empty() && r.drive != prev_drive {
            eprintln!("{bar_div}");
        }
        prev_drive.clone_from(&r.drive);
        let ok = if r.success { "✅" } else { "❌" };
        let t = &r.timing;
        eprintln!("║ {:<5} {:<12} {} {} {} {} {} {} {} {} ║",
            format!("{}:", r.drive), r.phase,
            fmt_dur(t.total_ms), fmt_dur(t.connect_ms),
            fmt_dur(t.ready_ms), fmt_dur(t.startup_ms),
            fmt_dur(t.daemon_search_ms), fmt_dur(t.ipc_ms),
            fmt_num(&t.records_scanned), ok);
    }
    eprintln!("{bar_bot}");

    // ── Speedup table ─────────────────────────────────────────────────
    const SW: usize = 68;
    let sbar_top = format!("╔{:═<SW$}╗", "");
    let sbar_mid = format!("╠{:═<SW$}╣", "");
    let sbar_div = format!("╟{:─<SW$}╢", "");
    let sbar_bot = format!("╚{:═<SW$}╝", "");

    eprintln!();
    eprintln!("{sbar_top}");
    eprintln!("║{:^SW$}║", "SPEEDUP");
    eprintln!("{sbar_mid}");
    eprintln!("║ {:<5} {:>11} {:>11} {:>11} {:>11} {:>11} ║",
        "Drive", "Cold", "Warm", "Hot", "C→H", "C→W");
    eprintln!("{sbar_mid}");

    let mut seen = Vec::new();
    for r in results { if !seen.contains(&r.drive) { seen.push(r.drive.clone()); } }
    let mut first = true;
    for drive in &seen {
        let cold = results.iter().find(|r| r.drive == *drive && r.phase == "COLD");
        let warm = results.iter().find(|r| r.drive == *drive && r.phase == "WARM CACHE");
        let hot  = results.iter().find(|r| r.drive == *drive && r.phase == "HOT");
        if let (Some(c), Some(h)) = (cold, hot) {
            if h.timing.total_ms > 0 {
                if !first { eprintln!("{sbar_div}"); }
                first = false;
                let cold_hot = c.timing.total_ms as f64 / h.timing.total_ms as f64;
                let warm_dur = warm.map_or(0, |w| w.timing.total_ms);
                let cold_warm = if warm_dur > 0 {
                    format!("{:>8.1}×  ", c.timing.total_ms as f64 / warm_dur as f64)
                } else {
                    format!("{:>11}", "—")
                };
                eprintln!("║ {:<5} {} {} {} {:>8.1}×   {} ║",
                    format!("{drive}:"),
                    fmt_dur(c.timing.total_ms),
                    fmt_dur(warm_dur),
                    fmt_dur(h.timing.total_ms),
                    cold_hot,
                    cold_warm,
                );
            }
        }
    }
    eprintln!("{sbar_bot}");
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    let (bin, mut drives) = parse_args();

    if !bin.exists() {
        eprintln!("ERROR: uffs binary not found at: {}", bin.display());
        eprintln!("Use --bin to specify the correct path.");
        std::process::exit(1);
    }

    // Version.
    let version = Command::new(&bin).arg("--version").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    // Discover drives if not specified.
    if drives.is_empty() {
        eprint!("  Auto-discovering drives... ");
        flush();
        drives = discover_drives(&bin);
        if drives.is_empty() {
            eprintln!("FAILED (no drives found). Use --drives C,D to specify.");
            std::process::exit(1);
        }
        eprintln!("found: {}", drives.join(", "));
    }

    eprintln!();
    const HW: usize = 60;
    eprintln!("╔{:═<HW$}╗", "");
    eprintln!("║{:^HW$}║", "UFFS Performance Profiler (3-Phase)");
    eprintln!("╠{:═<HW$}╣", "");
    eprintln!("║  Binary:   {:<w$}║", version, w = HW - 13);
    eprintln!("║  Drives:   {:<w$}║", drives.join(", "), w = HW - 13);
    eprintln!("║  Pattern:  {:<w$}║", "*", w = HW - 13);
    eprintln!("║  Limit:    {:<w$}║", "100 rows", w = HW - 13);
    eprintln!("║  Phases:   {:<w$}║", "COLD → WARM CACHE → HOT", w = HW - 13);
    eprintln!("╚{:═<HW$}╝", "");

    let total_start = Instant::now();
    let mut all_results: Vec<RunResult> = Vec::new();

    for drive in &drives {
        eprintln!();
        eprintln!("━━━ Drive {drive}: ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // ── COLD: kill daemon + delete cache ────────────────────────────
        eprintln!("  [COLD] Killing daemon + deleting cache...");
        kill_daemon(&bin);
        delete_cache();
        eprintln!("  [COLD] Running: uffs \"*\" --profile --drive {drive} --limit 100");
        let cold = run_profile(&bin, drive, "COLD");
        for line in &cold.timing.profile_lines { eprintln!("    {line}"); }
        eprintln!("  [COLD] Total: {}ms  {}", cold.timing.total_ms,
            if cold.success { "✅" } else { "❌" });
        all_results.push(cold);

        // ── WARM CACHE: kill daemon, cache files remain ─────────────────
        eprintln!();
        eprintln!("  [WARM CACHE] Killing daemon (cache files remain)...");
        kill_daemon(&bin);
        eprintln!("  [WARM CACHE] Running: uffs \"*\" --profile --drive {drive} --limit 100");
        let warm = run_profile(&bin, drive, "WARM CACHE");
        for line in &warm.timing.profile_lines { eprintln!("    {line}"); }
        eprintln!("  [WARM CACHE] Total: {}ms  {}", warm.timing.total_ms,
            if warm.success { "✅" } else { "❌" });
        all_results.push(warm);

        // ── HOT: daemon still running from warm cache run ───────────────
        eprintln!();
        eprintln!("  [HOT] Daemon still running from WARM CACHE phase...");
        eprintln!("  [HOT] Running: uffs \"*\" --profile --drive {drive} --limit 100");
        let hot = run_profile(&bin, drive, "HOT");
        for line in &hot.timing.profile_lines { eprintln!("    {line}"); }
        eprintln!("  [HOT] Total: {}ms  {}", hot.timing.total_ms,
            if hot.success { "✅" } else { "❌" });
        all_results.push(hot);
    }

    // ── ALL drives: COLD → WARM CACHE → HOT ───────────────────────────
    if drives.len() > 1 {
        eprintln!();
        eprintln!("━━━ ALL drives: ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // COLD ALL: kill daemon + delete cache
        eprintln!("  [COLD] Killing daemon + deleting cache...");
        kill_daemon(&bin);
        delete_cache();
        eprintln!("  [COLD] Running: uffs \"*\" --profile --limit 100");
        let cold_all = run_profile(&bin, "ALL", "COLD");
        for line in &cold_all.timing.profile_lines { eprintln!("    {line}"); }
        eprintln!("  [COLD] Total: {}ms  {}", cold_all.timing.total_ms,
            if cold_all.success { "✅" } else { "❌" });
        all_results.push(cold_all);

        // WARM CACHE ALL: kill daemon, cache files remain
        eprintln!();
        eprintln!("  [WARM CACHE] Killing daemon (cache files remain)...");
        kill_daemon(&bin);
        eprintln!("  [WARM CACHE] Running: uffs \"*\" --profile --limit 100");
        let warm_all = run_profile(&bin, "ALL", "WARM CACHE");
        for line in &warm_all.timing.profile_lines { eprintln!("    {line}"); }
        eprintln!("  [WARM CACHE] Total: {}ms  {}", warm_all.timing.total_ms,
            if warm_all.success { "✅" } else { "❌" });
        all_results.push(warm_all);

        // HOT ALL: daemon still running
        eprintln!();
        eprintln!("  [HOT] Daemon still running from WARM CACHE phase...");
        eprintln!("  [HOT] Running: uffs \"*\" --profile --limit 100");
        let hot_all = run_profile(&bin, "ALL", "HOT");
        for line in &hot_all.timing.profile_lines { eprintln!("    {line}"); }
        eprintln!("  [HOT] Total: {}ms  {}", hot_all.timing.total_ms,
            if hot_all.success { "✅" } else { "❌" });
        all_results.push(hot_all);
    }

    // ── Summary ─────────────────────────────────────────────────────────
    print_summary(&all_results);

    let total_secs = total_start.elapsed().as_secs();
    eprintln!();
    eprintln!("Total profiling time: {}m {}s", total_secs / 60, total_secs % 60);

    // Cleanup.
    kill_daemon(&bin);
}