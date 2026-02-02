#!/usr/bin/env rust-script
//! Comprehensive parity analysis for UFFS trial run data.
//!
//! This script analyzes C++ vs Rust scan outputs from trial_run.ps1 and generates
//! a detailed parity report including:
//! - Path matching analysis (Live scan)
//! - Path matching analysis (Offline scan - auto-generated if needed)
//! - ADS (Alternate Data Streams) comparison
//! - Tree metrics verification (size, descendants)
//! - Timestamp validation
//! - Boolean flag comparison
//! - Live vs Offline comparison
//!
//! # Usage
//!
//! ```bash
//! # Analyze a trial run directory (auto-detects files, generates offline if needed)
//! rust-script scripts/analyze_trial_parity.rs docs/trial_runs/g_drive
//!
//! # Analyze with custom output report name
//! rust-script scripts/analyze_trial_parity.rs docs/trial_runs/g_drive --report my_report.md
//!
//! # Analyze specific files (skips offline analysis)
//! rust-script scripts/analyze_trial_parity.rs --cpp cpp_g.txt --rust rust_live_g.txt
//! ```
//!
//! # Output
//!
//! Generates a markdown report with:
//! - Executive summary with pass/fail status for both Live and Offline
//! - Detailed field-by-field comparison
//! - Issue identification (tree metrics, timestamps)
//! - ADS entry listing
//! - Live vs Offline comparison table
//!
//! ```cargo
//! [dependencies]
//! chrono = "0.4"
//! ```

use chrono::Local;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage(&args[0]);
        std::process::exit(1);
    }

    let config = parse_args(&args);

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           UFFS TRIAL RUN PARITY ANALYSIS                             ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("📅 Date: {}", Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!();

    // Find input files for LIVE analysis
    let (cpp_file, rust_live_file) = find_input_files(&config);

    println!("═══════════════════════════════════════════════════════════════════════");
    println!("                         LIVE SCAN ANALYSIS");
    println!("═══════════════════════════════════════════════════════════════════════");
    println!();
    println!("📁 Input Files:");
    println!("   C++:  {}", cpp_file.display());
    println!("   Rust: {}", rust_live_file.display());
    println!();

    // Load and analyze LIVE
    let cpp_data = load_csv(&cpp_file, "C++");
    let rust_live_data = load_csv(&rust_live_file, "Rust Live");

    println!("📊 Row Counts:");
    println!("   C++:  {} rows", cpp_data.rows.len());
    println!("   Rust: {} rows", rust_live_data.rows.len());
    println!();

    // Perform LIVE analysis
    let live_results = analyze_parity(&cpp_data, &rust_live_data);

    // Print LIVE results
    print_results(&live_results, "LIVE");

    // OFFLINE analysis (only if we have a trial directory)
    let offline_results = if let Some(ref trial_dir) = config.trial_dir {
        run_offline_analysis(trial_dir, &cpp_data)
    } else {
        println!();
        println!("ℹ️  Skipping offline analysis (no trial directory specified)");
        None
    };

    // Generate comprehensive report
    let report_path = config.report_path.unwrap_or_else(|| {
        let dir = config.trial_dir.as_ref().cloned().unwrap_or_else(|| PathBuf::from("."));
        dir.join(format!("PARITY_ANALYSIS_{}.md", Local::now().format("%Y_%m_%d")))
    });

    write_comprehensive_report(
        &live_results,
        offline_results.as_ref(),
        &report_path,
        &cpp_file,
        &rust_live_file,
        config.trial_dir.as_ref(),
    );
    println!();
    println!("📝 Report written to: {}", report_path.display());
}

/// Run offline analysis: find/generate offline scan, then analyze
fn run_offline_analysis(trial_dir: &Path, cpp_data: &CsvData) -> Option<(AnalysisResults, PathBuf)> {
    println!();
    println!("═══════════════════════════════════════════════════════════════════════");
    println!("                        OFFLINE SCAN ANALYSIS");
    println!("═══════════════════════════════════════════════════════════════════════");
    println!();

    // Look for existing offline scan
    let offline_file = find_file_matching(trial_dir, "rust_offline_");

    let offline_file = if let Some(f) = offline_file {
        println!("📂 Found existing offline scan: {}", f.display());
        f
    } else {
        // Need to generate offline scan
        println!("📂 No offline scan found, generating...");

        // Find MFT file
        let mft_file = find_mft_file(trial_dir);
        if mft_file.is_none() {
            println!("   ⚠️  No MFT file found (looking for *_mft.bin or *.bin)");
            println!("   ⚠️  Skipping offline analysis");
            return None;
        }
        let mft_file = mft_file.unwrap();
        println!("   Found MFT file: {}", mft_file.display());

        // Determine drive letter from directory name or MFT filename
        let drive_letter = detect_drive_letter(trial_dir, &mft_file);
        if drive_letter.is_none() {
            println!("   ⚠️  Could not determine drive letter");
            println!("   ⚠️  Skipping offline analysis");
            return None;
        }
        let drive_letter = drive_letter.unwrap();
        println!("   Drive letter: {}", drive_letter);

        // Generate output path
        let output_file = trial_dir.join(format!("rust_offline_{}.txt", drive_letter.to_lowercase()));
        println!("   Output file: {}", output_file.display());

        // Run uffs-cli to generate offline scan
        if !generate_offline_scan(&mft_file, &drive_letter, &output_file) {
            println!("   ⚠️  Failed to generate offline scan");
            return None;
        }

        output_file
    };

    println!();

    // Load and analyze offline data
    let rust_offline_data = load_csv(&offline_file, "Rust Offline");

    println!("📊 Row Counts:");
    println!("   C++:  {} rows", cpp_data.rows.len());
    println!("   Rust: {} rows", rust_offline_data.rows.len());
    println!();

    // Perform OFFLINE analysis
    let offline_results = analyze_parity(cpp_data, &rust_offline_data);

    // Print OFFLINE results
    print_results(&offline_results, "OFFLINE");

    Some((offline_results, offline_file))
}

