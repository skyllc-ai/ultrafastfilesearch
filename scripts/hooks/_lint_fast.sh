#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Staged-scoped parallel fast gate.
#
# Called by:
#   - `scripts/hooks/pre-commit` (git hook)
#   - `just lint-fast`           (manual runs)
#
# Budget: sub-2 seconds on a warm repo.  Soft-skips missing optional tools
# (typos, taplo, reuse) with a one-line install hint so new contributors
# are not blocked before running `just install-dev-tools`.
#
# Design notes
# ------------
# 1. Fan all applicable jobs out in parallel and wait on all PIDs.  Cargo
#    target-dir locks serialise inside cargo anyway, so the extra parallelism
#    is free for the non-cargo jobs (typos, reuse, file-size, taplo).
# 2. Job scope:
#      * Rust changes staged  → `cargo fmt --all -- --check`
#      * TOML changes staged  → `taplo fmt --check` (if installed)
#      * Any changes staged   → `typos .`           (if installed)
#      * Always-on            → `reuse lint`        (if installed)
#      * Always-on            → `file-size-policy`
#    When invoked without a staged set (e.g. `just lint-fast` on a clean
#    worktree) we still run the always-on gates plus an fmt-check / typos
#    scan so the recipe is useful as a "quick sanity" pass.
# 3. Per-job output is captured to a tmpdir and only dumped on failure so
#    the success path stays uncluttered.

set -euo pipefail

# ── Colours ────────────────────────────────────────────────────────────
if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    C_BLUE=$'\033[0;34m'
    C_CYAN=$'\033[0;36m'
    C_GREEN=$'\033[0;32m'
    C_YELLOW=$'\033[1;33m'
    C_RED=$'\033[0;31m'
    C_RESET=$'\033[0m'
else
    C_BLUE= C_CYAN= C_GREEN= C_YELLOW= C_RED= C_RESET=
fi

# ── Staged-file inventory ──────────────────────────────────────────────
STAGED_ALL=$(git diff --cached --name-only --diff-filter=ACMR 2>/dev/null || true)
has_staged_rs()   { printf '%s\n' "$STAGED_ALL" | grep -q '\.rs$';   }
has_staged_toml() { printf '%s\n' "$STAGED_ALL" | grep -q '\.toml$'; }
has_any_staged()  { [[ -n "${STAGED_ALL//[[:space:]]/}" ]]; }

printf '%s🚦 lint-fast — staged-scoped parallel gate%s\n' "$C_BLUE" "$C_RESET"
START=$(date +%s)

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

NAMES=()
PIDS=()
spawn() {
    local name="$1"
    shift
    NAMES+=("$name")
    ( "$@" ) >"$TMP/$name.out" 2>&1 &
    PIDS+=($!)
}

# ── Job dispatch ───────────────────────────────────────────────────────
# Always-on (cheap, no tooling dep beyond bash)
spawn "file-size"   bash scripts/ci/check_file_size_policy.sh

# Rust changes → workspace fmt-check (rustfmt picks up `rustfmt.toml`
# and the nightly pin in `rust-toolchain.toml` automatically).
if has_staged_rs || ! has_any_staged; then
    spawn "fmt-check" cargo fmt --all -- --check
fi

# TOML changes → taplo fmt --check (optional).
if has_staged_toml && command -v taplo >/dev/null 2>&1; then
    spawn "taplo" taplo fmt --check
fi

# Any staged text → typos (optional).  Runs against the full workspace
# because typos is fast enough that scoping adds complexity for ~no win.
if command -v typos >/dev/null 2>&1; then
    spawn "typos" typos .
fi

# REUSE / SPDX compliance (optional; requires `reuse` via pipx).
if command -v reuse >/dev/null 2>&1; then
    spawn "reuse" reuse lint --quiet
fi

# ── Wait on all, collect failures ──────────────────────────────────────
FAILED=()
for i in "${!PIDS[@]}"; do
    if ! wait "${PIDS[$i]}"; then
        FAILED+=("${NAMES[$i]}")
    fi
done

# ── Per-job status line ────────────────────────────────────────────────
for i in "${!NAMES[@]}"; do
    name="${NAMES[$i]}"
    failed=0
    for f in "${FAILED[@]+"${FAILED[@]}"}"; do
        [[ "$f" == "$name" ]] && { failed=1; break; }
    done
    if (( failed )); then
        printf '  %s❌%s %s\n' "$C_RED" "$C_RESET" "$name"
    else
        printf '  %s✅%s %s\n' "$C_GREEN" "$C_RESET" "$name"
    fi
done

# ── Optional-tool hint (once, at the end) ──────────────────────────────
missing=()
command -v typos >/dev/null 2>&1 || missing+=("typos-cli")
command -v taplo >/dev/null 2>&1 || missing+=("taplo-cli")
command -v reuse >/dev/null 2>&1 || missing+=("reuse (pipx install reuse)")
if (( ${#missing[@]} > 0 )); then
    printf '  %s💡%s optional tools missing: %s — run %s`just install-dev-tools`%s\n' \
        "$C_CYAN" "$C_RESET" "${missing[*]}" "$C_CYAN" "$C_RESET"
fi

# ── Dump failing output ────────────────────────────────────────────────
if (( ${#FAILED[@]} > 0 )); then
    for name in "${FAILED[@]}"; do
        printf '\n%s==== %s output ====%s\n' "$C_RED" "$name" "$C_RESET"
        cat "$TMP/$name.out"
    done
    DUR=$(( $(date +%s) - START ))
    printf '\n%s❌ lint-fast FAILED (%ss)%s\n' "$C_RED" "$DUR" "$C_RESET" >&2
    exit 1
fi

DUR=$(( $(date +%s) - START ))
printf '%s✅ lint-fast passed (%ss)%s\n' "$C_GREEN" "$DUR" "$C_RESET"
