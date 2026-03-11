# Debug version of benchmark-flows with visible output.
benchmark-flows-debug:
    #!/usr/bin/env bash
    printf "\033[0;34m⚡ Workflow Orchestration Comparison (DEBUG MODE)...\033[0m\n"
    printf "\033[1;33m🎯 GOAL: Compare Rust script vs JUST file workflow approaches\033[0m\n"
    printf "\033[0;34m📋 SAME LOGIC: Both run identical steps with same commands\033[0m\n"
    printf "\033[0;34m📋 ONLY DIFFERENCE: Orchestration approach (async/parallel vs sequential)\033[0m\n"
    printf "\033[0;34m\n🔄 Test 1: Rust Script Workflow (just go)\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"
    cargo clean
    TEST1_START=$(date +%s)
    printf "\033[0;34m  → Running Rust script workflow with async/parallel optimization (VERBOSE)...\033[0m\n"
    if rust-script scripts/ci/ci-pipeline.rs go -v; then TEST1_END=$(date +%s); TEST1_TOTAL=$((TEST1_END - TEST1_START)); printf "\033[0;32m  ✅ Rust script completed successfully in ${TEST1_TOTAL}s\033[0m\n"; else TEST1_END=$(date +%s); TEST1_TOTAL=-1; printf "\033[0;31m  ❌ Rust script failed\033[0m\n"; fi
    printf "\033[0;34m\n🔄 Test 2: JUST File Workflow (just go-justfile)\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"
    cargo clean
    TEST2_START=$(date +%s)
    printf "\033[0;34m  → Running JUST file workflow with sequential execution...\033[0m\n"
    if just go-justfile; then TEST2_END=$(date +%s); TEST2_TOTAL=$((TEST2_END - TEST2_START)); printf "\033[0;32m  ✅ JUST file workflow completed successfully in ${TEST2_TOTAL}s\033[0m\n"; else TEST2_END=$(date +%s); TEST2_TOTAL=-1; printf "\033[0;31m  ❌ JUST file workflow failed\033[0m\n"; fi
    printf "\033[0;32m\n⚡ WORKFLOW ORCHESTRATION RESULTS:\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"
    if [ "$TEST1_TOTAL" -eq -1 ] && [ "$TEST2_TOTAL" -eq -1 ]; then printf "\033[0;31m❌ Both workflows failed - check output above for errors\033[0m\n"; elif [ "$TEST1_TOTAL" -eq -1 ]; then printf "\033[0;31m📊 Rust Script (async/parallel):  FAILED\033[0m\n"; printf "\033[0;34m📊 JUST File (sequential):        ${TEST2_TOTAL}s\033[0m\n"; printf "\033[0;32m🏆 WINNER: JUST File (just go-justfile)\033[0m\n"; elif [ "$TEST2_TOTAL" -eq -1 ]; then printf "\033[0;34m📊 Rust Script (async/parallel):  ${TEST1_TOTAL}s\033[0m\n"; printf "\033[0;31m📊 JUST File (sequential):        FAILED\033[0m\n"; printf "\033[0;32m🏆 WINNER: Rust Script (just go)\033[0m\n"; else printf "\033[0;34m📊 Rust Script (async/parallel):  ${TEST1_TOTAL}s\033[0m\n"; printf "\033[0;34m📊 JUST File (sequential):        ${TEST2_TOTAL}s\033[0m\n"; if [ "$TEST1_TOTAL" -lt "$TEST2_TOTAL" ]; then SAVINGS=$((TEST2_TOTAL - TEST1_TOTAL)); printf "\033[0;32m🏆 WINNER: Rust Script (just go)\033[0m\n"; printf "\033[0;32m💰 SAVINGS: ${SAVINGS}s faster\033[0m\n"; elif [ "$TEST2_TOTAL" -lt "$TEST1_TOTAL" ]; then SAVINGS=$((TEST1_TOTAL - TEST2_TOTAL)); printf "\033[0;32m🏆 WINNER: JUST File (just go-justfile)\033[0m\n"; printf "\033[0;32m💰 SAVINGS: ${SAVINGS}s faster\033[0m\n"; else printf "\033[0;33m🤝 TIE: Both approaches have identical performance\033[0m\n"; fi; fi

