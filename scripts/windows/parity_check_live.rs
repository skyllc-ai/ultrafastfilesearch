#!/usr/bin/env rust-script
//! Live MFT parity check: Rust uffs.exe vs C++ uffs.com
//!
//! Runs both binaries against LIVE NTFS drives, sorts output, computes SHA256,
//! and reports match/mismatch with detailed diff sampling.
//!
//! # Usage (Windows, elevated)
//!
//! ```powershell
//! rust-script scripts/windows/parity_check_live.rs              # All NTFS drives
//! rust-script scripts/windows/parity_check_live.rs --drive C    # Single drive
//! rust-script scripts/windows/parity_check_live.rs --drive C,D,E  # Multiple drives
//! rust-script scripts/windows/parity_check_live.rs --sample 50  # 50 diff samples (default: 30)
//! rust-script scripts/windows/parity_check_live.rs --out-dir D:\parity  # Custom output dir
//! ```
//!
//! # Requirements
//!
//! - Windows with Administrator privileges (MFT access)
//! - `uffs.exe` (Rust) and `uffs.com` (C++) in `%USERPROFILE%\bin`
//!
//! ```cargo
//! [dependencies]
//! sha2 = "0.10"
//! ```

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use std::{env, io};

const DEFAULT_SAMPLE_SIZE: usize = 30;

fn main() {
    let args: Vec<String> = env::args().collect();

    let bin_dir = env::var("USERPROFILE")
        .map(|h| PathBuf::from(h).join("bin"))
        .unwrap_or_else(|_| PathBuf::from(r"C:\bin"));

    let uffs_rust = bin_dir.join("uffs.exe");
    let uffs_cpp = bin_dir.join("uffs.com");

    if !uffs_rust.exists() {
        eprintln!("ERROR: Rust binary not found: {}", uffs_rust.display());
        std::process::exit(1);
    }
    if !uffs_cpp.exists() {
        eprintln!("ERROR: C++ binary not found: {}", uffs_cpp.display());
        std::process::exit(1);
    }

    let drives = parse_drives(&args);
    let sample_size = parse_sample_size(&args);
    let out_dir = parse_out_dir(&args);

    fs::create_dir_all(&out_dir).ok();
    let timestamp = chrono_timestamp();

    println!();
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║      UFFS LIVE PARITY CHECK — C++ vs Rust (Live MFT)           ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    println!("  C++ binary : {}", uffs_cpp.display());
    println!("  Rust binary: {}", uffs_rust.display());
    println!("  Drives     : {:?}", drives);
    println!("  Output dir : {}", out_dir.display());
    println!("  Sample size: {}", sample_size);
    println!();

    let mut all_passed = true;

    for drive in &drives {
        let passed = run_drive_parity(
            drive,
            &uffs_cpp,
            &uffs_rust,
            &out_dir,
            &timestamp,
            sample_size,
        );
        if !passed {
            all_passed = false;
        }
    }

    println!();
    if all_passed {
        println!("🎉 ALL DRIVES PASSED PARITY CHECK!");
    } else {
        println!("⚠️  SOME DRIVES HAD MISMATCHES — see diff files above");
        std::process::exit(1);
    }
}

