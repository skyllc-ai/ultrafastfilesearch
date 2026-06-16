#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! UFFS Bulk Retrieval Benchmark — measures throughput at varying result sizes.
//!
//! Tests the "Bulk retrieval" benchmark class from the public benchmark plan:
//!
//!   | Class          | Example workloads              | Primary metrics              |
//!   |----------------|--------------------------------|------------------------------|
//!   | Bulk retrieval | top 100, 10k, 1M, full export  | rows/sec, completion, peak RAM |
//!
//! For each limit tier, runs the search, counts actual rows returned,
//! calculates rows/sec, and reports completion status (PASS / DNF / OOM / TIMEOUT).
//!
//! # Usage
//!
//! ```powershell
//! rust-script scripts\windows\bulk-retrieval.rs
//! rust-script scripts\windows\bulk-retrieval.rs --data-dir ~/uffs_data
//! rust-script scripts\windows\bulk-retrieval.rs --drives C --rounds 3
//! rust-script scripts\windows\bulk-retrieval.rs --bin C:\tools\uffs.exe
//! rust-script scripts/windows/bulk-retrieval.rs --data-dir ~/uffs_data --tiers 100,10000
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

const TIMEOUT_SECS: u64 = 600; // 10 min per run — bulk can be slow

// ─── Limit tiers ────────────────────────────────────────────────────────────

/// Default tiers: 100, 1k, 10k, 100k, 1M, 0 (unlimited).
const DEFAULT_TIERS: &[u64] = &[100, 1_000, 10_000, 100_000, 1_000_000, 0];

fn tier_label(limit: u64) -> String {
    match limit {
        0 => "ALL".into(),
        n if n >= 1_000_000 => format!("{}M", n / 1_000_000),
        n if n >= 1_000 => format!("{}k", n / 1_000),
        n => format!("{n}"),
    }
}

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct TierResult {
    limit: u64,
    rows_returned: u64,
    wall_ms: u64,
    success: bool,
    timed_out: bool,
    stderr_snippet: String,
}

impl TierResult {
    fn status(&self) -> &'static str {
        if self.timed_out { "TIMEOUT" }
        else if !self.success { "DNF" }
        else { "PASS" }
    }
    fn rows_per_sec(&self) -> f64 {
        if self.wall_ms == 0 { return 0.0; }
        self.rows_returned as f64 / (self.wall_ms as f64 / 1000.0)
    }
}

struct DriveResults {
    drive: String,
    rounds: Vec<Vec<TierResult>>, // [round][tier]
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

struct BulkConfig {
    bin: PathBuf,
    drives: Vec<String>,
    rounds: usize,
    tiers: Vec<u64>,
    sources: DataSources,
    format: String, // csv, json, ndjson
    out_dir: Option<PathBuf>,
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

// ─── Row counting ───────────────────────────────────────────────────────────

/// Count lines in a file (minus header for CSV).
fn count_rows_in_file(path: &std::path::Path, format: &str) -> u64 {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let reader = std::io::BufReader::new(file);
    use std::io::BufRead;
    let count = reader.lines().count() as u64;
    match format {
        "csv" => count.saturating_sub(1), // skip header
        _ => count,
    }
}

/// Run a search and count rows. When `out_file` is Some, uses `--out <file>`
/// to bypass shell pipes; otherwise pipes stdout.
fn run_tier(
    bin: &PathBuf, source_args: &[String], limit: u64, format: &str,
    out_file: Option<&std::path::Path>,
) -> TierResult {
    let mut args: Vec<String> = vec!["*".into()];
    args.extend(source_args.iter().cloned());
    args.extend(["--format".into(), format.into()]);
    if limit > 0 {
        args.extend(["--limit".into(), limit.to_string()]);
    }
    if let Some(path) = out_file {
        args.extend(["--out".into(), path.to_string_lossy().into_owned()]);
    }

    let t0 = Instant::now();
    let child = Command::new(bin).args(&args)
        .stdout(if out_file.is_some() { Stdio::null() } else { Stdio::piped() })
        .stderr(Stdio::piped()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return TierResult {
                limit, rows_returned: 0, wall_ms: 0,
                success: false, timed_out: false,
                stderr_snippet: format!("spawn error: {e}"),
            };
        }
    };

    let output = child.wait_with_output();
    let wall_ms = t0.elapsed().as_millis() as u64;
    let timed_out = wall_ms > TIMEOUT_SECS * 1000;

    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);

