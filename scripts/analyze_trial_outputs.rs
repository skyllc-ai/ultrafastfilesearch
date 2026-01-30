#!/usr/bin/env rust-script
//! Analyze UFFS trial run outputs - compare C++ vs Rust outputs for exact parity.
//!
//! Usage:
//!   rust-script scripts/analyze_trial_outputs.rs <trial_dir>
//!   rust-script scripts/analyze_trial_outputs.rs docs/trial_runs/d_disk
//!
//! Expected files in trial_dir:
//!   - cpp_d.txt (or cpp_<drive>.txt) - C++ reference output
//!   - rust_new_d.txt (or rust_new_<drive>.txt) - Rust with new tree algo
//!
//! The script will:
//! 1. Parse both files and extract all paths
//! 2. Compare for exact match
//! 3. Report any differences with context
//! 4. Analyze patterns in differences (if any)
//!
//! ```cargo
//! [dependencies]
//! ```

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Record {
    line_num: usize,
    path: String,
    type_field: String,
    size: String,
    descendants: String,
}

/// Parse a CSV line with quoted fields (handles commas inside quotes)
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes {
                    // Check for escaped quote ""
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

fn parse_uffs_output(filepath: &Path) -> Vec<Record> {
    let file = match File::open(filepath) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening {}: {}", filepath.display(), e);
            return Vec::new();
        }
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.is_empty() {
            continue;
        }

        // Skip header line
        if line.starts_with("\"Path\"") || line.starts_with("Path") {
            continue;
        }

        let parts = parse_csv_line(&line);

        // CSV format: Path, Name, Path Only, Size, Size on Disk, Created, Last Written, Last Accessed, Descendants, ...
        // Index:      0     1     2          3     4             5        6             7              8
        if parts.len() >= 9 {
            records.push(Record {
                line_num: line_num + 1,
                path: parts[0].clone(),
                type_field: String::new(), // Not in this format
                size: parts[3].clone(),
                descendants: parts[8].clone(),
            });
        }
    }
    records
}

fn normalize_path(path: &str) -> String {
    path.to_lowercase().replace('\\', "/").trim_end_matches('/').to_string()
}

