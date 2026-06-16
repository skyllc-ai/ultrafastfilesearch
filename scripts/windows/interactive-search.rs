#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! UFFS Interactive Search Benchmark — p50/p95 latency with `--profile` breakdown.
//!
//! Tests the "Interactive" benchmark class from the public benchmark plan:
//!
//!   | Class       | Example workloads                                     | Metrics             |
//!   |-------------|-------------------------------------------------------|---------------------|
//!   | Interactive | *, exact name, prefix, *.dll, substring, date/size    | p50/p95 end-to-end  |
//!
//! Runs 20+ rounds of each query pattern against a HOT daemon, extracts
//! both **end-to-end** (wall clock) and **daemon-side** (from `--profile`)
//! timings, and computes p50 and p95 percentiles.
//!
//! # Usage
//!
//! ```powershell
//! rust-script scripts\windows\interactive-search.rs
//! rust-script scripts\windows\interactive-search.rs --data-dir ~/uffs_data
//! rust-script scripts\windows\interactive-search.rs --rounds 50 --drives C
//! rust-script scripts/windows/interactive-search.rs --data-dir ~/uffs_data --pattern "*.rs"
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

const TIMEOUT_SECS: u64 = 120;

/// Patterns that exercise different query code paths, matching the benchmark plan.
const DEFAULT_PATTERNS: &[(&str, &str)] = &[
    ("*",              "full scan"),
    ("notepad.exe",    "exact name"),
    ("win*",           "prefix"),
    ("*.dll",          "extension"),
    ("config",         "substring"),
];

/// Additional filter patterns (added with --full-suite).
const FILTER_PATTERNS: &[(&str, &[&str], &str)] = &[
    ("date filter",  &["*", "--newer", "30d"], "modified last 30 days"),
    ("size filter",  &["*", "--min-size", "1048576"], "files >= 1 MB"),
    ("combined",     &["*.log", "--min-size", "1024", "--newer", "90d"], "*.log >=1KB <90d"),
];

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct ProfileTiming {
    wall_ms: u64,
    daemon_search_ms: u64,
    ipc_ms: u64,
    connect_ms: u64,
    ready_ms: u64,
    records_scanned: String,
    success: bool,
    timed_out: bool,
}

struct PatternResult {
    label: String,
    description: String,
    timings: Vec<ProfileTiming>,
}

struct DriveResults {
    drive: String,
    patterns: Vec<PatternResult>,
}

struct InteractiveConfig {
    bin: PathBuf,
    drives: Vec<String>,
    rounds: usize,
    full_suite: bool,
    extra_patterns: Vec<(String, String)>,
    sources: DataSources,
}

struct DataSources {
    data_dir: Option<PathBuf>,
    drive_files: std::collections::HashMap<String, PathBuf>,
}

impl DataSources {
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
    /// Args for `uffs --daemon start` — only --data-dir / --mft-file (NOT --drive).
    fn daemon_start_args(&self) -> Vec<String> {
        match &self.data_dir {
            Some(d) => vec!["--data-dir".into(), d.to_string_lossy().into_owned()],
            None => vec![], // Windows live: daemon auto-discovers drives
        }
    }
    /// Args for search commands — can include --drive for single-drive queries.
    fn search_args(&self, drive: &str) -> Vec<String> {
        if drive != "ALL" {
            if let Some(mft) = self.drive_files.get(drive) {
                return vec!["--mft-file".into(), mft.to_string_lossy().into_owned()];
            }
            if self.data_dir.is_none() {
                return vec!["--drive".into(), drive.to_string()];
            }
        }
        match &self.data_dir {
            Some(d) => vec!["--data-dir".into(), d.to_string_lossy().into_owned()],
            None => vec![],
        }
    }
    fn available_drives(&self) -> Vec<String> {
        let mut d: Vec<String> = self.drive_files.keys().cloned().collect();
        d.sort();
        d
    }
}

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


// ─── Daemon lifecycle ───────────────────────────────────────────────────────

fn flush() { std::io::stderr().flush().ok(); }

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

