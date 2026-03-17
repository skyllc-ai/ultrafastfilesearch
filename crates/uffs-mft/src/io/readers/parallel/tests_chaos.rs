//! Deterministic chaos-order test harness for reproducing LIVE parser bugs.
//!
//! This module provides reproducible testing of out-of-order record processing
//! that occurs in Windows LIVE parsing due to:
//! - Overlapped I/O completion order (IOCP can complete chunks out of order)
//! - Parallel rayon parsing (extension records can be processed before base
//!   records)
//!
//! The harness reads an offline MFT, splits it into chunks, reorders them with
//! seeded randomization, and processes them through the same parsing pipeline
//! as LIVE.

use std::path::Path;
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

use crate::index::MftIndex;
use crate::io::chunking::{ReadChunk, generate_read_chunks};
use crate::io::fixup::apply_fixup;
use crate::io::merger::MftRecordMerger;
use crate::io::parser::{ParseResult, parse_record_full};
use crate::raw::{LoadRawOptions, load_raw_mft};

/// Strategy for chunk reordering in chaos mode.
#[derive(Debug, Clone, Copy)]
pub enum ChaosStrategy {
    /// Random shuffle with fixed seed (most realistic).
    Random { seed: u64 },
    /// Reverse order (simple but unrealistic).
    Reverse,
    /// Every other chunk swapped (controlled chaos).
    Interleaved,
    /// Sequential order (baseline for validation).
    Sequential,
}

/// Deterministic chaos-order MFT reader for testing.
///
/// This simulates LIVE parser's out-of-order chunk completion by:
/// 1. Reading offline MFT file
/// 2. Splitting into chunks (like IOCP does)
/// 3. Reordering chunks with controlled strategy
/// 4. Processing through parallel parsing pipeline
pub struct ChaosMftReader {
    strategy: ChaosStrategy,
    chunk_size: usize,
}

impl ChaosMftReader {
    /// Creates a new chaos reader with the given strategy.
    pub const fn new(strategy: ChaosStrategy, chunk_size: usize) -> Self {
        Self {
            strategy,
            chunk_size,
        }
    }