            // Count rows: from file if --out was used, otherwise from stdout.
            let rows = if let Some(path) = out_file {
                count_rows_in_file(path, format)
            } else {
                let stdout = String::from_utf8_lossy(&out.stdout);
                match format {
                    "csv" => {
                        let lines: Vec<&str> = stdout.lines().collect();
                        if lines.len() > 1 { (lines.len() - 1) as u64 } else { 0 }
                    }
                    "json" => {
                        stdout.matches("\"Name\"").count().saturating_sub(1) as u64
                    }
                    _ => stdout.lines().count() as u64,
                }
            };

            let snippet: String = stderr.lines().rev().take(3)
                .collect::<Vec<_>>().into_iter().rev()
                .collect::<Vec<_>>().join(" | ");

            TierResult {
                limit, rows_returned: rows, wall_ms,
                success: out.status.success(), timed_out,
                stderr_snippet: snippet,
            }
        }
        Err(e) => TierResult {
            limit, rows_returned: 0, wall_ms,
            success: false, timed_out,
            stderr_snippet: format!("wait error: {e}"),
        },
    }
}

// ─── Benchmark execution ────────────────────────────────────────────────────

fn bench_drive(cfg: &BulkConfig, drive: &str) -> DriveResults {
    eprintln!("\n━━━ Drive {drive}: Bulk Retrieval ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let daemon_args = cfg.sources.daemon_start_args();
    let search_src = cfg.sources.search_args(drive);

    // Ensure daemon is HOT before bulk tests.
    eprint!("  Ensuring daemon is running + Ready... "); flush();
    if !assert_ready(&cfg.bin) {
        if !start_and_await_ready(&cfg.bin, &daemon_args) {
            eprintln!("FAILED — daemon did not reach Ready");
            return DriveResults { drive: drive.into(), rounds: vec![] };
        }
        // Warmup search.
        let warmup_args = {
            let mut a: Vec<String> = vec!["*".into()];
            a.extend(search_src.iter().cloned());
            a.extend(["--limit".into(), "100".into()]);
            a
        };
        let _ = Command::new(&cfg.bin).args(&warmup_args)
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    eprintln!("ready");

    let mut rounds = Vec::new();
    for round in 1..=cfg.rounds {
        eprintln!("  ── Round {round}/{} ──", cfg.rounds);
        let mut tier_results = Vec::new();
        for &limit in &cfg.tiers {
            eprint!("    {:>6}: ", tier_label(limit)); flush();
            // Build --out file path if out_dir is set.
            let out_file = cfg.out_dir.as_ref().map(|dir| {
                dir.join(format!("{}_r{}_{}.{}", drive, round, tier_label(limit), &cfg.format))
            });
            let r = run_tier(&cfg.bin, &search_src, limit, &cfg.format,
                out_file.as_deref());
            let rps = r.rows_per_sec();
            let rps_str = if rps > 1_000_000.0 { format!("{:.1}M/s", rps / 1_000_000.0) }
                else if rps > 1_000.0 { format!("{:.0}k/s", rps / 1_000.0) }
                else { format!("{:.0}/s", rps) };
            eprintln!("{:>10} rows  {:>10} ms  {:>10}  {}",
                fmt_num_u64(r.rows_returned), r.wall_ms, rps_str, r.status());
            tier_results.push(r);
        }
        rounds.push(tier_results);
    }
    DriveResults { drive: drive.into(), rounds }
}


// ─── Formatting ─────────────────────────────────────────────────────────────

fn fmt_dur(ms: u64) -> String {
    if ms < 1_000 { format!("{ms:>8} ms") }
    else if ms < 60_000 { format!("{:>2} s {:>3} ms", ms / 1000, ms % 1000) }
    else { let s = (ms + 500) / 1000; format!("{:>2} m  {:>02} s ", s / 60, s % 60) }
}

fn fmt_num_u64(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (idx, ch) in s.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 { result.push(','); }
        result.push(ch);
    }
    result.chars().rev().collect()
}

fn fmt_rps(rps: f64) -> String {
    if rps > 1_000_000.0 { format!("{:.2}M/s", rps / 1_000_000.0) }
    else if rps > 1_000.0 { format!("{:.1}k/s", rps / 1_000.0) }
    else { format!("{:.0}/s", rps) }
}

// ─── Summary table ──────────────────────────────────────────────────────────

fn print_summary(all_results: &[DriveResults]) {
    const W: usize = 100;
    let bar_t = format!("╔{:═<W$}╗", "");
    let bar_m = format!("╠{:═<W$}╣", "");
    let bar_d = format!("╟{:─<W$}╢", "");
    let bar_b = format!("╚{:═<W$}╝", "");

    eprintln!("\n{bar_t}");
    eprintln!("║{:^W$}║", "BULK RETRIEVAL BENCHMARK");
    eprintln!("{bar_m}");
    eprintln!("║ {:<5} {:<6} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7}    ║",
        "Drive", "Tier", "Rows", "Min ms", "Avg ms", "Max ms", "Avg rows/s", "Status");
    eprintln!("{bar_m}");

    let mut first_drive = true;
    for dr in all_results {
        if !first_drive { eprintln!("{bar_d}"); }
        first_drive = false;

        if dr.rounds.is_empty() {
            eprintln!("║ {:<5} {:<6} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7}    ║",
                format!("{}:", dr.drive), "—", "—", "—", "—", "—", "—", "DNF");
            continue;
        }

        let n_tiers = dr.rounds[0].len();
        for ti in 0..n_tiers {
            let tier_runs: Vec<&TierResult> = dr.rounds.iter()
                .filter_map(|round| round.get(ti))
                .collect();
            let limit = tier_runs.first().map(|t| t.limit).unwrap_or(0);
            let ok_runs: Vec<&TierResult> = tier_runs.iter()
                .filter(|t| t.success && !t.timed_out)
                .copied().collect();

            if ok_runs.is_empty() {
                let status = tier_runs.first().map(|t| t.status()).unwrap_or("DNF");
                eprintln!("║ {:<5} {:<6} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7}    ║",
                    format!("{}:", dr.drive), tier_label(limit), "—", "—", "—", "—", "—", status);
                continue;
            }

            let walls: Vec<u64> = ok_runs.iter().map(|t| t.wall_ms).collect();
            let rows_avg = ok_runs.iter().map(|t| t.rows_returned).sum::<u64>() / ok_runs.len() as u64;
            let min = *walls.iter().min().unwrap_or(&0);
            let max = *walls.iter().max().unwrap_or(&0);
            let avg = walls.iter().sum::<u64>() / walls.len() as u64;
            let rps = if avg > 0 { rows_avg as f64 / (avg as f64 / 1000.0) } else { 0.0 };

            eprintln!("║ {:<5} {:<6} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7}    ║",
                format!("{}:", dr.drive), tier_label(limit),
                fmt_num_u64(rows_avg), fmt_dur(min), fmt_dur(avg), fmt_dur(max),
                fmt_rps(rps), "PASS");
        }
    }
    eprintln!("{bar_b}");
}

// ─── Drive discovery ────────────────────────────────────────────────────────

fn discover_drives(_bin: &PathBuf, sources: &DataSources) -> Vec<String> {
    if sources.data_dir.is_some() {
        let d = sources.available_drives();
        if !d.is_empty() { return d; }
    }
    if cfg!(windows) {
        return discover_windows_ntfs_drives();
    }
    Vec::new()
}

fn discover_windows_ntfs_drives() -> Vec<String> {
    if let Ok(out) = Command::new("wmic")
        .args(["logicaldisk", "where", "DriveType=2 or DriveType=3", "get", "DeviceID,FileSystem", "/format:csv"])
        .stdout(Stdio::piped()).stderr(Stdio::null()).output()
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut drives = Vec::new();
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 3 && parts[2].trim().eq_ignore_ascii_case("NTFS") {
                    if let Some(ch) = parts[1].trim().chars().next() {
                        if ch.is_ascii_uppercase() { drives.push(ch.to_string()); }
                    }
                }
            }
            if !drives.is_empty() { drives.sort(); return drives; }
        }
    }
    let mut drives = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if std::path::Path::new(&root).exists() { drives.push((letter as char).to_string()); }
    }
    drives
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
        Ok(s) if s.success() => {
            eprintln!("  ✅ Build completed in {:.1}s", start.elapsed().as_secs_f64());
        }
        _ => { eprintln!("  ❌ cargo build --release failed"); std::process::exit(1); }
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

