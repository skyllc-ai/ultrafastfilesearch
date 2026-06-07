# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.


# Cross-tool benchmark (UFFS vs Everything vs UFFS-C++).
#
# Thin wrapper around `scripts/windows/cross-tool-benchmark.rs` that
# uses the binary already installed in `~/bin/` (i.e. the one produced
# by `just use-local`).  This is the command used to generate the numbers
# tracked in `docs/research/perf-phase2-measurement-plan.md` §0.1.
#
# Arguments:
#   drives    — comma-separated drive letters, e.g. "D" or "C,D".
#               Defaults to "D" (the long-lived reference drive).
#   rounds    — iterations per pattern per tool.  More = narrower p50
#               confidence interval, slower run.  Defaults to 30.
#   patterns  — comma-separated pattern IDs passed through to the
#               script.  Defaults to the five core patterns.  Pass
#               "all" to include the cold/aggregate patterns too.
#   tools     — comma-separated tool IDs (uffs, everything, uffscpp).
#               Defaults to uffs + everything.
#
# Examples:
#   just bench-cross
#   just bench-cross D 30 exact,prefix,ext_rare,ext_dll,substring
#   just bench-cross C,D 50 all uffs,everything,uffscpp
[windows]
bench-cross drives="D" rounds="30" patterns="exact,prefix,ext_rare,ext_dll,substring" tools="uffs,everything":
    #!powershell
    $ErrorActionPreference = 'Stop'
    $uffsBin = Join-Path $HOME 'bin\uffs.exe'
    if (-not (Test-Path $uffsBin)) {
        Write-Host "[bench-cross] uffs.exe missing at $uffsBin" -ForegroundColor Red
        Write-Host "              Run 'just use-local' first to install the current workspace binary." -ForegroundColor DarkGray
        exit 1
    }
    Write-Host "[bench-cross] drives={{drives}} rounds={{rounds}} patterns={{patterns}} tools={{tools}}" -ForegroundColor Blue
    Write-Host "              uffs-bin: $uffsBin`n" -ForegroundColor DarkGray
    rust-script scripts\windows\cross-tool-benchmark.rs `
        --skip-cold `
        --drives '{{drives}}' `
        --rounds '{{rounds}}' `
        --patterns '{{patterns}}' `
        --tools '{{tools}}' `
        --uffs-bin $uffsBin
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Verify the Windows launcher has the expected delay-load table.
#
# `dumpbin /imports` lists BOTH the regular import table and the
# delay-load table in one output, which makes it impossible to tell
# whether a DLL is delay-loaded just by grepping for its name.  This
# recipe dumps the `.didata` section separately and succeeds only when
# every DLL declared in `crates/uffs-cli/build.rs` is actually present
# in the delay-load table.
[windows]
bench-verify:
    #!powershell
    $ErrorActionPreference = 'Stop'
    $bin = Join-Path $HOME 'bin\uffs.exe'
    if (-not (Test-Path $bin)) {
        Write-Host "[bench-verify] uffs.exe missing at $bin -- run 'just use-local' first" -ForegroundColor Red
        exit 1
    }

    # Find dumpbin.exe via vswhere.
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        Write-Host "[bench-verify] vswhere.exe not found; install Visual Studio 2022 Build Tools" -ForegroundColor Red
        exit 1
    }
    $vsPath = & $vswhere -latest -property installationPath
    $dumpbin = Get-ChildItem -Path $vsPath -Filter dumpbin.exe -Recurse -ErrorAction SilentlyContinue |
               Where-Object { $_.FullName -match 'Hostx64\\x64' } |
               Select-Object -First 1 -ExpandProperty FullName
    if (-not $dumpbin) {
        Write-Host "[bench-verify] dumpbin.exe not found under $vsPath" -ForegroundColor Red
        exit 1
    }
    Write-Host "[bench-verify] dumpbin: $dumpbin" -ForegroundColor DarkGray

    # Parse /imports output and split into regular vs delay-load.
    $raw = & $dumpbin /imports $bin 2>&1 | Out-String
    $regularBlock = ($raw -split 'Section contains the following delay load imports:')[0]
    $delayBlock   = ($raw -split 'Section contains the following delay load imports:')[1]

    $expected = @('combase.dll', 'oleaut32.dll')
    $missing  = @()
    foreach ($dll in $expected) {
        if (-not $delayBlock -or $delayBlock -notmatch [regex]::Escape($dll)) {
            $missing += $dll
        }
    }

    Write-Host "`nRegular imports (excerpt):" -ForegroundColor Blue
    ($regularBlock -split "`n" | Where-Object { $_ -match '\.dll' } | Select-Object -First 20) |
        ForEach-Object { "  $($_.Trim())" }

    Write-Host "`nDelay-load imports:" -ForegroundColor Blue
    if ($delayBlock) {
        ($delayBlock -split "`n" | Where-Object { $_ -match '\.dll' }) |
            ForEach-Object { "  $($_.Trim())" }
    } else {
        Write-Host "  (no delay-load table found)" -ForegroundColor Yellow
    }

    Write-Host ""
    if ($missing.Count -eq 0) {
        Write-Host "[bench-verify] OK: all expected DLLs are delay-loaded" -ForegroundColor Green
        exit 0
    } else {
        Write-Host "[bench-verify] FAIL: these DLLs should be delay-loaded but are not:" -ForegroundColor Red
        $missing | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
        Write-Host "`n  Check crates/uffs-cli/build.rs and rebuild with 'just ship'." -ForegroundColor DarkGray
        exit 1
    }

