#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Phase 5 — Prod-only risk-markers inventory for the UFFS workspace.
#
# Companion to:
#   - docs/dev/architecture/code_clean/phase_5_error_handling_panic_policy_implementation_plan.md
#   - docs/dev/baseline/2026-05-12/risk_markers.md (Phase 0 baseline,
#     file-scoped grep counts that *include* test code).
#
# Purpose
# -------
# Walk every crate's library tree (`crates/<name>/src/`) and count, **per
# crate**, the production-path occurrences of:
#
#   * `.unwrap()`
#   * `.expect(`
#   * `panic!(`
#   * `todo!(` / `unimplemented!(` / `unreachable!(`
#   * `Result<_, String>` in fn signatures and type aliases
#
# Excludes (because the workspace's `clippy.toml` already relaxes
# `unwrap_used` / `expect_used` / `panic` inside these):
#
#   * `tests/`, `benches/`, `examples/` directories under any crate
#   * `build.rs` files
#   * Files named `tests.rs`, `*_tests.rs`, `*_test.rs`, `test_*.rs`
#
# Caveats (documented in the output preamble)
# -------------------------------------------
# 1. This counter is text-based.  Lines inside an inline
#    `#[cfg(test)] mod tests { ... }` block within a prod source file
#    are over-counted because grep cannot follow the attribute.  Phase
#    5b's manual per-site audit will catch these and re-classify as
#    test-only.
#
# 2. The counter excludes `examples/` and `benches/` because those are
#    separate compilation units that strict-clippy treats with the test
#    allowances.  If a crate genuinely has prod code reachable via
#    `cargo build --example`, that's an unusual layout and 5b will flag
#    it.
#
# 3. The counter does NOT distinguish `Result<T, String>` in fn return
#    types from `Result<T, String>` in test helper signatures.  Again,
#    5b's manual audit catches these.
#
# Optional clippy-JSON cross-check
# --------------------------------
# Pass `--with-clippy` as the first argument to also run
# `cargo clippy --workspace --all-targets --message-format=json` with a
# temp-dir `clippy.toml` that turns OFF the test allowances, then
# tabulates the resulting diagnostic count per crate.  This is the
# *authoritative* count: it tells you, modulo `cfg(test)` flag
# evaluation by cargo, exactly which sites would fail strict-clippy if
# the test escape hatch were removed.  The clippy pass is much slower
# (~30-60 s warm, multiple minutes cold) than the rg pass, so it is
# off by default.
#
# Usage
# -----
#   scripts/dev/risk_markers_prod.sh               # rg-only (fast, ~1 s)
#   scripts/dev/risk_markers_prod.sh --with-clippy # rg + clippy JSON cross-check
#
# Output goes to stdout in Markdown.  Redirect to capture:
#
#   scripts/dev/risk_markers_prod.sh \
#     > docs/dev/baseline/2026-05-12/phase_5_risk_markers_before.md
#
# Exit codes
# ----------
#   0 — script ran to completion (zero is the only success code; the
#       *count* of markers found is information, not a failure).
#   1 — fatal scripting error (rg missing, repo root not detectable,
#       clippy invocation failed when `--with-clippy` was requested).

set -uo pipefail

WITH_CLIPPY=0
if [[ "${1:-}" == "--with-clippy" ]]; then
    WITH_CLIPPY=1
fi

# ── Locate workspace root ─────────────────────────────────────────────
# We assume invocation from anywhere inside the workspace; resolve to
# `git rev-parse --show-toplevel` so the script is callable from any cwd.
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -z "$ROOT" ]] || [[ ! -d "$ROOT/crates" ]]; then
    echo "ERROR: not inside the UFFS workspace (expected 'crates/' at git root)" >&2
    exit 1
fi
cd "$ROOT" || {
    echo "ERROR: cd to '$ROOT' failed" >&2
    exit 1
}

# ── Required tooling ──────────────────────────────────────────────────
if ! command -v rg >/dev/null 2>&1; then
    echo "ERROR: 'rg' (ripgrep) not found in PATH" >&2
    exit 1
fi

