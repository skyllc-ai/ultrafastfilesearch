# trial_run.ps1 - UFFS Live Data Collection (Windows only)
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
#
# What gets collected:
#   1. MFT snapshots (compressed .bin files) - for offline analysis on Mac
#   2. C++ baseline scan output - reference for parity comparison
#   3. Rust LIVE scan output + diagnostic logs - with chunk/record processing stats
#
# Diagnostic logging captures (in .log files):
#   - Chunk handoff, record boundaries, preload_concurrent timing
#   - USA fixup success/failure, records parsed, records not in-use
#   - Parallel sync (lock acquisition), chunk processing order
#
# After running this script, transfer all files to Mac for offline analysis using:
#   - uffs "*" --mft-file <mft_file> --drive <letter> for offline search
#   - uffs-diag tools for detailed comparison
#   - See: TESTING_TOOLS_GUIDE.md for full workflow
[CmdletBinding()]
param(
    [string[]]$Drives = @(),       # Drives to test (empty = auto-detect NTFS drives)
    [switch]$SkipMft = $true,      # Skip uffs_mft save tests (default: true - MFT collection confirmed OK)
    [switch]$CollectMft,           # Force MFT collection (overrides SkipMft)
    [string]$BinDir = "",          # Custom bin directory (default: $HOME\bin)
    [int]$ThrottleLimit = 2,       # Max physical disks in parallel (PS7+ only)
    [switch]$VerboseLog            # Enable verbose/trace logging (more detail, larger logs)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$env:RUST_BACKTRACE = "full"

# Enable diagnostic logging for live path analysis
# info level shows summary diagnostics; debug/trace shows per-chunk details
if ($VerboseLog) {
    $env:RUST_LOG = "uffs_mft::cpp_types=trace,uffs_mft::cpp_io_pipeline=debug,uffs_mft::reader=info,info"
    Write-Host "📋 Verbose logging enabled (trace level)" -ForegroundColor Yellow
} else {
    $env:RUST_LOG = "uffs_mft::cpp_types=info,uffs_mft::cpp_io_pipeline=info,uffs_mft::reader=info,info"
    Write-Host "📋 Standard logging enabled (info level)" -ForegroundColor Yellow
}

$WorkDir  = Get-Location
$FinalLog = Join-Path $WorkDir "trial_run.md"
$TempLog  = Join-Path $WorkDir "trial_run.md.tmp"

function Format-FileSize {
    param([long]$Bytes)
    if ($Bytes -ge 1GB) { return "{0:N2} GB" -f ($Bytes / 1GB) }
    if ($Bytes -ge 1MB) { return "{0:N2} MB" -f ($Bytes / 1MB) }
    if ($Bytes -ge 1KB) { return "{0:N2} KB" -f ($Bytes / 1KB) }
    return "$Bytes bytes"
}

function Get-NtfsDrives {
    Get-WmiObject Win32_LogicalDisk |
            Where-Object { $_.DriveType -eq 3 -and $_.FileSystem -eq "NTFS" } |
            ForEach-Object { $_.DeviceID.TrimEnd(':') }
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
        [string]$LogFileName
    )

    $logPath = Join-Path $WorkDir $LogFileName
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
    Write-Host "║    UFFS Trial Run — Data Collection    ║" -ForegroundColor Cyan
    Write-Host "╚════════════════════════════════════════╝" -ForegroundColor Cyan
    Write-Host ""

    LogLine "# UFFS Trial Run Report (data collection only)"
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
    $UffsMftExe = Join-Path $BinDir "uffs_mft.exe"

    $hasRust = Test-Path -LiteralPath $UffsExe
    $hasCpp  = Test-Path -LiteralPath $UffsCom
    $hasMft  = Test-Path -LiteralPath $UffsMftExe

    LogLine "## Binaries"
    LogLine ""
    LogLine "| Binary | Path | Exists |"
    LogLine "|--------|------|--------|"
    LogLine ("| uffs.exe (Rust) | ``$UffsExe`` | " + $(if ($hasRust) { "✅" } else { "❌" }) + " |")
    LogLine ("| uffs.com (C++) | ``$UffsCom`` | " + $(if ($hasCpp) { "✅" } else { "❌" }) + " |")
    LogLine ("| uffs_mft.exe | ``$UffsMftExe`` | " + $(if ($hasMft) { "✅" } else { "❌" }) + " |")
    LogLine ""

    if ($Drives.Count -eq 0) {
        $Drives = @(Get-NtfsDrives)
        Write-Host "Auto-detected NTFS drives: $($Drives -join ', ')" -ForegroundColor Yellow
    }

    LogLine ("**Drives to test:** " + ($Drives -join ", "))
    LogLine ""

    # Version check
    $timings = @()
    if ($hasRust) {
        $timings += Invoke-CmdToLog -Title "uffs --version" `
            -CommandLine ("`"$UffsExe`" --version") `
            -LogFileName "uffs_version.log"
        LogLine "- Version log: ``uffs_version.log``"
        LogLine ""
    }

    # MFT saves (only first drive), sequential; binaries write output files themselves.
    # Default: skipped (MFT collection confirmed 100% OK). Use -CollectMft to force collection.
    $doMftCollection = $CollectMft -or (-not $SkipMft)
    if ($doMftCollection -and $hasMft -and $Drives.Count -gt 0) {
        $mftDrive = $Drives[0]
        Write-Host "MFT Save (Drive $mftDrive)..." -ForegroundColor Cyan

        LogLine "---"
        LogLine ""
        LogLine "# MFT Save (Drive $mftDrive)"
        LogLine ""

        $mftBin        = "${mftDrive}_mft.bin"
        $mftNoCompress = "${mftDrive}_mft_no_compress.bin"
        $mftRaw        = "${mftDrive}_mft.raw"

        $timings += Invoke-CmdToLog -Title "uffs_mft save (compressed)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o `"$mftBin`"") `
            -LogFileName "${mftDrive}_mft_save_compressed.log"

        $timings += Invoke-CmdToLog -Title "uffs_mft save (no compress)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive --output `"$mftNoCompress`" --no-compress") `
            -LogFileName "${mftDrive}_mft_save_no_compress.log"

        $timings += Invoke-CmdToLog -Title "uffs_mft save (raw)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o `"$mftRaw`" --raw") `
            -LogFileName "${mftDrive}_mft_save_raw.log"

        LogLine "### Generated MFT Files"
        LogLine ""
        LogLine "| File | Size |"
        LogLine "|------|------|"
        foreach ($f in @($mftBin, $mftNoCompress, $mftRaw)) {
            $p = Join-Path $WorkDir $f
            if (Test-Path -LiteralPath $p) {
                $size = (Get-Item -LiteralPath $p).Length
                LogLine "| $f | $(Format-FileSize $size) |"
            } else {
                LogLine "| $f | (missing) |"
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
            [bool]$HasRust,
            [bool]$HasCpp,
            [bool]$HasMft
        )

        $groupResults = @()

        foreach ($Drive in $GroupDrives) {
            $driveLower = $Drive.ToLower()

            # Output files
            $cppOut         = "cpp_${driveLower}.txt"
            $rustLiveOut    = "rust_live_${driveLower}.txt"
            $rustOfflineOut = "rust_offline_${driveLower}.txt"

            # Log files (capture stderr with diagnostics)
            $cppLog         = "cpp_${driveLower}.log"
            $rustLiveLog    = "rust_live_${driveLower}.log"
            $rustOfflineLog = "rust_offline_${driveLower}.log"

            # MFT file for offline comparison
            $mftBin = "${driveLower}_mft.bin"

            function Run-LoggedLocal {
                param([string]$Title, [string]$CmdLine, [string]$LogFileName, [string]$OutFileName = "")

                $logPath = Join-Path $WorkDir $LogFileName
                $started = Get-Date
                $exitCode = 0

                Write-Host "  → $Title..." -NoNewline

                try {
                    # Run command with stdout to output file, stderr to log file
                    # This properly separates scan output from diagnostic logs
                    if ($OutFileName) {
                        $outPath = Join-Path $WorkDir $OutFileName
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

            # 1. C++ baseline (no diagnostics, just output)
            if ($HasCpp) {
                $runs += Run-LoggedLocal -Title "C++ (baseline): drive $Drive" `
                    -CmdLine ("`"$UffsCom`" `"*`" --drives=$Drive") `
                    -LogFileName $cppLog `
                    -OutFileName $cppOut
            } else {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="C++ (baseline)"; Command=""; LogFile=$cppLog; OutFile=$cppOut; DurationMs=$null; ExitCode=$null }
            }

            # 2. Rust LIVE scan (with diagnostic logging via RUST_LOG)
            if ($HasRust) {
                $runs += Run-LoggedLocal -Title "Rust LIVE (cpp io): drive $Drive" `
                    -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive --parse-algo=cpp_port --tree-algo=cpp --io-algo=cpp --no-bitmap") `
                    -LogFileName $rustLiveLog `
                    -OutFileName $rustLiveOut
            } else {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="Rust LIVE (cpp io)"; Command=""; LogFile=$rustLiveLog; OutFile=$rustLiveOut; DurationMs=$null; ExitCode=$null }
            }

            # 3. Rust OFFLINE scan - SKIPPED on Windows
            # Offline analysis is done on Mac for faster iteration (see TESTING_TOOLS_GUIDE.md)
            Write-Host "  → Rust OFFLINE: skipped (offline analysis done on Mac)" -ForegroundColor DarkGray

            $groupResults += [pscustomobject]@{
                Disk   = $DiskNumber
                Drive  = $Drive
                Files  = [pscustomobject]@{ Cpp=$cppOut; RustLive=$rustLiveOut; RustOffline=$rustOfflineOut }
                Logs   = [pscustomobject]@{ Cpp=$cppLog; RustLive=$rustLiveLog; RustOffline=$rustOfflineLog }
                Runs   = $runs
            }
        }

        return $groupResults
    }

    $scanResults = @()

    if (-not $allMapped -or -not $isPS7Plus -or $Drives.Count -le 1) {
        # Safe fallback: fully sequential
        Write-Host "Drive scans: running sequential (single drive / PS<7 / mapping unavailable)." -ForegroundColor Yellow

        foreach ($d in $Drives) {
            # treat each drive as its own "disk group"
            $scanResults += & $runDiskGroup -DiskNumber -1 -GroupDrives @($d) -WorkDir $WorkDir `
                -UffsExe $UffsExe -UffsCom $UffsCom -UffsMftExe $UffsMftExe `
                -HasRust $hasRust -HasCpp $hasCpp -HasMft $hasMft
        }
    } else {
        # Parallel across physical disks; sequential within each disk
        Write-Host "Drive scans: parallel across physical disks (ThrottleLimit=$ThrottleLimit), sequential within each disk." -ForegroundColor Yellow

        $diskNumbers = @($diskGroups.Keys | Sort-Object)
        $scanResults = $diskNumbers | ForEach-Object -Parallel {
            $diskNum = $_
            $allDiskGroups = $using:diskGroups
            $groupDrives = $allDiskGroups[$diskNum]
            & $using:runDiskGroup -DiskNumber $diskNum -GroupDrives $groupDrives -WorkDir $using:WorkDir `
                -UffsExe $using:UffsExe -UffsCom $using:UffsCom -UffsMftExe $using:UffsMftExe `
                -HasRust $using:hasRust -HasCpp $using:hasCpp -HasMft $using:hasMft
        } -ThrottleLimit $ThrottleLimit
    }

    # Consolidate results into markdown (single thread)
    LogLine "---"
    LogLine ""
    LogLine "# Scan Outputs"
    LogLine ""

    foreach ($r in $scanResults) {
        $drive = $r.Drive
        $disk  = $r.Disk

        LogLine "## Drive $drive (Disk $disk)"
        LogLine ""

        LogLine "| Flow | Output file | Size | Log file | Exit | Duration (ms) |"
        LogLine "|------|-------------|------|----------|------|---------------:|"

        foreach ($run in $r.Runs) {
            $outFile = $run.OutFile
            $outPath = if ($outFile) { Join-Path $WorkDir $outFile } else { $null }
            $sizeStr = "N/A"
            if ($outPath -and (Test-Path -LiteralPath $outPath)) {
                $sizeStr = Format-FileSize (Get-Item -LiteralPath $outPath).Length
            }

            $logPath = if ($run.LogFile) { Join-Path $WorkDir $run.LogFile } else { $null }
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
}
finally {
    if ($sw) { $sw.Flush(); $sw.Dispose() }
    if ($fs) { $fs.Dispose() }

    try {
        if (Test-Path -LiteralPath $FinalLog) { Remove-Item -LiteralPath $FinalLog -Force }
        Move-Item -LiteralPath $TempLog -Destination $FinalLog -Force
        Write-Host ""
        Write-Host "📄 Report written: $FinalLog" -ForegroundColor Cyan
    }
    catch {
        $fallback = Join-Path $WorkDir ("trial_run_" + (Get-Date -Format "yyyyMMdd_HHmmss") + ".md")
        Move-Item -LiteralPath $TempLog -Destination $fallback -Force
        Write-Warning ("trial_run.md was locked; wrote: " + $fallback)
    }
}
