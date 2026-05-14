# uffs-mft

**Direct NTFS Master File Table (MFT) reader for Windows.**

This crate provides ultra-fast, low-level access to the NTFS MFT, bypassing Windows file enumeration APIs entirely. It reads raw disk sectors and parses MFT records in parallel, delivering multi-gigabyte-per-second indexing on fast NVMe volumes while remaining efficient on SSDs and HDDs.

## 🚀 Performance

Track each drive against a saved golden baseline or the previous UFFS release for the same hardware and flags.

| Drive Type | Golden Baseline | Current | Delta |
|------------|-----------------|---------|-------|
| **NVMe C:** | 2.5s | 2.2s | **11% faster** |
| **NVMe F:** | 1.4s | 1.2s | **19% faster** |
| **HDD S:** | 41s | 41s | Same (I/O bound) |

### Key Optimizations

- **M1: Adaptive Concurrency** - 32 I/O ops for NVMe, 8 for SSD, 2 for HDD
- **M2: Large I/O Chunks** - 4MB for NVMe, 2MB for SSD, 1MB for HDD
- **M3: Parallel Parsing** - 24 workers on 24-core CPU
- **M4: Multi-Volume IOCP** - Single IOCP for multiple drives
- **M5: USN Journal** - Sub-second incremental updates
- **P1: Precise Chunks** - Skip unused MFT regions (30-50% less I/O)
- **P2: Direct I/O** - Each chunk = one I/O operation
- **P3: Zero-Copy** - In-place fixup, no per-record allocation

## Features

- **Direct disk access** - Reads raw clusters, bypassing filesystem overhead
- **Bitmap optimization** - Skips free/unused records (30-50% of MFT)
- **Parallel parsing** - Uses Rayon to parse records across all CPU cores
- **Drive type detection** - Auto-detects NVMe/SSD/HDD for optimal settings
- **Incremental updates** - USN Journal integration for sub-second refreshes
- **Multi-volume support** - Index multiple drives in parallel
- **Caching** - Persistent index cache with TTL-based freshness
- **Polars DataFrame output** - Columnar format with SIMD operations

## Binary: `uffs-mft`

The crate includes a standalone binary for MFT operations:

```bash
# Build the binary
cargo build --release -p uffs-mft

# The binary will be at: target/release/uffs-mft.exe
```

## Quick Start

```bash
# List available NTFS drives
uffs-mft drives

# Get MFT info for a drive (fast, ~10ms)
uffs-mft info --drive C

# Build and cache index (recommended for repeated use)
uffs-mft index-update --drive C

# Incremental update (sub-second after initial build)
uffs-mft index-update --drive C

# Export to Parquet for analysis
uffs-mft read --drive C --output mft.parquet
```

## Global Options

```bash
# Enable verbose output (shows detailed logging)
uffs-mft -v <command>

# Or use environment variable for fine-grained control
RUST_LOG=info uffs-mft <command>
RUST_LOG=debug uffs-mft <command>
```

## Commands Reference

### Core Commands

#### `drives` - List NTFS Drives

```bash
uffs-mft drives
```

Lists all available NTFS drives with size and estimated MFT records.

#### `info` - Show MFT Information

```bash
# Quick info (~10ms)
uffs-mft info --drive C

# Deep scan with file statistics (~2-10s)
uffs-mft info --drive C --deep

# Show unique FRS only (no hard link expansion)
uffs-mft info --drive C --deep --unique
```

**Options:**
- `--deep` - Read all MFT records for detailed statistics
- `--no-bitmap` - Disable bitmap optimization (read all records)
- `--unique` - Show unique FRS count (no hard link expansion)

#### `read` - Export MFT to Parquet

```bash
# Export to Parquet (recommended)
uffs-mft read --drive C --output mft.parquet

# Export to CSV
uffs-mft read --drive C --output mft.csv

# Full mode with extension record merging
uffs-mft read --drive C --output mft.parquet --full
```

