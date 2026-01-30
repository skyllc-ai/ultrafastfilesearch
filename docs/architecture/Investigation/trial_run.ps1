# trial_run.ps1 - UFFS Data Collection (MFT + three scan flows)
# Purpose: save MFT and run three scan flows per drive; do NOT perform any comparisons or line counts.
[CmdletBinding()]
param(
    [string[]]$Drives = @(),      # Drives to test (empty = auto-detect NTFS drives)
    [switch]$SkipMft,             # Skip uffs_mft save tests
    [string]$BinDir = "",         # Custom bin directory (default: $HOME\bin)
    [int]$ThrottleLimit = 2       # Parallelism level (PS7+ only). Keep low to avoid disk contention.
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Enable full Rust backtraces for debugging panics
$env:RUST_BACKTRACE = "full"

$WorkDir  = Get-Location
$FinalLog = Join-Path $WorkDir "trial_run.md"
$TempLog  = Join-Path $WorkDir "trial_run.md.tmp"

# Storage for simple timing results
$script:TimingResults = New-Object System.Collections.Generic.List[object]()
$script:DriveResults  = New-Object System.Collections.Generic.List[object]()

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

# Markdown log writer (single-threaded; main thread only)
$fs = New-Object System.IO.FileStream(
$TempLog,
[System.IO.FileMode]::Create,
[System.IO.FileAccess]::Write,
[System.IO.FileShare]::ReadWrite
)
$sw = New-Object System.IO.StreamWriter($fs, [System.Text.Encoding]::UTF8)
$sw.NewLine = "`r`n"
function LogLine { param([string]$Line = "") $sw.WriteLine($Line); $sw.Flush() }

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

$script:TimingResults.Add([pscustomobject]@{
Title      = $Title
DurationMs = $durMs
ExitCode   = $exitCode
OutFile    = $OutFilePath
}) | Out-Null

return [pscustomobject]@{
Title      = $Title
DurationMs = $durMs
ExitCode   = $exitCode
OutFile    = $OutFilePath
}
}

