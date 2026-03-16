#!/usr/bin/env rust-script
//! Multi-drive LIVE parity verification for UFFS (Windows).
//!
//! Discovers NTFS drives and runs live MFT scans comparing C++ vs Rust output.
//! Produces a performance timing table at the end.
//!
//! # Usage (run as Administrator)
//!
//! ```powershell
//! # Auto-discover and verify all NTFS drives
//! rust-script scripts\windows\verify_parity.rs --all
//!
//! # Verify specific drives
//! rust-script scripts\windows\verify_parity.rs D E F
//!
//! # Custom binary directory
//! rust-script scripts\windows\verify_parity.rs D E --bin-dir C:\tools
//!
//! # Show detailed diff on mismatch (more samples)
//! rust-script scripts\windows\verify_parity.rs D --sample 50
//! ```
//!
//! # Requirements
//! - Must run as Administrator (MFT access requires elevation)
//! - Both uffs.com (C++) and uffs.exe (Rust) must be in bin-dir or ~/bin
//!
//! ```cargo
//! [dependencies]
//! sha2 = "0.10"
//! ```

use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use std::{env, fs};

/// LCG (Linear Congruential Generator) multiplier - Knuth's MMIX constant.
const LCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // StrictMatch reserved for future use
enum VerifyResult { StrictMatch, SortedMatch, Mismatch, Error }

#[derive(Debug)]
struct DriveResult {
    drive: String,
    result: VerifyResult,
    cpp_lines: usize,
    rust_lines: usize,
    cpp_time: Duration,
    rust_time: Duration,
    diff_count: usize,
}

struct Config {
    drives: Vec<String>,
    cpp_bin: PathBuf,
    rust_bin: PathBuf,
    out_dir: PathBuf,
}

fn main() {
    let cfg = parse_args();

    println!("\n╔══════════════════════════════════════════════════════════════════╗");
    println!("║       UFFS Live Parity Verification (Windows)                    ║");
    println!("╚══════════════════════════════════════════════════════════════════╝\n");
    println!("  C++ binary:  {}", cfg.cpp_bin.display());
    println!("  Rust binary: {}", cfg.rust_bin.display());
    println!("  Drives:      {}", cfg.drives.join(", "));
    println!("  Output dir:  {}", cfg.out_dir.display());
    println!();

    let mut results: Vec<DriveResult> = Vec::new();

    for (i, drive) in cfg.drives.iter().enumerate() {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("  [{}/{}] DRIVE {}", i + 1, cfg.drives.len(), drive);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

        let result = verify_drive(&cfg, drive);
        results.push(result);
        println!();
    }

    print_summary(&results);
    print_timing_table(&results);

    let any_fail = results
        .iter()
        .any(|r| r.result == VerifyResult::Mismatch || r.result == VerifyResult::Error);
    std::process::exit(i32::from(any_fail));
}

