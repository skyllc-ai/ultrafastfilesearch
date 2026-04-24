# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.

# Verify Rust toolchain consistency.
verify-toolchain:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔧 Verifying Rust toolchain consistency...\033[0m\n"
    rustc --version
    cargo --version
    rustfmt --version
    cargo clippy --version
    cargo metadata --format-version 1 --no-deps >/dev/null
    printf "\033[0;32m✅ Nightly toolchain verified (via rust-toolchain.toml)\033[0m\n"

# Format code (nightly rustfmt with edition 2024 + style_edition 2024).
fmt:
    @printf "\033[0;34m📝 Formatting code (nightly rustfmt, style_edition 2024)...\033[0m\n"
    cargo fmt --all
    @printf "\033[0;32m✅ Formatting complete\033[0m\n"

# Check code formatting without applying fixes.
fmt-check:
    @printf "\033[0;34m📝 Checking code formatting (nightly rustfmt, style_edition 2024)...\033[0m\n"
    cargo fmt --all -- --check

# Run all tests with nextest.
test:
    @printf "\033[0;34m🧪 Running all tests with nextest (FAST-FAIL)...\033[0m\n"
    cargo llvm-cov nextest --workspace --all-features --all-targets --no-report

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
    @if command -v cargo-llvm-cov >/dev/null 2>&1; then \
        cargo llvm-cov nextest --workspace --all-features --all-targets --html; \
        printf "\033[0;32m📁 Coverage report: target/llvm-cov/html/index.html\033[0m\n"; \
    else \
        printf "\033[1;33mInstalling cargo-llvm-cov...\033[0m\n"; \
        just _install-if-missing cargo-llvm-cov cargo-llvm-cov; \
        cargo llvm-cov nextest --workspace --all-features --all-targets --html; \
        printf "\033[0;32m📁 Coverage report: target/llvm-cov/html/index.html\033[0m\n"; \
    fi

# Ultra-strict production linting (workspace lints + justfile overrides).
lint-prod:
    @printf "\033[0;34m🔍 Ultra-strict production linting (FAST-FAIL)...\033[0m\n"
    cargo clippy --workspace --lib --bins --all-features --no-deps -- {{ prod_flags }}

# Pragmatic test linting (allows unwrap/expect in test code).
lint-tests:
    @printf "\033[0;34m🧪 Pragmatic test linting (FAST-FAIL)...\033[0m\n"
    cargo clippy --workspace --tests --all-features --no-deps -- {{ test_flags }}

# Gate residual lints in CI (deny all warnings).
# MUST mirror .github/workflows/pr-fast.yml `clippy` job exactly so local
# runs catch everything CI catches.  Keep the two commands in lockstep.
lint-ci:
    @printf "\033[0;34m🚨 CI lint gating (FAST-FAIL)...\033[0m\n"
    cargo clippy --workspace --all-targets --all-features --no-deps -- -D warnings

# Cross-platform CI lint gate via Docker (Linux target).
# Catches Linux-only clippy drift that the local host (macOS) cannot reproduce
# — e.g. platform-dependent type aliases like `libc::time_t` where nightly
# clippy's `cast_possible_wrap` behaviour differs between targets.
#
# Uses a named Docker volume for `target/` so repeated runs reuse the Linux
# artifact cache (first run is cold, subsequent runs are incremental).
# The workspace nightly pin in `rust-toolchain.toml` is honoured via rustup.
#
# Requires: Docker (or compatible runtime such as colima, OrbStack, Podman).
lint-ci-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🐧 Cross-platform CI lint gate (Linux x86_64 via Docker)...\033[0m\n"
    if ! command -v docker >/dev/null 2>&1; then
        printf "\033[0;31m❌ docker not found — install Docker Desktop / colima / OrbStack to use this recipe.\033[0m\n"
        exit 1
    fi
    if ! docker info >/dev/null 2>&1; then
        printf "\033[0;31m❌ docker daemon unreachable — start your container runtime first.\033[0m\n"
        exit 1
    fi
    docker volume create uffs-linux-target >/dev/null
    docker volume create uffs-linux-cargo-registry >/dev/null
    docker volume create uffs-linux-cargo-git >/dev/null
    docker run --rm \
        --platform linux/amd64 \
        -v "$PWD:/workspace" \
        -v uffs-linux-target:/workspace/target \
        -v uffs-linux-cargo-registry:/usr/local/cargo/registry \
        -v uffs-linux-cargo-git:/usr/local/cargo/git \
        -w /workspace \
        -e CARGO_TERM_COLOR=always \
        rust:latest \
        bash -lc "rustup show >/dev/null && cargo clippy --workspace --all-targets --all-features --no-deps -- -D warnings"
    printf "\033[0;32m✅ Linux CI lint gate passed\033[0m\n"

# Cross-platform strict lint gate for Windows (cargo-xwin clippy).
#
# The Windows counterpart of `lint-prod` / `lint-tests` / `lint-ci` — mirrors
# the exact same flag stack so the strict clippy rules apply identically on
# Windows-gated code.  Until Phase W5 of
# `docs/architecture/windows-clippy-and-linux-cross-plan.md` lands, these
# recipes emit hundreds of lints (W0 baseline: prod=93, tests=1342, ci=1346)
# and are therefore not yet wired into `lint-all` or the pre-push hook.
# They exist as the authoritative surface the Windows backlog is being
# converged against phase by phase.  Requires `cargo-xwin` + target
# `x86_64-pc-windows-msvc` (both from `just install-dev-tools`).
#
# Ultra-strict Windows production linting (workspace lints on --lib --bins).
lint-prod-windows:
    @printf "\033[0;34m🪟 Windows strict production lint (cargo xwin clippy)...\033[0m\n"
    cargo xwin clippy --workspace --lib --bins --all-features --target x86_64-pc-windows-msvc --no-deps -- {{ prod_flags }}

# Pragmatic Windows test linting (allows unwrap/expect in test code).
lint-tests-windows:
    @printf "\033[0;34m🪟 Windows strict test lint (cargo xwin clippy)...\033[0m\n"
    cargo xwin clippy --workspace --tests --all-features --target x86_64-pc-windows-msvc --no-deps -- {{ test_flags }}

# This is the gate the PR Fast CI `windows-check` job will adopt after Phase
# W5 (see cross-target plan) completes.
#
# Windows CI lint gating (all-targets, deny warnings).
lint-ci-windows:
    @printf "\033[0;34m🚨 Windows CI lint gating (cargo xwin clippy)...\033[0m\n"
    cargo xwin clippy --workspace --all-targets --all-features --target x86_64-pc-windows-msvc --no-deps -- -D warnings

# Host-only on purpose — Windows (`lint-*-windows`) and Linux-in-Docker
# (`lint-ci-linux`) are separate recipes.  They will be wired into this
# sweep after their backlogs converge to zero (Phase W5 of the
# cross-target plan: `docs/architecture/windows-clippy-and-linux-cross-plan.md`).
#
# Full lint sweep: production + tests + CI gate + formatting.
lint-all:
    @printf "\033[0;34m🔍 Full lint sweep (prod + tests + CI + fmt)...\033[0m\n"
    just lint-prod
    just lint-tests
    just lint-ci
    just fmt-check
    @printf "\033[0;32m✅ All lint checks passed\033[0m\n"

# ── Shift-left quality gates ───────────────────────────────────────────
# Four-layer model (documented in CONTRIBUTING.md):
#   IDE save   → rust-analyzer check-on-save
#   pre-commit → `just lint-fast`       (staged-scoped, sub-2 s)
#   pre-push   → `just lint-pre-push`   (workspace-parallel, ≤ 60 s)
#   PR CI      → .github/workflows/pr-fast.yml (classify-gated matrix + native windows-check)
# Recipes below back both the git hooks and manual invocations so the
# two surfaces never drift — see `scripts/hooks/_lint_*.sh` for the
# shared parallel runners.

# Spell-check the workspace (requires `typos-cli`).
# Pass scopes or flags, e.g. `just typos crates/uffs-core/src`.
typos *ARGS:
    @printf "\033[0;34m🔤 typos (spell check)...\033[0m\n"
    typos {{ ARGS }}

# REUSE / SPDX compliance lint (requires `reuse`, install via pipx).
reuse:
    @printf "\033[0;34m📜 REUSE/SPDX lint...\033[0m\n"
    reuse lint --quiet

# Check TOML formatting (requires `taplo-cli`).
taplo-check *ARGS:
    @printf "\033[0;34m📋 taplo fmt --check...\033[0m\n"
    taplo fmt --check {{ ARGS }}

# Apply TOML formatting (requires `taplo-cli`).
taplo-fmt *ARGS:
    @printf "\033[0;34m📋 taplo fmt (apply)...\033[0m\n"
    taplo fmt {{ ARGS }}

# Install optional dev-tool binaries used by the pre-commit / pre-push hooks.
# `typos-cli` and `taplo-cli` are cargo-installable; `reuse` needs pipx.
# Idempotent: prints a ✓ line for each tool already present.
install-dev-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📦 Installing dev tools (typos-cli, taplo-cli, cargo-xwin)...\033[0m\n"
    if ! command -v typos >/dev/null 2>&1; then
        cargo install --locked typos-cli
    else
        printf "   \033[0;32m✓\033[0m typos already installed\n"
    fi
    if ! command -v taplo >/dev/null 2>&1; then
        cargo install --locked taplo-cli
    else
        printf "   \033[0;32m✓\033[0m taplo already installed\n"
    fi
    # cargo-xwin is required by the pre-commit / pre-push `check-windows`
    # gate.  Optional on Windows hosts (where `cargo check` is native), but
    # effectively mandatory on macOS / Linux dev machines since CI only
    # runs Ubuntu and cannot catch Windows-specific compile errors.
    if ! command -v cargo-xwin >/dev/null 2>&1; then
        cargo install --locked cargo-xwin
    else
        printf "   \033[0;32m✓\033[0m cargo-xwin already installed\n"
    fi
    # Ensure the Windows cross-compilation target is available for
    # `cargo xwin check`.  `rustup target add` is idempotent.
    rustup target add x86_64-pc-windows-msvc >/dev/null 2>&1 || true
    if rustup target list --installed 2>/dev/null | grep -q '^x86_64-pc-windows-msvc$'; then
        printf "   \033[0;32m✓\033[0m x86_64-pc-windows-msvc target installed\n"
    fi
    if ! command -v reuse >/dev/null 2>&1; then
        printf "   \033[1;33m!\033[0m reuse not found — install with: \033[0;36mpipx install reuse\033[0m\n"
    else
        printf "   \033[0;32m✓\033[0m reuse already installed\n"
    fi
    printf "\033[0;32m✅ Dev tools check complete.\033[0m\n"

# Fast staged-scoped gate (the pre-commit bundle; also runnable manually).
# Runs fmt-check (if *.rs staged), taplo-check (if *.toml staged + taplo),
# typos (optional), reuse (optional) and the file-size-policy script in
# parallel.  Budget: sub-2 s.  Soft-skips missing optional tools.
lint-fast:
    @bash scripts/hooks/_lint_fast.sh

# Parallel pre-push gate (the pre-push bundle; also runnable manually).
# Fans out clippy (-D warnings) + fmt-check + rustdoc (-D warnings) +
# cargo deny + nextest --no-run (test-binary link check) + file-size-policy
# + typos (optional) + reuse (optional).  Budget: ≈ 25–45 s warm, ≈ 60–90 s
# cold.  All failures surface in one pass — per-job ✅/❌ grid, then every
# failing job's full output is dumped before exit.
lint-pre-push:
    @bash scripts/hooks/_lint_pre_push.sh

# Install the tracked git hooks (pre-commit + pre-push).
# Idempotent — safe to re-run.  Sets `git config core.hooksPath` to the
# tracked `scripts/hooks/` directory so the hooks survive `git clone`
# and cannot be accidentally deleted from `.git/hooks/`.
install-hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🪝 Installing tracked git hooks...\033[0m\n"
    git config core.hooksPath scripts/hooks
    chmod +x scripts/hooks/pre-commit scripts/hooks/pre-push \
             scripts/hooks/_lint_fast.sh scripts/hooks/_lint_pre_push.sh
    printf "\033[0;32m✅ Git hooks installed:\033[0m\n"
    printf "   \033[0;36mpre-commit\033[0m → just lint-fast     (staged-scoped, sub-2 s)\n"
    printf "   \033[0;36mpre-push\033[0m   → just lint-pre-push  (workspace parallel)\n"
    printf "   \033[0;33mBypass once\033[0m with \`git commit --no-verify\` / \`git push --no-verify\`.\n"
    printf "   \033[0;33mInstall optional tools\033[0m with \`just install-dev-tools\`.\n"

# Uninstall the tracked git hooks (resets to .git/hooks/ default).
uninstall-hooks:
    @printf "\033[0;34m🪝 Uninstalling tracked git hooks...\033[0m\n"
    git config --unset core.hooksPath || true
    @printf "\033[0;32m✅ Git hooks uninstalled — commits and pushes are no longer gated.\033[0m\n"

# Validate rustdoc links and warnings (catches private-intra-doc-links etc.).
rustdoc:
    @printf "\033[0;34m📚 Rustdoc link validation...\033[0m\n"
    RUSTDOCFLAGS='-Dwarnings' cargo doc --workspace --all-features --no-deps

# Enforce oversized Rust file policy with explicit exceptions.
file-size-policy:
    @printf "\033[0;34m📏 Checking oversized Rust file policy...\033[0m\n"
    bash scripts/ci/check_file_size_policy.sh

# Comprehensive code validation (CI/CD grade).
check:
    @printf "\033[0;34m🔍 Comprehensive validation (CI/CD grade)...\033[0m\n"
    just file-size-policy
    cargo check --workspace --all-targets --all-features
    just fmt-check
    @printf "\033[0;32m✅ Comprehensive check passed\033[0m\n"

# Quick code validation.
check-quick:
    @printf "\033[0;34m⚡ Quick validation (development speed)...\033[0m\n"
    cargo check --workspace

# Bump Polars to latest main HEAD (updates rev= hash, refreshes lockfile, checks build).
polars:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📦 Bumping Polars to latest main HEAD...\033[0m\n"

    NEW_REV=$(git ls-remote https://github.com/pola-rs/polars refs/heads/main | awk '{print $1}')
    if [ -z "$NEW_REV" ]; then
        printf "\033[0;31m❌ Failed to fetch latest Polars commit\033[0m\n"
        exit 1
    fi

    OLD_REV=$(sed -n 's/.*rev = "\([a-f0-9]*\)".*/\1/p' crates/uffs-polars/Cargo.toml)
    if [ "$OLD_REV" = "$NEW_REV" ]; then
        printf "\033[0;32m✅ Already at latest: %s\033[0m\n" "${NEW_REV:0:12}"
        exit 0
    fi

    printf "   Old: %s\n" "${OLD_REV:0:12}"
    printf "   New: %s\n" "${NEW_REV:0:12}"

    # macOS sed requires a space after -i, Linux doesn't care — use backup + remove
    sed -i.bak "s|rev = \"${OLD_REV}\"|rev = \"${NEW_REV}\"|" crates/uffs-polars/Cargo.toml
    rm -f crates/uffs-polars/Cargo.toml.bak
    cargo update -p polars
    printf "\033[0;34m🔨 Checking build...\033[0m\n"
    cargo check -p uffs-polars
    printf "\033[0;32m✅ Polars bumped to %s\033[0m\n" "${NEW_REV:0:12}"

# Update all workspace dependencies to latest compatible versions.
update-deps:
    @printf "\033[0;34m📦 Updating all workspace dependencies...\033[0m\n"
    cargo update
    @printf "\033[0;32m✅ All dependencies updated\033[0m\n"

