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
$Timestamp = Get-Date -Format "yyyy-MM-dd_HH-mm-ss"
$ProfileOutput = "profile_$Timestamp.json"
$SummaryOutput = "profile_summary_$Timestamp.txt"

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
Write-Host "   Command: samply record --save-only -o $ProfileOutput -- .\uffs.exe `"*`" >null" -ForegroundColor Gray
Write-Host ""

Set-Location $ProfilingDir
$profilePath = Join-Path $ProfilingDir $ProfileOutput

# Run samply
samply record --save-only -o $profilePath -- .\uffs.exe "*" >null

if ($LASTEXITCODE -eq 0) {
    Write-Host ""
    Write-Host "✓ Profiling complete!" -ForegroundColor Green

    # Step 5: Generate profile summary
    Write-Host ""
    Write-Host "Step 5: Generating profile summary..." -ForegroundColor Yellow
    $summaryPath = Join-Path $ProfilingDir $SummaryOutput

    # Parse the profile JSON and extract key metrics
    try {
        $profileJson = Get-Content $profilePath -Raw | ConvertFrom-Json

        # Build summary
        $summary = @"
================================================================================
UFFS PROFILING SUMMARY
================================================================================
Timestamp: $Timestamp
Profile File: $ProfileOutput
Machine: $env:COMPUTERNAME
OS: $([System.Environment]::OSVersion.VersionString)

--------------------------------------------------------------------------------
PROFILE METADATA
--------------------------------------------------------------------------------
"@

        # Extract thread info
        if ($profileJson.threads) {
            $threadCount = $profileJson.threads.Count
            $summary += "`nThreads captured: $threadCount`n"

            foreach ($thread in $profileJson.threads) {
                $threadName = $thread.name
                $sampleCount = if ($thread.samples.length) { $thread.samples.length.Count } else { 0 }
                $summary += "  - $threadName : $sampleCount samples`n"
            }
        }

        # Extract timing info
        if ($profileJson.meta) {
            $startTime = $profileJson.meta.startTime
            $interval = $profileJson.meta.interval
            $summary += "`nSampling interval: ${interval}ms`n"
        }

        # Extract string table size (indicates complexity)
        if ($profileJson.threads[0].stringTable) {
            $stringCount = $profileJson.threads[0].stringTable.Count
            $summary += "Unique symbols: $stringCount`n"
        }

        # Top functions (from first thread's frame table)
        $summary += @"

--------------------------------------------------------------------------------
TOP FUNCTIONS (by sample count - approximate)
--------------------------------------------------------------------------------
Note: For detailed flame graph analysis, use 'just profile-load' on Mac
      or open the JSON in Firefox Profiler (https://profiler.firefox.com)

"@

        # Try to extract function names from string table
        if ($profileJson.threads[0].stringTable) {
            $strings = $profileJson.threads[0].stringTable
            # Look for uffs-related functions
            $uffsStrings = $strings | Where-Object { $_ -match "uffs|mft|polars|arrow" } | Select-Object -First 20
            if ($uffsStrings) {
                $summary += "Key functions found in profile:`n"
                foreach ($fn in $uffsStrings) {
                    $summary += "  • $fn`n"
                }
            }
        }

        $summary += @"

--------------------------------------------------------------------------------
NEXT STEPS
--------------------------------------------------------------------------------
1. Copy USB back to Mac
2. Run: just profile-load
3. Ask Augment to analyze: docs/profiles/$SummaryOutput

================================================================================
"@

        $summary | Out-File -FilePath $summaryPath -Encoding UTF8
        Write-Host "✓ Generated $SummaryOutput" -ForegroundColor Green
    }
    catch {
        Write-Host "⚠ Could not generate detailed summary: $_" -ForegroundColor Yellow
        # Create basic summary
        $basicSummary = @"
================================================================================
UFFS PROFILING SUMMARY
================================================================================
Timestamp: $Timestamp
Profile File: $ProfileOutput
Machine: $env:COMPUTERNAME

Note: Detailed parsing failed. Use Firefox Profiler for analysis.
================================================================================
"@
        $basicSummary | Out-File -FilePath $summaryPath -Encoding UTF8
    }

    # Step 6: Copy profile and summary back to USB
    Write-Host ""
    Write-Host "Step 6: Copying files back to USB..." -ForegroundColor Yellow
    $usbProfileDest = Join-Path $ScriptDir $ProfileOutput
    $usbSummaryDest = Join-Path $ScriptDir $SummaryOutput
    Copy-Item $profilePath $usbProfileDest -Force
    Write-Host "✓ Copied $ProfileOutput to USB" -ForegroundColor Green
    Copy-Item $summaryPath $usbSummaryDest -Force
    Write-Host "✓ Copied $SummaryOutput to USB" -ForegroundColor Green

    Write-Host ""
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
    Write-Host "  ✅ DONE! Profile and summary saved to USB" -ForegroundColor Green
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
    Write-Host ""
    Write-Host "  On Mac, run:" -ForegroundColor Cyan
    Write-Host "  just profile-load" -ForegroundColor White
    Write-Host ""
} else {
    Write-Host ""
    Write-Host "❌ Profiling failed with exit code $LASTEXITCODE" -ForegroundColor Red
}

