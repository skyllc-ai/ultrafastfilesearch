# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Rust MFT Statistics Collection Script
#
# This script runs `uffs-mft info --deep` for each NTFS drive and summarizes the results.
# Run this on Windows with administrator privileges.
#
# Usage:
#   .\scripts\windows\rust_mft_stats.ps1
#   .\scripts\windows\rust_mft_stats.ps1 -Drives C,D,E
#   .\scripts\windows\rust_mft_stats.ps1 -UffsMftPath "C:\path\to\uffs-mft.exe"

param(
    [string[]]$Drives = @(),
    [string]$UffsMftPath = ""
)

Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  Rust UFFS MFT Statistics Collection" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""

# Check if running as admin
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "WARNING: Not running as Administrator. MFT reading requires admin privileges." -ForegroundColor Yellow
    Write-Host ""
}

# Find uffs-mft executable
if ([string]::IsNullOrEmpty($UffsMftPath) -or -not (Test-Path $UffsMftPath)) {
    # Try common locations - use full paths or .\prefix
    $candidates = @(
        ".\uffs-mft.exe",
        ".\target\release\uffs-mft.exe",
        ".\target\debug\uffs-mft.exe",
        "$env:USERPROFILE\bin\uffs-mft.exe",
        "$env:USERPROFILE\.cargo\bin\uffs-mft.exe"
    )
    $UffsMftPath = ""
    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            $UffsMftPath = (Resolve-Path $candidate).Path
            break
        }
    }
}

if ([string]::IsNullOrEmpty($UffsMftPath) -or -not (Test-Path $UffsMftPath)) {
    Write-Host "ERROR: Cannot find uffs-mft.exe" -ForegroundColor Red
    Write-Host "Please build it first: cargo build --release -p uffs-mft" -ForegroundColor Yellow
    Write-Host "Or specify path: .\rust_mft_stats.ps1 -UffsMftPath 'C:\path\to\uffs-mft.exe'" -ForegroundColor Yellow
    exit 1
}

Write-Host "Using: $UffsMftPath" -ForegroundColor Green
Write-Host ""

# Get NTFS drives if not specified
if ($Drives.Count -eq 0) {
    $Drives = Get-WmiObject Win32_LogicalDisk | 
        Where-Object { $_.DriveType -eq 3 -and $_.FileSystem -eq "NTFS" } | 
        ForEach-Object { $_.DeviceID.TrimEnd(':') }
    Write-Host "Detected NTFS drives: $($Drives -join ', ')" -ForegroundColor Green
}

Write-Host ""

# Collect stats for each drive
$results = @{}
$totalFiles = 0
$totalDirs = 0
$totalTime = [TimeSpan]::Zero

foreach ($drive in $Drives) {
    $driveLetter = $drive.ToUpper().TrimEnd(':')
    Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Yellow
    Write-Host "  Processing drive $driveLetter`:" -ForegroundColor Yellow
    Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Yellow
    
    $startTime = Get-Date
    
    try {
        # Use & operator with full path to execute
        $output = & "$UffsMftPath" info --drive $driveLetter --deep 2>&1
        $exitCode = $LASTEXITCODE
        
        if ($exitCode -ne 0) {
            Write-Host "  ERROR: uffs-mft failed with exit code $exitCode" -ForegroundColor Red
            Write-Host "  Output: $output" -ForegroundColor Red
            $results[$driveLetter] = @{ Error = "Exit code $exitCode"; Files = 0; Dirs = 0 }
            continue
        }
        
        # Parse output for statistics
        $files = 0
        $dirs = 0
        
        foreach ($line in $output) {
            $lineStr = $line.ToString()
            if ($lineStr -match "Files:\s+(\d[\d,]*)") {
                $files = [int64]($Matches[1] -replace ',', '')
            }
            elseif ($lineStr -match "Directories:\s+(\d[\d,]*)") {
                $dirs = [int64]($Matches[1] -replace ',', '')
            }
        }
        
        $elapsed = (Get-Date) - $startTime
        $totalTime += $elapsed
        
        $results[$driveLetter] = @{
            Files = $files
            Dirs = $dirs
            Total = $files + $dirs
            Time = $elapsed.TotalSeconds
        }
        
        $totalFiles += $files
        $totalDirs += $dirs
        
        Write-Host "  Files:       $files" -ForegroundColor White
        Write-Host "  Directories: $dirs" -ForegroundColor White
        Write-Host "  Total:       $($files + $dirs)" -ForegroundColor White
        Write-Host "  Time:        $($elapsed.TotalSeconds.ToString('F2'))s" -ForegroundColor Gray
        Write-Host ""
    }
    catch {
        Write-Host "  ERROR: $($_.Exception.Message)" -ForegroundColor Red
        $results[$driveLetter] = @{ Error = $_.Exception.Message; Files = 0; Dirs = 0 }
    }
}

# Summary
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  SUMMARY" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""
Write-Host ("{0,8} {1,15} {2,15} {3,15} {4,10}" -f "Drive", "Files", "Directories", "Total", "Time(s)")
Write-Host ("{0,-8} {1,-15} {2,-15} {3,-15} {4,-10}" -f "--------", "---------------", "---------------", "---------------", "----------")

foreach ($drive in ($results.Keys | Sort-Object)) {
    $r = $results[$drive]
    if ($r.Error) {
        Write-Host ("{0,8} {1}" -f "$drive`:", $r.Error) -ForegroundColor Red
    } else {
        Write-Host ("{0,8} {1,15} {2,15} {3,15} {4,10:F2}" -f "$drive`:", $r.Files, $r.Dirs, $r.Total, $r.Time)
    }
}

Write-Host ("{0,-8} {1,-15} {2,-15} {3,-15} {4,-10}" -f "--------", "---------------", "---------------", "---------------", "----------")
Write-Host ("{0,8} {1,15} {2,15} {3,15} {4,10:F2}" -f "TOTAL", $totalFiles, $totalDirs, ($totalFiles + $totalDirs), $totalTime.TotalSeconds) -ForegroundColor Green
Write-Host ""

# Output in comparison format
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  SUMMARY (for comparison with C++)" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
foreach ($drive in ($results.Keys | Sort-Object)) {
    $r = $results[$drive]
    if (-not $r.Error) {
        Write-Host "$drive`:  files=$($r.Files), dirs=$($r.Dirs), total=$($r.Total)"
    }
}
Write-Host "---"
Write-Host "GRAND TOTAL: files=$totalFiles, dirs=$totalDirs, total=$($totalFiles + $totalDirs)"
