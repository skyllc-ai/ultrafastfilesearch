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

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use sha2::{Digest, Sha256};

#[derive(Debug)]
struct FileHashes {
    ordered_hash: String,
    sorted_hash: String,
    line_count: usize,
}

#[derive(Debug)]
struct UffsReleaseArtifact {
    workspace_root: PathBuf,
    cargo_target_dir: PathBuf,
    binary_path: PathBuf,
    target_dir_warning: Option<&'static str>,
}
fn main() {
    let args: Vec<String> = env::args().collect();

    // Parse arguments
    if args.len() < 4 {
        print_usage(&args[0]);
        std::process::exit(1);
    }

    let base_dir = PathBuf::from(&args[1]);
    let drive_letter = args[2].to_uppercase();
    let drive_lower = drive_letter.to_lowercase();

    // Resolve the actual drive data directory (supports drive_<letter> subdirs)
    let drive_dir = resolve_drive_dir(&base_dir, &drive_lower);

    // Parse optional arguments
    let explicit_tz = parse_tz_offset(&args);
    let custom_bin = parse_bin_path(&args);

    // Determine mode
    let mode = &args[3];
    let rust_output = match mode.as_str() {
        "--regenerate" => {
            // Find baseline first for tz auto-detection
            let golden_baseline = find_golden_baseline_file(&drive_dir, &drive_lower);

            // Auto-detect or use explicit tz
            let tz_offset =
                explicit_tz.unwrap_or_else(|| detect_tz_from_baseline(&golden_baseline));

            // Regenerate mode: run uffs to produce fresh output
            regenerate_rust_output(
                &drive_dir,
                &drive_letter,
                &drive_lower,
                tz_offset,
                custom_bin.as_deref(),
            )
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
    let golden_baseline_file = find_golden_baseline_file(&drive_dir, &drive_lower);

    if !rust_output.exists() {
        eprintln!(
            "ERROR: Rust output file not found: {}",
            rust_output.display()
        );
        std::process::exit(1);
    }
    println!("=== UFFS Strict Full-Output Parity Verification ===");
    println!("Base dir:      {}", base_dir.display());
    println!("Drive dir:     {}", drive_dir.display());
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
    println!("Golden baseline (sorted): {}", golden_hashes.sorted_hash);
    println!("Rust output (sorted):    {}", rust_hashes.sorted_hash);
    println!();

    if golden_hashes.sorted_hash == rust_hashes.sorted_hash {
        println!("RESULT: FULL OUTPUT MATCH AFTER LINE-SORT NORMALIZATION");
        println!("  Exact line order differs (different traversal order), but content matches.");
        println!("  This is acceptable — C++ and Rust walk the MFT/tree in different orders.");
        std::process::exit(0);
    }

    println!("RESULT: STRICT FULL OUTPUT MISMATCH");
    println!("  Sorted baseline:  {}", golden_hashes.sorted_hash);
    println!("  Sorted Rust:      {}", rust_hashes.sorted_hash);
    println!(
        "  Line count:       {} (baseline) vs {} (Rust)",
        golden_hashes.line_count, rust_hashes.line_count
    );
    println!();

    // Show SORTED diffs first — this is the meaningful comparison
    // (Ordered diffs are just noise from different traversal order)
    show_first_sorted_diffs(&golden_baseline_file, &rust_output);

    std::process::exit(1);
}

/// Resolves the drive data directory.
///
/// Supports two directory structures:
/// 1. New: `<base>/drive_<letter>/` (e.g., `/Users/rnio/uffs_data/drive_d/`)
/// 2. Legacy: `<base>/` with files directly in base (e.g.,
///    `/Users/rnio/uffs_data/D_mft.bin`)
fn resolve_drive_dir(base_dir: &Path, drive_lower: &str) -> PathBuf {
    // Try new structure first: base/drive_<letter>/
    let new_style = base_dir.join(format!("drive_{}", drive_lower));
    if new_style.exists() && new_style.is_dir() {
        return new_style;
    }
    // Fall back to legacy: files directly in base_dir
    base_dir.to_path_buf()
}

fn find_golden_baseline_file(data_dir: &Path, drive_lower: &str) -> PathBuf {
    // Try various naming conventions in order of preference
    let candidates = [
        format!("golden_{}.txt", drive_lower),
        format!("cpp_{}.txt", drive_lower), // C++ baseline output
        format!("rust_live_{}.txt", drive_lower), // Live scan output (when comparing offline)
    ];

    for name in &candidates {
        let path = data_dir.join(name);
        if path.exists() {
            return path;
        }
    }

    eprintln!(
        "ERROR: Golden baseline file not found in {}",
        data_dir.display()
    );
    eprintln!("  Checked:");
    for name in &candidates {
        eprintln!("    - {}", name);
    }
    std::process::exit(1);
}

fn print_usage(prog: &str) {
    eprintln!(
        "Usage: {} <base_dir> <drive_letter> [--rust <rust_output> | --regenerate] [OPTIONS]",
        prog
    );
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --tz OFFSET   Timezone offset in hours (default: auto-detect from baseline)");
    eprintln!("                Auto-detection uses Pacific time DST rules based on capture date.");
    eprintln!("                Override with -7 for PDT (Mar-Nov), -8 for PST (Nov-Mar)");
    eprintln!("  --bin PATH    Path to uffs binary (default: auto-detect from cargo metadata)");
    eprintln!();
    eprintln!("The script auto-detects the drive data directory:");
    eprintln!("  - New layout: <base_dir>/drive_<letter>/  (e.g., uffs_data/drive_d/)");
    eprintln!("  - Legacy:     <base_dir>/                 (files directly in base)");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  {} /Users/rnio/uffs_data F --regenerate", prog);
    eprintln!(
        "  {} /Users/rnio/uffs_data F --regenerate --tz -8  # override auto-detect",
        prog
    );
    eprintln!(
        "  {} /Users/rnio/uffs_data F --regenerate --bin ./target/release/uffs",
        prog
    );
    eprintln!(
        "  {} /Users/rnio/uffs_data D --rust /tmp/rust_output.txt",
        prog
    );
}

/// Parse --tz argument from command line. Returns None if not specified
/// (auto-detect).
fn parse_tz_offset(args: &[String]) -> Option<i32> {
    for i in 0..args.len() {
        if args[i] == "--tz" && i + 1 < args.len() {
            return Some(args[i + 1].parse().unwrap_or(-7));
        }
    }
    None // Auto-detect from baseline
}

/// Auto-detect timezone offset from C++ baseline file.
/// Extracts a date from the baseline and determines PDT vs PST based on Pacific
/// DST rules. Pacific DST: 2nd Sunday of March 2:00 AM to 1st Sunday of
/// November 2:00 AM
fn detect_tz_from_baseline(baseline_path: &Path) -> i32 {
    // Read first few lines to find a data line with a timestamp
    let file = match std::fs::File::open(baseline_path) {
        Ok(f) => f,
        Err(_) => return -7, // Default to PDT if can't read
    };
    let reader = std::io::BufReader::new(file);

    for line in std::io::BufRead::lines(reader).take(10).flatten() {
        // Look for a timestamp pattern: YYYY-MM-DD HH:MM:SS
        if let Some(date_match) = extract_date_from_line(&line) {
            let offset = pacific_tz_offset(date_match.0, date_match.1, date_match.2);
            println!(
                "Auto-detected timezone from baseline date {}-{:02}-{:02}: {} ({})",
                date_match.0,
                date_match.1,
                date_match.2,
                offset,
                if offset == -7 { "PDT" } else { "PST" }
            );
            return offset;
        }
    }

    println!("Could not auto-detect timezone, defaulting to -7 (PDT)");
    -7
}

/// Extract (year, month, day) from a CSV line containing a timestamp.
fn extract_date_from_line(line: &str) -> Option<(i32, u32, u32)> {
    // Look for pattern: YYYY-MM-DD (4 digits, dash, 2 digits, dash, 2 digits)
    let bytes = line.as_bytes();
    for i in 0..bytes.len().saturating_sub(10) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4] == b'-'
            && bytes[i + 5].is_ascii_digit()
            && bytes[i + 6].is_ascii_digit()
            && bytes[i + 7] == b'-'
            && bytes[i + 8].is_ascii_digit()
            && bytes[i + 9].is_ascii_digit()
        {
            let year: i32 = line[i..i + 4].parse().ok()?;
            let month: u32 = line[i + 5..i + 7].parse().ok()?;
            let day: u32 = line[i + 8..i + 10].parse().ok()?;
            if year >= 2000 && year <= 2100 && month >= 1 && month <= 12 && day >= 1 && day <= 31 {
                return Some((year, month, day));
            }
        }
    }
    None
}