/// Find MFT file in trial directory
fn find_mft_file(dir: &Path) -> Option<PathBuf> {
    // First try *_mft.bin pattern
    if let Some(f) = find_file_with_suffix(dir, "_mft.bin") {
        return Some(f);
    }
    // Then try any .bin file
    find_file_with_suffix(dir, ".bin")
}

fn find_file_with_suffix(dir: &Path, suffix: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).find_map(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.to_lowercase().ends_with(suffix) {
            Some(entry.path())
        } else {
            None
        }
    })
}

/// Detect drive letter from directory name or MFT filename
fn detect_drive_letter(trial_dir: &Path, mft_file: &Path) -> Option<String> {
    // Try from MFT filename first (e.g., "G_mft.bin" -> "G")
    if let Some(name) = mft_file.file_name() {
        let name = name.to_string_lossy();
        if name.len() >= 1 {
            let first_char = name.chars().next().unwrap();
            if first_char.is_ascii_alphabetic() {
                return Some(first_char.to_uppercase().to_string());
            }
        }
    }

    // Try from directory name (e.g., "g_drive" -> "G")
    if let Some(dir_name) = trial_dir.file_name() {
        let dir_name = dir_name.to_string_lossy().to_lowercase();
        if dir_name.contains("_drive") || dir_name.contains("_disk") {
            let first_char = dir_name.chars().next().unwrap();
            if first_char.is_ascii_alphabetic() {
                return Some(first_char.to_uppercase().to_string());
            }
        }
    }

    None
}

/// Generate offline scan using uffs-cli
fn generate_offline_scan(mft_file: &Path, drive_letter: &str, output_file: &Path) -> bool {
    println!("   🔄 Running uffs-cli to generate offline scan...");

    let result = Command::new("cargo")
        .args([
            "run", "--release", "-p", "uffs-cli", "--",
            "*",
            "--mft-file", &mft_file.to_string_lossy(),
            "--drive", drive_letter,
            "--parse-algo", "cpp_port",
            "--tree-algo", "cpp",
            "--io-algo", "cpp",
            "--chunk-algo", "cpp",
            "--out", &output_file.to_string_lossy(),
        ])
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                println!("   ✅ Offline scan generated successfully");
                true
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("   ❌ uffs-cli failed: {}", stderr.lines().take(5).collect::<Vec<_>>().join("\n"));
                false
            }
        }
        Err(e) => {
            println!("   ❌ Failed to run cargo: {}", e);
            false
        }
    }
}

fn print_usage(prog: &str) {
    eprintln!("Usage: {} <trial_dir> [options]", prog);
    eprintln!();
    eprintln!("Analyze C++ vs Rust parity from trial_run.ps1 output.");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  <trial_dir>           Directory containing trial run data");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --cpp <file>          C++ output file (default: auto-detect cpp_*.txt)");
    eprintln!("  --rust <file>         Rust output file (default: auto-detect rust_live_*.txt)");
    eprintln!("  --report <file>       Output report path (default: PARITY_ANALYSIS_<date>.md)");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  {} docs/trial_runs/g_drive", prog);
    eprintln!("  {} docs/trial_runs/g_drive --report my_report.md", prog);
    eprintln!("  {} --cpp cpp_g.txt --rust rust_live_g.txt", prog);
}

#[derive(Default)]
struct Config {
    trial_dir: Option<PathBuf>,
    cpp_file: Option<PathBuf>,
    rust_file: Option<PathBuf>,
    report_path: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Config {
    let mut config = Config::default();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "--cpp" if i + 1 < args.len() => {
                config.cpp_file = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--rust" if i + 1 < args.len() => {
                config.rust_file = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--report" if i + 1 < args.len() => {
                config.report_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            arg if !arg.starts_with('-') && config.trial_dir.is_none() => {
                config.trial_dir = Some(PathBuf::from(arg));
                i += 1;
            }
            _ => i += 1,
        }
    }
    config
}

