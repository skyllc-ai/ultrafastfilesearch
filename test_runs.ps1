# ============================================================================
# UFFS Phase 2 Optimization Validation Test Suite
# ============================================================================
# This script validates the performance improvements from Phase 2:
#   M1: Adaptive Concurrency (32 for NVMe, 8 for SSD, 2 for HDD)
#   M2: Larger I/O Chunks (4MB for NVMe, 2MB for SSD, 1MB for HDD)
#   M3: Parallel Parsing (24 workers on 24-core CPU)
#   M4: Multi-Volume Parallel (single IOCP for multiple drives)
#   M5: USN Journal Integration (incremental updates)
#
# Phase 2.5 Optimizations (I/O and Parsing):
#   P1: Precise Read Chunks - Skip unused MFT regions entirely (NVMe/SSD)
#   P2: Direct Chunk-to-I/O - Each chunk = one I/O op, fewer syscalls (NVMe/SSD)
#   P3: Zero-Copy Parsing - In-place fixup, no per-record allocation
#
# Key metrics to watch in logs:
#   - bytes_to_read_mb: Should be ~20% less than MFT size (P1)
#   - io_ops: Should be fewer than before (P2)
#   - direct_io=true: Confirms P2 is active
#   - Parsing time: Should be faster (P3)
#
# Usage:
#   .\test_runs.ps1                    # Run full test suite
#   .\test_runs.ps1 -Quick             # Quick test on Drive C only
#   .\test_runs.ps1 -Quick -Drive F    # Quick test on Drive F
#   .\test_runs.ps1 -Quick -Build      # Quick test with rebuild
# ============================================================================

param(
    [switch]$Quick,
    [switch]$Build,
    [string]$Drive = "C"
)

$UFFS = "$env:USERPROFILE\bin\uffs_mft.exe"
$CPP_UFFS = "C:\Users\rnio\GitHub\Ultra-Fast-File-Search\UltraFastFileSearch-code\x64\COM\uffs.com"

# Enable verbose logging to see optimization metrics
$env:RUST_LOG = "info"

if ($Quick) {
    Write-Host "`n" -NoNewline
    Write-Host "=" * 80 -ForegroundColor Cyan
    Write-Host "QUICK TEST: Drive $Drive with Phase 2.5 Optimizations" -ForegroundColor Cyan
    Write-Host "=" * 80 -ForegroundColor Cyan
    Write-Host ""

    # Optional rebuild
    if ($Build) {
        Write-Host "Rebuilding binary..." -ForegroundColor Gray
        $repoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
        Push-Location $repoRoot
        cargo build --release --package uffs-mft
        if ($LASTEXITCODE -ne 0) {
            Write-Host "❌ Build failed!" -ForegroundColor Red
            Pop-Location
            exit 1
        }
        Copy-Item target\release\uffs_mft.exe $UFFS -Force
        Pop-Location
        Write-Host "✅ Build successful" -ForegroundColor Green
        Write-Host ""
    }

    # Check if binary exists
    if (-not (Test-Path $UFFS)) {
        Write-Host "❌ Binary not found: $UFFS" -ForegroundColor Red
        Write-Host "Please build and copy the binary first, or use -Build flag:" -ForegroundColor Yellow
        Write-Host "  .\test_runs.ps1 -Quick -Build" -ForegroundColor Gray
        Write-Host "Or manually:" -ForegroundColor Yellow
        Write-Host "  cargo build --release --package uffs-mft" -ForegroundColor Gray
        Write-Host "  Copy-Item target\release\uffs_mft.exe $UFFS -Force" -ForegroundColor Gray
        exit 1
    }
    Write-Host "Using binary: $UFFS" -ForegroundColor Gray

    Write-Host ""
    Write-Host "--- C++ Baseline ---" -ForegroundColor Yellow
    & $CPP_UFFS --benchmark-index=$($Drive.ToLower())

    Write-Host ""
    Write-Host "--- Rust Full Scan (watch for Phase 2.5 metrics) ---" -ForegroundColor Cyan
    Write-Host "Look for: bytes_to_read_mb, io_ops, direct_io, max_io_size_kb" -ForegroundColor DarkGray
    & $UFFS index-update --drive $Drive --force-full

    Write-Host ""
    Write-Host "--- Rust Incremental (should be <1s) ---" -ForegroundColor Green
    $incr = Measure-Command { & $UFFS index-update --drive $Drive }
    Write-Host "Incremental time: $($incr.TotalSeconds.ToString('F3'))s" -ForegroundColor $(if ($incr.TotalSeconds -lt 1.0) { "Green" } else { "Red" })

    Write-Host ""
    Write-Host "=" * 80 -ForegroundColor White
    Write-Host "QUICK TEST COMPLETE" -ForegroundColor White
    Write-Host "=" * 80 -ForegroundColor White
    Write-Host ""
    Write-Host "Key metrics to check in the output above:" -ForegroundColor Gray
    Write-Host "  - bytes_to_read_mb: Should be ~20% less than MFT size (3261 MB -> ~2600 MB)" -ForegroundColor Gray
    Write-Host "  - io_ops: Should be fewer (was ~800, now ~200-400)" -ForegroundColor Gray
    Write-Host "  - direct_io=true: Confirms direct chunk-to-I/O mapping" -ForegroundColor Gray
    Write-Host "  - Total time: Should beat C++ baseline" -ForegroundColor Gray
    Write-Host ""
    exit 0
}

