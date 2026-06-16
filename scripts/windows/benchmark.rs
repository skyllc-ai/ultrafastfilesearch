#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! UFFS End-to-End Benchmark — unified cold / warm / hot measurement.
//!
//! Replaces the three PowerShell benchmark scripts with a single Rust script.
//!
//! # Phases
//!
//!   **COLD**       — daemon killed + cache deleted (full MFT read), pattern `*`
//!   **WARM CACHE** — daemon killed, cache persists (cache load), pattern `*`
//!   **HOT**        — daemon running, multiple patterns to exercise different
//!                    query code paths: `*` (full scan), `*.txt` (extension
//!                    filter), `test` (substring search)
//!
//! # Per-drive cycle
//!
//!   For each drive (individually, then all drives in parallel):
//!     COLD  → WARM CACHE → HOT × patterns
//!
//! # Usage
//!
//! ```powershell
//! rust-script scripts\windows\benchmark.rs
//! rust-script scripts\windows\benchmark.rs --drives C,D --rounds 5
//! rust-script scripts\windows\benchmark.rs --phase hot --pattern "*.dll" --pattern "config"
//! rust-script scripts\windows\benchmark.rs --bin C:\tools\uffs.exe
//! rust-script scripts\windows\benchmark.rs --benchmark-mode
//! rust-script scripts/windows/benchmark.rs --data-dir ~/uffs_data
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

const TIMEOUT_SECS: u64 = 300;

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase { Cold, WarmCache, Hot }
impl Phase {
    fn label(self) -> &'static str {
        match self { Self::Cold => "COLD", Self::WarmCache => "WARM CACHE", Self::Hot => "HOT" }
    }
    fn desc(self) -> &'static str {
        match self {
            Self::Cold => "no daemon, no cache",
            Self::WarmCache => "no daemon, cache on disk",
            Self::Hot => "daemon running, in-memory",
        }
    }
}

/// Default patterns that exercise different query code paths:
///   `*`      — full scan (DataFrame pass-through)
///   `*.txt`  — extension filter (Polars column predicate)
///   `test`   — substring search (contains match)
const DEFAULT_PATTERNS: &[&str] = &["*", "*.txt", "test"];

/// Default result limit so stdout doesn't dominate.
const DEFAULT_LIMIT: u32 = 100;

#[derive(Clone)]
struct RunTiming { wall_ms: u64, stderr_lines: Vec<String>, success: bool, timed_out: bool }

struct PhaseResult { drive: String, phase: Phase, pattern: String, timings: Vec<RunTiming> }

/// Per-drive MFT file mapping, discovered from `--data-dir`.
/// Maps drive letter (e.g. "C") → MFT file path.
struct DataSources {
    /// The root data directory (e.g. ~/uffs_data). `None` on Windows (live).
    data_dir: Option<PathBuf>,
    /// drive letter → best MFT file inside `data_dir/drive_<x>/`.
    drive_files: std::collections::HashMap<String, PathBuf>,
}

impl DataSources {
    /// Scan a data directory for `drive_*` subdirs, mimicking
    /// `uffs_mft::discovery::discover_mft_files`.
    fn from_data_dir(dir: &std::path::Path) -> Self {
        let mut drive_files = std::collections::HashMap::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() { continue; }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
                if let Some(letter) = name.strip_prefix("drive_") {
                    if letter.len() == 1 && letter.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                        if let Some(mft) = find_best_mft_file(&path) {
                            drive_files.insert(letter.to_uppercase(), mft);
                        }
                    }
                }
            }
        }
        DataSources { data_dir: Some(dir.to_path_buf()), drive_files }
    }

    fn empty() -> Self {
        DataSources { data_dir: None, drive_files: std::collections::HashMap::new() }
    }

    /// Source args for a specific drive or ALL.
    ///
    /// Non-Windows with data dir:
    ///   individual → `["--mft-file", "<path>"]` (loads only that drive)
    ///   ALL        → `["--data-dir", "<path>"]` (loads everything)
    ///
    /// Windows (no data dir):
    ///   individual → `["--drive", "C"]` (daemon loads only that drive)
    ///   ALL        → `[]` (daemon auto-discovers all drives)
    fn args_for(&self, drive: &str) -> Vec<String> {
        if drive != "ALL" {
            // Non-Windows: per-drive MFT file for true isolation.
            if let Some(mft) = self.drive_files.get(drive) {
                return vec!["--mft-file".into(), mft.to_string_lossy().into_owned()];
            }
            // Windows: use --drive so daemon only loads this drive.
            if self.data_dir.is_none() {
                return vec!["--drive".into(), drive.to_string()];
            }
        }
        // ALL drives or data-dir fallback.
        match &self.data_dir {
            Some(d) => vec!["--data-dir".into(), d.to_string_lossy().into_owned()],
            None => vec![], // Windows: daemon auto-discovers all drives.
        }
    }

    fn available_drives(&self) -> Vec<String> {
        let mut d: Vec<String> = self.drive_files.keys().cloned().collect();
        d.sort();
        d
    }
}