    /// Reads an offline MFT with controlled chaos ordering.
    ///
    /// # Arguments
    ///
    /// * `mft_path` - Path to offline MFT file
    /// * `volume` - Volume letter to use in the index
    ///
    /// # Returns
    ///
    /// Returns the parsed `MftIndex` with records potentially processed
    /// out-of-order.
    ///
    /// # Errors
    ///
    /// Returns an error if the MFT file cannot be read or is invalid.
    #[expect(
        clippy::too_many_lines,
        reason = "test harness orchestration requires sequential setup"
    )]
    pub fn read_with_chaos(&self, mft_path: &Path, volume: char) -> anyhow::Result<MftIndex> {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crossbeam_channel::{Sender, bounded};

        // Load raw MFT data
        let load_options = LoadRawOptions {
            header_only: false,
            volume_letter: Some(volume),
            forensic: false,
        };

        let raw_data = load_raw_mft(mft_path, &load_options)?;
        let header = raw_data.header;
        let mft_bytes = raw_data.data;

        let record_size = header.record_size as usize;
        let total_records = header.record_count as usize;

        // Create extent map (treat as contiguous for offline file)
        use crate::io::extent_map::MftExtentMap;
        let extent_map =
            MftExtentMap::contiguous(0, mft_bytes.len() as u64, record_size as u32, 1024);

        // Generate chunks (no bitmap - read everything)
        let mut chunks: Vec<ReadChunk> =
            generate_read_chunks(&extent_map, None, self.chunk_size);
        chunks.sort_by_key(|c| c.start_frs);

        // Apply chaos strategy
        self.apply_chaos(&mut chunks);

        // Calculate total records to parse
        let estimated_records = total_records;
        let num_workers = std::thread::available_parallelism().map_or(4, |p| p.get());

        tracing::info!(
            total_records,
            chunks = chunks.len(),
            chunk_size_kb = self.chunk_size / 1024,
            num_workers,
            strategy = ?self.strategy,
            "🌀 Starting CHAOS-ORDER parsing"
        );

        // Create channel for buffer handoff
        let channel_capacity = num_workers * 2;
        let (tx, rx): (
            Sender<Option<(Vec<u8>, u64, usize)>>,
            crossbeam_channel::Receiver<Option<(Vec<u8>, u64, usize)>>,
        ) = bounded(channel_capacity);

        // Shared counter for parsed records
        let records_parsed = Arc::new(AtomicUsize::new(0));

        // Spawn worker threads (same as LIVE parallel parser)
        let mut worker_handles = Vec::with_capacity(num_workers);
        let records_per_worker = (estimated_records / num_workers) + 1;

        for worker_id in 0..num_workers {
            let rx = rx.clone();
            let records_parsed = Arc::clone(&records_parsed);

            let handle = std::thread::spawn(move || {
                let mut results: Vec<ParseResult> = Vec::with_capacity(records_per_worker);
                let mut local_parsed = 0usize;

                // Process buffers until channel closes
                while let Ok(Some((mut buffer, start_frs, record_count))) = rx.recv() {
                    for i in 0..record_count {
                        let frs = start_frs + i as u64;
                        let offset = i * record_size;
                        let end = offset + record_size;
                        if end > buffer.len() {
                            break;
                        }

                        // Apply fixup in-place
                        let record_slice = &mut buffer[offset..end];
                        if !apply_fixup(record_slice) {
                            continue;
                        }

                        // Parse using unified pipeline (same as LIVE)
                        let result = parse_record_full(record_slice, frs);
                        if !matches!(result, ParseResult::Skip) {
                            local_parsed += 1;
                            results.push(result);
                        }
                    }
                }

                records_parsed.fetch_add(local_parsed, Ordering::Relaxed);
                tracing::debug!(worker_id, local_parsed, "Worker complete");
                results
            });

            worker_handles.push(handle);
        }

        // Drop receiver clone so workers can detect channel close
        drop(rx);

        // Send chunks to workers in chaos order
        let start_time = std::time::Instant::now();
        let mut bytes_sent = 0u64;

        for chunk in chunks {
            let skip_begin_bytes = chunk.skip_begin as usize * record_size;
            let effective_records = chunk.record_count - chunk.skip_begin - chunk.skip_end;
            if effective_records == 0 {
                continue;
            }

            let chunk_bytes = effective_records as usize * record_size;
            let start_frs = chunk.start_frs + chunk.skip_begin;

            // Calculate byte offset in the MFT file
            // For contiguous offline MFT, disk_offset is just FRS * record_size
            let byte_offset = start_frs as usize * record_size;
            let end_offset = byte_offset + chunk_bytes;

            if end_offset > mft_bytes.len() {
                tracing::warn!(
                    start_frs,
                    chunk_bytes,
                    byte_offset,
                    mft_len = mft_bytes.len(),
                    "Chunk exceeds MFT bounds, skipping"
                );
                continue;
            }

            // Extract chunk data
            let buffer_data = mft_bytes[byte_offset..end_offset].to_vec();
            let record_count = chunk_bytes / record_size;

            if tx
                .send(Some((buffer_data, start_frs, record_count)))
                .is_err()
            {
                tracing::warn!("Failed to send buffer to workers - channel closed");
                break;
            }

            bytes_sent += chunk_bytes as u64;
        }

        let send_ms = start_time.elapsed().as_millis();
        tracing::info!(send_ms, bytes_mb = bytes_sent / (1024 * 1024), "✅ Chunk dispatch complete");

        // Signal workers to stop
        for _ in 0..num_workers {
            let _ = tx.send(None);
        }
        drop(tx);

        // Collect results and merge (same as LIVE)
        let merge_start = std::time::Instant::now();
        let mut merger = MftRecordMerger::with_capacity(total_records);

        for handle in worker_handles {
            match handle.join() {
                Ok(results) => {
                    for result in results {
                        merger.add_result(result);
                    }
                }
                Err(e) => {
                    tracing::warn!("Worker thread panicked: {:?}", e);
                }
            }
        }

        let total_parsed = records_parsed.load(Ordering::Relaxed);

        // Build index from merged records
        let parsed_records = merger.merge();
        let index = MftIndex::from_parsed_records(volume, parsed_records);

        let merge_ms = merge_start.elapsed().as_millis();
        let total_ms = start_time.elapsed().as_millis();

        tracing::info!(
            total_ms,
            send_ms,
            merge_ms,
            records_parsed = total_parsed,
            index_entries = index.records.len(),
            "✅ CHAOS-ORDER parsing complete"
        );

        Ok(index)
    }

    /// Applies the chaos strategy to reorder chunks.
    fn apply_chaos(&self, chunks: &mut [ReadChunk]) {
        match self.strategy {
            ChaosStrategy::Random { seed } => {
                let mut rng = ChaCha8Rng::seed_from_u64(seed);
                chunks.shuffle(&mut rng);
            }
            ChaosStrategy::Reverse => {
                chunks.reverse();
            }
            ChaosStrategy::Interleaved => {
                // Swap every other chunk with the next one
                for i in (0..chunks.len() - 1).step_by(2) {
                    chunks.swap(i, i + 1);
                }
            }
            ChaosStrategy::Sequential => {
                // No reordering - chunks remain in sequential order
            }
        }
    }
}