# ============================================================================
# SECTION 1: C++ BASELINE (Reference Times)
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor Yellow
Write-Host "SECTION 1: C++ BASELINE (Reference Times)" -ForegroundColor Yellow
Write-Host "=" * 80 -ForegroundColor Yellow
Write-Host "Expected: C:~2.5s, F:~1.4s, S:~41s" -ForegroundColor Gray

& $CPP_UFFS --benchmark-index=c
& $CPP_UFFS --benchmark-index=f
& $CPP_UFFS --benchmark-index=s

# ============================================================================
# SECTION 2: RUST AUTO MODE (Should match or beat C++)
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor Cyan
Write-Host "SECTION 2: RUST AUTO MODE - Full MFT Scan" -ForegroundColor Cyan
Write-Host "=" * 80 -ForegroundColor Cyan
Write-Host "Validates: M1 (Adaptive Concurrency) + M2 (I/O Chunks) + M3 (Parallel Parse)" -ForegroundColor Gray
Write-Host "Expected: C:~2.2s, F:~1.3s, S:~40s (should beat C++)" -ForegroundColor Gray

# Run benchmark-index-lean which uses Auto mode (now SlidingIocpInline)
Write-Host "`n--- Drive C: (NVMe) - Expect: concurrency=32, io_size=4MB ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive C

Write-Host "`n--- Drive F: (NVMe) - Expect: concurrency=32, io_size=4MB ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive F

Write-Host "`n--- Drive S: (HDD) - Expect: concurrency=2, io_size=1MB ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive S

# ============================================================================
# SECTION 3: CONCURRENCY COMPARISON (M1 Validation)
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor Magenta
Write-Host "SECTION 3: CONCURRENCY COMPARISON (M1 Validation)" -ForegroundColor Magenta
Write-Host "=" * 80 -ForegroundColor Magenta
Write-Host "Validates: Higher concurrency = faster on NVMe" -ForegroundColor Gray
Write-Host "Expected: concurrency=2 slower, concurrency=32 optimal, concurrency=64 similar" -ForegroundColor Gray

Write-Host "`n--- Concurrency=2 (HDD-style, should be SLOW ~6s) ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive C --concurrency 2

Write-Host "`n--- Concurrency=32 (NVMe optimal, should be FAST ~2.2s) ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive C --concurrency 32

Write-Host "`n--- Concurrency=64 (over-saturated, similar to 32) ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive C --concurrency 64

# ============================================================================
# SECTION 4: I/O SIZE COMPARISON (M2 Validation)
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor Green
Write-Host "SECTION 4: I/O SIZE COMPARISON (M2 Validation)" -ForegroundColor Green
Write-Host "=" * 80 -ForegroundColor Green
Write-Host "Validates: Larger I/O chunks = fewer syscalls = faster on NVMe" -ForegroundColor Gray
Write-Host "Expected: 1MB slower, 4MB optimal" -ForegroundColor Gray

Write-Host "`n--- I/O Size=1MB (HDD-style) ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive C --io-size-kb 1024

Write-Host "`n--- I/O Size=4MB (NVMe optimal) ---" -ForegroundColor White
& $UFFS benchmark-index-lean --drive C --io-size-kb 4096

