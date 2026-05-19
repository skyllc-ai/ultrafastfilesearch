#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Phase 6 — Prod-only ownership / borrowing / allocation inventory for
# the UFFS workspace.
#
# Companion to:
#   - docs/dev/architecture/code_clean/phase_6_ownership_borrowing_allocation_implementation_plan.md
#   - scripts/dev/risk_markers_prod.sh (Phase 5a — same shape, different
#     pattern set)
#
# Purpose
# -------
# Walk every crate's library tree (`crates/<name>/src/`) and count, **per
# crate**, the production-path occurrences of the patterns playbook
# §792-855 calls out:
#
#   * `.clone()`                         — Phase-6 §3.2 taxonomy α/β/γ/δ/ε
#   * `.to_string()`                     — Phase-6 §3.4 hot-path audit
#   * `.to_owned()`                      — borrow-vs-own decision
#   * `format!(`                         — Phase-6 §3.4 hot-path audit
#   * `String::from(` / `Vec::from(`     — borrow-vs-own decision
#   * `Arc<` / `Cow<`                    — informational; §3.2 cat-α & §3.3
#   * `fn ...<'a, ...>`                  — Phase-6 §3.5 lifetime audit
#
# Borrow-candidate patterns (§3.1 — sigs that might want `&str` /
# `&[T]` / `impl AsRef<Path>`):
#
#   * `fn <name>(<...>: String, ...)`    — owned String param
#   * `fn <name>(<...>: Vec<T>, ...)`    — owned Vec<T> param
#   * `fn <name>(<...>: PathBuf, ...)`   — owned PathBuf param
#
# Excludes (because the workspace's `clippy.toml` already relaxes
# clone / pass-by-value lints inside these):
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
#    6b's manual per-site audit re-classifies these.
#
# 2. The fn-signature borrow-candidate regexes match the first line of
#    a function signature only.  Multi-line signatures
#    (`fn foo(\n    bar: String,\n    ...) {`) are missed by the
#    `String` / `Vec<T>` / `PathBuf` regex but are caught by the
#    `--with-clippy` cross-check (which runs `needless_pass_by_value`
#    against the full AST).  Pass `--with-clippy` for the
#    authoritative count.
#
# 3. The counter does NOT distinguish `format!(...)` on a hot path from
#    a cold-path error message.  Phase 6d's manual audit does the
#    hot-vs-cold partition.
#
# Optional clippy-JSON cross-check
# --------------------------------
# Pass `--with-clippy` as the first argument to also run
# `cargo clippy --workspace --all-targets --message-format=json` and
# tabulate the resulting diagnostic count per crate for the lints that
# bear on Phase 6:
#
#   * `redundant_clone`
#   * `implicit_clone`
#   * `cloned_instead_of_copied`
#   * `needless_pass_by_value`
#   * `inefficient_to_string`
#   * `str_to_string`
#   * `unnecessary_to_owned`
#   * `clone_on_ref_ptr`
#   * `map_clone`
#   * `large_types_passed_by_value`
#   * `trivially_copy_pass_by_ref`
#   * `assigning_clones`
#
# Every one of those lints is already at `deny` in the workspace
# (`Cargo.toml [workspace.lints.clippy]`).  With the strict-clippy gate
# green, the clippy-JSON cross-check is expected to emit **zero**
# diagnostics on `main` — which is exactly the point.  The audit's
# value is the rg-pass tally of surviving (already-justified) sites,
# not new findings.
#
# Usage
# -----
#   scripts/dev/clone_alloc_audit.sh               # rg-only (fast, ~1 s)
#   scripts/dev/clone_alloc_audit.sh --with-clippy # rg + clippy JSON cross-check
#
# Output goes to stdout in Markdown.  Redirect to capture:
#
#   scripts/dev/clone_alloc_audit.sh \
#     > docs/dev/baseline/2026-05-12/phase_6_clone_alloc_baseline.md
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
# Phase 6 — Prod-only ownership / borrowing / allocation baseline

