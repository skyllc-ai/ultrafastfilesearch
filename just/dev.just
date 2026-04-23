# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.

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
    cargo run -q --release -p uffs-ci-pipeline -- cross-check

# Type-check all #[cfg(windows)] code paths from macOS using cargo-xwin.
#
# Catches missing imports, wrong types, and dead code in Windows-only blocks
# that `cargo check` on macOS silently skips — including `#[cfg(windows)]`
# tests, benches, and `std::os::windows::*` usage.  CI today only runs on
# Linux (`ubuntu-22.04`), so this recipe is the ONLY pre-PR gate that
# exercises Windows-specific code.
#
# Uses `--all-targets --all-features` so tests and benches are checked
# alongside libs and bins (this is how the Windows-only bench / test bugs
# that motivated this gate were surfaced).  Runs in ~2–5 s warm once
# xwin has cached the MSVC SDK under `~/Library/Caches/xwin/`.
check-windows:
    @printf "\033[0;34m🪟 Windows cross-check (cargo xwin check, all-targets all-features)...\033[0m\n"
    cargo xwin check --workspace --all-targets --all-features --target x86_64-pc-windows-msvc
    @printf "\033[0;32m✅ Windows cross-check passed\033[0m\n"

# Cross-platform compile check across all three supported targets.
#
# Runs native (host) + Linux (via Docker, through `lint-ci-linux`) +
# Windows (via xwin, through `check-windows`).  Intended as a manual
# "pre-PR sweep"; the pre-push hook runs the host + Windows subset only
# (Linux-in-Docker is too slow for a commit-time gate — CI covers it).
#
# Skips silently if a prerequisite tool (cargo-xwin, docker) is missing,
# but reports which targets were exercised so you know the coverage.
check-all-targets:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🌐 Cross-platform compile check (macOS + Linux + Windows)...\033[0m\n"
    printf "\033[0;36m   ── macOS (native host) ──\033[0m\n"
    cargo check --workspace --all-targets --all-features
    printf "\033[0;32m   ✅ native host\033[0m\n\n"
    printf "\033[0;36m   ── Windows x86_64-pc-windows-msvc (xwin) ──\033[0m\n"
    if command -v cargo-xwin >/dev/null 2>&1; then
        cargo xwin check --workspace --all-targets --all-features --target x86_64-pc-windows-msvc
        printf "\033[0;32m   ✅ Windows (xwin)\033[0m\n\n"
    else
        printf "\033[1;33m   ⚠  cargo-xwin not installed — skipping; install with: \033[0;36mcargo install cargo-xwin\033[0m\n\n"
    fi
    printf "\033[0;36m   ── Linux x86_64-unknown-linux-gnu (Docker) ──\033[0m\n"
    if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
        just lint-ci-linux
    else
        printf "\033[1;33m   ⚠  Docker not available — skipping; start Docker Desktop / colima / OrbStack\033[0m\n"
    fi
    printf "\033[0;32m✅ Cross-platform check complete\033[0m\n"

# Update everything: Rust toolchain, cargo tools, system packages.
# Uses cargo-binstall (pre-built binaries) when available for ~100× faster installs.
# Run before `just go` or `just ship` to stay cutting-edge.
[unix]
update-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[1;34m🔄 Updating all development tools\033[0m\n"
    echo "========================================================"

    # ── 1. Rust toolchain ──────────────────────────────────────
    printf "\n\033[0;34m🦀 Step 1/3: Rust toolchain\033[0m\n"
    if command -v brew >/dev/null 2>&1 && brew list rustup 2>/dev/null | grep -q rustup; then
        printf "  → rustup managed by Homebrew\n"
        brew upgrade rustup 2>/dev/null || true
    else
        rustup self update 2>/dev/null || true
    fi
    rustup update
    just toolchain-sync
    printf "\033[0;32m  ✅ Rust toolchain up to date\033[0m\n"

    # ── 2. Cargo tools ─────────────────────────────────────────
    printf "\n\033[0;34m📦 Step 2/3: Cargo tools\033[0m\n"
    TOOLS=(
        cargo-nextest
        cargo-llvm-cov
        cargo-deny
        cargo-xwin
        sccache
        cargo-machete
        cargo-outdated
        cargo-semver-checks
        cargo-mutants
        cargo-insta
        rust-script
        flamegraph
        samply
    )
    TOOL_LIST="${TOOLS[*]}"
    if command -v cargo-binstall >/dev/null 2>&1; then
        printf "  → Using cargo-binstall (fast, pre-built binaries)\n"
        cargo binstall -y --no-confirm $TOOL_LIST
    else
        printf "  → Using cargo install (compiling from source — slower)\n"
        printf "  💡 Install cargo-binstall for ~100× faster updates:\n"
        printf "     curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash\n"
        cargo install $TOOL_LIST
    fi
    printf "\033[0;32m  ✅ Cargo tools up to date\033[0m\n"

    # ── 3. System packages ─────────────────────────────────────
    printf "\n\033[0;34m🍺 Step 3/3: System packages\033[0m\n"
    if command -v brew >/dev/null 2>&1; then
        brew upgrade just 2>/dev/null || printf "  → just already up to date\n"
    else
        printf "  → No Homebrew detected, skipping system packages\n"
    fi
    printf "\033[0;32m  ✅ System packages up to date\033[0m\n"

    echo ""
    printf "\033[1;32m🎉 All tools updated!\033[0m\n"
    printf "  rustc:    $(rustc --version)\n"
    printf "  nextest:  $(cargo nextest --version 2>/dev/null | head -1)\n"
    printf "  llvm-cov: $(cargo llvm-cov --version 2>/dev/null)\n"

[windows]
update-tools:
    #!powershell
    Write-Host "`n🔄 Updating all development tools" -ForegroundColor Blue
    Write-Host "========================================================"

    # ── 1. Rust toolchain ──────────────────────────────────────
    Write-Host "`n🦀 Step 1/3: Rust toolchain" -ForegroundColor Blue
    rustup self update 2>$null
    rustup update
    just toolchain-sync
    Write-Host "  ✅ Rust toolchain up to date" -ForegroundColor Green

    # ── 2. Cargo tools ─────────────────────────────────────────
    Write-Host "`n📦 Step 2/3: Cargo tools" -ForegroundColor Blue
    $tools = @(
        "cargo-nextest"
        "cargo-llvm-cov"
        "cargo-deny"
        "sccache"
        "cargo-machete"
        "cargo-outdated"
        "cargo-semver-checks"
        "cargo-mutants"
        "cargo-insta"
        "rust-script"
    )
    $toolList = $tools -join " "
    if (Get-Command cargo-binstall -ErrorAction SilentlyContinue) {
        Write-Host "  → Using cargo-binstall (fast, pre-built binaries)"
        Invoke-Expression "cargo binstall -y --no-confirm $toolList"
    } else {
        Write-Host "  → Using cargo install (compiling from source — slower)"
        Write-Host "  💡 Install cargo-binstall for faster updates:" -ForegroundColor Yellow
        Invoke-Expression "cargo install $toolList"
    }
    Write-Host "  ✅ Cargo tools up to date" -ForegroundColor Green

    # ── 3. System packages ─────────────────────────────────────
    Write-Host "`n📦 Step 3/3: System packages" -ForegroundColor Blue
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        winget upgrade Casey.Just 2>$null
    } elseif (Get-Command scoop -ErrorAction SilentlyContinue) {
        scoop update just 2>$null
    } else {
        Write-Host "  → No winget/scoop detected, skipping system packages"
    }
    Write-Host "  ✅ System packages up to date" -ForegroundColor Green

    Write-Host "`n🎉 All tools updated!" -ForegroundColor Green
    Write-Host "  rustc:    $(rustc --version)"
    Write-Host "  nextest:  $(cargo nextest --version 2>$null | Select-Object -First 1)"



# Quick deploy to Windows: version bump → xcompile → commit → push.
# Skips tests, lint, coverage, and macOS native builds. For debug iteration only.
# Alias: just q
[unix]
q: quick-deploy

[unix]
quick-deploy:
    #!/usr/bin/env bash
    set -euo pipefail
    TOTAL_START=$(date +%s)
    printf "\033[1;34m⚡ Quick Deploy to Windows (skip CI)\033[0m\n"
    printf "\033[1;33m⚠️  No tests, no linting — debug iteration only!\033[0m\n"
    echo "========================================================"

    # 1. Version bump
    printf "\n\033[0;34m📈 Step 1/4: Version bump...\033[0m\n"
    if [ -f "./build/update_all_versions.rs" ]; then
        ./build/update_all_versions.rs patch
    else
        printf "\033[1;31m❌ Version script not found\033[0m\n"
        exit 1
    fi
    NEW_VERSION=$(awk '/^\[workspace\.package\]/{flag=1; next} flag && /^version/{print $3; exit}' Cargo.toml | tr -d '"')
    printf "\033[0;32m  → v%s\033[0m\n" "$NEW_VERSION"

    # Sync workflow state so build-cross-all.rs uses the correct version
    STATE_FILE="build/.uffs-workflow-state.json"
    if [ -f "$STATE_FILE" ]; then
        # Update current_version in-place using portable sed
        sed -i.bak "s/\"current_version\": *\"[^\"]*\"/\"current_version\": \"${NEW_VERSION}\"/" "$STATE_FILE"
        rm -f "${STATE_FILE}.bak"
        printf "\033[0;36m  → Synced workflow state to v%s\033[0m\n" "$NEW_VERSION"
    fi

    # 2. Cross-compile for Windows (build-cross-all.rs uploads to GitHub Release)
    printf "\n\033[0;34m🔨 Step 2/4: Cross-compile for Windows...\033[0m\n"
    START=$(date +%s)
    rust-script scripts/ci/build-cross-all.rs
    END=$(date +%s)
    printf "\033[0;32m  → Build completed in %ds\033[0m\n" $((END - START))

    # 3. Git commit
    printf "\n\033[0;34m📝 Step 3/4: Git commit...\033[0m\n"
    git add .
    git -c commit.gpgsign=false commit -m "debug: quick-deploy v${NEW_VERSION} [skip ci]"
    printf "\033[0;32m  → Committed\033[0m\n"

    # 4. Git push
    printf "\n\033[0;34m🚀 Step 4/4: Git push...\033[0m\n"
    git pull origin main --rebase
    git push origin main
    printf "\033[0;32m  → Pushed\033[0m\n"

    TOTAL_END=$(date +%s)
    ELAPSED=$((TOTAL_END - TOTAL_START))
    MINS=$((ELAPSED / 60))
    SECS=$((ELAPSED % 60))
    printf "\n\033[1;32m⚡ Quick deploy complete: v%s in %dm %ds\033[0m\n" "$NEW_VERSION" "$MINS" "$SECS"
    printf "\033[0;34m💡 On Windows: git pull && just use\033[0m\n"