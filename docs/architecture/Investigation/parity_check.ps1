# parity_check.ps1 - UFFS C++ vs Rust sorted output parity comparison
#
# Purpose:
#   Run both the C++ (uffs.com) and Rust (uffs.exe) binaries with "*" search pattern,
#   sort the output alphabetically, compute SHA256 hashes on both, compare them,
#   and if they differ, sample random differing lines into a diff file.
#
# Usage:
#   .\parity_check.ps1                          # Auto-detect drives, default bin dir
#   .\parity_check.ps1 -Drives C               # Specific drive
#   .\parity_check.ps1 -BinDir "D:\tools"      # Custom binary location
#   .\parity_check.ps1 -SampleSize 50          # Show 50 random diff lines (default: 30)
#
[CmdletBinding()]
param(
    [string[]]$Drives = @(),       # Drives to test (empty = auto-detect NTFS drives)
    [string]$BinDir = "",          # Custom bin directory (default: $HOME\bin)
    [int]$SampleSize = 30,        # Number of random differing lines to sample
    [string]$OutDir = ""           # Output directory (default: current working directory)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── Setup ─────────────────────────────────────────────────────────────────────

if (-not $BinDir) { $BinDir = Join-Path $HOME "bin" }
if (-not $OutDir) { $OutDir = (Get-Location).Path }

$UffsExe = Join-Path $BinDir "uffs.exe"   # Rust
$UffsCom = Join-Path $BinDir "uffs.com"   # C++

$hasRust = Test-Path -LiteralPath $UffsExe
$hasCpp  = Test-Path -LiteralPath $UffsCom

if (-not $hasRust) { Write-Error "Rust binary not found: $UffsExe"; exit 1 }
if (-not $hasCpp)  { Write-Error "C++ binary not found: $UffsCom"; exit 1 }

function Get-NtfsDrives {
    Get-WmiObject Win32_LogicalDisk |
        Where-Object { $_.DriveType -eq 3 -and $_.FileSystem -eq "NTFS" } |
        ForEach-Object { $_.DeviceID.TrimEnd(':') }
}

if ($Drives.Count -eq 0) {
    $Drives = @(Get-NtfsDrives)
    Write-Host "Auto-detected NTFS drives: $($Drives -join ', ')" -ForegroundColor Yellow
}

if ($Drives.Count -eq 0) { Write-Error "No NTFS drives found."; exit 1 }

$timestamp = Get-Date -Format "yyyyMMdd_HHmmss"

Write-Host ""
Write-Host "╔════════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║   UFFS Parity Check — C++ vs Rust          ║" -ForegroundColor Cyan
Write-Host "╚════════════════════════════════════════════╝" -ForegroundColor Cyan
Write-Host ""
Write-Host "  C++ binary : $UffsCom" -ForegroundColor DarkGray
Write-Host "  Rust binary: $UffsExe" -ForegroundColor DarkGray
Write-Host "  Drives     : $($Drives -join ', ')" -ForegroundColor DarkGray
Write-Host "  Sample size: $SampleSize random diff lines" -ForegroundColor DarkGray
Write-Host ""

# ── Per-drive comparison ──────────────────────────────────────────────────────

foreach ($Drive in $Drives) {
    $driveLower = $Drive.ToLower()

    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Cyan
    Write-Host "  Drive $Drive" -ForegroundColor Cyan
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Cyan

    # File paths
    $cppRaw        = Join-Path $OutDir "parity_cpp_${driveLower}_raw_${timestamp}.txt"
    $rustRaw       = Join-Path $OutDir "parity_rust_${driveLower}_raw_${timestamp}.txt"
    $cppSorted     = Join-Path $OutDir "parity_cpp_${driveLower}_sorted_${timestamp}.txt"
    $rustSorted    = Join-Path $OutDir "parity_rust_${driveLower}_sorted_${timestamp}.txt"
    $diffFile      = Join-Path $OutDir "parity_diff_${driveLower}_${timestamp}.txt"

    # ── 1. Run C++ ────────────────────────────────────────────────────────────
    Write-Host "  [1/4] Running C++ scan..." -NoNewline
    $cppStart = Get-Date
    try {
        & cmd.exe /c "`"$UffsCom`" `"*`" --drives=$Drive > `"$cppRaw`" 2>nul"
        $cppExit = $LASTEXITCODE
    } catch {
        $cppExit = -1
    }
    $cppMs = [math]::Round((New-TimeSpan -Start $cppStart -End (Get-Date)).TotalMilliseconds)
    if ($cppExit -eq 0) {
        Write-Host " ✅ ($cppMs ms)" -ForegroundColor Green
    } else {
        Write-Host " ❌ (exit: $cppExit, $cppMs ms)" -ForegroundColor Red
    }

    # ── 2. Run Rust ───────────────────────────────────────────────────────────
    Write-Host "  [2/4] Running Rust scan..." -NoNewline
    $rustStart = Get-Date
    try {
        $savedExp = $env:UFFS_EXPERIMENTAL
        $env:UFFS_EXPERIMENTAL = "1"
        & cmd.exe /c "`"$UffsExe`" `"*`" --drive $Drive --parse-algo=cpp_port --tree-algo=cpp --io-algo=cpp --chunk-algo=cpp --no-cache > `"$rustRaw`" 2>nul"
        $rustExit = $LASTEXITCODE
        $env:UFFS_EXPERIMENTAL = $savedExp
    } catch {
        $rustExit = -1
        $env:UFFS_EXPERIMENTAL = $savedExp
    }
    $rustMs = [math]::Round((New-TimeSpan -Start $rustStart -End (Get-Date)).TotalMilliseconds)
    if ($rustExit -eq 0) {
        Write-Host " ✅ ($rustMs ms)" -ForegroundColor Green
    } else {
        Write-Host " ❌ (exit: $rustExit, $rustMs ms)" -ForegroundColor Red
    }

    # ── 3. Sort both outputs alphabetically ───────────────────────────────────
    Write-Host "  [3/4] Sorting outputs..." -NoNewline
    $sortStart = Get-Date

    # Read, filter blank lines, sort, write
    $cppLines = @(Get-Content -LiteralPath $cppRaw -Encoding UTF8 | Where-Object { $_.Trim() -ne "" })
    $rustLines = @(Get-Content -LiteralPath $rustRaw -Encoding UTF8 | Where-Object { $_.Trim() -ne "" })

    $cppLinesSorted = $cppLines | Sort-Object
    $rustLinesSorted = $rustLines | Sort-Object

    $cppLinesSorted | Set-Content -LiteralPath $cppSorted -Encoding UTF8
    $rustLinesSorted | Set-Content -LiteralPath $rustSorted -Encoding UTF8

    $sortMs = [math]::Round((New-TimeSpan -Start $sortStart -End (Get-Date)).TotalMilliseconds)
    Write-Host " ✅ ($sortMs ms)" -ForegroundColor Green
    Write-Host "    C++ lines : $($cppLines.Count)" -ForegroundColor DarkGray
    Write-Host "    Rust lines: $($rustLines.Count)" -ForegroundColor DarkGray

    # ── 4. SHA256 comparison ──────────────────────────────────────────────────
    Write-Host "  [4/4] Computing SHA256 hashes..." -NoNewline

    $cppHash  = (Get-FileHash -LiteralPath $cppSorted -Algorithm SHA256).Hash
    $rustHash = (Get-FileHash -LiteralPath $rustSorted -Algorithm SHA256).Hash

    if ($cppHash -eq $rustHash) {
        Write-Host " ✅ MATCH" -ForegroundColor Green
        Write-Host ""
        Write-Host "  ╔══════════════════════════════════════════╗" -ForegroundColor Green
        Write-Host "  ║  PARITY: PASS — Outputs are identical    ║" -ForegroundColor Green
        Write-Host "  ╚══════════════════════════════════════════╝" -ForegroundColor Green
        Write-Host "    SHA256: $cppHash" -ForegroundColor DarkGray
        Write-Host ""

        # Clean up raw files since sorted ones are identical
        Remove-Item -LiteralPath $cppRaw -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $rustRaw -Force -ErrorAction SilentlyContinue
    } else {
        Write-Host " ❌ MISMATCH" -ForegroundColor Red
        Write-Host ""
        Write-Host "  ╔══════════════════════════════════════════╗" -ForegroundColor Red
        Write-Host "  ║  PARITY: FAIL — Outputs differ           ║" -ForegroundColor Red
        Write-Host "  ╚══════════════════════════════════════════╝" -ForegroundColor Red
        Write-Host "    C++ SHA256 : $cppHash" -ForegroundColor Yellow
        Write-Host "    Rust SHA256: $rustHash" -ForegroundColor Yellow
        Write-Host ""

        # ── Build diff report ─────────────────────────────────────────────────
        Write-Host "  Building diff report..." -NoNewline

        # Convert to sets for comparison
        $cppSet  = [System.Collections.Generic.HashSet[string]]::new([string[]]$cppLinesSorted)
        $rustSet = [System.Collections.Generic.HashSet[string]]::new([string[]]$rustLinesSorted)

        # Lines only in C++
        $onlyCpp = [System.Collections.Generic.List[string]]::new()
        foreach ($line in $cppLinesSorted) {
            if (-not $rustSet.Contains($line)) {
                $onlyCpp.Add($line)
            }
        }

        # Lines only in Rust
        $onlyRust = [System.Collections.Generic.List[string]]::new()
        foreach ($line in $rustLinesSorted) {
            if (-not $cppSet.Contains($line)) {
                $onlyRust.Add($line)
            }
        }

        # Sample random lines from each side
        $rng = [System.Random]::new()

        function Get-RandomSample {
            param(
                [System.Collections.Generic.List[string]]$Source,
                [int]$Count,
                [System.Random]$Rng
            )
            if ($Source.Count -le $Count) { return $Source.ToArray() }
            $indices = [System.Collections.Generic.HashSet[int]]::new()
            while ($indices.Count -lt $Count) {
                [void]$indices.Add($Rng.Next($Source.Count))
            }
            $result = @()
            foreach ($i in ($indices | Sort-Object)) {
                $result += $Source[$i]
            }
            return $result
        }

        $sampleCpp  = Get-RandomSample -Source $onlyCpp  -Count $SampleSize -Rng $rng
        $sampleRust = Get-RandomSample -Source $onlyRust -Count $SampleSize -Rng $rng

        # Write diff file
        $diffContent = @()
        $diffContent += "# UFFS Parity Diff Report"
        $diffContent += "# Generated: $(Get-Date -Format o)"
        $diffContent += "# Drive: $Drive"
        $diffContent += "#"
        $diffContent += "# C++ sorted : $cppSorted"
        $diffContent += "# Rust sorted: $rustSorted"
        $diffContent += "# C++ SHA256 : $cppHash"
        $diffContent += "# Rust SHA256: $rustHash"
        $diffContent += "#"
        $diffContent += "# C++ total lines : $($cppLines.Count)"
        $diffContent += "# Rust total lines: $($rustLines.Count)"
        $diffContent += "# Lines only in C++ : $($onlyCpp.Count)"
        $diffContent += "# Lines only in Rust: $($onlyRust.Count)"
        $diffContent += "#"
        $diffContent += "# Below: random sample of up to $SampleSize differing lines from each side"
        $diffContent += ""
        $diffContent += "==============================================================================="
        $diffContent += "  LINES ONLY IN C++ ($($onlyCpp.Count) total, showing $($sampleCpp.Count) sampled)"
        $diffContent += "==============================================================================="
        foreach ($line in $sampleCpp) {
            $diffContent += "< $line"
        }
        $diffContent += ""
        $diffContent += "==============================================================================="
        $diffContent += "  LINES ONLY IN RUST ($($onlyRust.Count) total, showing $($sampleRust.Count) sampled)"
        $diffContent += "==============================================================================="
        foreach ($line in $sampleRust) {
            $diffContent += "> $line"
        }
        $diffContent += ""
        $diffContent += "==============================================================================="
        $diffContent += "  SUMMARY"
        $diffContent += "==============================================================================="
        $diffContent += "C++ lines  : $($cppLines.Count)"
        $diffContent += "Rust lines : $($rustLines.Count)"
        $diffContent += "Only in C++: $($onlyCpp.Count)"
        $diffContent += "Only in Rust: $($onlyRust.Count)"

        $diffContent | Set-Content -LiteralPath $diffFile -Encoding UTF8

        Write-Host " ✅" -ForegroundColor Green
        Write-Host ""
        Write-Host "    Only in C++ : $($onlyCpp.Count) lines" -ForegroundColor Yellow
        Write-Host "    Only in Rust: $($onlyRust.Count) lines" -ForegroundColor Yellow
        Write-Host "    Diff report : $diffFile" -ForegroundColor Cyan
        Write-Host ""

        # Show a few sample lines in the console too
        $consolePreview = [math]::Min(5, $sampleCpp.Count)
        if ($consolePreview -gt 0) {
            Write-Host "    Sample lines only in C++ (first $consolePreview):" -ForegroundColor Yellow
            for ($i = 0; $i -lt $consolePreview; $i++) {
                Write-Host "      < $($sampleCpp[$i])" -ForegroundColor DarkYellow
            }
        }
        $consolePreview = [math]::Min(5, $sampleRust.Count)
        if ($consolePreview -gt 0) {
            Write-Host "    Sample lines only in Rust (first $consolePreview):" -ForegroundColor Yellow
            for ($i = 0; $i -lt $consolePreview; $i++) {
                Write-Host "      > $($sampleRust[$i])" -ForegroundColor DarkYellow
            }
        }
        Write-Host ""
    }

    # ── Per-drive summary ─────────────────────────────────────────────────────
    Write-Host "  Output files:" -ForegroundColor DarkGray
    Write-Host "    C++ sorted : $cppSorted" -ForegroundColor DarkGray
    Write-Host "    Rust sorted: $rustSorted" -ForegroundColor DarkGray
    if (Test-Path -LiteralPath $diffFile -ErrorAction SilentlyContinue) {
        Write-Host "    Diff file  : $diffFile" -ForegroundColor DarkGray
    }
    Write-Host ""
}

Write-Host "Done." -ForegroundColor Cyan
