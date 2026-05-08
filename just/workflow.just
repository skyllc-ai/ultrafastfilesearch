# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.

# Phase 1: safe-by-default validation.
phase1-test:
    @printf "\033[0;34m🧪 PHASE 1: Safe-by-Default Validation\033[0m\n"
    @printf "\033[1;33mNo version bump, deploy, commit, or push in this lane.\033[0m\n"
    @echo "========================================================"
    @echo ""
    @printf "\033[0;34mStep 1: Compile validation without applying fixes...\033[0m\n"
    @printf "\033[0;36m  → cargo check --workspace --all-targets --all-features\033[0m\n"
    cargo check --workspace --all-targets --all-features
    @printf "\033[0;34mStep 1b: File size policy (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → bash scripts/ci/check_file_size_policy.sh\033[0m\n"
    bash scripts/ci/check_file_size_policy.sh
    @printf "\033[0;34mStep 2: Running workspace tests (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → cargo nextest run --workspace --all-features --lib --tests\033[0m\n"
    cargo nextest run --workspace --all-features --lib --tests
    @printf "\033[0;32m✅ All workspace tests passed\033[0m\n"
    @printf "\033[0;34mStep 3: Documentation tests validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just test-doc\033[0m\n"
    just test-doc
    @printf "\033[0;34mStep 3b: Rustdoc link validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → RUSTDOCFLAGS='-Dwarnings' cargo doc --workspace --all-features --no-deps\033[0m\n"
    RUSTDOCFLAGS='-Dwarnings' cargo doc --workspace --all-features --no-deps
    @printf "\033[0;34mStep 4: Ultra-strict production code linting (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-prod\033[0m\n"
    just lint-prod
    @printf "\033[0;34mStep 5: Pragmatic test code linting (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-tests\033[0m\n"
    just lint-tests
    @printf "\033[0;34mStep 6: Dependency security validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → cargo deny check\033[0m\n"
    cargo deny check
    @printf "\033[0;34mStep 7: CI lint gating (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-ci\033[0m\n"
    just lint-ci
    @echo ""
    @printf "\033[0;32m✅ PHASE 1 COMPLETE: Validation passed with no release-side effects.\033[0m\n"
    @printf "\033[0;36m📝 Note: Linux-specific code paths (cfg target_os = linux) are validated by CI on push.\033[0m\n"
    @printf "\033[0;34m💡 Next: Run 'just phase2-ship' only when ready to commit/push (release-plz bumps version on main)\033[0m\n"

# Phase 2: commit and push (post-R5: release-plz drives version bumps on main).
phase2-ship:
    @printf "\033[0;34m🚀 PHASE 2: Explicit Ship Lane\033[0m\n"
    @printf "\033[1;33mCommits + pushes the working branch.  release-plz bumps version on main.\033[0m\n"
    @echo "========================================================"
    cargo run -q --release -p uffs-ci-pipeline -- phase2

# Backward compatibility alias for phase2-ship.
phase2-commit: phase2-ship

# Safe-by-default validation workflow (Rust optimized).
# Lints macOS code paths locally; Linux paths validated by CI on push.
go:
    @printf "\033[0;34m🚀 Safe-by-Default Validation Workflow (RUST OPTIMIZED)\033[0m\n"
    @printf "\033[1;33mNo deploy, commit, or push in this default lane.\033[0m\n"
    @printf "\033[0;36m📝 Lints macOS code paths — Linux cfg() paths validated by CI on push.\033[0m\n"
    @echo "========================================================"
    cargo run -q --release -p uffs-ci-pipeline -- go