# Build a launcher variant with the C runtime statically linked.
#
# The shipped `uffs.exe` dynamically links to the Universal CRT, which
# pulls in six small `api-ms-win-crt-*.dll` forwarder DLLs (heap, math,
# locale, runtime, stdio, plus the synch core).  Each is a separate
# `LdrLoadDll` at process start.  Building with
# `-C target-feature=+crt-static` statically links libucrt.lib +
# libvcruntime.lib instead, so those DLLs disappear from the import
# table.
#
# Trade-offs:
#   + 6 fewer DLL loads on every launch (estimated 3-5 ms p50 saved)
#   + Less surface area for "wrong-CRT" mismatch issues on user systems
#   - Binary grows by ~100-200 KB (no shared CRT page caching)
#   - No automatic CRT security updates from the OS
#
# The build uses a sibling target directory so the main release cache
# is NOT invalidated by the rustflags change.  The resulting binary is
# installed as `~/bin/uffs-crt-static.exe` so it can be A/B benched
# against the shipping `~/bin/uffs.exe`.
#
# A/B benchmark with:
#   just bench-cross           # ~/bin/uffs.exe          (shipping)
#   just bench-cross-static    # ~/bin/uffs-crt-static.exe (variant)
#
# See docs/research/perf-phase2-measurement-plan.md §2.2 for the
# expected gain and the full A/B methodology.
[windows]
ship-crt-static:
    #!powershell
    $ErrorActionPreference = 'Stop'
    $env:RUSTFLAGS = "-C target-feature=+crt-static"

    # Use a sibling target directory so the main release build cache
    # is not invalidated by the rustflags change.  Honors any active
    # CARGO_TARGET_DIR (e.g. C:\rust-target\ttapi).
    if ($env:CARGO_TARGET_DIR) {
        $targetDir = "$env:CARGO_TARGET_DIR-crt-static"
    } else {
        $targetDir = "target-crt-static"
    }

    Write-Host "[ship-crt-static] target-dir: $targetDir" -ForegroundColor DarkGray
    Write-Host "[ship-crt-static] RUSTFLAGS:  $env:RUSTFLAGS`n" -ForegroundColor DarkGray

    cargo build --release --bin uffs --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    $src = Join-Path $targetDir "release\uffs.exe"
    if (-not (Test-Path $src)) {
        Write-Host "[ship-crt-static] expected binary missing at $src" -ForegroundColor Red
        exit 1
    }
    $dst = Join-Path $env:USERPROFILE "bin\uffs-crt-static.exe"
    Copy-Item -Path $src -Destination $dst -Force

    $crtSize = (Get-Item $dst).Length
    $shipped = Join-Path $env:USERPROFILE "bin\uffs.exe"
    Write-Host ""
    Write-Host "[ship-crt-static] static-CRT  : $($crtSize) bytes ($([math]::Round($crtSize / 1KB, 1)) KB)" -ForegroundColor Green
    if (Test-Path $shipped) {
        $shippedSize = (Get-Item $shipped).Length
        $delta = $crtSize - $shippedSize
        $deltaPct = if ($shippedSize -ne 0) { ($delta / $shippedSize) * 100 } else { 0 }
        Write-Host "[ship-crt-static] shipped     : $($shippedSize) bytes ($([math]::Round($shippedSize / 1KB, 1)) KB)" -ForegroundColor DarkGray
        $sign = if ($delta -ge 0) { "+" } else { "" }
        Write-Host "[ship-crt-static] delta       : $sign$delta bytes ($sign$([math]::Round($deltaPct, 1))%)" -ForegroundColor Yellow
    }
    Write-Host ""
    Write-Host "Verify imports with:" -ForegroundColor Yellow
    Write-Host "  just bench-verify-static"
    Write-Host ""
    Write-Host "A/B bench with:" -ForegroundColor Yellow
    Write-Host "  just bench-cross-static"

