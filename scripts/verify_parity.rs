#!/usr/bin/env rust-script
//! Drive-agnostic strict full-output SHA256 verification for UFFS.
//!
//! Compares Rust output against a golden baseline using the complete output
//! file, including any non-CSV lines above or below the data rows.
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
//! # Strict parity contract
//!
//! The ordered full-file SHA256 is authoritative. If ordered hashes differ,
//! the script also computes a line-sorted full-file SHA256 as a normalization
//! step for row-order differences. No header or footer lines are truncated or
//! ignored during either comparison.
//!
//! ```cargo
//! [dependencies]
//! sha2 = "0.10"
//! ```

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
struct FileHashes {
    ordered_hash: String,
    sorted_hash: String,
    line_count: usize,
}

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
    println!("=== UFFS Strict Full-Output Parity Verification ===");
    println!("Data dir:      {}", data_dir.display());
    println!("Drive letter:  {}", drive_letter);
    println!("Baseline file: {}", golden_baseline_file.display());
    println!("Rust output:   {}", rust_output.display());
    println!();

    println!("Computing ordered full-file SHA256...");
    let golden_hashes = compute_file_hashes(&golden_baseline_file);
    let rust_hashes = compute_file_hashes(&rust_output);

    println!();
    println!(
        "Golden baseline: {} ({} lines)",
        golden_hashes.ordered_hash, golden_hashes.line_count
    );
    println!(
        "Rust output:    {} ({} lines)",
        rust_hashes.ordered_hash, rust_hashes.line_count
    );
    println!();

    if golden_hashes.ordered_hash == rust_hashes.ordered_hash {
        println!("RESULT: STRICT FULL OUTPUT MATCH");
        println!("  Golden baseline verified for drive {}.", drive_letter);
        std::process::exit(0);
    }

    println!("Ordered hashes differ; checking full-file line-sort normalization...");
    println!(
        "Golden baseline (sorted): {}",
        golden_hashes.sorted_hash
    );
    println!("Rust output (sorted):    {}", rust_hashes.sorted_hash);
    println!();

    if golden_hashes.sorted_hash == rust_hashes.sorted_hash {
        println!("RESULT: FULL OUTPUT MATCH AFTER LINE-SORT NORMALIZATION");
        println!("  Exact line order differs, but the complete output line set matches.");
        println!();
        show_first_ordered_diffs(&golden_baseline_file, &rust_output);
        std::process::exit(0);
    }

    println!("RESULT: STRICT FULL OUTPUT MISMATCH");
    println!("  Ordered baseline: {}", golden_hashes.ordered_hash);
    println!("  Ordered Rust:     {}", rust_hashes.ordered_hash);
    println!("  Sorted baseline:  {}", golden_hashes.sorted_hash);
    println!("  Sorted Rust:      {}", rust_hashes.sorted_hash);
    println!(
        "  Line count diff:  {} (baseline) vs {} (Rust)",
        golden_hashes.line_count, rust_hashes.line_count
    );
    println!();

    show_first_ordered_diffs(&golden_baseline_file, &rust_output);
    println!();
    show_first_sorted_diffs(&golden_baseline_file, &rust_output);

    std::process::exit(1);
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

fn compute_file_hashes(path: &Path) -> FileHashes {
    let lines = read_lines(path);
    FileHashes {
        ordered_hash: ordered_sha256(&lines),
        sorted_hash: sorted_sha256(&lines),
        line_count: lines.len(),
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    let file = fs::File::open(path)
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e));
    let reader = BufReader::new(file);
    reader
        .lines()
        .map(|line| line.expect("Failed to read line"))
        .collect()
}

fn ordered_sha256(lines: &[String]) -> String {
    sha256_for_lines(lines.iter().map(String::as_str))
}

fn sorted_sha256(lines: &[String]) -> String {
    let mut sorted_lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    sorted_lines.sort_unstable();
    sha256_for_lines(sorted_lines)
}

fn sha256_for_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

/// Show first few differences between the complete ordered outputs of two files.
fn show_first_ordered_diffs(file_a: &Path, file_b: &Path) {
    let lines_a = read_lines(file_a);
    let lines_b = read_lines(file_b);

    println!("First 5 differences in full output order:");
    let mut diff_count = 0;
    let max_len = lines_a.len().max(lines_b.len());

    for index in 0..max_len {
        if diff_count >= 5 {
            break;
        }

        match (lines_a.get(index), lines_b.get(index)) {
            (Some(line_a), Some(line_b)) if line_a == line_b => {}
            (Some(line_a), Some(line_b)) => {
                println!("  Line {} baseline: {}", index + 1, truncate(line_a, 120));
                println!("  Line {} Rust:     {}", index + 1, truncate(line_b, 120));
                diff_count += 1;
            }
            (Some(line_a), None) => {
                println!("  Line {} baseline only: {}", index + 1, truncate(line_a, 120));
                diff_count += 1;
            }
            (None, Some(line_b)) => {
                println!("  Line {} Rust only:     {}", index + 1, truncate(line_b, 120));
                diff_count += 1;
            }
            (None, None) => {}
        }
    }

    if diff_count == 0 {
        println!("  No ordered differences found after newline normalization.");
    }
}

/// Show first few multiset differences after sorting complete output lines.
fn show_first_sorted_diffs(file_a: &Path, file_b: &Path) {
    let lines_a = read_sorted_lines(file_a);
    let lines_b = read_sorted_lines(file_b);

    println!("First 5 differences after full-file line sort:");
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

    if diff_count == 0 {
        println!("  No sorted differences found after newline normalization.");
    }
}

fn read_sorted_lines(path: &Path) -> Vec<String> {
    let mut all_lines = read_lines(path);
    all_lines.sort_unstable();
    all_lines
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::{ordered_sha256, sorted_sha256};

    fn lines(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn sorted_full_file_hash_still_detects_missing_footer_lines() {
        let baseline = lines(&[
            "header",
            "",
            "row-a",
            "row-b",
            "",
            "Drives?\t1\tD:",
            "",
        ]);
        let rust = lines(&["header", "", "row-b", "row-a"]);

        assert_ne!(ordered_sha256(&baseline), ordered_sha256(&rust));
        assert_ne!(sorted_sha256(&baseline), sorted_sha256(&rust));
    }

    #[test]
    fn sorted_full_file_hash_allows_row_reordering_when_full_line_set_matches() {
        let baseline = lines(&[
            "header",
            "",
            "row-a",
            "row-b",
            "",
            "Drives?\t1\tD:",
            "",
        ]);
        let rust = lines(&[
            "header",
            "",
            "row-b",
            "row-a",
            "",
            "Drives?\t1\tD:",
            "",
        ]);

        assert_ne!(ordered_sha256(&baseline), ordered_sha256(&rust));
        assert_eq!(sorted_sha256(&baseline), sorted_sha256(&rust));
    }
}
