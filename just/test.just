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
# Windows-gated code.  Phase W5 of
# `docs/architecture/windows-clippy-and-linux-cross-plan.md` cleared the
# backlog (W0 baseline: prod=93, tests=1342, ci=1346 → zero) so these
# recipes now exit 0 against the workspace.  `lint-ci-windows` is wired
# into the pre-push hook (W5.6) and `pr-fast.yml`'s `windows-lint` job
# runs the equivalent native clippy on `windows-latest` (W5.5).  Requires
# `cargo-xwin` + target `x86_64-pc-windows-msvc` (both from
# `just install-dev-tools`).
#
# Ultra-strict Windows production linting (workspace lints on --lib --bins).
lint-prod-windows:
    @printf "\033[0;34m🪟 Windows strict production lint (cargo xwin clippy)...\033[0m\n"
    cargo xwin clippy --workspace --lib --bins --all-features --target x86_64-pc-windows-msvc --no-deps -- {{ prod_flags }}

# Pragmatic Windows test linting (allows unwrap/expect in test code).
lint-tests-windows:
    @printf "\033[0;34m🪟 Windows strict test lint (cargo xwin clippy)...\033[0m\n"
    cargo xwin clippy --workspace --tests --all-features --target x86_64-pc-windows-msvc --no-deps -- {{ test_flags }}

# Authoritative cross-target lint gate for Windows — mirrors the strict
# `cargo clippy --all-targets --all-features --no-deps -- -D warnings`
# stack the `pr-fast.yml::windows-lint` job runs natively on
# `windows-latest` (Phase W5.5).  Pre-push runs this via
# `scripts/hooks/_lint_pre_push.sh` (Phase W5.6) so new Windows-gated
# code is gated on the same surface CI enforces.
lint-ci-windows:
    @printf "\033[0;34m🚨 Windows CI lint gating (cargo xwin clippy)...\033[0m\n"
    cargo xwin clippy --workspace --all-targets --all-features --target x86_64-pc-windows-msvc --no-deps -- -D warnings

# Native macOS → Linux clippy sweep via cargo-zigbuild (Phase L1.2 of
# `docs/architecture/windows-clippy-and-linux-cross-plan.md`).
#
# Faster inner-loop alternative to `lint-ci-linux` (which uses Docker).
# The Docker path stays authoritative because it mirrors CI's exact
# `rust:latest` image; this recipe is a developer accelerator that
# avoids spinning up a container for quick local sweeps.
#
# How it works: `cargo-zigbuild`'s `clippy` subcommand sets up a `zig
# cc` wrapper as the C cross-compiler so `cc-rs` (which drives
# `zstd-sys` / `psm` / `blake3` build.rs files) produces native Linux
# x86_64 ELF objects from a macOS host — no Docker, no Linux VM, no
# separate cross-toolchain install.
#
# Pinned to `zig 0.14.1`.  Homebrew's `zig` formula tracks latest
# (currently 0.16.x), which has unrelated incompat issues with `psm`
# and `blake3` x86_64 hand-written SIMD assembly that are not worth
# working around at this layer.  `just install-dev-tools` installs
# 0.14.1 from the official ziglang.org tarball into `~/.local/zig/`
# and symlinks it into `~/.cargo/bin/`.
#
# Requires: `zig 0.14.1` and `cargo-zigbuild` (both installed by
# `just install-dev-tools` on macOS hosts).
lint-ci-linux-zig:
    @printf "\033[0;34m🦎 Linux x86_64 clippy via cargo-zigbuild (native macOS → Linux)...\033[0m\n"
    @if ! command -v zig >/dev/null 2>&1; then printf "\033[0;31m❌ zig not found — install with: \033[0;36mjust install-dev-tools\033[0m\n"; exit 1; fi
    @if ! command -v cargo-zigbuild >/dev/null 2>&1; then printf "\033[0;31m❌ cargo-zigbuild not found — install with: \033[0;36mjust install-dev-tools\033[0m\n"; exit 1; fi
    # Override `.cargo/config.toml`'s `target-cpu=native` rustflags for
    # the cross-compile target.  On macOS the host CPU is `apple-m4`
    # (or similar arm64 codename) which cargo-zigbuild propagates to
    # `zig cc -mcpu=native`.  Combined with `-target x86_64-linux-gnu`
    # this corrupts the integrated assembler's dialect detection for
    # hand-written x86_64 SIMD assembly in transitive deps (`psm`'s
    # `src/arch/x86_64.s`, `blake3`'s `c/blake3_*_x86-64_unix.S`),
    # producing thousands of `unrecognized instruction mnemonic` /
    # `invalid operand` errors that look like syntax issues but are
    # really architecture-confusion.  Pin to `x86-64-v3` (the same
    # baseline `release.yml`'s Linux matrix uses) so behaviour matches
    # CI as closely as possible.
    #
    # Note: invoke the cargo-zigbuild binary directly (not via the `cargo
    # zigbuild` subcommand-plugin pattern).  `cargo zigbuild` always
    # routes into the `zigbuild` build subcommand and rejects further
    # subcommand args; the binary itself exposes `clippy` / `check` /
    # `test` / etc. as proper subcommands that wire up the same zig-cc
    # linker shim cargo-zigbuild's `zigbuild` does.
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C target-cpu=x86-64-v3 -C link-arg=-Wl,--gc-sections" \
        cargo-zigbuild clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu --no-deps -- -D warnings
    @printf "\033[0;32m✅ Linux clippy gate (zigbuild) passed\033[0m\n"

