#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! UFFS Scale Ceiling Benchmark — tests PASS/DNF at increasing corpus sizes.
//!
//! Tests the "Scale ceiling" benchmark class from the public benchmark plan:
//!
//!   | Class         | Example workloads              | Primary metrics          |
//!   |---------------|--------------------------------|--------------------------|
//!   | Scale ceiling | 1M, 5M, 10M, 25M, 50M corpora | PASS/DNF + failure mode  |
//!
//! Creates synthetic corpora by **copying the largest available MFT file** into
//! multiple `drive_X/` directories under a temporary data dir. Each copy gets a
//! unique drive letter (A–Z), so UFFS treats them as separate drives.
//!
//! For example, if your biggest MFT file has ~8M records:
//!   - 1 copy  =  ~8M records
//!   - 3 copies = ~24M records
//!   - 7 copies = ~56M records
//!
//! The script selects the number of copies needed to approximate each target tier.
//!
//! For each tier, it measures:
//!   - **COLD** (from raw MFT) — daemon killed, cache deleted, full load
//!   - **WARM CACHE** — daemon killed, restart from cache
//!   - **HOT** — query against running daemon
//!
//! Result for each tier: **PASS** (all 3 phases completed) or **DNF** (timeout,
//! crash, OOM) with failure mode details.
//!
//! # Usage
//!
//! ```powershell
//! rust-script scripts\windows\scale-ceiling.rs --data-dir ~/uffs_data
//! rust-script scripts\windows\scale-ceiling.rs --data-dir ~/uffs_data --tiers 5000000,25000000
//! rust-script scripts/windows/scale-ceiling.rs ~/uffs_data --timeout 600
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

/// Default target tiers (in records).
const DEFAULT_TIERS: &[u64] = &[1_000_000, 5_000_000, 10_000_000, 25_000_000, 50_000_000];

/// Maximum timeout per phase (seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Available "fake" drive letters for synthetic copies.
/// We skip letters that might conflict with real drives on Windows (A–G).
const SYNTH_LETTERS: &[char] = &[
    'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T',
    'U', 'V', 'W', 'X', 'Y', 'Z',
];

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct PhaseOutcome {
    phase: &'static str,
    wall_ms: u64,
    success: bool,
    timed_out: bool,
    failure_mode: String,
}

struct TierOutcome {
    target_records: u64,
    approx_records: u64,
    n_copies: usize,
    phases: Vec<PhaseOutcome>,
}

impl TierOutcome {
    fn overall_status(&self) -> &'static str {
        if self.phases.iter().all(|p| p.success && !p.timed_out) { "PASS" }
        else if self.phases.iter().any(|p| p.timed_out) { "TIMEOUT" }
        else { "DNF" }
    }
    fn failure_detail(&self) -> String {
        for p in &self.phases {
            if !p.success || p.timed_out {
                return format!("{}: {}", p.phase, p.failure_mode);
            }
        }
        String::new()
    }
}

struct ScaleConfig {
    bin: PathBuf,
    source_data_dir: PathBuf,
    tiers: Vec<u64>,
    timeout_secs: u64,
}

struct MftSource {
    drive_letter: String,
    path: PathBuf,
    file_size: u64,
    approx_records: u64,
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

fn delete_cache() {
    for (var, sub) in [("LOCALAPPDATA", "uffs/cache"), ("TEMP", "uffs_index_cache")] {
        if let Ok(base) = env::var(var) {
            let p = PathBuf::from(base).join(sub);
            if p.exists() { let _ = std::fs::remove_dir_all(&p); }
        }
    }
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

// ─── MFT discovery + record estimation ──────────────────────────────────────

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

/// Discover all MFT files in a data dir and estimate record counts.
/// Estimate: ~1 KB per MFT record is a reasonable ballpark for raw MFT files.
fn discover_sources(data_dir: &std::path::Path) -> Vec<MftSource> {
    let mut sources = Vec::new();
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if let Some(letter) = name.strip_prefix("drive_") {
                if letter.len() == 1 && letter.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                    if let Some(mft_path) = find_best_mft_file(&path) {
                        let file_size = std::fs::metadata(&mft_path)
                            .map(|m| m.len()).unwrap_or(0);
                        // MFT record size is typically 1024 bytes.
                        let approx_records = file_size / 1024;
                        sources.push(MftSource {
                            drive_letter: letter.to_uppercase(),
                            path: mft_path,
                            file_size,
                            approx_records,
                        });
                    }
                }
            }
        }
    }
    sources.sort_by(|a, b| b.file_size.cmp(&a.file_size)); // largest first
    sources
}