fn run_drive_parity(
    drive: &str,
    cpp_bin: &Path,
    rust_bin: &Path,
    out_dir: &Path,
    timestamp: &str,
    sample_size: usize,
) -> bool {
    let drive_lower = drive.to_lowercase();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Drive {}", drive.to_uppercase());
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let cpp_raw = out_dir.join(format!("cpp_{drive_lower}_{timestamp}.txt"));
    let rust_raw = out_dir.join(format!("rust_{drive_lower}_{timestamp}.txt"));
    let cpp_sorted = out_dir.join(format!("cpp_{drive_lower}_{timestamp}_sorted.txt"));
    let rust_sorted = out_dir.join(format!("rust_{drive_lower}_{timestamp}_sorted.txt"));
    let diff_file = out_dir.join(format!("diff_{drive_lower}_{timestamp}.txt"));

    // 1. Run C++
    print!("  [1/4] Running C++ scan...");
    io::stdout().flush().ok();
    let cpp_start = Instant::now();
    let cpp_status = Command::new(cpp_bin)
        .args(["*", &format!("--drives={}", drive.to_uppercase())])
        .stdout(File::create(&cpp_raw).expect("create cpp_raw"))
        .stderr(std::process::Stdio::null())
        .status();
    let cpp_ms = cpp_start.elapsed().as_millis();
    match cpp_status {
        Ok(s) if s.success() => println!(" ✅ ({}ms)", cpp_ms),
        _ => {
            println!(" ❌ FAILED");
            return false;
        }
    }

    // 2. Run Rust
    print!("  [2/4] Running Rust scan...");
    io::stdout().flush().ok();
    let rust_start = Instant::now();
    let rust_status = Command::new(rust_bin)
        .args(["*", "--drive", &drive.to_uppercase(), "--no-cache"])
        .stdout(File::create(&rust_raw).expect("create rust_raw"))
        .stderr(std::process::Stdio::null())
        .status();
    let rust_ms = rust_start.elapsed().as_millis();
    match rust_status {
        Ok(s) if s.success() => println!(" ✅ ({}ms)", rust_ms),
        _ => {
            println!(" ❌ FAILED");
            return false;
        }
    }

    // 3. Sort both outputs (streaming, memory-efficient)
    print!("  [3/4] Sorting outputs...");
    io::stdout().flush().ok();
    let sort_start = Instant::now();
    let cpp_lines = sort_file_to(&cpp_raw, &cpp_sorted);
    let rust_lines = sort_file_to(&rust_raw, &rust_sorted);
    let sort_ms = sort_start.elapsed().as_millis();
    println!(" ✅ ({}ms)", sort_ms);
    println!("       C++ lines : {}", cpp_lines);
    println!("       Rust lines: {}", rust_lines);

    // 4. SHA256 comparison
    print!("  [4/4] Computing SHA256...");
    io::stdout().flush().ok();
    let cpp_hash = sha256_file(&cpp_sorted);
    let rust_hash = sha256_file(&rust_sorted);

    if cpp_hash == rust_hash {
        println!(" ✅ MATCH");
        println!();
        println!("  ╔═══════════════════════════════════════════════════════════╗");
        println!("  ║  PARITY: PASS — Sorted outputs are identical              ║");
        println!("  ╚═══════════════════════════════════════════════════════════╝");
        println!("       SHA256: {}", &cpp_hash[..16]);
        println!();
        // Clean up raw files
        fs::remove_file(&cpp_raw).ok();
        fs::remove_file(&rust_raw).ok();
        return true;
    }

    println!(" ❌ MISMATCH");
    println!();
    println!("  ╔═══════════════════════════════════════════════════════════╗");
    println!("  ║  PARITY: FAIL — Outputs differ                            ║");
    println!("  ╚═══════════════════════════════════════════════════════════╝");
    println!("       C++ SHA256 : {}", &cpp_hash[..16]);
    println!("       Rust SHA256: {}", &rust_hash[..16]);
    println!();

    // Build diff report
    print!("  Building diff report...");
    io::stdout().flush().ok();
    let (only_cpp, only_rust) = compute_set_diff(&cpp_sorted, &rust_sorted);
    println!(" ✅");
    println!("       Only in C++ : {} lines", only_cpp.len());
    println!("       Only in Rust: {} lines", only_rust.len());

    // Sample and write diff file
    write_diff_report(
        &diff_file,
        drive,
        &cpp_sorted,
        &rust_sorted,
        &cpp_hash,
        &rust_hash,
        cpp_lines,
        rust_lines,
        &only_cpp,
        &only_rust,
        sample_size,
    );
    println!("       Diff report : {}", diff_file.display());

    // Show preview
    let preview_count = 5.min(only_cpp.len());
    if preview_count > 0 {
        println!();
        println!("       Sample lines only in C++ (first {}):", preview_count);
        for line in only_cpp.iter().take(preview_count) {
            println!("         < {}", truncate_line(line, 70));
        }
    }
    let preview_count = 5.min(only_rust.len());
    if preview_count > 0 {
        println!("       Sample lines only in Rust (first {}):", preview_count);
        for line in only_rust.iter().take(preview_count) {
            println!("         > {}", truncate_line(line, 70));
        }
    }
    println!();
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions
// ─────────────────────────────────────────────────────────────────────────────

fn parse_drives(args: &[String]) -> Vec<String> {
    for i in 0..args.len() {
        if args[i] == "--drive" && i + 1 < args.len() {
            return args[i + 1]
                .split(',')
                .map(|s| s.trim().to_uppercase())
                .filter(|s| s.len() == 1 && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic()))
                .collect();
        }
    }
    // Auto-detect NTFS drives (Windows only)
    detect_ntfs_drives()
}

fn detect_ntfs_drives() -> Vec<String> {
    // Try wmic to detect NTFS drives
    let output = Command::new("wmic")
        .args(["logicaldisk", "where", "DriveType=3", "get", "DeviceID,FileSystem"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .filter(|line| line.contains("NTFS"))
                .filter_map(|line| line.chars().next())
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_string().to_uppercase())
                .collect()
        }
        _ => vec!["C".to_string()], // Fallback to C:
    }
}

