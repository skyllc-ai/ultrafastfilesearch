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
//! - Diagnostic log analysis (tripwire verification, post-tree diagnostics)
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

    // Diagnostic log analysis (if trial directory specified)
    let diagnostic_analysis = if let Some(ref trial_dir) = config.trial_dir {
        println!();
        println!("═══════════════════════════════════════════════════════════════════════");
        println!("                      DIAGNOSTIC LOG ANALYSIS");
        println!("═══════════════════════════════════════════════════════════════════════");
        println!();

        let analysis = parse_diagnostic_log(trial_dir);
        print_diagnostic_analysis(&analysis);
        Some(analysis)
    } else {
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
        diagnostic_analysis.as_ref(),
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

    // Parse header - skip comment lines (e.g., "# TRIPWIRE: ...")
    let mut header_line = String::new();
    for line in lines.by_ref() {
        if let Ok(l) = line {
            if l.starts_with('#') {
                // Skip comment lines
                continue;
            }
            header_line = l;
            break;
        }
    }
    if !header_line.is_empty() {
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

    // Composition counts (Fix #3: surface "what's missing" instantly)
    cpp_dir_count: usize,
    cpp_file_count: usize,
    rust_dir_count: usize,
    rust_file_count: usize,

    // ADS
    cpp_ads_count: usize,
    rust_ads_count: usize,
    ads_match: bool,
    ads_cpp_only: Vec<String>,  // In C++ but not Rust
    ads_rust_only: Vec<String>, // In Rust but not C++

    // Tree metrics
    tree_metric_issues: Vec<TreeMetricIssue>,
    tree_metric_total_count: usize,  // Total count of ALL misaligned entries
    tree_metrics_ok: bool,

    // Timestamps
    timestamp_issues: Vec<TimestampIssue>,
    timestamps_ok: bool,

    // Boolean flags
    bool_flag_matches: HashMap<String, (usize, usize)>, // (matches, total)

    // Sample differences
    sample_cpp_only: Vec<String>,
    sample_rust_only: Vec<String>,

    // Missing path prefixes histogram (Fix #3)
    missing_path_prefixes: Vec<(String, usize)>,
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

/// Diagnostic log analysis results
#[derive(Default)]
struct DiagnosticLogAnalysis {
    /// Whether any tripwire marker was found (confirms fixed code is running)
    /// Fix #4: Accept ANY [TRIP] marker, not just a specific string
    tripwire_found: bool,
    /// Sample of tripwire markers found (for diagnostics)
    tripwire_samples: Vec<String>,
    /// Whether the post-tree diagnostic was found
    post_tree_diagnostic_found: bool,
    /// Number of directories with descendants==0 after all passes
    final_bad_dir_count: usize,
    /// Details of bad directories from the log
    bad_dir_details: Vec<BadDirDetail>,
    /// Self-heal triggered?
    self_heal_triggered: bool,
    /// Orphan sweep count
    orphan_sweep_count: usize,
    /// Log file path
    log_file: Option<PathBuf>,
}

#[derive(Clone)]
struct BadDirDetail {
    idx: usize,
    frs: u64,
    first_child: u32,
    name_count: u32,
    stream_count: u32,
    is_reparse: bool,
}

/// Parse diagnostic log file for tripwire and post-tree diagnostic messages
fn parse_diagnostic_log(trial_dir: &Path) -> DiagnosticLogAnalysis {
    let mut analysis = DiagnosticLogAnalysis::default();

    // Find log file (rust_live_*.log or uffs_*.log)
    let log_file = find_log_file(trial_dir);
    if log_file.is_none() {
        return analysis;
    }
    let log_file = log_file.unwrap();
    analysis.log_file = Some(log_file.clone());

    let file = match File::open(&log_file) {
        Ok(f) => f,
        Err(_) => return analysis,
    };
    let reader = BufReader::new(file);

    for line in reader.lines().filter_map(|l| l.ok()) {
        // Check for tripwire - accept [TRIPWIRE] or TRIPWIRE_ markers (Fix #4)
        // The log format is: [TRIPWIRE] TRIPWIRE_UFFS_CPP_TREE_FIX_v0.2.xxx
        if line.contains("[TRIPWIRE]") || line.contains("TRIPWIRE_") {
            analysis.tripwire_found = true;
            // Collect up to 5 samples for diagnostics
            if analysis.tripwire_samples.len() < 5 {
                // Extract the tripwire line for display
                let sample = if line.len() > 80 {
                    format!("{}...", &line[..80])
                } else {
                    line.clone()
                };
                analysis.tripwire_samples.push(sample);
            }
        }

        // Check for post-tree diagnostic
        if line.contains("[tree] FINAL: directories with descendants==0") {
            analysis.post_tree_diagnostic_found = true;
            // Parse bad_dir_count from the log line
            if let Some(count_str) = extract_field(&line, "bad_dir_count=") {
                analysis.final_bad_dir_count = count_str.parse().unwrap_or(0);
            }
        }

        // Check for bad directory details
        if line.contains("[tree] FINAL: bad directory details") {
            if let Some(detail) = parse_bad_dir_detail(&line) {
                analysis.bad_dir_details.push(detail);
            }
        }

        // Check for self-heal
        if line.contains("self-heal triggered") || line.contains("rebuild_children_from_names") {
            analysis.self_heal_triggered = true;
        }

        // Check for orphan sweep
        if line.contains("orphan sweep") || line.contains("orphan_count=") {
            if let Some(count_str) = extract_field(&line, "orphan_count=") {
                analysis.orphan_sweep_count = count_str.parse().unwrap_or(0);
            }
        }
    }

    analysis
}

fn find_log_file(dir: &Path) -> Option<PathBuf> {
    // Priority order for log files:
    // 1. rust_live_*.log (contains tripwire markers from LIVE scan)
    // 2. Any other .log file
    if let Ok(entries) = fs::read_dir(dir) {
        let mut logs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with(".log")
            })
            .map(|e| e.path())
            .collect();

        // Sort to prioritize rust_live_*.log files
        logs.sort_by(|a, b| {
            let a_name = a.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            let b_name = b.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            let a_is_rust_live = a_name.starts_with("rust_live");
            let b_is_rust_live = b_name.starts_with("rust_live");
            // rust_live_*.log files come first (but not rust_live_trace_*.log)
            let a_is_trace = a_name.contains("trace");
            let b_is_trace = b_name.contains("trace");
            match (a_is_rust_live && !a_is_trace, b_is_rust_live && !b_is_trace) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a_name.cmp(&b_name),
            }
        });

        return logs.into_iter().next();
    }
    None
}