// ─── Synthetic corpus creation ──────────────────────────────────────────────

/// Create a temporary data dir with N copies of the largest MFT file.
/// Returns (temp_dir_path, actual_number_of_copies).
fn create_synthetic_corpus(
    source: &MftSource,
    n_copies: usize,
    temp_base: &std::path::Path,
    tier_label: &str,
) -> PathBuf {
    let corpus_dir = temp_base.join(format!("scale_{tier_label}"));
    let _ = std::fs::create_dir_all(&corpus_dir);

    let ext = source.path.extension()
        .and_then(|e| e.to_str()).unwrap_or("bin");

    for i in 0..n_copies {
        if i >= SYNTH_LETTERS.len() { break; }
        let letter = SYNTH_LETTERS[i];
        let drive_dir = corpus_dir.join(format!("drive_{}", letter.to_ascii_lowercase()));
        let _ = std::fs::create_dir_all(&drive_dir);
        let dest = drive_dir.join(format!("{letter}_mft.{ext}"));
        if !dest.exists() {
            eprint!("    Copying MFT → drive_{} ({:.1} GB)... ",
                letter.to_ascii_lowercase(),
                source.file_size as f64 / 1_073_741_824.0);
            flush();
            match std::fs::copy(&source.path, &dest) {
                Ok(_) => eprintln!("done"),
                Err(e) => eprintln!("ERROR: {e}"),
            }
        }
    }
    corpus_dir
}

/// How many copies needed to approximate a target record count?
fn copies_needed(records_per_copy: u64, target: u64) -> usize {
    if records_per_copy == 0 { return 1; }
    let n = (target + records_per_copy - 1) / records_per_copy;
    (n as usize).max(1).min(SYNTH_LETTERS.len())
}

// ─── Phase execution ────────────────────────────────────────────────────────

fn run_phase(
    bin: &PathBuf, data_dir: &std::path::Path, phase: &'static str, timeout: u64,
) -> PhaseOutcome {
    let args: Vec<String> = vec![
        "*".into(), "--data-dir".into(), data_dir.to_string_lossy().into_owned(),
        "--limit".into(), "100".into(), "--profile".into(),
    ];

    let t0 = Instant::now();
    let child = Command::new(bin).args(&args)
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return PhaseOutcome {
                phase, wall_ms: 0, success: false, timed_out: false,
                failure_mode: format!("spawn error: {e}"),
            };
        }
    };

    let output = child.wait_with_output();
    let wall_ms = t0.elapsed().as_millis() as u64;
    let timed_out = wall_ms > timeout * 1000;

    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let failure_mode = if timed_out {
                "exceeded timeout".into()
            } else if !out.status.success() {
                // Extract last meaningful error line.
                stderr.lines().rev()
                    .find(|l| l.contains("ERROR") || l.contains("error") || l.contains("panic"))
                    .unwrap_or("non-zero exit")
                    .to_string()
            } else {
                String::new()
            };
            PhaseOutcome { phase, wall_ms, success: out.status.success(), timed_out, failure_mode }
        }
        Err(e) => PhaseOutcome {
            phase, wall_ms, success: false, timed_out,
            failure_mode: format!("wait error: {e}"),
        },
    }
}

// ─── Tier benchmarking ──────────────────────────────────────────────────────

fn tier_label(n: u64) -> String {
    if n >= 1_000_000 { format!("{}M", n / 1_000_000) }
    else if n >= 1_000 { format!("{}k", n / 1_000) }
    else { format!("{n}") }
}

fn fmt_dur(ms: u64) -> String {
    if ms == 0 { return "—".into(); }
    if ms < 1_000 { format!("{ms} ms") }
    else if ms < 60_000 { format!("{:.1}s", ms as f64 / 1000.0) }
    else { let s = (ms + 500) / 1000; format!("{}m {:02}s", s / 60, s % 60) }
}

fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (idx, ch) in s.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 { result.push(','); }
        result.push(ch);
    }
    result.chars().rev().collect()
}

