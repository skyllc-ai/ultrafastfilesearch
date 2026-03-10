#!/usr/bin/env rust-script
//! Diagnostic script to compare MFT record counts between the reference output and Rust output.
//!
//! Usage:
//!   rust-script scripts/diagnose_mft_counts.rs <reference_output.txt> <rust_output.txt>
//!
//! This analyzes:
//!   1. Total record counts per drive
//!   2. Directory vs file counts per drive
//!   3. Records with resolved vs unresolved paths
//!   4. Sample of unresolved paths per drive
//!
//! ```cargo
//! [dependencies]
//! polars = { version = "0.46", features = ["lazy", "csv", "strings"] }
//! ```

use polars::prelude::*;
use std::collections::HashMap;
use std::env;
use std::path::Path;

fn load_csv(path: &Path) -> PolarsResult<DataFrame> {
    // Reference output is CSV with quoted fields, comma separator.
    // Rust output is TSV
    // Try to detect format from first line
    let first_line = std::fs::read_to_string(path)
        .map(|s| s.lines().next().unwrap_or("").to_string())
        .unwrap_or_default();

    let separator = if first_line.contains("\t") { b'\t' } else { b',' };

    CsvReadOptions::default()
        .with_has_header(true)
        .with_skip_rows_after_header(1) // Skip empty line after header
        .with_parse_options(
            CsvParseOptions::default()
                .with_separator(separator)
                .with_quote_char(Some(b'"'))
        )
        .try_into_reader_with_file_path(Some(path.into()))?
        .finish()
}

fn extract_drive(path: &str) -> Option<char> {
    let path_lower = path.to_lowercase();
    if path_lower.len() >= 2 && path_lower.chars().nth(1) == Some(':') {
        path_lower.chars().next()
    } else if path_lower.starts_with("<unknown:") {
        // Can't determine drive for unresolved paths
        None
    } else {
        None
    }
}

fn analyze_file(label: &str, df: &DataFrame) -> HashMap<char, (usize, usize, usize)> {
    println!("\n{}", "=".repeat(70));
    println!("{} ANALYSIS", label);
    println!("{}", "=".repeat(70));
    println!("Total rows: {}", df.height());
    
    let path_col = df.column("Path").expect("No Path column");
    let dir_col = df.column("Directory Flag").ok();

    let mut drive_stats: HashMap<char, (usize, usize, usize)> = HashMap::new(); // (total, dirs, unresolved)
    let mut unresolved_count = 0;
    let mut null_count = 0;

    let path_series = path_col.str().expect("Path not string");
    // Directory Flag can be bool or i64 (0/1)
    let dir_series: Option<Vec<bool>> = dir_col.map(|c| {
        if let Ok(b) = c.bool() {
            (0..df.height()).map(|i| b.get(i).unwrap_or(false)).collect()
        } else if let Ok(i) = c.i64() {
            (0..df.height()).map(|idx| i.get(idx).unwrap_or(0) != 0).collect()
        } else {
            vec![false; df.height()]
        }
    });

    for i in 0..df.height() {
        let path_opt = path_series.get(i);
        let is_dir = dir_series.as_ref().map(|s| s[i]).unwrap_or(false);
        
        match path_opt {
            None => {
                null_count += 1;
            }
            Some(path) => {
                if path.starts_with("<unknown:") {
                    unresolved_count += 1;
                    // Try to find drive from later in path or mark as unknown
                    let entry = drive_stats.entry('?').or_insert((0, 0, 0));
                    entry.0 += 1;
                    if is_dir { entry.1 += 1; }
                    entry.2 += 1;
                } else if let Some(drive) = extract_drive(path) {
                    let entry = drive_stats.entry(drive).or_insert((0, 0, 0));
                    entry.0 += 1;
                    if is_dir { entry.1 += 1; }
                } else {
                    let entry = drive_stats.entry('?').or_insert((0, 0, 0));
                    entry.0 += 1;
                    if is_dir { entry.1 += 1; }
                }
            }
        }
    }
    
    println!("\nNull paths: {}", null_count);
    println!("Unresolved (<unknown:...>): {}", unresolved_count);
    
    println!("\nPer-drive breakdown:");
    println!("{:>6} {:>12} {:>12} {:>12}", "Drive", "Total", "Dirs", "Unresolved");
    println!("{:-<6} {:-<12} {:-<12} {:-<12}", "", "", "", "");
    
    let mut drives: Vec<_> = drive_stats.keys().collect();
    drives.sort();
    
    for drive in drives {
        let (total, dirs, unresolved) = drive_stats[drive];
        println!("{:>6} {:>12} {:>12} {:>12}", 
            format!("{}:", drive.to_uppercase()), 
            total, 
            dirs,
            unresolved);
    }
    
    drive_stats
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <reference_output.txt> <rust_output.txt>", args[0]);
        std::process::exit(1);
    }
    
    let reference_path = Path::new(&args[1]);
    let rust_path = Path::new(&args[2]);
    
    println!("Loading reference output: {}", reference_path.display());
    let reference_df = load_csv(reference_path).expect("Failed to load reference file");

    println!("Loading Rust output: {}", rust_path.display());
    let rust_df = load_csv(rust_path).expect("Failed to load Rust file");
    
    let reference_stats = analyze_file("REFERENCE", &reference_df);
    let rust_stats = analyze_file("RUST", &rust_df);
    
    // Comparison
    println!("\n{}", "=".repeat(70));
    println!("COMPARISON: REFERENCE vs Rust");
    println!("{}", "=".repeat(70));
    println!("{:>6} {:>16} {:>12} {:>12} {:>10}", "Drive", "Reference Total", "Rust Total", "Difference", "% Match");
    println!("{:-<6} {:-<12} {:-<12} {:-<12} {:-<10}", "", "", "", "", "");
    
    let mut all_drives: std::collections::HashSet<char> = reference_stats.keys().cloned().collect();
    all_drives.extend(rust_stats.keys().cloned());
    let mut drives: Vec<_> = all_drives.into_iter().collect();
    drives.sort();
    
    for drive in drives {
        let reference_total = reference_stats.get(&drive).map(|s| s.0).unwrap_or(0);
        let rust_total = rust_stats.get(&drive).map(|s| s.0).unwrap_or(0);
        let diff = reference_total as i64 - rust_total as i64;
        let pct = if reference_total > 0 { 
            (rust_total as f64 / reference_total as f64) * 100.0 
        } else { 
            0.0 
        };
        println!("{:>6} {:>12} {:>12} {:>+12} {:>9.1}%", 
            format!("{}:", drive.to_uppercase()), 
            reference_total, 
            rust_total,
            diff,
            pct);
    }
}