/// Find the best MFT file in a directory by format priority.
/// Prefers `.iocp` > `.bin` > `.mft` (matches uffs_mft::discovery).
fn find_best_mft_file(dir: &std::path::Path) -> Option<PathBuf> {
    let mut best: Option<(PathBuf, u8)> = None;
    for file in std::fs::read_dir(dir).ok()?.flatten() {
        let fp = file.path();
        if !fp.is_file() { continue; }
        let ext = fp.extension().and_then(|e| e.to_str()).unwrap_or("");
        let pri = match ext { "iocp" => 0u8, "bin" => 1, "mft" => 2, _ => continue };
        if best.as_ref().is_none_or(|(_, bp)| pri < *bp) { best = Some((fp, pri)); }
    }
    best.map(|(p, _)| p)
}

struct BenchConfig {
    bin: PathBuf, drives: Vec<String>, rounds: usize, patterns: Vec<String>,
    limit: u32, phases: Vec<Phase>, benchmark_mode: bool,
    extra_args: Vec<String>, sources: DataSources,
}

// ─── Daemon lifecycle (modelled on daemon-readiness.rs) ─────────────────────

fn flush() { std::io::stderr().flush().ok(); }

/// Kill daemon and **poll until it is confirmed stopped** (up to 10 s).
fn ensure_stopped(bin: &PathBuf) {
    let _ = Command::new(bin).args(["--daemon", "kill"])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        if let Ok(out) = Command::new(bin).args(["--daemon", "status"])
            .stderr(Stdio::null()).output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.contains("not running") { return; }
        }
    }
    eprintln!("    ⚠ daemon did not stop within 10 s");
}

/// Start daemon explicitly (`daemon start`) and **poll until Ready** (up to 120 s).
/// `source_args` should be e.g. `["--mft-file", "/path/to/C_mft.iocp"]` for a
/// single drive, or `["--data-dir", "/path"]` for all drives.
fn start_and_await_ready(bin: &PathBuf, source_args: &[String]) -> bool {
    let mut args: Vec<String> = vec!["--daemon".into(), "start".into()];
    args.extend(source_args.iter().cloned());
    let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let _ = Command::new(bin).args(&str_args)
        .stdout(Stdio::null()).stderr(Stdio::null()).status();

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if let Ok(out) = Command::new(bin).args(["--daemon", "status"])
            .stderr(Stdio::null()).output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.contains("Ready") { return true; }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// Assert daemon is Ready right now.
fn assert_ready(bin: &PathBuf) -> bool {
    if let Ok(out) = Command::new(bin).args(["--daemon", "status"])
        .stderr(Stdio::null()).output()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        return s.contains("Ready");
    }
    false
}

fn delete_cache() {
    // Windows
    for (var, sub) in [("LOCALAPPDATA", "uffs/cache"), ("TEMP", "uffs_index_cache")] {
        if let Ok(base) = env::var(var) {
            let p = PathBuf::from(base).join(sub);
            if p.exists() { let _ = std::fs::remove_dir_all(&p); }
        }
    }
    // macOS / Linux
    if let Ok(home) = env::var("HOME") {
        for sub in [
            "Library/Application Support/uffs/cache",
            "Library/Caches/uffs",
            ".local/share/uffs/cache",
            ".cache/uffs",
        ] {
            let p = PathBuf::from(&home).join(sub);
            if p.exists() { let _ = std::fs::remove_dir_all(&p); }
        }
    }
}

/// Discover available drives — no daemon needed.
///
/// Non-Windows: scan the data directory for `drive_*` subdirs.
/// Windows: enumerate fixed NTFS drives via `wmic` (lightweight, no MFT load).
fn discover_drives(_bin: &PathBuf, sources: &DataSources) -> Vec<String> {
    // If we have a data dir, drives were already discovered from the filesystem.
    if sources.data_dir.is_some() {
        let d = sources.available_drives();
        if !d.is_empty() { return d; }
    }
    // Windows: ask the OS directly — no daemon, no MFT loading.
    if cfg!(windows) {
        return discover_windows_ntfs_drives();
    }
    Vec::new()
}

