# UFFS Testing & Analysis Tools Guide

**Complete reference for all testing, diagnostic, and analysis tools in the UFFS project.**

---

## Table of Contents

1. [Overview](#overview)
2. [Data Collection](#data-collection)
3. [Main Binary Testing Capabilities](#main-binary-testing-capabilities)
4. [Diagnostic Binaries (uffs-diag)](#diagnostic-binaries-uffs-diag)
5. [Analysis Scripts](#analysis-scripts)
6. [Common Workflows](#common-workflows)

---

## Overview

The UFFS project includes a comprehensive suite of testing and analysis tools designed to:

- **Collect test data** from Windows systems (MFT snapshots + scan outputs)
- **Compare C++ vs Rust implementations** for parity verification
- **Diagnose MFT parsing issues** with offline analysis tools
- **Benchmark performance** across different algorithms and drive types
- **Analyze differences** in scan outputs with detailed statistics

### Tool Categories

| Category | Tools | Purpose |
|----------|-------|---------|
| **Data Collection** | `trial_run.ps1` | Collect MFT snapshots and scan outputs on Windows |
| **Main Binaries** | `uffs`, `uffs_mft` | Production binaries with built-in testing/benchmarking |
| **Diagnostic Tools** | 9 binaries in `uffs-diag` | Offline MFT analysis and comparison |
| **Analysis Scripts** | `scripts/*.rs`, `scripts/*.py` | Automated comparison and analysis |

---

## Data Collection

### `trial_run.ps1` - Comprehensive Data Collection Script

**Location:** `docs/architecture/Investigation/trial_run.ps1`

**Purpose:** Collect MFT snapshots and run four different scan flows for comparison testing.

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
- `rust_c.txt` - Rust current implementation
- `cpp_c.txt` - C++ reference implementation (uffs.com)
- `rust_new_c.txt` - Rust with C++ tree algorithm
- `rust_cpp_full_c.txt` - Rust with full C++ port (parsing + tree)

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
   uffs.exe "*" --drive C --out rust_c.txt
   ```

2. **C++** - Original C++ implementation (baseline)
   ```powershell
   uffs.com "*" C: > cpp_c.txt
   ```

3. **Rust (new tree)** - Rust with C++ tree algorithm port
   ```powershell
   uffs.exe "*" --drive C --tree-algo cpp --out rust_new_c.txt
   ```

4. **Rust (cpp full)** - Rust with both C++ parsing AND tree algorithms
   ```powershell
   uffs.exe "*" --drive C --parse-algo cpp_port --tree-algo cpp --out rust_cpp_full_c.txt
   ```

---

## Main Binary Testing Capabilities

### `uffs` - Main CLI Binary

**Location:** `crates/uffs-cli`

#### Testing-Related Switches

```bash
# Algorithm selection for parity testing
uffs "*" --tree-algo cpp          # Use C++ tree algorithm
uffs "*" --parse-algo cpp_port    # Use C++ parsing algorithm
uffs "*" --tree-algo cpp --parse-algo cpp_port  # Full C++ port

# Performance testing
uffs "*.txt" --benchmark          # Show detailed timing breakdown
uffs "*.txt" --profile            # Enable profiling output
uffs "*.txt" --debug-tree         # Show tree building debug info

# Cache control
uffs "*" --no-cache               # Force fresh MFT read (ignore cache)

# Bitmap control
uffs "*" --no-bitmap              # Read entire MFT (don't use bitmap optimization)

# Output for comparison
uffs "*" --out results.csv        # Save to CSV for comparison
uffs "*" --format json            # JSON output