# Gate-manifest drift detector (Phase 1 of
# `docs/architecture/gates-manifest-plan.md`).
#
# Verifies that every gate listed in `scripts/ci/gates.toml` is present
# in the corresponding consumer file (`scripts/hooks/_lint_fast.sh`,
# `scripts/hooks/_lint_pre_push.sh`, `.github/workflows/pr-fast.yml`),
# and that every gate present in those consumers has a matching
# manifest entry.  Hard-fails on mismatch; bypass once via
# `BYPASS_GATES_DRIFT=1 git push` (mirrors `COMMIT_SUBJECT_BYPASS=1`).
#
# Wired into the pre-push hook (Bucket 1, fire-and-forget) and the
# `pr-fast.yml::gates-drift` job so any new gate that gets added to a
# consumer without the manifest (or vice versa) blocks merge until
# the manifest is updated.
gates-drift:
    @bash scripts/ci/check_gates_drift.sh

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
#   PR CI      → .github/workflows/pr-fast.yml (classify-gated matrix + native windows-lint)
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
    # cargo-xwin is required by the pre-commit / pre-push `lint-ci-windows`
    # gate (Phase W5.6).  Optional on Windows hosts (where `cargo clippy`
    # is native), but effectively mandatory on macOS / Linux dev machines
    # since CI's authoritative `windows-lint` job runs on `windows-latest`
    # and we need the local mirror to catch issues before PR.
    if ! command -v cargo-xwin >/dev/null 2>&1; then
        cargo install --locked cargo-xwin
    else
        printf "   \033[0;32m✓\033[0m cargo-xwin already installed\n"
    fi
    # Ensure the Windows cross-compilation target is available for
    # `cargo xwin clippy`.  `rustup target add` is idempotent.
    rustup target add x86_64-pc-windows-msvc >/dev/null 2>&1 || true
    if rustup target list --installed 2>/dev/null | grep -q '^x86_64-pc-windows-msvc$'; then
        printf "   \033[0;32m✓\033[0m x86_64-pc-windows-msvc target installed\n"
    fi
    # zig + cargo-zigbuild back the `lint-ci-linux-zig` accelerator
    # (Phase L1).  Optional but recommended on macOS hosts — lets
    # contributors run the Linux clippy sweep natively (no Docker)
    # for fast inner-loop checks.  The Docker `lint-ci-linux` path
    # remains authoritative; this is a dev-loop accelerator only.
    #
    # zig is pinned to 0.14.1 (Homebrew's `zig` formula tracks latest,
    # currently 0.16.x, which has incompat issues with `psm` and
    # `blake3` x86_64 hand-written SIMD assembly that are not worth
    # working around at this layer).  Install via the official
    # ziglang.org tarball; symlink onto PATH via `~/.cargo/bin/zig`.
    if [[ "$(uname -s)" == "Darwin" ]]; then
        ZIG_PIN="0.14.1"
        ZIG_DIR="$HOME/.local/zig/${ZIG_PIN}"
        ZIG_LINK="$HOME/.cargo/bin/zig"
        ZIG_OK=0
        if [[ -x "$ZIG_LINK" ]] && "$ZIG_LINK" version 2>/dev/null | grep -qx "$ZIG_PIN"; then
            ZIG_OK=1
        fi
        if [[ "$ZIG_OK" == "1" ]]; then
            printf "   \033[0;32m✓\033[0m zig ${ZIG_PIN} already installed\n"
        else
            ZIG_ARCH="$(uname -m)"
            case "$ZIG_ARCH" in arm64) ZIG_ARCH="aarch64";; esac
            ZIG_TARBALL="zig-${ZIG_ARCH}-macos-${ZIG_PIN}.tar.xz"
            ZIG_URL="https://ziglang.org/download/${ZIG_PIN}/${ZIG_TARBALL}"
            printf "   \033[0;36m·\033[0m installing zig ${ZIG_PIN} from ${ZIG_URL}\n"
            mkdir -p "$HOME/.local/zig"
            tmpdir="$(mktemp -d)"
            (
                cd "$tmpdir" && \
                curl -fL --retry 5 --retry-delay 3 "$ZIG_URL" -o "$ZIG_TARBALL" && \
                tar -xJf "$ZIG_TARBALL" && \
                rm -rf "$ZIG_DIR" && \
                mv "zig-${ZIG_ARCH}-macos-${ZIG_PIN}" "$ZIG_DIR" && \
                xattr -dr com.apple.quarantine "$ZIG_DIR" 2>/dev/null || true
            )
            rm -rf "$tmpdir"
            ln -sf "$ZIG_DIR/zig" "$ZIG_LINK"
            if "$ZIG_LINK" version 2>/dev/null | grep -qx "$ZIG_PIN"; then
                printf "   \033[0;32m✓\033[0m zig ${ZIG_PIN} installed at \033[0;36m${ZIG_DIR}\033[0m (linked from \033[0;36m${ZIG_LINK}\033[0m)\n"
            else
                printf "   \033[0;31m✗\033[0m zig ${ZIG_PIN} install failed; check ${ZIG_URL} manually\n"
            fi
        fi
        if ! command -v cargo-zigbuild >/dev/null 2>&1; then
            cargo install --locked cargo-zigbuild
        else
            printf "   \033[0;32m✓\033[0m cargo-zigbuild already installed\n"
        fi
        # Ensure the Linux cross-compilation target is available for
        # `cargo-zigbuild clippy --target x86_64-unknown-linux-gnu`.
        rustup target add x86_64-unknown-linux-gnu >/dev/null 2>&1 || true
        if rustup target list --installed 2>/dev/null | grep -q '^x86_64-unknown-linux-gnu$'; then
            printf "   \033[0;32m✓\033[0m x86_64-unknown-linux-gnu target installed\n"
        fi
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