/// Enumerate NTFS drives on Windows using `wmic`.
///
/// Includes both fixed (DriveType=3) and removable (DriveType=2, e.g. USB)
/// drives, filtered to NTFS filesystem.
/// Falls back to A–Z probing if wmic is unavailable.
fn discover_windows_ntfs_drives() -> Vec<String> {
    // DriveType 2=Removable (USB), 3=Local Fixed. Both can be NTFS.
    if let Ok(out) = Command::new("wmic")
        .args(["logicaldisk", "where", "DriveType=2 or DriveType=3", "get", "DeviceID,FileSystem", "/format:csv"])
        .stdout(Stdio::piped()).stderr(Stdio::null())
        .output()
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut drives = Vec::new();
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(',').collect();
                // CSV format: Node,DeviceID,FileSystem
                if parts.len() >= 3 {
                    let device_id = parts[1].trim();
                    let fs = parts[2].trim();
                    if fs.eq_ignore_ascii_case("NTFS") {
                        if let Some(ch) = device_id.chars().next() {
                            if ch.is_ascii_uppercase() {
                                drives.push(ch.to_string());
                            }
                        }
                    }
                }
            }
            if !drives.is_empty() {
                drives.sort();
                return drives;
            }
        }
    }
    // Fallback: probe A–Z for existing drive roots.
    let mut drives = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if std::path::Path::new(&root).exists() {
            drives.push((letter as char).to_string());
        }
    }
    drives
}

fn run_once(bin: &PathBuf, args: &[String]) -> RunTiming {
    let t0 = Instant::now();
    let child = Command::new(bin).args(args)
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("      ERROR: spawn failed: {e}");
            return RunTiming { wall_ms: t0.elapsed().as_millis() as u64,
                stderr_lines: vec![], success: false, timed_out: false };
        }
    };
    let output = child.wait_with_output();
    let wall_ms = t0.elapsed().as_millis() as u64;
    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            RunTiming { wall_ms, stderr_lines: stderr.lines().map(String::from).collect(),
                success: out.status.success(), timed_out: wall_ms > TIMEOUT_SECS * 1000 }
        }
        Err(e) => {
            eprintln!("      ERROR: wait failed: {e}");
            RunTiming { wall_ms, stderr_lines: vec![], success: false, timed_out: false }
        }
    }
}

fn build_args(cfg: &BenchConfig, drive: &str, pattern: &str) -> Vec<String> {
    let mut a = vec![pattern.to_string()];
    // Source args: --mft-file (non-Windows per-drive), --drive (Windows per-drive),
    // or --data-dir (ALL). This handles both daemon start and search-time filtering.
    let src = cfg.sources.args_for(drive);
    a.extend(src);
    if cfg.benchmark_mode && pattern == "*" { a.push("--benchmark".into()); }
    a.extend(["--limit".into(), cfg.limit.to_string()]);
    a.extend(cfg.extra_args.clone());
    a
}

// ─── Benchmark execution ───────────────────────────────────────────────────
//
// Lifecycle per phase (mirrors daemon-readiness.rs Scenario K):
//
//   COLD (each round, pattern="*"):
//     1. ensure_stopped()        — kill + poll until "not running"
//     2. delete_cache()          — wipe all cache dirs
//     3. run search "*"          — daemon auto-starts, reads MFT, builds index, searches
//     (after last round: daemon running, cache files on disk)
//
//   WARM CACHE (each round, pattern="*"):
//     Before round 1: if no prior COLD phase populated cache, do a priming run.
//     Each round:
//     1. ensure_stopped()        — kill + poll (cache stays)
//     2. run search "*"          — daemon auto-starts from cache, searches
//
//   HOT (each round × each pattern):
//     Before round 1: ensure daemon running + Ready.
//     Each round runs every configured pattern (*, *.txt, test, …)
//     to exercise different query code paths:
//       "*"      — full scan (DataFrame pass-through)
//       "*.txt"  — extension filter (Polars column predicate)
//       "test"   — substring search (contains match)

