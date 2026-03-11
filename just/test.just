# Verify Rust toolchain consistency.
verify-toolchain:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔧 Verifying Rust toolchain consistency...\033[0m\n"
    cargo +nightly --version
    cargo +nightly metadata --format-version 1 --no-deps >/dev/null
    printf "\033[0;32m✅ Nightly toolchain metadata resolved successfully\033[0m\n"

# Format code.
fmt:
    @printf "\033[0;34m📝 Formatting code...\033[0m\n"
    cargo fmt --all

# Check code formatting without applying fixes.
fmt-check:
    @printf "\033[0;34m📝 Checking code formatting...\033[0m\n"
    cargo fmt --all -- --check

# Run all tests with nextest.
test:
    @printf "\033[0;34m🧪 Running all tests with nextest (FAST-FAIL)...\033[0m\n"
    cargo llvm-cov nextest run --workspace --all-features --all-targets --jobs 16 --no-report

# Run documentation tests separately.
test-doc:
    @printf "\033[0;34m📚 Running documentation tests (FAST-FAIL)...\033[0m\n"
    RUSTDOCFLAGS='-Dwarnings -Dmissing_docs' cargo llvm-cov test --doc --workspace --all-features --no-report

# Run slow or ignored tests.
test-slow:
    @printf "\033[0;34m🐌 Running slow/ignored tests (comprehensive validation)...\033[0m\n"
    @printf "\033[1;33m⚠️  These tests may take several minutes - includes full workspace validation\033[0m\n"
    cargo nextest run --workspace --all-features --all-targets --profile slow --run-ignored ignored-only

# Generate coverage report.
coverage:
    @printf "\033[0;34m📊 Generating coverage report...\033[0m\n"
    cargo clean
    @if command -v cargo-llvm-cov >/dev/null 2>&1; then \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --html; \
        printf "\033[0;32m📁 Coverage report: target/llvm-cov/html/index.html\033[0m\n"; \
    else \
        printf "\033[1;33mInstalling cargo-llvm-cov...\033[0m\n"; \
        just _install-if-missing cargo-llvm-cov cargo-llvm-cov; \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --html; \
        printf "\033[0;32m📁 Coverage report: target/llvm-cov/html/index.html\033[0m\n"; \
    fi

# Ultra-strict production linting.
lint-prod:
    @printf "\033[0;34m🔍 Ultra-strict production linting (FAST-FAIL)...\033[0m\n"
    cargo clippy --workspace --lib --bins --all-features --no-deps -- {{ prod_flags }}

# Pragmatic test linting.
lint-tests:
    @printf "\033[0;34m🧪 Pragmatic test linting (FAST-FAIL)...\033[0m\n"
    cargo clippy --workspace --tests --all-features --no-deps -- {{ test_flags }}

# Gate residual lints in CI.
lint-ci:
    @printf "\033[0;34m🚨 CI lint gating (PROVEN WORKFLOW)...\033[0m\n"
    cargo clippy --workspace --all-features -- -D warnings

# Enforce oversized Rust file policy with explicit exceptions.
file-size-policy:
    @printf "\033[0;34m📏 Checking oversized Rust file policy...\033[0m\n"
    bash scripts/ci/check_file_size_policy.sh

# Comprehensive code validation.
check:
    @printf "\033[0;34m🔍 Comprehensive validation (CI/CD grade)...\033[0m\n"
    just file-size-policy
    cargo check --workspace --all-targets --all-features
    cargo fmt --all -- --check

# Quick code validation.
check-quick:
    @printf "\033[0;34m⚡ Quick validation (development speed)...\033[0m\n"
    cargo check --workspace

# Update Polars git lock to latest main.
update-polars:
    @printf "\033[0;34m📦 Updating Polars (git, branch=main) lockfile...\033[0m\n"
    cargo update -w
    @printf "\033[0;32m✅ Polars lockfile updated\033[0m\n"

