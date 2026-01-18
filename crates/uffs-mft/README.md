# uffs-mft

**Direct NTFS Master File Table (MFT) reader for Windows.**

This crate provides ultra-fast, low-level access to the NTFS MFT, bypassing Windows file enumeration APIs entirely. It reads raw disk sectors and parses MFT records in parallel, outputting a Polars DataFrame ready for analysis.

## 🚀 Performance (v0.2.0)

| Drive Type | Throughput | Example |
|------------|------------|---------|
| **SSD** | **1,472-1,839 MB/s** | 1.77M records in 3.1s |
| **HDD** | **206-250 MB/s** | 7.18M records in 45.9s |

**55% faster** than v0.1.30 baseline. See [Benchmarks](#benchmarks) for details.

## Features

- **Direct disk access** - Reads raw clusters, bypassing filesystem overhead
- **Bitmap optimization** - Skips free/unused records (often 10-30% of MFT)
- **Parallel parsing** - Uses Rayon to parse records across all CPU cores
- **SoA layout** - Struct-of-Arrays for optimal DataFrame building
- **Fast/Full modes** - Skip extension records for speed, or merge for completeness
- **SSD/HDD auto-tuning** - Optimal chunk sizes per drive type
- **Polars DataFrame output** - Columnar format with SIMD operations
- **Comprehensive logging** - Rich tracing output for debugging and analysis

## Binary: `uffs_mft`

The crate includes a standalone binary for MFT operations:

```bash
# Build the binary
cargo build --release -p uffs-mft

# The binary will be at: target/release/uffs_mft.exe
```

### Commands

#### `info` - Show MFT Information (Fast)

Displays volume geometry and MFT metadata without reading all records (~10ms):

```bash
uffs_mft info --drive C
```

Output:
```
═══════════════════════════════════════════════════════════════
                    MFT INFO (Lightweight)
                    Drive: C:
═══════════════════════════════════════════════════════════════

📐 VOLUME GEOMETRY
  Bytes per sector:     512
  Bytes per cluster:    4096
  Bytes per MFT record: 1024
  Total clusters:       244190646
  Volume size:          931.51 GB

📁 MFT STRUCTURE
  MFT start LCN:        786432
  MFT size:             512.00 MB
  MFT % of volume:      0.054%
  Total records:        524288
  In-use records:       450000
  Free records:         74288
  Utilization:          85.8%
  Fragmentation:        1 extent(s) ✅

✅ HEALTH STATUS: Good (based on metadata)

💡 TIP: Use --deep for detailed file statistics.

⏱️  Completed in 8.2ms
═══════════════════════════════════════════════════════════════
```

#### `info --deep` - Full MFT Analysis

Reads and parses all MFT records for comprehensive statistics (~10-30s):

```bash
uffs_mft info --drive C --deep
```

Additional output with `--deep`:
```
📊 DEEP SCAN: Reading all MFT records...

📊 FILE SYSTEM STATISTICS
  Parsed records:       450000
  Directories:          50000
  Files:                400000

🏷️  ATTRIBUTE FLAGS
  Hidden:               1200
  System:               500
  Read-only:            150
  Archive:              380000
  Compressed:           100
  Encrypted:            50
  Sparse:               20
  Reparse points:       10

🔗 EXTENDED ATTRIBUTES
  Files with ADS:       25 (Alternate Data Streams)
  Files with hardlinks: 150

💾 STORAGE ANALYSIS
  Total file size:      450.25 GB
  Total allocated:      465.50 GB
  Slack space:          15616.00 MB (3.3%)

⏱️  Deep scan completed in 12.45s
```

#### `read` - Export MFT to Parquet

Reads the MFT and exports to a Parquet file:

```bash
# Fast mode (default) - maximum speed, skips extension records
uffs_mft read --drive C --output mft.parquet

# Full mode - complete data including extension records
uffs_mft read --drive C --output mft.parquet --full

# Specify read mode (auto, parallel, streaming, prefetch)
uffs_mft read --drive C --output mft.parquet --mode prefetch
```

**Options:**
- `--mode <MODE>` - Read mode: `auto` (default), `parallel`, `streaming`, `prefetch`
- `--full` - Merge extension records for complete data (slower, see [Fast vs Full Mode](#fast-vs-full-mode))

The output Parquet contains all file metadata:
- `frs`, `parent_frs` - File Record Segment numbers
- `name` - Primary file name
- `size`, `allocated_size` - File sizes
- `created`, `modified`, `accessed`, `mft_modified` - Timestamps
- `is_directory`, `is_hidden`, `is_system`, `is_compressed`, etc. - Flags

#### `bench` - Benchmark Single Drive

Benchmark MFT reading with detailed phase timing:

```bash
# Basic benchmark
uffs_mft bench --drive C

# Multiple runs for averaging
uffs_mft bench --drive C --runs 3

# JSON output for scripting
uffs_mft bench --drive C --json

# Compare fast vs full mode
uffs_mft bench --drive C          # Fast (default)
uffs_mft bench --drive C --full   # Full (with extension merging)
```

**Options:**
- `--runs <N>` - Number of runs for averaging (default: 1)
- `--json` - Output results as JSON
- `--no-df` - Skip DataFrame building (measure I/O + parse only)
- `--mode <MODE>` - Read mode: `auto`, `parallel`, `streaming`, `prefetch`
- `--full` - Merge extension records (slower)

#### `bench-all` - Benchmark All Drives

Benchmark all NTFS drives and save results to JSON:

```bash
# Benchmark all drives
uffs_mft bench-all

# Save to specific file
uffs_mft bench-all --output benchmark_results.json

# Multiple runs per drive
uffs_mft bench-all --runs 3

# Compare fast vs full mode
uffs_mft bench-all --output fast.json
uffs_mft bench-all --output full.json --full
```

#### `drives` - List NTFS Drives

```bash
uffs_mft drives
```

Output:
```
NTFS drives:
  C: (931.5 GB, ~524288 MFT records)
  D: (1863.0 GB, ~1048576 MFT records)
```

## Library Usage

```rust
use uffs_mft::MftReader;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Open MFT reader for C: drive
    let reader = MftReader::open('C').await?;
    
    // Read all records into a Polars DataFrame
    let df = reader.read_all().await?;
    
    println!("Read {} records", df.height());
    println!("{}", df.head(Some(10)));
    
    Ok(())
}
```

## MFT Data Levels

| Level | What it is | Size | Speed |
|-------|------------|------|-------|
| **Boot Sector** | Volume geometry | 512 bytes | Instant |
| **$MFT Record 0** | MFT extent map | 1 KB | Instant |
| **$MFT Bitmap** | In-use record flags | ~64 KB | <10ms |
| **Full MFT** | All file records | 500 MB - 5 GB | 5-30s |

## Comparison with Windows Tools

You can verify `uffs_mft` output against built-in Windows tools:

```powershell
# Volume geometry and MFT metadata
fsutil fsinfo ntfsinfo C:

# Fragmentation analysis
defrag C: /A /V
```

### Count Differences Explained

| Metric | uffs_mft | Windows defrag |
|--------|----------|----------------|
| Directories | Higher | Lower |
| Files | Higher | Lower |

**Why?** `uffs_mft` parses **all** MFT records including:
- Deleted file entries (not yet overwritten)
- System metadata files ($MFT, $Bitmap, $LogFile, $Secure, etc.)
- NTFS internal structures

Windows `defrag` counts only **active, movable** user files and folders.

### MFT Fragmentation Note

Windows `defrag /A /V` may report "0 MFT fragments" while `uffs_mft` shows multiple extents. Why the difference? Look at defrag's note:

> *"File fragments larger than 64MB are not included in the fragmentation statistics."* — Windows defrag

Example: Your MFT is 4.44 GB across 28 extents = **~162 MB per extent average**. Since each extent is >64MB, Windows defrag doesn't count them as fragments!

`uffs_mft` uses `FSCTL_GET_RETRIEVAL_POINTERS` which returns the actual physical extent map — it's technically correct that the MFT is spread across 28 non-contiguous disk regions.

**Bottom line:** Both are correct. `uffs_mft` shows the true physical layout, while `defrag` focuses on performance-impacting fragmentation (small fragments that cause excessive disk seeks). Large extents like these don't significantly impact read performance.

## Fast vs Full Mode

`uffs_mft` offers two parsing modes controlled by the `--full` flag:

### Fast Mode (Default)

```bash
uffs_mft read --drive C --output mft.parquet
```

- **~15-25% faster** on SSDs, modest improvement on HDDs
- Skips **extension records** (~1% of files)
- Ideal for file search, size analysis, and most use cases

### Full Mode

```bash
uffs_mft read --drive C --output mft.parquet --full
```

- Complete data for all files
- Merges extension record attributes into base records
- Required when you need complete hard link or ADS data

### What Are Extension Records?

When a file has too many attributes to fit in a single 1KB MFT record, NTFS creates **extension records** to hold the overflow. This happens for files with:

- **Many hard links** (>3-4 names pointing to the same file)
- **Many Alternate Data Streams** (ADS)
- **Very long file names** or many named attributes

Extension records are rare (~1% of files on typical systems).

### What's Skipped in Fast Mode?

| Data | Fast Mode | Full Mode |
|------|-----------|-----------|
| Primary file name | ✅ Captured | ✅ Captured |
| Primary data stream | ✅ Captured | ✅ Captured |
| File size, timestamps, flags | ✅ Captured | ✅ Captured |
| Additional hard link names | ❌ Skipped | ✅ Merged |
| Additional ADS streams | ❌ Skipped | ✅ Merged |

**For file search and size analysis, fast mode provides complete data.** Only use `--full` if you specifically need complete hard link enumeration or ADS analysis.

## Benchmarks

### v0.2.0 Results (Fast Mode)

Tested on Windows 11, 24-core CPU:

| Drive | Type | Records | Time | Throughput |
|-------|------|---------|------|------------|
| C: | SSD | 1.77M | **3.1s** | **1,472 MB/s** |
| F: | SSD | 1.53M | **2.5s** | **1,839 MB/s** |
| D: | HDD | 3.81M | **23.3s** | **206 MB/s** |
| S: | HDD | 7.18M | **45.9s** | **250 MB/s** |

### Phase Breakdown (SSD C:)

| Phase | Time | % |
|-------|------|---|
| Read | 813ms | 26% |
| Parse | 1,356ms | 44% |
| Merge | 542ms | 18% |
| DF Build | 185ms | 6% |
| **Total** | **3,090ms** | 100% |

### Fast vs Full Mode Comparison

| Drive | Fast | Full | Difference |
|-------|------|------|------------|
| SSD C: | 3.1s | 5.8s | Fast is 46% faster |
| SSD F: | 2.5s | 4.8s | Fast is 48% faster |
| HDD D: | 23.3s | 30.6s | Fast is 24% faster |
| HDD S: | 45.9s | 62.1s | Fast is 26% faster |

### Optimization History

| Version | Total (7 drives) | SSD Throughput | Key Changes |
|---------|------------------|----------------|-------------|
| v0.1.30 | 315s | 400-550 MB/s | Baseline |
| v0.1.38 | 173s | 791-942 MB/s | Bitmap fix, Rayon fold/reduce, prefetch |
| **v0.2.0** | **142s** | **1,472-1,839 MB/s** | SoA layout, fast path |

## Requirements

- **Windows only** - Uses Windows APIs for raw disk access
- **Administrator privileges** - Required for direct MFT access
- **Rust 1.85+** - Edition 2024

## License

MPL-2.0 - See [LICENSE](../../LICENSE) for details.

