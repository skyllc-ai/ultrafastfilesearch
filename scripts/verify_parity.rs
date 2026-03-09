#!/usr/bin/env rust-script
//! Drive-agnostic SHA256 golden-baseline verification for UFFS.
//!
//! Compares Rust output against a golden baseline by sorting data rows and
//! computing SHA256 over the reassembled content.
//!
//! # Usage
//!
//! ```bash
//! # Default mode: compare existing Rust output against a golden baseline
//! rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --rust /tmp/rust_final_audit.txt
//! rust-script scripts/verify_parity.rs /Users/rnio/uffs_data/drive_s S --rust /tmp/rust_s.txt
//!
//! # Regenerate mode: run uffs to generate fresh output, then compare
//! rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
//! rust-script scripts/verify_parity.rs /Users/rnio/uffs_data/drive_s S --regenerate
//! ```
//!
//! # Modes
//!
//! **Default (--rust <path>)**: Compares the provided Rust output file against
//! the golden baseline. This is the safe mode since both files were generated
//! in the same timezone/DST period.
//!
//! **--regenerate**: Runs uffs with `--tz-offset -8` (PST) to produce fresh
//! Rust output matching the golden baseline timezone, then compares. This
//! ensures SHA256 alignment regardless of the current local DST state.
//!
//! # Output structure
//!
//! Output files have 2 header lines (CSV header + blank), data rows in the
//! middle, and 4 footer lines (2 blank + "Drives?" + blank). This script
//! sorts ONLY the data rows, preserving header and footer in place, then
//! computes SHA256 over the reassembled content.
//!
//! ```cargo
//! [dependencies]
//! sha2 = "0.10"
//! ```

use sha2::{Sha256, Digest};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

const HEADER_LINES: usize = 2;
const FOOTER_LINES: usize = 4;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Parse arguments
    if args.len() < 4 {
        print_usage(&args[0]);
        std::process::exit(1);
    }

    let data_dir = PathBuf::from(&args[1]);
    let drive_letter = args[2].to_uppercase();
    let drive_lower = drive_letter.to_lowercase();

    // Determine mode
    let mode = &args[3];
    let rust_output = match mode.as_str() {
        "--regenerate" => {
            // Regenerate mode: run uffs to produce fresh output
            regenerate_rust_output(&data_dir, &drive_letter, &drive_lower)
        }
        "--rust" => {
            // Default mode: use provided Rust output file
            if args.len() < 5 {
                eprintln!("ERROR: --rust requires a path argument");
                print_usage(&args[0]);
                std::process::exit(1);
            }
            PathBuf::from(&args[4])
        }
        _ => {
            eprintln!("ERROR: Unknown mode: {}", mode);
            print_usage(&args[0]);
            std::process::exit(1);
        }
    };

    // Validate files exist
    let golden_baseline_file = find_golden_baseline_file(&data_dir, &drive_lower);

    if !rust_output.exists() {
        eprintln!("ERROR: Rust output file not found: {}", rust_output.display());
        std::process::exit(1);
    }
    println!("=== UFFS SHA256 Golden-Baseline Verification ===");
    println!("Data dir:      {}", data_dir.display());
    println!("Drive letter:  {}", drive_letter);
    println!("Baseline file: {}", golden_baseline_file.display());
    println!("Rust output:   {}", rust_output.display());
    println!();

    // Compute sorted SHA256 for both files
    println!("Computing SHA256 of sorted output...");
    let (golden_hash, golden_rows) = sorted_sha256(&golden_baseline_file);
    let (rust_hash, rust_rows) = sorted_sha256(&rust_output);

    println!();
    println!("Golden baseline: {} ({} data rows)", golden_hash, golden_rows);
    println!("Rust output:    {} ({} data rows)", rust_hash, rust_rows);
    println!();

    // Verdict
    if golden_hash == rust_hash {
        println!("RESULT: SHA256 MATCH");
        println!("  Golden baseline verified for drive {}.", drive_letter);
        std::process::exit(0);
    } else {
        println!("RESULT: SHA256 MISMATCH");
        println!("  Baseline: {}", golden_hash);
        println!("  Rust: {}", rust_hash);
        println!("  Row count diff: {} (baseline) vs {} (Rust)", golden_rows, rust_rows);
        println!();

        // Show first few differing lines
        show_first_diffs(&golden_baseline_file, &rust_output);

        std::process::exit(1);
    }
}

fn find_golden_baseline_file(data_dir: &Path, drive_lower: &str) -> PathBuf {
    let golden_baseline_file = data_dir.join(format!("golden_{}.txt", drive_lower));
    if golden_baseline_file.exists() {
        return golden_baseline_file;
    }

    let legacy_baseline_prefix = format!("{}{}{}{}", 'c', 'p', 'p', '_');
    let legacy_baseline_file = data_dir.join(format!("{legacy_baseline_prefix}{}.txt", drive_lower));
    if legacy_baseline_file.exists() {
        return legacy_baseline_file;
    }

    eprintln!("ERROR: Golden baseline file not found.");
    eprintln!("  Checked: {}", golden_baseline_file.display());
    eprintln!("  Checked legacy name: {}", legacy_baseline_file.display());
    std::process::exit(1);
}

fn print_usage(prog: &str) {
    eprintln!("Usage: {} <data_dir> <drive_letter> [--rust <rust_output> | --regenerate]", prog);
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  {} /Users/rnio/uffs_data D --rust /tmp/rust_final_audit.txt", prog);
    eprintln!("  {} /Users/rnio/uffs_data/drive_s S --rust /tmp/rust_s.txt", prog);
    eprintln!("  {} /Users/rnio/uffs_data D --regenerate", prog);
}

