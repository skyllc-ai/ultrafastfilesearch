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

### Benchmark Results

| Program | Records | Time | Notes |
|---------|---------|------|-------|
| **UFFS** | **19 Million** | **~120 seconds** | All disks in parallel |
| **UFFS** | **6.5 Million** | **~56 seconds** | Single HDD |
| Everything | 19 Million | 178 seconds | All disks |
| WizFile | 6.5 Million | 299 seconds | Single HDD |

> **UFFS is 68% faster than Everything and 4x faster than WizFile!**

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

```
crates/
├── uffs-polars   # Polars facade (compilation isolation)
├── uffs-mft      # Direct MFT reading → Polars DataFrame
├── uffs-core     # Query engine using Polars lazy API
├── uffs-cli      # Command-line interface
├── uffs-tui      # Terminal UI (interactive)
└── uffs-gui      # Graphical UI (future)
```

### Key Features

- **Direct MFT Access**: Bypasses Windows file enumeration APIs
- **Polars DataFrames**: Powerful, memory-efficient data manipulation
- **Async I/O**: High-throughput disk reading with Tokio
- **Parquet Persistence**: Compressed, columnar index storage
- **Multi-drive Parallel Search**: Query all drives concurrently
- **SIMD-accelerated Pattern Matching**: Fast glob and regex support

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
