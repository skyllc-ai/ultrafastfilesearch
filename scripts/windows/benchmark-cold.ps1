# UFFS Cold Start Benchmark
# Compares Rust vs C++ without cache (fresh MFT read each time)
#
# Usage:
#   .\benchmark-cold.ps1 -N 5 -Drive C,D,E,F,G,M,S

param(
    [int]$N = 3,                    # Rounds per test
    [string]$Pattern = "*",         # Search pattern (default: "*" for everything)
    [string[]]$Drive = @(),         # Drives (comma-separated): -Drive C,D,E,F
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

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  UFFS Cold Start Benchmark" -ForegroundColor Cyan
Write-Host "  Rounds per test: $N" -ForegroundColor Cyan
Write-Host "  Pattern: $Pattern" -ForegroundColor Cyan
if ($AllDrives.Count -gt 0) {
    Write-Host "  Drives: $($AllDrives -join ', ')" -ForegroundColor Cyan
}
Write-Host "  (Cache cleared before EACH run)" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

function BenchCold($label, $exePath, [string[]]$argList) {
    Write-Host "▶ $label" -ForegroundColor Yellow
    $times = @()
    1..$N | ForEach-Object {
        # Clear cache before each run
        Remove-Item $CACHE_DIR -Recurse -Force -ErrorAction SilentlyContinue

        # Use Start-Process with raw file redirect to bypass PowerShell's
        # encoding layer.  PowerShell's > operator re-encodes every line
        # through its UTF-16 pipeline, adding 20-30s for large outputs.
        $tempOut = [System.IO.Path]::GetTempFileName()
        $tempErr = [System.IO.Path]::GetTempFileName()

        # Show exact command on first run only
        if ($_ -eq 1) {
            Write-Host "     CMD: $exePath $($argList -join ' ')" -ForegroundColor DarkGray
        }
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $proc = Start-Process -FilePath $exePath -ArgumentList $argList `
                -RedirectStandardOutput $tempOut -RedirectStandardError $tempErr `
                -NoNewWindow -Wait -PassThru
        } catch {
            Write-Host "   ⚠️  Error: $_" -ForegroundColor Red
        }
        $sw.Stop()
        $ms = $sw.Elapsed.TotalMilliseconds
        $times += $ms
        Write-Host "   Run $_`: $([math]::Round($ms/1000, 2))s" -ForegroundColor Gray

        # Extract [TIMING] and [DIAG] lines from the temp file
        try {
            $timingLines = Select-String -Path $tempOut -Pattern '\[TIMING\]|\[DIAG\]' | ForEach-Object { $_.Line }
            if ($timingLines) {
                foreach ($line in $timingLines) {
                    Write-Host "     $line" -ForegroundColor DarkCyan
                }
            }
        } catch {
            # Ignore errors reading temp file
        }

        # Clean up temp files
        Remove-Item $tempOut -Force -ErrorAction SilentlyContinue
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
        BenchCold "Rust (cold)" $UFFS @($Pattern, '--drive', $driveLetter)
    }
    if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
        BenchCold "C++ (cold)" $UFFS_CPP @($Pattern, "--drives=$driveLetter")
    }
}

function RunAllDrivesBench() {
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    Write-Host "🌐 ALL DRIVES:" -ForegroundColor Yellow
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
    if (-not $CppOnly) {
        BenchCold "Rust (cold)" $UFFS @($Pattern)
    }
    if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
        BenchCold "C++ (cold)" $UFFS_CPP @($Pattern)
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
Write-Host "  Cold Start Benchmark Complete" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "`nThis measures fresh MFT reads (no cache)." -ForegroundColor Gray
Write-Host "Rust saves to cache after each run, but cache is cleared before next run." -ForegroundColor Gray