# Benchmark Rust script workflow approach.
benchmark-rust-script:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Benchmarking Rust script workflow approach...\033[0m\n"
    printf "\033[1;33m🚀 RUST SCRIPT: Async/parallel optimized workflow\033[0m\n"
    printf "\033[0;34m  → Cleaning for accurate measurement...\033[0m\n"
    cargo clean
    START_TIME=$(date +%s)
    printf "\033[0;34m  → Running Rust script workflow (just go)...\033[0m\n"
    just go
    END_TIME=$(date +%s)
    TOTAL_TIME=$((END_TIME - START_TIME))
    printf "\033[0;32m✅ Rust script workflow benchmark complete!\033[0m\n"
    printf "\033[0;34m📊 Performance summary:\033[0m\n"
    printf "\033[0;34m  → Total execution time: ${TOTAL_TIME}s\033[0m\n"
    printf "\033[0;34m  → Approach: Async/parallel with detailed step timing\033[0m\n"

# Benchmark JUST file workflow approach.
benchmark-justfile:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Benchmarking JUST file workflow approach...\033[0m\n"
    printf "\033[1;33m📋 JUST FILE: Traditional sequential workflow\033[0m\n"
    printf "\033[0;34m  → Cleaning for accurate measurement...\033[0m\n"
    cargo clean
    START_TIME=$(date +%s)
    printf "\033[0;34m  → Running JUST file workflow (just go-justfile)...\033[0m\n"
    just go-justfile
    END_TIME=$(date +%s)
    TOTAL_TIME=$((END_TIME - START_TIME))
    printf "\033[0;32m✅ JUST file workflow benchmark complete!\033[0m\n"
    printf "\033[0;34m📊 Performance summary:\033[0m\n"
    printf "\033[0;34m  → Total execution time: ${TOTAL_TIME}s\033[0m\n"
    printf "\033[0;34m  → Approach: Sequential with overall timing only\033[0m\n"

# Benchmark LLVM compilation reuse strategy.
benchmark-llvm-reuse:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Benchmarking LLVM compilation reuse strategy...\033[0m\n"
    printf "\033[1;33m🎯 FOCUS: Testing LLVM artifact reuse efficiency\033[0m\n"
    START_TIME=$(date +%s)
    printf "\033[0;34m  → Cleaning for accurate measurement...\033[0m\n"
    cargo clean
    printf "\033[0;34m  → Step 1: Coverage analysis (full LLVM compilation)...\033[0m\n"
    STEP1_START=$(date +%s)
    cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report
    STEP1_END=$(date +%s)
    STEP1_TIME=$((STEP1_END - STEP1_START))
    printf "\033[0;34m  → Step 2: Coverage data ready (HTML skipped for speed)...\033[0m\n"
    STEP2_START=$(date +%s)
    printf "\033[0;32m✅ Coverage data available (use 'just coverage-report' for HTML)\033[0m\n"
    STEP2_END=$(date +%s)
    STEP2_TIME=$((STEP2_END - STEP2_START))
    printf "\033[0;34m  → Step 3: Additional test run (artifact reuse)...\033[0m\n"
    STEP3_START=$(date +%s)
    cargo llvm-cov nextest run --workspace --all-features --all-targets --jobs 16 --no-report
    STEP3_END=$(date +%s)
    STEP3_TIME=$((STEP3_END - STEP3_START))
    END_TIME=$(date +%s)
    TOTAL_TIME=$((END_TIME - START_TIME))
    printf "\033[0;32m✅ LLVM reuse benchmark complete!\033[0m\n"
    printf "\033[0;34m📊 Performance breakdown:\033[0m\n"
    printf "\033[0;34m  → Step 1 (Full LLVM compilation): ${STEP1_TIME}s\033[0m\n"
    printf "\033[0;34m  → Step 2 (LLVM report reuse): ${STEP2_TIME}s\033[0m\n"
    printf "\033[0;34m  → Step 3 (Artifact reuse): ${STEP3_TIME}s\033[0m\n"
    printf "\033[0;34m  → Total execution time: ${TOTAL_TIME}s\033[0m\n"

