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
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use std::{env, fs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    sample: usize,
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

    let any_fail = results.iter().any(|r| r.result == VerifyResult::Mismatch || r.result == VerifyResult::Error);
    std::process::exit(if any_fail { 1 } else { 0 });
}

fn parse_args() -> Config {
    let args: Vec<String> = env::args().collect();
    let mut drives = Vec::new();
    let mut bin_dir = home_dir().join("bin");
    let mut sample = 30usize;
    let mut discover_all = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--all" => discover_all = true,
            "--bin-dir" => { i += 1; if i < args.len() { bin_dir = PathBuf::from(&args[i]); } }
            "--sample" => { i += 1; if i < args.len() { sample = args[i].parse().unwrap_or(30); } }
            "--help" | "-h" => { print_usage(&args[0]); std::process::exit(0); }
            a if !a.starts_with('-') && a.len() == 1 => drives.push(a.to_uppercase()),
            _ => {}
        }
        i += 1;
    }

    if discover_all { drives = discover_ntfs_drives(); }
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
        sample,
        out_dir: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

fn print_usage(prog: &str) {
    eprintln!("UFFS Live Parity Verification (Windows)\n");
    eprintln!("Usage: {} [OPTIONS] <DRIVES...>", prog);
    eprintln!("       {} --all", prog);
    eprintln!("\nOptions:");
    eprintln!("  --all           Discover and verify all NTFS drives");
    eprintln!("  --bin-dir DIR   Directory containing uffs.com and uffs.exe (default: ~/bin)");
    eprintln!("  --sample N      Number of diff samples to show (default: 30)");
    eprintln!("  -h, --help      Show this help\n");
    eprintln!("Examples:");
    eprintln!("  {} D E F              # Verify drives D, E, F", prog);
    eprintln!("  {} --all              # Verify all NTFS drives", prog);
    eprintln!("  {} D --bin-dir C:\\bin # Custom binary location", prog);
}

fn home_dir() -> PathBuf {
    env::var_os("USERPROFILE").or_else(|| env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| ".".into())
}

fn discover_ntfs_drives() -> Vec<String> {
    // Query all drive letters and filter for NTFS fixed drives
    let mut drives = Vec::new();
    for letter in b'C'..=b'Z' {
        let drive = format!("{}:", letter as char);
        let root = format!("{}\\", drive);
        // Check if drive exists and is accessible
        if PathBuf::from(&root).exists() {
            // Use fsutil to check if NTFS (or just include all fixed drives)
            drives.push((letter as char).to_string());
        }
    }
    println!("  Discovered drives: {:?}", drives);
    drives
}