fn find_input_files(config: &Config) -> (PathBuf, PathBuf) {
    if let (Some(cpp), Some(rust)) = (&config.cpp_file, &config.rust_file) {
        return (cpp.clone(), rust.clone());
    }

    let dir = config.trial_dir.as_ref().expect("Trial directory required");

    // Auto-detect files
    let cpp_file = find_file_matching(dir, "cpp_").expect("No cpp_*.txt file found");
    let rust_file = find_file_matching(dir, "rust_live_")
        .or_else(|| find_file_matching(dir, "rust_"))
        .expect("No rust_*.txt file found");

    (cpp_file, rust_file)
}

fn find_file_matching(dir: &Path, prefix: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).find_map(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(prefix) && name.ends_with(".txt") && !name.contains("trace") {
            Some(entry.path())
        } else {
            None
        }
    })
}

// ============================================================================
// CSV Loading
// ============================================================================

#[derive(Default)]
struct CsvData {
    headers: Vec<String>,
    rows: Vec<CsvRow>,
    path_index: HashMap<String, usize>,
}

#[derive(Default, Clone)]
#[allow(dead_code)] // Fields parsed for potential future analysis
struct CsvRow {
    path: String,
    name: String,
    size: u64,
    allocated_size: u64,
    descendants: u64,
    created: String,
    modified: String,
    accessed: String,
    is_directory: bool,
    is_archive: bool,
    is_system: bool,
    is_hidden: bool,
    is_reparse: bool,
    attributes: u32,
}

fn load_csv(path: &Path, label: &str) -> CsvData {
    println!("📂 Loading {}: {}", label, path.display());

    let file = File::open(path).expect(&format!("Failed to open {}", path.display()));
    let reader = BufReader::new(file);
    let mut data = CsvData::default();

    let mut lines = reader.lines();

    // Parse header
    if let Some(Ok(header_line)) = lines.next() {
        data.headers = parse_csv_line(&header_line);
    }

    // Build column index
    let col_idx: HashMap<String, usize> = data.headers.iter()
        .enumerate()
        .map(|(i, h)| (h.to_lowercase(), i))
        .collect();

    // Parse rows
    for line in lines {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        if line.is_empty() {
            continue;
        }

        let fields = parse_csv_line(&line);
        if fields.len() < 5 {
            continue;
        }

        let row = CsvRow {
            path: get_field(&fields, &col_idx, "path"),
            name: get_field(&fields, &col_idx, "name"),
            size: get_field_u64(&fields, &col_idx, "size"),
            allocated_size: get_field_u64(&fields, &col_idx, "size on disk"),
            descendants: get_field_u64(&fields, &col_idx, "descendants"),
            created: get_field(&fields, &col_idx, "created"),
            modified: get_field(&fields, &col_idx, "last written"),
            accessed: get_field(&fields, &col_idx, "last accessed"),
            is_directory: get_field_bool(&fields, &col_idx, "directory flag"),
            is_archive: get_field_bool(&fields, &col_idx, "archive"),
            is_system: get_field_bool(&fields, &col_idx, "system"),
            is_hidden: get_field_bool(&fields, &col_idx, "hidden"),
            is_reparse: get_field_bool(&fields, &col_idx, "reparse"),
            attributes: get_field_u64(&fields, &col_idx, "attributes") as u32,
        };

        // Skip debug output lines
        if row.path.to_lowercase().contains("searchstring")
            || row.path.to_lowercase().contains("drives?")
            || row.path.to_lowercase().contains("search path") {
            continue;
        }

        let normalized = normalize_path(&row.path);
        let idx = data.rows.len();
        data.path_index.insert(normalized, idx);
        data.rows.push(row);
    }

    println!("   Loaded {} rows", data.rows.len());
    data
}

fn get_field(fields: &[String], col_idx: &HashMap<String, usize>, name: &str) -> String {
    col_idx.get(name).and_then(|&i| fields.get(i)).cloned().unwrap_or_default()
}

fn get_field_u64(fields: &[String], col_idx: &HashMap<String, usize>, name: &str) -> u64 {
    get_field(fields, col_idx, name).parse().unwrap_or(0)
}

fn get_field_bool(fields: &[String], col_idx: &HashMap<String, usize>, name: &str) -> bool {
    let val = get_field(fields, col_idx, name);
    val == "1" || val.to_lowercase() == "true"
}

fn normalize_path(path: &str) -> String {
    path.to_lowercase().replace('\\', "/").trim_end_matches('/').to_string()
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes {
                    if chars.peek() == Some(&'"') {
                        current.push('"');
                        chars.next();
                    } else {
                        in_quotes = false;
                    }
                } else {
                    in_quotes = true;
                }
            }
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    fields.push(current);
    fields
}