/// Tests chaos-order parsing against the offline D: drive MFT.
///
/// This test is intentionally ignored because it:
/// - Requires a specific offline MFT file at a known path
/// - Is slow (processes 7M+ records)
/// - Is diagnostic/investigative rather than regression-preventive
///
/// Computes sorted SHA256 hash of CSV lines (for full-field validation).
///
/// This matches the verification strategy used in `scripts/verify_parity.rs`:
/// - Sort lines using byte-level comparison for cross-platform consistency
/// - Hash each line with trailing newline (NOT join with \n)
/// - This ensures ALL fields match, not just a subset
#[cfg(test)]
fn sorted_sha256(lines: &[String]) -> String {
    let mut indexed: Vec<(usize, &str)> = lines.iter().map(String::as_str).enumerate().collect();
    // Stable sort with byte-level comparison for cross-platform consistency
    indexed.sort_by(|(idx_a, a), (idx_b, b)| {
        match a.as_bytes().cmp(b.as_bytes()) {
            std::cmp::Ordering::Equal => idx_a.cmp(idx_b),
            other => other,
        }
    });
    sha256_for_lines(indexed.into_iter().map(|(_, s)| s))
}

/// Computes SHA256 hash of lines (helper for sorted and unsorted hashing).
///
/// This matches `scripts/verify_parity.rs:1229-1236` exactly.
#[cfg(test)]
fn sha256_for_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