fn compare_outputs(cpp_file: &Path, rust_file: &Path) {
    println!("\n{}", "=".repeat(70));
    println!("Comparing:");
    println!("  C++:  {}", cpp_file.display());
    println!("  Rust: {}", rust_file.display());
    println!("{}", "=".repeat(70));

    let cpp_records = parse_uffs_output(cpp_file);
    let rust_records = parse_uffs_output(rust_file);

    println!("\nRecord counts:");
    println!("  C++:  {:>12}", format_num(cpp_records.len()));
    println!("  Rust: {:>12}", format_num(rust_records.len()));
    let diff = rust_records.len() as i64 - cpp_records.len() as i64;
    println!("  Diff: {:>+12}", diff);

    // Build path maps
    let cpp_paths: HashMap<String, &Record> = cpp_records
        .iter()
        .map(|r| (normalize_path(&r.path), r))
        .collect();
    let rust_paths: HashMap<String, &Record> = rust_records
        .iter()
        .map(|r| (normalize_path(&r.path), r))
        .collect();

    let cpp_keys: HashSet<_> = cpp_paths.keys().cloned().collect();
    let rust_keys: HashSet<_> = rust_paths.keys().cloned().collect();

    let common: HashSet<_> = cpp_keys.intersection(&rust_keys).cloned().collect();
    let cpp_only: Vec<_> = cpp_keys.difference(&rust_keys).cloned().collect();
    let rust_only: Vec<_> = rust_keys.difference(&cpp_keys).cloned().collect();

    println!("\nPath comparison:");
    println!("  Common paths:  {:>12}", format_num(common.len()));
    println!("  C++ only:      {:>12}", format_num(cpp_only.len()));
    println!("  Rust only:     {:>12}", format_num(rust_only.len()));

    if !cpp_paths.is_empty() {
        let match_pct = common.len() as f64 / cpp_paths.len() as f64 * 100.0;
        println!("  Match rate:    {:>11.4}%", match_pct);
    }

    // Check attribute differences
    let mut attr_diffs = Vec::new();
    for path in &common {
        let cpp_rec = cpp_paths.get(path).unwrap();
        let rust_rec = rust_paths.get(path).unwrap();
        if cpp_rec.size != rust_rec.size || cpp_rec.descendants != rust_rec.descendants {
            attr_diffs.push((
                path.clone(),
                cpp_rec.size.clone(),
                rust_rec.size.clone(),
                cpp_rec.descendants.clone(),
                rust_rec.descendants.clone(),
            ));
        }
    }

    if !attr_diffs.is_empty() {
        println!("\n⚠️  Attribute differences in common paths: {}", attr_diffs.len());
        println!("\nFirst 10 attribute differences:");
        for (path, cpp_size, rust_size, cpp_desc, rust_desc) in attr_diffs.iter().take(10) {
            println!("  {}", path);
            println!("    Size: C++={} vs Rust={}", cpp_size, rust_size);
            println!("    Desc: C++={} vs Rust={}", cpp_desc, rust_desc);
        }
    }

    // Report missing paths
    if !cpp_only.is_empty() {
        println!("\n❌ Paths in C++ but NOT in Rust (first 20):");
        let mut sorted: Vec<_> = cpp_only.iter().collect();
        sorted.sort();
        for p in sorted.iter().take(20) {
            println!("  {}", p);
        }
        if cpp_only.len() > 20 {
            println!("  ... and {} more", cpp_only.len() - 20);
        }
    }

    if !rust_only.is_empty() {
        println!("\n❌ Paths in Rust but NOT in C++ (first 20):");
        let mut sorted: Vec<_> = rust_only.iter().collect();
        sorted.sort();
        for p in sorted.iter().take(20) {
            println!("  {}", p);
        }
        if rust_only.len() > 20 {
            println!("  ... and {} more", rust_only.len() - 20);
        }
    }

    // Perfect match check
    if cpp_only.is_empty() && rust_only.is_empty() && attr_diffs.is_empty() {
        println!("\n✅ PERFECT MATCH - All {} paths match exactly!", format_num(common.len()));
    }
}

fn format_num(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

fn find_files(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with(pattern) && name.ends_with(".txt") {
                        results.push(path);
                    }
                }
            }
        }
    }
    results.sort();
    results
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: rust-script {} <trial_dir>", args[0]);
        eprintln!();
        eprintln!("Expected files in trial_dir:");
        eprintln!("  - cpp_d.txt (or cpp_<drive>.txt) - C++ reference output");
        eprintln!("  - rust_new_d.txt (or rust_new_<drive>.txt) - Rust with new tree algo");
        std::process::exit(1);
    }

    let trial_dir = Path::new(&args[1]);
    if !trial_dir.exists() {
        eprintln!("Error: Directory not found: {}", trial_dir.display());
        std::process::exit(1);
    }

    let cpp_files = find_files(trial_dir, "cpp_");
    let rust_new_files = find_files(trial_dir, "rust_new_");

    println!("Trial directory: {}", trial_dir.display());
    println!("Found {} C++ files, {} Rust (new) files", cpp_files.len(), rust_new_files.len());

    for cpp_file in &cpp_files {
        if let Some(stem) = cpp_file.file_stem().and_then(|s| s.to_str()) {
            let drive = stem.strip_prefix("cpp_").unwrap_or("");
            let rust_file = trial_dir.join(format!("rust_new_{}.txt", drive));
            if rust_file.exists() {
                compare_outputs(cpp_file, &rust_file);
            } else {
                println!("\n⚠️  No matching Rust file for {}", cpp_file.display());
            }
        }
    }
}