fn parse_args() -> BulkConfig {
    let args: Vec<String> = env::args().collect();
    let mut bin: Option<PathBuf> = None;
    let mut drives: Vec<String> = Vec::new();
    let mut rounds = 3usize;
    let mut tiers: Vec<u64> = Vec::new();
    let mut data_dir: Option<PathBuf> = None;
    let mut format = "csv".to_string();
    let mut out_dir: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bin" | "-b" => { i += 1; if i < args.len() { bin = Some(PathBuf::from(&args[i])); } }
            "--drives" | "-d" => {
                i += 1;
                if i < args.len() { drives = args[i].split(',').map(|s| s.trim().to_uppercase()).collect(); }
            }
            "--rounds" | "-n" => { i += 1; if i < args.len() { rounds = args[i].parse().unwrap_or(3); } }
            "--tiers" => {
                i += 1;
                if i < args.len() {
                    tiers = args[i].split(',')
                        .filter_map(|s| s.trim().parse().ok())
                        .collect();
                }
            }
            "--format" => { i += 1; if i < args.len() { format = args[i].clone(); } }
            "--data-dir" => { i += 1; if i < args.len() { data_dir = Some(PathBuf::from(&args[i])); } }
            "--out-dir" => { i += 1; if i < args.len() { out_dir = Some(PathBuf::from(&args[i])); } }
            "--help" | "-h" => {
                eprintln!("UFFS Bulk Retrieval Benchmark");
                eprintln!();
                eprintln!("Usage: rust-script bulk-retrieval.rs [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --bin PATH          Path to uffs binary");
                eprintln!("  --drives C,D,E      Drives to benchmark (default: auto-discover)");
                eprintln!("  --rounds N          Rounds per tier (default: 3)");
                eprintln!("  --tiers 100,10000   Comma-separated limit tiers (default: 100,1k,10k,100k,1M,ALL)");
                eprintln!("  --format csv|json   Output format to test (default: csv)");
                eprintln!("  --data-dir PATH     MFT data directory");
                eprintln!("  --out-dir PATH      Write uffs output to files (bypasses pipe)");
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

    if tiers.is_empty() { tiers = DEFAULT_TIERS.to_vec(); }

    // Smart default for data source on non-Windows.
    if !cfg!(windows) && data_dir.is_none() {
        let home = env::var("HOME").unwrap_or_else(|_| ".".into());
        let default = PathBuf::from(&home).join("uffs_data");
        if default.is_dir() {
            eprintln!("  (defaulting to --data-dir {})", default.display());
            data_dir = Some(default);
        } else {
            eprintln!("ERROR: No data source given and ~/uffs_data not found.");
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
    // Create out_dir if specified.
    if let Some(ref dir) = out_dir {
        let _ = std::fs::create_dir_all(dir);
        eprintln!("  Output dir: {}", dir.display());
    }
    BulkConfig { bin, drives, rounds, tiers, sources, format, out_dir }
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
            eprintln!("FAILED. Use --drives C,D to specify.");
            std::process::exit(1);
        }
        eprintln!("found: {}", cfg.drives.join(", "));
    }

    // Header.
    const HW: usize = 60;
    eprintln!();
    eprintln!("╔{:═<HW$}╗", "");
    eprintln!("║{:^HW$}║", "UFFS Bulk Retrieval Benchmark");
    eprintln!("╠{:═<HW$}╣", "");
    eprintln!("║  Binary:   {:<w$}║", version, w = HW - 13);
    eprintln!("║  Drives:   {:<w$}║", cfg.drives.join(", "), w = HW - 13);
    let tier_str = cfg.tiers.iter().map(|t| tier_label(*t)).collect::<Vec<_>>().join(", ");
    eprintln!("║  Tiers:    {:<w$}║", tier_str, w = HW - 13);
    eprintln!("║  Rounds:   {:<w$}║", cfg.rounds, w = HW - 13);
    eprintln!("║  Format:   {:<w$}║", cfg.format, w = HW - 13);
    eprintln!("╚{:═<HW$}╝", "");

    let total_start = Instant::now();
    let mut all_results: Vec<DriveResults> = Vec::new();

    for drive in cfg.drives.clone() {
        let results = bench_drive(&cfg, &drive);
        all_results.push(results);
    }

    // If multiple drives, also benchmark ALL.
    if cfg.drives.len() > 1 {
        let results = bench_drive(&cfg, "ALL");
        all_results.push(results);
    }

    print_summary(&all_results);

    let total_secs = total_start.elapsed().as_secs();
    eprintln!("\nTotal benchmark time: {}m {}s", total_secs / 60, total_secs % 60);

    ensure_stopped(&cfg.bin);
    eprintln!("🧹 Daemon stopped after benchmark.");
}