# Worker for scanning a single drive (safe to call in PS7 parallel)
function Invoke-DriveScans {
param(
[string]$Drive,
[string]$WorkDir,
[string]$UffsExe,
[string]$UffsCom,
[bool]$HasRust,
[bool]$HasCpp
)

# Keep everything self-contained; no logging from parallel workers.
$driveLower = $Drive.ToLower()
$rustFile = "rust_${driveLower}.txt"
$cppFile = "cpp_${driveLower}.txt"
$rustNewFile = "rust_new_${driveLower}.txt"

$results = New-Object System.Collections.Generic.List[object]()

function Run-Cmd {
param([string]$Title, [string]$CmdLine, [string]$OutFile)

$started = Get-Date
$exitCode = 0
$outputLines = @()

try {
$raw = @(& cmd.exe /c $CmdLine 2>&1)
$exitCode = $LASTEXITCODE
$outputLines = Ensure-Array $raw
} catch {
$outputLines = @("PowerShell exception:", $_.Exception.ToString())
try { $exitCode = $LASTEXITCODE } catch { $exitCode = -1 }
}

if ($OutFile) {
$outPath = Join-Path $WorkDir $OutFile
$outputLines | Set-Content -LiteralPath $outPath -Encoding UTF8
}

$ended = Get-Date
$dur = New-TimeSpan -Start $started -End $ended
$durMs = [math]::Round($dur.TotalMilliseconds)

$outSize = $null
if ($OutFile) {
$outPath = Join-Path $WorkDir $OutFile
if (Test-Path -LiteralPath $outPath) {
$outSize = (Get-Item -LiteralPath $outPath).Length
}
}

return [pscustomobject]@{
Drive      = $Drive
Title      = $Title
Command    = $CmdLine
OutFile    = $OutFile
DurationMs = $durMs
ExitCode   = $exitCode
OutBytes   = $outSize
}
}

if ($HasRust) {
$results.Add((Run-Cmd `
            -Title "Rust (current): uffs scan drive $Drive" `
            -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive") `
            -OutFile $rustFile
)) | Out-Null
} else {
$results.Add([pscustomobject]@{
Drive=$Drive; Title="Rust (current)"; Command=""; OutFile=$rustFile; DurationMs=$null; ExitCode=$null; OutBytes=$null
}) | Out-Null
}

if ($HasCpp) {
$results.Add((Run-Cmd `
            -Title "C++: uffs.com scan drive $Drive" `
            -CmdLine ("`"$UffsCom`" `"*`" --drives=$Drive") `
            -OutFile $cppFile
)) | Out-Null
} else {
$results.Add([pscustomobject]@{
Drive=$Drive; Title="C++"; Command=""; OutFile=$cppFile; DurationMs=$null; ExitCode=$null; OutBytes=$null
}) | Out-Null
}

if ($HasRust) {
$results.Add((Run-Cmd `
            -Title "Rust (new tree algo): uffs scan drive $Drive --tree-algo=cpp" `
            -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive --tree-algo=cpp") `
            -OutFile $rustNewFile
)) | Out-Null
} else {
$results.Add([pscustomobject]@{
Drive=$Drive; Title="Rust (new tree algo)"; Command=""; OutFile=$rustNewFile; DurationMs=$null; ExitCode=$null; OutBytes=$null
}) | Out-Null
}

return [pscustomobject]@{
Drive  = $Drive
Files  = [pscustomobject]@{ Rust=$rustFile; Cpp=$cppFile; RustNew=$rustNewFile }
Runs   = @($results)
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

Write-Host "Binaries:" -ForegroundColor Yellow
Write-Host "  uffs.exe (Rust): $(if ($hasRust) { '✅' } else { '❌' }) $UffsExe"
Write-Host "  uffs.com (C++):  $(if ($hasCpp) { '✅' } else { '❌' }) $UffsCom"
Write-Host "  uffs_mft.exe:    $(if ($hasMft) { '✅' } else { '❌' }) $UffsMftExe"
Write-Host ""

if ($Drives.Count -eq 0) {
$Drives = @(Get-NtfsDrives)
Write-Host "Auto-detected NTFS drives: $($Drives -join ', ')" -ForegroundColor Yellow
}
LogLine ("**Drives to test:** " + ($Drives -join ", "))
LogLine ""

if ($hasRust) {
Invoke-Logged -Title "uffs --version" -CommandLine ("`"$UffsExe`" --version")
}

# MFT saves once (first drive), serial
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

# ============================================================
# Per-drive scans (parallel across drives in PS7+)
# ============================================================
LogLine "---"
LogLine ""
LogLine "# Drive Scans (three flows per drive)"
LogLine ""

$isPS7Plus = ($PSVersionTable.PSVersion.Major -ge 7)
LogLine ("- **Parallel mode:** " + $(if ($isPS7Plus) { "Enabled (ThrottleLimit=$ThrottleLimit)" } else { "Disabled (PowerShell < 7)" }))
LogLine ""

Write-Host ""
Write-Host "Running per-drive scans..." -ForegroundColor Cyan
if ($isPS7Plus) {
Write-Host "Parallel mode ON (ThrottleLimit=$ThrottleLimit)" -ForegroundColor Yellow
} else {
Write-Host "Parallel mode OFF (PowerShell < 7). Running serial." -ForegroundColor Yellow
}
Write-Host ""

$driveScanResults = @()

if ($isPS7Plus) {
# Parallel across drives; each worker does 3 scans serially for that drive
$driveScanResults = $Drives | ForEach-Object -Parallel {
Invoke-DriveScans -Drive $_ `
                -WorkDir $using:WorkDir `
                -UffsExe $using:UffsExe `
                -UffsCom $using:UffsCom `
                -HasRust $using:hasRust `
                -HasCpp $using:hasCpp
} -ThrottleLimit $ThrottleLimit
} else {
foreach ($d in $Drives) {
$driveScanResults += Invoke-DriveScans -Drive $d `
                -WorkDir $WorkDir `
                -UffsExe $UffsExe `
                -UffsCom $UffsCom `
                -HasRust $hasRust `
                -HasCpp $hasCpp
}
}

# Log results (main thread only)
foreach ($r in $driveScanResults) {
$drive = $r.Drive
LogLine "---"
LogLine ""
LogLine "# Drive $drive — Data Collection"
LogLine ""

Write-Host "Drive $drive complete:" -ForegroundColor Cyan

foreach ($run in $r.Runs) {
if ($null -eq $run.ExitCode) {
Write-Host "  - $($run.Title): skipped" -ForegroundColor DarkYellow
LogLine ("- **" + $run.Title + "**: skipped (binary missing)")
continue
}

$ok = ($run.ExitCode -eq 0)
$icon = if ($ok) { "✅" } else { "❌" }
$sizeStr = if ($null -ne $run.OutBytes) { Format-FileSize $run.OutBytes } else { "N/A" }

Write-Host ("  - " + $run.Title + " => " + $icon + " " + $run.DurationMs + " ms") -ForegroundColor (if ($ok) { "Green" } else { "Red" })
LogLine ("- **" + $run.Title + "**: " + $icon + " (" + $run.DurationMs + " ms), exit=" + $run.ExitCode + ", file=" + $run.OutFile + ", size=" + $sizeStr)

$script:TimingResults.Add([pscustomobject]@{
Title      = $run.Title
DurationMs = $run.DurationMs
ExitCode   = $run.ExitCode
OutFile    = $run.OutFile
}) | Out-Null
}

LogLine ""
LogLine "Saved files for drive ${drive}:"
LogLine ""
foreach ($f in @($r.Files.Rust, $r.Files.Cpp, $r.Files.RustNew)) {
$fPath = Join-Path $WorkDir $f
if (Test-Path -LiteralPath $fPath) {
$size = (Get-Item -LiteralPath $fPath).Length
LogLine "- $f ($(Format-FileSize $size))"
} else {
LogLine "- $f (missing)"
}
}
LogLine ""
}

# Final timing summary
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
