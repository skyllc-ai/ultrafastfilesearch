# Phase 7: Windows Testing Instructions

**Date**: 2026-01-25  
**Purpose**: Step-by-step instructions for running Phase 7 performance validation on Windows

---

## Overview

Since Windows has heap constraints that prevent debug/test builds from compiling, we use a two-step approach:

1. **Mac (cross-compilation)**: Build release binaries and run unit tests
2. **Windows (native)**: Run CLI benchmarks on real NTFS drives

---

## Part 1: Build on Mac (Cross-Compilation)

### Step 1: Run CI Pipeline

The CI pipeline will:
- Run all 47 unit tests (including performance tests)
- Cross-compile Windows binaries using `cargo-xwin`
- Place binaries in `dist/latest/windows-x64/`

**Command**:
```bash
rust-script scripts/ci-pipeline.rs go -v
```

**What this does**:
1. Runs all tests on Mac (where they work fine)
2. Runs clippy, format checks, etc.
3. Cross-compiles for Windows x64 using `cargo-xwin`
4. Places binaries in `dist/latest/windows-x64/`:
   - `uffs.exe` - Main CLI tool
   - `uffs_mft.exe` - MFT reading tool (what we need for Phase 7)
   - `uffs_tui.exe` - TUI (placeholder)
   - `uffs_gui.exe` - GUI (placeholder)

**Expected output**:
```
✅ PHASE 1 COMPLETE: All tests passed!
✅ PHASE 2 COMPLETE: Build and deploy successful!
📦 Binaries in dist/latest/windows-x64/
```

**Time**: ~5-10 minutes (much faster than building on Windows)

---

### Step 2: Verify Binaries

Check that the binaries were created:

```bash
ls -lh dist/latest/windows-x64/
```

**Expected output**:
```
-rwxr-xr-x  1 user  staff   15M Jan 25 16:00 uffs.exe
-rwxr-xr-x  1 user  staff   12M Jan 25 16:00 uffs_mft.exe
-rwxr-xr-x  1 user  staff   8M  Jan 25 16:00 uffs_tui.exe
-rwxr-xr-x  1 user  staff   8M  Jan 25 16:00 uffs_gui.exe
```

---

### Step 3: Copy to Windows

Transfer the `dist/latest/` directory to your Windows machine.

**Options**:
- USB drive
- Network share
- Git (binaries are tracked in the repo)
- Cloud storage (OneDrive, Dropbox, etc.)

**Example using git**:
```bash
# On Mac: Commit and push (CI pipeline does this automatically)
git add dist/latest/
git commit -m "Add Windows binaries for Phase 7 testing"
git push

# On Windows: Pull
git pull
```

---

## Part 2: Test on Windows

### Step 1: Open Elevated PowerShell

**Important**: You need Administrator privileges to access the MFT.

1. Press `Win + X`
2. Select "Windows PowerShell (Admin)" or "Terminal (Admin)"
3. Navigate to the repository directory:
   ```powershell
   cd C:\path\to\UltraFastFileSearch
   ```

---

### Step 2: Run Phase 7 Testing Script

**Using pre-built binaries (recommended)**:
```powershell
.\scripts\test-phase7-windows.ps1 -UseBinaries
```

**Custom drive and runs**:
```powershell
.\scripts\test-phase7-windows.ps1 -UseBinaries -Drive E -Runs 5
```

**What this does**:
1. Checks for pre-built binaries in `dist\latest\windows-x64\`
2. Runs `uffs_mft.exe bench --drive C --runs 3`
3. Collects performance metrics
4. Generates JSON report with results

**Expected output**:
```
═══════════════════════════════════════════════════════════════
PHASE 7: PERFORMANCE VALIDATION
═══════════════════════════════════════════════════════════════

ℹ️  Testing Enhanced MFT Parsing (Phases 1-6)
ℹ️  Drive: C
ℹ️  Runs: 3

═══════════════════════════════════════════════════════════════
Step 1: Using Pre-Built Binaries
═══════════════════════════════════════════════════════════════

✅ Found pre-built binary: dist\latest\windows-x64\uffs_mft.exe

═══════════════════════════════════════════════════════════════
Step 2: Running CLI Benchmarks
═══════════════════════════════════════════════════════════════

Running: dist\latest\windows-x64\uffs_mft.exe bench --drive C --runs 3

[Benchmark output with timing, throughput, etc.]

✅ Benchmark complete: 45.2s
ℹ️  Throughput: 125,000 records/sec
ℹ️  Total records: 5,650,000

═══════════════════════════════════════════════════════════════
Step 3: Generating Report
═══════════════════════════════════════════════════════════════

✅ Report saved to: uffs_phase7_results_20260125_160000.json

═══════════════════════════════════════════════════════════════
PHASE 7 VALIDATION COMPLETE
═══════════════════════════════════════════════════════════════

✅ Benchmark complete!
```

---

### Step 3: Review Results

Check the generated JSON report:

```powershell
cat uffs_phase7_results_*.json
```

**Example report**:
```json
{
  "timestamp": "2026-01-25 16:00:00",
  "drive": "C",
  "runs": 3,
  "binary_path": "dist\\latest\\windows-x64\\uffs_mft.exe",
  "used_prebuilt": true,
  "benchmark_output": "..."
}
```

---

## Alternative: Build on Windows (Not Recommended)

If you want to build on Windows (not recommended due to heap constraints):

```powershell
.\scripts\test-phase7-windows.ps1
```

This will attempt to run `cargo build --release -p uffs-mft` on Windows, which may fail due to heap limitations.

---

## Summary

**Recommended workflow**:
1. **Mac**: `rust-script scripts/ci-pipeline.rs go -v` (5-10 min)
2. **Transfer**: Copy `dist/latest/` to Windows
3. **Windows**: `.\scripts\test-phase7-windows.ps1 -UseBinaries` (1-2 min)

**Total time**: ~10-15 minutes

**Benefits**:
- ✅ Fast cross-compilation on Mac
- ✅ No heap issues on Windows
- ✅ All unit tests run on Mac (where they work)
- ✅ Real-world benchmarks run on Windows (where they matter)