# Full ship pipeline: Phase 1 validation + Phase 2 deploy (resumable).
# Re-runs automatically skip already-completed steps.
# Lints macOS code paths locally; Linux paths validated by CI on push.
# Pass flags like: just ship -v, just ship --coverage-report
ship *ARGS:
    @printf "\033[0;34m🚢 Full Ship Pipeline (Phase 1 + Phase 2, Resumable)\033[0m\n"
    @printf "\033[1;33mValidates and ships. Re-runs skip completed steps.\033[0m\n"
    @printf "\033[0;36m📝 Lints macOS code paths — Linux cfg() paths validated by CI on push.\033[0m\n"
    @echo "========================================================"
    cargo run -q --release -p uffs-ci-pipeline -- ship {{ ARGS }}

# Full ship pipeline from scratch (ignores previous progress).
# Pass flags like: just ship-fresh -v
ship-fresh *ARGS:
    @printf "\033[0;34m🚢 Full Ship Pipeline (Fresh Start)\033[0m\n"
    @printf "\033[1;33mIgnoring previous progress, starting from scratch.\033[0m\n"
    @echo "========================================================"
    cargo run -q --release -p uffs-ci-pipeline -- ship --fresh {{ ARGS }}

# Safe-by-default validation workflow (justfile fallback).
go-justfile:
    #!/usr/bin/env bash
    printf "\033[0;34m🚀 Safe-by-Default Validation Workflow (JUSTFILE)\033[0m\n"
    printf "\033[1;33mThis fallback validates only; ship actions stay in 'just phase2-ship'.\033[0m\n"
    echo "========================================================"
    echo ""
    START_TIME=$(date +%s)
    printf "\033[0;36m⏱️  Workflow started at: $(date +%H:%M:%S)\033[0m\n"
    echo ""
    printf "\033[0;34m🧪 PHASE 1: Comprehensive Fast-Fail Testing & Validation\033[0m\n"
    just phase1-test
    echo ""
    printf "\033[0;32m✅ PHASE 1 COMPLETE - All validation passed!\033[0m\n"
    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    echo ""
    printf "\033[0;32m🎉 SAFE VALIDATION WORKFLOW FINISHED!\033[0m\n"
    printf "\033[0;32m✅ Validation completed without release-side effects\033[0m\n"
    printf "\033[0;34m💡 Run 'just phase2-ship' only when ready to ship\033[0m\n"
    printf "\033[0;36m⏱️  Total execution time: ${DURATION}s\033[0m\n"

# Comprehensive validation (Rust optimized).
check-all:
    @printf "\033[0;34m📋 Comprehensive Validation (RUST OPTIMIZED)\033[0m\n"
    @printf "\033[1;33mUsing high-performance Rust script with parallel execution...\033[0m\n"
    @echo "========================================================"
    cargo run -q --release -p uffs-ci-pipeline -- check-all

# Generate coverage report (Rust optimized).
coverage-report:
    @printf "\033[0;34m📊 Coverage Report Generation (RUST OPTIMIZED)\033[0m\n"
    @printf "\033[1;33mSmart detection: generates from existing data or runs tests if needed...\033[0m\n"
    @echo "========================================================"
    cargo run -q --release -p uffs-ci-pipeline -- coverage-report

# Watch the latest release workflow run.
release-watch:
    @printf "\033[0;34m👀 Watching latest release workflow...\033[0m\n"
    @gh run watch $(gh run list --workflow=release.yml --limit=1 --json databaseId --jq '.[0].databaseId')

# Open release workflow in browser.
release-status:
    @printf "\033[0;34m🔗 Opening release workflow in browser...\033[0m\n"
    @gh run list --workflow=release.yml --limit=1 --web

# Check current workflow status.
workflow-status:
    @printf "\033[0;34m📊 Checking workflow status...\033[0m\n"
    @cargo run -q --release -p uffs-ci-pipeline -- workflow-status

# Reset workflow state.
workflow-reset:
    @printf "\033[0;33m⚠️  Resetting workflow state...\033[0m\n"
    @cargo run -q --release -p uffs-ci-pipeline -- workflow-reset

# Resume incomplete workflow.
workflow-resume:
    @printf "\033[0;34m🔄 Resuming workflow...\033[0m\n"
    @cargo run -q --release -p uffs-ci-pipeline -- workflow-resume

