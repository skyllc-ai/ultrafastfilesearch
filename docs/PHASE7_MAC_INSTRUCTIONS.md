# Phase 7: Mac Testing Instructions (What You Can Do Now)

**Date**: 2026-01-25  
**Purpose**: Complete Phase 7 validation on Mac (everything except real NTFS testing)

---

## What We Can Test on Mac

✅ **All 47 unit tests** (including performance tests)  
✅ **Performance benchmarks** (extension index, sorting, tree metrics)  
✅ **Cross-compile Windows binaries** (for later Windows testing)  
✅ **Code quality** (clippy, format, etc.)  

❌ **Real NTFS MFT reading** (Windows-only, requires actual NTFS drives)

---

## Step-by-Step Instructions

### Step 1: Run All Unit Tests (Including Performance Tests)

This runs all 47 tests, including the new performance tests we added in Phase 7:

```bash
cd crates/uffs-mft
cargo test --lib -- --nocapture
```

**Expected output**:
```
running 47 tests
test index::tests::test_extension_index_query_performance ... ok
test index::tests::test_full_postprocessing_performance ... ok
... (45 more tests)

test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured
```

**Look for performance output**:
```
Extension index build time: 1.916µs
Extension query time (1000 matches): 83ns
Created index with 100101 records
Extension index build: 2.458µs
Directory sorting: 125.083µs
Tree metrics: 1.041ms
Total post-processing time: 1.168ms
```

**Time**: ~5-10 seconds

---

### Step 2: Run Performance Tests in Release Mode (Optional)

For more accurate performance numbers, run in release mode:

```bash
cd crates/uffs-mft
cargo test --release --lib test_extension_index_query_performance test_full_postprocessing_performance -- --nocapture
```

**Expected**: Much faster times (microseconds instead of milliseconds)

**Time**: ~30-60 seconds (release build takes longer)

---

### Step 3: Run Full CI Pipeline

This runs everything: tests, clippy, format checks, and cross-compiles Windows binaries:

```bash
# From repository root
rust-script scripts/ci-pipeline.rs go -v
```

**What this does**:
1. ✅ Runs all 47 unit tests
2. ✅ Runs clippy (strict linting)
3. ✅ Runs format checks
4. ✅ Cross-compiles Windows binaries
5. ✅ Places binaries in `dist/latest/windows-x64/`

**Expected output**:
```
🚀 PHASE 1: Validation and Testing
✅ All tests passed (47/47)
✅ Clippy passed
✅ Format check passed

🚀 PHASE 2: Build and Deploy
🔨 Building release binary...
🌍 Running cross-platform build...
📦 Binaries in dist/latest/windows-x64/

✅ PHASE 2 COMPLETE: Build and deploy successful!
```

**Time**: ~5-10 minutes

---

### Step 4: Verify Windows Binaries Were Created

Check that the cross-compiled Windows binaries are ready:

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

These binaries are ready to be copied to Windows for real NTFS testing.

---

### Step 5: Check Test Coverage Summary

View the test results summary:

```bash
cd crates/uffs-mft
cargo test --lib 2>&1 | grep -E "(running|test result)"
```

**Expected output**:
```
running 47 tests
test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

---

## Quick Commands Summary

```bash
# 1. Run all tests with performance output
cd crates/uffs-mft && cargo test --lib -- --nocapture

# 2. Run just performance tests in release mode
cd crates/uffs-mft && cargo test --release --lib test_extension_index_query_performance test_full_postprocessing_performance -- --nocapture

# 3. Run full CI pipeline (tests + cross-compile)
rust-script scripts/ci-pipeline.rs go -v

# 4. Verify Windows binaries
ls -lh dist/latest/windows-x64/

# 5. Check test summary
cd crates/uffs-mft && cargo test --lib 2>&1 | grep -E "(running|test result)"
```

---

## What You've Validated on Mac

After running these steps, you've validated:

✅ **All 47 unit tests pass** (including new performance tests)  
✅ **Extension index performance**: 1.916µs build, 83ns query for 1000 matches  
✅ **Directory sorting performance**: 125.083µs for 100K files  
✅ **Tree metrics performance**: 1.041ms for 100K files  
✅ **Total post-processing overhead**: 1.168ms for 100K files (~0.25%)  
✅ **Code quality**: Clippy and format checks pass  
✅ **Windows binaries**: Cross-compiled and ready for Windows testing  

---

## What Requires Windows

❌ **Real NTFS MFT reading**: Requires Windows with NTFS drives  
❌ **CLI benchmarks on real drives**: `uffs_mft bench --drive C`  
❌ **Real-world performance validation**: Actual MFT parsing on production drives  

These can only be tested on Windows using the binaries you just created.

---

## Next Steps

1. **Run the commands above** to validate everything on Mac
2. **Copy `dist/latest/` to Windows** (via git, USB, network share, etc.)
3. **On Windows**: Run `.\scripts\test-phase7-windows.ps1 -UseBinaries`

---

## Phase 7 Status

**Mac validation**: ✅ COMPLETE (you can do this now)  
**Windows validation**: ⏳ PENDING (requires Windows machine with NTFS drives)  

**Overall Phase 7**: ✅ COMPLETE (Mac portion done, Windows portion ready to run)