/// Determine Pacific timezone offset for a given date.
/// Returns -7 for PDT (Daylight), -8 for PST (Standard).
/// Pacific DST: 2nd Sunday of March 2:00 AM to 1st Sunday of November 2:00 AM
fn pacific_tz_offset(year: i32, month: u32, day: u32) -> i32 {
    // Simple rule: March 15 - November 1 is approximately PDT
    // More precise would calculate exact Sunday transitions, but this is close
    // enough
    let dst_start = (3, 8); // March 8 (earliest 2nd Sunday)
    let dst_end = (11, 1); // November 1 (earliest 1st Sunday)

    if month > dst_start.0 && month < dst_end.0 {
        -7 // PDT: April through October
    } else if month == dst_start.0 && day >= 8 {
        -7 // PDT: March 8+ (approx 2nd Sunday)
    } else if month == dst_end.0 && day < 8 {
        -7 // PDT: November 1-7 (before 1st Sunday ends DST)
    } else {
        -8 // PST: November 8+ through early March
    }
}

/// Parse --bin argument from command line. Returns None if not specified.
fn parse_bin_path(args: &[String]) -> Option<PathBuf> {
    for i in 0..args.len() {
        if args[i] == "--bin" && i + 1 < args.len() {
            return Some(PathBuf::from(&args[i + 1]));
        }
    }
    None
}

