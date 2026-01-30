# trial_run.ps1 - UFFS Data Collection (MFT + three scan flows)
# Strategy:
#   - Never write binary outputs with Set-Content.
#   - Capture stdout/stderr to .log files only (text).
#   - Sequential per physical disk; parallel across physical disks (PS7+).
[CmdletBinding()]
param(
    [string[]]$Drives = @(),       # Drives to test (empty = auto-detect NTFS drives)
    [switch]$SkipMft,              # Skip uffs_mft save tests
    [string]$BinDir = "",          # Custom bin directory (default: $HOME\bin)
    [int]$ThrottleLimit = 2        # Max physical disks in parallel (PS7+ only)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$env:RUST_BACKTRACE = "full"

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
    if (-not $SkipMft -and $hasMft -and $Drives.Count -gt 0) {
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
    $runDiskGroup = {
        param(
            [int]$DiskNumber,
            [string[]]$GroupDrives,
            [string]$WorkDir,
            [string]$UffsExe,
            [string]$UffsCom,
            [bool]$HasRust,
            [bool]$HasCpp
        )

        $groupResults = @()

        foreach ($Drive in $GroupDrives) {
            $driveLower = $Drive.ToLower()

            $rustOut    = "rust_${driveLower}.txt"
            $cppOut     = "cpp_${driveLower}.txt"
            $rustNewOut = "rust_new_${driveLower}.txt"

            $rustLog    = "rust_${driveLower}.log"
            $cppLog     = "cpp_${driveLower}.log"
            $rustNewLog = "rust_new_${driveLower}.log"

            function Run-LoggedLocal {
                param([string]$Title, [string]$CmdLine, [string]$LogFileName)

                $logPath = Join-Path $WorkDir $LogFileName
                $started = Get-Date
                $exitCode = 0

                try {
                    $lines = @(& cmd.exe /c $CmdLine 2>&1)
                    $exitCode = $LASTEXITCODE
                    $lines | Set-Content -LiteralPath $logPath -Encoding UTF8
                } catch {
                    $exitCode = -1
                    @("PowerShell exception:", $_.Exception.ToString()) | Set-Content -LiteralPath $logPath -Encoding UTF8
                }

                $ended = Get-Date
                $durMs = [math]::Round((New-TimeSpan -Start $started -End $ended).TotalMilliseconds)

                return [pscustomobject]@{
                    Drive      = $Drive
                    Title      = $Title
                    Command    = $CmdLine
                    LogFile    = $LogFileName
                    DurationMs = $durMs
                    ExitCode   = $exitCode
                }
            }

            $runs = @()

            if ($HasRust) {
                $runs += Run-LoggedLocal -Title "Rust (current): drive $Drive" `
                    -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive > `"$rustOut`"") `
                    -LogFileName $rustLog
            } else {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="Rust (current)"; Command=""; LogFile=$rustLog; DurationMs=$null; ExitCode=$null }
            }

            if ($HasCpp) {
                $runs += Run-LoggedLocal -Title "C++: drive $Drive" `
                    -CmdLine ("`"$UffsCom`" `"*`" --drives=$Drive > `"$cppOut`"") `
                    -LogFileName $cppLog
            } else {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="C++"; Command=""; LogFile=$cppLog; DurationMs=$null; ExitCode=$null }
            }

            if ($HasRust) {
                $runs += Run-LoggedLocal -Title "Rust (new tree): drive $Drive" `
                    -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive --tree-algo=cpp > `"$rustNewOut`"") `
                    -LogFileName $rustNewLog
            } else {
                $runs += [pscustomobject]@{ Drive=$Drive; Title="Rust (new tree)"; Command=""; LogFile=$rustNewLog; DurationMs=$null; ExitCode=$null }
            }

            $groupResults += [pscustomobject]@{
                Disk   = $DiskNumber
                Drive  = $Drive
                Files  = [pscustomobject]@{ Rust=$rustOut; Cpp=$cppOut; RustNew=$rustNewOut }
                Logs   = [pscustomobject]@{ Rust=$rustLog; Cpp=$cppLog; RustNew=$rustNewLog }
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
                -UffsExe $UffsExe -UffsCom $UffsCom -HasRust $hasRust -HasCpp $hasCpp
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
                -UffsExe $using:UffsExe -UffsCom $using:UffsCom -HasRust $using:hasRust -HasCpp $using:hasCpp
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
            $outFile = ""
            if ($run.Title -like "Rust (current)*") { $outFile = $r.Files.Rust }
            elseif ($run.Title -like "C++*") { $outFile = $r.Files.Cpp }
            elseif ($run.Title -like "Rust (new tree)*") { $outFile = $r.Files.RustNew }

            $outPath = if ($outFile) { Join-Path $WorkDir $outFile } else { $null }
            $sizeStr = "N/A"
            if ($outPath -and (Test-Path -LiteralPath $outPath)) {
                $sizeStr = Format-FileSize (Get-Item -LiteralPath $outPath).Length
            }

            $exit = if ($null -eq $run.ExitCode) { "skipped" } else { "$($run.ExitCode)" }
            $dur  = if ($null -eq $run.DurationMs) { "N/A" } else { "$($run.DurationMs)" }

            LogLine "| $($run.Title) | $outFile | $sizeStr | $($run.LogFile) | $exit | $dur |"
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
