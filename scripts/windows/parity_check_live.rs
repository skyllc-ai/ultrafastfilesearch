#!/usr/bin/env rust-script
//! Live MFT parity check: Rust uffs.exe vs C++ uffs.com
//!
//! Runs both binaries against LIVE NTFS drives, sorts output, computes SHA256,
//! and reports match/mismatch with detailed diff sampling.
//!
//! # Usage (Windows, elevated)
//!
//! ```powershell
//! rust-script scripts/windows/parity_check_live.rs              # All NTFS drives, full scan
//! rust-script scripts/windows/parity_check_live.rs --drive C    # Single drive
//! rust-script scripts/windows/parity_check_live.rs --drive C,D,E  # Multiple drives
//! rust-script scripts/windows/parity_check_live.rs --pattern "*.txt"  # Glob pattern
//! rust-script scripts/windows/parity_check_live.rs --pattern ">C:\\Users\\.*\.(jpg|png)"  # Regex
//! rust-script scripts/windows/parity_check_live.rs --pattern "hallo" --name-only  # Filename-only matching
//! rust-script scripts/windows/parity_check_live.rs --sample 50  # 50 diff samples (default: 30)
//! rust-script scripts/windows/parity_check_live.rs --out-dir D:\parity  # Custom output dir
//! rust-script scripts/windows/parity_check_live.rs --keep       # Keep raw output files on success
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

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use std::{env, io};

use sha2::{Digest, Sha256};

const DEFAULT_SAMPLE_SIZE: usize = 50;

/// LCG (Linear Congruential Generator) multiplier - Knuth's MMIX constant.
const LCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyResult {
    StrictMatch,
    SortedMatch,
    Mismatch,
    Skipped,
}

/// Maximum retries for transient errors (sharing violation, access denied).
const MAX_RETRIES: u32 = 3;
/// Delay between retries in milliseconds.
const RETRY_DELAY_MS: u64 = 2000;

/// Decode a Windows process exit code into a human-readable error message.
fn decode_exit_code(code: i32) -> &'static str {
    match code {
        5 => "ERROR_ACCESS_DENIED — Run as Administrator",
        32 => "ERROR_SHARING_VIOLATION — Another process has the MFT locked (close other uffs/Everything instances)",
        33 => "ERROR_LOCK_VIOLATION — File region locked by another process",
        87 => "ERROR_INVALID_PARAMETER — Invalid command-line arguments",
        995 => "ERROR_OPERATION_ABORTED — I/O was cancelled",
        1359 => "ERROR_INTERNAL_ERROR — Internal error in volume access",
        _ => "Unknown error",
    }
}

/// Check if an exit code is a transient error worth retrying.
fn is_transient_error(code: i32) -> bool {
    matches!(code, 32 | 33)
}

#[derive(Debug)]
struct DriveResult {
    drive_letter: String,
    result: VerifyResult,
    cpp_lines: usize,
    rust_lines: usize,
    cpp_time_ms: u128,
    rust_time_ms: u128,
    _sort_time_ms: u128,
}

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
    let pattern = parse_pattern(&args);
    let name_only = args.iter().any(|a| a == "--name-only");
    let sample_size = parse_sample_size(&args);
    let out_dir = parse_out_dir(&args);
    let keep_files = args.iter().any(|a| a == "--keep");

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
    println!("  Pattern    : {}", pattern);
    if name_only {
        println!("  Name only  : yes (filename matching only)");
    }
    println!("  Output dir : {}", out_dir.display());
    println!("  Sample size: {}", sample_size);
    println!(
        "  Keep files : {}",
        if keep_files {
            "yes"
        } else {
            "no (clean on success)"
        }
    );
    println!();

    let mut results: Vec<DriveResult> = Vec::new();

    for (index, drive) in drives.iter().enumerate() {
        let result = run_drive_parity(
            drive,
            &pattern,
            name_only,
            &uffs_cpp,
            &uffs_rust,
            &out_dir,
            &timestamp,
            sample_size,
            keep_files,
            index + 1,
            drives.len(),
        );
        results.push(result);
    }

    // Print summary
    print_summary(&results);

    // Exit with failure if any drive mismatched
    let any_mismatch = results.iter().any(|r| r.result == VerifyResult::Mismatch);
    if any_mismatch {
        std::process::exit(1);
    }
}