fn regenerate_rust_output(
    data_dir: &Path,
    drive_letter: &str,
    drive_lower: &str,
    tz_offset: i32,
    custom_bin: Option<&Path>,
) -> PathBuf {
    let tz_str = format!("{}", tz_offset);
    let tz_label = match tz_offset {
        -7 => "PDT (Pacific Daylight)",
        -8 => "PST (Pacific Standard)",
        _ => "custom",
    };

    println!("Mode: --regenerate");
    println!(
        "Using --tz-offset {} ({}) to match the golden baseline timezone.",
        tz_offset, tz_label
    );
    println!();

    // Locate MFT file
    let mft_file = data_dir.join(format!("{}_mft.bin", drive_letter));
    if !mft_file.exists() {
        eprintln!("ERROR: MFT file not found: {}", mft_file.display());
        std::process::exit(1);
    }
    println!("MFT file:     {}", mft_file.display());

    // Determine which binary to use
    let binary_path = if let Some(custom) = custom_bin {
        println!("Using custom binary:   {}", custom.display());
        if !custom.exists() {
            eprintln!("ERROR: Custom binary not found: {}", custom.display());
            std::process::exit(1);
        }
        custom.to_path_buf()
    } else {
        // Locate authoritative workspace release artifact
        let artifact = find_workspace_release_artifact();
        println!(
            "Workspace root:        {}",
            artifact.workspace_root.display()
        );
        println!(
            "Cargo target dir:      {}",
            artifact.cargo_target_dir.display()
        );
        println!("UFFS release artifact: {}", artifact.binary_path.display());
        println!(
            "Artifact provenance:   cargo metadata target_directory → release/{}",
            uffs_binary_name()
        );
        if let Some(target_dir_warning) = artifact.target_dir_warning {
            println!("Target dir note:       {target_dir_warning}");
        }
        artifact.binary_path
    };
    println!();

    // Generate output
    let rust_output = data_dir.join(format!("verify_rust_{}.txt", drive_lower));
    println!("Running uffs scan (baseline-compatible algorithms)...");

    let status = Command::new(&binary_path)
        .args([
            "*",
            "--mft-file",
            &mft_file.to_string_lossy(),
            "--drive",
            drive_letter,
            "--tz-offset",
            &tz_str,
            "--out",
            &rust_output.to_string_lossy(),
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

/// Find the authoritative workspace release artifact via Cargo metadata.
fn find_workspace_release_artifact() -> UffsReleaseArtifact {
    let workspace_root = find_workspace_root();
    let cargo_target_dir = cargo_target_directory(&workspace_root);
    let target_dir_warning = literal_tilde_target_dir_warning(&cargo_target_dir);
    let release_artifact = cargo_target_dir.join("release").join(uffs_binary_name());
    if release_artifact.exists() {
        return UffsReleaseArtifact {
            workspace_root,
            cargo_target_dir,
            binary_path: release_artifact,
            target_dir_warning,
        };
    }

    eprintln!("ERROR: Fresh workspace release artifact not found.");
    eprintln!("  Workspace root: {}", workspace_root.display());
    eprintln!("  Cargo target dir: {}", cargo_target_dir.display());
    eprintln!(
        "  Expected release artifact: {}",
        release_artifact.display()
    );
    if let Some(target_dir_warning) = target_dir_warning {
        eprintln!("  Target dir note: {target_dir_warning}");
        eprintln!(
            "  Fix the checked-in config or set CARGO_TARGET_DIR to an explicit absolute path before rebuilding."
        );
    }
    eprintln!("  Build it first with: cargo build --release -p uffs-cli --bin uffs");
    std::process::exit(1);
}

fn literal_tilde_target_dir_warning(target_dir: &Path) -> Option<&'static str> {
    path_has_literal_tilde_component(target_dir).then_some(
        "literal `~` path component detected; Cargo config paths are config-relative, not home-directory expansions",
    )
}

fn path_has_literal_tilde_component(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "~")
}

fn cargo_target_directory(workspace_root: &Path) -> PathBuf {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(workspace_root)
        .output()
        .unwrap_or_else(|error| {
            eprintln!("ERROR: Failed to run cargo metadata: {error}");
            std::process::exit(1);
        });

    if !output.status.success() {
        eprintln!("ERROR: cargo metadata failed with status {}", output.status);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprintln!("  stderr: {}", stderr.trim());
        }
        std::process::exit(1);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw_target_dir =
        extract_json_string_field(&stdout, "target_directory").unwrap_or_else(|| {
            eprintln!("ERROR: cargo metadata output did not contain target_directory");
            std::process::exit(1);
        });

    let target_dir = PathBuf::from(raw_target_dir);
    if target_dir.is_absolute() {
        target_dir
    } else {
        workspace_root.join(target_dir)
    }
}

fn extract_json_string_field(json: &str, field_name: &str) -> Option<String> {
    let needle = format!("\"{field_name}\":\"");
    let start = json.find(&needle)? + needle.len();
    let mut result = String::new();
    let mut escaped = false;

    for ch in json[start..].chars() {
        if escaped {
            match ch {
                '"' | '\\' | '/' => result.push(ch),
                'b' => result.push('\u{0008}'),
                'f' => result.push('\u{000C}'),
                'n' => result.push('\n'),
                'r' => result.push('\r'),
                't' => result.push('\t'),
                _ => return None,
            }
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => return Some(result),
            _ => result.push(ch),
        }
    }

    None
}

fn uffs_binary_name() -> &'static str {
    if cfg!(windows) {
        "uffs.exe"
    } else {
        "uffs"
    }
}

