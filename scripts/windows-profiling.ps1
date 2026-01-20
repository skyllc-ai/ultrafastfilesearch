# UFFS Windows Profiling Script
# Run this from the USB drive: G:\uffs_profiling\run-profiling.ps1
#
# PROFILING BEST PRACTICES:
# - Uses samply with ETW (Event Tracing for Windows) for low-overhead sampling
# - Captures both on-CPU and off-CPU (context switch) samples
# - Uses Microsoft Symbol Server for Windows library symbols
#
# SYMBOLICATION WORKFLOW:
# - PDB file is already on USB (built on Mac, copied by `just profile-usb`)
# - Raw profile is saved with --save-only (captures addresses, no symbols)
# - Profile JSON is copied back to USB alongside the existing PDB
# - On Mac: `samply load` symbolicates using the PDB in the same directory
# - Firefox Profiler fetches symbols from samply's local server
#
# NOTE: PDB is NOT copied to C:\profiling - it stays on USB and is only
#       used on Mac during `samply load` for symbolication.

$ErrorActionPreference = "Stop"

Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Cyan
Write-Host "  UFFS Windows Profiling Script (v2)" -ForegroundColor Cyan
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

# Samply options for best profiling:
# --rate 1000       : 1000 Hz sampling (1ms interval) - good balance of detail vs overhead
# --save-only       : Save profile without opening browser (for offline analysis)
# Note: Context switches are captured automatically on Windows via ETW

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

# Step 3: Copy uffs.exe from USB (PDB stays on USB for Mac symbolication)
Write-Host ""
Write-Host "Step 3: Copying uffs.exe from USB..." -ForegroundColor Yellow
$exeSrc = Join-Path $ScriptDir "uffs.exe"
$exeDst = Join-Path $ProfilingDir "uffs.exe"
if (Test-Path $exeSrc) {
    Write-Host "   Copying uffs.exe..." -ForegroundColor Gray
    Copy-Item $exeSrc $exeDst -Force
    Write-Host "   ✓ uffs.exe" -ForegroundColor Green
} else {
    Write-Host "   ❌ uffs.exe not found on USB!" -ForegroundColor Red
    exit 1
}

# Check if PDB exists on USB (for info only - it's used on Mac, not here)
$pdbOnUsb = Test-Path (Join-Path $ScriptDir "uffs.pdb")
if ($pdbOnUsb) {
    Write-Host "   ✓ uffs.pdb found on USB (will be used for symbolication on Mac)" -ForegroundColor Green
} else {
    Write-Host "   ⚠ uffs.pdb not on USB - profile will lack uffs.exe symbols" -ForegroundColor Yellow
}

# Step 4: Run profiling
Write-Host ""
Write-Host "Step 4: Running profiling (all NTFS drives)..." -ForegroundColor Yellow
Write-Host "   Command: samply record --rate 1000 --save-only -o $ProfileOutput -- .\uffs.exe `"*`" >`$null" -ForegroundColor Gray
Write-Host ""

Set-Location $ProfilingDir
$profilePath = Join-Path $ProfilingDir $ProfileOutput

# Measure execution time
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

# Run samply with optimized settings
# --rate 1000: 1000 Hz sampling rate (1ms interval)
# --save-only: Don't open browser, just save the profile
samply record --rate 1000 --save-only -o $profilePath -- .\uffs.exe "*" >$null

$stopwatch.Stop()
$elapsedSeconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 1)

if ($LASTEXITCODE -eq 0) {
    Write-Host ""
    Write-Host "✓ Profiling complete! (${elapsedSeconds}s)" -ForegroundColor Green

    # Get profile file size
    $profileSize = [math]::Round((Get-Item $profilePath).Length / 1MB, 1)
    Write-Host "   Profile size: ${profileSize} MB" -ForegroundColor Gray

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
Profile Size: ${profileSize} MB
Execution Time: ${elapsedSeconds} seconds
Machine: $env:COMPUTERNAME
OS: $([System.Environment]::OSVersion.VersionString)
PDB on USB: $pdbOnUsb

--------------------------------------------------------------------------------
PROFILE METADATA
--------------------------------------------------------------------------------
"@

        # Extract thread info
        if ($profileJson.threads) {
            $threadCount = $profileJson.threads.Count
            $summary += "`nThreads captured: $threadCount`n"

            # Count total samples
            $totalSamples = 0
            foreach ($thread in $profileJson.threads) {
                $threadName = $thread.name
                $sampleCount = 0
                if ($thread.samples -and $thread.samples.stack) {
                    $sampleCount = $thread.samples.stack.Count
                }
                $totalSamples += $sampleCount
                if ($sampleCount -gt 0) {
                    $summary += "  - $threadName : $sampleCount samples`n"
                }
            }
            $summary += "`nTotal samples: $totalSamples`n"
        }

        # Extract timing info
        if ($profileJson.meta) {
            $interval = $profileJson.meta.interval
            $summary += "Sampling interval: ${interval}ms`n"
        }

        $summary += @"

--------------------------------------------------------------------------------
SYMBOLICATION INFO
--------------------------------------------------------------------------------
This profile contains raw addresses that need symbolication on Mac.

For uffs.exe symbols:
  - PDB file (uffs.pdb) is already on USB (built on Mac)
  - samply load will use it automatically when in same directory

For Windows library symbols:
  - samply fetches from Microsoft Symbol Server
  - Requires internet connection when viewing

--------------------------------------------------------------------------------
NEXT STEPS
--------------------------------------------------------------------------------
1. Eject USB and bring back to Mac
2. Run: just profile-load
3. Firefox Profiler will open with symbolicated profile

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
Execution Time: ${elapsedSeconds} seconds
Machine: $env:COMPUTERNAME

Note: Detailed parsing failed. Use Firefox Profiler for analysis.
================================================================================
"@
        $basicSummary | Out-File -FilePath $summaryPath -Encoding UTF8
    }

    # Step 6: Copy profile and summary back to USB (PDB is already there)
    Write-Host ""
    Write-Host "Step 6: Copying results back to USB..." -ForegroundColor Yellow

    # Copy profile
    $usbProfileDest = Join-Path $ScriptDir $ProfileOutput
    Copy-Item $profilePath $usbProfileDest -Force
    Write-Host "✓ Copied $ProfileOutput to USB" -ForegroundColor Green

    # Copy summary
    $usbSummaryDest = Join-Path $ScriptDir $SummaryOutput
    Copy-Item $summaryPath $usbSummaryDest -Force
    Write-Host "✓ Copied $SummaryOutput to USB" -ForegroundColor Green

    Write-Host ""
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
    Write-Host "  ✅ DONE! Profile and summary saved to USB" -ForegroundColor Green
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Files on USB:" -ForegroundColor Cyan
    Write-Host "    • $ProfileOutput (profile data - NEW)" -ForegroundColor White
    Write-Host "    • uffs.pdb (symbols - already on USB)" -ForegroundColor Gray
    Write-Host "    • $SummaryOutput (summary - NEW)" -ForegroundColor White
    Write-Host ""
    Write-Host "  On Mac, run:" -ForegroundColor Cyan
    Write-Host "    just profile-load" -ForegroundColor White
    Write-Host ""
} else {
    Write-Host ""
    Write-Host "❌ Profiling failed with exit code $LASTEXITCODE" -ForegroundColor Red
    Write-Host "   Check that uffs.exe runs correctly: .\uffs.exe `"*`"" -ForegroundColor Yellow
}