// ============================================================================
// Parity Analysis
// ============================================================================

#[derive(Default)]
struct AnalysisResults {
    cpp_rows: usize,
    rust_rows: usize,
    common_paths: usize,
    cpp_only_paths: usize,
    rust_only_paths: usize,
    path_match_rate: f64,

    // ADS
    cpp_ads_count: usize,
    rust_ads_count: usize,
    ads_match: bool,
    ads_cpp_only: Vec<String>,  // In C++ but not Rust
    ads_rust_only: Vec<String>, // In Rust but not C++

    // Tree metrics
    tree_metric_issues: Vec<TreeMetricIssue>,
    tree_metrics_ok: bool,

    // Timestamps
    timestamp_issues: Vec<TimestampIssue>,
    timestamps_ok: bool,

    // Boolean flags
    bool_flag_matches: HashMap<String, (usize, usize)>, // (matches, total)

    // Sample differences
    sample_cpp_only: Vec<String>,
    sample_rust_only: Vec<String>,
}

#[derive(Clone)]
struct TreeMetricIssue {
    path: String,
    cpp_size: u64,
    rust_size: u64,
    cpp_descendants: u64,
    rust_descendants: u64,
}

#[derive(Clone)]
struct TimestampIssue {
    path: String,
    cpp_created: String,
    rust_created: String,
    year_diff: i32,
}

fn analyze_parity(cpp: &CsvData, rust: &CsvData) -> AnalysisResults {
    let mut results = AnalysisResults::default();

    results.cpp_rows = cpp.rows.len();
    results.rust_rows = rust.rows.len();

    // Path matching
    let cpp_paths: HashSet<_> = cpp.path_index.keys().collect();
    let rust_paths: HashSet<_> = rust.path_index.keys().collect();

    let common: HashSet<_> = cpp_paths.intersection(&rust_paths).collect();
    let cpp_only: Vec<_> = cpp_paths.difference(&rust_paths).collect();
    let rust_only: Vec<_> = rust_paths.difference(&cpp_paths).collect();

    results.common_paths = common.len();
    results.cpp_only_paths = cpp_only.len();
    results.rust_only_paths = rust_only.len();
    results.path_match_rate = if cpp_paths.is_empty() {
        0.0
    } else {
        100.0 * common.len() as f64 / cpp_paths.len() as f64
    };

    // Sample missing paths
    results.sample_cpp_only = cpp_only.iter().take(10).map(|s| (**s).clone()).collect();
    results.sample_rust_only = rust_only.iter().take(10).map(|s| (**s).clone()).collect();

    // ADS analysis - collect and sort for comparison
    let mut cpp_ads: Vec<String> = cpp.rows.iter()
        .filter(|r| is_ads_path(&r.path))
        .map(|r| normalize_path(&r.path))
        .collect();
    let mut rust_ads: Vec<String> = rust.rows.iter()
        .filter(|r| is_ads_path(&r.path))
        .map(|r| normalize_path(&r.path))
        .collect();
    cpp_ads.sort();
    rust_ads.sort();

    results.cpp_ads_count = cpp_ads.len();
    results.rust_ads_count = rust_ads.len();

    // Find differences
    let cpp_ads_set: HashSet<_> = cpp_ads.iter().collect();
    let rust_ads_set: HashSet<_> = rust_ads.iter().collect();

    results.ads_cpp_only = cpp_ads.iter()
        .filter(|p| !rust_ads_set.contains(p))
        .cloned()
        .collect();
    results.ads_rust_only = rust_ads.iter()
        .filter(|p| !cpp_ads_set.contains(p))
        .cloned()
        .collect();

    results.ads_match = results.ads_cpp_only.is_empty() && results.ads_rust_only.is_empty();

    // Analyze common paths
    let mut bool_matches: HashMap<String, (usize, usize)> = HashMap::new();
    for flag in &["is_directory", "is_archive", "is_system", "is_hidden", "is_reparse"] {
        bool_matches.insert(flag.to_string(), (0, 0));
    }

    for path in &common {
        let cpp_idx = cpp.path_index[**path];
        let rust_idx = rust.path_index[**path];
        let cpp_row = &cpp.rows[cpp_idx];
        let rust_row = &rust.rows[rust_idx];

        // Tree metrics (only for directories)
        if cpp_row.is_directory {
            let size_diff = cpp_row.size != rust_row.size;
            let desc_diff = cpp_row.descendants != rust_row.descendants;

            if (size_diff || desc_diff) && results.tree_metric_issues.len() < 20 {
                results.tree_metric_issues.push(TreeMetricIssue {
                    path: cpp_row.path.clone(),
                    cpp_size: cpp_row.size,
                    rust_size: rust_row.size,
                    cpp_descendants: cpp_row.descendants,
                    rust_descendants: rust_row.descendants,
                });
            }
        }

        // Timestamp check
        if !cpp_row.created.is_empty() && !rust_row.created.is_empty() {
            let cpp_year = extract_year(&cpp_row.created);
            let rust_year = extract_year(&rust_row.created);

            if let (Some(cy), Some(ry)) = (cpp_year, rust_year) {
                let diff = (ry as i32) - (cy as i32);
                if diff.abs() > 100 && results.timestamp_issues.len() < 10 {
                    results.timestamp_issues.push(TimestampIssue {
                        path: cpp_row.path.clone(),
                        cpp_created: cpp_row.created.clone(),
                        rust_created: rust_row.created.clone(),
                        year_diff: diff,
                    });
                }
            }
        }

        // Boolean flags
        check_bool_match(&mut bool_matches, "is_directory", cpp_row.is_directory, rust_row.is_directory);
        check_bool_match(&mut bool_matches, "is_archive", cpp_row.is_archive, rust_row.is_archive);
        check_bool_match(&mut bool_matches, "is_system", cpp_row.is_system, rust_row.is_system);
        check_bool_match(&mut bool_matches, "is_hidden", cpp_row.is_hidden, rust_row.is_hidden);
        check_bool_match(&mut bool_matches, "is_reparse", cpp_row.is_reparse, rust_row.is_reparse);
    }

    results.bool_flag_matches = bool_matches;
    results.tree_metrics_ok = results.tree_metric_issues.is_empty();
    results.timestamps_ok = results.timestamp_issues.is_empty();

    results
}