/// Run with: `cargo test -p uffs-mft -- chaos_order --nocapture --ignored`
#[test]
#[ignore = "requires offline MFT at /Users/rnio/uffs_data/drive_d/D_mft.bin"]
fn test_chaos_order_d_drive() {
    use std::path::PathBuf;

    // Initialize logging for diagnostics
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();

    let mft_path = PathBuf::from("/Users/rnio/uffs_data/drive_d/D_mft.bin");
    if !mft_path.exists() {
        eprintln!("⚠️  Offline MFT not found at: {}", mft_path.display());
        eprintln!("   This test requires the offline D: drive MFT.");
        panic!("Test skipped: offline MFT not found");
    }

    println!("\n═══════════════════════════════════════════════════════");
    println!("     CHAOS-ORDER SHA256 VALIDATION TEST");
    println!("     (Full-field parity with C++ ground truth)");
    println!("═══════════════════════════════════════════════════════\n");

    // Step 1: Export chaos-order results using CLI with --chaos-seed
    println!("📤 Step 1: Exporting chaos-order results to CSV format via CLI");
    println!("   (Uses --chaos-seed 42 to randomize chunk processing order)");
    let temp_output = std::env::temp_dir().join("chaos_d.txt");

    let status = std::process::Command::new("cargo")
        .args(&["run", "--release", "-p", "uffs-cli", "--bin", "uffs", "--"])
        .args(&["*"])
        .args(&["--mft-file", mft_path.to_str().expect("valid path")])
        .args(&["--drive", "D"])
        .args(&["--chaos-seed", "42"])
        .args(&["--tz-offset", "-8"])
        .args(&["--format", "custom"])
        .args(&["--out", temp_output.to_str().expect("valid path")])
        .status()
        .expect("Failed to run uffs CLI");

    assert!(status.success(), "uffs CLI failed with status: {}", status);
    println!("   ✓ CSV export completed");
    println!();

    // Step 2: Compute sorted SHA256 of chaos output
    println!("🔐 Step 2: Computing sorted SHA256 of chaos-order output");
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let chaos_lines: Vec<String> = BufReader::new(File::open(&temp_output).expect("temp file exists"))
        .lines()
        .collect::<Result<_, _>>()
        .expect("read lines");

    let chaos_sha = sorted_sha256(&chaos_lines);
    println!("   Chaos SHA256:    {}", chaos_sha);
    println!();

    // Step 3: Export sequential output for comparison
    println!("📤 Step 3: Exporting sequential-order output for baseline comparison");
    let sequential_output = std::env::temp_dir().join("sequential_d.txt");

    let status = std::process::Command::new("cargo")
        .args(&["run", "--release", "-p", "uffs-cli", "--bin", "uffs", "--"])
        .args(&["*"])
        .args(&["--mft-file", mft_path.to_str().expect("valid path")])
        .args(&["--drive", "D"])
        .args(&["--tz-offset", "-8"])
        .args(&["--format", "custom"])
        .args(&["--out", sequential_output.to_str().expect("valid path")])
        .status()
        .expect("Failed to run uffs CLI for sequential export");

    assert!(status.success(), "Sequential uffs CLI failed with status: {}", status);
    println!("   ✓ Sequential CSV export completed");
    println!();

    // Step 4: Compute SHA256 of sequential output
    println!("🔐 Step 4: Computing sorted SHA256 of sequential-order output");
    let sequential_lines: Vec<String> = BufReader::new(File::open(&sequential_output).expect("sequential file exists"))
        .lines()
        .collect::<Result<_, _>>()
        .expect("read sequential lines");

    let sequential_sha = sorted_sha256(&sequential_lines);
    println!("   Sequential SHA256: {}", sequential_sha);
    println!();

    // Step 5: Compare against C++ ground truth (sorted comparison, like verify_parity.rs)
    println!("✅ Step 5: Validating against C++ ground truth (sorted SHA256)");
    const EXPECTED_SORTED_SHA: &str = "028356d4c9298ca8ef790229f4d4270ea29827ad155051e01181181fa34a531e";
    println!("   Expected sorted SHA: {}", EXPECTED_SORTED_SHA);
    println!("   Sequential sorted:   {}", sequential_sha);
    println!("   Chaos sorted:        {}", chaos_sha);
    println!();

    // Verify sequential matches ground truth (sorted comparison)
    // Note: Row order may differ (C++ vs Rust MFT/tree walk), but content must match
    if sequential_sha != EXPECTED_SORTED_SHA {
        println!("❌ SEQUENTIAL SHA256 MISMATCH!");
        println!();
        println!("   This indicates a critical bug in the base parser or output format.");
        println!("   Line counts:");
        println!("     Expected:   7,065,330 lines");
        println!("     Sequential: {} lines", sequential_lines.len());
        panic!("Sequential sorted SHA256 mismatch! Expected: {}, Got: {}", EXPECTED_SORTED_SHA, sequential_sha);
    }

    // Verify chaos matches sequential (sorted comparison)
    if chaos_sha != sequential_sha {
        println!("❌ CHAOS-ORDER SHA256 MISMATCH!");
        println!();
        println!("   This indicates the directory index merge fix is broken for out-of-order processing.");
        println!("   Line counts:");
        println!("     Sequential: {} lines", sequential_lines.len());
        println!("     Chaos:      {} lines", chaos_lines.len());
        println!();

        // Show first few differences using verify_parity.rs-style diagnostics
        println!("   📊 Comparing sorted outputs (first 10 differences):");
        let mut sequential_sorted = sequential_lines.clone();
        let mut chaos_sorted = chaos_lines.clone();
        sequential_sorted.sort_unstable();
        chaos_sorted.sort_unstable();

        let n = sequential_sorted.len().min(chaos_sorted.len());
        let mut diff_count = 0;
        for i in 0..n {
            if sequential_sorted[i] != chaos_sorted[i] && diff_count < 10 {
                println!("      Line {} differs:", i + 1);
                println!("        Sequential: {}", &sequential_sorted[i][..sequential_sorted[i].len().min(100)]);
                println!("        Chaos:      {}", &chaos_sorted[i][..chaos_sorted[i].len().min(100)]);
                diff_count += 1;
            }
        }

        panic!("Chaos-order SHA256 mismatch! Expected: {}, Got: {}", sequential_sha, chaos_sha);
    }

    assert_eq!(chaos_sha, EXPECTED_SORTED_SHA,
        "Chaos-order SHA256 mismatch! This indicates the directory index merge fix is broken.");

    println!();
    println!("✅ VALIDATION PASSED!");
    println!("   Chaos-order output matches C++ ground truth exactly (all fields, all records).");
    println!("   This proves 100% parity - not just 4 fields!");
}