/// Run a binary with retry logic for transient Windows errors.
///
/// Retries up to `MAX_RETRIES` times for sharing violations (exit code 32/33).
/// Returns `Ok(())` on success, `Err(message)` with a human-readable diagnostic.
fn run_with_retry(bin: &Path, args: &[&str], stdout_path: &Path, label: &str) -> Result<(), String> {
    for attempt in 0..=MAX_RETRIES {
        let output = Command::new(bin)
            .args(args)
            .stdout(File::create(stdout_path).map_err(|e| format!("cannot create output file: {e}"))?)
            .stderr(std::process::Stdio::piped())
            .output();

        match output {
            Ok(ref o) if o.status.success() => return Ok(()),
            Ok(ref o) => {
                let code = o.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&o.stderr);
                let stderr_msg = stderr.trim();
                let decoded = decode_exit_code(code);

                if is_transient_error(code) && attempt < MAX_RETRIES {
                    println!(
                        "\n       ⚠️  {} attempt {}/{} failed: exit code {} — {}",
                        label,
                        attempt + 1,
                        MAX_RETRIES + 1,
                        code,
                        decoded
                    );
                    println!(
                        "       Retrying in {}s...",
                        RETRY_DELAY_MS / 1000
                    );
                    std::thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
                    print!("  [retry] Running {} scan...", label);
                    io::stdout().flush().ok();
                    continue;
                }

                let mut msg = format!("exit code {} — {}", code, decoded);
                if !stderr_msg.is_empty() {
                    msg.push_str(&format!("\n       stderr: {}", stderr_msg));
                }
                return Err(msg);
            }
            Err(e) => return Err(format!("failed to execute {}: {}", bin.display(), e)),
        }
    }
    Err("max retries exceeded".into())
}