fn parse_sample_size(args: &[String]) -> usize {
    for i in 0..args.len() {
        if args[i] == "--sample" && i + 1 < args.len() {
            return args[i + 1].parse().unwrap_or(DEFAULT_SAMPLE_SIZE);
        }
    }
    DEFAULT_SAMPLE_SIZE
}

fn parse_out_dir(args: &[String]) -> PathBuf {
    for i in 0..args.len() {
        if args[i] == "--out-dir" && i + 1 < args.len() {
            return PathBuf::from(&args[i + 1]);
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn chrono_timestamp() -> String {
    // Simple timestamp without chrono dependency
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}", duration.as_secs())
}

fn sort_file_to(input: &Path, output: &Path) -> usize {
    let file = File::open(input).expect("open input for sorting");
    let reader = BufReader::new(file);
    let mut lines: Vec<String> = reader
        .lines()
        .filter_map(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines.sort_unstable();
    let count = lines.len();

    let out_file = File::create(output).expect("create sorted output");
    let mut writer = BufWriter::new(out_file);
    for line in &lines {
        writeln!(writer, "{}", line).ok();
    }
    count
}

fn sha256_file(path: &Path) -> String {
    let file = File::open(path).expect("open file for hashing");
    let reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    for line in reader.lines().filter_map(Result::ok) {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn compute_set_diff(cpp_path: &Path, rust_path: &Path) -> (Vec<String>, Vec<String>) {
    // Read Rust into HashSet
    let rust_file = File::open(rust_path).expect("open rust sorted");
    let rust_reader = BufReader::new(rust_file);
    let rust_set: HashSet<String> = rust_reader.lines().filter_map(Result::ok).collect();

    // Find lines only in C++
    let cpp_file = File::open(cpp_path).expect("open cpp sorted");
    let cpp_reader = BufReader::new(cpp_file);
    let only_cpp: Vec<String> = cpp_reader
        .lines()
        .filter_map(Result::ok)
        .filter(|line| !rust_set.contains(line))
        .collect();

    // Read C++ into HashSet
    let cpp_file2 = File::open(cpp_path).expect("open cpp sorted again");
    let cpp_reader2 = BufReader::new(cpp_file2);
    let cpp_set: HashSet<String> = cpp_reader2.lines().filter_map(Result::ok).collect();

    // Find lines only in Rust
    let rust_file2 = File::open(rust_path).expect("open rust sorted again");
    let rust_reader2 = BufReader::new(rust_file2);
    let only_rust: Vec<String> = rust_reader2
        .lines()
        .filter_map(Result::ok)
        .filter(|line| !cpp_set.contains(line))
        .collect();

    (only_cpp, only_rust)
}

#[allow(clippy::too_many_arguments)]
fn write_diff_report(
    path: &Path,
    drive: &str,
    cpp_sorted: &Path,
    rust_sorted: &Path,
    cpp_hash: &str,
    rust_hash: &str,
    cpp_lines: usize,
    rust_lines: usize,
    only_cpp: &[String],
    only_rust: &[String],
    sample_size: usize,
) {
    let file = File::create(path).expect("create diff report");
    let mut w = BufWriter::new(file);

    writeln!(w, "# UFFS Live Parity Diff Report").ok();
    writeln!(w, "# Drive: {}", drive.to_uppercase()).ok();
    writeln!(w, "# C++ sorted : {}", cpp_sorted.display()).ok();
    writeln!(w, "# Rust sorted: {}", rust_sorted.display()).ok();
    writeln!(w, "# C++ SHA256 : {}", cpp_hash).ok();
    writeln!(w, "# Rust SHA256: {}", rust_hash).ok();
    writeln!(w, "# C++ lines  : {}", cpp_lines).ok();
    writeln!(w, "# Rust lines : {}", rust_lines).ok();
    writeln!(w, "# Only in C++: {}", only_cpp.len()).ok();
    writeln!(w, "# Only in Rust: {}", only_rust.len()).ok();
    writeln!(w).ok();

    writeln!(w, "=== LINES ONLY IN C++ ({} total, showing up to {}) ===", only_cpp.len(), sample_size).ok();
    for line in only_cpp.iter().take(sample_size) {
        writeln!(w, "< {}", line).ok();
    }
    writeln!(w).ok();

    writeln!(w, "=== LINES ONLY IN RUST ({} total, showing up to {}) ===", only_rust.len(), sample_size).ok();
    for line in only_rust.iter().take(sample_size) {
        writeln!(w, "> {}", line).ok();
    }
}

fn truncate_line(line: &str, max_len: usize) -> String {
    if line.len() <= max_len {
        line.to_string()
    } else {
        format!("{}...", &line[..max_len])
    }
}

