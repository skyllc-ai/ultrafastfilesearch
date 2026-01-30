# trial_run.ps1 - UFFS Data Collection (MFT + three scan flows)
# Purpose: save MFT and run three scan flows per drive; do NOT perform any comparisons or line counts.
[CmdletBinding()]
param(
    [string[]]$Drives = @(),      # Drives to test (empty = auto-detect NTFS drives)
    [switch]$SkipMft,             # Skip uffs_mft save tests
    [string]$BinDir = ""          # Custom bin directory (default: $HOME\bin)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Enable full Rust backtraces for debugging panics
$env:RUST_BACKTRACE = "full"

$WorkDir = Get-Location
$FinalLog = Join-Path $WorkDir "trial_run.md"
$TempLog = Join-Path $WorkDir "trial_run.md.tmp"

# Storage for simple timing results
$script:TimingResults = @()

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
    Get-WmiObject Win32_LogicalDisk |
            Where-Object { $_.DriveType -eq 3 -and $_.FileSystem -eq "NTFS" } |
            ForEach-Object { $_.DeviceID.TrimEnd(':') }
}

# Simplified logger that writes to a temp markdown and optionally echoes to host
$fs = New-Object System.IO.FileStream(
$TempLog,
[System.IO.FileMode]::Create,
[System.IO.FileAccess]::Write,
[System.IO.FileShare]::ReadWrite
)
$sw = New-Object System.IO.StreamWriter($fs, [System.Text.Encoding]::UTF8)
$sw.NewLine = "`r`n"
function LogLine { param([string]$Line = "") $sw.WriteLine($Line); $sw.Flush(); Write-Host $Line }