fn is_ads_path(path: &str) -> bool {
    // ADS paths have : after the filename (not the drive letter)
    let path_lower = path.to_lowercase();
    if let Some(pos) = path_lower.find(':') {
        // Skip drive letter (e.g., "G:")
        if pos == 1 {
            // Check for another : after the drive letter
            return path_lower[2..].contains(':');
        }
        return true;
    }
    false
}

fn extract_year(timestamp: &str) -> Option<u32> {
    // Format: "2026-01-20 16:45:43" or "6220-07-18 23:37:14"
    if timestamp.len() >= 4 {
        timestamp[0..4].parse().ok()
    } else {
        None
    }
}

fn check_bool_match(matches: &mut HashMap<String, (usize, usize)>, field: &str, cpp: bool, rust: bool) {
    if let Some((m, t)) = matches.get_mut(field) {
        *t += 1;
        if cpp == rust {
            *m += 1;
        }
    }
}


// ============================================================================
// Output Functions
// ============================================================================

fn print_results(results: &AnalysisResults, label: &str) {
    // Path matching
    println!("🔗 PATH MATCHING ({})", label);
    println!("   Common paths:     {:>10}", results.common_paths);
    println!("   C++ only:         {:>10}", results.cpp_only_paths);
    println!("   Rust only:        {:>10}", results.rust_only_paths);
    println!("   Match rate:       {:>9.4}%", results.path_match_rate);
    println!();

    // ADS
    println!("📎 ALTERNATE DATA STREAMS (ADS)");
    println!("   C++ ADS entries:  {:>10}", results.cpp_ads_count);
    println!("   Rust ADS entries: {:>10}", results.rust_ads_count);
    if results.ads_match {
        println!("   Status:           ✅ MATCH");
    } else {
        println!("   Status:           ⚠️  MISMATCH");
        if !results.ads_cpp_only.is_empty() {
            println!();
            println!("   In C++ but NOT in Rust ({}):", results.ads_cpp_only.len());
            for (i, ads) in results.ads_cpp_only.iter().take(10).enumerate() {
                println!("     {}. {}", i + 1, ads);
            }
            if results.ads_cpp_only.len() > 10 {
                println!("     ... and {} more", results.ads_cpp_only.len() - 10);
            }
        }
        if !results.ads_rust_only.is_empty() {
            println!();
            println!("   In Rust but NOT in C++ ({}):", results.ads_rust_only.len());
            for (i, ads) in results.ads_rust_only.iter().take(10).enumerate() {
                println!("     {}. {}", i + 1, ads);
            }
            if results.ads_rust_only.len() > 10 {
                println!("     ... and {} more", results.ads_rust_only.len() - 10);
            }
        }
    }
    println!();

    // Boolean flags
    println!("🏷️  BOOLEAN FLAGS");
    let mut flags: Vec<_> = results.bool_flag_matches.keys().collect();
    flags.sort();
    for flag in flags {
        let (matches, total) = results.bool_flag_matches[flag];
        let rate = if total > 0 { 100.0 * matches as f64 / total as f64 } else { 0.0 };
        let status = if rate >= 100.0 { "✅" } else { "⚠️" };
        println!("   {:15} {:>9.4}% {}", flag, rate, status);
    }
    println!();

    // Tree metrics
    println!("🌳 TREE METRICS (size, descendants)");
    if results.tree_metrics_ok {
        println!("   Status:           ✅ ALL MATCH");
    } else {
        println!("   Status:           🔴 ISSUES FOUND ({} directories)", results.tree_metric_issues.len());
        println!();
        println!("   Sample issues:");
        for issue in results.tree_metric_issues.iter().take(5) {
            println!("   {} ", issue.path);
            println!("      Size: C++={} Rust={}", issue.cpp_size, issue.rust_size);
            println!("      Desc: C++={} Rust={}", issue.cpp_descendants, issue.rust_descendants);
        }
    }
    println!();

    // Timestamps
    println!("🕐 TIMESTAMPS");
    if results.timestamps_ok {
        println!("   Status:           ✅ ALL VALID");
    } else {
        println!("   Status:           🔴 ISSUES FOUND ({} files)", results.timestamp_issues.len());
        println!();
        println!("   Sample issues:");
        for issue in results.timestamp_issues.iter().take(3) {
            println!("   {} ", issue.path);
            println!("      C++:  {}", issue.cpp_created);
            println!("      Rust: {} (year diff: {})", issue.rust_created, issue.year_diff);
        }
    }
    println!();

    // Overall summary for this analysis
    println!("───────────────────────────────────────────────────────────────────────");
    println!("                         {} SUMMARY", label);
    println!("───────────────────────────────────────────────────────────────────────");

    let all_ok = results.path_match_rate >= 99.9
        && results.tree_metrics_ok
        && results.timestamps_ok
        && results.ads_match;

    if all_ok {
        println!();
        println!("   ✅ PERFECT PARITY - All checks passed!");
    } else {
        println!();
        if results.path_match_rate < 99.9 {
            println!("   ⚠️  Path match rate below 99.9%");
        }
        if !results.tree_metrics_ok {
            println!("   🔴 Tree metrics issues detected");
        }
        if !results.timestamps_ok {
            println!("   🔴 Timestamp conversion issues detected");
        }
        if !results.ads_match {
            println!("   ⚠️  ADS mismatch ({} in C++ only, {} in Rust only)",
                results.ads_cpp_only.len(), results.ads_rust_only.len());
        }
    }
    println!();
}

