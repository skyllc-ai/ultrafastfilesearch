#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! ```

use std::collections::BTreeSet;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <reference_output.txt> <missing_paths.txt>", args[0]);
        eprintln!();
        eprintln!("Extracts FRS numbers for missing paths and analyzes their distribution.");
        std::process::exit(1);
    }

    let reference_file = &args[1];
    let missing_file = &args[2];

    // Read missing paths
    println!("📖 Reading missing paths from: {}", missing_file);
    let missing_paths: BTreeSet<String> = BufReader::new(File::open(missing_file).unwrap())
        .lines()
        .filter_map(|line| line.ok())
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_lowercase())
        .collect();

    println!("   Found {} missing paths", missing_paths.len());

    // Read reference output and extract FRS for missing paths.
    println!("📖 Scanning reference output: {}", reference_file);
    let mut frs_numbers = Vec::new();
    let mut line_count = 0u64;

    for line in BufReader::new(File::open(reference_file).unwrap()).lines() {
        line_count += 1;
        if line_count % 1_000_000 == 0 {
            print!("\r   Processed {} million lines...", line_count / 1_000_000);
        }

        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Reference output format: path,size,timestamp,frs,parent_frs,flags
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 4 {
            continue;
        }

        let path = parts[0].trim().to_lowercase();
        if missing_paths.contains(&path) {
            // Extract FRS (4th column, index 3)
            if let Ok(frs) = parts[3].trim().parse::<u64>() {
                frs_numbers.push((frs, path.clone()));
            }
        }
    }
    println!("\r   Processed {} total lines", line_count);

    if frs_numbers.is_empty() {
        println!("❌ No FRS numbers found for missing paths!");
        return;
    }

    // Sort by FRS
    frs_numbers.sort_by_key(|(frs, _)| *frs);

    println!("\n📊 FRS Analysis for {} Missing Paths", frs_numbers.len());
    println!("{}", "=".repeat(80));

    // Print all FRS numbers
    println!("\n🔢 FRS Numbers (sorted):");
    for (frs, path) in &frs_numbers {
        println!("  FRS {:10} - {}", frs, path);
    }

    // Calculate statistics
    let min_frs = frs_numbers.first().unwrap().0;
    let max_frs = frs_numbers.last().unwrap().0;
    let range = max_frs - min_frs;

    println!("\n📈 Distribution Statistics:");
    println!("  Min FRS:     {:10}", min_frs);
    println!("  Max FRS:     {:10}", max_frs);
    println!("  Range:       {:10}", range);
    println!("  Total MFT:   ~7,058,029 records");
    println!("  Spread:      {:.2}% of MFT", (range as f64 / 7_058_029.0) * 100.0);

    // Check for clustering
    println!("\n🔍 Clustering Analysis:");
    let mut gaps = Vec::new();
    for i in 1..frs_numbers.len() {
        let gap = frs_numbers[i].0 - frs_numbers[i - 1].0;
        gaps.push(gap);
    }

    if !gaps.is_empty() {
        gaps.sort();
        let median_gap = gaps[gaps.len() / 2];
        let max_gap = *gaps.last().unwrap();
        let min_gap = *gaps.first().unwrap();

        println!("  Min gap:     {:10} records", min_gap);
        println!("  Median gap:  {:10} records", median_gap);
        println!("  Max gap:     {:10} records", max_gap);

        // Check if they cluster in specific ranges
        let chunk_size = 1024 * 1024 / 1024; // ~1MB chunks at 1024 bytes/record = 1024 records
        let mut chunks: BTreeSet<u64> = BTreeSet::new();
        for (frs, _) in &frs_numbers {
            chunks.insert(frs / chunk_size);
        }

        println!("\n📦 Chunk Distribution (1MB chunks = ~1024 records):");
        println!("  Missing paths span {} different chunks", chunks.len());
        println!("  Chunks: {:?}", chunks.iter().collect::<Vec<_>>());

        if chunks.len() < frs_numbers.len() / 2 {
            println!("  ⚠️  CLUSTERED: Multiple missing paths in same chunks!");
        } else {
            println!("  ✅ DISTRIBUTED: Missing paths spread across many chunks");
        }
    }

    println!();
}