# Validate every non-merge commit subject in `origin/main..HEAD` against
# the same Conventional Commits regex CI runs on the PR title (see
# `.github/workflows/commitlint.yml`).  Same validator the pre-push
# `commit-subjects` job invokes; useful for ad-hoc checks during a
# rebase / amend session before re-running `just lint-pre-push`.
#
# Bypass once: `COMMIT_SUBJECT_BYPASS=1 just check-commits` (logged).
check-commits:
    @printf "\033[0;34m📝 Checking commit subjects against Conventional Commits...\033[0m\n"
    @COMMIT_SUBJECT_VERBOSE=1 bash scripts/ci/check_commit_subjects.sh range origin/main..HEAD

# Install the tracked git hooks (pre-commit + commit-msg + pre-push).
# Idempotent — safe to re-run.  Sets `git config core.hooksPath` to the
# tracked `scripts/hooks/` directory so the hooks survive `git clone`
# and cannot be accidentally deleted from `.git/hooks/`.
install-hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🪝 Installing tracked git hooks...\033[0m\n"
    git config core.hooksPath scripts/hooks
    chmod +x scripts/hooks/pre-commit scripts/hooks/commit-msg scripts/hooks/pre-push \
             scripts/hooks/_lint_fast.sh scripts/hooks/_lint_pre_push.sh \
             scripts/ci/check_commit_subjects.sh
    printf "\033[0;32m✅ Git hooks installed:\033[0m\n"
    printf "   \033[0;36mpre-commit\033[0m → just lint-fast       (staged-scoped, sub-2 s)\n"
    printf "   \033[0;36mcommit-msg\033[0m → Conventional Commits  (subject-line, instant)\n"
    printf "   \033[0;36mpre-push\033[0m   → just lint-pre-push    (workspace parallel)\n"
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

