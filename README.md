# UFFS - Ultra Fast File Search

[![License: MPL 2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

**Ultra-high-performance file search for Windows using direct NTFS MFT reading and Polars DataFrames.**

> 🦀 This is the Rust rewrite of UFFS, replacing the original C++ version with modern, safe, and blazing-fast code.

---

## ⚡ Why UFFS is Lightning Fast

Traditional file search tools (including `os.walk`, `FindFirstFile`, etc.) work like this:

1. Ask the OS to find a file
2. OS reads the **entire MFT** (Master File Table) - the "phonebook" of all files
3. Returns info for **one file**
4. **Throws away the MFT**
5. Repeat for the next file 🐌

**UFFS reads the MFT directly** - once - and queries it in memory using Polars DataFrames. This is like reading the entire phonebook once instead of looking up each name individually.

### Benchmark Results (v0.2.51)

| Drive Type | Records | Time | Throughput |
|------------|---------|------|------------|
| **SSD** | 1.77M | **3.1s** | **1,472 MB/s** |
| **SSD** | 1.53M | **2.5s** | **1,839 MB/s** |
| **HDD** | 3.81M | **23.3s** | **206 MB/s** |
| **HDD** | 7.18M | **45.9s** | **250 MB/s** |
| **All 7 drives** | 18.7M | **142s** | - |

| Comparison | Records | Time | Notes |
|------------|---------|------|-------|
| **UFFS v0.2.51** | **18.7 Million** | **~142 seconds** | All disks, fast mode |
| UFFS v0.1.30 | 18.7 Million | ~315 seconds | Baseline |
| Everything | 19 Million | 178 seconds | All disks |
| WizFile | 6.5 Million | 299 seconds | Single HDD |

> **UFFS is 55% faster than v0.1.30 baseline, and achieves ~4x SSD throughput improvement!**

---

## 🚀 Quick Start

### Installation

```bash
# Build from source (requires Rust 1.85+)
cargo build --release

# The binary will be at:
#   Windows: target/release/uffs.exe
#   Linux/macOS: target/release/uffs
```

### Basic Usage

```bash
# Search for all .rs files on C: drive
uffs "*.rs" --drive C

# Search across multiple drives
uffs "*.txt" --drives C,D,E

# Search all drives (default)
uffs "project*"

# Use a pre-built index for instant searches
uffs index --drive C --output c_drive.parquet
uffs search "*.rs" --index c_drive.parquet
```

---

## 📖 Usage Examples

### Simple Search

| Command | Result |
|---------|--------|
| `uffs "c:/pro*"` | Files & folders starting with "pro" on C: |
| `uffs "*.txt"` | All .txt files on ALL drives |
| `uffs "*.txt" --drives C,D,M` | All .txt files on C:, D:, and M: |
| `uffs "project*" --ext rs,toml` | Rust project files |

### Filter Options

```bash
# Files only (no directories)
uffs "*.log" --files-only

# Directories only
uffs "node_modules" --dirs-only

# Size filters
uffs "*.mp4" --min-size 100MB --max-size 4GB

# Limit results
uffs "*.tmp" --limit 100

# Case-sensitive search
uffs "README" --case
```

### Output Options

```bash
# Output to CSV file
uffs "*.rs" --out results.csv

# Custom columns
uffs "*" --columns path,size,created --out files.csv

# Custom separator and quotes
uffs "*" --sep ";" --quotes "'" --out data.csv

# Include/exclude header
uffs "*" --header false --out raw.csv

# JSON output
uffs "*.rs" --format json
```

### Available Columns

| Column | Description |
|--------|-------------|
| `path` | Full path including filename |
| `name` | Filename only |
| `pathonly` | Directory path only |
| `size` | File size in bytes |
| `sizeondisk` | Actual disk space used |
| `created` | Creation timestamp |
| `written` | Last modified timestamp |
| `accessed` | Last accessed timestamp |
| `type` | File type |
| `directory` | Is a directory |
| `compressed` | Is compressed |
| `encrypted` | Is encrypted |
| `hidden` | Hidden attribute |
| `system` | System attribute |
| `readonly` | Read-only attribute |
| `all` | All available columns |

---

## 🛠️ Commands

### `uffs search` (default)
Search for files matching a pattern.

```bash
uffs search "*.rs" --drive C --files-only --limit 100
```

### `uffs index`
Build a persistent index for instant future searches.

```bash
# Index a single drive
uffs index --drive C --output c_drive.parquet

# Index multiple drives
uffs index --drives C,D,E --output all_drives.parquet
```

### `uffs info`
Display information about an index file.

```bash
uffs info c_drive.parquet
```

### `uffs stats`
Show statistics about indexed files.

```bash
uffs stats --index c_drive.parquet --top 20
```

### `uffs save-raw`
Save raw MFT bytes for offline analysis.

```bash
uffs save-raw --drive C --output c_mft.raw --compress
```

### `uffs load-raw`
Load and parse a saved raw MFT file.

```bash
uffs load-raw c_mft.raw --output parsed.parquet
```

---

## 🏗️ Architecture

UFFS is built as a modular Rust workspace:

| Crate | Description | Documentation |
|-------|-------------|---------------|
| `uffs-polars` | Polars facade (compilation isolation) | - |
| `uffs-mft` | Direct MFT reading → Polars DataFrame | [📖 README](crates/uffs-mft/README.md) |
| `uffs-core` | Query engine using Polars lazy API | - |
| `uffs-cli` | Command-line interface (`uffs`) | - |
| `uffs-tui` | Terminal UI (`uffs_tui`) | - |
| `uffs-gui` | Graphical UI (`uffs_gui`) | - |

### Key Features

- **Direct MFT Access**: Bypasses Windows file enumeration APIs
- **Polars DataFrames**: Powerful, memory-efficient data manipulation
- **Async I/O**: High-throughput disk reading with Tokio
- **Parquet Persistence**: Compressed, columnar index storage
- **Multi-drive Parallel Search**: Query all drives concurrently
- **SIMD-accelerated Pattern Matching**: Fast glob and regex support

### Low-Level MFT Tools

The `uffs_mft` binary provides direct MFT access for advanced users:

```bash
# Quick MFT info (~10ms)
uffs_mft info --drive C

# Full MFT analysis with file statistics (~10-30s)
uffs_mft info --drive C --deep

# Export MFT to Parquet (fast mode - default)
uffs_mft read --drive C --output mft.parquet

# Export with complete extension data (slower)
uffs_mft read --drive C --output mft.parquet --full

# Benchmark single drive
uffs_mft bench --drive C --runs 3

# Benchmark all drives
uffs_mft bench-all --output benchmark.json

# List NTFS drives
uffs_mft drives
```

**Read Modes:**
- `--mode auto` (default): SSD→parallel, HDD→prefetch
- `--mode parallel`: Best for SSDs (8MB chunks)
- `--mode prefetch`: Best for HDDs (double-buffered 4MB chunks)
- `--mode streaming`: Low memory usage

**Fast vs Full:**
- Default (fast): Skips extension records (~1% of files), ~15-25% faster
- `--full`: Merges extension records for complete hard link/ADS data

See [uffs-mft README](crates/uffs-mft/README.md) for detailed documentation.

---

## 🔥 What Makes UFFS Blazing Fast

UFFS employs multiple layers of optimization to achieve maximum performance when reading the NTFS Master File Table:

### 1. Direct MFT Access with `FILE_FLAG_NO_BUFFERING`

Instead of using Windows file enumeration APIs, UFFS opens the raw volume and reads the MFT directly using unbuffered I/O. This bypasses the Windows file system cache and gives us full control over read patterns.

### 2. SSD/HDD-Aware I/O Tuning

UFFS automatically detects whether a drive is an SSD or HDD using Windows storage APIs (`IOCTL_STORAGE_QUERY_PROPERTY`) and tunes I/O parameters accordingly:

| Drive Type | Chunk Size | Rationale |
|------------|------------|-----------|
| **SSD** | 8 MB | Large sequential reads, no seek penalty |
| **HDD** | 4 MB | Balance between syscall overhead and seek time |

### 3. Minimal System Calls

By using large chunk sizes (4-8 MB instead of the typical 1 MB), UFFS reduces the number of `ReadFile` system calls by 4-8x. For a 4.5 GB MFT, this means ~500-1000 syscalls instead of ~4,500.

### 4. Zero-Allocation Record Parsing

Each thread uses a thread-local buffer for record parsing, eliminating per-record heap allocations. This is critical when processing millions of MFT records:

```rust
// Instead of allocating per record:
let mut record_buf = record_data.to_vec();  // ❌ Allocates

// We use thread-local buffers:
parse_record_zero_alloc(record_data, frs);  // ✅ Reuses buffer
```

### 5. Double-Buffered Prefetch

The `PrefetchMftReader` uses two alternating buffers to overlap I/O with processing:
- Read into buffer A while processing buffer B
- Swap buffers and repeat
- CPU never waits for disk I/O

### 6. Parallel Record Processing with Rayon

After reading chunks from disk, UFFS uses Rayon's parallel iterators to parse records across all CPU cores. Each core processes a portion of the chunk simultaneously.

### 7. Fragmented MFT Support

The MFT can be scattered across multiple non-contiguous extents on disk. UFFS handles this by:
1. Getting the extent map via `FSCTL_GET_RETRIEVAL_POINTERS`
2. Mapping Virtual Cluster Numbers (VCN) to Logical Cluster Numbers (LCN)
3. Reading from the correct physical locations

### 8. Polars Lazy Evaluation

Query operations use Polars' lazy API, which optimizes the query plan before execution. Filters are pushed down, columns are pruned, and operations are parallelized automatically.

### 9. SoA Layout (Struct-of-Arrays)

Instead of parsing into `Vec<ParsedRecord>` (Array-of-Structs) and then converting to DataFrame columns, UFFS parses directly into column vectors (Struct-of-Arrays). This eliminates the expensive AoS→SoA transpose and reduces df_build time by **90%**.

### 10. Fast Path (Skip Extension Records)

Extension records (~1% of files) contain overflow attributes for files with many hard links or ADS. The fast path skips these for maximum speed, while `--full` mode merges them for complete data.

### Performance Summary

| Optimization | Impact |
|--------------|--------|
| Direct MFT access | Bypasses slow Windows APIs |
| Large chunk sizes | 4-8x fewer syscalls |
| SSD/HDD detection | Optimal I/O parameters per drive |
| Thread-local buffers | ~0 allocations during parsing |
| Double-buffering | Overlapped I/O with processing |
| Rayon parallelism | All CPU cores utilized |
| Polars lazy eval | Optimized query execution |
| **SoA layout** | **90% faster df_build** |
| **Fast path** | **15-25% faster on SSD** |

---

## ⚠️ Requirements

### Platform
- **Windows only** for MFT reading (the core functionality)
- Cross-platform for working with saved indexes

### Privileges
- **Administrator privileges required** for direct MFT access
- Windows will show a UAC prompt when running UFFS

### Build Requirements
- Rust 1.85+ (Edition 2024)
- Windows SDK (for MFT reading)

---

## 📊 Output Formats

### Console (default)
Pretty-printed table output for terminal viewing.

### CSV
```bash
uffs "*.rs" --out results.csv --sep "," --header true
```

### JSON
```bash
uffs "*.rs" --format json --out results.json
```

### Parquet
Indexes are stored in Parquet format for efficient storage and fast loading.

---

## 🔧 Advanced Usage

### Using with Polars/Pandas

Export to CSV or Parquet and load in your data analysis tools:

```python
import polars as pl

# Load UFFS index
df = pl.read_parquet("c_drive.parquet")

# Analyze file distribution
df.group_by("extension").agg(
    pl.count().alias("count"),
    pl.col("size").sum().alias("total_size")
).sort("total_size", descending=True)
```

### Piping to Other Tools

```bash
# Find large log files and process with grep
uffs "*.log" --min-size 100MB --out console | grep "error"

# Export for further processing
uffs "*" --columns path,size --out - | sort -t, -k2 -n -r | head -100
```

---

## 📜 License

This project is licensed under the **Mozilla Public License 2.0 (MPL-2.0)**.

See [LICENSE](LICENSE) for details.

---

## 🙏 Acknowledgments

This Rust implementation is inspired by the original C++ UFFS, which was based on [SwiftSearch](https://sourceforge.net/projects/swiftsearch/) by wfunction.

---

## 📬 Contact

- **Author**: Robert Nio
- **Repository**: [github.com/githubrobbi/UltraFastFileSearch](https://github.com/githubrobbi/UltraFastFileSearch)
