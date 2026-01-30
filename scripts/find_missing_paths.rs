#!/usr/bin/env rust-script
//! Find paths that are in C++ output but not in Rust output.
//! Output the full CSV lines for analysis.
//!
//! Usage:
//!   rust-script scripts/find_missing_paths.rs <cpp_file> <rust_file>
//!
//! ```cargo
//! [dependencies]
//! ```

use std::collections::HashSet;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn normalize_path(path: &str) -> String {
    path.to_lowercase().replace('\\', "/").trim_end_matches('/').to_string()
}

/// Parse a CSV line with quoted fields
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

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: rust-script {} <cpp_file> <rust_file>", args[0]);
        std::process::exit(1);
    }

    let cpp_file = &args[1];
    let rust_file = &args[2];

    // First pass: collect all Rust paths
    eprintln!("Reading Rust file...");
    let rust_reader = BufReader::new(File::open(rust_file).expect("Failed to open Rust file"));
    let mut rust_paths: HashSet<String> = HashSet::new();
    
    for line in rust_reader.lines().flatten() {
        if line.is_empty() || line.starts_with("\"Path\"") || line.starts_with("Path") {
            continue;
        }
        let fields = parse_csv_line(&line);
        if !fields.is_empty() {
            rust_paths.insert(normalize_path(&fields[0]));
        }
    }
    eprintln!("Rust paths: {}", rust_paths.len());

    // Second pass: find C++ paths not in Rust
    eprintln!("Reading C++ file and finding missing...");
    let cpp_reader = BufReader::new(File::open(cpp_file).expect("Failed to open C++ file"));
    let mut missing = Vec::new();
    
    for line in cpp_reader.lines().flatten() {
        if line.is_empty() || line.starts_with("\"Path\"") || line.starts_with("Path") {
            continue;
        }
        let fields = parse_csv_line(&line);
        if !fields.is_empty() {
            let norm = normalize_path(&fields[0]);
            if !rust_paths.contains(&norm) {
                missing.push((fields[0].clone(), line.clone()));
            }
        }
    }

    eprintln!("\nFound {} paths in C++ but not in Rust:\n", missing.len());
    
    // Output missing paths with full CSV line
    for (path, full_line) in &missing {
        println!("PATH: {}", path);
        println!("LINE: {}", full_line);
        println!();
    }
    
    // Write to file for further analysis
    let mut out = File::create("missing_paths.txt").expect("Failed to create output file");
    for (path, _) in &missing {
        writeln!(out, "{}", path).ok();
    }
    eprintln!("Written to missing_paths.txt");
}