# Verify the static-CRT launcher dropped the api-ms-win-crt-*.dll imports.
#
# Mirrors `just bench-verify` but targets `~/bin/uffs-crt-static.exe`
# and asserts that the six CRT forwarder DLLs are NOT in the regular
# import table (they should be statically linked into the binary).
[windows]
bench-verify-static:
    #!powershell
    $ErrorActionPreference = 'Stop'
    $bin = Join-Path $HOME 'bin\uffs-crt-static.exe'
    if (-not (Test-Path $bin)) {
        Write-Host "[bench-verify-static] uffs-crt-static.exe missing at $bin" -ForegroundColor Red
        Write-Host "                      Run 'just ship-crt-static' first." -ForegroundColor DarkGray
        exit 1
    }

    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        Write-Host "[bench-verify-static] vswhere.exe not found; install Visual Studio 2022 Build Tools" -ForegroundColor Red
        exit 1
    }
    $vsPath = & $vswhere -latest -property installationPath
    $dumpbin = Get-ChildItem -Path $vsPath -Filter dumpbin.exe -Recurse -ErrorAction SilentlyContinue |
               Where-Object { $_.FullName -match 'Hostx64\\x64' } |
               Select-Object -First 1 -ExpandProperty FullName
    if (-not $dumpbin) {
        Write-Host "[bench-verify-static] dumpbin.exe not found under $vsPath" -ForegroundColor Red
        exit 1
    }
    Write-Host "[bench-verify-static] dumpbin: $dumpbin" -ForegroundColor DarkGray
    Write-Host "[bench-verify-static] binary : $bin" -ForegroundColor DarkGray

    $raw = & $dumpbin /imports $bin 2>&1 | Out-String
    $regularBlock = ($raw -split 'Section contains the following delay load imports:')[0]

    # These six should disappear when crt-static is in effect.
    $shouldBeAbsent = @(
        'api-ms-win-crt-runtime-l1-1-0.dll',
        'api-ms-win-crt-stdio-l1-1-0.dll',
        'api-ms-win-crt-math-l1-1-0.dll',
        'api-ms-win-crt-locale-l1-1-0.dll',
        'api-ms-win-crt-heap-l1-1-0.dll',
        'vcruntime140.dll'
    )

    $stillPresent = @()
    foreach ($dll in $shouldBeAbsent) {
        if ($regularBlock -match [regex]::Escape($dll)) {
            $stillPresent += $dll
        }
    }

    Write-Host "`nRegular imports (excerpt):" -ForegroundColor Blue
    ($regularBlock -split "`n" | Where-Object { $_ -match '\.dll' } | Select-Object -First 20) |
        ForEach-Object { "  $($_.Trim())" }

    Write-Host ""
    if ($stillPresent.Count -eq 0) {
        Write-Host "[bench-verify-static] OK: all CRT forwarder DLLs are absent (static link confirmed)" -ForegroundColor Green
        exit 0
    } else {
        Write-Host "[bench-verify-static] WARN: these DLLs were expected to be absent but are still imported:" -ForegroundColor Yellow
        $stillPresent | ForEach-Object { Write-Host "  - $_" -ForegroundColor Yellow }
        Write-Host "`n  The static-CRT build may be partial.  Check RUSTFLAGS and rebuild." -ForegroundColor DarkGray
        exit 1
    }

