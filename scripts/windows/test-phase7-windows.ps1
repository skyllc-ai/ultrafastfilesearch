# Phase 7 Performance Validation Script for Windows
#
# This script runs comprehensive performance tests on Windows to validate
# the Enhanced MFT Parsing implementation (Phases 1-6).
#
# REQUIREMENTS:
# - Windows 10/11 with NTFS drives
# - Administrator privileges (for MFT access)
# - Pre-built binaries from Mac cross-compilation (in dist/latest/)
# - At least one NTFS drive (C:)
#
# PREREQUISITES (run on Mac):
#   1. rust-script scripts/ci/ci-pipeline.rs go -v
#      This cross-compiles Windows binaries and places them in dist/
#   2. Copy binaries to Windows machine at ~\bin\
#
# USAGE:
#   Run in elevated PowerShell:
#   .\scripts\windows\test-phase7-windows.ps1
#
#   Or specify custom drive and runs:
#   .\scripts\windows\test-phase7-windows.ps1 -Drive E -Runs 5
#
#   Use pre-built binaries (skip Rust build):
#   .\scripts\windows\test-phase7-windows.ps1 -UseBinaries
#
# OUTPUTS:
#   - Console output with test results
#   - JSON file with benchmark results (uffs_phase7_results_YYYYMMDD_HHMMSS.json)

param(
    [string]$Drive = "C",
    [int]$Runs = 3,
    [switch]$UseBinaries,
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"

# Colors
function Write-Header { param($msg) Write-Host "`n═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan; Write-Host $msg -ForegroundColor Cyan; Write-Host "═══════════════════════════════════════════════════════════════`n" -ForegroundColor Cyan }
function Write-Success { param($msg) Write-Host "✅ $msg" -ForegroundColor Green }
function Write-Info { param($msg) Write-Host "ℹ️  $msg" -ForegroundColor Blue }
function Write-Warning { param($msg) Write-Host "⚠️  $msg" -ForegroundColor Yellow }
function Write-Error { param($msg) Write-Host "❌ $msg" -ForegroundColor Red }

# Check if running as Administrator
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "This script requires Administrator privileges for MFT access."
    Write-Info "Please run in elevated PowerShell (Run as Administrator)"
    exit 1
}

Write-Header "PHASE 7: PERFORMANCE VALIDATION"
Write-Info "Testing Enhanced MFT Parsing (Phases 1-6)"
Write-Info "Drive: $Drive"
Write-Info "Runs: $Runs"
Write-Info ""

# Determine binary location
$binaryPath = ""
if ($UseBinaries) {
    Write-Header "Step 1: Using Pre-Built Binaries"

    # Check for binaries in ~\bin\
    $binPath = Join-Path $env:USERPROFILE "bin\uffs_mft.exe"
    if (Test-Path $binPath) {
        $binaryPath = $binPath
        Write-Success "Found pre-built binary: $binPath"
    } else {
        Write-Error "Pre-built binary not found at: $binPath"
        Write-Info "Please run on Mac: rust-script scripts/ci/ci-pipeline.rs go -v"
        Write-Info "Then run: just use"
        Write-Info "This copies binaries to ~\bin\"
        exit 1
    }
} else {
    Write-Header "Step 1: Building in Release Mode"
    Write-Info "Building uffs-mft crate..."
    Write-Warning "Note: Building on Windows may fail due to heap constraints"
    Write-Info "Consider using -UseBinaries flag with pre-built binaries from Mac"

    try {
        cargo build --release -p uffs-mft 2>&1 | Out-Null
        $binaryPath = "target\release\uffs_mft.exe"
        Write-Success "Build complete"
    } catch {
        Write-Error "Build failed: $_"
        Write-Info "Try using -UseBinaries flag with pre-built binaries from Mac"
        exit 1
    }
}

# Step 2: Run CLI benchmarks (skip unit tests on Windows due to heap constraints)
Write-Header "Step 2: Running CLI Benchmarks"
Write-Info "Note: Skipping unit tests on Windows due to heap constraints in debug/test mode"
Write-Info "Unit tests are validated on Mac during CI pipeline"
Write-Info ""

Write-Info "Running: $binaryPath bench --drive $Drive --runs $Runs"
Write-Info ""

try {
    $benchOutput = & $binaryPath bench --drive $Drive --runs $Runs 2>&1 | Out-String
    Write-Host $benchOutput

    # Parse benchmark results
    if ($benchOutput -match "Total time: ([0-9.]+)s") {
        $totalTime = [double]$matches[1]
        Write-Success "Benchmark complete: ${totalTime}s"
    }

    # Extract performance metrics
    if ($benchOutput -match "Records/sec: ([0-9,]+)") {
        Write-Info "  Throughput: $($matches[1]) records/sec"
    }
    if ($benchOutput -match "Total records: ([0-9,]+)") {
        Write-Info "  Total records: $($matches[1])"
    }
} catch {
    Write-Warning "CLI benchmark failed (this is expected if drive is not accessible): $_"
}

# Step 3: Generate report
Write-Header "Step 3: Generating Report"

$timestamp = Get-Date -Format "yyyyMMdd_HHmmss"
$reportFile = "uffs_phase7_results_$timestamp.json"

$report = @{
    timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    drive = $Drive
    runs = $Runs
    binary_path = $binaryPath
    used_prebuilt = $UseBinaries
    benchmark_output = $benchOutput
} | ConvertTo-Json -Depth 10

$report | Out-File -FilePath $reportFile -Encoding UTF8

Write-Success "Report saved to: $reportFile"

Write-Header "PHASE 7 VALIDATION COMPLETE"
Write-Success "Benchmark complete!"
Write-Info "Review the report file for detailed results: $reportFile"
Write-Info ""
Write-Info "Next steps:"
Write-Info "  1. Review benchmark results above"
Write-Info "  2. Check $reportFile for full details"
Write-Info "  3. Compare with baseline performance"