fn assert_ready(bin: &PathBuf) -> bool {
    if let Ok(out) = Command::new(bin).args(["--daemon", "status"])
        .stderr(Stdio::null()).output()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        return s.contains("Ready");
    }
    false
}

// ─── Profile extraction ─────────────────────────────────────────────────────

fn extract_ms(line: &str, prefix: &str) -> Option<u64> {
    let idx = line.find(prefix)?;
    let after = &line[idx + prefix.len()..];
    let num_start = after.find(|c: char| c.is_ascii_digit())?;
    let num_end = after[num_start..].find(|c: char| !c.is_ascii_digit())
        .map_or(after.len(), |e| num_start + e);
    after[num_start..num_end].parse().ok()
}

fn parse_profile_stderr(stderr: &str) -> ProfileTiming {
    let mut t = ProfileTiming::default();
    for line in stderr.lines() {
        let s = line.trim();
        if let Some(v) = extract_ms(s, "Connect:") { t.connect_ms = v; }
        if let Some(v) = extract_ms(s, "Await ready:") { t.ready_ms = v; }
        if let Some(v) = extract_ms(s, "Search (IPC):") { t.ipc_ms = v; }
        if s.starts_with("Search:") {
            if let Some(v) = extract_ms(s, "Search:") { t.daemon_search_ms = v; }
            if let (Some(a), Some(b)) = (s.find('('), s.find(" records")) {
                t.records_scanned = s[a+1..b].to_string();
            }
        }
        if s.starts_with("=== TOTAL:") {
            if let Some(v) = extract_ms(s, "TOTAL:") { t.wall_ms = v; }
        }
    }
    t
}

fn run_profiled(bin: &PathBuf, args: &[String]) -> ProfileTiming {
    let t0 = Instant::now();
    let child = Command::new(bin).args(args)
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(_) => return ProfileTiming { success: false, ..Default::default() },
    };
    let output = child.wait_with_output();
    let wall_ms = t0.elapsed().as_millis() as u64;
    let timed_out = wall_ms > TIMEOUT_SECS * 1000;
    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut t = parse_profile_stderr(&stderr);
            if t.wall_ms == 0 { t.wall_ms = wall_ms; }
            t.success = out.status.success();
            t.timed_out = timed_out;
            t
        }
        Err(_) => ProfileTiming { wall_ms, success: false, timed_out, ..Default::default() },
    }
}

// ─── Percentile computation ─────────────────────────────────────────────────

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() { return 0; }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

struct LatencyStats {
    e2e_p50: u64, e2e_p95: u64, e2e_min: u64, e2e_max: u64, e2e_avg: u64,
    daemon_p50: u64, daemon_p95: u64, daemon_avg: u64,
    ok: usize, fail: usize,
}

fn compute_stats(timings: &[ProfileTiming]) -> LatencyStats {
    let mut walls: Vec<u64> = timings.iter()
        .filter(|t| t.success && !t.timed_out).map(|t| t.wall_ms).collect();
    let mut daemon: Vec<u64> = timings.iter()
        .filter(|t| t.success && !t.timed_out).map(|t| t.daemon_search_ms).collect();
    walls.sort();
    daemon.sort();
    let ok = walls.len();
    let fail = timings.iter().filter(|t| !t.success || t.timed_out).count();
    LatencyStats {
        e2e_p50: percentile(&walls, 50.0), e2e_p95: percentile(&walls, 95.0),
        e2e_min: walls.first().copied().unwrap_or(0),
        e2e_max: walls.last().copied().unwrap_or(0),
        e2e_avg: if ok > 0 { walls.iter().sum::<u64>() / ok as u64 } else { 0 },
        daemon_p50: percentile(&daemon, 50.0), daemon_p95: percentile(&daemon, 95.0),
        daemon_avg: if ok > 0 { daemon.iter().sum::<u64>() / ok as u64 } else { 0 },
        ok, fail,
    }
}

// ─── Benchmark execution ────────────────────────────────────────────────────

