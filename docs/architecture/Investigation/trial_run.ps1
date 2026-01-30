# trial_run.ps1 - UFFS Rust vs C++ Comparison Tool
# Enhanced version with automatic comparison and metrics extraction
#
# Usage:
#   .\trial_run.ps1                    # Test all available NTFS drives
#   .\trial_run.ps1 -Drives F          # Test only drive F
#   .\trial_run.ps1 -Drives F,G        # Test drives F and G
#   .\trial_run.ps1 -SkipMft           # Skip MFT save tests
#   .\trial_run.ps1 -Verbose           # Show progress on console

[CmdletBinding()]
param(
    [string[]]$Drives = @(),           # Drives to test (empty = auto-detect NTFS drives)
    [switch]$SkipMft,                  # Skip uffs_mft save tests
    [switch]$SkipComparison,           # Skip Rust vs C++ comparison
    [string]$BinDir = ""               # Custom bin directory (default: $HOME\bin)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Enable full Rust backtraces for debugging panics
$env:RUST_BACKTRACE = "full"

$WorkDir = Get-Location
$FinalLog = Join-Path $WorkDir "trial_run.md"
$TempLog = Join-Path $WorkDir "trial_run.md.tmp"

# Timing results for summary
$script:TimingResults = @()
$script:ComparisonResults = @()

function Ensure-Array {
    param($v)
    if ($null -eq $v) { return @() }
    if ($v -is [System.Collections.IEnumerable] -and -not ($v -is [string])) { return @($v) }
    return @([string]$v)
}

function Format-FileSize {
    param([long]$Bytes)
    if ($Bytes -ge 1GB) { return "{0:N2} GB" -f ($Bytes / 1GB) }
    if ($Bytes -ge 1MB) { return "{0:N2} MB" -f ($Bytes / 1MB) }
    if ($Bytes -ge 1KB) { return "{0:N2} KB" -f ($Bytes / 1KB) }
    return "$Bytes bytes"
}

function Get-NtfsDrives {
    # Get all NTFS drives on the system
    Get-WmiObject Win32_LogicalDisk |
        Where-Object { $_.DriveType -eq 3 -and $_.FileSystem -eq "NTFS" } |
        ForEach-Object { $_.DeviceID.TrimEnd(':') }
}

function Extract-RootMetrics {
    param([string]$FilePath)
    # Extract root entry metrics from uffs output
    # Format: "G:\	<DIR>	581.64 MB	15119	2025-01-27 21:50:09"
    if (-not (Test-Path -LiteralPath $FilePath)) { return $null }

    $content = Get-Content -LiteralPath $FilePath -TotalCount 5
    foreach ($line in $content) {
        # Match root entry pattern: "X:\<TAB><DIR><TAB>size<TAB>descendants<TAB>date"
        if ($line -match '^([A-Z]):\\\t<DIR>\t([^\t]+)\t(\d+)\t') {
            return @{
                Drive = $Matches[1]
                Treesize = $Matches[2]
                Descendants = [int]$Matches[3]
            }
        }
    }
    return $null
}

function Compare-Outputs {
    param(
        [string]$RustFile,
        [string]$CppFile,
        [string]$Drive,
        [int]$RustExitCode = 0,
        [int]$CppExitCode = 0
    )

    $result = @{
        Drive = $Drive
        RustFile = $RustFile
        CppFile = $CppFile
        RustExists = Test-Path -LiteralPath $RustFile
        CppExists = Test-Path -LiteralPath $CppFile
        RustLines = 0
        CppLines = 0
        RustTreesize = ""
        CppTreesize = ""
        RustDescendants = 0
        CppDescendants = 0
        Match = $false
        TreesizeMatch = $false
        DescendantsMatch = $false
        RustExitCode = $RustExitCode
        CppExitCode = $CppExitCode
        RustFailed = ($RustExitCode -ne 0)
        CppFailed = ($CppExitCode -ne 0)
    }

    if ($result.RustExists) {
        # FAST line count using find /c /v "" (native Windows, ~100x faster than Get-Content)
        # This counts lines by counting non-matches of empty string
        $countOutput = & cmd.exe /c "find /c /v `"`" `"$RustFile`"" 2>$null
        if ($countOutput -match ': (\d+)') {
            $result.RustLines = [int]$Matches[1]
        }
        $rustMetrics = Extract-RootMetrics -FilePath $RustFile
        if ($rustMetrics) {
            $result.RustTreesize = $rustMetrics.Treesize
            $result.RustDescendants = $rustMetrics.Descendants
        }
    }

    if ($result.CppExists) {
        # FAST line count using find /c /v "" (native Windows, ~100x faster than Get-Content)
        $countOutput = & cmd.exe /c "find /c /v `"`" `"$CppFile`"" 2>$null
        if ($countOutput -match ': (\d+)') {
            $result.CppLines = [int]$Matches[1]
        }
        $cppMetrics = Extract-RootMetrics -FilePath $CppFile
        if ($cppMetrics) {
            $result.CppTreesize = $cppMetrics.Treesize
            $result.CppDescendants = $cppMetrics.Descendants
        }
    }

    # Only consider it a match if both succeeded and have valid data
    if ($result.RustFailed -or $result.CppFailed) {
        $result.TreesizeMatch = $false
        $result.DescendantsMatch = $false
        $result.Match = $false
    } elseif ([string]::IsNullOrEmpty($result.RustTreesize) -or [string]::IsNullOrEmpty($result.CppTreesize)) {
        # If either has no treesize data, it's not a valid comparison
        $result.TreesizeMatch = $false
        $result.DescendantsMatch = $false
        $result.Match = $false
    } else {
        $result.TreesizeMatch = ($result.RustTreesize -eq $result.CppTreesize)
        $result.DescendantsMatch = ($result.RustDescendants -eq $result.CppDescendants)
        $result.Match = $result.TreesizeMatch -and $result.DescendantsMatch
    }

    return $result
}

# Create temp writer (avoid locking final file while writing)
$fs = New-Object System.IO.FileStream(
    $TempLog,
    [System.IO.FileMode]::Create,
    [System.IO.FileAccess]::Write,
    [System.IO.FileShare]::ReadWrite
)
$sw = New-Object System.IO.StreamWriter($fs, [System.Text.Encoding]::UTF8)
$sw.NewLine = "`r`n"

function LogLine {
    param([string]$Line = "")
    $sw.WriteLine($Line)
    $sw.Flush()
    if ($VerbosePreference -eq 'Continue') {
        Write-Host $Line
    }
}

function Invoke-Logged {
    param(
        [string]$Title = "",
        [string]$CommandLine = "",
        [string]$OutFilePath = "",
        [switch]$RecordTiming
    )

    LogLine ("## " + $Title)
    LogLine ""
    LogLine "**Command:**"
    LogLine ""
    LogLine '```text'
    LogLine $CommandLine
    LogLine '```'
    LogLine ""

    $started = Get-Date
    LogLine ("**Started:** " + $started.ToString("o"))
    LogLine ""

    $status = "OK"
    $exitCode = 0
    $outputLines = @()

    Write-Host "  → $Title..." -NoNewline

    try {
        $raw = @(& cmd.exe /c $CommandLine 2>&1)
        $exitCode = $LASTEXITCODE
        $outputLines = Ensure-Array $raw
    }
    catch {
        $status = "ERROR"
        $outputLines = @("PowerShell exception:", $_.Exception.ToString())
        try { $exitCode = $LASTEXITCODE } catch { $exitCode = -1 }
    }

    if ($OutFilePath) {
        $outPath = Join-Path $WorkDir $OutFilePath
        $outputLines | Set-Content -LiteralPath $outPath -Encoding UTF8
    }

    $ended = Get-Date
    $dur = New-TimeSpan -Start $started -End $ended
    $durMs = [math]::Round($dur.TotalMilliseconds)

    if ($exitCode -eq 0) {
        Write-Host " ✅ ($durMs ms)" -ForegroundColor Green
    } else {
        Write-Host " ❌ (exit: $exitCode)" -ForegroundColor Red
    }

    LogLine ("**Ended:** " + $ended.ToString("o"))
    LogLine ("**Duration:** " + $dur.ToString() + " ($durMs ms)")
    LogLine ("**Exit code:** " + $exitCode)
    LogLine ("**Status:** " + $status)
    if ($OutFilePath) {
        LogLine ("**Output file:** " + $OutFilePath)
        $outPath = Join-Path $WorkDir $OutFilePath
        if (Test-Path -LiteralPath $outPath) {
            $fileInfo = Get-Item -LiteralPath $outPath
            # FAST line count using find /c /v "" (native Windows, ~100x faster than Get-Content)
            $countOutput = & cmd.exe /c "find /c /v `"`" `"$outPath`"" 2>$null
            $lineCount = 0
            if ($countOutput -match ': (\d+)') {
                $lineCount = [int]$Matches[1]
            }
            LogLine ("**Output file size:** " + (Format-FileSize $fileInfo.Length))
            LogLine ("**Output line count:** " + $lineCount)
        }
    }
    LogLine ""
    LogLine "**Console output:**"
    LogLine ""
    LogLine '```text'

    $n = ($outputLines | Measure-Object).Count
    if ($n -gt 0) {
        # Limit console output in log to first 50 lines
        $shown = 0
        foreach ($l in $outputLines) {
            if ($shown -lt 50) {
                LogLine $l
                $shown++
            }
        }
        if ($n -gt 50) {
            LogLine "... ($($n - 50) more lines)"
        }
    } else {
        LogLine "(no output)"
    }

    LogLine '```'
    LogLine ""

    if ($RecordTiming) {
        $script:TimingResults += @{
            Title = $Title
            Duration = $dur
            DurationMs = $durMs
            ExitCode = $exitCode
            Status = $status
        }
    }

    return @{
        Duration = $dur
        DurationMs = $durMs
        ExitCode = $exitCode
        Status = $status
        LineCount = $n
    }
}

try {
    Write-Host ""
    Write-Host "╔══════════════════════════════════════════════════════════════╗" -ForegroundColor Cyan
    Write-Host "║         UFFS Trial Run - Rust vs C++ Comparison              ║" -ForegroundColor Cyan
    Write-Host "╚══════════════════════════════════════════════════════════════╝" -ForegroundColor Cyan
    Write-Host ""

    LogLine "# UFFS Trial Run Report"
    LogLine ""
    LogLine ("- **Started:** " + (Get-Date -Format o))
    LogLine ("- **Working dir:** " + $WorkDir.ToString())
    LogLine ("- **User:** " + (whoami))
    LogLine ("- **Computer:** " + $env:COMPUTERNAME)
    LogLine ("- **PowerShell:** " + $PSVersionTable.PSVersion.ToString())
    LogLine ""

    # Determine bin directory
    if (-not $BinDir) { $BinDir = Join-Path $HOME "bin" }

    $UffsExe = Join-Path $BinDir "uffs.exe"
    $UffsCom = Join-Path $BinDir "uffs.com"
    $UffsMftExe = Join-Path $BinDir "uffs_mft.exe"

    $hasRust = Test-Path -LiteralPath $UffsExe
    $hasCpp = Test-Path -LiteralPath $UffsCom
    $hasMft = Test-Path -LiteralPath $UffsMftExe

    LogLine "## Environment"
    LogLine ""
    LogLine "| Binary | Path | Exists |"
    LogLine "|--------|------|--------|"
    LogLine ("| uffs.exe (Rust) | ``$UffsExe`` | " + $(if ($hasRust) { "✅" } else { "❌" }) + " |")
    LogLine ("| uffs.com (C++) | ``$UffsCom`` | " + $(if ($hasCpp) { "✅" } else { "❌" }) + " |")
    LogLine ("| uffs_mft.exe | ``$UffsMftExe`` | " + $(if ($hasMft) { "✅" } else { "❌" }) + " |")
    LogLine ""

    Write-Host "Binaries:" -ForegroundColor Yellow
    Write-Host "  uffs.exe (Rust): $(if ($hasRust) { '✅' } else { '❌' }) $UffsExe"
    Write-Host "  uffs.com (C++):  $(if ($hasCpp) { '✅' } else { '❌' }) $UffsCom"
    Write-Host "  uffs_mft.exe:    $(if ($hasMft) { '✅' } else { '❌' }) $UffsMftExe"
    Write-Host ""

    # Determine drives to test
    if ($Drives.Count -eq 0) {
        $Drives = @(Get-NtfsDrives)
        Write-Host "Auto-detected NTFS drives: $($Drives -join ', ')" -ForegroundColor Yellow
    }

    LogLine ("**Drives to test:** " + ($Drives -join ", "))
    LogLine ""

    # Version check
    if ($hasRust) {
        LogLine "## Version Information"
        LogLine ""
        Invoke-Logged -Title "uffs --version" -CommandLine ("`"$UffsExe`" --version")
    }

    # MFT save tests (run once for first drive only)
    if (-not $SkipMft -and $hasMft -and $Drives.Count -gt 0) {
        $mftDrive = $Drives[0]
        LogLine "---"
        LogLine ""
        LogLine "# MFT Save Tests (Drive $mftDrive)"
        LogLine ""

        Write-Host ""
        Write-Host "MFT Save Tests (Drive $mftDrive)..." -ForegroundColor Cyan
        Write-Host ""

        $mftBin = "${mftDrive}_mft.bin"
        $mftRaw = "${mftDrive}_mft.raw"
        $mftNoCompress = "${mftDrive}_mft_no_compress.bin"

        Invoke-Logged -Title "uffs_mft save (compressed)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o $mftBin") `
            -RecordTiming

        Invoke-Logged -Title "uffs_mft save (no compress)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive --output $mftNoCompress --no-compress") `
            -RecordTiming

        Invoke-Logged -Title "uffs_mft save (raw)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o $mftRaw --raw") `
            -RecordTiming

        # Report file sizes
        LogLine "### Generated MFT Files"
        LogLine ""
        LogLine "| File | Size |"
        LogLine "|------|------|"
        foreach ($f in @($mftBin, $mftNoCompress, $mftRaw)) {
            $fPath = Join-Path $WorkDir $f
            if (Test-Path -LiteralPath $fPath) {
                $size = (Get-Item -LiteralPath $fPath).Length
                LogLine "| $f | $(Format-FileSize $size) |"
            }
        }
        LogLine ""
    }

    # Test each drive (Rust vs C++ comparison)
    foreach ($drive in $Drives) {
        LogLine "---"
        LogLine ""
        LogLine "# Drive $drive - Scan Comparison"
        LogLine ""

        Write-Host ""
        Write-Host "Testing Drive $drive..." -ForegroundColor Cyan
        Write-Host ""

        $rustFile = "rust_$($drive.ToLower()).txt"
        $cppFile = "cpp_$($drive.ToLower()).txt"
        $rustExitCode = -1
        $cppExitCode = -1

        # Rust scan
        if ($hasRust) {
            $rustResult = Invoke-Logged -Title "Rust: uffs scan drive $drive" `
                -CommandLine ("`"$UffsExe`" `"*`" --drive $drive") `
                -OutFilePath $rustFile `
                -RecordTiming
            $rustExitCode = $rustResult.ExitCode
        }

        # C++ scan
        if ($hasCpp) {
            $cppResult = Invoke-Logged -Title "C++: uffs.com scan drive $drive" `
                -CommandLine ("`"$UffsCom`" `"*`" --drives=$drive") `
                -OutFilePath $cppFile `
                -RecordTiming
            $cppExitCode = $cppResult.ExitCode
        }

        # Compare outputs
        if ($hasRust -and $hasCpp -and -not $SkipComparison) {
            $rustPath = Join-Path $WorkDir $rustFile
            $cppPath = Join-Path $WorkDir $cppFile
            $comparison = Compare-Outputs -RustFile $rustPath -CppFile $cppPath -Drive $drive `
                -RustExitCode $rustExitCode -CppExitCode $cppExitCode
            $script:ComparisonResults += $comparison

            LogLine "### Comparison: Drive $drive"
            LogLine ""

            # Show failure status if either failed
            if ($comparison.RustFailed) {
                LogLine "**⚠️ Rust scan FAILED (exit code: $rustExitCode)**"
                LogLine ""
            }
            if ($comparison.CppFailed) {
                LogLine "**⚠️ C++ scan FAILED (exit code: $cppExitCode)**"
                LogLine ""
            }

            LogLine "| Metric | Rust | C++ | Match |"
            LogLine "|--------|------|-----|-------|"
            LogLine ("| Exit Code | $rustExitCode | $cppExitCode | " +
                $(if ($rustExitCode -eq 0 -and $cppExitCode -eq 0) { "✅" } else { "❌" }) + " |")
            LogLine ("| Lines | $($comparison.RustLines) | $($comparison.CppLines) | " +
                $(if ($comparison.RustLines -eq $comparison.CppLines) { "✅" } else { "⚠️" }) + " |")
            LogLine ("| Root Treesize | $($comparison.RustTreesize) | $($comparison.CppTreesize) | " +
                $(if ($comparison.TreesizeMatch) { "✅" } else { "❌" }) + " |")
            LogLine ("| Root Descendants | $($comparison.RustDescendants) | $($comparison.CppDescendants) | " +
                $(if ($comparison.DescendantsMatch) { "✅" } else { "❌" }) + " |")
            LogLine ""

            if ($comparison.RustFailed) {
                Write-Host "  ❌ RUST FAILED for drive $drive (exit: $rustExitCode)" -ForegroundColor Red
            } elseif ($comparison.Match) {
                Write-Host "  ✅ PARITY ACHIEVED for drive $drive" -ForegroundColor Green
            } else {
                Write-Host "  ⚠️ Differences detected for drive $drive" -ForegroundColor Yellow
            }
        }
    }

    # Summary section
    LogLine "---"
    LogLine ""
    LogLine "# Summary"
    LogLine ""

    # Timing summary
    if (@($script:TimingResults).Count -gt 0) {
        LogLine "## Timing Results"
        LogLine ""
        LogLine "| Test | Duration | Status |"
        LogLine "|------|----------|--------|"
        foreach ($t in $script:TimingResults) {
            $statusIcon = if ($t.ExitCode -eq 0) { "✅" } else { "❌" }
            LogLine "| $($t.Title) | $($t.DurationMs) ms | $statusIcon |"
        }
        LogLine ""
    }

    # Comparison summary
    if (@($script:ComparisonResults).Count -gt 0) {
        LogLine "## Parity Results"
        LogLine ""
        LogLine "| Drive | Rust Status | Treesize Match | Descendants Match | Overall |"
        LogLine "|-------|-------------|----------------|-------------------|---------|"
        foreach ($c in $script:ComparisonResults) {
            $rustStatus = if ($c.RustFailed) { "❌ FAILED" } else { "✅ OK" }
            $tsIcon = if ($c.TreesizeMatch) { "✅" } else { "❌" }
            $descIcon = if ($c.DescendantsMatch) { "✅" } else { "❌" }
            $overallIcon = if ($c.RustFailed) { "❌ RUST FAILED" } elseif ($c.Match) { "✅ PARITY" } else { "⚠️ DIFF" }
            LogLine "| $($c.Drive) | $rustStatus | $tsIcon $($c.RustTreesize) vs $($c.CppTreesize) | $descIcon $($c.RustDescendants) vs $($c.CppDescendants) | $overallIcon |"
        }
        LogLine ""

        # Use @() to force array and Measure-Object for reliable counting
        $failedCount = @($script:ComparisonResults | Where-Object { -not $_.Match }).Count
        $rustFailedCount = @($script:ComparisonResults | Where-Object { $_.RustFailed }).Count

        if ($rustFailedCount -gt 0) {
            LogLine "> **❌ Rust scan failed on $rustFailedCount drive(s) - fix required!**"
            Write-Host ""
            Write-Host "❌ Rust scan failed on $rustFailedCount drive(s) - review trial_run.md for details." -ForegroundColor Red
        } elseif ($failedCount -eq 0) {
            LogLine "> **🎉 100% PARITY ACHIEVED across all tested drives!**"
            Write-Host ""
            Write-Host "🎉 100% PARITY ACHIEVED across all tested drives!" -ForegroundColor Green
        } else {
            LogLine "> **⚠️ Some differences detected - review details above.**"
            Write-Host ""
            Write-Host "⚠️ Some differences detected - review trial_run.md for details." -ForegroundColor Yellow
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