/// Run N rounds of a single pattern, returning one PhaseResult.
fn bench_rounds(
    cfg: &BenchConfig, drive: &str, phase: Phase, pattern: &str,
) -> PhaseResult {
    let args = build_args(cfg, drive, pattern);
    let src = cfg.sources.args_for(drive);
    eprintln!("    CMD: {} {}", cfg.bin.display(), args.join(" "));
    let mut timings = Vec::new();
    for round in 1..=cfg.rounds {
        // Per-round setup
        match phase {
            Phase::Cold => {
                eprint!("    [prep] ensure_stopped + delete_cache... "); flush();
                ensure_stopped(&cfg.bin);
                delete_cache();
                eprintln!("done");
            }
            Phase::WarmCache => {
                eprint!("    [prep] ensure_stopped (cache stays)... "); flush();
                ensure_stopped(&cfg.bin);
                eprintln!("done");
            }
            Phase::Hot => {
                if !assert_ready(&cfg.bin) {
                    eprintln!("    ⚠ daemon not Ready before round {round}, restarting...");
                    start_and_await_ready(&cfg.bin, &src);
                }
            }
        }
        let t = run_once(&cfg.bin, &args);
        let s = if t.timed_out { "TIMEOUT".into() } else if !t.success { "FAIL".into() }
                else { format!("{}ms", t.wall_ms) };
        eprint!("    Run {round}/{}: {s}", cfg.rounds);
        for line in &t.stderr_lines {
            if line.contains("[TIMING]") || line.contains("[CACHE_PROFILE]")
                || line.contains("BENCHMARK MODE") || line.contains("[DIAG]") {
                eprint!("  │ {line}");
            }
        }
        eprintln!();
        if t.timed_out { ensure_stopped(&cfg.bin); }
        timings.push(t);
    }
    PhaseResult { drive: drive.to_string(), phase, pattern: pattern.to_string(), timings }
}

/// Phase setup (once), then delegate to bench_rounds.
fn bench_phase(cfg: &BenchConfig, drive: &str, phase: Phase, prev_phase: Option<Phase>) -> Vec<PhaseResult> {
    eprintln!();
    eprintln!("  [{:>10}] {} — {}", phase.label(), drive, phase.desc());
    let src = cfg.sources.args_for(drive);

    // ── Phase setup (once before all rounds) ────────────────────────────
    match phase {
        Phase::Cold => { /* each round handles its own lifecycle */ }
        Phase::WarmCache => {
            if prev_phase != Some(Phase::Cold) {
                eprint!("    Priming cache (no prior COLD phase)... "); flush();
                ensure_stopped(&cfg.bin);
                delete_cache();
                let t0 = Instant::now();
                if start_and_await_ready(&cfg.bin, &src) {
                    eprintln!("done ({}ms)", t0.elapsed().as_millis());
                } else {
                    eprintln!("FAILED — daemon did not reach Ready");
                }
            }
            ensure_stopped(&cfg.bin);
        }
        Phase::Hot => {
            eprint!("    Ensuring daemon is running + Ready... "); flush();
            let t0 = Instant::now();
            if !assert_ready(&cfg.bin) {
                if !start_and_await_ready(&cfg.bin, &src) {
                    eprintln!("FAILED — daemon did not reach Ready");
                } else {
                    // Priming search to warm in-memory query path.
                    let warmup_args = build_args(cfg, drive, "*");
                    let _ = run_once(&cfg.bin, &warmup_args);
                }
            }
            eprintln!("ready ({}ms)", t0.elapsed().as_millis());
        }
    }

    // ── Run rounds ──────────────────────────────────────────────────────
    // COLD / WARM CACHE: single pattern ("*") — the cost is startup, not query.
    // HOT: all configured patterns — each exercises a different query path.
    let patterns: Vec<String> = match phase {
        Phase::Cold | Phase::WarmCache => vec!["*".to_string()],
        Phase::Hot => cfg.patterns.clone(),
    };

    let mut results = Vec::new();
    for pat in &patterns {
        if patterns.len() > 1 {
            eprintln!("    ── pattern: {pat:?} ──");
        }
        results.push(bench_rounds(cfg, drive, phase, pat));
    }
    results
}

