# Phase 7: Quick Start Guide

**TL;DR**: Run these commands on Mac, then copy binaries to Windows for real NTFS testing.

---

## On Mac (What You Can Do Now)

### Option 1: Quick Test (30 seconds)
```bash
cd crates/uffs-mft
cargo test --lib -- --nocapture | grep -E "(running|test result|Extension|Directory|Tree|Total post)"
```

### Option 2: Full CI Pipeline (5-10 minutes)
```bash
rust-script scripts/ci-pipeline.rs go -v
```

This runs all tests + cross-compiles Windows binaries to `dist/latest/windows-x64/`

---

## On Windows (Requires NTFS Drives)

### Prerequisites
1. Copy `dist/latest/` from Mac to Windows (via git/USB/network)
2. Open **elevated PowerShell** (Run as Administrator)
3. Navigate to repository directory

### Run Benchmarks
```powershell
.\scripts\test-phase7-windows.ps1 -UseBinaries
```

Or with custom drive:
```powershell
.\scripts\test-phase7-windows.ps1 -UseBinaries -Drive E -Runs 5
```

---

## Expected Results

### Mac Tests (47 tests)
```
running 47 tests
test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured

Extension index build time: 1.916µs
Extension query time (1000 matches): 83ns
Total post-processing time: 1.168ms
```

### Windows Benchmarks
```
✅ Benchmark complete: 45.2s
ℹ️  Throughput: 125,000 records/sec
ℹ️  Total records: 5,650,000
```

---

## Phase 7 Checklist

- [x] All 47 unit tests passing on Mac
- [x] Performance tests validate targets
- [x] Windows binaries cross-compiled
- [ ] Windows benchmarks run (requires Windows machine)

---

## Documentation

- **Mac Instructions**: `docs/PHASE7_MAC_INSTRUCTIONS.md`
- **Windows Instructions**: `docs/PHASE7_WINDOWS_TESTING.md`
- **Full Validation Report**: `docs/architecture/PHASE7_PERFORMANCE_VALIDATION.md`
- **Project Summary**: `docs/architecture/ENHANCED_MFT_COMPLETE.md`