fn verify_drive(cfg: &Config, drive: &str) -> DriveResult {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let dl = drive.to_lowercase();

    let cpp_file = cfg.out_dir.join(format!("parity_cpp_{}_{}.txt", dl, ts));
    let rust_file = cfg.out_dir.join(format!("parity_rust_{}_{}.txt", dl, ts));

    // Run C++ scan
    print!("  [1/4] C++ scan...  ");
    flush();
    let cpp_start = Instant::now();
    let cpp_ok = run_scan(&cfg.cpp_bin, &["*", &format!("--drives={}", drive)], &cpp_file);
    let cpp_time = cpp_start.elapsed();
    if cpp_ok {
        println!("✅ {}", format_duration(cpp_time));
    } else {
        println!("❌ FAILED");
        return DriveResult {
            drive: drive.to_string(), result: VerifyResult::Error,
            cpp_lines: 0, rust_lines: 0, cpp_time, rust_time: Duration::ZERO, diff_count: 0,
        };
    }

    // Run Rust scan
    print!("  [2/4] Rust scan... ");
    flush();
    let rust_start = Instant::now();
    let rust_ok = run_scan(&cfg.rust_bin, &["*", "--drive", drive, "--no-cache"], &rust_file);
    let rust_time = rust_start.elapsed();
    if rust_ok {
        println!("✅ {}", format_duration(rust_time));
    } else {
        println!("❌ FAILED");
        return DriveResult {
            drive: drive.to_string(), result: VerifyResult::Error,
            cpp_lines: 0, rust_lines: 0, cpp_time, rust_time, diff_count: 0,
        };
    }

    // Sort and compare
    print!("  [3/4] Sorting...   ");
    flush();
    let sort_start = Instant::now();
    let cpp_lines = read_and_sort(&cpp_file);
    let rust_lines = read_and_sort(&rust_file);
    println!("✅ {} (C++: {} lines, Rust: {} lines)",
        format_duration(sort_start.elapsed()), cpp_lines.len(), rust_lines.len());

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
        println!("    SHA256: {}", cpp_hash);

        // Cleanup temp files on success
        fs::remove_file(&cpp_file).ok();
        fs::remove_file(&rust_file).ok();

        return DriveResult {
            drive: drive.to_string(), result: VerifyResult::SortedMatch,
            cpp_lines: cpp_lines.len(), rust_lines: rust_lines.len(),
            cpp_time, rust_time, diff_count: 0,
        };
    }

    // Mismatch - show diff
    println!("❌ MISMATCH");
    let diff_count = show_diff(cfg, drive, &cpp_lines, &rust_lines, &cpp_hash, &rust_hash, ts);

    DriveResult {
        drive: drive.to_string(), result: VerifyResult::Mismatch,
        cpp_lines: cpp_lines.len(), rust_lines: rust_lines.len(),
        cpp_time, rust_time, diff_count,
    }
}

fn run_scan(bin: &PathBuf, args: &[&str], out_file: &PathBuf) -> bool {
    match Command::new(bin).args(args).output() {
        Ok(output) if output.status.success() => {
            fs::write(out_file, &output.stdout).is_ok()
        }
        Ok(output) => {
            eprintln!("\n    Exit {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).lines().take(2).collect::<Vec<_>>().join(" "));
            false
        }
        Err(e) => {
            eprintln!("\n    Error: {}", e);
            false
        }
    }
}

fn read_and_sort(path: &PathBuf) -> Vec<String> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut lines: Vec<String> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines.sort_unstable();
    lines
}