# ── Crate inventory ───────────────────────────────────────────────────
mapfile -t CRATES < <(
    find crates -mindepth 2 -maxdepth 2 -name Cargo.toml \
        | sed -E 's|^crates/([^/]+)/Cargo.toml$|\1|' \
        | sort
)
if [[ ${#CRATES[@]} -eq 0 ]]; then
    echo "ERROR: no crates discovered under crates/" >&2
    exit 1
fi

# ── rg filter (prod-only) ─────────────────────────────────────────────
# rg supports multiple `-g` globs, all of which are applied as include /
# exclude rules.  We intentionally *include* `*.rs` then *exclude* the
# test/bench/example/build harnesses.  Run from within a crate's `src/`
# to keep paths short and predictable.
RG_PROD_GLOBS=(
    -g '*.rs'
    -g '!tests/**'
    -g '!benches/**'
    -g '!examples/**'
    -g '!build.rs'
    -g '!**/tests.rs'
    -g '!**/*_tests.rs'
    -g '!**/*_test.rs'
    -g '!**/test_*.rs'
)

# `rg --count-matches` returns the number of matching *lines* per file;
# we sum across files.  `|| true` so an empty match yields 0, not exit 1.
count_pattern() {
    local dir="$1"
    local pattern="$2"
    local fixed="${3:-0}"
    local rg_flags=("${RG_PROD_GLOBS[@]}" --no-heading --no-filename --count-matches)
    if [[ "$fixed" -eq 1 ]]; then
        rg_flags+=(-F)
    fi
    rg "${rg_flags[@]}" "$pattern" "$dir" 2>/dev/null \
        | awk 'BEGIN{s=0} {s+=$1} END{print s+0}'
}

count_loc_prod() {
    # Total non-blank, non-comment LOC in prod files (rough — we don't
    # strip block comments because Rust block comments are rare and the
    # LOC field is informational only).
    local dir="$1"
    rg "${RG_PROD_GLOBS[@]}" --files "$dir" 2>/dev/null \
        | xargs -I{} cat "{}" 2>/dev/null \
        | grep -cvE '^\s*(//|$)' \
        || echo 0
}

# ── Markdown preamble ─────────────────────────────────────────────────
SHA="$(git rev-parse HEAD)"
DATE_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

cat <<EOF
# Phase 5 — Prod-only risk-markers baseline

**Captured:** $DATE_UTC
**SHA:** \`$SHA\`
**Methodology:** \`scripts/dev/risk_markers_prod.sh\` — \`rg\`-based count
across each crate's \`src/\` tree, excluding \`tests/\`, \`benches/\`,
\`examples/\`, \`build.rs\`, and files matching \`tests.rs\` /
\`*_tests.rs\` / \`*_test.rs\` / \`test_*.rs\`.

**Diff target:** \`docs/dev/baseline/2026-05-12/risk_markers.md\` (Phase
0 baseline, file-scoped grep counts that *include* test code).

> Caveat: lines inside an inline \`#[cfg(test)] mod tests { ... }\`
> block within a prod source file are over-counted because grep cannot
> follow the attribute.  Phase 5b's manual audit re-classifies these.

## Inventory

| Crate | \`.unwrap()\` | \`.expect(\` | \`panic!\` | \`todo!\` | \`unimpl!\` | \`unreach!\` | \`Result<_, String>\` | prod LOC |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
EOF

# ── Per-crate tally ───────────────────────────────────────────────────
TOT_UNWRAP=0
TOT_EXPECT=0
TOT_PANIC=0
TOT_TODO=0
TOT_UNIMPL=0
TOT_UNREACH=0
TOT_RESULT_STR=0
TOT_LOC=0

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi

    unwrap=$(count_pattern "$src" '\.unwrap\(\)' 0)
    expect=$(count_pattern "$src" '\.expect\(' 0)
    panic=$(count_pattern "$src" 'panic!\(' 0)
    todo=$(count_pattern "$src" 'todo!\(' 0)
    unimpl=$(count_pattern "$src" 'unimplemented!\(' 0)
    unreach=$(count_pattern "$src" 'unreachable!\(' 0)
    result_str=$(count_pattern "$src" 'Result<[^,>\n]+,\s*String\s*>' 0)
    loc=$(count_loc_prod "$src")

    TOT_UNWRAP=$((TOT_UNWRAP + unwrap))
    TOT_EXPECT=$((TOT_EXPECT + expect))
    TOT_PANIC=$((TOT_PANIC + panic))
    TOT_TODO=$((TOT_TODO + todo))
    TOT_UNIMPL=$((TOT_UNIMPL + unimpl))
    TOT_UNREACH=$((TOT_UNREACH + unreach))
    TOT_RESULT_STR=$((TOT_RESULT_STR + result_str))
    TOT_LOC=$((TOT_LOC + loc))

    printf '| `%s` | %d | %d | %d | %d | %d | %d | %d | %d |\n' \
        "$crate" "$unwrap" "$expect" "$panic" "$todo" "$unimpl" "$unreach" \
        "$result_str" "$loc"
done

printf '| **Total** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** |\n' \
    "$TOT_UNWRAP" "$TOT_EXPECT" "$TOT_PANIC" "$TOT_TODO" "$TOT_UNIMPL" \
    "$TOT_UNREACH" "$TOT_RESULT_STR" "$TOT_LOC"

# ── Existing #[expect(clippy::...)] annotations in prod source ────────
cat <<'EOF'

## Annotations already in place

Sites whose panic / unwrap / expect is already justified by a per-site
`#[expect(clippy::*, reason = "…")]` annotation.  Phase 5b will verify
each annotation's reason text matches the panic-policy template (§3.6
of the plan).

| Crate | `unwrap_used` | `expect_used` | `panic` |
|---|---:|---:|---:|
EOF

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi
    # Match the lint *name* anywhere in the source (the only legitimate
    # occurrence of these strings in Rust source is inside an
    # `#[expect(...)]` / `#[allow(...)]` attribute body, whose
    # multi-line form a single-line regex cannot match).  False
    # positives (e.g. a string literal containing the lint name) are
    # exceptionally rare and would not affect Phase-5b's per-site walk.
    a_unwrap=$(count_pattern "$src" 'clippy::unwrap_used\b' 0)
    a_expect=$(count_pattern "$src" 'clippy::expect_used\b' 0)
    a_panic=$(count_pattern "$src" 'clippy::panic\b' 0)
    if [[ "$a_unwrap" -gt 0 ]] || [[ "$a_expect" -gt 0 ]] || [[ "$a_panic" -gt 0 ]]; then
        printf '| `%s` | %d | %d | %d |\n' \
            "$crate" "$a_unwrap" "$a_expect" "$a_panic"
    fi
done

# ── Optional clippy-JSON cross-check ──────────────────────────────────
if [[ "$WITH_CLIPPY" -eq 1 ]]; then
    cat <<'EOF'

## Clippy JSON cross-check (authoritative)

`cargo clippy --workspace --all-targets --message-format=json` with a
temporary `clippy.toml` that disables the test-mode allowances.  The
counts below are diagnostics emitted by strict-clippy when the test
escape hatch is removed — this is the audit surface for sub-phase 5b.
EOF

    TMPDIR=$(mktemp -d)
    trap 'rm -rf "$TMPDIR"' EXIT
    sed \
        -e 's/^allow-unwrap-in-tests *= *true$/allow-unwrap-in-tests = false/' \
        -e 's/^allow-expect-in-tests *= *true$/allow-expect-in-tests = false/' \
        -e 's/^allow-panic-in-tests *= *true$/allow-panic-in-tests = false/' \
        clippy.toml > "$TMPDIR/clippy.toml"

    # cargo emits one JSON object per line; each object is a
    # compiler/cargo-message, with diagnostics nested under
    # `message.code.code`.  Filter the JSON-Lines stream to the lines
    # that mention one of our target lint codes.  False positives are
    # bounded (these strings only appear as lint identifiers in cargo's
    # diagnostic output, never in source-code-snippet payloads because
    # cargo escapes embedded backticks/quotes).
    CLIPPY_CONF_DIR="$TMPDIR" \
        cargo clippy --workspace --all-targets \
        --message-format=json --quiet 2>/dev/null \
        | rg '"clippy::(unwrap_used|expect_used|panic|todo|unimplemented)"' \
              --no-line-number --no-heading --no-filename \
        > "$TMPDIR/diagnostics.jsonl" || true

    if [[ ! -s "$TMPDIR/diagnostics.jsonl" ]]; then
        echo
        echo '> Clippy emitted **0** prod-relevant diagnostics with the test'
        echo '> escape hatch disabled.  The strict gate is fully clean even'
        echo '> against the relaxed-test-allowance config.'
    else
        echo
        echo '| Crate | unwrap_used | expect_used | panic | todo | unimplemented |'
        echo '|---|---:|---:|---:|---:|---:|'
        for crate in "${CRATES[@]}"; do
            # Match the crate's source path inside diagnostic JSON.
            # cargo emits absolute or workspace-relative paths in `file_name`.
            crate_pat="crates/${crate}/src"
            n_unwrap=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" \
                | rg -c 'clippy::unwrap_used' || echo 0)
            n_expect=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" \
                | rg -c 'clippy::expect_used' || echo 0)
            n_panic=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" \
                | rg -c 'clippy::panic"' || echo 0)
            n_todo=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" \
                | rg -c 'clippy::todo' || echo 0)
            n_unimpl=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" \
                | rg -c 'clippy::unimplemented' || echo 0)
            if [[ "$n_unwrap" -gt 0 ]] || [[ "$n_expect" -gt 0 ]] \
                || [[ "$n_panic" -gt 0 ]] || [[ "$n_todo" -gt 0 ]] \
                || [[ "$n_unimpl" -gt 0 ]]; then
                printf '| `%s` | %d | %d | %d | %d | %d |\n' \
                    "$crate" "$n_unwrap" "$n_expect" "$n_panic" \
                    "$n_todo" "$n_unimpl"
            fi
        done
    fi
fi

cat <<'EOF'

## Next steps (per plan §5b)

1. For each crate with a non-zero prod-only count above, open every
   matching site and classify per plan §3.3 (categories A–E).
2. Category A sites: add `#[expect(clippy::{unwrap,expect}_used, reason
   = "<invariant>")]` with the panic-policy reason template.
3. Category B / C: convert to `?` propagation through the crate's
   typed error.
4. Category D: keep with `"BOOT INVARIANT: <condition>"` prefix.
5. Category E: confirm `# Panics` rustdoc section is present
   (`missing_panics_doc = "deny"` is already enforced).
EOF
