#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Workspace-wide parallel pre-push gate.
#
# Called by:
#   - `scripts/hooks/pre-push` (git hook)
#   - `just lint-pre-push`     (manual runs)
#
# Budget: ≈ 25–45 s on an sccache-warm workspace; ≈ 60–90 s cold.  The
# heaviest jobs (clippy, rustdoc) share the same target-dir; cargo's
# file lock serialises them as needed, while the non-cargo jobs (deny,
# typos, reuse, file-size) genuinely run in parallel.
#
# Mandatory jobs (any failure aborts the push):
#   * clippy    — `-D warnings`, `--all-targets --all-features`
#                 (covers test / example / bench compile as a side-effect).
#   * fmt       — `cargo fmt --all -- --check`.
#   * rustdoc   — `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps`.
#   * deny      — advisories / bans / licences / sources.
#   * file-size — oversized-Rust-file policy.
#   * tests     — `cargo nextest run --no-run`: links every test binary
#                 without running it.  Catches `#[cfg(test)]` drift,
#                 missing dev-dep, and linker-level regressions that
#                 `cargo clippy --all-targets` (check-only, no linking)
#                 misses.  On sccache-warm runs this costs ~5 s on top
#                 of clippy's compile; on cold it dominates the push.
#
# Optional jobs (soft-skipped when tool missing):
#   * typos     — cheap spell-check across the repo.
#   * reuse     — SPDX / licence-header compliance.
#
# The Linux-only lint drift gate (`just lint-ci-linux` via Docker) is NOT
# run here — it is a minutes-scale cross-platform check best left to CI
# or a conscious manual invocation.

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

printf '%s🚦 lint-pre-push — workspace parallel gate%s\n' "$C_BLUE" "$C_RESET"
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

# ── Mandatory gates ────────────────────────────────────────────────────
# NOTE: clippy `--all-targets --all-features --no-deps` kept in lockstep with
# `.github/workflows/ci.yml`'s `Tier 1 / Clippy` step.  Keep the flag set
# identical to avoid local / CI drift (see `just lint-ci`).
spawn "clippy"    cargo clippy --workspace --all-targets --all-features --no-deps -- -D warnings
spawn "fmt"       cargo fmt --all -- --check
spawn "rustdoc"   env RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --all-features --no-deps
spawn "deny"      cargo deny check --hide-inclusion-graph
spawn "tests"     cargo nextest run --workspace --all-targets --all-features --no-run --hide-progress-bar
spawn "file-size" bash scripts/ci/check_file_size_policy.sh

# ── Optional gates ─────────────────────────────────────────────────────
if command -v typos >/dev/null 2>&1; then
    spawn "typos" typos .
fi
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

# ── Optional-tool hint ─────────────────────────────────────────────────
missing=()
command -v typos >/dev/null 2>&1 || missing+=("typos-cli")
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
    printf '\n%s❌ lint-pre-push FAILED (%ss) — push aborted%s\n' "$C_RED" "$DUR" "$C_RESET" >&2
    printf '%s   Fix the warnings and retry, or bypass once with `git push --no-verify`.%s\n' "$C_YELLOW" "$C_RESET" >&2
    exit 1
fi

DUR=$(( $(date +%s) - START ))
printf '%s✅ lint-pre-push passed (%ss)%s\n' "$C_GREEN" "$DUR" "$C_RESET"