fn bench_drive(cfg: &BenchConfig, drive: &str) -> Vec<PhaseResult> {
    eprintln!("\n━━━ Drive {drive}: ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let mut results = Vec::new();
    let mut prev_phase: Option<Phase> = None;
    for &phase in &cfg.phases {
        results.extend(bench_phase(cfg, drive, phase, prev_phase));
        prev_phase = Some(phase);
    }
    results
}

// ─── Formatting ─────────────────────────────────────────────────────────────

fn fmt_dur(ms: u64) -> String {
    if ms < 1_000 { format!("{ms:>8} ms") }
    else if ms < 60_000 { format!("{:>2} s {:>3} ms", ms / 1000, ms % 1000) }
    else { let s = (ms + 500) / 1000; format!("{:>2} m  {:>02} s ", s / 60, s % 60) }
}

fn stats(timings: &[RunTiming]) -> (u64, u64, u64, usize) {
    let ok: Vec<u64> = timings.iter().filter(|t| t.success && !t.timed_out)
        .map(|t| t.wall_ms).collect();
    if ok.is_empty() { return (0, 0, 0, 0); }
    let min = *ok.iter().min().unwrap_or(&0);
    let max = *ok.iter().max().unwrap_or(&0);
    let avg = ok.iter().sum::<u64>() / ok.len() as u64;
    (min, avg, max, ok.len())
}

fn print_summary(results: &[PhaseResult]) {
    const W: usize = 105;
    let bar_t = format!("╔{:═<W$}╗", "");
    let bar_m = format!("╠{:═<W$}╣", "");
    let bar_d = format!("╟{:─<W$}╢", "");
    let bar_b = format!("╚{:═<W$}╝", "");

    eprintln!("\n{bar_t}");
    eprintln!("║{:^W$}║", "BENCHMARK RESULTS");
    eprintln!("{bar_m}");
    eprintln!("║ {:<5} {:<12} {:<10} {:>11} {:>11} {:>11} {:>6} {:>6} {:>6}      ║",
        "Drive", "Phase", "Pattern", "Min", "Avg", "Max", "OK", "Fail", "T/O");
    eprintln!("{bar_m}");

    let mut prev_drive = String::new();
    for r in results {
        if !prev_drive.is_empty() && r.drive != prev_drive { eprintln!("{bar_d}"); }
        prev_drive.clone_from(&r.drive);
        let (min, avg, max, ok) = stats(&r.timings);
        let fail = r.timings.iter().filter(|t| !t.success && !t.timed_out).count();
        let timeout = r.timings.iter().filter(|t| t.timed_out).count();
        let pat_display = if r.pattern.len() > 10 {
            format!("{}…", &r.pattern[..9])
        } else { r.pattern.clone() };
        if ok > 0 {
            eprintln!("║ {:<5} {:<12} {:<10} {} {} {} {:>6} {:>6} {:>6}      ║",
                format!("{}:", r.drive), r.phase.label(), pat_display,
                fmt_dur(min), fmt_dur(avg), fmt_dur(max), ok, fail, timeout);
        } else {
            eprintln!("║ {:<5} {:<12} {:<10} {:>11} {:>11} {:>11} {:>6} {:>6} {:>6}      ║",
                format!("{}:", r.drive), r.phase.label(), pat_display,
                "—", "—", "—", ok, fail, timeout);
        }
    }
    eprintln!("{bar_b}");

    // ── Speedup table (COLD * → HOT *) ──────────────────────────────────
    let mut seen_drives = Vec::new();
    for r in results { if !seen_drives.contains(&r.drive) { seen_drives.push(r.drive.clone()); } }

    let has_speedups = seen_drives.iter().any(|d| {
        let cold = results.iter().find(|r| r.drive == *d && r.phase == Phase::Cold && r.pattern == "*");
        let hot  = results.iter().find(|r| r.drive == *d && r.phase == Phase::Hot && r.pattern == "*");
        cold.is_some() && hot.is_some()
    });

    if has_speedups {
        const SW: usize = 68;
        let sb_t = format!("╔{:═<SW$}╗", "");
        let sb_m = format!("╠{:═<SW$}╣", "");
        let sb_d = format!("╟{:─<SW$}╢", "");
        let sb_b = format!("╚{:═<SW$}╝", "");

        eprintln!("\n{sb_t}");
        eprintln!("║{:^SW$}║", "SPEEDUP (Cold * → Hot *)");
        eprintln!("{sb_m}");
        eprintln!("║ {:<5} {:>11} {:>11} {:>11} {:>11} {:>11} ║",
            "Drive", "Cold", "Warm", "Hot *", "C→H", "C→W");
        eprintln!("{sb_m}");

        let mut first = true;
        for drive in &seen_drives {
            let cold_avg = results.iter()
                .find(|r| r.drive == *drive && r.phase == Phase::Cold && r.pattern == "*")
                .map(|r| stats(&r.timings).1).unwrap_or(0);
            let warm_avg = results.iter()
                .find(|r| r.drive == *drive && r.phase == Phase::WarmCache && r.pattern == "*")
                .map(|r| stats(&r.timings).1).unwrap_or(0);
            let hot_avg = results.iter()
                .find(|r| r.drive == *drive && r.phase == Phase::Hot && r.pattern == "*")
                .map(|r| stats(&r.timings).1).unwrap_or(0);
            if cold_avg == 0 || hot_avg == 0 { continue; }
            if !first { eprintln!("{sb_d}"); }
            first = false;
            let c2h = cold_avg as f64 / hot_avg as f64;
            let c2w = if warm_avg > 0 { format!("{:>8.1}×  ", cold_avg as f64 / warm_avg as f64) }
                else { format!("{:>11}", "—") };
            eprintln!("║ {:<5} {} {} {} {:>8.1}×   {} ║",
                format!("{drive}:"), fmt_dur(cold_avg), fmt_dur(warm_avg),
                fmt_dur(hot_avg), c2h, c2w);
        }
        eprintln!("{sb_b}");
    }

    // ── HOT pattern comparison ──────────────────────────────────────────
    let hot_results: Vec<&PhaseResult> = results.iter()
        .filter(|r| r.phase == Phase::Hot)
        .collect();
    let hot_patterns: Vec<String> = {
        let mut p = Vec::new();
        for r in &hot_results { if !p.contains(&r.pattern) { p.push(r.pattern.clone()); } }
        p
    };
    if hot_patterns.len() > 1 {
        const PW: usize = 70;
        let pb_t = format!("╔{:═<PW$}╗", "");
        let pb_m = format!("╠{:═<PW$}╣", "");
        let pb_d = format!("╟{:─<PW$}╢", "");
        let pb_b = format!("╚{:═<PW$}╝", "");

        eprintln!("\n{pb_t}");
        eprintln!("║{:^PW$}║", "HOT QUERY COMPARISON");
        eprintln!("{pb_m}");
        eprintln!("║ {:<5} {:<10} {:>11} {:>11} {:>11} {:>11}    ║",
            "Drive", "Pattern", "Min", "Avg", "Max", "vs *");
        eprintln!("{pb_m}");

        let mut first = true;
        for drive in &seen_drives {
            let star_avg = hot_results.iter()
                .find(|r| r.drive == *drive && r.pattern == "*")
                .map(|r| stats(&r.timings).1).unwrap_or(0);
            let drive_hot: Vec<&&PhaseResult> = hot_results.iter()
                .filter(|r| r.drive == *drive)
                .collect();
            if drive_hot.is_empty() { continue; }
            if !first { eprintln!("{pb_d}"); }
            first = false;
            for r in drive_hot {
                let (min, avg, max, _) = stats(&r.timings);
                let vs_star = if r.pattern == "*" || star_avg == 0 {
                    format!("{:>11}", "—")
                } else {
                    format!("{:>8.1}×  ", star_avg as f64 / avg.max(1) as f64)
                };
                let pat_display = if r.pattern.len() > 10 {
                    format!("{}…", &r.pattern[..9])
                } else { r.pattern.clone() };
                eprintln!("║ {:<5} {:<10} {} {} {} {}    ║",
                    format!("{}:", r.drive), pat_display,
                    fmt_dur(min), fmt_dur(avg), fmt_dur(max), vs_star);
            }
        }
        eprintln!("{pb_b}");
    }
}

// ─── Arg parsing ────────────────────────────────────────────────────────────

fn parse_args() -> BenchConfig {
    let args: Vec<String> = env::args().collect();
    let mut bin: Option<PathBuf> = None;
    let mut drives: Vec<String> = Vec::new();
    let mut rounds = 3usize;
    let mut patterns: Vec<String> = Vec::new();
    let mut limit: Option<u32> = None;
    let mut phases: Vec<Phase> = Vec::new();
    let mut benchmark_mode = false;
    let mut extra_args: Vec<String> = Vec::new();
    let mut data_dir: Option<PathBuf> = None;
    let mut mft_file: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bin" | "-b" => { i += 1; if i < args.len() { bin = Some(PathBuf::from(&args[i])); } }
            "--drives" | "-d" => {
                i += 1;
                if i < args.len() {
                    drives = args[i].split(',').map(|s| s.trim().to_uppercase()).collect();
                }
            }
            "--rounds" | "-n" => { i += 1; if i < args.len() { rounds = args[i].parse().unwrap_or(3); } }
            "--pattern" | "-p" => {
                i += 1;
                if i < args.len() { patterns.push(args[i].clone()); }
            }
            "--limit" | "-l" => { i += 1; if i < args.len() { limit = args[i].parse().ok(); } }
            "--phase" => {
                i += 1;
                if i < args.len() {
                    match args[i].to_lowercase().as_str() {
                        "cold" => phases.push(Phase::Cold),
                        "warm" | "warm-cache" | "warm_cache" => phases.push(Phase::WarmCache),
                        "hot" => phases.push(Phase::Hot),
                        "all" => phases = vec![Phase::Cold, Phase::WarmCache, Phase::Hot],
                        other => { eprintln!("Unknown phase: {other}"); std::process::exit(1); }
                    }
                }
            }
            "--benchmark-mode" => benchmark_mode = true,
            "--data-dir" => {
                i += 1;
                if i < args.len() { data_dir = Some(PathBuf::from(&args[i])); }
            }
            "--mft-file" => {
                i += 1;
                if i < args.len() { mft_file = Some(PathBuf::from(&args[i])); }
            }
            "--help" | "-h" => {
                eprintln!("UFFS End-to-End Benchmark");
                eprintln!();
                eprintln!("Usage: rust-script benchmark.rs [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --bin PATH          Path to uffs binary");
                eprintln!("  --drives C,D,E      Drives to benchmark (default: auto-discover)");
                eprintln!("  --rounds N          Rounds per phase (default: 3)");
                eprintln!("  --pattern PAT       HOT-phase search pattern (repeatable)");
                eprintln!("                      Default: *, *.txt, test");
                eprintln!("                      COLD/WARM always use * (full scan)");
                eprintln!("  --limit N           Limit result rows (default: {DEFAULT_LIMIT})");
                eprintln!("  --phase PHASE       Phase: cold, warm, hot, all (default: all)");
                eprintln!("  --benchmark-mode    Use --benchmark flag for full scans");
                eprintln!("  --data-dir PATH     MFT data directory (drive_c/, drive_d/, ...)");
                eprintln!("                      Per-drive benchmarks use --mft-file for isolation");
                eprintln!("  --mft-file PATH     Single MFT file (benchmarks only that drive)");
                eprintln!("  -- EXTRA_ARGS       Extra args passed to uffs");
                eprintln!();
                eprintln!("Patterns exercise different query code paths:");
                eprintln!("  *       full scan (DataFrame pass-through)");
                eprintln!("  *.txt   extension filter (Polars column predicate)");
                eprintln!("  test    substring search (contains match)");
                eprintln!();
                eprintln!("Per-drive isolation:");
                eprintln!("  When --data-dir contains drive_c/, drive_d/, etc., individual");
                eprintln!("  drives are benchmarked with --mft-file (loads only that drive's");
                eprintln!("  MFT). The ALL-drives run uses --data-dir (loads everything).");
                std::process::exit(0);
            }
            "--" => { extra_args = args[i+1..].to_vec(); break; }
            other => {
                let p = std::path::Path::new(other);
                if p.exists() {
                    if p.is_dir() { data_dir = Some(p.to_path_buf()); }
                    else if p.is_file() { mft_file = Some(p.to_path_buf()); }
                } else {
                    eprintln!("Unknown argument: {other}"); std::process::exit(1);
                }
            }
        }
        i += 1;
    }

    if phases.is_empty() { phases = vec![Phase::Cold, Phase::WarmCache, Phase::Hot]; }
    if patterns.is_empty() {
        patterns = DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();
    }
    if !patterns.contains(&"*".to_string()) {
        patterns.insert(0, "*".to_string());
    }

    // ── Smart default for data source on non-Windows ────────────────────
    if !cfg!(windows) && data_dir.is_none() && mft_file.is_none() {
        let home = env::var("HOME").unwrap_or_else(|_| ".".into());
        let default = PathBuf::from(&home).join("uffs_data");
        if default.is_dir() {
            eprintln!("  (defaulting to --data-dir {})", default.display());
            data_dir = Some(default);
        } else {
            eprintln!("ERROR: No data source given and ~/uffs_data not found.");
            eprintln!();
            eprintln!("On non-Windows you need MFT data to benchmark against:");
            eprintln!("  rust-script benchmark.rs ~/uffs_data");
            eprintln!("  rust-script benchmark.rs --data-dir /path/to/data");
            eprintln!("  rust-script benchmark.rs --mft-file /path/to/C_mft.iocp");
            std::process::exit(1);
        }
    }

    // ── Build DataSources ───────────────────────────────────────────────
    let sources = if let Some(ref dir) = data_dir {
        let s = DataSources::from_data_dir(dir);
        let n = s.drive_files.len();
        let drives_found: Vec<&String> = { let mut d: Vec<_> = s.drive_files.keys().collect(); d.sort(); d };
        eprintln!("  Data dir: {} ({n} drives: {})", dir.display(),
            drives_found.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(", "));
        for (letter, path) in &s.drive_files {
            eprintln!("    {letter}: → {}", path.display());
        }
        s
    } else if let Some(ref _mft) = mft_file {
        // Single MFT file — create a DataSources with just that drive.
        // The drive letter is inferred from the filename (e.g. C_mft.iocp → C).
        let mut ds = DataSources::empty();
        let fname = _mft.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let letter = fname.chars().next()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "C".into());
        ds.drive_files.insert(letter, _mft.clone());
        ds
    } else {
        DataSources::empty() // Windows live drives
    };

    let bin = bin.unwrap_or_else(|| PathBuf::from(default_binary()));
    let limit = limit.unwrap_or(DEFAULT_LIMIT);

    BenchConfig { bin, drives, rounds, patterns, limit, phases, benchmark_mode, extra_args, sources }
}

