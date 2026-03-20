# UFFS Cache Benchmark Script
# Tests index vs dataframe modes across single drives and all-drives scenarios
# Compares against C++ baseline (uffs.com)

param(
    [int]$N = 5,                    # Rounds per test
    [string]$Pattern = "*",         # Search pattern (default: "*" for everything)
    [switch]$ClearCache,            # Clear cache before running
    [switch]$SkipCpp                # Skip C++ baseline tests
)

$ErrorActionPreference = "Stop"
$UFFS = "$env:USERPROFILE\bin\uffs.exe"
$UFFS_CPP = "$env:USERPROFILE\bin\uffs.com"
$CACHE_DIR = "$env:TEMP\uffs_index_cache"

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  UFFS Cache Benchmark" -ForegroundColor Cyan
Write-Host "  Rounds per test: $N" -ForegroundColor Cyan
Write-Host "  Pattern: $Pattern" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

# Clear cache if requested
if ($ClearCache) {
    Write-Host "🗑️  Clearing cache at $CACHE_DIR..." -ForegroundColor Yellow
    Remove-Item $CACHE_DIR -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "   Cache cleared.`n" -ForegroundColor Green
}

# Show cache status
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

function Bench($label, $cmd) {
    $times = @()
    1..$N | ForEach-Object {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            & $cmd | Out-Null
        } catch {
            Write-Host "   ⚠️  Error: $_" -ForegroundColor Red
        }
        $sw.Stop()
        $times += $sw.Elapsed.TotalMilliseconds
    }
    
    if ($times.Count -gt 0) {
        $avg = ($times | Measure-Object -Average).Average
        $min = ($times | Measure-Object -Minimum).Minimum
        $max = ($times | Measure-Object -Maximum).Maximum
        "{0,-25} avg={1,8:N0} ms   min={2,8:N0}   max={3,8:N0}" -f $label, $avg, $min, $max
    } else {
        "{0,-25} FAILED" -f $label
    }
}

# ============================================
# WARM-UP (populates cache)
# ============================================
Write-Host "🔥 Warm-up (populating cache)..." -ForegroundColor Yellow
& $UFFS search $Pattern --drive F 2>$null | Out-Null
& $UFFS search $Pattern --drive S 2>$null | Out-Null
& $UFFS search $Pattern 2>$null | Out-Null  # All drives
Write-Host "   Done.`n" -ForegroundColor Green

# ============================================
# SINGLE DRIVE: F
# ============================================
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host "📁 DRIVE F: (single drive)" -ForegroundColor White
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Bench "F: index"     { & $UFFS search $Pattern --drive F --query-mode=index }
Bench "F: dataframe" { & $UFFS search $Pattern --drive F --query-mode=dataframe }
Bench "F: default"   { & $UFFS search $Pattern --drive F }
if (-not $SkipCpp -and (Test-Path $UFFS_CPP)) {
    Bench "F: C++ baseline" { & $UFFS_CPP $Pattern --drives=F }
}
Write-Host ""

# ============================================
# SINGLE DRIVE: S
# ============================================
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host "📁 DRIVE S: (single drive)" -ForegroundColor White
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Bench "S: index"     { & $UFFS search $Pattern --drive S --query-mode=index }
Bench "S: dataframe" { & $UFFS search $Pattern --drive S --query-mode=dataframe }
Bench "S: default"   { & $UFFS search $Pattern --drive S }
if (-not $SkipCpp -and (Test-Path $UFFS_CPP)) {
    Bench "S: C++ baseline" { & $UFFS_CPP $Pattern --drives=S }
}
Write-Host ""

# ============================================
# ALL DRIVES (no --drive flag)
# ============================================
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Write-Host "🌐 ALL DRIVES: (no --drive specified)" -ForegroundColor White
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor DarkGray
Bench "ALL: index"     { & $UFFS search $Pattern --query-mode=index }
Bench "ALL: dataframe" { & $UFFS search $Pattern --query-mode=dataframe }
Bench "ALL: default"   { & $UFFS search $Pattern }
if (-not $SkipCpp -and (Test-Path $UFFS_CPP)) {
    Bench "ALL: C++ baseline" { & $UFFS_CPP $Pattern }
}
Write-Host ""

# ============================================
# SUMMARY
# ============================================
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Benchmark Complete" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "`nNotes:" -ForegroundColor Gray
Write-Host "  - 'index' mode: Fast path using MftIndex (lean, no DataFrame)" -ForegroundColor Gray
Write-Host "  - 'dataframe' mode: Converts MftIndex to DataFrame" -ForegroundColor Gray
Write-Host "  - 'default' mode: Auto-selects best path (currently index)" -ForegroundColor Gray
Write-Host "  - First run populates cache; subsequent runs use cache" -ForegroundColor Gray
Write-Host "  - Cache TTL: 10 minutes (600 seconds)" -ForegroundColor Gray
Write-Host "`nCache location: $CACHE_DIR" -ForegroundColor Gray

