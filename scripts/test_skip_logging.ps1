#!/usr/bin/env pwsh
# Test script to verify skip_begin and skip_end logging
# This script runs a quick scan with --no-bitmap and captures detailed logs

param(
    [string]$Drive = "D",
    [string]$UffsExe = "$HOME\bin\uffs.exe"
)

$ErrorActionPreference = "Stop"

Write-Host "🔍 Testing skip_begin/skip_end logging with --no-bitmap flag" -ForegroundColor Cyan
Write-Host ""

# Create output directory
$OutputDir = "docs/trial_runs/skip_logging_test"
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

# Set RUST_LOG to enable debug logging
$env:RUST_LOG = "uffs_mft=debug,uffs_cli=info"

Write-Host "📊 Running scan with --no-bitmap and debug logging..." -ForegroundColor Yellow
Write-Host "   Drive: $Drive" -ForegroundColor Gray
Write-Host "   RUST_LOG: $env:RUST_LOG" -ForegroundColor Gray
Write-Host ""

# Run the scan
$LogFile = "$OutputDir/scan_with_logging.log"
$OutputFile = "$OutputDir/scan_output.txt"

$StartTime = Get-Date

try {
    & $UffsExe "*" --drive $Drive --no-bitmap --format csv --columns path `
        > $OutputFile 2> $LogFile
    
    $ExitCode = $LASTEXITCODE
    $EndTime = Get-Date
    $Duration = ($EndTime - $StartTime).TotalSeconds
    
    Write-Host "✅ Scan completed in $([math]::Round($Duration, 1))s (exit code: $ExitCode)" -ForegroundColor Green
} catch {
    Write-Host "❌ Scan failed: $_" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "📄 Analyzing logs..." -ForegroundColor Cyan

# Check if bitmap was disabled
$BitmapDisabled = Select-String -Path $LogFile -Pattern "Bitmap optimization DISABLED" -Quiet
if ($BitmapDisabled) {
    Write-Host "✅ Bitmap optimization was DISABLED" -ForegroundColor Green
} else {
    Write-Host "⚠️  Bitmap optimization status unclear" -ForegroundColor Yellow
}

# Check for chunks with skips
$ChunksWithSkips = Select-String -Path $LogFile -Pattern "Chunk has skip_begin or skip_end > 0"
if ($ChunksWithSkips) {
    $SkipCount = ($ChunksWithSkips | Measure-Object).Count
    Write-Host "⚠️  Found $SkipCount chunks with skip_begin or skip_end > 0" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "First 10 chunks with skips:" -ForegroundColor Gray
    $ChunksWithSkips | Select-Object -First 10 | ForEach-Object {
        Write-Host "   $_" -ForegroundColor Gray
    }
} else {
    Write-Host "✅ No chunks with skip_begin or skip_end > 0" -ForegroundColor Green
}

# Check for skipped entire chunks
$SkippedChunks = Select-String -Path $LogFile -Pattern "SKIPPING ENTIRE CHUNK"
if ($SkippedChunks) {
    $SkipCount = ($SkippedChunks | Measure-Object).Count
    Write-Host "❌ Found $SkipCount chunks that were ENTIRELY SKIPPED" -ForegroundColor Red
    Write-Host ""
    Write-Host "First 10 skipped chunks:" -ForegroundColor Gray
    $SkippedChunks | Select-Object -First 10 | ForEach-Object {
        Write-Host "   $_" -ForegroundColor Gray
    }
} else {
    Write-Host "✅ No chunks were entirely skipped" -ForegroundColor Green
}

# Check for total records skipped
$RecordsSkipped = Select-String -Path $LogFile -Pattern "records will be skipped based on bitmap"
if ($RecordsSkipped) {
    Write-Host ""
    Write-Host "⚠️  Records skipped summary:" -ForegroundColor Yellow
    $RecordsSkipped | ForEach-Object {
        Write-Host "   $_" -ForegroundColor Gray
    }
}

# Check read plan
$ReadPlan = Select-String -Path $LogFile -Pattern "Read plan generated"
if ($ReadPlan) {
    Write-Host ""
    Write-Host "📊 Read plan:" -ForegroundColor Cyan
    $ReadPlan | ForEach-Object {
        Write-Host "   $_" -ForegroundColor Gray
    }
}

Write-Host ""
Write-Host "📁 Output files:" -ForegroundColor Cyan
Write-Host "   Log:    $LogFile" -ForegroundColor Gray
Write-Host "   Output: $OutputFile" -ForegroundColor Gray
Write-Host ""
Write-Host "💡 To view full logs:" -ForegroundColor Yellow
Write-Host "   cat $LogFile | grep -E '(skip|Skip|SKIP|bitmap|Bitmap)'" -ForegroundColor Gray