#[allow(clippy::too_many_arguments)]
fn run_drive_parity(
    drive: &str,
    pattern: &str,
    name_only: bool,
    cpp_bin: &Path,
    rust_bin: &Path,
    out_dir: &Path,
    timestamp: &str,
    sample_size: usize,
    keep_files: bool,
    drive_index: usize,
    total_drives: usize,
) -> DriveResult {
    let drive_upper = drive.to_uppercase();
    let drive_lower = drive.to_lowercase();

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "  [{}/{}] DRIVE {} — Live MFT Scan",
        drive_index, total_drives, drive_upper
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let cpp_raw = out_dir.join(format!("cpp_{drive_lower}_{timestamp}.txt"));
    let rust_raw = out_dir.join(format!("rust_{drive_lower}_{timestamp}.txt"));
    let cpp_sorted = out_dir.join(format!("cpp_{drive_lower}_{timestamp}_sorted.txt"));
    let rust_sorted = out_dir.join(format!("rust_{drive_lower}_{timestamp}_sorted.txt"));
    let diff_file = out_dir.join(format!("diff_{drive_lower}_{timestamp}.txt"));

    // 1. Run C++ (with retry for transient errors like sharing violations)
    print!("  [1/4] Running C++ scan...");
    io::stdout().flush().ok();
    let cpp_start = Instant::now();
    let cpp_drives_arg = format!("--drives={}", drive_upper);
    let cpp_result = run_with_retry(
        cpp_bin,
        &[pattern, &cpp_drives_arg],
        &cpp_raw,
        "C++",
    );
    let cpp_ms = cpp_start.elapsed().as_millis();
    match cpp_result {
        Ok(()) => println!(" ✅ ({})", format_duration_ms(cpp_ms)),
        Err(msg) => {
            println!(" ❌ SKIPPED — {}", msg);
            println!();
            return DriveResult {
                drive_letter: drive_upper,
                result: VerifyResult::Skipped,
                cpp_lines: 0,
                rust_lines: 0,
                cpp_time_ms: cpp_ms,
                rust_time_ms: 0,
                _sort_time_ms: 0,
            };
        }
    }

    // 2. Run Rust (with --format custom to match C++ output format)
    print!("  [2/4] Running Rust scan...");
    io::stdout().flush().ok();
    let rust_start = Instant::now();
    let mut rust_args: Vec<&str> = vec![
        pattern,
        "--drive",
        &drive_upper,
        "--no-cache",
        "--format",
        "custom",
    ];
    if name_only {
        rust_args.push("--name-only");
    }
    let rust_result = run_with_retry(
        rust_bin,
        &rust_args,
        &rust_raw,
        "Rust",
    );
    let rust_ms = rust_start.elapsed().as_millis();
    match rust_result {
        Ok(()) => println!(" ✅ ({})", format_duration_ms(rust_ms)),
        Err(msg) => {
            println!(" ❌ SKIPPED — {}", msg);
            println!();
            return DriveResult {
                drive_letter: drive_upper,
                result: VerifyResult::Skipped,
                cpp_lines: 0,
                rust_lines: 0,
                cpp_time_ms: cpp_ms,
                rust_time_ms: rust_ms,
                _sort_time_ms: 0,
            };
        }
    }

    // 3. Sort both outputs (byte-level stable sort for cross-platform consistency)
    print!("  [3/4] Sorting outputs...");
    io::stdout().flush().ok();
    let sort_start = Instant::now();
    let cpp_lines = sort_file_to(&cpp_raw, &cpp_sorted);
    let rust_lines = sort_file_to(&rust_raw, &rust_sorted);
    let sort_ms = sort_start.elapsed().as_millis();
    println!(" ✅ ({})", format_duration_ms(sort_ms));
    println!("       C++ lines : {}", cpp_lines);
    println!("       Rust lines: {}", rust_lines);
    println!();

    // 4. SHA256 comparison - ordered first, then sorted
    print!("  [4/4] Computing SHA256...");
    io::stdout().flush().ok();
    let cpp_ordered_hash = sha256_file(&cpp_raw);
    let rust_ordered_hash = sha256_file(&rust_raw);
    let cpp_sorted_hash = sha256_file(&cpp_sorted);
    let rust_sorted_hash = sha256_file(&rust_sorted);
    println!(" ✅");
    println!();

    // Check for strict (ordered) match first
    if cpp_ordered_hash == rust_ordered_hash {
        println!("  ╔═══════════════════════════════════════════════════════════╗");
        println!("  ║  ✅ PARITY: STRICT MATCH — Ordered outputs identical      ║");
        println!("  ╚═══════════════════════════════════════════════════════════╝");
        println!("       SHA256: {}...", &cpp_ordered_hash[..16]);
        println!();

        // Clean up files unless --keep
        if !keep_files {
            fs::remove_file(&cpp_raw).ok();
            fs::remove_file(&rust_raw).ok();
            fs::remove_file(&cpp_sorted).ok();
            fs::remove_file(&rust_sorted).ok();
        }

        return DriveResult {
            drive_letter: drive_upper,
            result: VerifyResult::StrictMatch,
            cpp_lines,
            rust_lines,
            cpp_time_ms: cpp_ms,
            rust_time_ms: rust_ms,
            _sort_time_ms: sort_ms,
        };
    }

    // Check for sorted match (different traversal order but same content)
    if cpp_sorted_hash == rust_sorted_hash {
        println!("  ╔═══════════════════════════════════════════════════════════╗");
        println!("  ║  ✅ PARITY: SORTED MATCH — Content identical              ║");
        println!("  ║     (traversal order differs, but all lines match)        ║");
        println!("  ╚═══════════════════════════════════════════════════════════╝");
        println!("       Sorted SHA256: {}...", &cpp_sorted_hash[..16]);
        println!();

        // Clean up files unless --keep
        if !keep_files {
            fs::remove_file(&cpp_raw).ok();
            fs::remove_file(&rust_raw).ok();
            fs::remove_file(&cpp_sorted).ok();
            fs::remove_file(&rust_sorted).ok();
        }

        return DriveResult {
            drive_letter: drive_upper,
            result: VerifyResult::SortedMatch,
            cpp_lines,
            rust_lines,
            cpp_time_ms: cpp_ms,
            rust_time_ms: rust_ms,
            _sort_time_ms: sort_ms,
        };
    }

    // Mismatch - show details
    println!("  ╔═══════════════════════════════════════════════════════════╗");
    println!("  ║  ❌ PARITY: MISMATCH — Outputs differ                     ║");
    println!("  ╚═══════════════════════════════════════════════════════════╝");
    println!();
    println!("  Hashes:");
    println!("       C++ ordered SHA256 : {}...", &cpp_ordered_hash[..16]);
    println!(
        "       Rust ordered SHA256: {}...",
        &rust_ordered_hash[..16]
    );
    println!("       C++ sorted SHA256  : {}...", &cpp_sorted_hash[..16]);
    println!("       Rust sorted SHA256 : {}...", &rust_sorted_hash[..16]);
    println!();
    println!("  Line count: {} (C++) vs {} (Rust)", cpp_lines, rust_lines);
    println!();

    // Show sorted side-by-side comparison (most useful for debugging)
    show_first_sorted_diffs(&cpp_sorted, &rust_sorted, sample_size);

    // Write diff report to file
    write_diff_report(
        &diff_file,
        &drive_upper,
        &cpp_sorted,
        &rust_sorted,
        &cpp_sorted_hash,
        &rust_sorted_hash,
        cpp_lines,
        rust_lines,
        sample_size,
    );
    println!();
    println!("  Diff report: {}", diff_file.display());
    println!("  Sorted C++:  {}", cpp_sorted.display());
    println!("  Sorted Rust: {}", rust_sorted.display());
    println!();

    DriveResult {
        drive_letter: drive_upper,
        result: VerifyResult::Mismatch,
        cpp_lines,
        rust_lines,
        cpp_time_ms: cpp_ms,
        rust_time_ms: rust_ms,
        _sort_time_ms: sort_ms,
    }
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
                .filter(|s| {
                    s.len() == 1 && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
                })
                .collect();
        }
    }
    // Auto-detect NTFS drives (Windows only)
    detect_ntfs_drives()
}