# Cross-tool benchmark using the static-CRT launcher variant.
#
# Mirrors `just bench-cross` but points at `~/bin/uffs-crt-static.exe`.
# Use this together with `just bench-cross` (shipping binary) to A/B
# the static-CRT change.
[windows]
bench-cross-static drives="D" rounds="30" patterns="exact,prefix,ext_rare,ext_dll,substring" tools="uffs,everything":
    #!powershell
    $ErrorActionPreference = 'Stop'
    $uffsBin = Join-Path $HOME 'bin\uffs-crt-static.exe'
    if (-not (Test-Path $uffsBin)) {
        Write-Host "[bench-cross-static] uffs-crt-static.exe missing at $uffsBin" -ForegroundColor Red
        Write-Host "                     Run 'just ship-crt-static' first." -ForegroundColor DarkGray
        exit 1
    }
    Write-Host "[bench-cross-static] drives={{drives}} rounds={{rounds}} patterns={{patterns}} tools={{tools}}" -ForegroundColor Blue
    Write-Host "                     uffs-bin: $uffsBin`n" -ForegroundColor DarkGray
    rust-script scripts\windows\cross-tool-benchmark.rs `
        --skip-cold `
        --drives '{{drives}}' `
        --rounds '{{rounds}}' `
        --patterns '{{patterns}}' `
        --tools '{{tools}}' `
        --uffs-bin $uffsBin
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Compare Rust vs reference binary (integration benchmark).
bench-vs-cpp drive="C":
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🏁 UFFS Benchmark: Rust vs reference binary\033[0m\n"
    printf "\033[0;34m   Drive: {{drive}}:\033[0m\n"
    printf "\033[0;34m   Pattern: *.txt\033[0m\n"
    echo ""
    CPP_UFFS="$HOME/bin/uffs.com"
    if [[ ! -f "$CPP_UFFS" ]]; then printf "\033[1;31m❌ Reference binary not found at $CPP_UFFS\033[0m\n"; printf "\033[0;33m   Please ensure uffs.com is in ~/bin/\033[0m\n"; exit 1; fi
    printf "\033[0;34m📦 Building Rust release...\033[0m\n"
    cargo build --release -p uffs-cli --quiet
    RUST_UFFS="target/release/uffs.exe"; if [[ ! -f "$RUST_UFFS" ]]; then RUST_UFFS="target/release/uffs"; fi
    TEMP_DIR=$(mktemp -d); CPP_OUTPUT="$TEMP_DIR/cpp_output.txt"; RUST_OUTPUT="$TEMP_DIR/rust_output.txt"; trap "rm -rf $TEMP_DIR" EXIT
    printf "\033[0;34m\n⏱️  Benchmarking reference binary (uffs.com)...\033[0m\n"
    CPP_START=$(date +%s.%N); "$CPP_UFFS" "*.txt" --drive {{drive}} > "$CPP_OUTPUT" 2>&1 || true; CPP_END=$(date +%s.%N); CPP_TIME=$(echo "$CPP_END - $CPP_START" | bc); CPP_COUNT=$(wc -l < "$CPP_OUTPUT" | tr -d ' ')
    printf "\033[0;32m   Reference Time: ${CPP_TIME}s, Files: ${CPP_COUNT}\033[0m\n"
    printf "\033[0;34m\n⏱️  Benchmarking Rust (uffs)...\033[0m\n"
    RUST_START=$(date +%s.%N); RUST_LOG=error "$RUST_UFFS" "*.txt" --drive {{drive}} > "$RUST_OUTPUT" 2>&1 || true; RUST_END=$(date +%s.%N); RUST_TIME=$(echo "$RUST_END - $RUST_START" | bc); RUST_COUNT=$(wc -l < "$RUST_OUTPUT" | tr -d ' ')
    printf "\033[0;32m   Rust Time: ${RUST_TIME}s, Files: ${RUST_COUNT}\033[0m\n"
    if (( $(echo "$CPP_TIME > 0" | bc -l) )); then RATIO=$(echo "scale=2; $RUST_TIME / $CPP_TIME" | bc); SPEEDUP=$(echo "scale=2; $CPP_TIME / $RUST_TIME" | bc); else RATIO="N/A"; SPEEDUP="N/A"; fi
    COUNT_DIFF=$((RUST_COUNT - CPP_COUNT))
    printf "\033[0;34m\n═══════════════════════════════════════════════════════════\033[0m\n"
    printf "\033[0;34m📊 BENCHMARK RESULTS\033[0m\n"
    printf "\033[0;34m═══════════════════════════════════════════════════════════\033[0m\n"
    printf "%-20s %15s %15s\n" "Metric" "Reference" "Rust"
    printf "%-20s %15s %15s\n" "────────────────────" "───────────────" "───────────────"
    printf "%-20s %14.2fs %14.2fs\n" "Time" "$CPP_TIME" "$RUST_TIME"
    printf "%-20s %15s %15s\n" "Files Found" "$CPP_COUNT" "$RUST_COUNT"
    printf "\033[0;34m═══════════════════════════════════════════════════════════\033[0m\n"
    if (( $(echo "$SPEEDUP > 1" | bc -l) )); then printf "\033[0;32m✅ Rust is ${SPEEDUP}x faster than the reference binary\033[0m\n"; elif (( $(echo "$SPEEDUP < 1" | bc -l) )); then printf "\033[1;33m⚠️  Rust is ${RATIO}x slower than the reference binary\033[0m\n"; else printf "\033[0;34m   Rust and the reference binary are approximately equal\033[0m\n"; fi
    if [[ $COUNT_DIFF -ne 0 ]]; then printf "\033[1;33m⚠️  File count difference: $COUNT_DIFF (Rust - reference)\033[0m\n"; printf "\033[0;33m   This may indicate path resolution issues\033[0m\n"; else printf "\033[0;32m✅ File counts match!\033[0m\n"; fi
    RESULTS_DIR="benchmarks"; mkdir -p "$RESULTS_DIR"; TIMESTAMP=$(date +%Y%m%d_%H%M%S); RESULTS_FILE="$RESULTS_DIR/bench_${TIMESTAMP}.json"
    echo "{" > "$RESULTS_FILE"; echo "    \"timestamp\": \"$(date -Iseconds)\"," >> "$RESULTS_FILE"; echo "    \"drive\": \"{{drive}}\"," >> "$RESULTS_FILE"; echo "    \"pattern\": \"*.txt\"," >> "$RESULTS_FILE"; echo "    \"cpp\": {" >> "$RESULTS_FILE"; echo "        \"time_seconds\": $CPP_TIME," >> "$RESULTS_FILE"; echo "        \"file_count\": $CPP_COUNT" >> "$RESULTS_FILE"; echo "    }," >> "$RESULTS_FILE"; echo "    \"rust\": {" >> "$RESULTS_FILE"; echo "        \"time_seconds\": $RUST_TIME," >> "$RESULTS_FILE"; echo "        \"file_count\": $RUST_COUNT" >> "$RESULTS_FILE"; echo "    }," >> "$RESULTS_FILE"; echo "    \"speedup\": $SPEEDUP," >> "$RESULTS_FILE"; echo "    \"file_count_diff\": $COUNT_DIFF" >> "$RESULTS_FILE"; echo "}" >> "$RESULTS_FILE"
    printf "\033[0;34m📁 Results saved to: $RESULTS_FILE\033[0m\n"

# Run criterion micro-benchmarks.
bench-micro:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔬 Running Criterion Micro-Benchmarks...\033[0m\n"
    echo ""
    printf "\033[0;34m📊 uffs-core benchmarks (pattern, query, path resolution)...\033[0m\n"
    cargo bench -p uffs-core --bench search_benchmarks
    printf "\033[0;34m\n📊 uffs-mft benchmarks (MFT reading)...\033[0m\n"
    cargo bench -p uffs-mft --bench mft_read
    printf "\033[0;32m\n✅ Micro-benchmarks complete!\033[0m\n"
    printf "\033[0;34m📊 View HTML report: target/criterion/report/index.html\033[0m\n"

# End-to-end search benchmark using cached MFT.
bench-search drive="C":
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔍 End-to-End Search Benchmark\033[0m\n"
    printf "\033[0;34m   Drive: {{drive}}:\033[0m\n"
    echo ""
    cargo build --release -p uffs-cli --quiet
    RUST_UFFS="target/release/uffs.exe"; if [[ ! -f "$RUST_UFFS" ]]; then RUST_UFFS="target/release/uffs"; fi
    PATTERNS=("*.txt" "*.rs" "*.log" "*.dll" "*.exe")
    printf "\033[0;34m%-20s %15s %15s\033[0m\n" "Pattern" "Time (s)" "Files"
    printf "%-20s %15s %15s\n" "────────────────────" "───────────────" "───────────────"
    for PATTERN in "${PATTERNS[@]}"; do START=$(date +%s.%N); COUNT=$(RUST_LOG=error "$RUST_UFFS" "$PATTERN" --drive {{drive}} 2>/dev/null | wc -l | tr -d ' '); END=$(date +%s.%N); TIME=$(echo "$END - $START" | bc); printf "%-20s %14.2fs %15s\n" "$PATTERN" "$TIME" "$COUNT"; done
    printf "\033[0;32m\n✅ Search benchmark complete!\033[0m\n"

# Run all benchmarks and compare to baseline.
bench-all-compare:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📊 Running All Benchmarks with Baseline Comparison...\033[0m\n"
    echo ""
    BASELINE_FILE="benchmarks/baseline.json"
    just bench-vs-cpp C
    if [[ -f "$BASELINE_FILE" ]]; then
        printf "\033[0;34m\n📈 Comparing to baseline...\033[0m\n"
        BASELINE_TIME=$(jq -r '.rust.time_seconds' "$BASELINE_FILE")
        LATEST_FILE=$(ls -t benchmarks/bench_*.json 2>/dev/null | head -1)
        if [[ -n "$LATEST_FILE" ]]; then
            CURRENT_TIME=$(jq -r '.rust.time_seconds' "$LATEST_FILE")
            CHANGE=$(echo "scale=2; (($CURRENT_TIME - $BASELINE_TIME) / $BASELINE_TIME) * 100" | bc)
            if (( $(echo "$CHANGE > 10" | bc -l) )); then printf "\033[1;31m⚠️  REGRESSION: ${CHANGE}%% slower than baseline!\033[0m\n"; elif (( $(echo "$CHANGE < -10" | bc -l) )); then printf "\033[0;32m🎉 IMPROVEMENT: ${CHANGE}%% faster than baseline!\033[0m\n"; else printf "\033[0;34m   Within 10%% of baseline (${CHANGE}%%)\033[0m\n"; fi
        fi
    else
        printf "\033[1;33m⚠️  No baseline found. Run 'just bench-update-baseline' to create one.\033[0m\n"
    fi
    printf "\033[0;32m\n✅ All benchmarks complete!\033[0m\n"

# Update baseline with current benchmark results.
bench-update-baseline:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📝 Updating Benchmark Baseline...\033[0m\n"
    just bench-vs-cpp C
    LATEST_FILE=$(ls -t benchmarks/bench_*.json 2>/dev/null | head -1)
    if [[ -n "$LATEST_FILE" ]]; then
        cp "$LATEST_FILE" "benchmarks/baseline.json"
        printf "\033[0;32m✅ Baseline updated from: $LATEST_FILE\033[0m\n"
        printf "\033[0;34m   Baseline saved to: benchmarks/baseline.json\033[0m\n"
    else
        printf "\033[1;31m❌ No benchmark results found!\033[0m\n"
        exit 1
    fi

# Quick benchmark help.
bench-help:
    #!/usr/bin/env bash
    printf "\033[0;34m🏁 UFFS Benchmark Commands\033[0m\n"
    echo ""
    printf "\033[0;32m  Integration Benchmarks (Rust vs reference):\033[0m\n"
    echo "    just bench-vs-cpp          # Compare on C: drive"
    echo "    just bench-vs-cpp D        # Compare on D: drive"
    echo ""
    printf "\033[0;32m  Micro-Benchmarks (Criterion):\033[0m\n"
    echo "    just bench-micro           # Run all criterion benchmarks"
    echo ""
    printf "\033[0;32m  End-to-End Benchmarks:\033[0m\n"
    echo "    just bench-search          # Search benchmark with multiple patterns"
    echo "    just bench-search D        # Search benchmark on D: drive"
    echo ""
    printf "\033[0;32m  Baseline Management:\033[0m\n"
    echo "    just bench-all-compare     # Run all and compare to baseline"
    echo "    just bench-update-baseline # Update baseline with current results"
    echo ""
    printf "\033[0;34m📊 Results are saved to: benchmarks/*.json\033[0m\n"
    printf "\033[0;34m📈 Criterion reports: target/criterion/report/index.html\033[0m\n"


# ─────────────────────────────────────────────────────────────────────────────
# Robust benchmark-suite orchestrator (`crates/uffs-bench`)
#
# Thin wrappers around the `uffs-bench` binary that drives the reproducible
# Stage 0→5 flow (env fingerprint → competitor preflight → matrix → measurement
# → report → teardown) with the "no crumb left behind" snapshot/restore guard.
# All side effects flow through the `Host` seam, so dry-run and preflight are
# safe on any OS; live measurement stages are Windows-only (later phases).
# See docs/benchmarks/robust-benchmark-flow-execution-plan.md.
# ─────────────────────────────────────────────────────────────────────────────

# Full benchmark suite (Stage 0→5); guided on first run, terse after.
# Pass extra flags through, e.g. `just bench-suite --drives C,D --only-stage 1`.
bench-suite *args:
    cargo run -p uffs-bench -- {{args}}

# Full suite with no prompts (snapshot/restore/verify still run).
bench-suite-auto *args:
    cargo run -p uffs-bench -- --auto {{args}}

# Render every card and the negotiated matrix, mutating nothing.
bench-suite-dry *args:
    cargo run -p uffs-bench -- --dry-run {{args}}

# Stage 0 only: env fingerprint + competitor preflight + matrix (no measurement).
bench-preflight *args:
    cargo run -p uffs-bench -- --only-stage 0 {{args}}

# Resume an existing bundle: load its state.json, skip Done steps, continue.
bench-resume bundle *args:
    cargo run -p uffs-bench -- --bundle {{bundle}} {{args}}

# Download + SHA-256-verify the pinned competitor (`es.exe`) into the bundle.
# Reads scripts/windows/competitors.toml (the single pinned version), fetches
# into <bundle>/tools/, and fails closed on a hash mismatch. Pass `--keep-tools`
# to retain the binary; otherwise it is removed at teardown. Forward flags like
# `--bundle <dir>`, e.g. `just bench-fetch-competitors --bundle out/bench`.
bench-fetch-competitors *args:
    cargo run -p uffs-bench -- {{args}} fetch-competitors

# Replay a bundle's restore-manifest.json after a hard kill (crash recovery).
# Re-applies every serialized undo in LIFO order to return the host to its
# as-found state. Fails closed (non-zero exit) when any undo cannot be replayed.
# On success the manifest is reset to a no-op sentinel. Requires --bundle <dir>.
# Example: just bench-restore --bundle LOG/bench/20260601T120000Z
bench-restore bundle:
    cargo run -p uffs-bench -- --bundle {{bundle}} restore

# Re-capture the host fingerprint and diff it against a bundle's baseline.
# Compares the current host state against fingerprint-before.json from a
# previous run. Fails closed (non-zero exit) when any difference is detected;
# writes fingerprint-after.json for forensics. Requires --bundle <dir>.
# Example: just bench-suite-verify --bundle LOG/bench/20260601T120000Z
bench-suite-verify bundle:
    cargo run -p uffs-bench -- --bundle {{bundle}} verify