fn write_comprehensive_report(
    live_results: &AnalysisResults,
    offline_results: Option<&(AnalysisResults, PathBuf)>,
    path: &Path,
    cpp_file: &Path,
    rust_live_file: &Path,
    trial_dir: Option<&PathBuf>,
) {
    let mut f = File::create(path).expect("Failed to create report file");

    writeln!(f, "# UFFS Comprehensive Parity Analysis Report").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "> Generated by: `scripts/analyze_trial_parity.rs`").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "**Date:** {}", Local::now().format("%Y-%m-%d %H:%M:%S")).unwrap();
    if let Some(dir) = trial_dir {
        writeln!(f, "**Trial Directory:** `{}`", dir.display()).unwrap();
    }
    writeln!(f, "**C++ Reference:** `{}`", cpp_file.display()).unwrap();
    writeln!(f, "**Rust Live Scan:** `{}`", rust_live_file.display()).unwrap();
    if let Some((_, offline_file)) = offline_results {
        writeln!(f, "**Rust Offline Scan:** `{}`", offline_file.display()).unwrap();
    }
    writeln!(f).unwrap();

    // Quick Comparison Table (if we have both)
    if let Some((offline, _)) = offline_results {
        writeln!(f, "## Quick Comparison: Live vs Offline").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "| Metric | Live Scan | Offline Scan |").unwrap();
        writeln!(f, "|--------|-----------|--------------|").unwrap();
        writeln!(f, "| **Row Count** | {} {} | {} {} |",
            live_results.rust_rows,
            if live_results.rust_rows == live_results.cpp_rows { "✅" } else { "⚠️" },
            offline.rust_rows,
            if offline.rust_rows == offline.cpp_rows { "✅" } else { "⚠️" }
        ).unwrap();
        writeln!(f, "| **Path Match** | {:.2}% {} | {:.2}% {} |",
            live_results.path_match_rate,
            if live_results.path_match_rate >= 99.9 { "✅" } else { "⚠️" },
            offline.path_match_rate,
            if offline.path_match_rate >= 99.9 { "✅" } else { "⚠️" }
        ).unwrap();
        writeln!(f, "| **ADS** | {} {} | {} {} |",
            live_results.rust_ads_count,
            if live_results.ads_match { "✅" } else { "⚠️" },
            offline.rust_ads_count,
            if offline.ads_match { "✅" } else { "⚠️" }
        ).unwrap();
        writeln!(f, "| **Tree Metrics** | {} | {} |",
            if live_results.tree_metrics_ok { "✅ OK".to_string() } else { format!("🔴 {} issues", live_results.tree_metric_issues.len()) },
            if offline.tree_metrics_ok { "✅ OK".to_string() } else { format!("🔴 {} issues", offline.tree_metric_issues.len()) }
        ).unwrap();
        writeln!(f, "| **Timestamps** | {} | {} |",
            if live_results.timestamps_ok { "✅ OK".to_string() } else { format!("🔴 {} issues", live_results.timestamp_issues.len()) },
            if offline.timestamps_ok { "✅ OK".to_string() } else { format!("🔴 {} issues", offline.timestamp_issues.len()) }
        ).unwrap();
        writeln!(f).unwrap();
    }

    // LIVE SCAN SECTION
    writeln!(f, "---").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "# Part 1: Live Scan Analysis").unwrap();
    writeln!(f).unwrap();
    write_analysis_section(&mut f, live_results, "Live");

    // OFFLINE SCAN SECTION
    if let Some((offline, _)) = offline_results {
        writeln!(f, "---").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "# Part 2: Offline Scan Analysis").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "> Offline scan uses a saved MFT file instead of reading live from disk.").unwrap();
        writeln!(f, "> This isolates MFT parsing from live I/O issues.").unwrap();
        writeln!(f).unwrap();
        write_analysis_section(&mut f, offline, "Offline");
    }

    // CONCLUSIONS
    writeln!(f, "---").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "# Conclusions").unwrap();
    writeln!(f).unwrap();

    if let Some((offline, _)) = offline_results {
        // Compare live vs offline - more nuanced analysis
        let live_tree_ok = live_results.tree_metrics_ok;
        let live_ts_ok = live_results.timestamps_ok;
        let offline_tree_ok = offline.tree_metrics_ok;
        let offline_ts_ok = offline.timestamps_ok;

        let live_ok = live_tree_ok && live_ts_ok;
        let offline_ok = offline_tree_ok && offline_ts_ok;

        // Check if offline is significantly better than live
        let offline_better = (!live_ts_ok && offline_ts_ok)
            || (live_results.tree_metric_issues.len() > offline.tree_metric_issues.len() * 2);

        if live_ok && offline_ok {
            writeln!(f, "✅ **Both Live and Offline scans show perfect parity with C++ reference.**").unwrap();
        } else if !live_ok && offline_ok {
            writeln!(f, "⚠️ **Live scan has issues, but Offline scan is correct.**").unwrap();
            writeln!(f).unwrap();
            writeln!(f, "This suggests the issues are related to **live I/O** rather than MFT parsing:").unwrap();
            if !live_ts_ok && offline_ts_ok {
                writeln!(f, "- **Timestamps:** Live scan has year offset issues, offline is correct").unwrap();
            }
            if !live_tree_ok && offline_tree_ok {
                writeln!(f, "- **Tree Metrics:** Live scan shows zeros, offline computes correctly").unwrap();
            }
        } else if live_ok && !offline_ok {
            writeln!(f, "⚠️ **Offline scan has issues, but Live scan is correct.**").unwrap();
            writeln!(f).unwrap();
            writeln!(f, "This is unusual - investigate the MFT file or offline parsing.").unwrap();
        } else if offline_better {
            // Both have issues but offline is significantly better
            writeln!(f, "⚠️ **Both scans have issues, but Offline is significantly better.**").unwrap();
            writeln!(f).unwrap();
            writeln!(f, "| Issue | Live | Offline |").unwrap();
            writeln!(f, "|-------|------|---------|").unwrap();
            writeln!(f, "| Tree Metrics | {} issues | {} issues |",
                live_results.tree_metric_issues.len(),
                offline.tree_metric_issues.len()
            ).unwrap();
            writeln!(f, "| Timestamps | {} issues | {} issues |",
                live_results.timestamp_issues.len(),
                offline.timestamp_issues.len()
            ).unwrap();
            writeln!(f).unwrap();
            writeln!(f, "**Analysis:**").unwrap();
            if !live_ts_ok && offline_ts_ok {
                writeln!(f, "- **Timestamps:** The 4194-year offset in live scan is a **live I/O issue** (offline is correct)").unwrap();
            }
            if live_results.tree_metric_issues.len() > offline.tree_metric_issues.len() {
                let live_zeros = live_results.tree_metric_issues.iter()
                    .filter(|i| i.rust_size == 0 && i.rust_descendants == 0)
                    .count();
                if live_zeros > 0 {
                    writeln!(f, "- **Tree Metrics:** Live scan has {} directories with Size=0/Desc=0 (not computed)", live_zeros).unwrap();
                    writeln!(f, "- **Tree Metrics:** Offline scan computes values, just {} small differences remain", offline.tree_metric_issues.len()).unwrap();
                }
            }
        } else {
            writeln!(f, "🔴 **Both Live and Offline scans have issues.**").unwrap();
            writeln!(f).unwrap();
            writeln!(f, "Issues found in both modes suggest core parsing problems.").unwrap();
        }
    } else {
        // Only live results
        let live_ok = live_results.path_match_rate >= 99.9
            && live_results.tree_metrics_ok
            && live_results.timestamps_ok
            && live_results.ads_match;

        if live_ok {
            writeln!(f, "✅ **Live scan shows perfect parity with C++ reference.**").unwrap();
        } else {
            writeln!(f, "⚠️ **Live scan has issues.** Consider running with offline MFT for comparison.").unwrap();
        }
    }
    writeln!(f).unwrap();
}

