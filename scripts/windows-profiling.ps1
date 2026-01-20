# UFFS Windows Profiling Script
# Run this from the USB drive: G:\uffs_profiling\run-profiling.ps1

$ErrorActionPreference = "Stop"

Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Cyan
Write-Host "  UFFS Windows Profiling Script" -ForegroundColor Cyan
Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Cyan
Write-Host ""

# Detect USB drive (where this script is running from)
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$UsbDrive = (Get-Item $ScriptDir).PSDrive.Name + ":"
Write-Host "✓ USB drive detected: $UsbDrive" -ForegroundColor Green

# Configuration
$ProfilingDir = "C:\profiling"
$ProfileOutput = "profile.json"

# Step 1: Check if samply is installed
Write-Host ""
Write-Host "Step 1: Checking samply installation..." -ForegroundColor Yellow
$samply = Get-Command samply -ErrorAction SilentlyContinue
if (-not $samply) {
    Write-Host "❌ samply not found. Installing..." -ForegroundColor Red
    Write-Host "   Running: cargo install --locked samply" -ForegroundColor Gray
    cargo install --locked samply
    if ($LASTEXITCODE -ne 0) {
        Write-Host "❌ Failed to install samply. Make sure Rust/Cargo is installed." -ForegroundColor Red
        exit 1
    }
}
Write-Host "✓ samply is installed" -ForegroundColor Green

# Step 2: Create profiling directory
Write-Host ""
Write-Host "Step 2: Setting up profiling directory..." -ForegroundColor Yellow
if (-not (Test-Path $ProfilingDir)) {
    New-Item -ItemType Directory -Path $ProfilingDir | Out-Null
    Write-Host "✓ Created $ProfilingDir" -ForegroundColor Green
} else {
    Write-Host "✓ $ProfilingDir already exists" -ForegroundColor Green
}

# Step 3: Copy files from USB
Write-Host ""
Write-Host "Step 3: Copying files from USB..." -ForegroundColor Yellow
$SourceFiles = @("uffs.exe", "uffs.pdb")
foreach ($file in $SourceFiles) {
    $src = Join-Path $ScriptDir $file
    $dst = Join-Path $ProfilingDir $file
    if (Test-Path $src) {
        Write-Host "   Copying $file..." -ForegroundColor Gray
        Copy-Item $src $dst -Force
        Write-Host "   ✓ $file" -ForegroundColor Green
    } else {
        Write-Host "   ⚠ $file not found on USB" -ForegroundColor Yellow
    }
}

# Step 4: Run profiling
Write-Host ""
Write-Host "Step 4: Running profiling..." -ForegroundColor Yellow
Write-Host "   Command: samply record --save-only -o $ProfileOutput -- .\uffs.exe `"*`" --drives=C" -ForegroundColor Gray
Write-Host ""

Set-Location $ProfilingDir
$profilePath = Join-Path $ProfilingDir $ProfileOutput

# Run samply
samply record --save-only -o $profilePath -- .\uffs.exe "*" --drives=C

if ($LASTEXITCODE -eq 0) {
    Write-Host ""
    Write-Host "✓ Profiling complete!" -ForegroundColor Green
    
    # Step 5: Copy profile back to USB
    Write-Host ""
    Write-Host "Step 5: Copying profile back to USB..." -ForegroundColor Yellow
    $usbDest = Join-Path $ScriptDir $ProfileOutput
    Copy-Item $profilePath $usbDest -Force
    Write-Host "✓ Copied $ProfileOutput to USB" -ForegroundColor Green
    
    Write-Host ""
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
    Write-Host "  ✅ DONE! Profile saved to USB" -ForegroundColor Green
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
    Write-Host ""
    Write-Host "  On Mac, run:" -ForegroundColor Cyan
    Write-Host "  samply load /Volumes/UFFSPRO/uffs_profiling/$ProfileOutput" -ForegroundColor White
    Write-Host ""
} else {
    Write-Host ""
    Write-Host "❌ Profiling failed with exit code $LASTEXITCODE" -ForegroundColor Red
}