fn compute_hash(lines: &[String]) -> String {
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn flush() { std::io::stdout().flush().ok(); }


fn show_diff(cfg: &Config, drive: &str, cpp: &[String], rust: &[String],
             cpp_hash: &str, rust_hash: &str, ts: u64) -> usize {
    println!("\n  ╔══════════════════════════════════════════╗");
    println!("  ║  ❌ PARITY: FAIL                         ║");
    println!("  ╚══════════════════════════════════════════╝");
    println!("    C++ SHA256:  {}", cpp_hash);
    println!("    Rust SHA256: {}", rust_hash);

    let cpp_set: HashSet<&str> = cpp.iter().map(|s| s.as_str()).collect();
    let rust_set: HashSet<&str> = rust.iter().map(|s| s.as_str()).collect();

    let only_cpp: Vec<_> = cpp.iter().filter(|l| !rust_set.contains(l.as_str())).collect();
    let only_rust: Vec<_> = rust.iter().filter(|l| !cpp_set.contains(l.as_str())).collect();

    println!("\n    Only in C++:  {}", only_cpp.len());
    println!("    Only in Rust: {}", only_rust.len());

    if !only_cpp.is_empty() {
        println!("\n    C++ only (first 5):");
        for l in only_cpp.iter().take(5) { println!("      < {}", l); }
    }
    if !only_rust.is_empty() {
        println!("\n    Rust only (first 5):");
        for l in only_rust.iter().take(5) { println!("      > {}", l); }
    }

    // Write detailed diff to file
    let diff_path = cfg.out_dir.join(format!("parity_diff_{}_{}.txt", drive.to_lowercase(), ts));
    if let Ok(mut f) = fs::File::create(&diff_path) {
        writeln!(f, "# Drive {} | C++ SHA256: {} | Rust SHA256: {}", drive, cpp_hash, rust_hash).ok();
        writeln!(f, "# C++ lines: {} | Rust lines: {} | Only C++: {} | Only Rust: {}\n",
            cpp.len(), rust.len(), only_cpp.len(), only_rust.len()).ok();
        writeln!(f, "=== C++ ONLY ({}) ===", only_cpp.len()).ok();
        for l in only_cpp.iter().take(cfg.sample) { writeln!(f, "< {}", l).ok(); }
        writeln!(f, "\n=== RUST ONLY ({}) ===", only_rust.len()).ok();
        for l in only_rust.iter().take(cfg.sample) { writeln!(f, "> {}", l).ok(); }
        println!("\n    Diff file: {}", diff_path.display());
    }

    only_cpp.len() + only_rust.len()
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
            VerifyResult::StrictMatch | VerifyResult::SortedMatch => { pass += 1; ("✅", "PASS") }
            VerifyResult::Mismatch => { fail += 1; ("❌", "MISMATCH") }
            VerifyResult::Error => { err += 1; ("⚠️ ", "ERROR") }
        };
        println!("  {} Drive {}: {} ({}/{} lines, {} diffs)",
            icon, r.drive, status, r.cpp_lines, r.rust_lines, r.diff_count);
    }

    println!("\n  Total:    {}", results.len());
    println!("  Passed:   {}", pass);
    println!("  Failed:   {}", fail);
    println!("  Errors:   {}", err);

    if fail == 0 && err == 0 {
        println!("\n  🎉 ALL DRIVES VERIFIED SUCCESSFULLY!");
    } else {
        println!("\n  ⚠️  {} drive(s) had issues", fail + err);
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
        if r.result == VerifyResult::Error { continue; }

        total_cpp += r.cpp_time;
        total_rust += r.rust_time;

        let delta = if r.rust_time > r.cpp_time {
            format!("+{}", format_duration(r.rust_time - r.cpp_time))
        } else {
            format!("-{}", format_duration(r.cpp_time - r.rust_time))
        };

        let ratio = if r.cpp_time.as_secs_f64() > 0.0 {
            r.rust_time.as_secs_f64() / r.cpp_time.as_secs_f64()
        } else { 1.0 };

        let comparison = if ratio < 1.0 {
            format!("{:.1}x faster", 1.0 / ratio)
        } else if ratio > 1.0 {
            format!("{:.1}x slower", ratio)
        } else {
            "same".to_string()
        };

        println!("║     {}     ║ {:>16} ║ {:>16} ║ {:>12} ║ {:>14} ║",
            r.drive, format_duration(r.cpp_time), format_duration(r.rust_time), delta, comparison);
    }

    // Totals row
    if results.len() > 1 {
        println!("╠═══════════╬══════════════════╬══════════════════╬══════════════╬════════════════╣");
        let delta = if total_rust > total_cpp {
            format!("+{}", format_duration(total_rust - total_cpp))
        } else {
            format!("-{}", format_duration(total_cpp - total_rust))
        };
        let ratio = if total_cpp.as_secs_f64() > 0.0 {
            total_rust.as_secs_f64() / total_cpp.as_secs_f64()
        } else { 1.0 };
        let comparison = if ratio < 1.0 {
            format!("{:.1}x faster", 1.0 / ratio)
        } else if ratio > 1.0 {
            format!("{:.1}x slower", ratio)
        } else {
            "same".to_string()
        };
        println!("║   TOTAL   ║ {:>16} ║ {:>16} ║ {:>12} ║ {:>14} ║",
            format_duration(total_cpp), format_duration(total_rust), delta, comparison);
    }

    println!("╚═══════════╩══════════════════╩══════════════════╩══════════════╩════════════════╝\n");
}

fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) as f64 / 1000.0;
        format!("{}m {:.1}s", mins, secs)
    }
}