/// Write a single analysis section (used for both Live and Offline)
fn write_analysis_section(f: &mut File, results: &AnalysisResults, label: &str) {
    // Executive Summary
    writeln!(f, "## {} Scan Summary", label).unwrap();
    writeln!(f).unwrap();
    writeln!(f, "| Metric | C++ | Rust | Status |").unwrap();
    writeln!(f, "|--------|-----|------|--------|").unwrap();
    writeln!(f, "| **Total Rows** | {} | {} | {} |",
        results.cpp_rows, results.rust_rows,
        if (results.cpp_rows as i64 - results.rust_rows as i64).abs() <= 5 { "✅" } else { "⚠️" }
    ).unwrap();
    writeln!(f, "| **Path Match Rate** | - | - | {:.4}% {} |",
        results.path_match_rate,
        if results.path_match_rate >= 99.9 { "✅" } else { "⚠️" }
    ).unwrap();
    writeln!(f, "| **ADS Entries** | {} | {} | {} |",
        results.cpp_ads_count, results.rust_ads_count,
        if results.ads_match { "✅" } else { "⚠️" }
    ).unwrap();
    writeln!(f, "| **Tree Metrics** | ✅ | {} | {} |",
        if results.tree_metrics_ok { "✅" } else { "❌" },
        if results.tree_metrics_ok { "✅" } else { "🔴 ISSUE" }
    ).unwrap();
    writeln!(f, "| **Timestamps** | ✅ | {} | {} |",
        if results.timestamps_ok { "✅" } else { "❌" },
        if results.timestamps_ok { "✅" } else { "🔴 ISSUE" }
    ).unwrap();
    writeln!(f).unwrap();

    // Boolean Flags
    writeln!(f, "### Boolean Flags").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "| Flag | Match Rate | Status |").unwrap();
    writeln!(f, "|------|------------|--------|").unwrap();
    let mut flags: Vec<_> = results.bool_flag_matches.keys().collect();
    flags.sort();
    for flag in flags {
        let (matches, total) = results.bool_flag_matches[flag];
        let rate = if total > 0 { 100.0 * matches as f64 / total as f64 } else { 0.0 };
        writeln!(f, "| {} | {:.4}% | {} |", flag, rate, if rate >= 100.0 { "✅" } else { "⚠️" }).unwrap();
    }
    writeln!(f).unwrap();

    // ADS Entries
    writeln!(f, "### Alternate Data Streams (ADS)").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "| Metric | Count |").unwrap();
    writeln!(f, "|--------|-------|").unwrap();
    writeln!(f, "| C++ ADS entries | {} |", results.cpp_ads_count).unwrap();
    writeln!(f, "| Rust ADS entries | {} |", results.rust_ads_count).unwrap();
    writeln!(f, "| Status | {} |", if results.ads_match { "✅ MATCH" } else { "⚠️ MISMATCH" }).unwrap();
    writeln!(f).unwrap();

    if !results.ads_match {
        if !results.ads_cpp_only.is_empty() {
            writeln!(f, "#### In C++ but NOT in Rust ({}):", results.ads_cpp_only.len()).unwrap();
            for ads in &results.ads_cpp_only {
                writeln!(f, "- `{}`", ads).unwrap();
            }
            writeln!(f).unwrap();
        }

        if !results.ads_rust_only.is_empty() {
            writeln!(f, "#### In Rust but NOT in C++ ({}):", results.ads_rust_only.len()).unwrap();
            for ads in &results.ads_rust_only {
                writeln!(f, "- `{}`", ads).unwrap();
            }
            writeln!(f).unwrap();
        }
    }

    // Issues
    if !results.tree_metric_issues.is_empty() {
        writeln!(f, "### 🔴 Tree Metrics Issues").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "| Path | C++ Size | Rust Size | C++ Desc | Rust Desc |").unwrap();
        writeln!(f, "|------|----------|-----------|----------|-----------|").unwrap();
        for issue in &results.tree_metric_issues {
            writeln!(f, "| `{}` | {} | {} | {} | {} |",
                issue.path, issue.cpp_size, issue.rust_size,
                issue.cpp_descendants, issue.rust_descendants
            ).unwrap();
        }
        writeln!(f).unwrap();
    }

    if !results.timestamp_issues.is_empty() {
        writeln!(f, "### 🔴 Timestamp Issues").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "| Path | C++ Created | Rust Created | Year Diff |").unwrap();
        writeln!(f, "|------|-------------|--------------|-----------|").unwrap();
        for issue in &results.timestamp_issues {
            writeln!(f, "| `{}` | {} | {} | {} |",
                issue.path, issue.cpp_created, issue.rust_created, issue.year_diff
            ).unwrap();
        }
        writeln!(f).unwrap();
    }
}