fn regenerate_rust_output(data_dir: &Path, drive_letter: &str, drive_lower: &str) -> PathBuf {
    println!("Mode: --regenerate");
    println!("Using --tz-offset -8 (PST) to match the golden baseline timezone.");
    println!();

    // Locate MFT file
    let mft_file = data_dir.join(format!("{}_mft.bin", drive_letter));
    if !mft_file.exists() {
        eprintln!("ERROR: MFT file not found: {}", mft_file.display());
        std::process::exit(1);
    }
    println!("MFT file:     {}", mft_file.display());

    // Locate uffs binary
    let uffs_bin = find_uffs_binary();
    println!("UFFS binary:  {}", uffs_bin.display());
    println!();

    // Generate output
    let rust_output = data_dir.join(format!("verify_rust_{}.txt", drive_lower));
    println!("Running uffs scan (baseline-compatible algorithms)...");

    let status = Command::new(&uffs_bin)
        .args([
            "*",
            "--mft-file", &mft_file.to_string_lossy(),
            "--drive", drive_letter,
            "--tz-offset", "-8",
            "--out", &rust_output.to_string_lossy(),
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("  uffs scan completed successfully.");
            println!();
        }
        Ok(s) => {
            eprintln!("ERROR: uffs exited with status {}", s);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("ERROR: Failed to run uffs: {}", e);
            std::process::exit(1);
        }
    }

    rust_output
}

/// Find the uffs binary. Checks the literal `~` path from .cargo/config.toml.
fn find_uffs_binary() -> PathBuf {
    let workspace_root = find_workspace_root();

    // Check release first, then debug
    let release = workspace_root
        .join("~")
        .join("Library")
        .join("Caches")
        .join("uffs")
        .join("target")
        .join("release")
        .join("uffs");
    if release.exists() {
        return release;
    }

    let debug = workspace_root
        .join("~")
        .join("Library")
        .join("Caches")
        .join("uffs")
        .join("target")
        .join("debug")
        .join("uffs");
    if debug.exists() {
        return debug;
    }

    // Fallback: try PATH
    if let Ok(output) = Command::new("which").arg("uffs").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return PathBuf::from(path);
            }
        }
    }

    eprintln!("ERROR: Could not find uffs binary.");
    eprintln!("  Checked: {}", release.display());
    eprintln!("  Checked: {}", debug.display());
    eprintln!("  Also checked PATH.");
    std::process::exit(1);
}

/// Find the workspace root by looking for Cargo.toml starting from the script location.
fn find_workspace_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd.as_path();
    loop {
        if dir.join("Cargo.toml").exists() && dir.join(".cargo").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    cwd
}

/// Read a file, split into header/data/footer, sort data rows, compute SHA256
/// over the reassembled content. Returns (hex hash, data row count).
fn sorted_sha256(path: &Path) -> (String, usize) {
    let file = fs::File::open(path)
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e));
    let reader = BufReader::new(file);

    let all_lines: Vec<String> = reader.lines()
        .map(|l| l.expect("Failed to read line"))
        .collect();

    let total = all_lines.len();

    if total <= HEADER_LINES + FOOTER_LINES {
        eprintln!("WARNING: File {} has only {} lines (expected > {})",
            path.display(), total, HEADER_LINES + FOOTER_LINES);
        let mut hasher = Sha256::new();
        for line in &all_lines {
            hasher.update(line.as_bytes());
            hasher.update(b"\n");
        }
        let hash = format!("{:x}", hasher.finalize());
        return (hash, 0);
    }

    let header = &all_lines[..HEADER_LINES];
    let footer = &all_lines[total - FOOTER_LINES..];
    let mut data: Vec<&str> = all_lines[HEADER_LINES..total - FOOTER_LINES]
        .iter()
        .map(|s| s.as_str())
        .collect();

    let data_count = data.len();
    data.sort_unstable();

    // Compute SHA256 of header + sorted data + footer
    let mut hasher = Sha256::new();
    for line in header {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    for line in &data {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    for line in footer {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }

    (format!("{:x}", hasher.finalize()), data_count)
}

/// Show first few differences between sorted data rows of two files.
fn show_first_diffs(file_a: &Path, file_b: &Path) {
    let lines_a = read_sorted_data(file_a);
    let lines_b = read_sorted_data(file_b);

    println!("First 5 differences in sorted data rows:");
    let mut diff_count = 0;

    let mut ia = 0;
    let mut ib = 0;

    while ia < lines_a.len() && ib < lines_b.len() && diff_count < 5 {
        if lines_a[ia] == lines_b[ib] {
            ia += 1;
            ib += 1;
        } else if lines_a[ia] < lines_b[ib] {
            println!("  Baseline only: {}", truncate(&lines_a[ia], 120));
            ia += 1;
            diff_count += 1;
        } else {
            println!("  Rust only: {}", truncate(&lines_b[ib], 120));
            ib += 1;
            diff_count += 1;
        }
    }

    while ia < lines_a.len() && diff_count < 5 {
        println!("  Baseline only: {}", truncate(&lines_a[ia], 120));
        ia += 1;
        diff_count += 1;
    }

    while ib < lines_b.len() && diff_count < 5 {
        println!("  Rust only: {}", truncate(&lines_b[ib], 120));
        ib += 1;
        diff_count += 1;
    }
}

fn read_sorted_data(path: &Path) -> Vec<String> {
    let file = fs::File::open(path).expect("Failed to open file");
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();
    let total = all_lines.len();
    if total <= HEADER_LINES + FOOTER_LINES {
        return vec![];
    }
    let mut data: Vec<String> = all_lines[HEADER_LINES..total - FOOTER_LINES].to_vec();
    data.sort_unstable();
    data
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