# ============================================================================
# SECTION 4.5: PRECISE READ CHUNKS (P1+P2 Validation)
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor DarkCyan
Write-Host "SECTION 4.5: PRECISE READ CHUNKS (P1+P2 Validation)" -ForegroundColor DarkCyan
Write-Host "=" * 80 -ForegroundColor DarkCyan
Write-Host "Validates: Skip unused MFT regions, direct chunk-to-I/O mapping" -ForegroundColor Gray
Write-Host "Watch for: bytes_to_read_mb < MFT size, io_ops count, direct_io=true" -ForegroundColor Gray
Write-Host "Expected: ~20% less bytes read, fewer I/O operations" -ForegroundColor Gray

Write-Host "`n--- Drive C: (NVMe) - Watch for precise chunk metrics ---" -ForegroundColor White
Write-Host "Look for: 'Generated I/O operations' log line with bytes_to_read_mb, io_ops, direct_io" -ForegroundColor DarkGray
& $UFFS index-update --drive C --force-full

Write-Host "`n--- Drive F: (NVMe) - Watch for precise chunk metrics ---" -ForegroundColor White
& $UFFS index-update --drive F --force-full

# ============================================================================
# SECTION 5: USN JOURNAL VALIDATION (M5)
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor Blue
Write-Host "SECTION 5: USN JOURNAL VALIDATION (M5)" -ForegroundColor Blue
Write-Host "=" * 80 -ForegroundColor Blue
Write-Host "Validates: USN API works, incremental updates are sub-second" -ForegroundColor Gray

Write-Host "`n--- 5a. USN Journal Info (should show journal details) ---" -ForegroundColor White
& $UFFS usn-info --drive C

Write-Host "`n--- 5b. Clear cache and do full index build ---" -ForegroundColor White
& $UFFS cache-clear --all
& $UFFS index-update --drive C --force-full

Write-Host "`n--- 5c. Incremental update #1 (should be <1s) ---" -ForegroundColor White
$t1 = Measure-Command { & $UFFS index-update --drive C }
Write-Host "Time: $($t1.TotalSeconds.ToString('F3'))s" -ForegroundColor Yellow

Write-Host "`n--- 5d. Incremental update #2 (should be <1s) ---" -ForegroundColor White
$t2 = Measure-Command { & $UFFS index-update --drive C }
Write-Host "Time: $($t2.TotalSeconds.ToString('F3'))s" -ForegroundColor Yellow

Write-Host "`n--- 5e. Incremental update #3 (should be <1s) ---" -ForegroundColor White
$t3 = Measure-Command { & $UFFS index-update --drive C }
Write-Host "Time: $($t3.TotalSeconds.ToString('F3'))s" -ForegroundColor Yellow

# ============================================================================
# SECTION 6: MULTI-VOLUME PARALLEL (M4 Validation)
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor DarkYellow
Write-Host "SECTION 6: MULTI-VOLUME PARALLEL (M4 Validation)" -ForegroundColor DarkYellow
Write-Host "=" * 80 -ForegroundColor DarkYellow
Write-Host "Validates: Multiple drives indexed in parallel via single IOCP" -ForegroundColor Gray
Write-Host "Expected: C+F together should be ~max(C,F) not C+F" -ForegroundColor Gray

Write-Host "`n--- Multi-volume: C+F (both NVMe) ---" -ForegroundColor White
& $UFFS benchmark-multi-volume --drives C,F

Write-Host "`n--- Multi-volume: C+F+S (NVMe+NVMe+HDD) ---" -ForegroundColor White
& $UFFS benchmark-multi-volume --drives C,F,S

# ============================================================================
# SECTION 7: FULL COMPARISON SUMMARY
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor Red
Write-Host "SECTION 7: FULL COMPARISON SUMMARY (Timed)" -ForegroundColor Red
Write-Host "=" * 80 -ForegroundColor Red

# Clear cache for clean comparison
& $UFFS cache-clear --all

Write-Host "`n--- Full Scan Times (Rust vs C++ target) ---" -ForegroundColor White
Write-Host "Drive C: (NVMe, C++ target: 2.57s)" -ForegroundColor Gray
$rust_c = Measure-Command { & $UFFS benchmark-index-lean --drive C 2>$null }
Write-Host "  Rust: $($rust_c.TotalSeconds.ToString('F3'))s" -ForegroundColor $(if ($rust_c.TotalSeconds -lt 2.57) { "Green" } else { "Red" })