**Options:**
- `--output <FILE>` - Output file (.parquet or .csv)
- `--full` - Merge extension records for complete data
- `--mode <MODE>` - Read mode: `auto`, `parallel`, `streaming`, `prefetch`

---

### Indexing Commands (Recommended for Production)

These commands use the optimized lean index format with caching.

#### `index-update` - Build or Update Index (⭐ Primary Command)

```bash
# Build index (or update incrementally if cache exists)
uffs-mft index-update --drive C

# Force full rebuild (ignore cache)
uffs-mft index-update --drive C --force-full

# Custom TTL (default: 600 seconds = 10 minutes)
uffs-mft index-update --drive C --ttl 3600
```

**How it works:**
1. Checks for cached index (default location: `%TEMP%\uffs_index_cache\`)
2. If cache is fresh and USN journal is valid → applies incremental changes (~0.8s)
3. If cache is stale/missing → performs full MFT scan (~2.2s for NVMe)
4. If volume is read-only → uses cached index directly (instant)

**This is the recommended command for production use.**

#### `index-all` - Index All NTFS Drives

```bash
# Index all NTFS drives (uses cache + USN updates)
uffs-mft index-all

# Force fresh rebuild (ignore cache)
uffs-mft index-all --no-cache

# Custom TTL
uffs-mft index-all --ttl 300
```

**How it works:**
1. Detects all NTFS drives automatically
2. Reads indices in parallel (one thread per drive)
3. For each drive: loads from cache if fresh, applies USN changes, or rebuilds if stale
4. Returns combined statistics for all drives

**Performance:**
- First run (cold): ~40s for 23M entries across 7 drives
- Subsequent runs (cache + USN): ~1s for the same 23M entries

#### `cache-status` - Show Cache Status

```bash
# Show status for all cached drives
uffs-mft cache-status

# Show status for specific drive
uffs-mft cache-status --drive C
```

Shows cache age, record count, and freshness status.

#### `cache-get` - Get or Refresh Cache

```bash
# Get cached index (refresh if stale)
uffs-mft cache-get --drive C

# Force refresh
uffs-mft cache-get --drive C --force
```

#### `cache-clear` - Clear Cached Indices

```bash
# Clear all cached indices
uffs-mft cache-clear

# Clear specific drive
uffs-mft cache-clear --drive C
```

#### `index-save` / `index-load` - Manual Index Management

```bash
# Save index to custom location
uffs-mft index-save --drive C --output my_index.uffs

# Load and inspect saved index
uffs-mft index-load --input my_index.uffs
```

---

### USN Journal Commands (M5 Optimization)

#### `usn-info` - Query USN Journal

```bash
uffs-mft usn-info --drive C
```

Shows journal ID, first/next USN, and estimated change count.

#### `usn-read` - Read Recent Changes

```bash
# Read last 100 changes
uffs-mft usn-read --drive C --limit 100

# Read changes since specific USN
uffs-mft usn-read --drive C --since 1234567890
```

---

### Benchmark Commands

#### `benchmark-index-lean` - Lean Index Benchmark (⭐ Recommended)

```bash
# Basic benchmark (auto-detects drive type and uses optimal settings)
uffs-mft benchmark-index-lean --drive C

# With custom concurrency (override auto-detection)
uffs-mft benchmark-index-lean --drive C --concurrency 64

# With custom I/O size
uffs-mft benchmark-index-lean --drive C --io-size-kb 8192

# Force parallel parsing (default: auto based on drive type)
uffs-mft benchmark-index-lean --drive C --parallel-parse

# Disable bitmap optimization (read entire MFT)
uffs-mft benchmark-index-lean --drive C --no-bitmap

# Disable placeholder creation (saves ~15% CPU)
uffs-mft benchmark-index-lean --drive C --no-placeholders
```

**Use this to capture a golden baseline for a drive/configuration or compare a new run with the previous UFFS release.** Measures only MFT read + parse + index build, without cache save overhead.

**Options:**
- `--mode <MODE>` - Read mode: `auto`, `sliding-iocp-inline`, `pipelined`, `pipelined-parallel`
- `--concurrency <N>` - I/O ops in flight (default: 32 NVMe, 8 SSD, 2 HDD)
- `--io-size-kb <KB>` - I/O chunk size (default: 4096 NVMe, 2048 SSD, 1024 HDD)
- `--parallel-parse` - Enable parallel parsing workers
- `--parse-workers <N>` - Number of parsing threads (default: CPU cores)
- `--no-bitmap` - Disable bitmap optimization (read all records)
- `--no-placeholders` - Disable placeholder creation

| Metric | Golden Baseline | Current Example |
|--------|-----------------|-----------------|
| Time | 2.5s | 2.2s |
| Speed | 1826 MB/s | 2042 MB/s |

#### `benchmark-index` - Full Index Benchmark

```bash
uffs-mft benchmark-index --drive C
```

Useful for comparing a current full-index run with saved benchmark artifacts. Includes DataFrame overhead.

#### `benchmark-mft` - Raw MFT Read Benchmark

```bash
uffs-mft benchmark-mft --drive C
```

Measures raw MFT reading speed without parsing so you can track low-level read regressions against saved baselines.

#### `benchmark-multi-volume` - Multi-Volume IOCP Benchmark

```bash
# Benchmark two NVMe drives in parallel
uffs-mft benchmark-multi-volume --drives C,F

# Benchmark all drives
uffs-mft benchmark-multi-volume --drives C,F,S
```

Tests M4 optimization: single IOCP handling multiple volumes simultaneously.

#### `bench` - Detailed Phase Timing

```bash
# Basic benchmark with phase breakdown
uffs-mft bench --drive C

# Multiple runs for averaging
uffs-mft bench --drive C --runs 3

# JSON output
uffs-mft bench --drive C --json

# Skip DataFrame building (measure I/O + parse only)
uffs-mft bench --drive C --no-df
```

**Options:**
- `--runs <N>` - Number of runs for averaging
- `--json` - Output as JSON
- `--no-df` - Skip DataFrame building
- `--mode <MODE>` - Read mode
- `--full` - Merge extension records

#### `bench-all` - Benchmark All Drives

```bash
uffs-mft bench-all --output results.json --runs 3
```

---

### Diagnostic Commands

#### `bitmap-diag` - MFT Bitmap Diagnostics

```bash
uffs-mft bitmap-diag --drive C
```

Analyzes MFT bitmap to show in-use vs free record distribution.

#### `save` / `load` - Offline Analysis

```bash
# Save raw MFT bytes for offline analysis (compressed)
uffs-mft save --drive C -o mft_raw.bin

# Load and export to CSV (37 columns)
uffs-mft load mft_raw.bin -o mft.csv

# Load and export to Parquet
uffs-mft load mft_raw.bin -o mft.parquet

# Build index only (show tree metrics, no export)
uffs-mft load mft_raw.bin --build-index
```

Useful for debugging or analyzing MFT from another machine.

**Compression:** The `save` command uses LZ4 compression, typically achieving 95-97% space savings (e.g., 20MB MFT → 670KB `.bin` file).

---

### Forensic Mode

The `--forensic` flag enables recovery of deleted, corrupt, and extension records that are normally filtered out.

#### When to Use Forensic Mode

| Use Case | Normal Mode | Forensic Mode |
|----------|-------------|---------------|
| File search | ✅ | ❌ |
| Size analysis | ✅ | ❌ |
| Deleted file recovery | ❌ | ✅ |
| Incident response | ❌ | ✅ |
| MFT corruption analysis | ❌ | ✅ |
| Extension record debugging | ❌ | ✅ |

#### Forensic Mode Commands

```bash
# Export with forensic records (41 columns)
uffs-mft load mft_raw.bin -o mft_forensic.csv --forensic

# Direct from drive with forensic mode
uffs-mft read --drive C --output mft_forensic.parquet --forensic
```

#### Output Columns

| Mode | Columns | Description |
|------|---------|-------------|
| Normal | 37 | Core MFT fields, timestamps, flags, path |
| Forensic | 41 | + `is_deleted`, `is_corrupt`, `is_extension`, `base_frs` |

#### Forensic Columns Explained

| Column | Type | Description |
|--------|------|-------------|
| `is_deleted` | bool | Record marked as deleted (FILE flags bit 0 = 0) |
| `is_corrupt` | bool | Record failed validation (bad signature, fixup, etc.) |
| `is_extension` | bool | Extension record (base_frs ≠ 0) |
| `base_frs` | u64 | FRS of base record (0 for base records) |

#### Example: Deleted File Recovery

```bash
# Save MFT from suspect drive
uffs-mft save --drive G -o evidence.bin

# Export with forensic data
uffs-mft load evidence.bin -o evidence.csv --forensic

# In your analysis tool, filter for deleted files:
# WHERE is_deleted = TRUE AND name LIKE '%.docx'
```

#### Forensic Mode Statistics

Typical forensic mode output includes 10-50% more records than normal mode:

| Record Type | Normal | Forensic |
|-------------|--------|----------|
| Active files/dirs | ✅ | ✅ |
| Deleted (recoverable) | ❌ | ✅ |
| Corrupt records | ❌ | ✅ |
| Extension records | ❌ | ✅ |

**Example (20MB MFT):**
- Normal mode: 15,085 records
- Forensic mode: 20,220 records (+34%)

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

You can verify `uffs-mft` output against built-in Windows tools:

```powershell
# Volume geometry and MFT metadata
fsutil fsinfo ntfsinfo C:

# Fragmentation analysis
defrag C: /A /V
```

### Count Differences Explained

| Metric | uffs-mft | Windows defrag |
|--------|----------|----------------|
| Directories | Higher | Lower |
| Files | Higher | Lower |

**Why?** `uffs-mft` parses **all** MFT records including:
- Deleted file entries (not yet overwritten)
- System metadata files ($MFT, $Bitmap, $LogFile, $Secure, etc.)
- NTFS internal structures

Windows `defrag` counts only **active, movable** user files and folders.

### MFT Fragmentation Note

Windows `defrag /A /V` may report "0 MFT fragments" while `uffs-mft` shows multiple extents. Why the difference? Look at defrag's note:

> *"File fragments larger than 64MB are not included in the fragmentation statistics."* — Windows defrag

Example: Your MFT is 4.44 GB across 28 extents = **~162 MB per extent average**. Since each extent is >64MB, Windows defrag doesn't count them as fragments!

`uffs-mft` uses `FSCTL_GET_RETRIEVAL_POINTERS` which returns the actual physical extent map — it's technically correct that the MFT is spread across 28 non-contiguous disk regions.

**Bottom line:** Both are correct. `uffs-mft` shows the true physical layout, while `defrag` focuses on performance-impacting fragmentation (small fragments that cause excessive disk seeks). Large extents like these don't significantly impact read performance.

## Fast vs Full Mode

`uffs-mft` offers two parsing modes controlled by the `--full` flag:

### Fast Mode (Default)

```bash
uffs-mft read --drive C --output mft.parquet
```

- **~15-25% faster** on SSDs, modest improvement on HDDs
- Skips **extension records** (~1% of files)
- Ideal for file search, size analysis, and most use cases

### Full Mode

```bash
uffs-mft read --drive C --output mft.parquet --full
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

### Latest Results (v0.2.71)

Tested on Windows 11, 24-core CPU, NVMe drives. Compare current runs against saved golden baselines for the same machine and flags.

#### Full Scan (current vs baseline)

| Drive | Type | MFT Size | Baseline | Current | Delta |
|-------|------|----------|----------|---------|-------|
| C: | NVMe | 4547 MB | 2.50s | **2.22s** | **11% faster** |
| F: | NVMe | 4547 MB | 1.44s | **1.16s** | **19% faster** |
| S: | HDD | 11483 MB | 41.6s | 41.0s | Same (I/O bound) |

#### Phase 2.5 Optimizations

| Drive | MFT Size | Bytes Read | Reduction | I/O Ops |
|-------|----------|------------|-----------|---------|
| C: | 4547 MB | 3199 MB | **30% less** | 343 |
| F: | 4547 MB | 2565 MB | **44% less** | 777 |

#### Incremental Updates (M5)

| Operation | Time | Notes |
|-----------|------|-------|
| Initial build | 2.2s | Full MFT scan |
| Incremental update | **0.8s** | USN Journal delta |
| Read-only volume | **0.3s** | Cache load only |

### Phase Breakdown (NVMe C:)

| Phase | Time | % |
|-------|------|---|
| I/O Read | 1870ms | 87% |
| Parse + Merge | 260ms | 12% |
| Overhead | 20ms | 1% |
| **Total** | **2150ms** | 100% |

### Optimization History

| Version | Key Changes |
|---------|-------------|
| v0.1.30 | Baseline |
| v0.1.38 | Bitmap fix, Rayon fold/reduce, prefetch |
| v0.2.0 | SoA layout, fast path |
| v0.2.50 | M1-M5 optimizations (adaptive concurrency, large I/O, parallel parse, multi-volume, USN) |
| **v0.2.71** | **P1-P3 optimizations (precise chunks, direct I/O, zero-copy) - 11-19% faster than the stored baseline** |

## Performance Tuning

### Auto-Detection (Default)

By default, `uffs-mft` auto-detects drive type and uses optimal settings:

| Setting | NVMe | SSD | HDD |
|---------|------|-----|-----|
| **Concurrency** | 32 | 8 | 2 |
| **I/O Size** | 4 MB | 2 MB | 1 MB |
| **Parallel Parse** | Yes | No | No |
| **Bitmap** | Yes | Yes | Optional |

### Manual Tuning

Override auto-detection for experimentation:

```bash
# Maximum concurrency for fast NVMe
uffs-mft benchmark-index-lean --drive C --concurrency 64 --io-size-kb 8192

# Conservative settings for older SSD
uffs-mft benchmark-index-lean --drive D --concurrency 4 --io-size-kb 1024

# HDD: try disabling bitmap (sequential may beat seeking)
uffs-mft benchmark-index-lean --drive S --no-bitmap

# Force parallel parsing with custom worker count
uffs-mft benchmark-index-lean --drive C --parallel-parse --parse-workers 16
```

### Key Tuning Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--concurrency <N>` | I/O operations in flight | Auto (2-32) |
| `--io-size-kb <KB>` | Chunk size in KB | Auto (1024-4096) |
| `--parallel-parse` | Enable parallel parsing | Auto |
| `--parse-workers <N>` | Parsing thread count | CPU cores |
| `--no-bitmap` | Read entire MFT | false |
| `--no-placeholders` | Skip placeholder dirs | false |
| `--mode <MODE>` | Read strategy | auto |

### Read Modes

| Mode | Description | Best For |
|------|-------------|----------|
| `auto` | Auto-select based on drive type | Default |
| `sliding-iocp-inline` | IOCP with inline parsing | NVMe/SSD |
| `pipelined` | I/O+CPU overlap, single-threaded | SSD |
| `pipelined-parallel` | I/O+CPU overlap, multi-core | HDD |

## Requirements

- **Windows only** - Uses Windows APIs for raw disk access
- **Administrator privileges** - Required for direct MFT access
- **Rust 1.85+** - Edition 2024

## License

MPL-2.0 - See [LICENSE](../../LICENSE) for details.