function Invoke-Logged {
    param(
        [string]$Title = "",
        [string]$CommandLine = "",
        [string]$OutFilePath = ""
    )

    LogLine ("## " + $Title)
    LogLine ""
    LogLine "**Command:**"
    LogLine '```text'
    LogLine $CommandLine
    LogLine '```'
    LogLine ""

    $started = Get-Date
    LogLine ("**Started:** " + $started.ToString("o"))
    LogLine ""

    $exitCode = 0
    $outputLines = @()
    Write-Host "  → $Title..." -NoNewline

    try {
        # Run the command via cmd.exe to preserve the same behaviour you used originally
        $raw = @(& cmd.exe /c $CommandLine 2>&1)
        $exitCode = $LASTEXITCODE
        $outputLines = Ensure-Array $raw
    }
    catch {
        $outputLines = @("PowerShell exception:", $_.Exception.ToString())
        try { $exitCode = $LASTEXITCODE } catch { $exitCode = -1 }
    }

    if ($OutFilePath) {
        $outPath = Join-Path $WorkDir $OutFilePath
        # Save raw output (no trimming)
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
    if ($OutFilePath) {
        LogLine ("**Output file:** " + $OutFilePath)
        $outPath = Join-Path $WorkDir $OutFilePath
        if (Test-Path -LiteralPath $outPath) {
            $fileInfo = Get-Item -LiteralPath $outPath
            LogLine ("**Output file size:** " + (Format-FileSize $fileInfo.Length))
        } else {
            LogLine "(output file not found)"
        }
    }
    LogLine ""
    LogLine "**Console output (first 200 lines shown):**"
    LogLine '```text'
    $shown = 0
    foreach ($l in $outputLines) {
        if ($shown -ge 200) { break }
        LogLine $l
        $shown++
    }
    if (($outputLines | Measure-Object).Count -gt $shown) { LogLine "... (truncated)" }
    LogLine '```'
    LogLine ""

    # record timing
    $script:TimingResults += @{
        Title = $Title
        DurationMs = $durMs
        ExitCode = $exitCode
        OutFile = $OutFilePath
    }

    return @{ Duration = $dur; DurationMs = $durMs; ExitCode = $exitCode }
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
    LogLine ""

    if (-not $BinDir) { $BinDir = Join-Path $HOME "bin" }

    $UffsExe = Join-Path $BinDir "uffs.exe"
    $UffsCom = Join-Path $BinDir "uffs.com"
    $UffsMftExe = Join-Path $BinDir "uffs_mft.exe"

    $hasRust = Test-Path -LiteralPath $UffsExe
    $hasCpp = Test-Path -LiteralPath $UffsCom
    $hasMft = Test-Path -LiteralPath $UffsMftExe

    LogLine "## Binaries"
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

    # Determine drives
    if ($Drives.Count -eq 0) {
        $Drives = @(Get-NtfsDrives)
        Write-Host "Auto-detected NTFS drives: $($Drives -join ', ')" -ForegroundColor Yellow
    }
    LogLine ("**Drives to test:** " + ($Drives -join ", "))
    LogLine ""

    # Version info if available
    if ($hasRust) {
        Invoke-Logged -Title "uffs --version" -CommandLine ("`"$UffsExe`" --version")
    }

    # MFT save tests — only once on first drive and only if present and not skipped
    if (-not $SkipMft -and $hasMft -and $Drives.Count -gt 0) {
        $mftDrive = $Drives[0]
        LogLine "---"
        LogLine ""
        LogLine "# MFT Save (Drive $mftDrive)"
        LogLine ""
        Write-Host "MFT Save (Drive $mftDrive)..." -ForegroundColor Cyan

        $mftBin = "${mftDrive}_mft.bin"
        $mftRaw = "${mftDrive}_mft.raw"
        $mftNoCompress = "${mftDrive}_mft_no_compress.bin"

        Invoke-Logged -Title "uffs_mft save (compressed)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o $mftBin") `
            -OutFilePath $mftBin

        Invoke-Logged -Title "uffs_mft save (no compress)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive --output $mftNoCompress --no-compress") `
            -OutFilePath $mftNoCompress

        Invoke-Logged -Title "uffs_mft save (raw)" `
            -CommandLine ("`"$UffsMftExe`" save --drive $mftDrive -o $mftRaw --raw") `
            -OutFilePath $mftRaw

        LogLine "### Generated MFT Files"
        LogLine ""
        LogLine "| File | Size |"
        LogLine "|------|------|"
        foreach ($f in @($mftBin, $mftNoCompress, $mftRaw)) {
            $fPath = Join-Path $WorkDir $f
            if (Test-Path -LiteralPath $fPath) {
                $size = (Get-Item -LiteralPath $fPath).Length
                LogLine "| $f | $(Format-FileSize $size) |"
            } else {
                LogLine "| $f | (missing) |"
            }
        }
        LogLine ""
    } else {
        if ($SkipMft) { LogLine "> MFT save skipped by -SkipMft flag." }
        elseif (-not $hasMft) { LogLine "> uffs_mft.exe not present; skipping MFT saves." }
    }

    # For each drive: run three flows and save outputs (no comparisons, no line counts)
    foreach ($drive in $Drives) {
        LogLine "---"
        LogLine ""
        LogLine "# Drive $drive — Data Collection"
        LogLine ""
        Write-Host "Running scans for drive $drive..." -ForegroundColor Cyan

        $rustFile = "rust_$($drive.ToLower()).txt"
        $cppFile = "cpp_$($drive.ToLower()).txt"
        $rustNewFile = "rust_new_$($drive.ToLower()).txt"

        if ($hasRust) {
            Invoke-Logged -Title "Rust (current): uffs scan drive $drive" `
                -CommandLine ("`"$UffsExe`" `"*`" --drive $drive") `
                -OutFilePath $rustFile
        } else {
            LogLine "**Skipped Rust (current): uffs.exe not found**"
        }

        if ($hasCpp) {
            Invoke-Logged -Title "C++: uffs.com scan drive $drive" `
                -CommandLine ("`"$UffsCom`" `"*`" --drives=$drive") `
                -OutFilePath $cppFile
        } else {
            LogLine "**Skipped C++: uffs.com not found**"
        }

        if ($hasRust) {
            Invoke-Logged -Title "Rust (new tree algo): uffs scan drive $drive --tree-algo=cpp" `
                -CommandLine ("`"$UffsExe`" `"*`" --drive $drive --tree-algo=cpp") `
                -OutFilePath $rustNewFile
        } else {
            LogLine "**Skipped Rust (new): uffs.exe not found**"
        }

        LogLine ""
        LogLine "Saved files for drive $drive:"
        LogLine ""
        foreach ($f in @($rustFile, $cppFile, $rustNewFile)) {
            $fPath = Join-Path $WorkDir $f
            if (Test-Path -LiteralPath $fPath) {
                $size = (Get-Item -LiteralPath $fPath).Length
                LogLine "- $f (`$(Format-FileSize $size)`)"
            } else {
                LogLine "- $f (missing)"
            }
        }
        LogLine ""
    }

    # Final simple timing summary
    LogLine "---"
    LogLine ""
    LogLine "# Timings (per-command)"
    LogLine ""
    LogLine "| Command | Duration (ms) | Exit | OutFile |"
    LogLine "|---------|---------------:|:----:|--------|"
    foreach ($t in $script:TimingResults) {
        LogLine ("| " + ($t.Title -replace '\|','/') + " | " + $t.DurationMs + " | " + $t.ExitCode + " | " + ($t.OutFile -replace '\|','/') + " |")
    }
    LogLine ""
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