/// Find the workspace root by looking for Cargo.toml starting from the script
/// location.
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
    let file =
        fs::File::open(path).unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e));
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

// Note: ordered diff functions kept for debugging but not used in main flow.
// Sorted comparison is the meaningful one since C++ and Rust walk differently.

#[allow(dead_code)]
fn collect_ordered_diffs(
    file_a: &Path,
    file_b: &Path,
) -> Vec<(usize, Option<String>, Option<String>)> {
    let lines_a = read_lines(file_a);
    let lines_b = read_lines(file_b);
    let max_len = lines_a.len().max(lines_b.len());
    let mut diffs = Vec::new();

    for index in 0..max_len {
        match (lines_a.get(index), lines_b.get(index)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => diffs.push((index + 1, Some(a.clone()), Some(b.clone()))),
            (Some(a), None) => diffs.push((index + 1, Some(a.clone()), None)),
            (None, Some(b)) => diffs.push((index + 1, None, Some(b.clone()))),
            (None, None) => {}
        }
    }
    diffs
}

#[allow(dead_code)]
fn show_first_ordered_diffs(file_a: &Path, file_b: &Path) {
    let diffs = collect_ordered_diffs(file_a, file_b);

    if diffs.is_empty() {
        println!("No ordered differences found.");
        return;
    }

    println!("Total ordered differences: {}", diffs.len());
    println!();

    // First 5
    let first_n = 5.min(diffs.len());
    println!("=== FIRST {} DIFFERENCES ===", first_n);
    for (line_num, baseline, rust) in diffs.iter().take(first_n) {
        print_diff_pair(*line_num, baseline.as_deref(), rust.as_deref());
    }

    if diffs.len() <= 10 {
        return; // Already showed everything
    }

    // Last 5
    let last_start = diffs.len().saturating_sub(5);
    println!("\n=== LAST 5 DIFFERENCES ===");
    for (line_num, baseline, rust) in diffs.iter().skip(last_start) {
        print_diff_pair(*line_num, baseline.as_deref(), rust.as_deref());
    }

    // 10 random from middle (if enough diffs)
    if diffs.len() > 10 {
        let middle_start = first_n;
        let middle_end = last_start;
        if middle_end > middle_start {
            println!("\n=== 10 RANDOM MIDDLE DIFFERENCES ===");
            let middle: Vec<_> = diffs[middle_start..middle_end].to_vec();
            let sample_count = 10.min(middle.len());
            let mut rng_seed = diffs.len() as u64; // deterministic seed
            let mut indices: Vec<usize> = (0..middle.len()).collect();
            // Simple shuffle with LCG
            for i in (1..indices.len()).rev() {
                rng_seed = rng_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let j = (rng_seed as usize) % (i + 1);
                indices.swap(i, j);
            }
            for &idx in indices.iter().take(sample_count) {
                let (line_num, baseline, rust) = &middle[idx];
                print_diff_pair(*line_num, baseline.as_deref(), rust.as_deref());
            }
        }
    }
}

