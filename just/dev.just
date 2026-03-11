# Watch mode development.
dev:
    @printf "\033[0;34m🔄 Starting watch mode...\033[0m\n"
    @if command -v cargo-watch >/dev/null 2>&1; then \
        cargo watch -x "nextest run --workspace --jobs 16" -x "clippy --workspace --all-targets --all-features --no-deps -- {{ test_flags }}"; \
    else \
        printf "\033[1;33mInstalling cargo-watch...\033[0m\n"; \
        just _install-if-missing cargo-watch cargo-watch; \
        cargo watch -x "nextest run --workspace --jobs 16" -x "clippy --workspace --all-targets --all-features --no-deps -- {{ test_flags }}"; \
    fi

# Clean build artifacts.
clean:
    @printf "\033[0;34m🧹 Cleaning build artifacts...\033[0m\n"
    cargo clean

# Emergency killswitch.
kill:
    @printf "\033[0;31m🚨 EMERGENCY: Killing all cargo processes...\033[0m\n"
    @pkill -f "cargo" 2>/dev/null || true
    @pkill -f "rustc" 2>/dev/null || true
    @printf "\033[0;32m✅ All cargo processes terminated\033[0m\n"

# Install cross-compilation targets for GitHub Actions compatibility.
setup-cross:
    @printf "\033[0;34m Installing cross-compilation targets...\033[0m\n"
    rustup target add x86_64-unknown-linux-gnu
    rustup target add aarch64-unknown-linux-gnu
    rustup target add x86_64-pc-windows-msvc
    @printf "\033[0;32m✅ Cross-compilation targets installed\033[0m\n"

# Cross-compile check for GitHub Actions compatibility.
check-cross:
    @printf "\033[0;34m Cross-compilation validation using CI pipeline...\033[0m\n"
    rust-script scripts/ci/ci-pipeline.rs cross-check

# Install core development tools.
devenv-tools:
    @printf "\033[0;34m🛠️ Installing core development tools...\033[0m\n"
    cargo install cargo-nextest
    cargo install cargo-llvm-cov
    cargo install cargo-audit
    cargo install cargo-deny
    cargo install cargo-semver-checks
    @printf "\033[0;32m✅ Core tools installed\033[0m\n"

# Run all tests (alias for test).
devenv-test-all: test
    @printf "\033[0;32m✅ All tests complete\033[0m\n"

# Install all modern development tools.
install-tools:
    @printf "\033[0;34m🔧 Installing modern development tools...\033[0m\n"
    @printf "\033[1;33mUsing cargo-binstall for fast installation...\033[0m\n"
    cargo binstall cargo-nextest cargo-llvm-cov cargo-audit cargo-watch just --no-confirm
    @printf "\033[0;32m✅ All modern tools installed successfully!\033[0m\n"

# Update all development tools to latest versions.
update-tools:
    @printf "\033[0;34m🔄 Updating development tools...\033[0m\n"
    @if command -v cargo-install-update >/dev/null 2>&1; then \
        cargo install-update -a; \
    else \
        printf "\033[1;33mInstalling cargo-update for efficient updates...\033[0m\n"; \
        just _install-if-missing cargo-update cargo-update; \
        cargo install-update -a; \
    fi
    @printf "\033[0;32m✅ All tools updated to latest versions!\033[0m\n"