/// Tests reverse-order parsing (simpler chaos strategy).
#[test]
#[ignore = "requires offline MFT at /Users/rnio/uffs_data/drive_d/D_mft.bin"]
fn test_reverse_order_d_drive() {
    use std::path::PathBuf;

    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();

    let mft_path = PathBuf::from("/Users/rnio/uffs_data/drive_d/D_mft.bin");
    if !mft_path.exists() {
        panic!("Test skipped: offline MFT not found at {}", mft_path.display());
    }

    let chaos_reader = ChaosMftReader::new(
        ChaosStrategy::Reverse,
        2 * 1024 * 1024,
    );

    let result = chaos_reader.read_with_chaos(&mft_path, 'D');

    match result {
        Ok(index) => {
            println!("\n✅ REVERSE-ORDER parsing completed");
            println!("   Total records: {}", index.records.len());
        }
        Err(e) => {
            eprintln!("\n❌ REVERSE-ORDER parsing FAILED: {e:?}");
            panic!("Reverse-order test failed");
        }
    }
}

/// Tests interleaved chunk order (controlled chaos).
#[test]
#[ignore = "requires offline MFT at /Users/rnio/uffs_data/drive_d/D_mft.bin"]
fn test_interleaved_order_d_drive() {
    use std::path::PathBuf;

    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();

    let mft_path = PathBuf::from("/Users/rnio/uffs_data/drive_d/D_mft.bin");
    if !mft_path.exists() {
        panic!("Test skipped: offline MFT not found at {}", mft_path.display());
    }

    let chaos_reader = ChaosMftReader::new(
        ChaosStrategy::Interleaved,
        2 * 1024 * 1024,
    );

    let result = chaos_reader.read_with_chaos(&mft_path, 'D');

    match result {
        Ok(index) => {
            println!("\n✅ INTERLEAVED-ORDER parsing completed");
            println!("   Total records: {}", index.records.len());
        }
        Err(e) => {
            eprintln!("\n❌ INTERLEAVED-ORDER parsing FAILED: {e:?}");
            panic!("Interleaved-order test failed");
        }
    }
}