#[allow(dead_code)]
fn print_diff_pair(line_num: usize, baseline: Option<&str>, rust: Option<&str>) {
    match (baseline, rust) {
        (Some(b), Some(r)) => {
            println!("  Line {}:", line_num);
            println!("    BASELINE: {}", b);
            println!("    RUST:     {}", r);
        }
        (Some(b), None) => {
            println!("  Line {} BASELINE ONLY:", line_num);
            println!("    {}", b);
        }
        (None, Some(r)) => {
            println!("  Line {} RUST ONLY:", line_num);
            println!("    {}", r);
        }
        (None, None) => {}
    }
}

/// Collect all sorted multiset differences.
#[allow(dead_code)]
fn collect_sorted_diffs(file_a: &Path, file_b: &Path) -> (Vec<String>, Vec<String>) {
    let lines_a = read_sorted_lines(file_a);
    let lines_b = read_sorted_lines(file_b);

    let mut only_a = Vec::new();
    let mut only_b = Vec::new();

    let mut ia = 0;
    let mut ib = 0;

    while ia < lines_a.len() && ib < lines_b.len() {
        if lines_a[ia] == lines_b[ib] {
            ia += 1;
            ib += 1;
        } else if lines_a[ia] < lines_b[ib] {
            only_a.push(lines_a[ia].clone());
            ia += 1;
        } else {
            only_b.push(lines_b[ib].clone());
            ib += 1;
        }
    }
    while ia < lines_a.len() {
        only_a.push(lines_a[ia].clone());
        ia += 1;
    }
    while ib < lines_b.len() {
        only_b.push(lines_b[ib].clone());
        ib += 1;
    }
    (only_a, only_b)
}

