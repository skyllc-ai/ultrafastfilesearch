# benchmark_tree_comparison.ps1 - UFFS Performance Comparison (C++ vs Rust)
# Compares full pipeline and tree metrics performance between implementations
#
# Usage:
#   .\benchmark_tree_comparison.ps1 -Drive C              # Single drive
#   .\benchmark_tree_comparison.ps1 -Drives C,D,E         # Multiple drives
#   .\benchmark_tree_comparison.ps1 -Drive C -Iterations 10  # More iterations
#   .\benchmark_tree_comparison.ps1 -Drive C -FullPipeline   # Include I/O+Parse comparison
#   .\benchmark_tree_comparison.ps1 -Drive C -CppPort        # Use C++ port algorithms (apple-to-apple)

[CmdletBinding()]
param(
    [string]$Drive = "",                # Single drive to test
    [string[]]$Drives = @(),            # Multiple drives to test
    [int]$Iterations = 5,               # Number of iterations for Rust benchmark
    [string]$BinDir = "",               # Custom bin directory (default: $HOME\bin)
    [switch]$NoCache,                   # Skip cache, build fresh index
    [switch]$FullPipeline,              # Also run benchmark-index-lean for full comparison
    [switch]$CppPort                    # Use C++ port algorithms for Rust (apple-to-apple comparison)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Determine bin directory
if (-not $BinDir) { $BinDir = Join-Path $HOME "bin" }

$UffsCom = Join-Path $BinDir "uffs.com"
$UffsMftExe = Join-Path $BinDir "uffs_mft.exe"

# Validate binaries exist
$hasCpp = Test-Path -LiteralPath $UffsCom
$hasRust = Test-Path -LiteralPath $UffsMftExe

if (-not $hasCpp) {
    Write-Warning "C++ binary not found: $UffsCom"
}
if (-not $hasRust) {
    Write-Error "Rust binary not found: $UffsMftExe"
    exit 1
}

# Determine drives to test
if ($Drive) {
    $Drives = @($Drive.ToUpper())
} elseif ($Drives.Count -eq 0) {
    Write-Error "Specify -Drive or -Drives parameter"
    exit 1
} else {
    $Drives = $Drives | ForEach-Object { $_.ToUpper() }
}

Write-Host ""
Write-Host "╔══════════════════════════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║     UFFS Tree Metrics Benchmark - C++ vs Rust Comparison     ║" -ForegroundColor Cyan
Write-Host "╚══════════════════════════════════════════════════════════════╝" -ForegroundColor Cyan
Write-Host ""
Write-Host "Drives: $($Drives -join ', ')" -ForegroundColor Yellow
Write-Host "Iterations: $Iterations" -ForegroundColor Yellow
Write-Host "Cache: $(if ($NoCache) { 'disabled' } else { 'enabled' })" -ForegroundColor Yellow
Write-Host "C++ Port: $(if ($CppPort) { 'enabled (apple-to-apple)' } else { 'disabled (default Rust algos)' })" -ForegroundColor Yellow
Write-Host ""

# Set environment variables for C++ port algorithms if requested
if ($CppPort) {
    $env:UFFS_PARSE_ALGO = "cpp_port"
    $env:UFFS_TREE_ALGO = "cpp_port"
    Write-Host "Using C++ port algorithms for Rust benchmarks" -ForegroundColor Cyan
    Write-Host ""
}

$results = @()

foreach ($drv in $Drives) {
    Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host "  Testing Drive $drv" -ForegroundColor Cyan
    Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host ""

    $cppPreprocessMs = $null
    $rustTreeMetricsMs = $null
    $cppTotalMs = $null
    $rustTotalMs = $null

    # Run C++ benchmark-index
    if ($hasCpp) {
        Write-Host "  [C++] Running --benchmark-index=$drv`:..." -NoNewline
        $cppOutput = & $UffsCom "--benchmark-index=$drv`:" 2>&1
        $cppExitCode = $LASTEXITCODE

        if ($cppExitCode -eq 0) {
            # Parse C++ output for Preprocess timing
            foreach ($line in $cppOutput) {
                if ($line -match 'Preprocess[:\s]+(\d+)\s*ms') {
                    $cppPreprocessMs = [int]$Matches[1]
                }
                if ($line -match 'Total[:\s]+(\d+)\s*ms') {
                    $cppTotalMs = [int]$Matches[1]
                }
            }
            Write-Host " ✅ Preprocess: $cppPreprocessMs ms" -ForegroundColor Green
        } else {
            Write-Host " ❌ Failed (exit: $cppExitCode)" -ForegroundColor Red
        }
    }

    # Run Rust benchmark-tree
    Write-Host "  [Rust] Running benchmark-tree --drive $drv --iterations $Iterations..." -NoNewline
    $cacheArg = if ($NoCache) { "--no-cache" } else { "" }
    $rustOutput = & $UffsMftExe benchmark-tree --drive $drv --iterations $Iterations $cacheArg 2>&1
    $rustExitCode = $LASTEXITCODE

    if ($rustExitCode -eq 0) {
        # Parse Rust output for tree metrics timing
        foreach ($line in $rustOutput) {
            if ($line -match 'Avg[:\s]+(\d+)\s*ms') {
                $rustTreeMetricsMs = [int]$Matches[1]
            }
        }
        Write-Host " ✅ Tree Metrics: $rustTreeMetricsMs ms (avg)" -ForegroundColor Green
    } else {
        Write-Host " ❌ Failed (exit: $rustExitCode)" -ForegroundColor Red
    }

    # Run Rust benchmark-index-lean for full pipeline comparison (if requested)
    $rustIoMs = $null
    $rustParseMs = $null
    $rustMergeMs = $null
    $rustIoParseMergeMs = $null

    if ($FullPipeline) {
        Write-Host "  [Rust] Running benchmark-index-lean --drive $drv..." -NoNewline
        $leanOutput = & $UffsMftExe benchmark-index-lean --drive $drv 2>&1
        $leanExitCode = $LASTEXITCODE

        if ($leanExitCode -eq 0) {
            foreach ($line in $leanOutput) {
                if ($line -match 'I/O \(read\):\s+(\d+)\s*ms') {
                    $rustIoMs = [int]$Matches[1]
                }
                if ($line -match 'Parse:\s+(\d+)\s*ms') {
                    $rustParseMs = [int]$Matches[1]
                }
                if ($line -match 'Merge:\s+(\d+)\s*ms') {
                    $rustMergeMs = [int]$Matches[1]
                }
                if ($line -match 'I/O \+ Parse \+ Merge:\s+(\d+)\s*ms') {
                    $rustIoParseMergeMs = [int]$Matches[1]
                }
            }
            Write-Host " ✅ I/O: $rustIoMs ms, Parse: $rustParseMs ms, Merge: $rustMergeMs ms" -ForegroundColor Green
        } else {
            Write-Host " ❌ Failed (exit: $leanExitCode)" -ForegroundColor Red
        }
    }

    # Calculate comparison
    $speedup = $null
    if ($cppPreprocessMs -and $rustTreeMetricsMs -and $rustTreeMetricsMs -gt 0) {
        $speedup = [math]::Round($cppPreprocessMs / $rustTreeMetricsMs, 2)
    }

    $results += [PSCustomObject]@{
        Drive = $drv
        CppPreprocessMs = $cppPreprocessMs
        RustTreeMetricsMs = $rustTreeMetricsMs
        RustIoMs = $rustIoMs
        RustParseMs = $rustParseMs
        RustMergeMs = $rustMergeMs
        Speedup = $speedup
        Winner = if ($speedup -gt 1) { "Rust" } elseif ($speedup -lt 1) { "C++" } else { "Tie" }
    }

    Write-Host ""
}

# Summary - Tree Metrics Comparison
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host "  Summary - Tree Metrics (Preprocess) Comparison" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host ""
Write-Host "| Drive | C++ Preprocess | Rust Tree Metrics | Speedup | Winner |" -ForegroundColor White
Write-Host "|-------|----------------|-------------------|---------|--------|" -ForegroundColor White

foreach ($r in $results) {
    $cppStr = if ($r.CppPreprocessMs) { "$($r.CppPreprocessMs) ms" } else { "N/A" }
    $rustStr = if ($r.RustTreeMetricsMs) { "$($r.RustTreeMetricsMs) ms" } else { "N/A" }
    $speedupStr = if ($r.Speedup) { "$($r.Speedup)x" } else { "N/A" }
    $winnerColor = if ($r.Winner -eq "Rust") { "Green" } elseif ($r.Winner -eq "C++") { "Yellow" } else { "White" }
    Write-Host "| $($r.Drive)     | $cppStr | $rustStr | $speedupStr | " -NoNewline
    Write-Host "$($r.Winner)" -ForegroundColor $winnerColor -NoNewline
    Write-Host " |"
}

Write-Host ""
Write-Host "Note: Speedup > 1.0 means Rust is faster" -ForegroundColor Gray

# Full Pipeline Breakdown (if -FullPipeline was used)
if ($FullPipeline) {
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host "  Full Pipeline Breakdown (Rust - Accurate Timing)" -ForegroundColor Cyan
    Write-Host "═══════════════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "| Drive | I/O (read) | Parse | Merge | Total I/O+Parse+Merge |" -ForegroundColor White
    Write-Host "|-------|------------|-------|-------|----------------------|" -ForegroundColor White

    foreach ($r in $results) {
        $ioStr = if ($r.RustIoMs) { "$($r.RustIoMs) ms" } else { "N/A" }
        $parseStr = if ($r.RustParseMs) { "$($r.RustParseMs) ms" } else { "N/A" }
        $mergeStr = if ($r.RustMergeMs) { "$($r.RustMergeMs) ms" } else { "N/A" }
        $totalStr = if ($r.RustIoMs -and $r.RustParseMs -and $r.RustMergeMs) {
            "$($r.RustIoMs + $r.RustParseMs + $r.RustMergeMs) ms"
        } else { "N/A" }
        Write-Host "| $($r.Drive)     | $ioStr | $parseStr | $mergeStr | $totalStr |"
    }

    Write-Host ""
    Write-Host "Note: These are ACCURATE timings (not estimates) from instrumented reader" -ForegroundColor Gray
}

Write-Host ""
