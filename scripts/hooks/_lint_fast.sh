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
# Budget:
#   * docs / config-only commits:        sub-2 s
#   * Rust commits (warm sccache):       ~8–15 s (three clippy passes;
#                                        cargo's target-dir lock serialises
#                                        them so the 2nd and 3rd are
#                                        incremental-cheap)
#
# Windows xwin check was removed from this gate in Phase 2 of
# dev-flow-implementation-plan.md § 2.4 because its 40-90 s cold cost
# violated the T1 budget.  xwin lives at pre-push (advisory, upgraded
# to strict clippy in Phase W5.6 of windows-clippy-and-linux-cross-plan.md)
# and `pr-fast.yml` (authoritative native `windows-lint` job).
#
# Soft-skips missing optional tools (typos, taplo, reuse) with a
# one-line install hint so new contributors are not blocked before
# running `just install-dev-tools`.
#
# Design notes
# ------------
# 1. Fan all applicable jobs out in parallel and wait on all PIDs.  Cargo
#    target-dir locks serialise the three clippy invocations anyway, but
#    the non-cargo jobs (typos, reuse, file-size, taplo) genuinely run
#    concurrently, and incremental / sccache keep the second and third
#    clippy passes cheap (≈ 1–3 s each when fed warm artifacts).
# 2. Job scope:
#      * Rust changes staged  → `cargo fmt --all -- --check`
#                              + `just lint-prod`   (ULTRA-STRICT prod
#                                  lints: pedantic + nursery + cargo +
#                                  unwrap_used + missing_docs_in_private)
#                              + `just lint-tests`  (same base with
#                                  unwrap/expect allowed in tests)
#                              + `just lint-ci`     (CI-mirror
#                                  `--all-targets -D warnings`)
#                              Same lint stack CI and `just ship` Phase 1
#                              enforce — nothing dirty gets committed.
#      * TOML changes staged  → `taplo fmt --check` (if installed)
#      * Any changes staged   → `typos .`           (if installed)
#      * Always-on            → `reuse lint`        (if installed)
#      * Always-on            → `file-size-policy`
#    When invoked without a staged set (e.g. `just lint-fast` on a clean
#    worktree) we still run the always-on gates plus an fmt-check so
#    the recipe is useful as a "quick sanity" pass.  Clippy passes are
#    skipped on the no-staged path to keep manual invocations snappy.
# 3. Per-job output is captured to a tmpdir and only dumped on failure so
#    the success path stays uncluttered.

set -euo pipefail

# ── Colours ────────────────────────────────────────────────────────────
if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    C_BLUE=$'\033[0;34m'
    C_CYAN=$'\033[0;36m'
    C_GREEN=$'\033[0;32m'
    C_RED=$'\033[0;31m'
    C_RESET=$'\033[0m'
else
    # Explicit empty-string form.  The `VAR=` shorthand works but
    # trips SC1007 because a stray trailing space would turn it into
    # `VAR= other_cmd` (a single-line env override in front of a
    # command invocation).  Using `VAR=''` makes the intent
    # unambiguous and keeps the linter quiet.
    C_BLUE=''
    C_CYAN=''
    C_GREEN=''
    C_RED=''
    C_RESET=''
fi

# ── Staged-file inventory ──────────────────────────────────────────────
# Tool-routing rationale (see also .taplo.toml):
# `supply-chain/*.toml` is cargo-vet's data store, formatted by
# `cargo vet fmt` (which has opinions taplo does not share — e.g.
# column-aligned trailing comments).  Two formatters fighting over the
# same file is a pre-push dead-end, so we split ownership at the
# pre-commit hook: taplo handles every other TOML; cargo-vet handles
# the store.
STAGED_ALL=$(git diff --cached --name-only --diff-filter=ACMR 2>/dev/null || true)
STAGED_TOML=$(printf '%s\n' "$STAGED_ALL" | grep '\.toml$' || true)
STAGED_TOML_NONVET=$(printf '%s\n' "$STAGED_TOML" | grep -v '^supply-chain/' || true)
STAGED_VET=$(printf '%s\n' "$STAGED_TOML" | grep '^supply-chain/' || true)
has_staged_rs()          { printf '%s\n' "$STAGED_ALL" | grep -q '\.rs$';   }
has_staged_toml_nonvet() { [[ -n "${STAGED_TOML_NONVET//[[:space:]]/}" ]]; }
has_staged_vet()         { [[ -n "${STAGED_VET//[[:space:]]/}" ]]; }
has_any_staged()         { [[ -n "${STAGED_ALL//[[:space:]]/}" ]]; }

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

# Rust changes → full ultra-strict clippy trio (same lints `just ship`
# Phase 1 runs).  Skipped on no-staged invocations so manual
# `just lint-fast` stays snappy; `just lint-pre-push` or `just lint-all`
# cover the clippy passes on a clean worktree.
#
# Windows cross-check (cargo-xwin) was REMOVED from pre-commit in
# Phase 2 of dev-flow-implementation-plan.md § 2.4 because its 40-90 s
# cold cost violates the < 15 s T1 budget.  xwin lives at pre-push
# (advisory — strict clippy after Phase W5.6) and `pr-fast.yml`
# (authoritative native `windows-lint` job).
if has_staged_rs; then
    spawn "lint-prod"    just lint-prod
    spawn "lint-tests"   just lint-tests
    spawn "lint-ci"      just lint-ci
fi

# TOML changes (non-supply-chain) → taplo fmt --check on staged files
# only.  Checking the whole workspace would drive-by-flag unrelated
# TOML drift (test definitions, historical configs) that is out of
# scope for this commit.
if has_staged_toml_nonvet && command -v taplo >/dev/null 2>&1; then
    # shellcheck disable=SC2086
    spawn "taplo" bash -c "taplo fmt --check $(printf '%s ' $STAGED_TOML_NONVET)"
fi

# Supply-chain TOML changes → format-drift detector for cargo-vet's
# store.  Logic lives in `scripts/hooks/_check_vet_fmt.sh`; the hook
# just wires it into the parallel scheduler.  See that script's
# header for the cargo-vet-owns-supply-chain rationale.
if has_staged_vet && command -v cargo-vet >/dev/null 2>&1; then
    spawn "vet-fmt" bash scripts/hooks/_check_vet_fmt.sh
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
    # NOTE: no backticks around `just install-dev-tools` — the cyan
    # ANSI codes already emphasise the command, and literal backticks
    # inside a single-quoted printf format string trip shellcheck
    # SC2016 ("expressions don't expand in single quotes") even
    # though they are harmless literal bytes in this context.
    printf '  %s💡%s optional tools missing: %s — run %sjust install-dev-tools%s\n' \
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
