# UFFS Cold Start Benchmark
# Compares Rust vs C++ without cache (fresh MFT read each time)

param(
    [int]$N = 3,                    # Rounds per test
    [string]$Pattern = "*",         # Search pattern (default: "*" for everything)
    [string]$Drive = "",            # Single drive to benchmark (e.g., "C", "F", "S")
    [switch]$RustOnly,              # Skip C++ tests
    [switch]$CppOnly                # Skip Rust tests
)

$ErrorActionPreference = "Stop"
$UFFS = "$env:USERPROFILE\bin\uffs.exe"
$UFFS_CPP = "$env:USERPROFILE\bin\uffs.com"
$CACHE_DIR = "$env:TEMP\uffs_index_cache"

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  UFFS Cold Start Benchmark" -ForegroundColor Cyan
Write-Host "  Rounds per test: $N" -ForegroundColor Cyan
Write-Host "  Pattern: $Pattern" -ForegroundColor Cyan
if ($Drive) {
    Write-Host "  Drive: $Drive" -ForegroundColor Cyan
}
Write-Host "  (Cache cleared before EACH run)" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

function BenchCold($label, $cmd) {
    Write-Host "▶ $label" -ForegroundColor Yellow
    $times = @()
    1..$N | ForEach-Object {
        # Clear cache before each run
        Remove-Item $CACHE_DIR -Recurse -Force -ErrorAction SilentlyContinue

        # Use a temp file for stdout to avoid PowerShell pipeline overhead.
        # Capturing 7M+ lines as .NET objects adds 100-150s of overhead.
        $tempOut = [System.IO.Path]::GetTempFileName()
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            & $cmd > $tempOut 2>&1
        } catch {
            Write-Host "   ⚠️  Error: $_" -ForegroundColor Red
        }
        $sw.Stop()
        $ms = $sw.Elapsed.TotalMilliseconds
        $times += $ms
        Write-Host "   Run $_`: $([math]::Round($ms/1000, 2))s" -ForegroundColor Gray

        # Extract [TIMING] lines from the temp file (fast grep, not full load)
        try {
            $timingLines = Select-String -Path $tempOut -Pattern '\[TIMING\]' -SimpleMatch | ForEach-Object { $_.Line }
            if ($timingLines) {
                foreach ($line in $timingLines) {
                    Write-Host "     $line" -ForegroundColor DarkCyan
                }
            }
        } catch {
            # Ignore errors reading temp file
        }

        # Clean up temp file
        Remove-Item $tempOut -Force -ErrorAction SilentlyContinue
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
        # Note: no "search" subcommand - pattern is the default action
        BenchCold "Rust (cold)" { & $UFFS "$Pattern" --drive $driveLetter }
    }
    if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
        BenchCold "C++ (cold)" { & $UFFS_CPP "$Pattern" --drives=$driveLetter }
    }
}

function RunAllDrivesBench() {
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    Write-Host "🌐 ALL DRIVES:" -ForegroundColor Yellow
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    if (-not $CppOnly) {
        # Note: no "search" subcommand - pattern is the default action
        BenchCold "Rust (cold)" { & $UFFS "$Pattern" }
    }
    if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
        BenchCold "C++ (cold)" { & $UFFS_CPP "$Pattern" }
    }
}

if ($Drive) {
    # Single drive specified
    RunDriveBench $Drive.ToUpper()
} else {
    # Default: benchmark F, S, then all
    RunDriveBench "F"
    RunDriveBench "S"
    RunAllDrivesBench
}

# ============================================
# SUMMARY
# ============================================
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Cold Start Benchmark Complete" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "`nThis measures fresh MFT reads (no cache)." -ForegroundColor Gray
Write-Host "Rust saves to cache after each run, but cache is cleared before next run." -ForegroundColor Gray