fn bench_drive(cfg: &InteractiveConfig, drive: &str) -> DriveResults {
    eprintln!("\n━━━ Drive {drive}: Interactive Search ━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let daemon_args = cfg.sources.daemon_start_args();
    let search_src = cfg.sources.search_args(drive);

    // Ensure daemon is HOT — start with daemon args (--data-dir), not --drive.
    eprint!("  Ensuring daemon is running + Ready... "); flush();
    if !assert_ready(&cfg.bin) {
        if !start_and_await_ready(&cfg.bin, &daemon_args) {
            eprintln!("FAILED");
            return DriveResults { drive: drive.into(), patterns: vec![] };
        }
        // Warmup search (uses search args which CAN include --drive).
        let mut warmup: Vec<String> = vec!["*".into(), "--profile".into(), "--limit".into(), "100".into()];
        warmup.extend(search_src.iter().cloned());
        let _ = run_profiled(&cfg.bin, &warmup);
    }
    eprintln!("ready");

    let mut all_patterns: Vec<PatternResult> = Vec::new();

    // Simple patterns (just a search pattern).
    for &(pat, desc) in DEFAULT_PATTERNS {
        eprintln!("  ── {:16} ({desc}) — {} rounds ──", format!("{pat:?}"), cfg.rounds);
        let mut timings = Vec::new();
        for round in 1..=cfg.rounds {
            let mut args: Vec<String> = vec![
                pat.into(), "--profile".into(), "--limit".into(), "100".into(),
            ];
            args.extend(search_src.iter().cloned());
            let t = run_profiled(&cfg.bin, &args);
            let status = if t.timed_out { "T/O" } else if !t.success { "FAIL" }
                else { "OK" };
            if round <= 3 || round == cfg.rounds {
                eprint!("    R{round}: {}ms (daemon {}ms) {status}  ", t.wall_ms, t.daemon_search_ms);
            } else if round == 4 {
                eprint!("...");
            }
            timings.push(t);
        }
        eprintln!();
        all_patterns.push(PatternResult { label: pat.into(), description: desc.into(), timings });
    }

    // Extra user patterns.
    for (pat, desc) in &cfg.extra_patterns {
        eprintln!("  ── {:16} ({desc}) — {} rounds ──", format!("{pat:?}"), cfg.rounds);
        let mut timings = Vec::new();
        for _round in 1..=cfg.rounds {
            let mut args: Vec<String> = vec![
                pat.clone(), "--profile".into(), "--limit".into(), "100".into(),
            ];
            args.extend(search_src.iter().cloned());
            timings.push(run_profiled(&cfg.bin, &args));
        }
        all_patterns.push(PatternResult { label: pat.clone(), description: desc.clone(), timings });
    }

    // Filter patterns (--full-suite).
    if cfg.full_suite {
        for &(label, filter_args, desc) in FILTER_PATTERNS {
            eprintln!("  ── {:16} ({desc}) — {} rounds ──", label, cfg.rounds);
            let mut timings = Vec::new();
            for _round in 1..=cfg.rounds {
                let mut args: Vec<String> = filter_args.iter().map(|s| s.to_string()).collect();
                args.extend(["--profile".into(), "--limit".into(), "100".into()]);
                args.extend(search_src.iter().cloned());
                timings.push(run_profiled(&cfg.bin, &args));
            }
            all_patterns.push(PatternResult {
                label: label.into(), description: desc.into(), timings,
            });
        }
    }

    DriveResults { drive: drive.into(), patterns: all_patterns }
}

// ─── Formatting ─────────────────────────────────────────────────────────────

fn fmt_dur(ms: u64) -> String {
    if ms == 0 { return format!("{:>8}", "—"); }
    if ms < 1_000 { format!("{ms:>5} ms") }
    else if ms < 60_000 { format!("{:>2}.{:>01}s  ", ms / 1000, (ms % 1000) / 100) }
    else { let s = (ms + 500) / 1000; format!("{:>2}m {:02}s", s / 60, s % 60) }
}

// ─── Summary table ──────────────────────────────────────────────────────────

fn print_summary(all_results: &[DriveResults]) {
    const W: usize = 115;
    let bar_t = format!("╔{:═<W$}╗", "");
    let bar_m = format!("╠{:═<W$}╣", "");
    let bar_d = format!("╟{:─<W$}╢", "");
    let bar_b = format!("╚{:═<W$}╝", "");

    eprintln!("\n{bar_t}");
    eprintln!("║{:^W$}║", "INTERACTIVE SEARCH — PERCENTILE LATENCY (ms)");
    eprintln!("{bar_m}");
    eprintln!("║ {:<5} {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>4} {:>4}  ║",
        "Drive", "Pattern", "e2e p50", "e2e p95", "e2e min", "e2e max", "e2e avg",
        "dae p50", "dae p95", "dae avg", "OK", "Fail");
    eprintln!("{bar_m}");

    let mut first_drive = true;
    for dr in all_results {
        if !first_drive { eprintln!("{bar_d}"); }
        first_drive = false;
        for pr in &dr.patterns {
            let s = compute_stats(&pr.timings);
            let label = if pr.label.len() > 16 {
                format!("{}…", &pr.label[..15])
            } else { pr.label.clone() };
            eprintln!("║ {:<5} {:<16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>4} {:>4}  ║",
                format!("{}:", dr.drive), label,
                fmt_dur(s.e2e_p50), fmt_dur(s.e2e_p95),
                fmt_dur(s.e2e_min), fmt_dur(s.e2e_max), fmt_dur(s.e2e_avg),
                fmt_dur(s.daemon_p50), fmt_dur(s.daemon_p95), fmt_dur(s.daemon_avg),
                s.ok, s.fail);
        }
    }
    eprintln!("{bar_b}");
}

// ─── Drive discovery + build ────────────────────────────────────────────────

fn discover_drives(_bin: &PathBuf, sources: &DataSources) -> Vec<String> {
    if sources.data_dir.is_some() {
        let d = sources.available_drives();
        if !d.is_empty() { return d; }
    }
    if cfg!(windows) {
        let mut drives = Vec::new();
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            if std::path::Path::new(&root).exists() { drives.push((letter as char).to_string()); }
        }
        return drives;
    }
    Vec::new()
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

fn ensure_fresh_release_build() -> String {
    let workspace = find_workspace_root();
    let binary_path = workspace.join("target").join("release").join("uffs");
    eprintln!("  Building fresh release binary...");
    let start = Instant::now();
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "uffs-cli"])
        .current_dir(&workspace).status();
    match status {
        Ok(s) if s.success() => eprintln!("  ✅ Build in {:.1}s", start.elapsed().as_secs_f64()),
        _ => { eprintln!("  ❌ Build failed"); std::process::exit(1); }
    }
    binary_path.to_string_lossy().into_owned()
}