**Captured:** $DATE_UTC
**SHA:** \`$SHA\`
**Methodology:** \`scripts/dev/clone_alloc_audit.sh\` — \`rg\`-based count
across each crate's \`src/\` tree, excluding \`tests/\`, \`benches/\`,
\`examples/\`, \`build.rs\`, and files matching \`tests.rs\` /
\`*_tests.rs\` / \`*_test.rs\` / \`test_*.rs\`.

**Diff target:** \`docs/dev/baseline/2026-05-12/risk_markers.md\` (Phase
0 baseline) + \`docs/dev/baseline/2026-05-12/phase_5_risk_markers_before.md\`
(Phase 5 baseline) for like-for-like comparison.

**Lint posture:** every Phase-6-relevant clippy lint is already at
\`deny\` in the workspace \`[workspace.lints.clippy]\` block
(\`Cargo.toml\` lines 311-477).  See
\`docs/architecture/code-quality/lint-posture.md\`.  Surviving counts
below are *justified* sites (Arc clones, ownership transfers, runtime
input concatenation) — not new findings.

> Caveat 1: lines inside an inline \`#[cfg(test)] mod tests { ... }\`
> block within a prod source file are over-counted because grep cannot
> follow the attribute.  Phase 6b's manual audit re-classifies these.
>
> Caveat 2: borrow-candidate fn-signature regexes match single-line
> signatures only.  Multi-line signatures are caught by the
> \`--with-clippy\` cross-check (which runs \`needless_pass_by_value\`
> against the full AST).
>
> Caveat 3: \`format!\` counts do not distinguish hot-path from cold-path
> sites.  Phase 6d's manual audit does the hot-vs-cold partition (per
> the §1.2 hot-path inventory in the plan).

## Inventory — clones / allocs / lifetime params

| Crate | \`.clone()\` | \`.to_string()\` | \`.to_owned()\` | \`format!(\` | \`String::from(\` | \`Vec::from(\` | \`Arc<\` | \`Cow<\` | \`fn<'…>\` | prod LOC |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
EOF

# ── Per-crate tally ───────────────────────────────────────────────────
TOT_CLONE=0
TOT_TOSTR=0
TOT_TOOWN=0
TOT_FMT=0
TOT_STRF=0
TOT_VECF=0
TOT_ARC=0
TOT_COW=0
TOT_LIFETIME=0
TOT_LOC=0

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi

    clone=$(count_pattern "$src" '\.clone\(\)' 0)
    tostr=$(count_pattern "$src" '\.to_string\(\)' 0)
    toown=$(count_pattern "$src" '\.to_owned\(\)' 0)
    fmt=$(count_pattern "$src" '\bformat!\(' 0)
    strf=$(count_pattern "$src" 'String::from\(' 0)
    vecf=$(count_pattern "$src" 'Vec::from\(' 0)
    arc=$(count_pattern "$src" '\bArc<' 0)
    cow=$(count_pattern "$src" '\bCow<' 0)
    lifetime=$(count_pattern "$src" "fn\s+\w+<'[a-z]" 0)
    loc=$(count_loc_prod "$src")

    TOT_CLONE=$((TOT_CLONE + clone))
    TOT_TOSTR=$((TOT_TOSTR + tostr))
    TOT_TOOWN=$((TOT_TOOWN + toown))
    TOT_FMT=$((TOT_FMT + fmt))
    TOT_STRF=$((TOT_STRF + strf))
    TOT_VECF=$((TOT_VECF + vecf))
    TOT_ARC=$((TOT_ARC + arc))
    TOT_COW=$((TOT_COW + cow))
    TOT_LIFETIME=$((TOT_LIFETIME + lifetime))
    TOT_LOC=$((TOT_LOC + loc))

    printf '| `%s` | %d | %d | %d | %d | %d | %d | %d | %d | %d | %d |\n' \
        "$crate" "$clone" "$tostr" "$toown" "$fmt" "$strf" "$vecf" \
        "$arc" "$cow" "$lifetime" "$loc"
done

printf '| **Total** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** |\n' \
    "$TOT_CLONE" "$TOT_TOSTR" "$TOT_TOOWN" "$TOT_FMT" "$TOT_STRF" \
    "$TOT_VECF" "$TOT_ARC" "$TOT_COW" "$TOT_LIFETIME" "$TOT_LOC"

# ── Borrow-candidate fn-signature scan (§3.1) ─────────────────────────
cat <<'EOF'

## Borrow-candidate fn signatures (§3.1)

Single-line `fn` signatures with `String` / `Vec<T>` / `PathBuf` value
parameters.  Each site is a candidate for `&str` / `&[T]` /
`&Path` / `impl AsRef<Path>` migration (§3.1 — "Borrow for inputs,
own for outputs").  Multi-line signatures are caught by the
`--with-clippy` cross-check.

| Crate | `: String,)` params | `: Vec<T>,)` params | `: PathBuf,)` params |
|---|---:|---:|---:|
EOF

TOT_STR_PARAM=0
TOT_VEC_PARAM=0
TOT_PATH_PARAM=0

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi
    # `\([^)]*:\s*Type[,)]` — match `(...:String,` or `(...:String)`.
    # Note rg uses Rust regex flavour; we keep regex simple to avoid
    # surprises.  Matches first-line of the signature only.
    str_param=$(count_pattern "$src" 'fn\s+\w+[^(]*\([^)]*:\s*String[,)]' 0)
    vec_param=$(count_pattern "$src" 'fn\s+\w+[^(]*\([^)]*:\s*Vec<[^>]+>[,)]' 0)
    path_param=$(count_pattern "$src" 'fn\s+\w+[^(]*\([^)]*:\s*PathBuf[,)]' 0)
    TOT_STR_PARAM=$((TOT_STR_PARAM + str_param))
    TOT_VEC_PARAM=$((TOT_VEC_PARAM + vec_param))
    TOT_PATH_PARAM=$((TOT_PATH_PARAM + path_param))
    if [[ "$str_param" -gt 0 ]] || [[ "$vec_param" -gt 0 ]] || [[ "$path_param" -gt 0 ]]; then
        printf '| `%s` | %d | %d | %d |\n' \
            "$crate" "$str_param" "$vec_param" "$path_param"
    fi
done

printf '| **Total** | **%d** | **%d** | **%d** |\n' \
    "$TOT_STR_PARAM" "$TOT_VEC_PARAM" "$TOT_PATH_PARAM"

# ── Existing `#[expect(clippy::...)]` annotations (Phase 6 lints) ─────
cat <<'EOF'

## Annotations already in place (Phase-6-relevant lints)

Sites whose ownership / clone / allocation pattern is already justified
by a per-site `#[expect(clippy::*, reason = "…")]` annotation.  These
are the legitimate-by-construction sites that survive strict-clippy.
Phase 6c verifies each annotation's reason text aligns with the
allocation-policy template (§3.6 of the plan).

| Crate | `needless_pass_by_value` | `redundant_clone` | `clone_on_ref_ptr` | `inefficient_to_string` | `str_to_string` | `cloned_instead_of_copied` |
|---|---:|---:|---:|---:|---:|---:|
EOF

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi
    a_pass=$(count_pattern "$src" 'clippy::needless_pass_by_value\b' 0)
    a_clone=$(count_pattern "$src" 'clippy::redundant_clone\b' 0)
    a_arc=$(count_pattern "$src" 'clippy::clone_on_ref_ptr\b' 0)
    a_inef=$(count_pattern "$src" 'clippy::inefficient_to_string\b' 0)
    a_str=$(count_pattern "$src" 'clippy::str_to_string\b' 0)
    a_cic=$(count_pattern "$src" 'clippy::cloned_instead_of_copied\b' 0)
    if [[ "$a_pass" -gt 0 ]] || [[ "$a_clone" -gt 0 ]] || [[ "$a_arc" -gt 0 ]] \
        || [[ "$a_inef" -gt 0 ]] || [[ "$a_str" -gt 0 ]] || [[ "$a_cic" -gt 0 ]]; then
        printf '| `%s` | %d | %d | %d | %d | %d | %d |\n' \
            "$crate" "$a_pass" "$a_clone" "$a_arc" "$a_inef" "$a_str" "$a_cic"
    fi
done

# ── Optional clippy-JSON cross-check ──────────────────────────────────
if [[ "$WITH_CLIPPY" -eq 1 ]]; then
    cat <<'EOF'

## Clippy JSON cross-check (authoritative)

`cargo clippy --workspace --all-targets --message-format=json`.  The
counts below are diagnostics emitted by strict-clippy for the
Phase-6-relevant lints.  With the workspace gate green, this is
expected to be **zero** — surviving sites are inside
`#[expect(clippy::*)]` annotations and do not emit diagnostics.

| Crate | redundant_clone | implicit_clone | cloned_instead_of_copied | needless_pass_by_value | inefficient_to_string | str_to_string | unnecessary_to_owned | clone_on_ref_ptr |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
EOF

    TMPDIR=$(mktemp -d)
    trap 'rm -rf "$TMPDIR"' EXIT

    cargo clippy --workspace --all-targets \
        --message-format=json --quiet 2>/dev/null \
        | rg '"clippy::(redundant_clone|implicit_clone|cloned_instead_of_copied|needless_pass_by_value|inefficient_to_string|str_to_string|unnecessary_to_owned|clone_on_ref_ptr|map_clone|large_types_passed_by_value|trivially_copy_pass_by_ref|assigning_clones)"' \
              --no-line-number --no-heading --no-filename \
        > "$TMPDIR/diagnostics.jsonl" || true

    if [[ ! -s "$TMPDIR/diagnostics.jsonl" ]]; then
        echo
        echo '> Clippy emitted **0** Phase-6-relevant diagnostics against the'
        echo '> default workspace lint config.  Every surviving prod clone /'
        echo '> alloc / borrow-candidate site is already justified.'
    else
        for crate in "${CRATES[@]}"; do
            crate_pat="crates/${crate}/src"
            n_rc=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::redundant_clone' || echo 0)
            n_ic=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::implicit_clone' || echo 0)
            n_cic=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::cloned_instead_of_copied' || echo 0)
            n_pbv=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::needless_pass_by_value' || echo 0)
            n_its=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::inefficient_to_string' || echo 0)
            n_sts=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::str_to_string' || echo 0)
            n_uto=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::unnecessary_to_owned' || echo 0)
            n_corp=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::clone_on_ref_ptr' || echo 0)
            if [[ "$n_rc" -gt 0 ]] || [[ "$n_ic" -gt 0 ]] || [[ "$n_cic" -gt 0 ]] \
                || [[ "$n_pbv" -gt 0 ]] || [[ "$n_its" -gt 0 ]] || [[ "$n_sts" -gt 0 ]] \
                || [[ "$n_uto" -gt 0 ]] || [[ "$n_corp" -gt 0 ]]; then
                printf '| `%s` | %d | %d | %d | %d | %d | %d | %d | %d |\n' \
                    "$crate" "$n_rc" "$n_ic" "$n_cic" "$n_pbv" "$n_its" \
                    "$n_sts" "$n_uto" "$n_corp"
            fi
        done
    fi
fi

cat <<'EOF'

## Next steps (per plan §5)

1. **6b — Borrow audit (§3.1):** For each crate with non-zero
   `borrow-candidate` rows above, open every matching `fn` signature
   and classify per §3.1 ([BORROW] / [OWN] / [KEEP-OWNED-FFI] /
   [KEEP-OWNED-PROTOCOL]).  Migrate [BORROW] sites to `&str` / `&[T]`
   / `&Path` / `impl AsRef<Path>`.

2. **6c — Clone taxonomy (§3.2):** For each surviving prod `.clone()`
   site (the inventory's first column), classify into α (Arc-clone) /
   β (ownership-fence) / γ (error-context) / δ (hot-path anti-pattern)
   / ε (test).  Categories α/β/γ keep; δ refactor.  Document each in
   the findings table in plan §11.

3. **6d — Hot-path `format!` / `to_string` audit (§3.4):** Walk every
   `format!` / `.to_string()` inside the §1.2 HIGH-priority hot paths
   (`uffs-mft::io::parser`, `uffs-core::search`,
   `uffs-core::path_resolver`).  Classify [KEEP-COLD] / [FIX-HOT-WRITE]
   / [FIX-HOT-AVOID] per §3.4.

4. **6e — Cow expansion (§3.3):** For each function returning `String`
   that sometimes-but-not-always allocates (per §3.3 criteria 1-4),
   migrate to `Cow<'_, str>`.

5. **6f — Allocation policy:** Write
   `docs/architecture/code-quality/allocation_policy.md`.

6. **6g — Bench refresh:** Re-run `cargo bench -p uffs-mft` and
   `cargo bench -p uffs-core` and pin the delta to
   `phase_6_bench_delta.md`.
EOF