fn extract_field(line: &str, prefix: &str) -> Option<String> {
    if let Some(pos) = line.find(prefix) {
        let start = pos + prefix.len();
        let rest = &line[start..];
        // Extract until whitespace or comma or end
        let end = rest.find(|c: char| c.is_whitespace() || c == ',' || c == '}').unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    None
}

fn parse_bad_dir_detail(line: &str) -> Option<BadDirDetail> {
    Some(BadDirDetail {
        idx: extract_field(line, "idx=")?.parse().ok()?,
        frs: extract_field(line, "frs=")?.parse().ok()?,
        first_child: extract_field(line, "first_child=")?.parse().ok()?,
        name_count: extract_field(line, "name_count=")?.parse().ok()?,
        stream_count: extract_field(line, "stream_count=")?.parse().ok()?,
        is_reparse: extract_field(line, "is_reparse=")?.parse().ok().unwrap_or(false),
    })
}

fn analyze_parity(cpp: &CsvData, rust: &CsvData) -> AnalysisResults {
    let mut results = AnalysisResults::default();

    results.cpp_rows = cpp.rows.len();
    results.rust_rows = rust.rows.len();

    // Composition counts (Fix #3: surface "what's missing" instantly)
    results.cpp_dir_count = cpp.rows.iter().filter(|r| r.is_directory).count();
    results.cpp_file_count = cpp.rows.iter().filter(|r| !r.is_directory && !is_ads_path(&r.path)).count();
    results.rust_dir_count = rust.rows.iter().filter(|r| r.is_directory).count();
    results.rust_file_count = rust.rows.iter().filter(|r| !r.is_directory && !is_ads_path(&r.path)).count();

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

    // Missing path prefixes histogram (Fix #3)
    // Group missing C++ paths by top-level prefix to identify patterns
    if !cpp_only.is_empty() {
        let mut prefix_counts: HashMap<String, usize> = HashMap::new();
        for path in cpp_only.iter().take(10000) {
            // Extract top-level prefix (e.g., "c:/windows" from "c:/windows/system32/foo.dll")
            let parts: Vec<&str> = path.split('/').collect();
            let prefix = if parts.len() >= 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                parts[0].to_string()
            };
            *prefix_counts.entry(prefix).or_insert(0) += 1;
        }
        // Sort by count descending
        let mut sorted: Vec<_> = prefix_counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        results.missing_path_prefixes = sorted.into_iter().take(10).collect();
    }

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

        // Tree metrics (only for directories) - collect ALL issues
        if cpp_row.is_directory {
            let size_diff = cpp_row.size != rust_row.size;
            let desc_diff = cpp_row.descendants != rust_row.descendants;

            if size_diff || desc_diff {
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
    results.tree_metric_total_count = results.tree_metric_issues.len();
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
    // Composition counts (Fix #3: surface "what's missing" instantly)
    println!("📊 COMPOSITION COUNTS ({})", label);
    println!("   C++ total rows:   {:>10}", results.cpp_rows);
    println!("   C++ directories:  {:>10}", results.cpp_dir_count);
    println!("   C++ files:        {:>10}", results.cpp_file_count);
    println!("   C++ ADS:          {:>10}", results.cpp_ads_count);
    println!();
    println!("   Rust total rows:  {:>10}", results.rust_rows);
    println!("   Rust directories: {:>10}", results.rust_dir_count);
    println!("   Rust files:       {:>10}", results.rust_file_count);
    println!("   Rust ADS:         {:>10}", results.rust_ads_count);
    println!();

    // Quick diagnosis: if Rust has ~0 files, it's "directories-only" output
    if results.rust_file_count == 0 && results.cpp_file_count > 0 {
        println!("   🔴 DIAGNOSIS: Rust output appears to be DIRECTORIES-ONLY!");
        println!("      This suggests a bug in file emission, not parsing.");
        println!();
    } else if results.rust_file_count < results.cpp_file_count / 2 {
        println!("   ⚠️  DIAGNOSIS: Rust is missing >50% of files!");
        println!();
    }

    // Path matching
    println!("🔗 PATH MATCHING ({})", label);
    println!("   Common paths:     {:>10}", results.common_paths);
    println!("   C++ only:         {:>10}", results.cpp_only_paths);
    println!("   Rust only:        {:>10}", results.rust_only_paths);
    println!("   Match rate:       {:>9.4}%", results.path_match_rate);
    println!();

    // Missing path prefixes histogram (Fix #3)
    if !results.missing_path_prefixes.is_empty() {
        println!("📁 MISSING PATH PREFIXES (top 10 from C++ only paths)");
        for (prefix, count) in &results.missing_path_prefixes {
            println!("   {:>6} {}", count, prefix);
        }
        println!();
    }

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
        let total = results.tree_metric_total_count;
        println!();
        println!("   📊 TOTAL MISALIGNED ENTRIES: {}", total);
        println!("   Status:           🔴 ISSUES FOUND");
        println!();

        // Show first 5
        println!("   ── First 5 issues ──");
        for issue in results.tree_metric_issues.iter().take(5) {
            println!("   {} ", issue.path);
            println!("      Size: C++={} Rust={}", issue.cpp_size, issue.rust_size);
            println!("      Desc: C++={} Rust={}", issue.cpp_descendants, issue.rust_descendants);
        }

        // Show 20 random samples if there are more than 25 total
        if total > 25 {
            println!();
            println!("   ── Random 20 samples (out of {}) ──", total);

            // Simple deterministic shuffle using LCG
            let mut indices: Vec<usize> = (0..total).collect();
            let seed = 12345u64;
            let a = 1103515245u64;
            let c = 12345u64;
            let m = 2u64.pow(31);
            let mut state = seed;

            // Fisher-Yates shuffle with LCG
            for i in (1..indices.len()).rev() {
                state = (a.wrapping_mul(state).wrapping_add(c)) % m;
                let j = (state as usize) % (i + 1);
                indices.swap(i, j);
            }

            // Take first 20 from shuffled indices (skip first 5 to avoid duplicates)
            let random_indices: Vec<usize> = indices.iter()
                .filter(|&&idx| idx >= 5)  // Skip first 5 already shown
                .take(20)
                .copied()
                .collect();

            for idx in random_indices {
                if let Some(issue) = results.tree_metric_issues.get(idx) {
                    println!("   {} ", issue.path);
                    println!("      Size: C++={} Rust={}", issue.cpp_size, issue.rust_size);
                    println!("      Desc: C++={} Rust={}", issue.cpp_descendants, issue.rust_descendants);
                }
            }
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

fn print_diagnostic_analysis(analysis: &DiagnosticLogAnalysis) {
    if let Some(ref log_file) = analysis.log_file {
        println!("📂 Log file: {}", log_file.display());
    } else {
        println!("⚠️  No log file found");
        return;
    }
    println!();

    // Tripwire verification (Fix #4: accept ANY [TRIP] marker)
    println!("🔍 TRIPWIRE VERIFICATION");
    if analysis.tripwire_found {
        println!("   ✅ Tripwire markers found in log");
        println!("   → Confirms the fixed code paths are executing");
        if !analysis.tripwire_samples.is_empty() {
            println!("   Samples:");
            for sample in &analysis.tripwire_samples {
                println!("     {}", sample);
            }
        }
    } else {
        println!("   🔴 No [TRIPWIRE] markers found in log!");
        println!("   → The binary may not have the fixed code paths");
        println!("   → Rebuild with latest code and verify with: strings uffs.exe | grep TRIPWIRE");
    }
    println!();

    // Post-tree diagnostic
    println!("📊 POST-TREE DIAGNOSTIC");
    if analysis.post_tree_diagnostic_found {
        println!("   Found: [tree] FINAL: directories with descendants==0");
        println!("   Bad directories after all passes: {}", analysis.final_bad_dir_count);

        if analysis.final_bad_dir_count == 0 {
            println!("   ✅ All directories have valid tree metrics");
        } else if analysis.final_bad_dir_count <= 3 {
            println!("   ⚠️  Small number of bad directories - likely root + reparse points");
            println!("   → This is the 'unstamped directory' pattern from the deep-dive");
        } else {
            println!("   🔴 Many directories with descendants==0 - investigate!");
        }

        if !analysis.bad_dir_details.is_empty() {
            println!();
            println!("   Bad directory details:");
            for detail in &analysis.bad_dir_details {
                println!("     idx={} frs={} first_child={} names={} streams={} reparse={}",
                    detail.idx, detail.frs, detail.first_child,
                    detail.name_count, detail.stream_count, detail.is_reparse);
            }
        }
    } else {
        println!("   ⚠️  Post-tree diagnostic not found in log");
        println!("   → The binary may not have the diagnostic code");
    }
    println!();

    // Self-heal and orphan sweep
    println!("🔧 SELF-HEAL & ORPHAN SWEEP");
    println!("   Self-heal triggered: {}", if analysis.self_heal_triggered { "Yes" } else { "No" });
    println!("   Orphan sweep count: {}", analysis.orphan_sweep_count);
    println!();
}

fn write_comprehensive_report(
    live_results: &AnalysisResults,
    offline_results: Option<&(AnalysisResults, PathBuf)>,
    diagnostic_analysis: Option<&DiagnosticLogAnalysis>,
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

    // DIAGNOSTIC LOG SECTION
    if let Some(diag) = diagnostic_analysis {
        writeln!(f, "---").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "# Part 3: Diagnostic Log Analysis").unwrap();
        writeln!(f).unwrap();
        write_diagnostic_section(&mut f, diag, live_results);
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
        let total = results.tree_metric_total_count;
        writeln!(f, "### 🔴 Tree Metrics Issues").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "**📊 TOTAL MISALIGNED ENTRIES: {}**", total).unwrap();
        writeln!(f).unwrap();

        // First 5
        writeln!(f, "#### First 5 Issues").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "| Path | C++ Size | Rust Size | C++ Desc | Rust Desc |").unwrap();
        writeln!(f, "|------|----------|-----------|----------|-----------|").unwrap();
        for issue in results.tree_metric_issues.iter().take(5) {
            writeln!(f, "| `{}` | {} | {} | {} | {} |",
                issue.path, issue.cpp_size, issue.rust_size,
                issue.cpp_descendants, issue.rust_descendants
            ).unwrap();
        }
        writeln!(f).unwrap();

        // Random 20 samples if more than 25 total
        if total > 25 {
            writeln!(f, "#### Random 20 Samples (out of {})", total).unwrap();
            writeln!(f).unwrap();
            writeln!(f, "| Path | C++ Size | Rust Size | C++ Desc | Rust Desc |").unwrap();
            writeln!(f, "|------|----------|-----------|----------|-----------|").unwrap();

            // Simple deterministic shuffle using LCG
            let mut indices: Vec<usize> = (0..total).collect();
            let seed = 12345u64;
            let a = 1103515245u64;
            let c = 12345u64;
            let m = 2u64.pow(31);
            let mut state = seed;

            // Fisher-Yates shuffle with LCG
            for i in (1..indices.len()).rev() {
                state = (a.wrapping_mul(state).wrapping_add(c)) % m;
                let j = (state as usize) % (i + 1);
                indices.swap(i, j);
            }

            // Take first 20 from shuffled indices (skip first 5 to avoid duplicates)
            let random_indices: Vec<usize> = indices.iter()
                .filter(|&&idx| idx >= 5)
                .take(20)
                .copied()
                .collect();

            for idx in random_indices {
                if let Some(issue) = results.tree_metric_issues.get(idx) {
                    writeln!(f, "| `{}` | {} | {} | {} | {} |",
                        issue.path, issue.cpp_size, issue.rust_size,
                        issue.cpp_descendants, issue.rust_descendants
                    ).unwrap();
                }
            }
            writeln!(f).unwrap();
        }
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

/// Write diagnostic log analysis section to report
fn write_diagnostic_section(f: &mut File, diag: &DiagnosticLogAnalysis, live_results: &AnalysisResults) {
    writeln!(f, "> Analysis of runtime diagnostic logs to verify correct code paths are executing.").unwrap();
    writeln!(f).unwrap();

    if let Some(ref log_file) = diag.log_file {
        writeln!(f, "**Log file:** `{}`", log_file.display()).unwrap();
    } else {
        writeln!(f, "⚠️ No log file found in trial directory.").unwrap();
        return;
    }
    writeln!(f).unwrap();

    // Tripwire verification table (Fix #4: accept ANY [TRIP] marker)
    writeln!(f, "## Tripwire Verification").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "| Check | Status | Meaning |").unwrap();
    writeln!(f, "|-------|--------|---------|").unwrap();
    writeln!(f, "| [TRIPWIRE] markers | {} | {} |",
        if diag.tripwire_found { "✅ Found" } else { "🔴 NOT FOUND" },
        if diag.tripwire_found { "Fixed code paths are executing" } else { "Binary may not have fixed code - REBUILD!" }
    ).unwrap();
    writeln!(f, "| Post-tree diagnostic | {} | {} |",
        if diag.post_tree_diagnostic_found { "✅ Found" } else { "⚠️ Not found" },
        if diag.post_tree_diagnostic_found { "Diagnostic logging is active" } else { "Diagnostic code may be missing" }
    ).unwrap();
    writeln!(f).unwrap();

    // Show tripwire samples if found
    if diag.tripwire_found && !diag.tripwire_samples.is_empty() {
        writeln!(f, "**Tripwire samples found:**").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "```").unwrap();
        for sample in &diag.tripwire_samples {
            writeln!(f, "{}", sample).unwrap();
        }
        writeln!(f, "```").unwrap();
        writeln!(f).unwrap();
    }

    // Post-tree diagnostic results
    if diag.post_tree_diagnostic_found {
        writeln!(f, "## Post-Tree Diagnostic Results").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "**Directories with descendants==0 after all passes:** {}", diag.final_bad_dir_count).unwrap();
        writeln!(f).unwrap();

        // Interpret the pattern
        if diag.final_bad_dir_count == 0 {
            writeln!(f, "✅ All directories have valid tree metrics after tree computation.").unwrap();
        } else {
            // Check if this matches the "unstamped directory" pattern
            let zeros_in_csv = live_results.tree_metric_issues.iter()
                .filter(|i| i.rust_size == 0 && i.rust_descendants == 0)
                .count();

            if diag.final_bad_dir_count <= 3 && zeros_in_csv <= 3 {
                writeln!(f, "⚠️ **Unstamped Directory Pattern Detected**").unwrap();
                writeln!(f).unwrap();
                writeln!(f, "This matches the pattern from `LIVE_TREE_METRICS_REMAINING_GAP_DEEP_DIVE.md`:").unwrap();
                writeln!(f, "- Small number of directories (≤3) with Size=0/Descendants=0").unwrap();
                writeln!(f, "- Typically: root directory + reparse points (junctions/symlinks)").unwrap();
                writeln!(f).unwrap();
                writeln!(f, "**Likely cause:** These directories are not being stamped by the tree metrics algorithm.").unwrap();
            } else if diag.final_bad_dir_count > 0 && zeros_in_csv == 0 {
                writeln!(f, "⚠️ **Failure Mode B: Reset After Compute**").unwrap();
                writeln!(f).unwrap();
                writeln!(f, "The log shows {} bad directories, but CSV shows 0 with zeros.", diag.final_bad_dir_count).unwrap();
                writeln!(f, "This suggests records are being stamped but then reset/replaced later.").unwrap();
            } else {
                writeln!(f, "🔴 **Multiple directories with invalid tree metrics**").unwrap();
                writeln!(f).unwrap();
                writeln!(f, "Log shows {} bad directories, CSV shows {} with zeros.", diag.final_bad_dir_count, zeros_in_csv).unwrap();
            }
        }
        writeln!(f).unwrap();

        // Bad directory details
        if !diag.bad_dir_details.is_empty() {
            writeln!(f, "### Bad Directory Details").unwrap();
            writeln!(f).unwrap();
            writeln!(f, "| idx | FRS | first_child | names | streams | reparse |").unwrap();
            writeln!(f, "|-----|-----|-------------|-------|---------|---------|").unwrap();
            for detail in &diag.bad_dir_details {
                writeln!(f, "| {} | {} | {} | {} | {} | {} |",
                    detail.idx, detail.frs, detail.first_child,
                    detail.name_count, detail.stream_count, detail.is_reparse
                ).unwrap();
            }
            writeln!(f).unwrap();
        }
    }

    // Self-heal and orphan sweep
    writeln!(f, "## Self-Heal & Orphan Sweep").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "| Mechanism | Status |").unwrap();
    writeln!(f, "|-----------|--------|").unwrap();
    writeln!(f, "| Self-heal triggered | {} |", if diag.self_heal_triggered { "Yes" } else { "No" }).unwrap();
    writeln!(f, "| Orphan sweep count | {} |", diag.orphan_sweep_count).unwrap();
    writeln!(f).unwrap();
}