/// Build a fresh release binary and return the path to it (macOS/Linux).
fn ensure_fresh_release_build() -> String {
    let workspace = find_workspace_root();
    let binary_path = workspace.join("target").join("release").join("uffs");

    eprintln!("╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  Building fresh release binary...                                ║");
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
    eprintln!("  Workspace: {}", workspace.display());

    let start = Instant::now();
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "uffs-cli"])
        .current_dir(&workspace)
        .status();

    match status {
        Ok(s) if s.success() => {
            eprintln!("  ✅ Build completed in {:.1}s", start.elapsed().as_secs_f64());
            eprintln!("  Binary: {}", binary_path.display());
            eprintln!();
        }
        Ok(s) => {
            eprintln!("  ❌ cargo build --release failed (exit {s})");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("  ❌ Failed to run cargo: {e}");
            std::process::exit(1);
        }
    }

    binary_path.to_string_lossy().into_owned()
}

/// Locate the uffs binary.
///
/// On non-Windows: build a fresh release binary.
/// On Windows: check ~/bin, target/release, then PATH.
fn default_binary() -> String {
    if !cfg!(windows) {
        return ensure_fresh_release_build();
    }
    let bin_name = "uffs.exe";
    let home = env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    let candidates = [
        format!("{home}\\bin\\{bin_name}"),
        format!("target\\release\\{bin_name}"),
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() { return c.clone(); }
    }
    bin_name.to_string()
}

