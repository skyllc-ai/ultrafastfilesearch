# trial_run.ps1 - UFFS Three-Way Comparison Tool
# Compares: Rust (current) vs C++ vs Rust (new tree algo)
#
# This script runs three implementations and compares their output:
#   1. Rust (current) - existing uffs.exe with current tree algorithm
#   2. C++ - reference uffs.com implementation
#   3. Rust (new tree algo) - uffs.exe with --tree-algo=cpp flag
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
                Drive = $script:Matches[1]
                Treesize = $script:Matches[2]
                Descendants = [int]$script:Matches[3]
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
        if ($countOutput -and $countOutput -match ': (\d+)') {
            $result.RustLines = [int]$script:Matches[1]
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
        if ($countOutput -and $countOutput -match ': (\d+)') {
            $result.CppLines = [int]$script:Matches[1]
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
            if ($countOutput -and $countOutput -match ': (\d+)') {
                $lineCount = [int]$script:Matches[1]
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

    # Test each drive (Rust vs C++ vs Rust-New comparison)
    foreach ($drive in $Drives) {
        LogLine "---"
        LogLine ""
        LogLine "# Drive $drive - Three-Way Scan Comparison"
        LogLine ""

        Write-Host ""
        Write-Host "Testing Drive $drive (3-way comparison)..." -ForegroundColor Cyan
        Write-Host ""

        $rustFile = "rust_$($drive.ToLower()).txt"
        $cppFile = "cpp_$($drive.ToLower()).txt"
        $rustNewFile = "rust_new_$($drive.ToLower()).txt"
        $rustExitCode = -1
        $cppExitCode = -1
        $rustNewExitCode = -1

        # Current Rust scan (existing tree algorithm)
        if ($hasRust) {
            $rustResult = Invoke-Logged -Title "Rust (current): uffs scan drive $drive" `
                -CommandLine ("`"$UffsExe`" `"*`" --drive $drive") `
                -OutFilePath $rustFile `
                -RecordTiming
            $rustExitCode = $rustResult.ExitCode
        }

        # C++ scan (reference implementation)
        if ($hasCpp) {
            $cppResult = Invoke-Logged -Title "C++: uffs.com scan drive $drive" `
                -CommandLine ("`"$UffsCom`" `"*`" --drives=$drive") `
                -OutFilePath $cppFile `
                -RecordTiming
            $cppExitCode = $cppResult.ExitCode
        }

        # New Rust scan (with C++ tree algorithm port)
        # Uses --tree-algo=cpp flag to enable the new algorithm
        if ($hasRust) {
            $rustNewResult = Invoke-Logged -Title "Rust (new tree algo): uffs scan drive $drive --tree-algo=cpp" `
                -CommandLine ("`"$UffsExe`" `"*`" --drive $drive --tree-algo=cpp") `
                -OutFilePath $rustNewFile `
                -RecordTiming
            $rustNewExitCode = $rustNewResult.ExitCode
        }

        # Compare all three outputs
        if (-not $SkipComparison) {
            $rustPath = Join-Path $WorkDir $rustFile
            $cppPath = Join-Path $WorkDir $cppFile
            $rustNewPath = Join-Path $WorkDir $rustNewFile

            # Extract metrics from all three
            $rustMetrics = if (Test-Path -LiteralPath $rustPath) { Extract-RootMetrics -FilePath $rustPath } else { $null }
            $cppMetrics = if (Test-Path -LiteralPath $cppPath) { Extract-RootMetrics -FilePath $cppPath } else { $null }
            $rustNewMetrics = if (Test-Path -LiteralPath $rustNewPath) { Extract-RootMetrics -FilePath $rustNewPath } else { $null }

            # Line counts (fast method)
            $rustLines = 0; $cppLines = 0; $rustNewLines = 0
            if (Test-Path -LiteralPath $rustPath) {
                $countOutput = & cmd.exe /c "find /c /v `"`" `"$rustPath`"" 2>$null
                if ($countOutput -and $countOutput -match ': (\d+)') { $rustLines = [int]$script:Matches[1] }
            }
            if (Test-Path -LiteralPath $cppPath) {
                $countOutput = & cmd.exe /c "find /c /v `"`" `"$cppPath`"" 2>$null
                if ($countOutput -and $countOutput -match ': (\d+)') { $cppLines = [int]$script:Matches[1] }
            }
            if (Test-Path -LiteralPath $rustNewPath) {
                $countOutput = & cmd.exe /c "find /c /v `"`" `"$rustNewPath`"" 2>$null
                if ($countOutput -and $countOutput -match ': (\d+)') { $rustNewLines = [int]$script:Matches[1] }
            }

            LogLine "### Three-Way Comparison: Drive $drive"
            LogLine ""

            # Show failure status
            if ($rustExitCode -ne 0) { LogLine "**⚠️ Rust (current) FAILED (exit code: $rustExitCode)**"; LogLine "" }
            if ($cppExitCode -ne 0) { LogLine "**⚠️ C++ FAILED (exit code: $cppExitCode)**"; LogLine "" }
            if ($rustNewExitCode -ne 0) { LogLine "**⚠️ Rust (new) FAILED (exit code: $rustNewExitCode)**"; LogLine "" }

            LogLine "| Metric | Rust (current) | C++ | Rust (new tree) | C++ Match |"
            LogLine "|--------|----------------|-----|-----------------|-----------|"

            # Exit codes
            $exitMatch = ($cppExitCode -eq 0 -and $rustNewExitCode -eq 0)
            LogLine "| Exit Code | $rustExitCode | $cppExitCode | $rustNewExitCode | $(if ($exitMatch) { '✅' } else { '❌' }) |"

            # Line counts
            $linesMatch = ($cppLines -eq $rustNewLines)
            LogLine "| Lines | $rustLines | $cppLines | $rustNewLines | $(if ($linesMatch) { '✅' } else { '⚠️' }) |"

            # Treesize
            $rustTs = if ($rustMetrics) { $rustMetrics.Treesize } else { "N/A" }
            $cppTs = if ($cppMetrics) { $cppMetrics.Treesize } else { "N/A" }
            $rustNewTs = if ($rustNewMetrics) { $rustNewMetrics.Treesize } else { "N/A" }
            $tsMatch = ($cppTs -eq $rustNewTs -and $cppTs -ne "N/A")
            LogLine "| Root Treesize | $rustTs | $cppTs | $rustNewTs | $(if ($tsMatch) { '✅' } else { '❌' }) |"

            # Descendants
            $rustDesc = if ($rustMetrics) { $rustMetrics.Descendants } else { 0 }
            $cppDesc = if ($cppMetrics) { $cppMetrics.Descendants } else { 0 }
            $rustNewDesc = if ($rustNewMetrics) { $rustNewMetrics.Descendants } else { 0 }
            $descMatch = ($cppDesc -eq $rustNewDesc -and $cppDesc -gt 0)
            LogLine "| Root Descendants | $rustDesc | $cppDesc | $rustNewDesc | $(if ($descMatch) { '✅' } else { '❌' }) |"

            LogLine ""

            # Store comparison for summary
            $comparison = @{
                Drive = $drive
                RustExitCode = $rustExitCode
                CppExitCode = $cppExitCode
                RustNewExitCode = $rustNewExitCode
                RustLines = $rustLines
                CppLines = $cppLines
                RustNewLines = $rustNewLines
                RustTreesize = $rustTs
                CppTreesize = $cppTs
                RustNewTreesize = $rustNewTs
                RustDescendants = $rustDesc
                CppDescendants = $cppDesc
                RustNewDescendants = $rustNewDesc
                RustFailed = ($rustExitCode -ne 0)
                CppFailed = ($cppExitCode -ne 0)
                RustNewFailed = ($rustNewExitCode -ne 0)
                TreesizeMatch = $tsMatch
                DescendantsMatch = $descMatch
                NewMatchesCpp = ($tsMatch -and $descMatch -and $exitMatch)
            }
            $script:ComparisonResults += $comparison

            # Console summary
            if ($comparison.RustNewFailed) {
                Write-Host "  ❌ RUST (new) FAILED for drive $drive (exit: $rustNewExitCode)" -ForegroundColor Red
            } elseif ($comparison.NewMatchesCpp) {
                Write-Host "  ✅ NEW RUST matches C++ for drive $drive" -ForegroundColor Green
            } else {
                Write-Host "  ⚠️ Differences detected for drive $drive" -ForegroundColor Yellow
            }
        }
    }

    # =========================================================================
    # Tree Algorithm Performance Benchmark - Timing Comparison
    # =========================================================================
    LogLine "---"
    LogLine ""
    LogLine "# Tree Algorithm Performance (Timing)"
    LogLine ""
    LogLine "Comparing **timing** of C++ 'Preprocess' phase vs Rust 'Tree Metrics' computation."
    LogLine "This measures how fast each implementation computes tree metrics (treesize, descendants)."
    LogLine ""

    Write-Host ""
    Write-Host "Tree Algorithm Performance Benchmark..." -ForegroundColor Cyan
    Write-Host ""

    $script:TreeBenchResults = @()

    foreach ($drive in $Drives) {
        $cppPreprocessMs = $null
        $rustTreeMetricsMs = $null

        # Run C++ benchmark-index to get Preprocess timing
        if ($hasCpp) {
            Write-Host "  [C++] benchmark-index=$drive..." -NoNewline
            $cppOutput = & $UffsCom "--benchmark-index=$drive`:" 2>&1
            $cppExitCode = $LASTEXITCODE

            if ($cppExitCode -eq 0) {
                foreach ($line in $cppOutput) {
                    if ($line -match 'Preprocess[:\s]+(\d+)\s*ms') {
                        $cppPreprocessMs = [int]$script:Matches[1]
                    }
                }
                Write-Host " ✅ Preprocess: $cppPreprocessMs ms" -ForegroundColor Green
            } else {
                Write-Host " ❌ Failed (exit: $cppExitCode)" -ForegroundColor Red
            }
        }

        # Run Rust benchmark-tree to get Tree Metrics timing
        if ($hasMft) {
            Write-Host "  [Rust] benchmark-tree --drive $drive..." -NoNewline
            $rustOutput = & $UffsMftExe benchmark-tree --drive $drive --iterations 3 2>&1
            $rustExitCode = $LASTEXITCODE

            if ($rustExitCode -eq 0) {
                foreach ($line in $rustOutput) {
                    if ($line -match 'Avg[:\s]+(\d+)\s*ms') {
                        $rustTreeMetricsMs = [int]$script:Matches[1]
                    }
                }
                Write-Host " ✅ Tree Metrics: $rustTreeMetricsMs ms (avg)" -ForegroundColor Green
            } else {
                Write-Host " ❌ Failed (exit: $rustExitCode)" -ForegroundColor Red
            }
        }

        # Calculate speedup
        $speedup = $null
        $winner = "N/A"
        if ($cppPreprocessMs -and $rustTreeMetricsMs -and $rustTreeMetricsMs -gt 0) {
            $speedup = [math]::Round($cppPreprocessMs / $rustTreeMetricsMs, 2)
            $winner = if ($speedup -gt 1) { "Rust" } elseif ($speedup -lt 1) { "C++" } else { "Tie" }
        }

        $script:TreeBenchResults += @{
            Drive = $drive
            CppPreprocessMs = $cppPreprocessMs
            RustTreeMetricsMs = $rustTreeMetricsMs
            Speedup = $speedup
            Winner = $winner
        }
    }

    # Log tree benchmark results
    LogLine "## Tree Metrics Comparison (C++ Preprocess vs Rust Tree Metrics)"
    LogLine ""
    LogLine "| Drive | C++ Preprocess | Rust Tree Metrics | Speedup | Winner |"
    LogLine "|-------|----------------|-------------------|---------|--------|"

    foreach ($r in $script:TreeBenchResults) {
        $cppStr = if ($r.CppPreprocessMs) { "$($r.CppPreprocessMs) ms" } else { "N/A" }
        $rustStr = if ($r.RustTreeMetricsMs) { "$($r.RustTreeMetricsMs) ms" } else { "N/A" }
        $speedupStr = if ($r.Speedup) { "$($r.Speedup)x" } else { "N/A" }
        LogLine "| $($r.Drive) | $cppStr | $rustStr | $speedupStr | $($r.Winner) |"
    }
    LogLine ""
    LogLine "> **Note:** Speedup > 1.0 means Rust is faster than C++."
    LogLine ""

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

    # Three-way comparison summary
    if (@($script:ComparisonResults).Count -gt 0) {
        LogLine "## Three-Way Parity Results"
        LogLine ""
        LogLine "Comparing: **Rust (current)** vs **C++** vs **Rust (new tree algo)**"
        LogLine ""
        LogLine "| Drive | Rust (current) | C++ | Rust (new) | New vs C++ |"
        LogLine "|-------|----------------|-----|------------|------------|"
        foreach ($c in $script:ComparisonResults) {
            $rustStatus = if ($c.RustFailed) { "❌" } else { "✅" }
            $cppStatus = if ($c.CppFailed) { "❌" } else { "✅" }
            $rustNewStatus = if ($c.RustNewFailed) { "❌" } else { "✅" }
            $matchIcon = if ($c.RustNewFailed) { "❌ FAILED" } elseif ($c.NewMatchesCpp) { "✅ MATCH" } else { "⚠️ DIFF" }
            LogLine "| $($c.Drive) | $rustStatus $($c.RustTreesize) | $cppStatus $($c.CppTreesize) | $rustNewStatus $($c.RustNewTreesize) | $matchIcon |"
        }
        LogLine ""

        # Detailed breakdown
        LogLine "### Detailed Metrics"
        LogLine ""
        LogLine "| Drive | Metric | Rust (current) | C++ | Rust (new) |"
        LogLine "|-------|--------|----------------|-----|------------|"
        foreach ($c in $script:ComparisonResults) {
            LogLine "| $($c.Drive) | Treesize | $($c.RustTreesize) | $($c.CppTreesize) | $($c.RustNewTreesize) |"
            LogLine "| $($c.Drive) | Descendants | $($c.RustDescendants) | $($c.CppDescendants) | $($c.RustNewDescendants) |"
            LogLine "| $($c.Drive) | Lines | $($c.RustLines) | $($c.CppLines) | $($c.RustNewLines) |"
        }
        LogLine ""

        # Summary counts
        $newMatchCount = @($script:ComparisonResults | Where-Object { $_.NewMatchesCpp }).Count
        $newFailedCount = @($script:ComparisonResults | Where-Object { $_.RustNewFailed }).Count
        $totalDrives = @($script:ComparisonResults).Count

        if ($newFailedCount -gt 0) {
            LogLine "> **❌ Rust (new tree algo) failed on $newFailedCount drive(s) - fix required!**"
            Write-Host ""
            Write-Host "❌ Rust (new tree algo) failed on $newFailedCount drive(s) - review trial_run.md for details." -ForegroundColor Red
        } elseif ($newMatchCount -eq $totalDrives) {
            LogLine "> **🎉 NEW RUST TREE ALGO matches C++ on all $totalDrives drive(s)!**"
            Write-Host ""
            Write-Host "🎉 NEW RUST TREE ALGO matches C++ on all $totalDrives drive(s)!" -ForegroundColor Green
        } else {
            $diffCount = $totalDrives - $newMatchCount
            LogLine "> **⚠️ New Rust tree algo differs from C++ on $diffCount drive(s) - review details above.**"
            Write-Host ""
            Write-Host "⚠️ New Rust tree algo differs from C++ on $diffCount drive(s) - review trial_run.md for details." -ForegroundColor Yellow
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
