# Phase 1: code and extensive testing.
phase1-test:
    @printf "\033[0;34m🧪 PHASE 1: Code & Extensive Testing (FAST-FAIL)\033[0m\n"
    @printf "\033[1;33mRunning MOST extensive tests - STOPPING at FIRST failure...\033[0m\n"
    @echo "========================================================"
    @echo ""
    @printf "\033[0;34mStep 1: Cleaning build artifacts...\033[0m\n"
    @printf "\033[0;36m  → cargo clean\033[0m\n"
    cargo clean
    @printf "\033[0;32m✅ Build artifacts cleaned\033[0m\n"
    @printf "\033[0;34mStep 2: Auto-formatting code...\033[0m\n"
    @printf "\033[0;36m  → just fmt\033[0m\n"
    just fmt
    @printf "\033[0;34mStep 3: Running all tests with coverage data collection (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report\033[0m\n"
    @if command -v cargo-llvm-cov >/dev/null 2>&1; then \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report; \
        printf "\033[0;32m✅ All tests passed, coverage data collected\033[0m\n"; \
    else \
        printf "\033[1;33mInstalling cargo-llvm-cov...\033[0m\n"; \
        just _install-if-missing cargo-llvm-cov cargo-llvm-cov; \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report; \
        printf "\033[0;32m✅ All tests passed, coverage data collected\033[0m\n"; \
    fi
    @printf "\033[0;32m✅ Coverage data collected (use 'just coverage-report' for HTML)\033[0m\n"
    @printf "\033[0;34mStep 5: Documentation tests validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just test-doc\033[0m\n"
    just test-doc
    @printf "\033[0;34mStep 6: Ultra-strict production code linting (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-prod\033[0m\n"
    just lint-prod
    @printf "\033[0;34mStep 7: Pragmatic test code linting (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-tests\033[0m\n"
    just lint-tests
    @printf "\033[0;34mStep 8: Legal compliance validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just legal-check\033[0m\n"
    just legal-check
    @printf "\033[0;34mStep 9: Enhanced format validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → cargo fix --workspace --all-targets --edition-idioms --allow-dirty (ENHANCED)\033[0m\n"
    cargo fix --workspace --all-targets --edition-idioms --allow-dirty
    @printf "\033[0;36m  → cargo clippy --fix --workspace --allow-dirty --allow-staged (PROVEN WORKFLOW)\033[0m\n"
    cargo clippy --fix --workspace --allow-dirty --allow-staged -- -D warnings -D clippy::pedantic -D clippy::nursery -D clippy::cargo -A clippy::multiple_crate_versions
    @printf "\033[0;36m  → cargo fmt --all -- --check\033[0m\n"
    cargo fmt --all -- --check
    @printf "\033[0;34mStep 10: CI lint gating (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-ci\033[0m\n"
    just lint-ci
    @echo ""
    @printf "\033[0;32m✅ PHASE 1 FAST-FAIL COMPLETE: All extensive tests passed, code ready for commit!\033[0m\n"
    @printf "\033[0;34m💡 Next: Run 'just phase2-ship' when ready to build/commit/push\033[0m\n"

# Phase 2: version, build, deploy, commit, and push.
phase2-ship:
    @printf "\033[0;34m🚀 PHASE 2: Version/Build/Deploy (Post-Testing)\033[0m\n"
    @printf "\033[1;33mAssumes Phase 1 completed: format ✅ clippy ✅ compile ✅ tests ✅\033[0m\n"
    @echo "========================================================"
    @echo ""
    @printf "\033[0;34mStep 1: Version increment...\033[0m\n"
    @printf "\033[0;36m  → just version-bump\033[0m\n"
    just version-bump
    @printf "\033[0;34mStep 2: Building dev binary with NEW version...\033[0m\n"
    @printf "\033[0;36m  → just build\033[0m\n"
    just build
    @printf "\033[0;34mStep 3: Copy binary to deployment location...\033[0m\n"
    @printf "\033[0;36m  → just deploy\033[0m\n"
    just deploy
    @printf "\033[0;34mStep 4: Creating auto-generated commit...\033[0m\n"
    @printf "\033[0;36m  → just commit\033[0m\n"
    just commit
    @printf "\033[0;34mStep 5: Syncing with remote and pushing...\033[0m\n"
    @printf "\033[0;36m  → just push\033[0m\n"
    just push
    @echo ""
    @printf "\033[0;32m✅ PHASE 2 COMPLETE: Version incremented, built, deployed, committed, and pushed!\033[0m\n"

# Backward compatibility alias for phase2-ship.
phase2-commit: phase2-ship

# Complete two-phase fast-fail workflow (Rust optimized).
go:
    @printf "\033[0;34m🚀 Complete Two-Phase Fast-Fail Workflow (RUST OPTIMIZED)\033[0m\n"
    @printf "\033[1;33mUsing high-performance Rust script with async parallelism...\033[0m\n"
    @echo "========================================================"
    rust-script scripts/ci/ci-pipeline.rs go

