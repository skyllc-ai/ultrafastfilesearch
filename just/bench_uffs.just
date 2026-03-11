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