/// Show side-by-side comparison of DIFFERENT lines from sorted files.
/// Only shows lines where baseline != rust. First 5 diffs, last 5 diffs, 10
/// random from middle.
fn show_first_sorted_diffs(file_a: &Path, file_b: &Path) {
    let sorted_baseline = read_sorted_lines(file_a);
    let sorted_rust = read_sorted_lines(file_b);

    let n = sorted_baseline.len().min(sorted_rust.len());
    if n == 0 {
        println!("No lines to compare.");
        return;
    }

    // Collect indices of lines that differ
    let diff_indices: Vec<usize> = (0..n)
        .filter(|&i| sorted_baseline[i] != sorted_rust[i])
        .collect();

    println!("\n=== SORTED SIDE-BY-SIDE COMPARISON (differences only) ===");
    println!("  Baseline lines: {}", sorted_baseline.len());
    println!("  Rust lines:     {}", sorted_rust.len());
    println!("  Lines that differ: {}", diff_indices.len());

    if diff_indices.is_empty() {
        println!("\n  ✅ All lines match!");
        return;
    }

    let total_diffs = diff_indices.len();
    let first_n = 5.min(total_diffs);
    let last_n = 5.min(total_diffs);

    // First 5 differences
    println!("\n--- FIRST {} DIFFERENCES ---", first_n);
    for &idx in diff_indices.iter().take(first_n) {
        println!("  Line {}:", idx + 1);
        println!("    BASELINE: {}", sorted_baseline[idx]);
        println!("    RUST:     {}", sorted_rust[idx]);
    }

    // Last 5 differences (if different from first 5)
    if total_diffs > 10 {
        let last_start = total_diffs.saturating_sub(last_n);
        println!("\n--- LAST {} DIFFERENCES ---", last_n);
        for &idx in diff_indices.iter().skip(last_start) {
            println!("  Line {}:", idx + 1);
            println!("    BASELINE: {}", sorted_baseline[idx]);
            println!("    RUST:     {}", sorted_rust[idx]);
        }
    }

    // 10 random from middle differences
    if total_diffs > 10 {
        let middle_start = first_n;
        let middle_end = total_diffs.saturating_sub(last_n);
        if middle_end > middle_start {
            let middle_diff_indices: Vec<usize> = diff_indices[middle_start..middle_end].to_vec();
            let sample_count = 10.min(middle_diff_indices.len());

            println!(
                "\n--- {} RANDOM DIFFERENCES FROM MIDDLE ({} middle diffs) ---",
                sample_count,
                middle_diff_indices.len()
            );

            // Deterministic shuffle using LCG
            let mut rng_seed = total_diffs as u64;
            let mut shuffled: Vec<usize> = middle_diff_indices;
            for i in (1..shuffled.len()).rev() {
                rng_seed = rng_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let j = (rng_seed as usize) % (i + 1);
                shuffled.swap(i, j);
            }

            for &idx in shuffled.iter().take(sample_count) {
                println!("  Line {}:", idx + 1);
                println!("    BASELINE: {}", sorted_baseline[idx]);
                println!("    RUST:     {}", sorted_rust[idx]);
            }
        }
    }
}

fn read_sorted_lines(path: &Path) -> Vec<String> {
    let mut all_lines = read_lines(path);
    all_lines.sort_unstable();
    all_lines
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        extract_json_string_field, ordered_sha256, path_has_literal_tilde_component, sorted_sha256,
        uffs_binary_name,
    };

    fn lines(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn sorted_full_file_hash_still_detects_missing_footer_lines() {
        let baseline = lines(&["header", "", "row-a", "row-b", "", "Drives?\t1\tD:", ""]);
        let rust = lines(&["header", "", "row-b", "row-a"]);

        assert_ne!(ordered_sha256(&baseline), ordered_sha256(&rust));
        assert_ne!(sorted_sha256(&baseline), sorted_sha256(&rust));
    }

    #[test]
    fn sorted_full_file_hash_allows_row_reordering_when_full_line_set_matches() {
        let baseline = lines(&["header", "", "row-a", "row-b", "", "Drives?\t1\tD:", ""]);
        let rust = lines(&["header", "", "row-b", "row-a", "", "Drives?\t1\tD:", ""]);

        assert_ne!(ordered_sha256(&baseline), ordered_sha256(&rust));
        assert_eq!(sorted_sha256(&baseline), sorted_sha256(&rust));
    }

    #[test]
    fn extracts_target_directory_from_cargo_metadata_json() {
        let metadata = r#"{"target_directory":"/tmp/workspace/~/Library/Caches/uffs/target"}"#;

        assert_eq!(
            extract_json_string_field(metadata, "target_directory"),
            Some("/tmp/workspace/~/Library/Caches/uffs/target".to_string())
        );
    }

    #[test]
    fn extracts_escaped_windows_target_directory_from_cargo_metadata_json() {
        let metadata = r#"{"target_directory":"C:\\rust-target\\uffs"}"#;

        assert_eq!(
            extract_json_string_field(metadata, "target_directory"),
            Some(r"C:\rust-target\uffs".to_string())
        );
    }

    #[test]
    fn detects_literal_tilde_path_component() {
        assert!(path_has_literal_tilde_component(Path::new(
            "/workspace/~/Library/Caches/uffs/target"
        )));
        assert!(!path_has_literal_tilde_component(Path::new(
            "/Users/test/Library/Caches/uffs/target"
        )));
    }

    #[test]
    fn uffs_binary_name_matches_platform_expectation() {
        let expected_suffix = if cfg!(windows) { "uffs.exe" } else { "uffs" };
        assert_eq!(uffs_binary_name(), expected_suffix);
        assert_eq!(
            Path::new("release")
                .join(uffs_binary_name())
                .file_name()
                .and_then(|name| name.to_str()),
            Some(expected_suffix)
        );
    }
}