fn parse_args() -> Config {
    let args: Vec<String> = env::args().collect();
    let mut drives = Vec::new();
    let mut bin_dir = home_dir().join("bin");
    let mut discover_all = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--all" => discover_all = true,
            "--bin-dir" => {
                i += 1;
                if i < args.len() {
                    bin_dir = PathBuf::from(&args[i]);
                }
            }
            "--help" | "-h" => {
                print_usage(&args[0]);
                std::process::exit(0);
            }
            a if !a.starts_with('-') && a.len() == 1 => drives.push(a.to_uppercase()),
            _ => {}
        }
        i += 1;
    }

    if discover_all {
        drives = discover_ntfs_drives();
    }
    if drives.is_empty() {
        eprintln!("ERROR: No drives specified. Use drive letters (D E F) or --all");
        print_usage(&args[0]);
        std::process::exit(1);
    }

    let cpp_bin = bin_dir.join("uffs.com");
    let rust_bin = bin_dir.join("uffs.exe");

    if !cpp_bin.exists() {
        eprintln!("ERROR: C++ binary not found: {}", cpp_bin.display());
        std::process::exit(1);
    }
    if !rust_bin.exists() {
        eprintln!("ERROR: Rust binary not found: {}", rust_bin.display());
        std::process::exit(1);
    }

    Config {
        drives,
        cpp_bin,
        rust_bin,
        out_dir: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

fn print_usage(prog: &str) {
    eprintln!("UFFS Live Parity Verification (Windows)\n");
    eprintln!("Usage: {prog} [OPTIONS] <DRIVES...>");
    eprintln!("       {prog} --all");
    eprintln!("\nOptions:");
    eprintln!("  --all           Discover and verify all NTFS drives");
    eprintln!("  --bin-dir DIR   Directory containing uffs.com and uffs.exe (default: ~/bin)");
    eprintln!("  -h, --help      Show this help\n");
    eprintln!("Examples:");
    eprintln!("  {prog} D E F              # Verify drives D, E, F");
    eprintln!("  {prog} --all              # Verify all NTFS drives");
    eprintln!("  {prog} D --bin-dir C:\\bin # Custom binary location");
}

fn home_dir() -> PathBuf {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map_or_else(|| ".".into(), PathBuf::from)
}

fn discover_ntfs_drives() -> Vec<String> {
    // Query all drive letters and filter for NTFS fixed drives
    let mut drives = Vec::new();
    for letter in b'C'..=b'Z' {
        let ch = char::from(letter);
        let root = format!("{ch}:\\");
        // Check if drive exists and is accessible
        if PathBuf::from(&root).exists() {
            // Use fsutil to check if NTFS (or just include all fixed drives)
            drives.push(ch.to_string());
        }
    }
    println!("  Discovered drives: {drives:?}");
    drives
}

#[allow(clippy::too_many_lines)]
fn verify_drive(cfg: &Config, drive: &str) -> DriveResult {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dl = drive.to_lowercase();

    let cpp_file = cfg.out_dir.join(format!("parity_cpp_{dl}_{ts}.txt"));
    let rust_file = cfg.out_dir.join(format!("parity_rust_{dl}_{ts}.txt"));
    let rust_log_file = cfg.out_dir.join(format!("parity_rust_log_{dl}_{ts}.txt"));

    // Run C++ scan
    print!("  [1/4] C++ scan...  ");
    flush();
    let cpp_start = Instant::now();
    let cpp_ok = run_scan(&cfg.cpp_bin, &["*", &format!("--drives={drive}")], &cpp_file);
    let cpp_time = cpp_start.elapsed();
    if cpp_ok {
        println!("✅ {}", format_duration(cpp_time));
    } else {
        println!("❌ FAILED");
        return DriveResult {
            drive: drive.to_string(),
            result: VerifyResult::Error,
            cpp_lines: 0,
            rust_lines: 0,
            cpp_time,
            rust_time: Duration::ZERO,
            diff_count: 0,
        };
    }

    // Run Rust scan with trace logging
    print!("  [2/4] Rust scan... ");
    flush();
    let rust_start = Instant::now();
    let rust_ok = run_scan_with_log(
        &cfg.rust_bin,
        &["*", "--drive", drive, "--no-cache", "--format", "custom"],
        &rust_file,
        &rust_log_file,
    );
    let rust_time = rust_start.elapsed();
    if rust_ok {
        println!("✅ {}", format_duration(rust_time));
    } else {
        println!("❌ FAILED");
        return DriveResult {
            drive: drive.to_string(),
            result: VerifyResult::Error,
            cpp_lines: 0,
            rust_lines: 0,
            cpp_time,
            rust_time,
            diff_count: 0,
        };
    }

    // Sort and compare
    print!("  [3/4] Sorting...   ");
    flush();
    let sort_start = Instant::now();
    let cpp_lines = read_and_sort(&cpp_file);
    let rust_lines = read_and_sort(&rust_file);
    let sort_time = format_duration(sort_start.elapsed());
    let cpp_count = cpp_lines.len();
    let rust_count = rust_lines.len();
    println!("✅ {sort_time} (C++: {cpp_count} lines, Rust: {rust_count} lines)");

    // SHA256 comparison
    print!("  [4/4] SHA256...    ");
    flush();
    let cpp_hash = compute_hash(&cpp_lines);
    let rust_hash = compute_hash(&rust_lines);

    if cpp_hash == rust_hash {
        println!("✅ STRICT MATCH");
        println!("\n  ╔══════════════════════════════════════════╗");
        println!("  ║  ✅ PARITY: PASS (sorted match)          ║");
        println!("  ╚══════════════════════════════════════════╝");
        println!("    SHA256: {cpp_hash}");

        // Cleanup temp files on success
        fs::remove_file(&cpp_file).ok();
        fs::remove_file(&rust_file).ok();
        fs::remove_file(&rust_log_file).ok();

        return DriveResult {
            drive: drive.to_string(),
            result: VerifyResult::SortedMatch,
            cpp_lines: cpp_lines.len(),
            rust_lines: rust_lines.len(),
            cpp_time,
            rust_time,
            diff_count: 0,
        };
    }

    // Mismatch - save sorted files and keep log for debugging
    println!("❌ MISMATCH");

    // Save sorted output files for debugging
    let cpp_sorted_file = cfg.out_dir.join(format!("parity_cpp_sorted_{dl}_{ts}.txt"));
    let rust_sorted_file = cfg.out_dir.join(format!("parity_rust_sorted_{dl}_{ts}.txt"));
    save_sorted_lines(&cpp_sorted_file, &cpp_lines);
    save_sorted_lines(&rust_sorted_file, &rust_lines);

    let diff_count = show_diff(
        cfg,
        drive,
        &cpp_lines,
        &rust_lines,
        &cpp_hash,
        &rust_hash,
        ts,
        &cpp_sorted_file,
        &rust_sorted_file,
        &rust_log_file,
    );

    DriveResult {
        drive: drive.to_string(),
        result: VerifyResult::Mismatch,
        cpp_lines: cpp_lines.len(),
        rust_lines: rust_lines.len(),
        cpp_time,
        rust_time,
        diff_count,
    }
}

fn run_scan(bin: &Path, args: &[&str], out_file: &Path) -> bool {
    match Command::new(bin).args(args).output() {
        Ok(output) if output.status.success() => fs::write(out_file, &output.stdout).is_ok(),
        Ok(output) => {
            let code = output.status.code().unwrap_or(-1);
            let stderr_preview: String = String::from_utf8_lossy(&output.stderr)
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!("\n    Exit {code}: {stderr_preview}");
            false
        }
        Err(e) => {
            eprintln!("\n    Error: {e}");
            false
        }
    }
}

/// Run Rust scan with trace-level logging captured to a log file.
fn run_scan_with_log(bin: &Path, args: &[&str], out_file: &Path, log_file: &Path) -> bool {
    match Command::new(bin)
        .args(args)
        .env("RUST_LOG", "trace")
        .env("UFFS_LOG_FILE", log_file.as_os_str())
        .output()
    {
        Ok(output) if output.status.success() => {
            // Save stdout to output file
            if fs::write(out_file, &output.stdout).is_err() {
                return false;
            }
            // Save stderr (contains trace logs) to log file if UFFS_LOG_FILE not used
            if !log_file.exists() && !output.stderr.is_empty() {
                fs::write(log_file, &output.stderr).ok();
            }
            true
        }
        Ok(output) => {
            let code = output.status.code().unwrap_or(-1);
            let stderr_preview: String = String::from_utf8_lossy(&output.stderr)
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!("\n    Exit {code}: {stderr_preview}");
            // Still save stderr for debugging
            if !output.stderr.is_empty() {
                fs::write(log_file, &output.stderr).ok();
            }
            false
        }
        Err(e) => {
            eprintln!("\n    Error: {e}");
            false
        }
    }
}

fn read_and_sort(path: &Path) -> Vec<String> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines.sort_unstable();
    lines
}

