# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# test_runs.ps1 - UFFS Live Data Collection (Windows only)
#
# Purpose:
#   Collect live MFT data and scan outputs on Windows for offline analysis on Mac.
#   This script focuses on LIVE data collection only - offline analysis is done on Mac.
#
# Strategy:
#   - Never write binary outputs with Set-Content.
#   - Capture stdout/stderr to .log files (text) with diagnostic logging.
#   - Sequential per physical disk; parallel across physical disks (PS7+).
#   - Enable diagnostic logging for live path analysis.
#   - ALWAYS save IOCP capture (.iocp) for each drive - captures real Windows IOCP order
#   - ALWAYS save uncompressed MFT snapshot (.bin) as fallback for offline analysis
#
# What gets collected:
#   1. IOCP captures (.iocp files) - captures IOCP completion order for 100% accurate replay
#   2. MFT snapshots (uncompressed .bin files) - fallback for sequential offline analysis
#   3. C++ baseline scan output - reference for parity comparison
#   4. Rust LIVE scan output + diagnostic logs - with chunk/record processing stats
#
# Diagnostic logging captures (in .log files):
#   - Chunk handoff, record boundaries, preload_concurrent timing
#   - USA fixup success/failure, records parsed, records not in-use
#   - Parallel sync (lock acquisition), chunk processing order
#
# After running this script, transfer all files to Mac for offline analysis using:
#   - uffs "*" --mft-file <mft_file.iocp> --drive <letter> --parity-compat  (IOCP replay)
#   - uffs "*" --mft-file <mft_file.bin> --drive <letter> --parity-compat   (sequential fallback)
#   - rust-script scripts/verify_parity.rs <data_dir> <drive> --regenerate
#   - See: TESTING_TOOLS_GUIDE.md for full workflow
[CmdletBinding()]
param(
    [string]$WorkDir = "",         # Output directory (default: current dir, auto-created if missing)
    [string[]]$Drives = @(),       # Drives to test (empty = auto-detect NTFS drives)
    [switch]$SkipMftExtras,        # Skip extra MFT formats (compressed, raw) - uncompressed always saved
    [string]$BinDir = "",          # Custom bin directory (default: $HOME\bin)
    [int]$ThrottleLimit = 2,       # Max physical disks in parallel (PS7+ only)
    [switch]$VerboseLog            # Enable verbose/trace logging (more detail, larger logs)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$env:RUST_BACKTRACE = "full"

# Logging configuration for C++ algorithm parity analysis
# Modules:
#   uffs_mft::cpp_tree        - C++ tree metrics algorithm port
#   uffs_mft::cpp_types       - C++ types and parsing structures
#   uffs_mft::cpp_io_pipeline - C++ I/O pipeline (bitmap sync, chunk processing)
#   uffs_mft::parse           - MFT record parsing
#   uffs_mft::io              - I/O operations
#   uffs_mft::reader          - MFT reader
#   uffs_mft::index           - Index building and tree metrics
#   uffs_cli::commands        - CLI command execution
#
# Levels: error < warn < info < debug < trace
#
# IMPORTANT: The post-tree diagnostic for LIVE issues uses tracing::warn!
# so it will appear even at "warn" level. The tripwire logs use tracing::debug!
# so they require at least "debug" level to appear.
if ($VerboseLog) {
    # TRACE: Maximum verbosity - all C++ algorithm modules at trace level
    $env:RUST_LOG = "uffs-mft=trace,uffs_cli=trace,uffs_core=trace"
    Write-Host "📋 Verbose logging enabled (TRACE level for all uffs modules)" -ForegroundColor Yellow
} else {
    # Default: warn level - captures post-tree diagnostics for LIVE issues
    # The "[tree] FINAL: directories with descendants==0" warning will appear here
    $env:RUST_LOG = "warn"
    Write-Host "📋 Standard logging (warn level - captures tree diagnostics, use -VerboseLog for trace)" -ForegroundColor Yellow
}

# Resolve WorkDir: default to current directory, auto-create if missing
if (-not $WorkDir) { $WorkDir = (Get-Location).Path }
$WorkDir = [System.IO.Path]::GetFullPath($WorkDir)
if (-not (Test-Path -LiteralPath $WorkDir)) {
    Write-Host "Creating output directory: $WorkDir" -ForegroundColor Yellow
    New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null
}
$FinalLog = Join-Path $WorkDir "test_runs.md"
$TempLog  = Join-Path $WorkDir "test_runs.md.tmp"

function Format-FileSize {
    param([long]$Bytes)
    if ($Bytes -ge 1GB) { return "{0:N2} GB" -f ($Bytes / 1GB) }
    if ($Bytes -ge 1MB) { return "{0:N2} MB" -f ($Bytes / 1MB) }
    if ($Bytes -ge 1KB) { return "{0:N2} KB" -f ($Bytes / 1KB) }
    return "$Bytes bytes"
}

# Smart-skip: returns $true if the artifact file exists, is less than 7 days old,
# and exceeds the minimum size threshold (default 1KB).
# When $true the caller should skip re-creation to save time on re-runs.
function Test-ArtifactFresh {
    param(
        [string]$Path,
        [long]$MinBytes = 1024,      # 1 KB default
        [int]$MaxAgeDays = 7
    )
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $item = Get-Item -LiteralPath $Path
    if ($item.Length -lt $MinBytes) { return $false }
    if ($item.LastWriteTime -lt (Get-Date).AddDays(-$MaxAgeDays)) { return $false }
    return $true
}

# Validate an artifact after creation. If the file is missing or too small, record
# a failure and emit a loud console error. Returns $true on success, $false on failure.
# Caller supplies a human-readable $Label for the error message.
$script:artifactFailures = @()

function Confirm-Artifact {
    param(
        [string]$Path,
        [string]$Label,
        [long]$MinBytes = 1024       # 1 KB default
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        $msg = "ARTIFACT MISSING: $Label → $Path"
        Write-Host ""
        Write-Host "  ╔══════════════════════════════════════════════════════════════╗" -ForegroundColor Red
        Write-Host "  ║  ❌ $Label" -ForegroundColor Red
        Write-Host "  ║     File does not exist: $Path" -ForegroundColor Red
        Write-Host "  ╚══════════════════════════════════════════════════════════════╝" -ForegroundColor Red
        $script:artifactFailures += $msg
        return $false
    }
    $size = (Get-Item -LiteralPath $Path).Length
    if ($size -lt $MinBytes) {
        $msg = "ARTIFACT TOO SMALL: $Label → $Path ($(Format-FileSize $size) < $(Format-FileSize $MinBytes))"
        Write-Host ""
        Write-Host "  ╔══════════════════════════════════════════════════════════════╗" -ForegroundColor Red
        Write-Host "  ║  ❌ $Label" -ForegroundColor Red
        Write-Host "  ║     File too small: $(Format-FileSize $size) (need ≥ $(Format-FileSize $MinBytes))" -ForegroundColor Red
        Write-Host "  ║     Path: $Path" -ForegroundColor Red
        Write-Host "  ╚══════════════════════════════════════════════════════════════╝" -ForegroundColor Red
        $script:artifactFailures += $msg
        return $false
    }
    return $true
}

function Get-NtfsDrives {
    # Use Get-CimInstance (faster, non-blocking) instead of deprecated Get-WmiObject
    # DriveType 3 = Fixed, DriveType 2 = Removable (USB NTFS drives like G:)
    try {
        Get-CimInstance Win32_LogicalDisk -Filter "(DriveType=3 OR DriveType=2) AND FileSystem='NTFS'" |
            ForEach-Object { $_.DeviceID.TrimEnd(':') }
    } catch {
        # Fallback: Get-Volume (available on Win10+) — picks up all NTFS regardless of bus type
        Get-Volume | Where-Object { $_.FileSystemType -eq 'NTFS' -and $_.DriveLetter } |
            ForEach-Object { $_.DriveLetter }
    }
}

# Best-effort mapping: Drive letter -> Physical disk number
# Requires Storage module (usually present on Win10/11). If it fails, we return $null for that drive.
function Get-PhysicalDiskNumberForDrive {
    param([string]$DriveLetter)

    try {
        $part = Get-Partition -DriveLetter $DriveLetter -ErrorAction Stop
        $disk = Get-Disk -Number $part.DiskNumber -ErrorAction Stop
        return [int]$disk.Number
    } catch {
        return $null
    }
}

# Simple markdown logger (single writer)
$fs = New-Object System.IO.FileStream(
$TempLog,
[System.IO.FileMode]::Create,
[System.IO.FileAccess]::Write,
[System.IO.FileShare]::ReadWrite
)
$sw = New-Object System.IO.StreamWriter($fs, [System.Text.Encoding]::UTF8)
$sw.NewLine = "`r`n"
function LogLine { param([string]$Line="") $sw.WriteLine($Line); $sw.Flush() }

# Run a command and write stdout/stderr to a TEXT log file.
# Does NOT try to "write output files" itself.
function Invoke-CmdToLog {
    param(
        [string]$Title,
        [string]$CommandLine,
        [string]$LogFileName,
        [string]$OutDir = ""
    )

    if (-not $OutDir) { $OutDir = $WorkDir }
    $logPath = Join-Path $OutDir $LogFileName
    $started = Get-Date
    $exitCode = 0

    Write-Host "  → $Title..." -NoNewline

    try {
        # Capture cmd.exe output lines, then write to log (text)
        $lines = @(& cmd.exe /c $CommandLine 2>&1)
        $exitCode = $LASTEXITCODE
        $lines | Set-Content -LiteralPath $logPath -Encoding UTF8
    } catch {
        $exitCode = -1
        @("PowerShell exception:", $_.Exception.ToString()) | Set-Content -LiteralPath $logPath -Encoding UTF8
    }

    $ended = Get-Date
    $durMs = [math]::Round((New-TimeSpan -Start $started -End $ended).TotalMilliseconds)

    if ($exitCode -eq 0) { Write-Host " ✅ ($durMs ms)" -ForegroundColor Green }
    else { Write-Host " ❌ (exit: $exitCode)" -ForegroundColor Red }

    return [pscustomobject]@{
        Title      = $Title
        Command    = $CommandLine
        LogFile    = $LogFileName
        Started    = $started
        Ended      = $ended
        DurationMs = $durMs
        ExitCode   = $exitCode
    }
}

try {
    Write-Host ""
    Write-Host "╔════════════════════════════════════════╗" -ForegroundColor Cyan
    Write-Host "║    UFFS Test Run — Data Collection      ║" -ForegroundColor Cyan
    Write-Host "╚════════════════════════════════════════╝" -ForegroundColor Cyan
    Write-Host ""

    LogLine "# UFFS Test Run Report (data collection only)"
    LogLine ""
    LogLine ("- **Started:** " + (Get-Date -Format o))
    LogLine ("- **Working dir:** " + $WorkDir.ToString())
    LogLine ("- **User:** " + (whoami))
    LogLine ("- **Computer:** " + $env:COMPUTERNAME)
    LogLine ("- **PowerShell:** " + $PSVersionTable.PSVersion.ToString())
    LogLine ""

    if (-not $BinDir) { $BinDir = Join-Path $HOME "bin" }

    $UffsExe    = Join-Path $BinDir "uffs.exe"
    $UffsCom    = Join-Path $BinDir "uffs.com"
    $UffsMftExe = Join-Path $BinDir "uffs-mft.exe"

    # Everything — gold-standard MFT-based reference
    # We edit the APPDATA ini per-drive to avoid indexing all 25M+ files at once.
    # Requires: Everything.exe (GUI) + es.exe (CLI query tool)
    $EverythingExe = $null
    $EsExe = $null
    $EverythingIniPath = Join-Path $env:APPDATA "Everything\Everything.ini"

    $pf86 = ${env:ProgramFiles(x86)}

    $etCandidates = @(
        (Join-Path ${env:ProgramFiles} "Everything\Everything.exe"),
        (Join-Path ${env:LOCALAPPDATA} "Everything\Everything.exe"),
        (Join-Path $BinDir "Everything.exe")
    )
    if ($pf86) { $etCandidates += (Join-Path $pf86 "Everything\Everything.exe") }
    if (${env:ProgramW6432}) { $etCandidates += (Join-Path ${env:ProgramW6432} "Everything\Everything.exe") }
    $etCandidates = @($etCandidates | Where-Object { $_ -and (Test-Path -LiteralPath $_ -ErrorAction SilentlyContinue) })
    if ($etCandidates.Count -gt 0) { $EverythingExe = $etCandidates[0] }
    $esCandidates = @(
        (Join-Path $BinDir "es.exe"),
        (Join-Path ${env:ProgramFiles} "Everything\es.exe")
    )
    if ($pf86) { $esCandidates += (Join-Path $pf86 "Everything\es.exe") }
    $esCandidates += (Join-Path ${env:LOCALAPPDATA} "Everything\es.exe")
    $esFromPath = Get-Command "es.exe" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue
    if ($esFromPath) { $esCandidates = @($esFromPath) + $esCandidates }
    $esCandidates = @($esCandidates | Where-Object { $_ -and (Test-Path -LiteralPath $_ -ErrorAction SilentlyContinue) })
    if ($esCandidates.Count -gt 0) { $EsExe = $esCandidates[0] }

    $hasRust = Test-Path -LiteralPath $UffsExe
    $hasCpp  = Test-Path -LiteralPath $UffsCom
    $hasMft  = Test-Path -LiteralPath $UffsMftExe
    $hasEverything = ($null -ne $EverythingExe) -and ($null -ne $EsExe) -and (Test-Path -LiteralPath $EverythingIniPath)

    LogLine "## Binaries"
    LogLine ""
    LogLine "| Binary | Path | Exists |"
    LogLine "|--------|------|--------|"
    LogLine ("| uffs.exe (Rust) | ``$UffsExe`` | " + $(if ($hasRust) { "✅" } else { "❌" }) + " |")
    LogLine ("| uffs.com (C++) | ``$UffsCom`` | " + $(if ($hasCpp) { "✅" } else { "❌" }) + " |")
    LogLine ("| uffs-mft.exe | ``$UffsMftExe`` | " + $(if ($hasMft) { "✅" } else { "❌" }) + " |")
    LogLine ("| Everything.exe | ``$(if ($EverythingExe) { $EverythingExe } else { '(not found)' })`` | " + $(if ($EverythingExe) { "✅" } else { "⏭️" }) + " |")
    LogLine ("| es.exe | ``$(if ($EsExe) { $EsExe } else { '(not found)' })`` | " + $(if ($EsExe) { "✅" } else { "⏭️" }) + " |")
    LogLine ("| Everything INI | ``$EverythingIniPath`` | " + $(if (Test-Path -LiteralPath $EverythingIniPath) { "✅" } else { "⏭️" }) + " |")
    LogLine ""
    if (-not $hasEverything) {
        $missingEs = @()
        if (-not $EverythingExe) { $missingEs += "Everything.exe" }
        if (-not $EsExe) { $missingEs += "es.exe" }
        if (-not (Test-Path -LiteralPath $EverythingIniPath)) { $missingEs += "Everything.ini" }
        Write-Host "  ⏭️ Everything baseline will be skipped (missing: $($missingEs -join ', '))" -ForegroundColor DarkGray
        Write-Host "    Install from: https://www.voidtools.com/" -ForegroundColor DarkGray
    }

    if (@($Drives).Count -eq 0) {
        $Drives = @(Get-NtfsDrives)
        Write-Host "Auto-detected NTFS drives: $($Drives -join ', ')" -ForegroundColor Yellow
    }

    LogLine ("**Drives to test:** " + ($Drives -join ", "))
    LogLine ""

    # Version check — saved into each drive dir (each capture is a point-in-time snapshot)
    $timings = @()
    if ($hasRust) {
        foreach ($d in $Drives) {
            $dd = Join-Path $WorkDir "drive_$($d.ToLower())"
            if (-not (Test-Path -LiteralPath $dd)) {
                New-Item -ItemType Directory -Path $dd -Force | Out-Null
            }
            $timings += Invoke-CmdToLog -Title "uffs --version (drive $d)" `
                -CommandLine ("`"$UffsExe`" --version") `
                -LogFileName "uffs_version.log" `
                -OutDir $dd
        }
        LogLine "- Version log saved to each ``drive_<x>/uffs_version.log``"
        LogLine ""
    }

    # MFT saves - ALWAYS save IOCP capture (primary) and uncompressed MFT (fallback)
    # IOCP capture is preferred as it captures real Windows IOCP completion order
    # Extra formats (compressed, raw) can be skipped with -SkipMftExtras
    if ($hasMft -and @($Drives).Count -gt 0) {
        LogLine "---"
        LogLine ""
        LogLine "# MFT Snapshots"
        LogLine ""

        foreach ($mftDrive in $Drives) {
            Write-Host "MFT Save (Drive $mftDrive)..." -ForegroundColor Cyan

            # Create drive subdirectory
            $driveDir = Join-Path $WorkDir "drive_$($mftDrive.ToLower())"
            if (-not (Test-Path -LiteralPath $driveDir)) {
                New-Item -ItemType Directory -Path $driveDir -Force | Out-Null
            }

            $mftIocp = "${mftDrive}_mft.iocp"       # IOCP capture - captures real IOCP order (PRIMARY)
            $mftNoCompress = "${mftDrive}_mft.bin"  # Uncompressed sequential (fallback)
            $mftIocpPath = Join-Path $driveDir $mftIocp
            $mftNoCompressPath = Join-Path $driveDir $mftNoCompress

            # Always save IOCP capture (captures real Windows IOCP completion order)
            # This is the PRIMARY format for 100% accurate LIVE replay on Mac
            if (Test-ArtifactFresh $mftIocpPath) {
                Write-Host "  ⏭️  IOCP capture fresh — skipping: $mftIocp" -ForegroundColor DarkGreen
            } else {
                $timings += Invoke-CmdToLog -Title "uffs-mft save (IOCP capture): drive $mftDrive" `
                    -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive --output `"$mftIocpPath`" --iocp") `
                    -LogFileName "${mftDrive}_mft_save_iocp.log" `
                    -OutDir $driveDir
                Confirm-Artifact -Path $mftIocpPath -Label "MFT IOCP capture (drive $mftDrive)" | Out-Null
            }

            # Always save uncompressed MFT (fallback for sequential offline analysis)
            if (Test-ArtifactFresh $mftNoCompressPath) {
                Write-Host "  ⏭️  Uncompressed MFT fresh — skipping: $mftNoCompress" -ForegroundColor DarkGreen
            } else {
                $timings += Invoke-CmdToLog -Title "uffs-mft save (uncompressed): drive $mftDrive" `
                    -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive --output `"$mftNoCompressPath`" --no-compress") `
                    -LogFileName "${mftDrive}_mft_save.log" `
                    -OutDir $driveDir
                Confirm-Artifact -Path $mftNoCompressPath -Label "MFT uncompressed (drive $mftDrive)" | Out-Null
            }

            # Extra formats (optional)
            if (-not $SkipMftExtras) {
                $mftCompressed = "${mftDrive}_mft_compressed.bin"
                $mftRaw        = "${mftDrive}_mft.raw"
                $mftCompressedPath = Join-Path $driveDir $mftCompressed
                $mftRawPath = Join-Path $driveDir $mftRaw

                if (Test-ArtifactFresh $mftCompressedPath) {
                    Write-Host "  ⏭️  Compressed MFT fresh — skipping: $mftCompressed" -ForegroundColor DarkGreen
                } else {
                    $timings += Invoke-CmdToLog -Title "uffs-mft save (compressed): drive $mftDrive" `
                        -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o `"$mftCompressedPath`"") `
                        -LogFileName "${mftDrive}_mft_save_compressed.log" `
                        -OutDir $driveDir
                    Confirm-Artifact -Path $mftCompressedPath -Label "MFT compressed (drive $mftDrive)" | Out-Null
                }

                if (Test-ArtifactFresh $mftRawPath) {
                    Write-Host "  ⏭️  Raw MFT fresh — skipping: $mftRaw" -ForegroundColor DarkGreen
                } else {
                    $timings += Invoke-CmdToLog -Title "uffs-mft save (raw): drive $mftDrive" `
                        -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o `"$mftRawPath`" --raw") `
                        -LogFileName "${mftDrive}_mft_save_raw.log" `
                        -OutDir $driveDir
                    Confirm-Artifact -Path $mftRawPath -Label "MFT raw (drive $mftDrive)" | Out-Null
                }
            }
        }

        LogLine "### Generated MFT Files"
        LogLine ""
        LogLine "| Drive | File | Format | Size |"
        LogLine "|-------|------|--------|------|"
        foreach ($mftDrive in $Drives) {
            $driveDir = Join-Path $WorkDir "drive_$($mftDrive.ToLower())"

            # IOCP capture (primary)
            $mftIocp = "${mftDrive}_mft.iocp"
            $p = Join-Path $driveDir $mftIocp
            if (Test-Path -LiteralPath $p) {
                $size = (Get-Item -LiteralPath $p).Length
                LogLine "| $mftDrive | $mftIocp | **IOCP capture** (primary) | $(Format-FileSize $size) |"
            } else {
                LogLine "| $mftDrive | $mftIocp | IOCP capture | (missing) |"
            }

            # Uncompressed MFT (fallback)
            $mftNoCompress = "${mftDrive}_mft.bin"
            $p = Join-Path $driveDir $mftNoCompress
            if (Test-Path -LiteralPath $p) {
                $size = (Get-Item -LiteralPath $p).Length
                LogLine "| $mftDrive | $mftNoCompress | Sequential (fallback) | $(Format-FileSize $size) |"
            } else {
                LogLine "| $mftDrive | $mftNoCompress | Sequential | (missing) |"
            }

            if (-not $SkipMftExtras) {
                $mftCompressed = "${mftDrive}_mft_compressed.bin"
                $mftRaw        = "${mftDrive}_mft.raw"
                foreach ($f in @($mftCompressed, $mftRaw)) {
                    $p = Join-Path $driveDir $f
                    if (Test-Path -LiteralPath $p) {
                        $size = (Get-Item -LiteralPath $p).Length
                        LogLine "| $mftDrive | $f | Extra | $(Format-FileSize $size) |"
                    } else {
                        LogLine "| $mftDrive | $f | Extra | (missing) |"
                    }
                }
            }
        }
        LogLine ""
    }

    # Group drives by physical disk number (best effort)
    $driveToDisk = @{}
    foreach ($d in $Drives) {
        $driveToDisk[$d] = Get-PhysicalDiskNumberForDrive -DriveLetter $d
    }

    $allMapped = $true
    foreach ($d in $Drives) {
        if ($null -eq $driveToDisk[$d]) { $allMapped = $false; break }
    }

    $isPS7Plus = ($PSVersionTable.PSVersion.Major -ge 7)

    LogLine "---"
    LogLine ""
    LogLine "# Drive Scans"
    LogLine ""
    LogLine ("- **PS7+ available:** " + $(if ($isPS7Plus) { "Yes" } else { "No" }))
    LogLine ("- **Physical disk mapping available:** " + $(if ($allMapped) { "Yes" } else { "No (falling back to sequential)" }))
    LogLine ("- **Policy:** sequential per physical disk; parallel across disks")
    LogLine ""

    # Build disk groups: @{ diskNumber = @('D','E') }
    $diskGroups = @{}
    if ($allMapped) {
        foreach ($d in $Drives) {
            $diskNum = $driveToDisk[$d]
            if (-not $diskGroups.ContainsKey($diskNum)) { $diskGroups[$diskNum] = @() }
            $diskGroups[$diskNum] += $d
        }
    }

    # Worker logic (sequential for drives within a disk group)
    # Focus on: C++ baseline, Rust live (cpp io), Rust offline (from saved MFT)
    $runDiskGroup = {
        param(
            [int]$DiskNumber,
            [string[]]$GroupDrives,
            [string]$WorkDir,
            [string]$UffsExe,
            [string]$UffsCom,
            [string]$UffsMftExe,
            [string]$EverythingExe,
            [string]$EsExe,
            [string]$EverythingIniPath,
            [bool]$HasRust,
            [bool]$HasCpp,
            [bool]$HasMft,
            [bool]$HasEverything
        )

        $groupResults = @()

        foreach ($Drive in $GroupDrives) {
            $driveLower = $Drive.ToLower()

            # Create drive subdirectory
            $driveDir = Join-Path $WorkDir "drive_${driveLower}"
            if (-not (Test-Path -LiteralPath $driveDir)) {
                New-Item -ItemType Directory -Path $driveDir -Force | Out-Null
            }

            # Output files (inside drive_<letter>/)
            $cppOut         = "cpp_${driveLower}.txt"
            $rustLiveOut    = "rust_live_${driveLower}.txt"
            $rustLiveTraceOut = "rust_live_trace_${driveLower}.txt"
            $rustOfflineOut = "rust_offline_${driveLower}.txt"
            $esOut          = "es_${driveLower}.txt"

            # Log files (capture stderr with diagnostics)
            $cppLog         = "cpp_${driveLower}.log"
            $rustLiveLog    = "rust_live_${driveLower}.log"
            $rustLiveTraceLog = "rust_live_trace_${driveLower}.log"
            $rustOfflineLog = "rust_offline_${driveLower}.log"
            $esLog          = "es_${driveLower}.log"

            # MFT file for offline comparison (inside drive_<letter>/)
            $mftBin = "${driveLower}_mft.bin"

            function Run-LoggedLocal {
                param([string]$Title, [string]$CmdLine, [string]$LogFileName, [string]$OutFileName = "")

                $logPath = Join-Path $driveDir $LogFileName
                $started = Get-Date
                $exitCode = 0

                Write-Host "  → $Title..." -NoNewline

                try {
                    # Run command with stdout to output file, stderr to log file
                    # This properly separates scan output from diagnostic logs
                    if ($OutFileName) {
                        $outPath = Join-Path $driveDir $OutFileName
                        # Use cmd.exe to properly separate stdout (>output) and stderr (2>log)
                        & cmd.exe /c "$CmdLine > `"$outPath`" 2> `"$logPath`""
                        $exitCode = $LASTEXITCODE
                    } else {
                        # No output file - capture everything to log
                        $lines = @(& cmd.exe /c $CmdLine 2>&1)
                        $exitCode = $LASTEXITCODE
                        $lines | Set-Content -LiteralPath $logPath -Encoding UTF8
                    }
                } catch {
                    $exitCode = -1
                    @("PowerShell exception:", $_.Exception.ToString()) | Set-Content -LiteralPath $logPath -Encoding UTF8
                }

                $ended = Get-Date
                $durMs = [math]::Round((New-TimeSpan -Start $started -End $ended).TotalMilliseconds)

                if ($exitCode -eq 0) {
                    Write-Host " ✅ ($durMs ms)" -ForegroundColor Green
                } else {
                    Write-Host " ❌ (exit: $exitCode, $durMs ms)" -ForegroundColor Red
                    # Show log content on error
                    Write-Host "    📋 Log ($LogFileName):" -ForegroundColor Yellow
                    if (Test-Path -LiteralPath $logPath) {
                        $logContent = Get-Content -LiteralPath $logPath -TotalCount 20
                        foreach ($line in $logContent) {
                            Write-Host "       $line" -ForegroundColor DarkYellow
                        }
                        $totalLines = (Get-Content -LiteralPath $logPath | Measure-Object -Line).Lines
                        if ($totalLines -gt 20) {
                            Write-Host "       ... ($($totalLines - 20) more lines in $LogFileName)" -ForegroundColor DarkYellow
                        }
                    } else {
                        Write-Host "       (log file not found)" -ForegroundColor DarkYellow
                    }
                }

                return [pscustomobject]@{
                    Drive      = $Drive
                    Title      = $Title
                    Command    = $CmdLine
                    LogFile    = $LogFileName
                    OutFile    = $OutFileName
                    DurationMs = $durMs
                    ExitCode   = $exitCode
                }
            }

            $runs = @()

            # Track which scans need re-collection (only clear cache if we're actually scanning)
            $cppOutPath       = Join-Path $driveDir $cppOut
            $rustLiveOutPath  = Join-Path $driveDir $rustLiveOut
            $rustLiveTraceOutPath = Join-Path $driveDir $rustLiveTraceOut

            $needCpp       = $HasCpp  -and -not (Test-ArtifactFresh $cppOutPath)
            $needRustLive  = $HasRust -and -not (Test-ArtifactFresh $rustLiveOutPath)
            $needRustTrace = $HasRust -and -not (Test-ArtifactFresh $rustLiveTraceOutPath)
            $needAnyScan   = $needCpp -or $needRustLive -or $needRustTrace

            # 0. Clear Rust cache for this drive (only if we're re-scanning)
            if ($HasMft -and $needAnyScan) {
                Write-Host "  → Clearing Rust cache for drive $Drive..." -NoNewline
                & cmd.exe /c "`"$UffsMftExe`" cache-clear --drive $Drive" 2>&1 | Out-Null
                Write-Host " ✅" -ForegroundColor Green
            }

            # 1. C++ baseline (no diagnostics, just output)
            # C++ always reads MFT fresh (no caching)
            if (-not $HasCpp) {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="C++ (baseline)"; Command=""; LogFile=$cppLog; OutFile=$cppOut; DurationMs=$null; ExitCode=$null }
            } elseif (-not $needCpp) {
                Write-Host "  ⏭️  C++ baseline fresh — skipping: $cppOut" -ForegroundColor DarkGreen
            } else {
                $runs += Run-LoggedLocal -Title "C++ (baseline): drive $Drive" `
                    -CmdLine ("`"$UffsCom`" `"*`" --drives=$Drive") `
                    -LogFileName $cppLog `
                    -OutFileName $cppOut
                Confirm-Artifact -Path $cppOutPath -Label "C++ baseline (drive $Drive)" | Out-Null
            }

            # 2. Rust LIVE scan (with diagnostic logging via RUST_LOG)
            # --no-cache forces fresh MFT read to ensure tree metrics are computed
            # --parity-compat --format custom: match C++ output format for parity comparison
            if (-not $HasRust) {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="Rust LIVE"; Command=""; LogFile=$rustLiveLog; OutFile=$rustLiveOut; DurationMs=$null; ExitCode=$null }
            } elseif (-not $needRustLive) {
                Write-Host "  ⏭️  Rust LIVE fresh — skipping: $rustLiveOut" -ForegroundColor DarkGreen
            } else {
                $runs += Run-LoggedLocal -Title "Rust LIVE: drive $Drive" `
                    -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive --no-cache --parity-compat --format custom") `
                    -LogFileName $rustLiveLog `
                    -OutFileName $rustLiveOut
                Confirm-Artifact -Path $rustLiveOutPath -Label "Rust LIVE scan (drive $Drive)" | Out-Null
            }

            # 2b. Rust LIVE scan with DEBUG logging (for detailed diagnostics)
            # Temporarily enables debug-level logging to capture detailed diagnostics
            # NOTE: Trace logging can cause stack overflow on deep directory trees - use debug level instead
            if (-not $HasRust) {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="Rust LIVE TRACE"; Command=""; LogFile=$rustLiveTraceLog; OutFile=$rustLiveTraceOut; DurationMs=$null; ExitCode=$null }
            } elseif (-not $needRustTrace) {
                Write-Host "  ⏭️  Rust LIVE TRACE fresh — skipping: $rustLiveTraceOut" -ForegroundColor DarkGreen
            } else {
                $savedRustLog = $env:RUST_LOG
                $env:RUST_LOG = "uffs-mft=debug,uffs_cli=debug,uffs_core=debug"
                $runs += Run-LoggedLocal -Title "Rust LIVE TRACE: drive $Drive" `
                    -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive --no-cache --parity-compat --format custom") `
                    -LogFileName $rustLiveTraceLog `
                    -OutFileName $rustLiveTraceOut
                $env:RUST_LOG = $savedRustLog
                Confirm-Artifact -Path $rustLiveTraceOutPath -Label "Rust LIVE TRACE (drive $Drive)" | Out-Null
            }

            # 3. Everything — DISABLED (2026-03-27)
            # Everything 1.4 has a 2GB IPC memory limit that prevents es.exe from
            # exporting data on drives with >2M entries. Both -export-efu and CSV
            # stdout hit this wall (even path-only output OOMs on 4.7M files).
            # Everything can INDEX the MFT fine — the limit is purely in the IPC
            # data transfer between Everything.exe and es.exe.
            # Benchmark timing (start → index ready) is still available via benchmark.ps1.
            # Re-enable when Everything 1.5 ships with the IPC memory fix.
            # See: https://www.voidtools.com/forum/viewtopic.php?t=15249
            Write-Host "  → Everything: disabled (es.exe 2GB IPC limit — see benchmark.ps1 for timing)" -ForegroundColor DarkGray
            <#  ── BEGIN COMMENTED-OUT EVERYTHING PARITY COLLECTION ──
            if ($HasEverything) {
                Write-Host "  → Everything: configuring for ${Drive}: only..." -ForegroundColor DarkYellow

                # Stop any running Everything
                Get-Process -Name "Everything" -ErrorAction SilentlyContinue |
                    ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
                Start-Sleep -Seconds 2

                # Back up ini (only once — check if backup already exists)
                $iniBak = "${EverythingIniPath}.uffs_bak"
                if (-not (Test-Path -LiteralPath $iniBak)) {
                    Copy-Item -LiteralPath $EverythingIniPath -Destination $iniBak -Force
                }

                # Read ini and find the drive's position in ntfs_volume_paths
                $iniContent = Get-Content -LiteralPath $EverythingIniPath -Raw
                $volPathsMatch = [regex]::Match($iniContent, 'ntfs_volume_paths=(.*)')
                if ($volPathsMatch.Success) {
                    $volPaths = $volPathsMatch.Groups[1].Value -split ','
                    $driveIdx = -1
                    for ($vi = 0; $vi -lt $volPaths.Count; $vi++) {
                        $vp = $volPaths[$vi].Trim().Trim('"')
                        if ($vp -eq "${Drive}:") { $driveIdx = $vi; break }
                    }

                    if ($driveIdx -ge 0) {
                        # Build includes string: 0 for all, 1 for target drive
                        $includesList = @(0) * $volPaths.Count
                        $includesList[$driveIdx] = 1
                        $includesStr = $includesList -join ","

                        # Apply changes: only this drive, all index fields, no auto-include
                        $iniContent = $iniContent -replace 'ntfs_volume_includes=.*', "ntfs_volume_includes=$includesStr"
                        $iniContent = $iniContent -replace 'auto_include_fixed_volumes=.*', 'auto_include_fixed_volumes=0'
                        $iniContent = $iniContent -replace 'auto_include_removable_volumes=.*', 'auto_include_removable_volumes=0'
                        $iniContent = $iniContent -replace 'index_date_created=.*', 'index_date_created=1'
                        $iniContent = $iniContent -replace 'index_date_accessed=.*', 'index_date_accessed=1'
                        $iniContent = $iniContent -replace 'index_date_modified=.*', 'index_date_modified=1'
                        $iniContent = $iniContent -replace 'index_attributes=.*', 'index_attributes=1'
                        $iniContent = $iniContent -replace 'index_size=.*', 'index_size=1'
                        $iniContent | Out-File -FilePath $EverythingIniPath -Encoding ascii -NoNewline

                        # Start Everything (indexes only target drive)
                        Start-Process -FilePath $EverythingExe -ArgumentList "-startup -minimized" -WindowStyle Hidden
                        Write-Host "  → Everything: started, waiting for MFT index of ${Drive}:..." -ForegroundColor DarkYellow

                        # Wait for indexing (poll es.exe -get-result-count until > 0 or timeout)
                        $esReady = $false
                        for ($wi = 1; $wi -le 30; $wi++) {
                            Start-Sleep -Seconds 2
                            $null = & $EsExe -get-result-count 2>&1
                            if ($LASTEXITCODE -eq 0) {
                                $rcText = & $EsExe -get-result-count 2>&1
                                $rc = 0; [int]::TryParse($rcText, [ref]$rc) | Out-Null
                                if ($rc -gt 0) {
                                    $esReady = $true
                                    Write-Host "  → Everything: indexed $rc entries ($($wi*2)s)" -ForegroundColor DarkGreen
                                    break
                                }
                            }
                        }

                        if ($esReady) {
                            # Export via -export-efu (writes directly to file, bypasses IPC stdout limit).
                            # EFU format: Filename,Size,Date Modified,Date Created,Attributes (FILETIME i64)
                            # Fallback: if EFU export fails (IPC >2GB on v1.4), try CSV with fewer columns.
                            $esOutPath = Join-Path $driveDir $esOut
                            $esExportOk = $false

                            # Attempt 1: EFU export (dev-recommended for large drives)
                            $efuResult = Run-LoggedLocal -Title "Everything (es.exe EFU export): drive $Drive" `
                                -CmdLine ("`"$EsExe`" -path `"${Drive}:\`" -export-efu `"$esOutPath`"") `
                                -LogFileName $esLog
                            $runs += $efuResult
                            if ($efuResult.ExitCode -eq 0 -and (Test-Path -LiteralPath $esOutPath) -and (Get-Item $esOutPath).Length -gt 0) {
                                $esExportOk = $true
                            }

                            # Attempt 2: path + size + attributes (smaller IPC payload, ~150B/entry)
                            # 4.7M × 150B ≈ 700MB — safely under Everything's 2GB IPC limit
                            if (-not $esExportOk) {
                                Write-Host "    ⚠️  EFU export failed — retrying with path+size+attributes..." -ForegroundColor DarkYellow
                                $fallbackResult = Run-LoggedLocal -Title "Everything (es.exe lite): drive $Drive" `
                                    -CmdLine ("`"$EsExe`" -path `"${Drive}:\`" -name -path-column -size -attributes -no-digit-grouping -csv") `
                                    -LogFileName $esLog `
                                    -OutFileName $esOut
                                $runs += $fallbackResult
                            }
                        } else {
                            Write-Host "  → Everything: indexing timed out (60s) — skipping" -ForegroundColor DarkRed
                            $runs += [pscustomobject]@{ Drive=$Drive; Title="Everything (es.exe MFT)"; Command=""; LogFile=$esLog; OutFile=$esOut; DurationMs=$null; ExitCode=$null }
                        }

                        # Stop Everything after this drive
                        Get-Process -Name "Everything" -ErrorAction SilentlyContinue |
                            ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
                        Start-Sleep -Seconds 1
                    } else {
                        Write-Host "  → Everything: drive ${Drive}: not found in ntfs_volume_paths — skipping" -ForegroundColor DarkRed
                        $runs += [pscustomobject]@{ Drive=$Drive; Title="Everything (es.exe MFT)"; Command=""; LogFile=$esLog; OutFile=$esOut; DurationMs=$null; ExitCode=$null }
                    }
                } else {
                    Write-Host "  → Everything: ntfs_volume_paths not found in ini — skipping" -ForegroundColor DarkRed
                    $runs += [pscustomobject]@{ Drive=$Drive; Title="Everything (es.exe MFT)"; Command=""; LogFile=$esLog; OutFile=$esOut; DurationMs=$null; ExitCode=$null }
                }
            } else {
                Write-Host "  → Everything: skipped (Everything.exe / es.exe / ini not found)" -ForegroundColor DarkGray
                $runs += [pscustomobject]@{ Drive=$Drive; Title="Everything (es.exe MFT)"; Command=""; LogFile=$esLog; OutFile=$esOut; DurationMs=$null; ExitCode=$null }
            }
            #>
            # ── END COMMENTED-OUT EVERYTHING PARITY COLLECTION ──

            # 4. Rust OFFLINE scan - SKIPPED on Windows
            # Offline analysis is done on Mac for faster iteration (see TESTING_TOOLS_GUIDE.md)
            Write-Host "  → Rust OFFLINE: skipped (offline analysis done on Mac)" -ForegroundColor DarkGray

            $groupResults += [pscustomobject]@{
                Disk   = $DiskNumber
                Drive  = $Drive
                Files  = [pscustomobject]@{ Cpp=$cppOut; RustLive=$rustLiveOut; RustLiveTrace=$rustLiveTraceOut; RustOffline=$rustOfflineOut; Es=$esOut }
                Logs   = [pscustomobject]@{ Cpp=$cppLog; RustLive=$rustLiveLog; RustLiveTrace=$rustLiveTraceLog; RustOffline=$rustOfflineLog; Es=$esLog }
                Runs   = $runs
            }
        }

        return $groupResults
    }

    $scanResults = @()

    # Sequential scan for all drives — grouped by physical disk for optimal I/O
    # (ForEach-Object -Parallel cannot accept scriptblock variables, so we run
    #  disk groups sequentially but process drives within each group sequentially too,
    #  which is correct since drives on the same physical disk should not be read in parallel)
    if ($allMapped -and @($diskGroups.Keys).Count -gt 1) {
        Write-Host "Drive scans: sequential by physical disk group ($(@($diskGroups.Keys).Count) disks)." -ForegroundColor Yellow
        foreach ($diskNum in @($diskGroups.Keys | Sort-Object)) {
            $groupDrives = $diskGroups[$diskNum]
            $scanResults += & $runDiskGroup -DiskNumber $diskNum -GroupDrives $groupDrives -WorkDir $WorkDir `
                -UffsExe $UffsExe -UffsCom $UffsCom -UffsMftExe $UffsMftExe `
                -EverythingExe "$EverythingExe" -EsExe "$EsExe" -EverythingIniPath "$EverythingIniPath" `
                -HasRust $hasRust -HasCpp $hasCpp -HasMft $hasMft -HasEverything $hasEverything
        }
    } else {
        Write-Host "Drive scans: running sequential." -ForegroundColor Yellow
        foreach ($d in $Drives) {
            $scanResults += & $runDiskGroup -DiskNumber -1 -GroupDrives @($d) -WorkDir $WorkDir `
                -UffsExe $UffsExe -UffsCom $UffsCom -UffsMftExe $UffsMftExe `
                -EverythingExe "$EverythingExe" -EsExe "$EsExe" -EverythingIniPath "$EverythingIniPath" `
                -HasRust $hasRust -HasCpp $hasCpp -HasMft $hasMft -HasEverything $hasEverything
        }
    }


    # Restore Everything ini from backup (if we modified it)
    if ($hasEverything) {
        $iniBak = "${EverythingIniPath}.uffs_bak"
        if (Test-Path -LiteralPath $iniBak) {
            # Stop Everything if still running
            Get-Process -Name "Everything" -ErrorAction SilentlyContinue |
                ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
            Start-Sleep -Seconds 1
            Copy-Item -LiteralPath $iniBak -Destination $EverythingIniPath -Force
            Remove-Item -LiteralPath $iniBak -Force -ErrorAction SilentlyContinue
            Write-Host "  → Everything ini restored from backup" -ForegroundColor DarkGreen
        }
    }

    # Consolidate results into markdown (single thread)
    LogLine "---"
    LogLine ""
    LogLine "# Scan Outputs"
    LogLine ""

    foreach ($r in $scanResults) {
        $drive = $r.Drive
        $disk  = $r.Disk
        $driveDir = Join-Path $WorkDir "drive_$($drive.ToLower())"

        LogLine "## Drive $drive (Disk $disk)"
        LogLine ""

        LogLine "| Flow | Output file | Size | Log file | Exit | Duration (ms) |"
        LogLine "|------|-------------|------|----------|------|---------------:|"

        foreach ($run in $r.Runs) {
            $outFile = $run.OutFile
            $outPath = if ($outFile) { Join-Path $driveDir $outFile } else { $null }
            $sizeStr = "N/A"
            if ($outPath -and (Test-Path -LiteralPath $outPath)) {
                $sizeStr = Format-FileSize (Get-Item -LiteralPath $outPath).Length
            }

            $logPath = if ($run.LogFile) { Join-Path $driveDir $run.LogFile } else { $null }
            $logSizeStr = ""
            if ($logPath -and (Test-Path -LiteralPath $logPath)) {
                $logSize = (Get-Item -LiteralPath $logPath).Length
                $logSizeStr = " ($(Format-FileSize $logSize))"
            }

            $exit = if ($null -eq $run.ExitCode) { "skipped" } else { "$($run.ExitCode)" }
            $dur  = if ($null -eq $run.DurationMs) { "N/A" } else { "$($run.DurationMs)" }

            LogLine "| $($run.Title) | $outFile | $sizeStr | $($run.LogFile)$logSizeStr | $exit | $dur |"
        }

        LogLine ""
    }

    LogLine "---"
    LogLine ("**Completed:** " + (Get-Date -Format o))

    # Write per-drive summary files so each drive_<x>/ folder is self-contained
    foreach ($r in @($scanResults)) {
        if (-not $r -or -not $r.Drive) { continue }
        $drive = $r.Drive
        $driveLower = $drive.ToLower()
        $driveDir = Join-Path $WorkDir "drive_${driveLower}"
        if (-not (Test-Path -LiteralPath $driveDir)) { continue }

        $driveSummary = Join-Path $driveDir "drive_${driveLower}_test_runs.md"
        $lines = @()
        $lines += "# Test Run — Drive $drive"
        $lines += ""
        $lines += "**Generated:** $(Get-Date -Format o)"
        $lines += "**Host:** $env:COMPUTERNAME"
        $lines += ""
        $lines += "| Flow | Output file | Size | Log file | Exit | Duration (ms) |"
        $lines += "|------|-------------|------|----------|------|---------------:|"
        foreach ($run in $r.Runs) {
            $outFile = $run.OutFile
            $outPath = if ($outFile) { Join-Path $driveDir $outFile } else { $null }
            $sizeStr = "N/A"
            if ($outPath -and (Test-Path -LiteralPath $outPath)) {
                $sizeStr = Format-FileSize (Get-Item -LiteralPath $outPath).Length
            }
            $logPath = if ($run.LogFile) { Join-Path $driveDir $run.LogFile } else { $null }
            $logSizeStr = ""
            if ($logPath -and (Test-Path -LiteralPath $logPath)) {
                $logSize = (Get-Item -LiteralPath $logPath).Length
                $logSizeStr = " ($(Format-FileSize $logSize))"
            }
            $exit = if ($null -eq $run.ExitCode) { "skipped" } else { "$($run.ExitCode)" }
            $dur  = if ($null -eq $run.DurationMs) { "N/A" } else { "$($run.DurationMs)" }
            $lines += "| $($run.Title) | $outFile | $sizeStr | $($run.LogFile)$logSizeStr | $exit | $dur |"
        }
        $lines += ""

        # MFT file inventory for this drive
        $mftFiles = Get-ChildItem -LiteralPath $driveDir -Filter "${drive}_mft*" -ErrorAction SilentlyContinue
        if (@($mftFiles).Count -gt 0) {
            $lines += "## MFT Snapshots"
            $lines += ""
            $lines += "| File | Size |"
            $lines += "|------|------|"
            foreach ($mf in ($mftFiles | Sort-Object Name)) {
                $lines += "| $($mf.Name) | $(Format-FileSize $mf.Length) |"
            }
            $lines += ""
        }

        $lines | Out-File -FilePath $driveSummary -Encoding utf8
        Write-Host "📄 Drive summary: $driveSummary" -ForegroundColor DarkCyan
    }
}
finally {
    if ($sw) { $sw.Flush(); $sw.Dispose() }
    if ($fs) { $fs.Dispose() }

    try {
        # Copy report into each drive dir (each folder is self-contained)
        foreach ($d in $Drives) {
            $dd = Join-Path $WorkDir "drive_$($d.ToLower())"
            if (Test-Path -LiteralPath $dd) {
                Copy-Item -LiteralPath $TempLog -Destination (Join-Path $dd "test_runs.md") -Force
                Write-Host "📄 Report: $(Join-Path $dd 'test_runs.md')" -ForegroundColor Cyan
            }
        }

        # Remove temp file — no root copy (drives may be captured at different times)
        Remove-Item -LiteralPath $TempLog -Force -ErrorAction SilentlyContinue
        # Also clean up any stale root test_runs.md from previous runs
        if (Test-Path -LiteralPath $FinalLog) {
            Remove-Item -LiteralPath $FinalLog -Force -ErrorAction SilentlyContinue
        }
    }
    catch {
        Write-Warning ("Failed to finalize report: " + $_.Exception.Message)
    }

    # ── ARTIFACT FAILURE SUMMARY ──
    # Print a loud, unmissable summary of any artifacts that were missing or too small.
    # Exit with non-zero code so the caller knows data collection was incomplete.
    if ($script:artifactFailures.Count -gt 0) {
        Write-Host ""
        Write-Host "╔══════════════════════════════════════════════════════════════╗" -ForegroundColor Red
        Write-Host "║                                                              ║" -ForegroundColor Red
        Write-Host "║   ❌  DATA COLLECTION INCOMPLETE — ARTIFACTS FAILED          ║" -ForegroundColor Red
        Write-Host "║                                                              ║" -ForegroundColor Red
        Write-Host "╚══════════════════════════════════════════════════════════════╝" -ForegroundColor Red
        Write-Host ""
        Write-Host "  $($script:artifactFailures.Count) artifact(s) failed:" -ForegroundColor Red
        Write-Host ""
        $idx = 0
        foreach ($fail in $script:artifactFailures) {
            $idx++
            Write-Host "    ${idx}. $fail" -ForegroundColor Yellow
        }
        Write-Host ""
        Write-Host "  Re-run this script to retry only the missing artifacts." -ForegroundColor Yellow
        Write-Host "  (Fresh artifacts < 7 days old and > 1 KB are automatically skipped.)" -ForegroundColor DarkGray
        Write-Host ""
        exit 1
    } else {
        Write-Host ""
        Write-Host "╔══════════════════════════════════════════════════════════════╗" -ForegroundColor Green
        Write-Host "║   ✅  ALL ARTIFACTS COLLECTED SUCCESSFULLY                   ║" -ForegroundColor Green
        Write-Host "╚══════════════════════════════════════════════════════════════╝" -ForegroundColor Green
        Write-Host ""
    }
}
