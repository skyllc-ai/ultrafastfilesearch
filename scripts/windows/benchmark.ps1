# UFFS Benchmark (unified cold / cached)
# Default: cold start (cache cleared before EACH run)
# With -Cache: warm start (cache persists across runs)
#
# Usage:
#   .\benchmark.ps1 -N 5 -Drive C,D,E,F,G,M,S          # cold (default)
#   .\benchmark.ps1 -N 5 -Drive C,D,E,F,G,M,S -Cache    # warm / cached

param(
    [int]$N = 3,                    # Rounds per test
    [string]$Pattern = "*",         # Search pattern (default: "*" for everything)
    [string[]]$Drive = @(),         # Drives (comma-separated): -Drive C,D,E,F
    [switch]$Cache,                 # Keep cache between runs (warm benchmark)
    [switch]$RustOnly,              # Skip C++ tests
    [switch]$CppOnly,               # Skip Rust tests
    [switch]$NoAll                  # Skip the final "all drives" parallel run
)

$ErrorActionPreference = "Stop"
$UFFS = "$env:USERPROFILE\bin\uffs.exe"
$UFFS_CPP = "$env:USERPROFILE\bin\uffs.com"
$CACHE_DIR = "$env:TEMP\uffs_index_cache"

# Normalize drives to uppercase
$AllDrives = $Drive | ForEach-Object { $_.ToUpper().Trim() } | Where-Object { $_ }

$mode = if ($Cache) { "Cached (warm)" } else { "Cold Start" }

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  UFFS Benchmark — $mode" -ForegroundColor Cyan
Write-Host "  Rounds per test: $N" -ForegroundColor Cyan
Write-Host "  Pattern: $Pattern" -ForegroundColor Cyan
if ($AllDrives.Count -gt 0) {
    Write-Host "  Drives: $($AllDrives -join ', ')" -ForegroundColor Cyan
}
if ($Cache) {
    Write-Host "  (Cache kept between runs)" -ForegroundColor Cyan
} else {
    Write-Host "  (Cache cleared before EACH run)" -ForegroundColor Cyan
}
Write-Host "========================================`n" -ForegroundColor Cyan

# Show cache status when running in cached mode
if ($Cache) {
    if (Test-Path $CACHE_DIR) {
        $cacheFiles = Get-ChildItem $CACHE_DIR -Filter "*.uffs" -ErrorAction SilentlyContinue
        Write-Host "📦 Cache status: $($cacheFiles.Count) cached drive(s)" -ForegroundColor Gray
        foreach ($f in $cacheFiles) {
            $age = [math]::Round(((Get-Date) - $f.LastWriteTime).TotalMinutes, 1)
            Write-Host "   - $($f.Name) (age: ${age}m, size: $([math]::Round($f.Length/1MB, 1))MB)" -ForegroundColor Gray
        }
        Write-Host ""
    } else {
        Write-Host "📦 Cache status: Empty (first run will populate)`n" -ForegroundColor Gray
    }
}