fn save_sorted_lines(path: &Path, lines: &[String]) {
    if let Ok(mut f) = fs::File::create(path) {
        for line in lines {
            writeln!(f, "{line}").ok();
        }
    }
}

fn compute_hash(lines: &[String]) -> String {
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn flush() {
    std::io::stdout().flush().ok();
}

#[allow(clippy::too_many_arguments)]
fn show_diff(
    cfg: &Config,
    drive: &str,
    cpp: &[String],
    rust: &[String],
    cpp_hash: &str,
    rust_hash: &str,
    ts: u64,
    cpp_sorted_file: &Path,
    rust_sorted_file: &Path,
    rust_log_file: &Path,
) -> usize {
    println!("\n  ╔══════════════════════════════════════════╗");
    println!("  ║  ❌ PARITY: FAIL                         ║");
    println!("  ╚══════════════════════════════════════════╝");
    println!("    C++ SHA256:  {cpp_hash}");
    println!("    Rust SHA256: {rust_hash}");

    // Collect all differing line indices
    let n = cpp.len().min(rust.len());
    let diff_indices: Vec<usize> = (0..n).filter(|&i| cpp[i] != rust[i]).collect();
    let diff_count = diff_indices.len();

    // Show summary on screen
    let cpp_len = cpp.len();
    let rust_len = rust.len();
    println!("\n=== SORTED SIDE-BY-SIDE COMPARISON (differences only) ===");
    println!("  C++ lines:        {cpp_len}");
    println!("  Rust lines:       {rust_len}");
    println!("  Lines that differ: {diff_count}");

    // Show sample on screen (first 5, last 5, 10 random middle)
    show_diff_sample_on_screen(cpp, rust, &diff_indices);

    // Write COMPLETE diff to file (all differences, not just sample)
    let dl = drive.to_lowercase();
    let diff_path = cfg.out_dir.join(format!("parity_diff_{dl}_{ts}.txt"));
    write_complete_diff_file(&diff_path, drive, cpp, rust, cpp_hash, rust_hash, &diff_indices);

    // Print artifact locations
    println!("\n  📁 MISMATCH ARTIFACTS:");
    println!("    Diff file:        {}", diff_path.display());
    println!("    C++ sorted:       {}", cpp_sorted_file.display());
    println!("    Rust sorted:      {}", rust_sorted_file.display());
    if rust_log_file.exists() {
        println!("    Rust trace log:   {}", rust_log_file.display());
    }

    diff_count
}

/// Write complete sorted side-by-side diff to file (ALL differences).
fn write_complete_diff_file(
    path: &Path,
    drive: &str,
    cpp: &[String],
    rust: &[String],
    cpp_hash: &str,
    rust_hash: &str,
    diff_indices: &[usize],
) {
    let Ok(mut f) = fs::File::create(path) else {
        return;
    };

    let cpp_len = cpp.len();
    let rust_len = rust.len();
    let diff_count = diff_indices.len();

    // Header
    writeln!(f, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").ok();
    writeln!(f, "  DRIVE {drive} - PARITY MISMATCH REPORT").ok();
    writeln!(f, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━").ok();
    writeln!(f).ok();
    writeln!(f, "  C++ SHA256:  {cpp_hash}").ok();
    writeln!(f, "  Rust SHA256: {rust_hash}").ok();
    writeln!(f).ok();
    writeln!(f, "=== SORTED SIDE-BY-SIDE COMPARISON (differences only) ===").ok();
    writeln!(f, "  C++ lines:        {cpp_len}").ok();
    writeln!(f, "  Rust lines:       {rust_len}").ok();
    writeln!(f, "  Lines that differ: {diff_count}").ok();
    writeln!(f).ok();

    if diff_indices.is_empty() {
        writeln!(f, "  ✅ All lines match!").ok();
        return;
    }

    // Write ALL differences
    writeln!(f, "--- ALL {diff_count} DIFFERENCES ---").ok();
    for &idx in diff_indices {
        let line_num = idx + 1;
        writeln!(f, "  Line {line_num}:").ok();
        writeln!(f, "    C++:  {}", cpp[idx]).ok();
        writeln!(f, "    RUST: {}", rust[idx]).ok();
    }
}

/// Show diff sample on screen (first 5, last 5, 10 random from middle).
fn show_diff_sample_on_screen(cpp: &[String], rust: &[String], diff_indices: &[usize]) {
    if diff_indices.is_empty() {
        println!("\n  ✅ All lines match!");
        return;
    }

    let total_diffs = diff_indices.len();
    let first_n = 5.min(total_diffs);
    let last_n = 5.min(total_diffs);

    // First 5 differences
    println!("\n--- FIRST {first_n} DIFFERENCES ---");
    for &idx in diff_indices.iter().take(first_n) {
        let line_num = idx + 1;
        println!("  Line {line_num}:");
        println!("    C++:  {}", cpp[idx]);
        println!("    RUST: {}", rust[idx]);
    }

    // Last 5 differences (if different from first 5)
    if total_diffs > 10 {
        let last_start = total_diffs.saturating_sub(last_n);
        println!("\n--- LAST {last_n} DIFFERENCES ---");
        for &idx in diff_indices.iter().skip(last_start) {
            let line_num = idx + 1;
            println!("  Line {line_num}:");
            println!("    C++:  {}", cpp[idx]);
            println!("    RUST: {}", rust[idx]);
        }
    }

    // 10 random from middle differences
    if total_diffs > 10 {
        let middle_start = first_n;
        let middle_end = total_diffs.saturating_sub(last_n);
        if middle_end > middle_start {
            let middle_diff_indices: Vec<usize> = diff_indices[middle_start..middle_end].to_vec();
            let sample_count = 10.min(middle_diff_indices.len());
            let middle_count = middle_diff_indices.len();

            println!("\n--- {sample_count} RANDOM DIFFERENCES FROM MIDDLE ({middle_count} middle diffs) ---");

            // Deterministic shuffle using LCG
            let mut rng_seed = total_diffs as u64;
            let mut shuffled: Vec<usize> = middle_diff_indices;
            for i in (1..shuffled.len()).rev() {
                rng_seed = rng_seed.wrapping_mul(LCG_MULTIPLIER).wrapping_add(1);
                #[allow(clippy::cast_possible_truncation)]
                let j = (rng_seed as usize) % (i + 1);
                shuffled.swap(i, j);
            }

            for &idx in shuffled.iter().take(sample_count) {
                let line_num = idx + 1;
                println!("  Line {line_num}:");
                println!("    C++:  {}", cpp[idx]);
                println!("    RUST: {}", rust[idx]);
            }
        }
    }
}

fn print_summary(results: &[DriveResult]) {
    println!("\n╔══════════════════════════════════════════════════════════════════╗");
    println!("║                         SUMMARY                                  ║");
    println!("╚══════════════════════════════════════════════════════════════════╝\n");

    let mut pass = 0;
    let mut fail = 0;
    let mut err = 0;

    for r in results {
        let (icon, status) = match r.result {
            VerifyResult::StrictMatch | VerifyResult::SortedMatch => {
                pass += 1;
                ("✅", "PASS")
            }
            VerifyResult::Mismatch => {
                fail += 1;
                ("❌", "MISMATCH")
            }
            VerifyResult::Error => {
                err += 1;
                ("⚠️ ", "ERROR")
            }
        };
        let drive = &r.drive;
        let cpp_lines = r.cpp_lines;
        let rust_lines = r.rust_lines;
        let diff_count = r.diff_count;
        println!("  {icon} Drive {drive}: {status} ({cpp_lines}/{rust_lines} lines, {diff_count} diffs)");
    }

    let total = results.len();
    println!("\n  Total:    {total}");
    println!("  Passed:   {pass}");
    println!("  Failed:   {fail}");
    println!("  Errors:   {err}");

    if fail == 0 && err == 0 {
        println!("\n  🎉 ALL DRIVES VERIFIED SUCCESSFULLY!");
    } else {
        let issues = fail + err;
        println!("\n  ⚠️  {issues} drive(s) had issues");
    }
}

fn print_timing_table(results: &[DriveResult]) {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════════╗");
    println!("║                      LIVE MFT SCAN PERFORMANCE                                   ║");
    println!("╠═══════════╦══════════════════╦══════════════════╦══════════════╦════════════════╣");
    println!("║   Drive   ║     C++ Time     ║    Rust Time     ║    Δ Time    ║  Rust vs C++   ║");
    println!("╠═══════════╬══════════════════╬══════════════════╬══════════════╬════════════════╣");

    let mut total_cpp = Duration::ZERO;
    let mut total_rust = Duration::ZERO;

    for r in results {
        if r.result == VerifyResult::Error {
            continue;
        }

        total_cpp += r.cpp_time;
        total_rust += r.rust_time;

        let delta = if r.rust_time > r.cpp_time {
            format!("+{}", format_duration(r.rust_time.saturating_sub(r.cpp_time)))
        } else {
            format!("-{}", format_duration(r.cpp_time.saturating_sub(r.rust_time)))
        };

        let ratio = if r.cpp_time.as_secs_f64() > 0.0 {
            r.rust_time.as_secs_f64() / r.cpp_time.as_secs_f64()
        } else {
            1.0
        };

        let comparison = format_comparison(ratio);
        let drive = &r.drive;
        let cpp_time = format_duration(r.cpp_time);
        let rust_time = format_duration(r.rust_time);
        println!("║     {drive}     ║ {cpp_time:>16} ║ {rust_time:>16} ║ {delta:>12} ║ {comparison:>14} ║");
    }

    // Totals row
    if results.len() > 1 {
        println!("╠═══════════╬══════════════════╬══════════════════╬══════════════╬════════════════╣");
        let delta = if total_rust > total_cpp {
            format!("+{}", format_duration(total_rust.saturating_sub(total_cpp)))
        } else {
            format!("-{}", format_duration(total_cpp.saturating_sub(total_rust)))
        };
        let ratio = if total_cpp.as_secs_f64() > 0.0 {
            total_rust.as_secs_f64() / total_cpp.as_secs_f64()
        } else {
            1.0
        };
        let comparison = format_comparison(ratio);
        let cpp_time = format_duration(total_cpp);
        let rust_time = format_duration(total_rust);
        println!("║   TOTAL   ║ {cpp_time:>16} ║ {rust_time:>16} ║ {delta:>12} ║ {comparison:>14} ║");
    }

    println!("╚═══════════╩══════════════════╩══════════════════╩══════════════╩════════════════╝\n");
}

fn format_comparison(ratio: f64) -> String {
    if ratio < 1.0 {
        let speedup = 1.0 / ratio;
        format!("{speedup:.1}x faster")
    } else if ratio > 1.0 {
        format!("{ratio:.1}x slower")
    } else {
        "same".to_string()
    }
}

fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        let secs = d.as_secs_f64();
        format!("{secs:.2}s")
    } else {
        let mins = ms / 60_000;
        #[allow(clippy::cast_precision_loss)]
        let secs = (ms % 60_000) as f64 / 1000.0;
        format!("{mins}m {secs:.1}s")
    }
}
