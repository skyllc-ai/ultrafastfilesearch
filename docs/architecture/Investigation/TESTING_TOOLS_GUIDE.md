# UFFS Testing & Analysis Tools Guide

**Complete reference for all testing, diagnostic, and analysis tools in the UFFS project.**

---

## Table of Contents

1. [Overview](#overview)
2. [Data Collection](#data-collection)
   - [trial_run.ps1](#trial_runps1---comprehensive-data-collection-script)
3. [Main Binary Testing Capabilities](#main-binary-testing-capabilities)
   - [uffs CLI](#uffs---main-cli-binary)
   - [uffs_mft](#uffs_mft---mft-utility-binary)
4. [Diagnostic Binaries (uffs-diag)](#diagnostic-binaries-uffs-diag)
   - [compare_scan_parity](#compare_scan_parity---comprehensive-parity-comparison)
   - [analyze_diff](#analyze_diff---deep-output-comparison)
   - [compare_raw_mft](#compare_raw_mft---raw-mft-file-comparison)
   - [dump_mft_records](#dump_mft_records---inspect-specific-mft-records)
   - [scan_mft_magic](#scan_mft_magic---magic-value-distribution-analysis)
   - [inspect_mft_record_flow](#inspect_mft_record_flow---parse-pipeline-inspection)
   - [analyze_mft_parents](#analyze_mft_parents---parent-child-coverage-analysis)
   - [cross_check_mft_reference](#cross_check_mft_reference---reference-validation)
   - [dump_mft_extents](#dump_mft_extents---mft-extent-layout-windows-only)
5. [Analysis Scripts (scripts/)](#analysis-scripts-scripts)
   - [analyze_cpp_stats.rs](#analyze_cpp_statsrs---c-output-statistics)
   - [analyze_trial_outputs.rs](#analyze_trial_outputsrs---trial-run-comparison)
   - [compare_outputs.py](#compare_outputspy---python-output-comparison)
   - [diagnose_mft_counts.rs](#diagnose_mft_countsrs---mft-count-diagnostic)
   - [find_missing_paths.rs](#find_missing_pathsrs---missing-path-extractor)
   - [analyze_parity_differences.rs](#analyze_parity_differencesrs---parity-difference-analyzer)
6. [Common Workflows](#common-workflows)

---

## Overview

The UFFS project includes a comprehensive suite of testing and analysis tools designed to:

- **Collect test data** from Windows systems (MFT snapshots + scan outputs)
- **Compare C++ vs Rust implementations** for parity verification
- **Diagnose MFT parsing issues** with offline analysis tools
- **Benchmark performance** across different algorithms and drive types
- **Analyze differences** in scan outputs with detailed statistics
- **Inspect raw MFT data** at the record level for debugging

### Tool Categories

| Category | Tools | Purpose |
|----------|-------|---------|
| **Data Collection** | `trial_run.ps1` | Collect MFT snapshots and scan outputs on Windows |
| **Main Binaries** | `uffs`, `uffs_mft` | Production binaries with built-in testing/benchmarking |
| **Diagnostic Tools** | 9 binaries in `uffs-diag` | Offline MFT analysis and comparison |
| **Analysis Scripts** | 6 scripts in `scripts/` | Lightweight analysis with rust-script and Python |

---

## Data Collection

### `trial_run.ps1` - Comprehensive Data Collection Script

**Location:** `docs/architecture/Investigation/trial_run.ps1`

**Purpose:** Collect MFT snapshots and run four different scan flows for comparison testing.

**Platform:** Windows only (requires admin privileges)

#### Usage

```powershell
# Run on all NTFS drives (auto-detect)
.\trial_run.ps1

# Run on specific drives
.\trial_run.ps1 -Drives C,D,E

# Skip MFT save tests (faster)
.\trial_run.ps1 -SkipMft

# Custom binary directory
.\trial_run.ps1 -BinDir "C:\custom\bin"

# Parallel processing (PowerShell 7+)
.\trial_run.ps1 -ThrottleLimit 4
```

#### What It Collects

For each drive (e.g., `C`), the script generates:

**Scan Outputs (CSV format):**
- `rust_c.txt` - Rust current implementation (with `--no-bitmap`)
- `cpp_c.txt` - C++ reference implementation (uffs.com)
- `rust_new_c.txt` - Rust with C++ tree algorithm (with `--no-bitmap`)
- `rust_cpp_full_c.txt` - Rust with full C++ port (parsing + tree, with `--no-bitmap`)

**Log Files:**
- `rust_c.log`, `cpp_c.log`, `rust_new_c.log`, `rust_cpp_full_c.log`

**MFT Snapshots (first drive only):**
- `C_mft.bin` - Compressed MFT snapshot (zstd)
- `C_mft_no_compress.bin` - Uncompressed MFT snapshot
- `C_mft.raw` - Raw MFT bytes (for diagnostic tools)

**Summary Report:**
- `trial_run.md` - Markdown report with timing and file sizes

#### Scan Flows Explained

1. **Rust (current)** - Default Rust implementation
   ```powershell
   uffs.exe "*" --drive C --no-bitmap --out rust_c.txt
   ```

2. **C++** - Original C++ implementation (baseline)
   ```powershell
   uffs.com "*" C: > cpp_c.txt
   ```

3. **Rust (new tree)** - Rust with C++ tree algorithm port
   ```powershell
   uffs.exe "*" --drive C --tree-algo cpp --no-bitmap --out rust_new_c.txt
   ```

4. **Rust (cpp full)** - Rust with both C++ parsing AND tree algorithms
   ```powershell
   uffs.exe "*" --drive C --parse-algo cpp_port --tree-algo cpp --no-bitmap --out rust_cpp_full_c.txt
   ```

**Note:** The `--no-bitmap` flag ensures all MFT records are read (not just "in-use" ones according to bitmap), matching the behavior of `uffs_mft save` and achieving perfect parity with offline scans.

---

## Main Binary Testing Capabilities

### `uffs` - Main CLI Binary

**Location:** `crates/uffs-cli`

**Purpose:** Main search CLI with built-in testing and benchmarking capabilities.

#### Subcommands

**Search (Default - No Subcommand):**
```bash
# Search is the default action - no subcommand needed
uffs "*.txt"                      # Find all .txt files
uffs "c:/pro*"                    # Find files starting with "pro" on C:
```

**Index Management:**
```bash
# Build index from MFT(s)
uffs index output.parquet         # Index ALL NTFS drives
uffs index -d C output.parquet    # Index only C: drive
uffs index --drives C,D,E out.parquet  # Index specific drives

# Show index information
uffs info index.parquet

# Show statistics
uffs stats index.parquet          # Top 10 largest files
uffs stats index.parquet --top 50 # Top 50 largest files
```

#### Data Sources

```bash
# Live MFT (default - all drives)
uffs "*.txt"

# Specific drive
uffs "*.txt" --drive C

# Multiple drives
uffs "*.txt" --drives C,D,E

# Pre-built index file
uffs "*.txt" --index index.parquet

# Offline MFT file (cross-platform)
uffs "*.txt" --mft-file D_mft.bin
uffs "*.txt" --mft-file D_mft.bin --drive D  # Specify drive letter for path resolution
```

#### Algorithm Selection (Parity Testing)

```bash
# Current Rust algorithms (default)
uffs "*.txt"

# C++ port algorithms (for parity testing)
uffs "*" --tree-algo cpp          # Use C++ tree algorithm
uffs "*" --parse-algo cpp_port    # Use C++ parsing algorithm
uffs "*" --tree-algo cpp --parse-algo cpp_port  # Full C++ port
```

#### Filtering Options

```bash
# File type filters
uffs "*" --files-only             # Only files (exclude directories)
uffs "*" --dirs-only              # Only directories
uffs "*" --hide-system            # Hide system files (starting with $)

# Extension filter
uffs "*" --ext rs,toml            # Only .rs and .toml files

# Size filters
uffs "*" --min-size 1048576       # Files >= 1MB
uffs "*" --max-size 10485760      # Files <= 10MB
uffs "*" --min-size 1048576 --max-size 10485760  # Between 1MB and 10MB

# Result limiting
uffs "*" --limit 100              # Show only first 100 results
uffs "*" -n 50                    # Short form
```

#### Query and Matching

```bash
# Case sensitivity
uffs "*.TXT" --case sensitive     # Case-sensitive matching
uffs "*.TXT" --case insensitive   # Case-insensitive (default)

# Query modes
uffs "*.txt" --query-mode glob    # Glob pattern (default)
uffs "*.txt" --query-mode regex   # Regular expression
uffs "*.txt" --query-mode literal # Literal string

# Path filters (positive/negative)
uffs "*" --pos "src,test"         # Only paths containing "src" or "test"
uffs "*" --neg "target,node_modules"  # Exclude paths containing these
```

#### Output Formatting

```bash
# Format selection
uffs "*" --format csv             # CSV output (default)
uffs "*" --format json            # JSON output
uffs "*" --format table           # Human-readable table

# CSV customization
uffs "*" --columns path,size,modified  # Select specific columns
uffs "*" --sep "|"                # Custom separator (default: tab)
uffs "*" --quotes always          # Quote all fields
uffs "*" --quotes never           # Never quote fields
uffs "*" --quotes auto            # Quote only when needed (default)
uffs "*" --header                 # Include header row (default: no header)

# Output to file
uffs "*" --out results.csv        # Save to file
```

#### Performance and Debugging

```bash
# Performance testing
uffs "*.txt" --benchmark          # Show detailed timing breakdown
uffs "*.txt" --profile            # Enable profiling output
uffs "*.txt" --debug-tree         # Show tree building debug info

# Cache control
uffs "*" --no-cache               # Force fresh MFT read (ignore cache)

# Bitmap control
uffs "*" --no-bitmap              # Read entire MFT (don't use bitmap optimization)
```

#### Key Testing Scenarios

**Parity Testing:**
```bash
# Full C++ port mode (for comparison with C++ baseline)
uffs "*" --drive C --parse-algo cpp_port --tree-algo cpp --no-bitmap --out rust_cpp_full.txt
```

**Offline MFT Testing:**
```bash
# Search using saved MFT file (cross-platform)
uffs "*" --mft-file D_mft.bin --parse-algo cpp_port --tree-algo cpp --out rust_offline.txt
```

**Filtered Testing:**
```bash
# Test with various filters
uffs "*.rs" --min-size 10000 --files-only --out rust_files.csv
uffs "*" --dirs-only --hide-system --out directories.csv
uffs "*" --ext txt,md --limit 1000 --out limited.csv
```

---

### `uffs_mft` - MFT Utility Binary

**Location:** `crates/uffs-mft`

**Purpose:** Low-level MFT operations, benchmarking, and raw MFT save/load.

**Platform:** Most commands are Windows-only (require live NTFS access). `load` command works cross-platform.

#### Core Commands

**`read` - Read MFT and Export to Parquet:**
```bash
# Basic read (auto-detect best mode for drive type)
uffs_mft read --drive C --output c_drive.parquet

# Specify read mode
uffs_mft read --drive C --output c.parquet --mode parallel    # Best for SSD
uffs_mft read --drive C --output c.parquet --mode streaming   # Lower memory
uffs_mft read --drive C --output c.parquet --mode prefetch    # Best for HDD

# Full mode (merge extension records)
uffs_mft read --drive C --output c.parquet --full

# Unique mode (skip hard link duplicates)
uffs_mft read --drive C --output c.parquet --unique

# Forensic mode (include deleted files)
uffs_mft read --drive C --output c.parquet --forensic
```

**`save` - Save Raw MFT for Offline Analysis:**
```bash
# Save compressed (default, recommended for cross-platform analysis)
uffs_mft save --drive C -o C_mft.bin

# Save uncompressed
uffs_mft save --drive C -o C_mft.bin --no-compress

# Save raw-compatible format (for other MFT tools like mft-reader-rs)
uffs_mft save --drive C -o C_mft.raw --raw
```

**`load` - Load Saved MFT and Export (Cross-Platform):**
```bash
# Info only (show metadata)
uffs_mft load C_mft.bin --info-only

# Export to CSV
uffs_mft load C_mft.bin -o mft.csv

# Export to Parquet
uffs_mft load C_mft.bin -o mft.parquet

# Build index with tree metrics (debug mode)
uffs_mft load C_mft.bin --build-index
uffs_mft load C_mft.bin --build-index --debug-tree  # Verbose tree building

# Load raw NTFS format (from other tools)
uffs_mft load C_mft.raw --drive C -o output.csv

# Forensic mode (include deleted files)
uffs_mft load C_mft.bin -o output.csv --forensic
```

**`drives` - List All NTFS Drives (Windows):**
```bash
uffs_mft drives
```

#### Information and Diagnostics

**`info` - Show MFT Information:**
```bash
# Basic info (fast, reads MFT header only)
uffs_mft info --drive C

# Deep scan (reads all records, shows detailed statistics)
uffs_mft info --drive C --deep

# Disable bitmap optimization
uffs_mft info --drive C --no-bitmap

# Unique mode (skip hard link duplicates)
uffs_mft info --drive C --unique
```

**`bitmap-diag` - Diagnose MFT Bitmap:**
```bash
# Show bitmap statistics
uffs_mft bitmap-diag --drive C

# Show sample of individual record states
uffs_mft bitmap-diag --drive C --samples
```

**`usn-info` - Show USN Journal Information:**
```bash
uffs_mft usn-info --drive C
```

#### Benchmarking Commands

**`benchmark-index-lean` - Benchmark Lean Index Building:**
```bash
# Basic benchmark (auto-detect best settings)
uffs_mft benchmark-index-lean --drive C

# Tune concurrency
uffs_mft benchmark-index-lean --drive C --concurrency 2
uffs_mft benchmark-index-lean --drive C --concurrency 8
uffs_mft benchmark-index-lean --drive C --concurrency 32

# Tune I/O size
uffs_mft benchmark-index-lean --drive C --io-size-kb 1024
uffs_mft benchmark-index-lean --drive C --io-size-kb 2048
uffs_mft benchmark-index-lean --drive C --io-size-kb 4096

# Disable optimizations
uffs_mft benchmark-index-lean --drive C --no-bitmap        # Read all records
uffs_mft benchmark-index-lean --drive C --no-placeholders  # Skip placeholder creation
```

**`bench` - Benchmark Single Drive:**
```bash
# Basic benchmark
uffs_mft bench --drive C

# JSON output
uffs_mft bench --drive C --json

# Skip DataFrame building (measure I/O + parse only)
uffs_mft bench --drive C --no-df

# Multiple runs for averaging
uffs_mft bench --drive C --runs 5

# Specify read mode
uffs_mft bench --drive C --mode parallel
uffs_mft bench --drive C --mode prefetch

# Full mode (merge extension records)
uffs_mft bench --drive C --full
```

**`bench-all` - Benchmark All NTFS Drives:**
```bash
# Benchmark all drives
uffs_mft bench-all

# Save results to JSON
uffs_mft bench-all --output benchmark_results.json

# Skip DataFrame building
uffs_mft bench-all --no-df

# Multiple runs per drive
uffs_mft bench-all --runs 3

# Full mode
uffs_mft bench-all --full
```

**`index-all` - Index All NTFS Drives (Lean Mode):**
```bash
# Index all drives (uses cache by default)
uffs_mft index-all

# Index specific drives
uffs_mft index-all --drives C,D,E

# Force fresh read (bypass cache)
uffs_mft index-all --no-cache

# Custom cache TTL (seconds)
uffs_mft index-all --ttl 300  # 5 minutes
```

---

## Diagnostic Binaries (uffs-diag)

**Location:** `crates/uffs-diag`

**Purpose:** Offline, cross-platform diagnostic tools for analyzing MFT snapshots and comparing outputs.

**Platform:** Most tools are cross-platform (work on Mac/Linux with saved MFT files). Some are Windows-only where noted.

**Build:** All diagnostic tools are built together:
```bash
cargo build --release -p uffs-diag
```

**Run:** Use cargo run with the specific binary name:
```bash
cargo run --release -p uffs-diag --bin <tool_name> -- <args>
```

---

### `compare_scan_parity` - Comprehensive Parity Comparison

**Purpose:** Deep comparison of C++ vs Rust scan outputs with field-by-field analysis and statistical reporting.

**Best For:** Validating that Rust implementation produces identical results to C++ baseline.

#### Usage

```bash
# Basic comparison
cargo run --release -p uffs-diag --bin compare_scan_parity -- \
  docs/trial_runs/d_disk/cpp_d.txt \
  docs/trial_runs/d_disk/rust_cpp_full_d.txt

# With markdown report
cargo run --release -p uffs-diag --bin compare_scan_parity -- \
  cpp_d.txt rust_cpp_full_d.txt \
  --report parity_report.md

# Verbose mode (show all differences)
cargo run --release -p uffs-diag --bin compare_scan_parity -- \
  cpp_d.txt rust_cpp_full_d.txt -v
```

#### What It Analyzes

- **Path matching** with normalization (case, slashes)
- **Size metrics** (`size`, `allocated_size`)
- **Tree metrics** (`descendants`, `treesize`, `tree_allocated`)
- **Timestamps** (`created`, `modified`, `accessed`)
- **Boolean flags** (`directory`, `hidden`, `system`, `readonly`, etc.)
- **ADS (Alternate Data Streams)** analysis
- **Statistical summary** (match rates, mean/median/max differences)

#### Output

Terminal output includes:
- Row counts for both files
- Common paths count
- Paths only in C++ / only in Rust
- Field-by-field mismatch statistics
- Sample differences for each field
- ADS comparison

Optional markdown report with detailed tables and statistics.

---

### `analyze_diff` - Deep Output Comparison

**Purpose:** Development/debugging tool for identifying structural differences and missing records.

**Best For:** Initial investigation when outputs don't match.

#### Usage

```bash
cargo run --release -p uffs-diag --bin analyze_diff -- \
  cpp.txt rust.txt
```

#### What It Analyzes

- Column comparison between C++ and Rust outputs
- Path matching statistics
- Missing path analysis by drive and parent directory
- Pattern analysis for system files and unknown paths

---

### `compare_raw_mft` - Raw MFT File Comparison

**Purpose:** Record-by-record comparison of two raw MFT files without loading entire files into memory.

**Best For:** Verifying that MFT save/load pipeline preserves data integrity, or comparing C++ vs Rust raw MFT dumps.

#### Usage

```bash
cargo run --release -p uffs-diag --bin compare_raw_mft -- \
  f_mft_cpp.raw f_mft_rust.raw
```

#### What It Does

- Streams through both files record-by-record
- Compares headers (version, flags, record_size, record_count)
- Counts identical vs different records
- Reports differing byte counts
- Shows sample differences (first 20 differing records)
- Memory efficient (doesn't load entire MFT)

#### Output

```
=== Raw MFT Comparison ===
File A: f_mft_cpp.raw
File B: f_mft_rust.raw

Header A: version=1, flags=1, record_size=1024, record_count=7058034, original_size=...
Header B: version=1, flags=1, record_size=1024, record_count=7058034, original_size=...

Comparing 7058034 records of 1024 bytes each (6.72 GiB)...

=== Comparison Complete ===
Total records:  7058034
Same records:   7058034
Diff records:   0 (0.000000%)
```

---

### `dump_mft_records` - Inspect Specific MFT Records

**Purpose:** Dump raw MFT record headers for specific FRS (File Record Segment) values.

**Best For:** Debugging specific records, inspecting flags, checking base vs extension records.

#### Usage

```bash
# Dump specific records
cargo run --release -p uffs-diag --bin dump_mft_records -- \
  C_mft.raw 0 5 100 2640657

# Test extension record merging
cargo run --release -p uffs-diag --bin dump_mft_records -- \
  --test-merge C_mft.raw <base_frs> <ext_frs>
```

#### What It Shows

For each FRS:
- Magic value (FILE/RCRD/INDX/ZERO)
- Flags (in_use, directory, base_record)
- Sequence number
- Base file record segment (for extension records)
- First attribute offset
- Bytes in use / bytes allocated

#### Example Output

```
FRS 2640657:
  magic                = FILE (0x454C4946)
  flags                = 0x0001 (in_use=true, directory=false)
  sequence             = 42
  base_file_record_seg = 0 (is_base_record=true)
  first_attr_offset    = 56
  bytes_in_use         = 512
  bytes_allocated      = 1024
```



---

### `scan_mft_magic` - Magic Value Distribution Analysis

**Purpose:** Scan all records in a raw MFT and classify by magic value (FILE/RCRD/INDX/ZERO/other).

**Best For:** Understanding MFT structure, finding where valid FILE records end, detecting corruption.

#### Usage

```bash
# Default bucket size (100,000 records)
cargo run --release -p uffs-diag --bin scan_mft_magic -- \
  C_mft.raw

# Custom bucket size
cargo run --release -p uffs-diag --bin scan_mft_magic -- \
  C_mft.raw 50000
```

#### What It Does

- Reads magic value from each record header
- Classifies as FILE/RCRD/INDX/ZERO/other
- Aggregates counts by FRS bucket
- Shows distribution across the MFT

#### Output

```
Bucket      FILE      RCRD      INDX      ZERO     OTHER
------  --------  --------  --------  --------  --------
     0    99,842       158         0         0         0
     1   100,000         0         0         0         0
     2   100,000         0         0         0         0
   ...
    70    58,034         0         0    41,966         0
```

**Use Case:** Identify where valid records end and unused space begins.

---

### `inspect_mft_record_flow` - Parse Pipeline Inspection

**Purpose:** Inspect the full raw→fixup→parse pipeline for specific FRS values.

**Best For:** Debugging why a specific record is being dropped or parsed incorrectly.

#### Usage

```bash
cargo run --release -p uffs-diag --bin inspect_mft_record_flow -- \
  C_mft.raw 2640657 2631176 2628892
```

#### What It Does

- Loads raw bytes for each FRS
- Shows raw header fields
- Applies fixup (on Windows)
- Runs full parse pipeline
- Reports success/failure at each stage

#### Output

```
============================================================
FRS 2640657 - raw -> fixup -> parse_record_full
============================================================
Raw header:
  magic                = FILE (0x454C4946)
  flags                = 0x0001 (in_use=true, directory=false)
  sequence             = 42
  base_file_record_seg = 0 (is_base_record=true)

Fixup: SUCCESS
Parse: SUCCESS
  - Found 8 attributes
  - Has $FILE_NAME attribute
  - Has $DATA attribute
```

**Use Case:** Pinpoint where a record fails (fixup vs parse) compared to reference data.

---

### `analyze_mft_parents` - Parent-Child Coverage Analysis

**Purpose:** Analyze parent/child coverage in an MFT Parquet file to find missing parent directories.

**Best For:** Understanding why path resolution needs placeholders, debugging incomplete directory trees.

#### Usage

```bash
cargo run --release -p uffs-diag --bin analyze_mft_parents -- \
  docs/trial_runs/f_mft.parquet
```

#### What It Analyzes

- Which `parent_frs` values referenced by children don't have corresponding directory rows
- How many children reference each missing parent
- Statistics on missing parent coverage

#### Output

```
=======================================================================
MFT Parent Coverage Analysis
=======================================================================
Input Parquet: docs/trial_runs/f_mft.parquet

Rows: 2,845,123
Cols: 37

Analyzing parent/child relationships...

Missing parents: 1,234
  - 1,234 parent_frs values referenced but not present as directories

Top missing parents by child count:
  parent_frs=2640657: 523 children
  parent_frs=2631176: 412 children
  parent_frs=2628892: 387 children
```

**Use Case:** Explain why path resolver injects `<dir:XXXXXX>` placeholders.

---

### `cross_check_mft_reference` - Reference Validation

**Purpose:** Cross-check UFFS Parquet output against reference CSV from external MFT tools.

**Best For:** Validating UFFS parsing against known-good reference data.

#### Usage

```bash
cargo run --release -p uffs-diag --bin cross_check_mft_reference -- \
  f_mft_reference.csv \
  f_mft.parquet
```

#### What It Does

- Loads reference CSV (from tools like mft-reader-rs)
- Loads UFFS Parquet output
- Joins on FRS (record number)
- Compares `is_directory` / `is_base_record` flags
- Reports agreement statistics
- Focuses on high-impact missing parents

#### Output

```
=======================================================================
MFT Reference vs Parquet Cross-Check
=======================================================================
Reference CSV : f_mft_reference.csv
Parquet       : f_mft.parquet

Reference rows: 2,845,123
Parquet rows  : 2,845,123
Joined rows   : 2,845,123

Directory flag agreement: 99.95% (2,843,700 / 2,845,123)
Base record flag agreement: 100.00% (2,845,123 / 2,845,123)

Mismatches in is_directory: 1,423 records
```

**Use Case:** Validate UFFS parsing correctness against external tools.

---

### `dump_mft_extents` - MFT Extent Layout (Windows Only)

**Purpose:** Dump $MFT extent list (VCN, cluster_count, LCN) for a given NTFS volume.

**Platform:** Windows only

**Best For:** Debugging MFT fragmentation, comparing extent detection with other tools.

#### Usage

```bash
# Windows only
cargo run --release -p uffs-diag --bin dump_mft_extents -- F
```

#### What It Shows

- Volume data (bytes per sector/cluster, MFT start LCN, etc.)
- Complete extent list with VCN, cluster count, LCN
- Byte offsets and sizes for each extent
- Summary statistics (total clusters, bytes, approx records)

#### Output

```
===============================================
Dumping $MFT extents for volume F:
===============================================
Volume data:
  bytes_per_sector            = 512
  bytes_per_cluster           = 4096
  bytes_per_file_record_seg   = 1024
  mft_start_lcn               = 786432
  mft_valid_data_length (B)   = 7227426816

Extents:
 Idx        VCN    Clusters            LCN     ByteOffset       ByteSize
   0          0      192000         786432     3221225472     786432000
   1     192000       48000       12345678    50593792000     196608000
  ...

Summary:
  extent_count      = 28
  total_clusters    = 1764864
  total_bytes       = 7227426816
  approx_records    = 7058034
```

**Use Case:** Compare UFFS extent detection with `ntfsinfo` or `fsutil file queryextents`.

---

## Analysis Scripts (scripts/)

**Location:** `scripts/`

**Purpose:** Lightweight analysis scripts using `rust-script` and Python for quick analysis of scan outputs.

**Platform:** Cross-platform (Mac/Linux/Windows)

**Requirements:**
- **Rust scripts:** Install `rust-script` with `cargo install rust-script`
- **Python scripts:** Python 3.6+ (no external dependencies)

---

### `analyze_cpp_stats.rs` - C++ Output Statistics

**Purpose:** Extract file/directory statistics per drive from C++ UFFS output.

**Best For:** Quick overview of C++ baseline data before comparison.

#### Usage

```bash
rust-script scripts/analyze_cpp_stats.rs docs/trial_runs/d_disk/cpp_d.txt
```

#### What It Analyzes

- Total record counts
- Per-drive file and directory counts
- Streaming analysis (memory efficient, doesn't load entire file)

#### Output

```
═══════════════════════════════════════════════════════════════
  C++ UFFS Output Analysis
═══════════════════════════════════════════════════════════════
File: docs/trial_runs/d_disk/cpp_d.txt

Drive    Total Files    Total Dirs
-----  -----------  ------------
C:       1,234,567       123,456
D:       5,678,901       567,890
-----  -----------  ------------
TOTAL    6,913,468       691,346

Processing time: 12.3s
```

---

### `analyze_trial_outputs.rs` - Trial Run Comparison

**Purpose:** Compare C++ vs Rust trial run outputs for exact parity.

**Best For:** Quick path-level comparison after running `trial_run.ps1`.

#### Usage

```bash
rust-script scripts/analyze_trial_outputs.rs docs/trial_runs/d_disk
```

#### What It Does

- Auto-detects `cpp_*.txt` and `rust_new_*.txt` files in directory
- Extracts all paths from both files
- Compares for exact match
- Reports differences with context
- Analyzes patterns in differences

#### Output

```
Analyzing trial outputs in: docs/trial_runs/d_disk

Found files:
  C++:  cpp_d.txt
  Rust: rust_new_d.txt

Loading C++ paths...
Loading Rust paths...

C++ paths:   7,058,034
Rust paths:  7,057,993

Common paths: 7,057,990
C++ only:     44
Rust only:    3

Match rate: 99.9994%

❌ Paths in C++ but NOT in Rust (first 20):
  d:\rust\target\rls\debug\incremental\uffs_cli-123\s-abc.bin
  d:\rust\target\rls\debug\incremental\uffs_cli-456\s-def.bin
  ...

⚠️  Attribute differences in common paths: 12
  d:\some\file.txt
    Size: C++=1024 vs Rust=1024
    Desc: C++=5 vs Rust=6
```

---

### `compare_outputs.py` - Python Output Comparison

**Purpose:** Lightweight Python script for quick C++ vs Rust comparison.

**Best For:** Quick analysis when you don't want to compile Rust tools.

#### Usage

```bash
python3 scripts/compare_outputs.py cpp_d.txt rust_d.txt
```

#### What It Analyzes

- File sizes
- Drive coverage (per-drive record counts)
- Path comparison (unique paths, common, differences)
- Match rate percentage

#### Output

```
============================================================
UFFS Output Comparison: C++ vs Rust
============================================================

File sizes:
  C++:  1,234,567,890 bytes
  Rust: 1,234,500,000 bytes

--- Drive Coverage ---
  Drive         C++         Rust         Diff
  ------  ----------   ----------   ----------
  C:       1,234,567    1,234,567            =
  D:       5,678,901    5,678,860          -41
  ------  ----------   ----------   ----------
  TOTAL    6,913,468    6,913,427          -41

--- Path Comparison ---
  C++ unique paths:   6,913,468
  Rust unique paths:  6,913,427
  Common:             6,913,427
  C++ only:           41
  Rust only:          0
  Match rate:         99.9994% (vs C++ baseline)

--- Sample: In C++ but NOT in Rust (first 10) ---
  d:\rust\target\rls\debug\incremental\uffs_cli-123\s-abc.bin
  ...
```

---

### `diagnose_mft_counts.rs` - MFT Count Diagnostic

**Purpose:** Detailed comparison of MFT record counts between C++ and Rust outputs using Polars.

**Best For:** Diagnosing count mismatches, analyzing unresolved paths.

#### Usage

```bash
rust-script scripts/diagnose_mft_counts.rs cpp_d.txt rust_d.txt
```

#### What It Analyzes

- Total record counts per drive
- Directory vs file counts per drive
- Records with resolved vs unresolved paths
- Sample of unresolved paths per drive

#### Output

```
Loading C++ output: cpp_d.txt
Loading Rust output: rust_d.txt

======================================================================
C++ ANALYSIS
======================================================================
Total rows: 7,058,034

Null paths: 0
Unresolved (<unknown:...>): 0

Per-drive breakdown:
 Drive        Total         Dirs   Unresolved
------ ------------ ------------ ------------
    C:    1,234,567      123,456            0
    D:    5,823,467      582,346            0

======================================================================
RUST ANALYSIS
======================================================================
Total rows: 7,057,993

Null paths: 0
Unresolved (<unknown:...>): 0

Per-drive breakdown:
 Drive        Total         Dirs   Unresolved
------ ------------ ------------ ------------
    C:    1,234,567      123,456            0
    D:    5,823,426      582,346            0

======================================================================
COMPARISON: C++ vs Rust
======================================================================
 Drive    C++ Total   Rust Total   Difference    % Match
------ ------------ ------------ ------------ ----------
    C:    1,234,567    1,234,567            0   100.00%
    D:    5,823,467    5,823,426          -41    99.99%
------ ------------ ------------ ------------ ----------
 TOTAL    7,058,034    7,057,993          -41    99.99%
```

---

### `find_missing_paths.rs` - Missing Path Extractor

**Purpose:** Find and extract full CSV lines for paths in C++ but not in Rust.

**Best For:** Detailed analysis of specific missing paths with all attributes.

#### Usage

```bash
rust-script scripts/find_missing_paths.rs cpp_d.txt rust_d.txt
```

#### What It Does

- Reads all Rust paths into memory
- Streams through C++ file to find missing paths
- Outputs full CSV line for each missing path
- Writes missing paths to `missing_paths.txt`

#### Output

```
Reading Rust file...
Rust paths: 7,057,993

Reading C++ file and finding missing...

Found 41 paths in C++ but not in Rust:

PATH: d:\rust\target\rls\debug\incremental\uffs_cli-123\s-abc.bin
LINE: "d:\rust\target\rls\debug\incremental\uffs_cli-123\s-abc.bin",1024,4096,0,2024-01-15 10:30:45,...

PATH: d:\rust\target\rls\debug\incremental\uffs_cli-456\s-def.bin
LINE: "d:\rust\target\rls\debug\incremental\uffs_cli-456\s-def.bin",2048,4096,0,2024-01-15 10:31:12,...

...

Written to missing_paths.txt
```

**Use Case:** Get full attribute data for missing paths to understand why they're missing (timestamps, flags, parent FRS, etc.).

---

### `analyze_parity_differences.rs` - Parity Difference Analyzer

**Purpose:** Analyze parity differences between C++ and Rust scan outputs with pattern analysis.

**Best For:** Understanding what types of files differ between C++ and Rust outputs, identifying systematic patterns in differences.

#### Usage

```bash
rust-script scripts/analyze_parity_differences.rs cpp_d.txt rust_d.txt
```

#### What It Does

- Extracts all paths from both C++ and Rust outputs
- Finds paths in Rust but NOT in C++ (extra paths)
- Finds paths in C++ but NOT in Rust (missing paths)
- Analyzes patterns in the differences:
  - ADS (Alternate Data Streams) count
  - Directory vs file breakdown
  - File type distribution (.bin, .exe, .dll, .txt, Zone.Identifier, etc.)
  - Path pattern analysis (Rust target dirs, Dropbox paths, system paths)

#### Output

```
🔍 Analyzing Parity Differences: Rust vs C++ Baseline

📊 Step 1: Extracting paths from both files...
  This may take a few minutes for 7M+ lines...

  C++ paths:  7,058,031
  Rust paths: 7,057,991

📊 Step 2: Finding differences...

  Paths in Rust but NOT in C++: 1
  Paths in C++ but NOT in Rust: 41

🔍 Analyzing: Missing in Rust

📄 First 20 paths:
  1. d:/rust/target/rls/debug/incremental/.../dep-graph.bin
  2. d:/rust/target/rls/debug/incremental/.../query-cache.bin
  ...

📊 Pattern analysis:

  Paths with ':' (potential ADS): 41
  Paths ending with '/' (directories): 0

  File type breakdown:
    .bin files: 26
    .exe files: 1
    Zone.Identifier: 14

  Path patterns:
    Rust target dirs: 26
    Dropbox paths: 40
```

**Use Case:** Quickly identify systematic patterns in parity differences (e.g., "all missing paths are ADS", "all missing paths are in Rust target directories").

---

### Other Scripts

**`ci-pipeline.rs`** - Full CI pipeline runner
```bash
rust-script scripts/ci-pipeline.rs go -v
```
Runs complete CI pipeline: format check, clippy, tests, builds.

**`build-local.rs`** - Local build helper
```bash
rust-script scripts/build-local.rs
```
Builds all workspace crates in release mode.

**`create_mft_test_tree.ps1`** - Test data generator (Windows)
```powershell
.\scripts\create_mft_test_tree.ps1 -Drive D -Count 1000
```
Creates test directory structure for MFT testing.

---

## Common Workflows

### Workflow 1: Full Parity Testing (Windows)

**Goal:** Verify Rust implementation matches C++ baseline exactly.

```powershell
# 1. Collect test data on Windows
cd docs/architecture/Investigation
.\trial_run.ps1 -Drives D

# 2. Compare C++ vs Rust (full C++ port)
cargo run --release -p uffs-diag --bin compare_scan_parity -- \
  docs/trial_runs/d_disk/cpp_d.txt \
  docs/trial_runs/d_disk/rust_cpp_full_d.txt \
  --report docs/trial_runs/d_disk/parity_report.md

# 3. Review report
cat docs/trial_runs/d_disk/parity_report.md
```

**Expected Result:** 100.0000% match rate (or very close, with explainable differences).

---

### Workflow 2: Offline MFT Analysis (Cross-Platform)

**Goal:** Analyze Windows MFT on Mac/Linux without live NTFS access.

```bash
# On Windows: Save MFT
uffs_mft save --drive D -o D_mft.bin

# Transfer D_mft.bin to Mac/Linux

# On Mac/Linux: Analyze
# 1. Export to Parquet
uffs_mft load D_mft.bin -o d_mft.parquet

# 2. Analyze parent coverage
cargo run --release -p uffs-diag --bin analyze_mft_parents -- d_mft.parquet

# 3. Inspect specific records
cargo run --release -p uffs-diag --bin dump_mft_records -- D_mft.bin 0 5 100

# 4. Scan magic distribution
cargo run --release -p uffs-diag --bin scan_mft_magic -- D_mft.bin

# 5. Run search on saved MFT
uffs "*" --mft-file D_mft.bin --parse-algo cpp_port --tree-algo cpp --out rust_offline.txt
```

---

### Workflow 3: Debugging Missing Paths

**Goal:** Understand why certain paths are missing from Rust output.

```bash
# 1. Run parity comparison to identify missing paths
cargo run --release -p uffs-diag --bin compare_scan_parity -- \
  cpp_d.txt rust_d.txt --report parity.md

# 2. Review parity.md to find missing FRS values
# (Look in "Paths only in C++" section)

# 3. Inspect those specific records
cargo run --release -p uffs-diag --bin dump_mft_records -- \
  D_mft.raw 2640657 2631176

# 4. Check parse pipeline
cargo run --release -p uffs-diag --bin inspect_mft_record_flow -- \
  D_mft.raw 2640657 2631176

# 5. Analyze parent coverage
cargo run --release -p uffs-diag --bin analyze_mft_parents -- d_mft.parquet
```

---

### Workflow 4: Raw MFT Comparison (C++ vs Rust)

**Goal:** Verify that C++ and Rust read identical raw MFT data.

```powershell
# On Windows:
# 1. Save MFT with C++ tool (using spec from docs/CPP_RAW_MFT_DUMP_TOOL_SPEC.md)
.\uffs_dump_mft.exe F f_mft_cpp.raw

# 2. Save MFT with Rust tool
uffs_mft save --drive F -o f_mft_rust.raw --raw

# 3. Compare record-by-record
cargo run --release -p uffs-diag --bin compare_raw_mft -- \
  f_mft_cpp.raw f_mft_rust.raw
```

**Expected Result:** 100% match (0 differing records).

---

### Workflow 5: Performance Benchmarking

**Goal:** Measure and compare performance across different configurations.

```bash
# 1. Benchmark with different concurrency levels
uffs_mft benchmark-index-lean --drive C --concurrency 2
uffs_mft benchmark-index-lean --drive C --concurrency 8
uffs_mft benchmark-index-lean --drive C --concurrency 32

# 2. Benchmark with different I/O sizes
uffs_mft benchmark-index-lean --drive C --io-size-kb 1024
uffs_mft benchmark-index-lean --drive C --io-size-kb 2048
uffs_mft benchmark-index-lean --drive C --io-size-kb 4096

# 3. Compare with/without bitmap optimization
uffs_mft benchmark-index-lean --drive C
uffs_mft benchmark-index-lean --drive C --no-bitmap

# 4. Compare with/without placeholders
uffs_mft benchmark-index-lean --drive C
uffs_mft benchmark-index-lean --drive C --no-placeholders
```

---

### Workflow 6: Quick Analysis with Scripts

**Goal:** Fast analysis using lightweight scripts without compiling diagnostic tools.

```bash
# 1. Quick Python comparison (no dependencies)
python3 scripts/compare_outputs.py cpp_d.txt rust_d.txt

# 2. Get C++ baseline statistics
rust-script scripts/analyze_cpp_stats.rs cpp_d.txt

# 3. Analyze trial run directory
rust-script scripts/analyze_trial_outputs.rs docs/trial_runs/d_disk

# 4. Detailed count diagnostics with Polars
rust-script scripts/diagnose_mft_counts.rs cpp_d.txt rust_d.txt

# 5. Extract full CSV lines for missing paths
rust-script scripts/find_missing_paths.rs cpp_d.txt rust_d.txt
# Review missing_paths.txt for detailed analysis
```

**When to use scripts vs diagnostic binaries:**
- **Scripts:** Quick analysis, prototyping, one-off investigations
- **Diagnostic binaries:** Production analysis, detailed reports, complex workflows

---

## Summary

The UFFS testing toolkit provides comprehensive coverage for:

✅ **Data Collection** - Automated Windows test data gathering (`trial_run.ps1`)
✅ **Parity Verification** - Field-by-field C++ vs Rust comparison (`compare_scan_parity`)
✅ **Offline Analysis** - Cross-platform MFT inspection without live NTFS (`uffs_mft load`, diagnostic tools)
✅ **Record-Level Debugging** - Inspect specific FRS values through parse pipeline (`dump_mft_records`, `inspect_mft_record_flow`)
✅ **Performance Testing** - Benchmark different configurations (`uffs_mft benchmark-index-lean`)
✅ **Raw Data Validation** - Verify MFT save/load integrity (`compare_raw_mft`)
✅ **Quick Analysis** - Lightweight scripts for rapid comparison (`analyze_trial_outputs.rs`, `compare_outputs.py`)

### Tool Selection Guide

**For quick analysis:**
- Use `compare_outputs.py` or `analyze_trial_outputs.rs` for fast path comparison
- Use `analyze_cpp_stats.rs` for baseline statistics
- Use `analyze_parity_differences.rs` for pattern analysis of differences

**For detailed parity testing:**
- Use `compare_scan_parity` for comprehensive field-by-field analysis with reports
- Use `diagnose_mft_counts.rs` for count-focused diagnostics

**For debugging specific issues:**
- Use `analyze_parity_differences.rs` to identify systematic patterns in missing paths (ADS, file types, path patterns)
- Use `find_missing_paths.rs` to extract full CSV lines for missing paths
- Use `dump_mft_records` to inspect specific FRS headers
- Use `inspect_mft_record_flow` to trace parse pipeline for specific records
- Use `analyze_mft_parents` to understand missing parent directories

**For raw MFT analysis:**
- Use `compare_raw_mft` for record-by-record comparison
- Use `scan_mft_magic` for magic value distribution
- Use `dump_mft_extents` (Windows) for extent layout

All tools are designed to work together in a cohesive workflow, from data collection on Windows to detailed analysis on any platform.