function BenchRun($label, $exePath, [string[]]$argList) {
    Write-Host "▶ $label" -ForegroundColor Yellow
    $times = @()
    1..$N | ForEach-Object {
        # Clear cache before each run in cold mode
        if (-not $Cache) {
            Remove-Item $CACHE_DIR -Recurse -Force -ErrorAction SilentlyContinue
        }

        # Redirect stdout to NUL — we only need wall-clock time and any
        # [TIMING]/[DIAG] lines that go to stderr.  Writing millions of
        # result lines to a temp file added 10-20s of pure I/O overhead per
        # run, and the subsequent Select-String scan of that multi-GB file
        # added another 5-10s.
        $tempErr = [System.IO.Path]::GetTempFileName()

        # Show exact command on first run only
        if ($_ -eq 1) {
            Write-Host "     CMD: $exePath $($argList -join ' ')" -ForegroundColor DarkGray
        }
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $proc = Start-Process -FilePath $exePath -ArgumentList $argList `
                -RedirectStandardOutput "NUL" -RedirectStandardError $tempErr `
                -NoNewWindow -Wait -PassThru
        } catch {
            Write-Host "   ⚠️  Error: $_" -ForegroundColor Red
        }
        $sw.Stop()
        $ms = $sw.Elapsed.TotalMilliseconds
        $times += $ms
        Write-Host "   Run $_`: $([math]::Round($ms/1000, 2))s" -ForegroundColor Gray

        # Extract [TIMING] and [DIAG] lines from stderr (small file)
        try {
            $timingLines = Select-String -Path $tempErr -Pattern '\[TIMING\]|\[DIAG\]' | ForEach-Object { $_.Line }
            if ($timingLines) {
                foreach ($line in $timingLines) {
                    Write-Host "     $line" -ForegroundColor DarkCyan
                }
            }
        } catch {
            # Ignore errors reading temp file
        }

        # Clean up temp file
        Remove-Item $tempErr -Force -ErrorAction SilentlyContinue
    }

    if ($times.Count -gt 0) {
        $avg = ($times | Measure-Object -Average).Average
        $min = ($times | Measure-Object -Minimum).Minimum
        $max = ($times | Measure-Object -Maximum).Maximum
        Write-Host ("{0,-20} avg={1,8:N0} ms   min={2,8:N0}   max={3,8:N0}" -f $label, $avg, $min, $max) -ForegroundColor Green
    }
    Write-Host ""
}

# ============================================
# Run benchmarks based on -Drive parameter
# ============================================

function RunDriveBench($driveLetter) {
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    Write-Host "📁 DRIVE ${driveLetter}:" -ForegroundColor Yellow
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    if (-not $CppOnly) {
        BenchRun "Rust $mode" $UFFS @($Pattern, '--drive', $driveLetter)
    }
    if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
        BenchRun "C++ $mode" $UFFS_CPP @($Pattern, "--drives=$driveLetter")
    }
}

function RunAllDrivesBench() {
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    Write-Host "🌐 ALL DRIVES:" -ForegroundColor Yellow
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    if (-not $CppOnly) {
        BenchRun "Rust $mode" $UFFS @($Pattern)
    }
    if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
        BenchRun "C++ $mode" $UFFS_CPP @($Pattern)
    }
}

# ============================================
# Main execution: run each drive, then "all" at the end
# ============================================

if ($AllDrives.Count -eq 0) {
    # No drives specified: just run all-drives parallel
    RunAllDrivesBench
} else {
    # Run each drive individually
    foreach ($d in $AllDrives) {
        if ($d -eq "ALL") {
            # "ALL" as a drive means run the parallel benchmark
            RunAllDrivesBench
        } else {
            RunDriveBench $d
        }
    }

    # After all individual drives, run the parallel "all drives" benchmark
    if (-not $NoAll) {
        Write-Host "`n" -NoNewline
        Write-Host "╔══════════════════════════════════════╗" -ForegroundColor Magenta
        Write-Host "║  FINAL: All Drives Parallel Run      ║" -ForegroundColor Magenta
        Write-Host "╚══════════════════════════════════════╝" -ForegroundColor Magenta
        RunAllDrivesBench
    }
}

# ============================================
# SUMMARY
# ============================================
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Benchmark Complete ($mode)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
if ($Cache) {
    Write-Host "`nThis measures cached performance (MFT index loaded from disk cache)." -ForegroundColor Gray
    Write-Host "Cache location: $CACHE_DIR" -ForegroundColor Gray
} else {
    Write-Host "`nThis measures fresh MFT reads (no cache)." -ForegroundColor Gray
    Write-Host "Rust saves to cache after each run, but cache is cleared before next run." -ForegroundColor Gray
    Write-Host "Note: OS filesystem cache (RAM) is NOT cleared. Later runs benefit from" -ForegroundColor DarkGray
    Write-Host "MFT data kept in RAM by Windows. C++ has no disk cache (only OS cache)." -ForegroundColor DarkGray
}