fn bench_tier(
    cfg: &ScaleConfig,
    source: &MftSource,
    target: u64,
    temp_base: &std::path::Path,
) -> TierOutcome {
    let label = tier_label(target);
    let n = copies_needed(source.approx_records, target);
    let approx = source.approx_records * n as u64;

    eprintln!("\n━━━ Tier {label} — target {} records, {} copies ≈ {} records ━━━",
        fmt_num(target), n, fmt_num(approx));

    // Create synthetic corpus.
    eprint!("  Creating synthetic corpus... "); flush();
    let corpus = create_synthetic_corpus(source, n, temp_base, &label);
    eprintln!("ready at {}", corpus.display());

    let mut phases = Vec::new();

    // ── Phase 1: COLD (kill daemon, delete cache, run from raw MFT) ──
    eprintln!("  ── COLD ──");
    ensure_stopped(&cfg.bin);
    delete_cache();
    eprint!("    Running... "); flush();
    let cold = run_phase(&cfg.bin, &corpus, "COLD", cfg.timeout_secs);
    eprintln!("{} — {}", fmt_dur(cold.wall_ms), if cold.success { "PASS" } else { &cold.failure_mode });
    phases.push(cold);

    // ── Phase 2: WARM CACHE (kill daemon, keep cache, restart) ──
    eprintln!("  ── WARM CACHE ──");
    ensure_stopped(&cfg.bin);
    eprint!("    Running... "); flush();
    let warm = run_phase(&cfg.bin, &corpus, "WARM", cfg.timeout_secs);
    eprintln!("{} — {}", fmt_dur(warm.wall_ms), if warm.success { "PASS" } else { &warm.failure_mode });
    phases.push(warm);

    // ── Phase 3: HOT (query against already-running daemon) ──
    eprintln!("  ── HOT ──");
    // Daemon should still be running from WARM phase.
    if !assert_ready(&cfg.bin) {
        // If warm failed, try to start fresh.
        let start_args: Vec<String> = vec!["--daemon".into(), "start".into(),
            "--data-dir".into(), corpus.to_string_lossy().into_owned()];
        let _ = Command::new(&cfg.bin).args(&start_args)
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if assert_ready(&cfg.bin) { break; }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    eprint!("    Running... "); flush();
    let hot = run_phase(&cfg.bin, &corpus, "HOT", cfg.timeout_secs);
    eprintln!("{} — {}", fmt_dur(hot.wall_ms), if hot.success { "PASS" } else { &hot.failure_mode });
    phases.push(hot);

    // Stop daemon before next tier.
    ensure_stopped(&cfg.bin);

    TierOutcome { target_records: target, approx_records: approx, n_copies: n, phases }
}


// ─── Summary table ──────────────────────────────────────────────────────────

fn print_summary(outcomes: &[TierOutcome]) {
    const W: usize = 105;
    let bar_t = format!("╔{:═<W$}╗", "");
    let bar_m = format!("╠{:═<W$}╣", "");
    let bar_b = format!("╚{:═<W$}╝", "");

    eprintln!("\n{bar_t}");
    eprintln!("║{:^W$}║", "SCALE CEILING BENCHMARK — PASS / DNF");
    eprintln!("{bar_m}");
    eprintln!("║ {:>8} {:>12} {:>6} {:>12} {:>12} {:>12} {:>8} {:<24}  ║",
        "Target", "≈ Records", "Copies", "COLD", "WARM", "HOT", "Result", "Failure");
    eprintln!("{bar_m}");

    for t in outcomes {
        let cold = t.phases.iter().find(|p| p.phase == "COLD");
        let warm = t.phases.iter().find(|p| p.phase == "WARM");
        let hot  = t.phases.iter().find(|p| p.phase == "HOT");
        eprintln!("║ {:>8} {:>12} {:>6} {:>12} {:>12} {:>12} {:>8} {:<24}  ║",
            tier_label(t.target_records),
            fmt_num(t.approx_records),
            t.n_copies,
            cold.map_or("—".into(), |p| fmt_dur(p.wall_ms)),
            warm.map_or("—".into(), |p| fmt_dur(p.wall_ms)),
            hot.map_or("—".into(), |p| fmt_dur(p.wall_ms)),
            t.overall_status(),
            {
                let d = t.failure_detail();
                if d.len() > 24 { format!("{}…", &d[..23]) } else { d }
            });
    }
    eprintln!("{bar_b}");
}

// ─── Build helper + arg parsing ─────────────────────────────────────────────

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

fn parse_args() -> ScaleConfig {
    let args: Vec<String> = env::args().collect();
    let mut bin: Option<PathBuf> = None;
    let mut tiers: Vec<u64> = Vec::new();
    let mut data_dir: Option<PathBuf> = None;
    let mut timeout = DEFAULT_TIMEOUT_SECS;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bin" | "-b" => { i += 1; if i < args.len() { bin = Some(PathBuf::from(&args[i])); } }
            "--tiers" => {
                i += 1;
                if i < args.len() {
                    tiers = args[i].split(',').filter_map(|s| s.trim().parse().ok()).collect();
                }
            }
            "--timeout" => { i += 1; if i < args.len() { timeout = args[i].parse().unwrap_or(DEFAULT_TIMEOUT_SECS); } }
            "--data-dir" => { i += 1; if i < args.len() { data_dir = Some(PathBuf::from(&args[i])); } }
            "--help" | "-h" => {
                eprintln!("UFFS Scale Ceiling Benchmark");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --bin PATH          Path to uffs binary");
                eprintln!("  --data-dir PATH     Source MFT data directory (required)");
                eprintln!("  --tiers 1M,5M,...   Target tiers (default: 1M,5M,10M,25M,50M)");
                eprintln!("  --timeout SECS      Per-phase timeout (default: 600)");
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

    if data_dir.is_none() {
        if !cfg!(windows) {
            let home = env::var("HOME").unwrap_or_else(|_| ".".into());
            let default = PathBuf::from(&home).join("uffs_data");
            if default.is_dir() { data_dir = Some(default); }
        }
        if data_dir.is_none() {
            eprintln!("ERROR: --data-dir is required for scale-ceiling benchmark.");
            std::process::exit(1);
        }
    }

    let bin = bin.unwrap_or_else(|| PathBuf::from(default_binary()));
    ScaleConfig { bin, source_data_dir: data_dir.unwrap_or_default(), tiers, timeout_secs: timeout }
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    let cfg = parse_args();

    if !cfg.bin.exists() {
        eprintln!("ERROR: uffs binary not found at: {}", cfg.bin.display());
        std::process::exit(1);
    }

    let version = Command::new(&cfg.bin).arg("--version").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    // Discover source MFTs.
    let sources = discover_sources(&cfg.source_data_dir);
    if sources.is_empty() {
        eprintln!("ERROR: No MFT files found in {}", cfg.source_data_dir.display());
        std::process::exit(1);
    }

    let biggest = &sources[0];
    let total_existing: u64 = sources.iter().map(|s| s.approx_records).sum();

    const HW: usize = 65;
    eprintln!();
    eprintln!("╔{:═<HW$}╗", "");
    eprintln!("║{:^HW$}║", "UFFS Scale Ceiling Benchmark");
    eprintln!("╠{:═<HW$}╣", "");
    eprintln!("║  Binary:      {:<w$}║", version, w = HW - 16);
    eprintln!("║  Source dir:  {:<w$}║", cfg.source_data_dir.display(), w = HW - 16);
    eprintln!("║  Drives:      {:<w$}║", sources.len(), w = HW - 16);
    eprintln!("║  Biggest:     {} ({} records, {:.1} GB){:<w$}║",
        biggest.drive_letter, fmt_num(biggest.approx_records),
        biggest.file_size as f64 / 1_073_741_824.0,
        "", w = HW.saturating_sub(60));
    eprintln!("║  Total real:  {} records{:<w$}║",
        fmt_num(total_existing), "", w = HW.saturating_sub(40));
    let tier_str = cfg.tiers.iter().map(|t| tier_label(*t)).collect::<Vec<_>>().join(", ");
    eprintln!("║  Tiers:       {:<w$}║", tier_str, w = HW - 16);
    eprintln!("║  Timeout:     {} s / phase{:<w$}║", cfg.timeout_secs, "", w = HW.saturating_sub(35));
    eprintln!("╚{:═<HW$}╝", "");

    // Temp directory for synthetic corpora — sibling of source data dir.
    let temp_base = cfg.source_data_dir.parent()
        .unwrap_or(&cfg.source_data_dir)
        .join("uffs_scale_bench");
    let _ = std::fs::create_dir_all(&temp_base);
    eprintln!("  Synthetic corpora at: {}", temp_base.display());

    let total_start = Instant::now();
    let mut outcomes: Vec<TierOutcome> = Vec::new();

    for &target in &cfg.tiers {
        let outcome = bench_tier(&cfg, biggest, target, &temp_base);
        let status = outcome.overall_status();
        outcomes.push(outcome);

        // If DNF, still continue to next tier — "DNF is a result".
        if status != "PASS" {
            eprintln!("  ⚠ Tier {} → {status} — continuing to next tier.", tier_label(target));
        }
    }

    print_summary(&outcomes);

    let total_secs = total_start.elapsed().as_secs();
    eprintln!("\nTotal benchmark time: {}m {}s", total_secs / 60, total_secs % 60);

    // Cleanup.
    ensure_stopped(&cfg.bin);
    eprintln!("🧹 Daemon stopped.");
    eprintln!("  ℹ Synthetic corpora NOT deleted: {}", temp_base.display());
    eprintln!("    Delete manually when done:  rm -rf {}", temp_base.display());
}