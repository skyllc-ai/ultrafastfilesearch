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

# Find and kill orphan `uffs` processes left behind by aborted CLI runs,
# crashed test harnesses, or Ctrl-C'd validation scripts.  The
# legitimate long-running daemon is identified via `uffs daemon status`
# and explicitly preserved.  Pass `--dry-run` to list orphans without
# killing them, or `--bin <path>` to use a specific uffs binary for
# the daemon-PID lookup.
#
# Pulled out of the validation suites
# (`scripts/windows/{api,cli,mcp}-validation.rs`) so those scripts only
# observe daemon state — they never mutate the host process table.
orphan *ARGS:
    @rust-script scripts/dev/orphan-cleanup.rs {{ ARGS }}

# Daemon-readiness end-to-end verification.
#
# Exercises every meaningful daemon lifecycle (clean start/stop, kill
# recovery, double-start, restart, auto-start), the Phase 5/6/7 stats
# accumulation, COLD→WARM→HOT timing capture, AND the Phase 8 / 9
# operator-driven memory-tiering surface (status_drives, hibernate,
# preload --pin-minutes, forget --force, full round-trip cycle, and
# the Phase 9 promotions_total counter contract).  Renders a per-RPC
# timing summary at the end so an operator can see where time is
# spent across the operator commands.
#
# Pass `~/uffs_data` (or any data dir / MFT file) on macOS / Linux;
# omit on Windows for live NTFS auto-discovery.  Use `--forget-drive Z`
# (default) to avoid actually deleting any cache files; override with
# a small test drive (e.g. `M`) to exercise the destructive path.
#
# Examples:
#   just readiness ~/uffs_data
#   just readiness ~/uffs_data --skip-phase8
#   just readiness ~/uffs_data --forget-drive Z --pattern '*.txt'
readiness *ARGS:
    @rust-script scripts/dev/daemon-readiness.rs {{ ARGS }}

# Tier-load A/B benchmark — Cold vs Parked worst-case.
#
# Empirically validates that worst-case re-promote wall-clock is
# **identical** between the `Cold` and `Parked` source tiers when
# every drive's bloom hits the query (the broadest possible filter).
# The bloom optimisation in `ensure_warm_for_dispatch` only helps
# when some blooms *miss* — under a broad filter every drive must
# body-load regardless of source tier, and the per-drive decrypt
# cost is shared by the same `load_or_join_in_flight` primitive on
# both code paths.
#
# Runs N iterations (default 3) of each phase with a hard 15 %
# parity assertion at the end.  Total wall-clock ~3 min for N=3
# (driven by the 35 s warm→parked TTL wait per Phase B iteration).
#
# Pass `~/uffs_data` (or any data dir / MFT file) on macOS / Linux;
# omit on Windows for live NTFS auto-discovery.  The script self-
# manages the daemon TTL env vars (`UFFS_WARM_TO_PARKED_IDLE_SECS=5`,
# `UFFS_PARKED_TO_COLD_IDLE_SECS=900`) so callers don't have to.
#
# Examples:
#   just tier-compare ~/uffs_data
#   just tier-compare ~/uffs_data --pattern '*.dll' --iterations 5
#   just tier-compare ~/uffs_data --max-delta-percent 5     # tighter
#   just tier-compare ~/uffs_data --pattern 'xyzzy.log'     # narrow query — expect FAIL
tier-compare *ARGS:
    @rust-script scripts/dev/tier-load-compare.rs {{ ARGS }}

# 24h Phase 6 / Phase 7 Windows-host validation soaks.
#
# Closes the two outstanding Windows-host gates for the v0.6.0
# memory-tiering epic.  Subcommands:
#
#   phase6 — `min_tier="Warm"` 24h idle soak.  Writes a temporary
#            daemon.toml with the per-drive override, soaks idle
#            for 24h, captures hourly per-drive tier snapshots,
#            then drives a 5-min synthetic load and validates:
#            (1) drive C never demotes below Warm, (2) peer drives
#            demote normally, (3) `chosen_ttl_sec` differs between
#            high-rate (C) and idle drives.
#
#   phase7 — Per-shard USN-journal continuous-churn 24h soak.
#            Spawns a background create/modify/delete worker in
#            `%USERPROFILE%\uffs-soak\churn` (or `--churn-dir`),
#            captures hourly snapshots including encrypted-cache
#            file sizes, then validates: new-file latency ≤ 2s,
#            no `uffsd.exe` Working-Set growth > 1.5×, encrypted-
#            cache refresh fired during the soak, no panic / OOM /
#            unexpected demote-to-Parked.
#
# Both subcommands accept `--dev` for an abbreviated 5-min shake-out
# (1-min snapshot interval, relaxed validation) so the harness can
# be sanity-checked on Mac without burning 24h.
#
# Output: $HOME/uffs_soak/<gate>-<YYYYMMDD-HHMMSS>/
#   - daemon.log                   (full RUST_LOG-filtered tee)
#   - snapshots/<NN>h-*.{txt,json} (per-snapshot artifacts)
#   - validation/*.{pass,fail}     (one breadcrumb per assertion)
#   - summary.txt                  (human-readable PASS/FAIL roll-up)
#
# Examples:
#   just soak phase6                    # 24h idle soak, drive C
#   just soak phase6 --drive D          # apply min_tier=Warm to D instead
#   just soak phase6 --dev              # 5-min Mac shake-out
#   just soak phase7                    # 24h churn soak, default churn dir
#   just soak phase7 --churn-rate 20    # 20 files/sec churn rate
#   just soak phase7 --duration 1h      # 1-hour smoke run on real Windows
soak *ARGS:
    @rust-script scripts/dev/long-soak.rs {{ ARGS }}

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
    #
    # Uses `git commit -S` (force-sign) because this commit is
    # pushed directly to `main`, which has the GitHub branch-
    # protection rule "Require signed commits".  Bypassing signing
    # here used to silently break the merge gate on PRs that
    # touched the same files later (PR #130, 2026-05-05).  Inherits
    # the user's `user.signingkey`; fails loudly if no key is
    # configured (correct: the commit cannot land on `main`
    # without one).
    printf "\n\033[0;34m📝 Step 3/4: Git commit...\033[0m\n"
    git add .
    git commit -S -m "debug: quick-deploy v${NEW_VERSION} [skip ci]"
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