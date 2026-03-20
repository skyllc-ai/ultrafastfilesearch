# UFFS Cold Start Benchmark
# Compares Rust vs C++ without cache (fresh MFT read each time)

param(
    [int]$N = 3,                    # Rounds per test
    [string]$Pattern = "*",         # Search pattern (default: "*" for everything)
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
Write-Host "  (Cache cleared before EACH run)" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

function BenchCold($label, $cmd) {
    $times = @()
    1..$N | ForEach-Object {
        # Clear cache before each run
        Remove-Item $CACHE_DIR -Recurse -Force -ErrorAction SilentlyContinue
        
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            & $cmd | Out-Null
        } catch {
            Write-Host "   ⚠️  Error: $_" -ForegroundColor Red
        }
        $sw.Stop()
        $ms = $sw.Elapsed.TotalMilliseconds
        $times += $ms
        Write-Host "   Run $_`: $([math]::Round($ms/1000, 2))s" -ForegroundColor Gray
    }
    
    if ($times.Count -gt 0) {
        $avg = ($times | Measure-Object -Average).Average
        $min = ($times | Measure-Object -Minimum).Minimum
        $max = ($times | Measure-Object -Maximum).Maximum
        Write-Host ("{0,-20} avg={1,8:N0} ms   min={2,8:N0}   max={3,8:N0}" -f $label, $avg, $min, $max) -ForegroundColor White
    }
    Write-Host ""
}

# ============================================
# DRIVE F
# ============================================
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host "📁 DRIVE F:" -ForegroundColor Yellow
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
if (-not $CppOnly) {
    BenchCold "Rust (cold)" { & $UFFS search $Pattern --drive F }
}
if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
    BenchCold "C++ (cold)" { & $UFFS_CPP $Pattern --drives=F }
}

# ============================================
# DRIVE S
# ============================================
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host "📁 DRIVE S:" -ForegroundColor Yellow
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
if (-not $CppOnly) {
    BenchCold "Rust (cold)" { & $UFFS search $Pattern --drive S }
}
if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
    BenchCold "C++ (cold)" { & $UFFS_CPP $Pattern --drives=S }
}

# ============================================
# ALL DRIVES
# ============================================
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host "🌐 ALL DRIVES:" -ForegroundColor Yellow
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
if (-not $CppOnly) {
    BenchCold "Rust (cold)" { & $UFFS search $Pattern }
}
if (-not $RustOnly -and (Test-Path $UFFS_CPP)) {
    BenchCold "C++ (cold)" { & $UFFS_CPP $Pattern }
}

# ============================================
# SUMMARY
# ============================================
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Cold Start Benchmark Complete" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "`nThis measures fresh MFT reads (no cache)." -ForegroundColor Gray
Write-Host "Rust saves to cache after each run, but cache is cleared before next run." -ForegroundColor Gray

