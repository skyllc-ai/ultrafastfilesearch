# Live vs Offline MFT Processing Parity Investigation

**Date:** 2026-02-01  
**Status:** 🔴 Active Investigation  
**Branch:** `feature/cpp-io-pipeline-port`

## Executive Summary

When processing the same MFT data, **offline processing achieves 100% parity with C++**, but **live Windows scanning loses 40 files**. This proves the parsing, tree-building, and output logic are correct - the issue is in the live I/O processing pipeline.

## Key Finding: 100% Offline Match

| Mode | Unique Paths | Match with C++ |
|------|--------------|----------------|
| **C++ (ground truth)** | 7,058,031 | - |
| **Rust offline** | 7,058,030 | **100.00%** |
| Rust live (all flows) | 7,057,989 | 99.9994% (-40) |

The single "C++ only" path is a header line (`drives? 1 d:`), not an actual file.

### Verification Command (macOS)
```bash
cargo run --release -p uffs-cli --bin uffs -- "*" \
  --mft-file docs/trial_runs/d_disk/D_mft.bin \
  --drive D --parse-algo cpp_port --tree-algo cpp \
  --out docs/trial_runs/d_disk/rust_offline_d.txt
```

## What This Proves

| Component | Status | Evidence |
|-----------|--------|----------|
| MFT Record Parsing | ✅ **Correct** | Offline 100% match |
| Tree Building Algorithm | ✅ **Correct** | Offline 100% match |
| ADS Expansion | ✅ **Correct** | Offline 100% match |
| Compressed/Sparse File Handling | ✅ **Correct** | Offline 100% match |
| Output Formatting | ✅ **Correct** | Offline 100% match |
| Raw I/O Reading | ✅ **Correct** | 40 files ARE in saved MFT |
| **Live Processing Pipeline** | ❌ **Issue Here** | 40 files lost |

## The 40 Missing Files

All 40 files exist in the saved MFT (`D_mft.bin`) but are missing from live output:

### Pattern 1: Rust Incremental Compilation Files (26 files)
- Location: `target/.../incremental/.../s-*-working/`
- Files: `work-products.bin`, `query-cache.bin`, `dep-graph.bin`
- Attributes: Compressed (0x820), Size on Disk = 0 or small

### Pattern 2: Zone.Identifier ADS (14 files)
- Format: `filename.ext:Zone.Identifier`
- Attributes: Size on Disk = 0

### Common Characteristics
- All have **Compressed** flag set (attribute 2080 = 0x820)
- All have **Size on Disk = 0** or very small
- All are **correctly parsed** when processing offline

## Root Cause Hypothesis

The issue is in the **live I/O processing pipeline** - somewhere between:
1. Raw MFT bytes read from disk ✅ (proven: bytes are in saved MFT)
2. Records parsed and output ❌ (40 files missing)

### Eliminated Causes

| Cause | Status | Evidence |
|-------|--------|----------|
| **Skip Range / Bitmap Filtering** | ❌ Eliminated | All test runs use `--no-bitmap` flag |
| **Bitmap Sync Timing** | ❌ Eliminated | Bitmap optimization disabled |
| **MFT Parsing Logic** | ❌ Eliminated | Offline 100% match |
| **Tree Building Logic** | ❌ Eliminated | Offline 100% match |
| **ADS Expansion Logic** | ❌ Eliminated | Offline 100% match |
| **Compressed File Handling** | ❌ Eliminated | Offline 100% match |

> **Note:** The `trial_run.ps1` script runs ALL Rust flows with `--no-bitmap`:
> ```powershell
> uffs.exe "*" --drive $Drive --no-bitmap > rust_d.txt
> uffs.exe "*" --drive $Drive --tree-algo=cpp --no-bitmap > rust_new_d.txt
> uffs.exe "*" --drive $Drive --parse-algo=cpp_port --tree-algo=cpp --no-bitmap > rust_cpp_full_d.txt
> uffs.exe "*" --drive $Drive --parse-algo=cpp_port --tree-algo=cpp --io-algo=cpp --no-bitmap > rust_cpp_io_d.txt
> ```

### Remaining Possible Causes

1. **Chunk Handoff Issue**
   - Chunks containing those 40 files not properly handed to parser
   - Parallel processing race condition

2. **Record Boundary Issue**
   - Records spanning chunk boundaries handled differently
   - Live chunking vs offline complete-file processing

3. **Live-Only Code Path**
   - Some filtering or processing that only applies during live I/O
   - Different data flow between live and offline modes

## Live vs Offline Code Path Differences

| Aspect | Live Path | Offline Path |
|--------|-----------|--------------|
| Data Source | IOCP async reads | File load |
| Chunking | Sliding window chunks | Complete file |
| Parallelism | Parallel chunk processing | Sequential |
| Bitmap | Read and applied live | Not used |
| Skip Ranges | Computed from bitmap | Not used |

## Investigation Plan

- [ ] Compare live vs offline code paths in detail
- [ ] Add diagnostic logging to track the 40 missing FRS numbers
- [ ] Verify chunk boundaries don't split those records incorrectly
- [ ] Check for any live-only filtering or processing logic
- [ ] Trace the data flow for one of the missing files through both paths

## Test Data Location

```
docs/trial_runs/d_disk/
├── D_mft.bin              # Compressed MFT snapshot (427 MB)
├── D_mft.raw              # Raw MFT (5 GB)
├── cpp_d.txt              # C++ output (ground truth)
├── rust_offline_d.txt     # Rust offline output (100% match)
├── rust_d.txt             # Rust live - current algo
├── rust_new_d.txt         # Rust live - cpp tree
├── rust_cpp_full_d.txt    # Rust live - cpp parse + tree
├── rust_cpp_io_d.txt      # Rust live - cpp parse + tree + io
└── missing_paths.txt      # List of 40 missing paths
```

## Related Documents

- [CPP_IO_PIPELINE_PORT.md](../CPP_IO_PIPELINE_PORT.md) - I/O pipeline porting details
- [TESTING_TOOLS_GUIDE.md](./TESTING_TOOLS_GUIDE.md) - Analysis tools reference
- [trial_run.ps1](./trial_run.ps1) - Windows test harness

## Changelog

| Date | Update |
|------|--------|
| 2026-02-01 | Initial investigation - discovered 100% offline match |