fn detect_ntfs_drives() -> Vec<String> {
    // Try wmic to detect NTFS drives
    let output = Command::new("wmic")
        .args([
            "logicaldisk",
            "where",
            "DriveType=3",
            "get",
            "DeviceID,FileSystem",
        ])
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

fn parse_pattern(args: &[String]) -> String {
    for i in 0..args.len() {
        if args[i] == "--pattern" && i + 1 < args.len() {
            return args[i + 1].clone();
        }
    }
    "*".to_string()
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
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

/// Sort file using byte-level stable sort for cross-platform consistency.
fn sort_file_to(input: &Path, output: &Path) -> usize {
    let file = File::open(input).expect("open input for sorting");
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().filter_map(Result::ok).collect();

    // Stable sort with byte-level comparison + index tie-break
    let mut indexed: Vec<(usize, &str)> = lines.iter().map(String::as_str).enumerate().collect();
    indexed.sort_by(
        |(idx_a, a), (idx_b, b)| match a.as_bytes().cmp(b.as_bytes()) {
            Ordering::Equal => idx_a.cmp(idx_b),
            other => other,
        },
    );

    let count = indexed.len();
    let out_file = File::create(output).expect("create sorted output");
    let mut writer = BufWriter::new(out_file);
    for (_, line) in &indexed {
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

/// Read file and sort lines using robust byte-level comparison.
fn read_sorted_lines(path: &Path) -> Vec<String> {
    let file = File::open(path).expect("open file for reading");
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().filter_map(Result::ok).collect();
    let mut indexed: Vec<(usize, String)> = all_lines.into_iter().enumerate().collect();

    // Stable sort with byte-level comparison + index tie-break
    indexed.sort_by(
        |(idx_a, a), (idx_b, b)| match a.as_bytes().cmp(b.as_bytes()) {
            Ordering::Equal => idx_a.cmp(idx_b),
            other => other,
        },
    );

    indexed.into_iter().map(|(_, line)| line).collect()
}

/// Show side-by-side comparison of DIFFERENT lines from sorted files.
fn show_first_sorted_diffs(cpp_sorted: &Path, rust_sorted: &Path, sample_size: usize) {
    let sorted_cpp = read_sorted_lines(cpp_sorted);
    let sorted_rust = read_sorted_lines(rust_sorted);

    let n = sorted_cpp.len().min(sorted_rust.len());
    if n == 0 {
        println!("  No lines to compare.");
        return;
    }

    // Collect indices of lines that differ
    let diff_indices: Vec<usize> = (0..n)
        .filter(|&i| sorted_cpp[i] != sorted_rust[i])
        .collect();

    println!("=== SORTED SIDE-BY-SIDE COMPARISON (differences only) ===");
    println!("  C++ lines:   {}", sorted_cpp.len());
    println!("  Rust lines:  {}", sorted_rust.len());
    println!("  Lines that differ: {}", diff_indices.len());

    if diff_indices.is_empty() {
        println!();
        println!("  ✅ All lines match!");
        return;
    }

    let total_diffs = diff_indices.len();
    let first_n = sample_size.min(total_diffs);
    let last_n = sample_size.min(total_diffs);
    let middle_sample = sample_size * 2;

    // First N differences
    println!();
    println!("--- FIRST {} DIFFERENCES ---", first_n);
    for &idx in diff_indices.iter().take(first_n) {
        let line_num = idx + 1;
        println!("  Line {}:", line_num);
        println!("    C++:  {}", sorted_cpp[idx]);
        println!("    RUST: {}", sorted_rust[idx]);
    }

    // Last N differences (if different from first N)
    if total_diffs > first_n + last_n {
        let last_start = total_diffs.saturating_sub(last_n);
        println!();
        println!("--- LAST {} DIFFERENCES ---", last_n);
        for &idx in diff_indices.iter().skip(last_start) {
            let line_num = idx + 1;
            println!("  Line {}:", line_num);
            println!("    C++:  {}", sorted_cpp[idx]);
            println!("    RUST: {}", sorted_rust[idx]);
        }
    }

    // Random sample from middle differences
    if total_diffs > first_n + last_n {
        let middle_start = first_n;
        let middle_end = total_diffs.saturating_sub(last_n);
        if middle_end > middle_start {
            let middle_diff_indices: Vec<usize> = diff_indices[middle_start..middle_end].to_vec();
            let sample_count = middle_sample.min(middle_diff_indices.len());
            let middle_count = middle_diff_indices.len();

            println!();
            println!(
                "--- {} RANDOM DIFFERENCES FROM MIDDLE ({} middle diffs) ---",
                sample_count, middle_count
            );

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
                println!("  Line {}:", line_num);
                println!("    C++:  {}", sorted_cpp[idx]);
                println!("    RUST: {}", sorted_rust[idx]);
            }
        }
    }
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
    sample_size: usize,
) {
    let sorted_cpp = read_sorted_lines(cpp_sorted);
    let sorted_rust = read_sorted_lines(rust_sorted);

    let file = File::create(path).expect("create diff report");
    let mut w = BufWriter::new(file);

    writeln!(w, "# UFFS Live Parity Diff Report").ok();
    writeln!(w, "# Drive: {}", drive).ok();
    writeln!(w, "# C++ sorted : {}", cpp_sorted.display()).ok();
    writeln!(w, "# Rust sorted: {}", rust_sorted.display()).ok();
    writeln!(w, "# C++ SHA256 : {}", cpp_hash).ok();
    writeln!(w, "# Rust SHA256: {}", rust_hash).ok();
    writeln!(w, "# C++ lines  : {}", cpp_lines).ok();
    writeln!(w, "# Rust lines : {}", rust_lines).ok();
    writeln!(w).ok();

    // Compute side-by-side diff stats
    let n = sorted_cpp.len().min(sorted_rust.len());
    let diff_indices: Vec<usize> = (0..n)
        .filter(|&i| sorted_cpp[i] != sorted_rust[i])
        .collect();

    writeln!(w, "# Lines that differ: {}", diff_indices.len()).ok();
    writeln!(w).ok();

    writeln!(
        w,
        "=== SIDE-BY-SIDE DIFFERENCES ({} total, showing up to {}) ===",
        diff_indices.len(),
        sample_size
    )
    .ok();
    for &idx in diff_indices.iter().take(sample_size) {
        let line_num = idx + 1;
        writeln!(w, "Line {}:", line_num).ok();
        writeln!(w, "  C++:  {}", sorted_cpp[idx]).ok();
        writeln!(w, "  RUST: {}", sorted_rust[idx]).ok();
        writeln!(w).ok();
    }
}

/// Print final summary of all drive results
fn print_summary(results: &[DriveResult]) {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║                         SUMMARY                                  ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!();

    let mut strict_match = 0;
    let mut sorted_match = 0;
    let mut mismatch = 0;
    let mut skipped = 0;

    for result in results {
        let (icon, status) = match result.result {
            VerifyResult::StrictMatch => {
                strict_match += 1;
                ("✅", "STRICT MATCH")
            }
            VerifyResult::SortedMatch => {
                sorted_match += 1;
                ("✅", "SORTED MATCH")
            }
            VerifyResult::Mismatch => {
                mismatch += 1;
                ("❌", "MISMATCH")
            }
            VerifyResult::Skipped => {
                skipped += 1;
                ("⏭️", "SKIPPED (access error)")
            }
        };
        if result.result == VerifyResult::Skipped {
            println!("  {} Drive {}: {}", icon, result.drive_letter, status);
        } else {
            println!(
                "  {} Drive {}: {} ({} / {} lines)",
                icon, result.drive_letter, status, result.cpp_lines, result.rust_lines
            );
        }
    }

    println!();
    let total_drives = results.len();
    let tested = total_drives - skipped;
    println!("  Total drives:    {}", total_drives);
    println!("  Tested:          {}", tested);
    println!("  Strict matches:  {}", strict_match);
    println!("  Sorted matches:  {}", sorted_match);
    println!("  Mismatches:      {}", mismatch);
    if skipped > 0 {
        println!("  Skipped:         {} (access errors — close other MFT readers)", skipped);
    }
    println!();

    if mismatch == 0 && skipped == 0 {
        println!("  🎉 ALL DRIVES VERIFIED SUCCESSFULLY!");
    } else if mismatch == 0 {
        println!("  ✅ All tested drives match! ({} skipped due to access errors)", skipped);
    } else {
        println!("  ⚠️  {} drive(s) had mismatches", mismatch);
    }

    // Print timing table
    print_timing_table(results);
    println!();
}

/// Print a timing table for scan performance comparison
fn print_timing_table(results: &[DriveResult]) {
    println!();
    println!(
        "╔══════════════════════════════════════════════════════════════════════════════════════╗"
    );
    println!(
        "║                           SCAN PERFORMANCE COMPARISON                                ║"
    );
    println!(
        "╠═══════════╦══════════════════╦══════════════════╦═════════════╦═════════════════════╣"
    );
    println!(
        "║   Drive   ║   C++ Time       ║   Rust Time      ║  Speedup    ║   Files/sec (Rust)  ║"
    );
    println!(
        "╠═══════════╬══════════════════╬══════════════════╬═════════════╬═════════════════════╣"
    );

    let mut total_cpp_ms: u128 = 0;
    let mut total_rust_ms: u128 = 0;
    let mut total_files: usize = 0;

    for result in results {
        if result.result == VerifyResult::Skipped {
            println!(
                "║     {}     ║ {:>16} ║ {:>16} ║ {:>11} ║ {:>19} ║",
                result.drive_letter, "—", "—", "SKIPPED", "—"
            );
            continue;
        }

        let cpp_time = format_duration_ms(result.cpp_time_ms);
        let rust_time = format_duration_ms(result.rust_time_ms);

        #[allow(clippy::cast_precision_loss)]
        let speedup = if result.rust_time_ms > 0 {
            result.cpp_time_ms as f64 / result.rust_time_ms as f64
        } else {
            0.0
        };

        let speedup_str = if speedup >= 1.0 {
            format!("{:.2}x faster", speedup)
        } else if speedup > 0.0 {
            format!("{:.2}x slower", 1.0 / speedup)
        } else {
            "N/A".to_string()
        };

        #[allow(clippy::cast_precision_loss)]
        let files_per_sec = if result.rust_time_ms > 0 {
            (result.rust_lines as f64) / (result.rust_time_ms as f64 / 1000.0)
        } else {
            0.0
        };

        total_cpp_ms += result.cpp_time_ms;
        total_rust_ms += result.rust_time_ms;
        total_files += result.rust_lines;

        println!(
            "║     {}     ║ {:>16} ║ {:>16} ║ {:>11} ║ {:>17.0}/s ║",
            result.drive_letter, cpp_time, rust_time, speedup_str, files_per_sec
        );
    }

    // Print totals
    if results.len() > 1 {
        println!("╠═══════════╬══════════════════╬══════════════════╬═════════════╬═════════════════════╣");

        let total_cpp = format_duration_ms(total_cpp_ms);
        let total_rust = format_duration_ms(total_rust_ms);

        #[allow(clippy::cast_precision_loss)]
        let total_speedup = if total_rust_ms > 0 {
            total_cpp_ms as f64 / total_rust_ms as f64
        } else {
            0.0
        };

        let speedup_str = if total_speedup >= 1.0 {
            format!("{:.2}x faster", total_speedup)
        } else if total_speedup > 0.0 {
            format!("{:.2}x slower", 1.0 / total_speedup)
        } else {
            "N/A".to_string()
        };

        #[allow(clippy::cast_precision_loss)]
        let avg_files_per_sec = if total_rust_ms > 0 {
            (total_files as f64) / (total_rust_ms as f64 / 1000.0)
        } else {
            0.0
        };

        println!(
            "║   TOTAL   ║ {:>16} ║ {:>16} ║ {:>11} ║ {:>17.0}/s ║",
            total_cpp, total_rust, speedup_str, avg_files_per_sec
        );
    }

    println!(
        "╚═══════════╩══════════════════╩══════════════════╩═════════════╩═════════════════════╝"
    );
}

/// Format milliseconds as a human-readable string
fn format_duration_ms(ms: u128) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        let duration = Duration::from_millis(ms as u64);
        let total_secs = duration.as_secs_f64();
        if total_secs < 60.0 {
            format!("{:.2}s", total_secs)
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let mins = (total_secs / 60.0).floor() as u64;
            let secs = total_secs % 60.0;
            format!("{}m {:.1}s", mins, secs)
        }
    }
}
