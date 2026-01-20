#!/usr/bin/env rust-script
//! Analyze C++ UFFS output file (cpp.txt) to extract file/directory statistics per drive.
//!
//! This script streams through the large CSV file efficiently without loading it all into memory.
//!
//! Usage:
//!   rust-script scripts/analyze_cpp_stats.rs /path/to/cpp.txt
//!
//! Output:
//!   Per-drive and total counts of files and directories.
//!
//! ```cargo
//! [dependencies]
//! csv = "1.3"
//! ```

use csv::ReaderBuilder;
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <cpp_output.txt>", args[0]);
        eprintln!("\nAnalyzes C++ UFFS output to extract file/directory counts per drive.");
        std::process::exit(1);
    }

    let path = &args[1];
    println!("═══════════════════════════════════════════════════════════════");
    println!("  C++ UFFS Output Analysis");
    println!("═══════════════════════════════════════════════════════════════");
    println!("File: {}", path);
    println!();

    let start = Instant::now();
    
    let file = File::open(path).expect("Failed to open file");
    let reader = BufReader::with_capacity(8 * 1024 * 1024, file); // 8MB buffer
    
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(reader);

    // Get header indices
    let headers = csv_reader.headers().expect("Failed to read headers").clone();
    let path_idx = headers.iter().position(|h| h == "Path").expect("No Path column");
    let dir_flag_idx = headers.iter().position(|h| h == "Directory Flag").expect("No Directory Flag column");

    println!("Found columns: Path at {}, Directory Flag at {}", path_idx, dir_flag_idx);
    println!();

    // Stats per drive: (files, directories)
    let mut drive_stats: HashMap<char, (u64, u64)> = HashMap::new();
    let mut total_rows: u64 = 0;
    let mut parse_errors: u64 = 0;
    let mut unknown_drive: u64 = 0;

    for result in csv_reader.records() {
        match result {
            Ok(record) => {
                total_rows += 1;
                
                // Progress every 1M rows
                if total_rows % 1_000_000 == 0 {
                    let elapsed = start.elapsed().as_secs_f64();
                    let rate = total_rows as f64 / elapsed;
                    eprintln!("  Processed {} rows ({:.0} rows/sec)...", total_rows, rate);
                }

                let path = record.get(path_idx).unwrap_or("");
                let dir_flag = record.get(dir_flag_idx).unwrap_or("0");
                
                // Extract drive letter from path (e.g., "C:\..." -> 'C')
                let drive = if path.len() >= 2 && path.chars().nth(1) == Some(':') {
                    path.chars().next().unwrap().to_ascii_uppercase()
                } else {
                    unknown_drive += 1;
                    '?'
                };

                let is_dir = dir_flag == "1";
                let entry = drive_stats.entry(drive).or_insert((0, 0));
                if is_dir {
                    entry.1 += 1;
                } else {
                    entry.0 += 1;
                }
            }
            Err(e) => {
                parse_errors += 1;
                if parse_errors <= 5 {
                    eprintln!("  Parse error at row {}: {}", total_rows + 1, e);
                }
            }
        }
    }

    let elapsed = start.elapsed();
    
    println!("═══════════════════════════════════════════════════════════════");
    println!("  RESULTS");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("Processing time: {:.2}s", elapsed.as_secs_f64());
    println!("Total rows:      {}", total_rows);
    println!("Parse errors:    {}", parse_errors);
    println!("Unknown drive:   {}", unknown_drive);
    println!();
    
    println!("{:>8} {:>15} {:>15} {:>15}", "Drive", "Files", "Directories", "Total");
    println!("{:-<8} {:-<15} {:-<15} {:-<15}", "", "", "", "");
    
    let mut drives: Vec<_> = drive_stats.keys().collect();
    drives.sort();
    
    let mut grand_files: u64 = 0;
    let mut grand_dirs: u64 = 0;
    
    for drive in &drives {
        let (files, dirs) = drive_stats[drive];
        grand_files += files;
        grand_dirs += dirs;
        println!("{:>8} {:>15} {:>15} {:>15}", 
            format!("{}:", drive), 
            files, 
            dirs,
            files + dirs);
    }
    
    println!("{:-<8} {:-<15} {:-<15} {:-<15}", "", "", "", "");
    println!("{:>8} {:>15} {:>15} {:>15}", "TOTAL", grand_files, grand_dirs, grand_files + grand_dirs);
    println!();
    
    // Output in a format easy to compare
    println!("═══════════════════════════════════════════════════════════════");
    println!("  SUMMARY (for comparison with Rust)");
    println!("═══════════════════════════════════════════════════════════════");
    for drive in drives {
        let (files, dirs) = drive_stats[drive];
        println!("{}:  files={}, dirs={}, total={}", drive, files, dirs, files + dirs);
    }
    println!("---");
    println!("GRAND TOTAL: files={}, dirs={}, total={}", grand_files, grand_dirs, grand_files + grand_dirs);
}