fn default_binary() -> String {
    if !cfg!(windows) { return ensure_fresh_release_build(); }
    let home = env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    for c in [format!("{home}\\bin\\uffs.exe"), "target\\release\\uffs.exe".into()] {
        if std::path::Path::new(&c).exists() { return c; }
    }
    "uffs.exe".into()
}

// ─── Arg parsing ────────────────────────────────────────────────────────────

fn parse_args() -> InteractiveConfig {
    let args: Vec<String> = env::args().collect();
    let mut bin: Option<PathBuf> = None;
    let mut drives: Vec<String> = Vec::new();
    let mut rounds = 20usize;
    let mut full_suite = false;
    let mut extra_patterns: Vec<(String, String)> = Vec::new();
    let mut data_dir: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bin" | "-b" => { i += 1; if i < args.len() { bin = Some(PathBuf::from(&args[i])); } }
            "--drives" | "-d" => {
                i += 1;
                if i < args.len() { drives = args[i].split(',').map(|s| s.trim().to_uppercase()).collect(); }
            }
            "--rounds" | "-n" => { i += 1; if i < args.len() { rounds = args[i].parse().unwrap_or(20); } }
            "--full-suite" => full_suite = true,
            "--pattern" | "-p" => {
                i += 1;
                if i < args.len() { extra_patterns.push((args[i].clone(), "user pattern".into())); }
            }
            "--data-dir" => { i += 1; if i < args.len() { data_dir = Some(PathBuf::from(&args[i])); } }
            "--help" | "-h" => {
                eprintln!("UFFS Interactive Search Benchmark (p50/p95)");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --bin PATH          Path to uffs binary");
                eprintln!("  --drives C,D,E      Drives (default: auto)");
                eprintln!("  --rounds N          Rounds per pattern (default: 20)");
                eprintln!("  --full-suite        Include date/size filter patterns");
                eprintln!("  --pattern PAT       Additional patterns (repeatable)");
                eprintln!("  --data-dir PATH     MFT data directory");
                std::process::exit(0);
            }
            other => {
                let p = std::path::Path::new(other);
                if p.exists() && p.is_dir() { data_dir = Some(p.to_path_buf()); }
                else { eprintln!("Unknown argument: {other}"); std::process::exit(1); }
            }
        }
        i += 1;
    }

    if !cfg!(windows) && data_dir.is_none() {
        let home = env::var("HOME").unwrap_or_else(|_| ".".into());
        let default = PathBuf::from(&home).join("uffs_data");
        if default.is_dir() {
            eprintln!("  (defaulting to --data-dir {})", default.display());
            data_dir = Some(default);
        } else {
            eprintln!("ERROR: No data source. Use --data-dir.");
            std::process::exit(1);
        }
    }

    let sources = match data_dir {
        Some(ref dir) => {
            let s = DataSources::from_data_dir(dir);
            eprintln!("  Data dir: {} ({} drives)", dir.display(), s.drive_files.len());
            s
        }
        None => DataSources::empty(),
    };
    let bin = bin.unwrap_or_else(|| PathBuf::from(default_binary()));
    InteractiveConfig { bin, drives, rounds, full_suite, extra_patterns, sources }
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    let mut cfg = parse_args();

    if !cfg.bin.exists() {
        eprintln!("ERROR: uffs binary not found at: {}", cfg.bin.display());
        std::process::exit(1);
    }

    let version = Command::new(&cfg.bin).arg("--version").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    if cfg.drives.is_empty() {
        eprint!("  Auto-discovering drives... "); flush();
        cfg.drives = discover_drives(&cfg.bin, &cfg.sources);
        if cfg.drives.is_empty() {
            eprintln!("FAILED. Use --drives C,D.");
            std::process::exit(1);
        }
        eprintln!("found: {}", cfg.drives.join(", "));
    }

    let n_patterns = DEFAULT_PATTERNS.len() + cfg.extra_patterns.len()
        + if cfg.full_suite { FILTER_PATTERNS.len() } else { 0 };

    const HW: usize = 60;
    eprintln!();
    eprintln!("╔{:═<HW$}╗", "");
    eprintln!("║{:^HW$}║", "UFFS Interactive Search Benchmark");
    eprintln!("╠{:═<HW$}╣", "");
    eprintln!("║  Binary:     {:<w$}║", version, w = HW - 15);
    eprintln!("║  Drives:     {:<w$}║", cfg.drives.join(", "), w = HW - 15);
    eprintln!("║  Patterns:   {:<w$}║", n_patterns, w = HW - 15);
    eprintln!("║  Rounds:     {:<w$}║", cfg.rounds, w = HW - 15);
    eprintln!("║  Full suite: {:<w$}║", if cfg.full_suite { "yes" } else { "no" }, w = HW - 15);
    eprintln!("╚{:═<HW$}╝", "");

    let total_start = Instant::now();
    let mut all_results: Vec<DriveResults> = Vec::new();

    for drive in cfg.drives.clone() {
        all_results.push(bench_drive(&cfg, &drive));
    }

    if cfg.drives.len() > 1 {
        all_results.push(bench_drive(&cfg, "ALL"));
    }

    print_summary(&all_results);

    let total_secs = total_start.elapsed().as_secs();
    eprintln!("\nTotal benchmark time: {}m {}s", total_secs / 60, total_secs % 60);

    ensure_stopped(&cfg.bin);
    eprintln!("🧹 Daemon stopped after benchmark.");
}