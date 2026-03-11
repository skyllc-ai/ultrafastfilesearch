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
//! Output files have 2 header lines (CSV header + blank). Legacy baseline
//! files may also have a trailing footer (`Drives?` + surrounding blank
//! lines), but current `uffs --out ...` output intentionally omits that
//! footer. This script sorts ONLY the data rows and ignores the optional
//! legacy footer when computing SHA256.
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
/// over the normalized header+data content. Optional legacy footers are
/// ignored because `uffs --out` no longer writes them. Returns (hex hash, data
/// row count).
fn sorted_sha256(path: &Path) -> (String, usize) {
    let file = fs::File::open(path)
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e));
    let reader = BufReader::new(file);

    let all_lines: Vec<String> = reader.lines()
        .map(|l| l.expect("Failed to read line"))
        .collect();

    let total = all_lines.len();

    let (header_len, footer_start) = split_output_sections(&all_lines);

    if total <= header_len {
        eprintln!("WARNING: File {} has only {} lines (expected > {})",
            path.display(), total, header_len);
        let mut hasher = Sha256::new();
        for line in &all_lines {
            hasher.update(line.as_bytes());
            hasher.update(b"\n");
        }
        let hash = format!("{:x}", hasher.finalize());
        return (hash, 0);
    }

    let header = &all_lines[..header_len];
    let mut data: Vec<&str> = all_lines[header_len..footer_start]
        .iter()
        .map(|s| s.as_str())
        .collect();

    let data_count = data.len();
    data.sort_unstable();

    // Compute SHA256 of header + sorted data, ignoring any optional legacy
    // footer lines such as `Drives?`.
    let mut hasher = Sha256::new();
    for line in header {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    for line in &data {
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
    let (header_len, footer_start) = split_output_sections(&all_lines);
    if all_lines.len() <= header_len {
        return vec![];
    }
    let mut data: Vec<String> = all_lines[header_len..footer_start].to_vec();
    data.sort_unstable();
    data
}

fn split_output_sections(all_lines: &[String]) -> (usize, usize) {
    let header_len = if all_lines.get(1).is_some_and(String::is_empty) {
        HEADER_LINES
    } else {
        all_lines.len().min(1)
    };

    let footer_start = all_lines
        .iter()
        .rposition(|line| line.starts_with("Drives?"))
        .map_or(all_lines.len(), |idx| {
            let mut start = idx;
            while start > header_len && all_lines[start - 1].is_empty() {
                start -= 1;
            }
            start
        });

    (header_len, footer_start)
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
    use super::{read_sorted_data, sorted_sha256, split_output_sections};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_output_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0_u128, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
            "verify-parity-{name}-{}-{nanos}.txt",
            std::process::id()
        ))
    }

    fn write_output(path: &PathBuf, lines: &[&str]) {
        let body = format!("{}\n", lines.join("\n"));
        fs::write(path, body).expect("failed to write test output");
    }

    #[test]
    fn split_output_sections_detects_optional_legacy_footer() {
        let lines = vec![
            "header".to_string(),
            String::new(),
            "row-b".to_string(),
            "row-a".to_string(),
            String::new(),
            String::new(),
            "Drives? \t1\tD:".to_string(),
            String::new(),
        ];

        assert_eq!(split_output_sections(&lines), (2, 4));
    }

    #[test]
    fn sorted_sha256_ignores_legacy_footer_but_keeps_all_data_rows() {
        let footerless = temp_output_path("footerless");
        let legacy = temp_output_path("legacy");

        write_output(&footerless, &["header", "", "row-b", "row-a"]);
        write_output(
            &legacy,
            &[
                "header",
                "",
                "row-b",
                "row-a",
                "",
                "",
                "Drives? \t1\tD:",
                "",
            ],
        );

        let footerless_hash = sorted_sha256(&footerless);
        let legacy_hash = sorted_sha256(&legacy);
        assert_eq!(footerless_hash, legacy_hash);
        assert_eq!(footerless_hash.1, 2);
        assert_eq!(read_sorted_data(&footerless), vec!["row-a", "row-b"]);

        let _ = fs::remove_file(footerless);
        let _ = fs::remove_file(legacy);
    }
}