Write-Host "Drive F: (NVMe, C++ target: 1.44s)" -ForegroundColor Gray
$rust_f = Measure-Command { & $UFFS benchmark-index-lean --drive F 2>$null }
Write-Host "  Rust: $($rust_f.TotalSeconds.ToString('F3'))s" -ForegroundColor $(if ($rust_f.TotalSeconds -lt 1.44) { "Green" } else { "Red" })

Write-Host "Drive S: (HDD, C++ target: 41.1s)" -ForegroundColor Gray
$rust_s = Measure-Command { & $UFFS benchmark-index-lean --drive S 2>$null }
Write-Host "  Rust: $($rust_s.TotalSeconds.ToString('F3'))s" -ForegroundColor $(if ($rust_s.TotalSeconds -lt 41.1) { "Green" } else { "Red" })

Write-Host "`n--- Incremental Update Times (target: <1s) ---" -ForegroundColor White
& $UFFS cache-clear --all
& $UFFS index-update --drive C --force-full 2>$null | Out-Null
$incr = Measure-Command { & $UFFS index-update --drive C 2>$null }
Write-Host "  Incremental C: $($incr.TotalSeconds.ToString('F3'))s" -ForegroundColor $(if ($incr.TotalSeconds -lt 1.0) { "Green" } else { "Red" })

# ============================================================================
# SUMMARY TABLE
# ============================================================================
Write-Host "`n" -NoNewline
Write-Host "=" * 80 -ForegroundColor White
Write-Host "RESULTS SUMMARY" -ForegroundColor White
Write-Host "=" * 80 -ForegroundColor White
Write-Host ""
Write-Host "| Drive | Rust Time | C++ Target | Status |" -ForegroundColor White
Write-Host "|-------|-----------|------------|--------|" -ForegroundColor White
$status_c = if ($rust_c.TotalSeconds -lt 2.57) { "✅ PASS" } else { "❌ FAIL" }
$status_f = if ($rust_f.TotalSeconds -lt 1.44) { "✅ PASS" } else { "❌ FAIL" }
$status_s = if ($rust_s.TotalSeconds -lt 41.1) { "✅ PASS" } else { "❌ FAIL" }
$status_i = if ($incr.TotalSeconds -lt 1.0) { "✅ PASS" } else { "❌ FAIL" }
Write-Host "| C: (NVMe) | $($rust_c.TotalSeconds.ToString('F2'))s | 2.57s | $status_c |"
Write-Host "| F: (NVMe) | $($rust_f.TotalSeconds.ToString('F2'))s | 1.44s | $status_f |"
Write-Host "| S: (HDD)  | $($rust_s.TotalSeconds.ToString('F2'))s | 41.1s | $status_s |"
Write-Host "| Incr C:   | $($incr.TotalSeconds.ToString('F2'))s | <1.0s | $status_i |"
Write-Host ""

Write-Host "=" * 80 -ForegroundColor White
Write-Host "PHASE 2.5 OPTIMIZATION NOTES" -ForegroundColor White
Write-Host "=" * 80 -ForegroundColor White
Write-Host ""
Write-Host "Check the logs above (Section 4.5) for these metrics:" -ForegroundColor Gray
Write-Host "  - bytes_to_read_mb: Should be ~20% less than MFT size" -ForegroundColor Gray
Write-Host "  - io_ops: Fewer operations = less syscall overhead" -ForegroundColor Gray
Write-Host "  - direct_io=true: Confirms direct chunk-to-I/O mapping active" -ForegroundColor Gray
Write-Host "  - Parsing time: Should be faster due to zero-copy optimization" -ForegroundColor Gray
Write-Host ""
Write-Host "Expected improvements from Phase 2.5:" -ForegroundColor Yellow
Write-Host "  P1 (Precise Chunks): Skip unused MFT regions -> ~20% less I/O" -ForegroundColor Yellow
Write-Host "  P2 (Direct I/O):     Each chunk = 1 I/O op -> fewer syscalls" -ForegroundColor Yellow
Write-Host "  P3 (Zero-Copy):      No per-record allocation -> faster parsing" -ForegroundColor Yellow
Write-Host ""