fn find_workspace_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd.as_path();
    loop {
        if dir.join("Cargo.toml").exists() && dir.join(".cargo").exists() { return dir.to_path_buf(); }
        match dir.parent() { Some(p) => dir = p, None => break }
    }
    cwd
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    let mut cfg = parse_args();

    if !cfg.bin.exists() {
        eprintln!("ERROR: uffs binary not found at: {}", cfg.bin.display());
        eprintln!("Use --bin to specify the correct path.");
        std::process::exit(1);
    }

    // Version.
    let version = Command::new(&cfg.bin).arg("--version").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    // Auto-discover drives if none specified.
    if cfg.drives.is_empty() {
        eprint!("  Auto-discovering drives... "); flush();
        cfg.drives = discover_drives(&cfg.bin, &cfg.sources);
        if cfg.drives.is_empty() {
            eprintln!("FAILED. Use --drives C,D to specify.");
            std::process::exit(1);
        }
        eprintln!("found: {}", cfg.drives.join(", "));
    }

    // Header.
    eprintln!();
    const HW: usize = 60;
    eprintln!("╔{:═<HW$}╗", "");
    eprintln!("║{:^HW$}║", "UFFS End-to-End Benchmark");
    eprintln!("╠{:═<HW$}╣", "");
    eprintln!("║  Binary:   {:<w$}║", version, w = HW - 13);
    eprintln!("║  Drives:   {:<w$}║", cfg.drives.join(", "), w = HW - 13);
    let pat_str = cfg.patterns.join(", ");
    eprintln!("║  Patterns: {:<w$}║", pat_str, w = HW - 13);
    eprintln!("║  Rounds:   {:<w$}║", cfg.rounds, w = HW - 13);
    let phase_str = cfg.phases.iter().map(|p| p.label()).collect::<Vec<_>>().join(" → ");
    eprintln!("║  Phases:   {:<w$}║", phase_str, w = HW - 13);
    eprintln!("║  Limit:    {:<w$}║", cfg.limit, w = HW - 13);
    if cfg.benchmark_mode {
        eprintln!("║  Mode:     {:<w$}║", "--benchmark (no stdout)", w = HW - 13);
    }
    if let Some(ref d) = cfg.sources.data_dir {
        eprintln!("║  Source:   {:<w$}║", format!("--data-dir {}", d.display()), w = HW - 13);
        let n = cfg.sources.drive_files.len();
        eprintln!("║  Drives:   {:<w$}║",
            format!("{n} MFT files (per-drive isolation)"), w = HW - 13);
    }
    eprintln!("╚{:═<HW$}╝", "");

    let total_start = Instant::now();
    let mut all_results: Vec<PhaseResult> = Vec::new();

    // Benchmark each drive individually.
    for drive in cfg.drives.clone() {
        let results = bench_drive(&cfg, &drive);
        all_results.extend(results);
    }

    // If multiple drives, also benchmark "ALL" (parallel).
    if cfg.drives.len() > 1 {
        eprintln!("\n╔══════════════════════════════════════╗");
        eprintln!("║  ALL DRIVES (parallel)               ║");
        eprintln!("╚══════════════════════════════════════╝");
        let results = bench_drive(&cfg, "ALL");
        all_results.extend(results);
    }

    // Summary tables.
    print_summary(&all_results);

    let total_secs = total_start.elapsed().as_secs();
    eprintln!("\nTotal benchmark time: {}m {}s", total_secs / 60, total_secs % 60);

    // Cleanup.
    ensure_stopped(&cfg.bin);
    eprintln!("🧹 Daemon stopped after benchmark.");
}
