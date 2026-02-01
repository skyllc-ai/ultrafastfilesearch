#!/usr/bin/env rust-script
//! Analyze parity differences between C++ and Rust scan outputs.
//! Finds paths in Rust but not in C++, and vice versa.
//! Provides detailed pattern analysis of the differences.
//!
//! Usage:
//!   rust-script scripts/analyze_parity_differences.rs <cpp_file> <rust_file>
//!
//! Example:
//!   rust-script scripts/analyze_parity_differences.rs \
//!     docs/trial_runs/d_disk/cpp_d.txt \
//!     docs/trial_runs/d_disk/scan_output.txt
//!
//! ```cargo
//! [dependencies]
//! ```

use std::collections::HashSet;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <cpp_file> <rust_file>", args[0]);
        eprintln!("\nExample:");
        eprintln!("  rust-script scripts/analyze_parity_differences.rs cpp_d.txt rust_d.txt");
        std::process::exit(1);
    }

    let cpp_file = &args[1];
    let rust_file = &args[2];

    println!("🔍 Analyzing Parity Differences: Rust vs C++ Baseline");
    println!();

    // Extract paths from both files
    println!("📊 Step 1: Extracting paths from both files...");
    println!("  This may take a few minutes for 7M+ lines...");
    println!();

    let cpp_paths = extract_cpp_paths(cpp_file);
    let rust_paths = extract_rust_paths(rust_file);

    println!("  C++ paths:  {}", cpp_paths.len());
    println!("  Rust paths: {}", rust_paths.len());
    println!();

    // Find differences
    println!("📊 Step 2: Finding differences...");
    println!();

    let extra_in_rust: Vec<_> = rust_paths.difference(&cpp_paths).collect();
    let missing_in_rust: Vec<_> = cpp_paths.difference(&rust_paths).collect();

    println!("  Paths in Rust but NOT in C++: {}", extra_in_rust.len());
    println!("  Paths in C++ but NOT in Rust: {}", missing_in_rust.len());
    println!();

    // Analyze extra paths in Rust
    if !extra_in_rust.is_empty() {
        analyze_paths("Extra in Rust", &extra_in_rust);
    }

    // Analyze missing paths in Rust
    if !missing_in_rust.is_empty() {
        analyze_paths("Missing in Rust", &missing_in_rust);
    }

    println!("✅ Analysis complete!");
}

fn extract_cpp_paths(file: &str) -> HashSet<String> {
    let f = File::open(file).expect("Failed to open C++ file");
    let reader = BufReader::new(f);
    let mut paths = HashSet::new();

    for line in reader.lines() {
        let line = line.expect("Failed to read line");
        if let Some(path) = extract_cpp_path(&line) {
            paths.insert(path);
        }
    }

    paths
}

fn extract_rust_paths(file: &str) -> HashSet<String> {
    let f = File::open(file).expect("Failed to open Rust file");
    let reader = BufReader::new(f);
    let mut paths = HashSet::new();

    for line in reader.lines() {
        let line = line.expect("Failed to read line");
        if let Some(path) = extract_rust_path(&line) {
            paths.insert(path);
        }
    }

    paths
}

fn extract_cpp_path(line: &str) -> Option<String> {
    // C++ format: "D:\path\to\file","name","parent",...
    // Path is first column, quoted
    if line.is_empty() {
        return None;
    }

    let fields = parse_csv_line(line);
    if fields.is_empty() {
        return None;
    }

    Some(normalize_path(&fields[0]))
}

fn extract_rust_path(line: &str) -> Option<String> {
    // Rust format: frs,parent_frs,name,ext,path,...
    // Path is 5th column (index 4)
    if line.is_empty() {
        return None;
    }

    let fields = parse_csv_line(line);
    if fields.len() < 5 {
        return None;
    }

    Some(normalize_path(&fields[4]))
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

fn analyze_paths(label: &str, paths: &[&String]) {
    println!("🔍 Analyzing: {}", label);
    println!();

    // Show first 20 paths
    println!("📄 First 20 paths:");
    for (i, path) in paths.iter().take(20).enumerate() {
        println!("  {}. {}", i + 1, path);
    }
    println!();

    // Pattern analysis
    println!("📊 Pattern analysis:");
    println!();

    // Count ADS (Alternate Data Streams)
    let ads_count = paths.iter().filter(|p| p.contains(':')).count();
    println!("  Paths with ':' (potential ADS): {}", ads_count);

    // Count directories (ending with /)
    let dir_count = paths.iter().filter(|p| p.ends_with('/')).count();
    println!("  Paths ending with '/' (directories): {}", dir_count);

    println!();
    println!("  File type breakdown:");

    // File type counts
    let bin_count = paths.iter().filter(|p| p.contains(".bin")).count();
    let exe_count = paths.iter().filter(|p| p.contains(".exe")).count();
    let dll_count = paths.iter().filter(|p| p.contains(".dll")).count();
    let txt_count = paths.iter().filter(|p| p.contains(".txt")).count();
    let zone_id_count = paths.iter().filter(|p| p.contains("zone.identifier")).count();
    let dropbox_attrs_count = paths.iter().filter(|p| p.contains("com.dropbox.attrs")).count();

    println!("    .bin files: {}", bin_count);
    println!("    .exe files: {}", exe_count);
    println!("    .dll files: {}", dll_count);
    println!("    .txt files: {}", txt_count);
    println!("    Zone.Identifier: {}", zone_id_count);
    println!("    com.dropbox.attrs: {}", dropbox_attrs_count);

    println!();
    println!("  Path patterns:");

    // Path pattern counts
    let rust_target_count = paths.iter().filter(|p| p.contains("target/")).count();
    let dropbox_count = paths.iter().filter(|p| p.contains("dropbox")).count();
    let windows_count = paths.iter().filter(|p| p.contains("windows/")).count();

    println!("    Rust target dirs: {}", rust_target_count);
    println!("    Dropbox paths: {}", dropbox_count);
    println!("    System paths: {}", windows_count);

    println!();
}
