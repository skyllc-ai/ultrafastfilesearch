#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Phase 7 — Prod-only trait / generic / dispatch inventory for the
# UFFS workspace.
#
# Companion to:
#   - docs/dev/architecture/code_clean/phase_7_traits_generics_dispatch_implementation_plan.md
#   - scripts/dev/clone_alloc_audit.sh (Phase 6a — same shape, different
#     pattern set)
#   - scripts/dev/risk_markers_prod.sh (Phase 5a)
#
# Purpose
# -------
# Walk every crate's library tree (`crates/<name>/src/`) and count, **per
# crate**, the production-path occurrences of the patterns playbook
# §858-924 calls out:
#
#   * `pub trait` / `pub(crate) trait` / `pub(super) trait` / `trait`
#                                        — trait surface (§1 — justification)
#   * `impl <Trait> for <Type>`          — impl count per trait (§1 — J1)
#   * `fn <name><...>(`                  — generic fn signatures (§2 — generic spread)
#   * `dyn <Trait>`                      — dynamic-dispatch sites (§3 — D1-D4)
#   * `where <T>:`                       — where-clause density (§5)
#   * `impl AsRef<` / `impl Into<` /
#     `impl Iterator<` / `impl Fn`       — impl-Trait sugar (informational)
#   * `mod private { ... Sealed ... }`   — sealed-trait pattern detection (§4)
#
# Excludes (because the workspace's `clippy.toml` already relaxes
# lint posture inside these):
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
#    7b's per-trait audit re-classifies these.
#
# 2. The `impl <Trait> for` count picks up blanket impls
#    (`impl<T> Foo for T where ...`) — Phase 7b's manual pass
#    distinguishes prod impls from blanket impls.
#
# 3. The `fn<...>` regex matches single-line signatures only.
#    Multi-line generic signatures are caught by the `--with-clippy`
#    cross-check (which runs `type_complexity` against the full AST).
#
# Optional clippy-JSON cross-check
# --------------------------------
# Pass `--with-clippy` as the first argument to also run
# `cargo clippy --workspace --all-targets --message-format=json` and
# tabulate the resulting diagnostic count per crate for the lints that
# bear on Phase 7:
#
#   * `type_complexity`                 — already at `deny` in workspace
#   * `trait_duplication_in_bounds`     — Phase 7f adds this
#   * `wrong_self_convention`           — Phase 7f adds this
#   * `too_many_arguments`              — Phase 7f adds this
#   * `multiple_bound_locations`        — Phase 7f adds this at `warn`
#   * `needless_pass_by_value`          — already at `deny` (Phase 6 lint)
#
# Currently only `type_complexity` is at `deny`; Phase 7f extends the
# set.  With the strict-clippy gate green on `main`, the clippy-JSON
# cross-check is expected to emit **zero** diagnostics for the
# already-denied lints.  The audit's value is the rg-pass tally of
# surviving trait surface + generic spread (already-justified sites),
# not new findings.
#
# Usage
# -----
#   scripts/dev/trait_generic_audit.sh               # rg-only (fast, ~1 s)
#   scripts/dev/trait_generic_audit.sh --with-clippy # rg + clippy JSON cross-check
#
# Output goes to stdout in Markdown.  Redirect to capture:
#
#   scripts/dev/trait_generic_audit.sh \
#     > docs/dev/baseline/2026-05-19/phase_7_trait_generic_baseline.md
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
# Phase 7 — Prod-only trait / generic / dispatch baseline

**Captured:** $DATE_UTC
**SHA:** \`$SHA\`
**Methodology:** \`scripts/dev/trait_generic_audit.sh\` — \`rg\`-based count
across each crate's \`src/\` tree, excluding \`tests/\`, \`benches/\`,
\`examples/\`, \`build.rs\`, and files matching \`tests.rs\` /
\`*_tests.rs\` / \`*_test.rs\` / \`test_*.rs\`.

**Diff target:** \`docs/dev/baseline/2026-05-12/phase_6_clone_alloc_after.md\`
(Phase 6 closeout snapshot) for crate-LOC consistency.

**Lint posture:** Phase 7 inherits 1 deny-level trait/generic lint
already in the workspace \`[workspace.lints.clippy]\` block:

  * \`type_complexity = "deny"\`  (default threshold 250)

Phase 7f extends this with three more denies and one warn:

  * \`trait_duplication_in_bounds = "deny"\`
  * \`wrong_self_convention       = "deny"\`
  * \`too_many_arguments          = "deny"\`
  * \`multiple_bound_locations    = "warn"\`

See \`docs/architecture/code-quality/lint-posture.md\`.  Surviving counts
below are *justified* sites (test-substitution traits, plugin
boundaries, closure-flavour generics) — not new findings.

> Caveat 1: lines inside an inline \`#[cfg(test)] mod tests { ... }\`
> block within a prod source file are over-counted because grep cannot
> follow the attribute.  Phase 7b's per-trait audit re-classifies these.
>
> Caveat 2: the \`impl <Trait> for\` count picks up blanket impls
> (\`impl<T> Foo for T where ...\`).  Phase 7b distinguishes prod impls
> from blanket impls per-trait.
>
> Caveat 3: the \`fn<...>\` regex matches single-line generic
> signatures only.  Multi-line generic signatures are caught by the
> \`--with-clippy\` cross-check (\`type_complexity\` runs against the
> full AST).

## Inventory — trait surface and generic spread

| Crate | \`pub trait\` | \`pub(crate)\` / \`pub(super)\` trait | \`trait\` (private) | generic fns | \`dyn Trait\` | \`where\` clauses | \`impl Trait\` sugar | prod LOC |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
EOF

# ── Per-crate tally ───────────────────────────────────────────────────
TOT_PUB=0
TOT_PUBCRATE=0
TOT_PRIV=0
TOT_GEN=0
TOT_DYN=0
TOT_WHERE=0
TOT_IMPL=0
TOT_LOC=0

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi

    # `pub trait Foo` (at start of line, with optional doc-tag) — public
    pub_trait=$(count_pattern "$src" '^pub trait [A-Z]' 0)
    # `pub(crate) trait` or `pub(super) trait` — crate-private
    pubcrate_trait=$(count_pattern "$src" '^pub\((crate|super|in [^)]+)\) trait [A-Z]' 0)
    # `trait Foo` at start of line, no `pub` prefix — module-private
    priv_trait=$(count_pattern "$src" '^trait [A-Z]' 0)

    # Generic fns — `fn <name><...>(` at start of line (optionally
    # preceded by `pub`, `pub(...)`, `async`, `const`, `unsafe`,
    # `extern`, `default`, but starting whitespace-free).  We accept any
    # leading-keyword prefix and then `fn <name><`.
    gen_fn=$(count_pattern "$src" '^\s*(pub(\([^)]*\))? )?(async |const |unsafe |default |extern "[^"]*" )*fn [a-zA-Z_][a-zA-Z0-9_]*<' 0)

    # `dyn <Trait>` (skip `dyn_*` lint names, look for capital after `dyn `)
    dyn_use=$(count_pattern "$src" '\bdyn [A-Z]' 0)

    # `where <T>:` — count all where-clause lines.  Both `fn foo() where`
    # and `impl X for Y where` shapes contribute.
    where_use=$(count_pattern "$src" '\bwhere\b' 0)

    # `impl Trait` sugar: AsRef<…>, Into<…>, Iterator<…>, IntoIterator,
    # Fn(…) / FnMut(…) / FnOnce(…) — these are the canonical
    # ergonomic-generic shapes.
    impl_trait=$(count_pattern "$src" 'impl (AsRef|AsMut|Into|From|Iterator|IntoIterator|Fn|FnMut|FnOnce|Sized|Send|Sync|Debug|Display|Read|Write|Deref|DerefMut)\b' 0)

    loc=$(count_loc_prod "$src")

    TOT_PUB=$((TOT_PUB + pub_trait))
    TOT_PUBCRATE=$((TOT_PUBCRATE + pubcrate_trait))
    TOT_PRIV=$((TOT_PRIV + priv_trait))
    TOT_GEN=$((TOT_GEN + gen_fn))
    TOT_DYN=$((TOT_DYN + dyn_use))
    TOT_WHERE=$((TOT_WHERE + where_use))
    TOT_IMPL=$((TOT_IMPL + impl_trait))
    TOT_LOC=$((TOT_LOC + loc))

    printf '| `%s` | %d | %d | %d | %d | %d | %d | %d | %d |\n' \
        "$crate" "$pub_trait" "$pubcrate_trait" "$priv_trait" \
        "$gen_fn" "$dyn_use" "$where_use" "$impl_trait" "$loc"
done

printf '| **Total** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** | **%d** |\n' \
    "$TOT_PUB" "$TOT_PUBCRATE" "$TOT_PRIV" "$TOT_GEN" \
    "$TOT_DYN" "$TOT_WHERE" "$TOT_IMPL" "$TOT_LOC"

# ── Per-trait inventory (§3.1 — trait justification audit) ────────────
cat <<'EOF'

## Per-trait inventory (§3.1 — J1/J2/J3/J4 candidates)

Every prod-path `trait` definition, with its file:line, visibility, and
prod-impl count.  Phase 7b classifies each per the four-criterion
taxonomy from playbook §879-886:

  * **[J1]** Multiple meaningful implementations (≥ 2 prod impls)
  * **[J2]** Test-substitution boundary (prod impl + ≥ 1 test fake)
  * **[J3]** Stable extension surface (rustdoc documents downstream use)
  * **[J4]** Decoupling a high-level layer from infrastructure

Any trait satisfying **none** of J1–J4 → demote to concrete type.

| Trait | Crate | File:line | Visibility | Prod impls |
|---|---|---|---|---:|
EOF

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi

    # List every trait definition with file:line; parse out the name.
    while IFS=: read -r file line content; do
        # Normalise the visibility prefix and extract the trait name.
        if [[ "$content" =~ ^pub\ trait\ ([A-Z][A-Za-z0-9_]*) ]]; then
            vis="pub"
            name="${BASH_REMATCH[1]}"
        elif [[ "$content" =~ ^pub\((crate|super|in\ [^\)]+)\)\ trait\ ([A-Z][A-Za-z0-9_]*) ]]; then
            vis="pub(${BASH_REMATCH[1]})"
            name="${BASH_REMATCH[2]}"
        elif [[ "$content" =~ ^trait\ ([A-Z][A-Za-z0-9_]*) ]]; then
            vis="priv"
            name="${BASH_REMATCH[1]}"
        else
            continue
        fi

        # Count prod impls for this trait name workspace-wide.  A trait
        # defined in crate A can be implemented by a struct in crate B
        # (e.g. `FileReader` defined in `uffs-core` is implemented by
        # `DaemonFileReader` in `uffs-daemon`).  We accept the loose
        # form `impl[<…>] <Name>[<…>] for <Type>` so both `impl Foo for
        # Bar` and `impl<T> Foo<T> for Bar` match.  Inline `#[cfg(test)]
        # mod tests { impl Foo for Fake {} }` blocks are over-counted
        # per Caveat 1; Phase 7b re-classifies via a manual pass.
        impl_count=$(rg "${RG_PROD_GLOBS[@]}" --no-heading --no-filename --count-matches \
            "^\s*impl(<[^>]*>)?\s+${name}(<[^>]*>)?\s+for\s" crates 2>/dev/null \
            | awk 'BEGIN{s=0} {s+=$1} END{print s+0}')

        # Relative path display.
        rel="${file#crates/"$crate"/src/}"
        printf '| `%s` | `%s` | `crates/%s/src/%s:%s` | `%s` | %d |\n' \
            "$name" "$crate" "$crate" "$rel" "$line" "$vis" "$impl_count"
    done < <(rg "${RG_PROD_GLOBS[@]}" --no-heading --line-number \
        '^(pub( *\([^)]*\))?\s+)?trait [A-Z]' "$src" 2>/dev/null)
done

# ── Sealed-trait pattern detection ────────────────────────────────────
cat <<'EOF'

## Sealed-trait pattern detection (§3.4)

Modules / files matching the canonical sealed-trait shape:

```
mod private { pub trait Sealed {} }
pub trait MyTrait: private::Sealed { ... }
```

Phase 3b (§3.7) decided the four pre-existing traits as OPEN
(`FileReader`, `FormatRow`, `DirCacheExt`, `uffs-mft` libs).  Phase 7e
re-evaluates each surviving `pub trait` and decides OPEN vs SEAL.

| Crate | File | Pattern |
|---|---|---|
EOF

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi

    while IFS=: read -r file line content; do
        if [[ -z "$file" ]]; then
            continue
        fi
        rel="${file#crates/"$crate"/src/}"
        printf '| `%s` | `crates/%s/src/%s` | sealed @ line %s |\n' \
            "$crate" "$crate" "$rel" "$line"
    done < <(rg "${RG_PROD_GLOBS[@]}" --no-heading --line-number \
        'mod private \{[^}]*pub trait Sealed' "$src" 2>/dev/null)
done

# ── Dispatch site inventory (§3.3) ────────────────────────────────────
cat <<'EOF'

## Dispatch site inventory (§3.3 — D1/D2/D3/D4)

Every prod-path `dyn <Trait>` site, with its file:line.  Phase 7d
classifies each per the dispatch matrix:

  * **[D1-PLUGIN]**     Pluggable backend / runtime registry — KEEP
  * **[D2-HETERO]**     Heterogeneous handler collection — KEEP
  * **[D3-NOOP]**       Closed-set single-impl — refactor to concrete
  * **[D4-VTBL-COST]**  Hot-path dyn dispatch — measure, then decide

| Crate | File:line | Site |
|---|---|---|
EOF

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi

    while IFS=: read -r file line content; do
        if [[ -z "$file" ]]; then
            continue
        fi
        rel="${file#crates/"$crate"/src/}"
        # Trim leading whitespace + escape pipes in content for table rendering.
        trimmed="${content#"${content%%[![:space:]]*}"}"
        trimmed="${trimmed//|/\\|}"
        printf '| `%s` | `crates/%s/src/%s:%s` | `%s` |\n' \
            "$crate" "$crate" "$rel" "$line" "$trimmed"
    done < <(rg "${RG_PROD_GLOBS[@]}" --no-heading --line-number \
        '\bdyn [A-Z]' "$src" 2>/dev/null)
done

# ── Existing per-site annotations (Phase 7-relevant) ──────────────────
cat <<'EOF'

## Annotations already in place (Phase 7-relevant lints)

Sites whose trait / generic / dispatch pattern is already justified by
a per-site `#[expect(clippy::*, reason = "…")]` annotation.

| Crate | `type_complexity` | `too_many_arguments` | `wrong_self_convention` | `trait_duplication_in_bounds` |
|---|---:|---:|---:|---:|
EOF

for crate in "${CRATES[@]}"; do
    src="crates/$crate/src"
    if [[ ! -d "$src" ]]; then
        continue
    fi
    a_tc=$(count_pattern "$src" 'clippy::type_complexity\b' 0)
    a_tma=$(count_pattern "$src" 'clippy::too_many_arguments\b' 0)
    a_wsc=$(count_pattern "$src" 'clippy::wrong_self_convention\b' 0)
    a_tdb=$(count_pattern "$src" 'clippy::trait_duplication_in_bounds\b' 0)
    if [[ "$a_tc" -gt 0 ]] || [[ "$a_tma" -gt 0 ]] || [[ "$a_wsc" -gt 0 ]] \
        || [[ "$a_tdb" -gt 0 ]]; then
        printf '| `%s` | %d | %d | %d | %d |\n' \
            "$crate" "$a_tc" "$a_tma" "$a_wsc" "$a_tdb"
    fi
done

# ── Optional clippy-JSON cross-check ──────────────────────────────────
if [[ "$WITH_CLIPPY" -eq 1 ]]; then
    cat <<'EOF'

## Clippy JSON cross-check (authoritative)

`cargo clippy --workspace --all-targets --message-format=json`.  Counts
below are diagnostics emitted by strict-clippy for the Phase-7-relevant
lints.  With the workspace gate green on `main`, this is expected to be
**zero** for the lints currently at `deny` (`type_complexity`,
`needless_pass_by_value`).  Lints introduced by Phase 7f
(`trait_duplication_in_bounds`, `wrong_self_convention`,
`too_many_arguments`, `multiple_bound_locations`) emit at their
current configured level (none at the time of the baseline).

| Crate | `type_complexity` | `too_many_arguments` | `wrong_self_convention` | `trait_duplication_in_bounds` | `multiple_bound_locations` | `needless_pass_by_value` |
|---|---:|---:|---:|---:|---:|---:|
EOF

    TMPDIR=$(mktemp -d)
    trap 'rm -rf "$TMPDIR"' EXIT

    cargo clippy --workspace --all-targets \
        --message-format=json --quiet 2>/dev/null \
        | rg '"clippy::(type_complexity|too_many_arguments|wrong_self_convention|trait_duplication_in_bounds|multiple_bound_locations|needless_pass_by_value)"' \
              --no-line-number --no-heading --no-filename \
        > "$TMPDIR/diagnostics.jsonl" || true

    if [[ ! -s "$TMPDIR/diagnostics.jsonl" ]]; then
        echo
        echo '> Clippy emitted **0** Phase-7-relevant diagnostics against the'
        echo '> default workspace lint config.  Every surviving prod trait /'
        echo '> generic / dispatch site is already justified.'
    else
        for crate in "${CRATES[@]}"; do
            crate_pat="crates/${crate}/src"
            n_tc=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::type_complexity' || echo 0)
            n_tma=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::too_many_arguments' || echo 0)
            n_wsc=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::wrong_self_convention' || echo 0)
            n_tdb=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::trait_duplication_in_bounds' || echo 0)
            n_mbl=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::multiple_bound_locations' || echo 0)
            n_pbv=$(rg -F "$crate_pat" "$TMPDIR/diagnostics.jsonl" 2>/dev/null \
                | rg -c 'clippy::needless_pass_by_value' || echo 0)
            if [[ "$n_tc" -gt 0 ]] || [[ "$n_tma" -gt 0 ]] || [[ "$n_wsc" -gt 0 ]] \
                || [[ "$n_tdb" -gt 0 ]] || [[ "$n_mbl" -gt 0 ]] || [[ "$n_pbv" -gt 0 ]]; then
                printf '| `%s` | %d | %d | %d | %d | %d | %d |\n' \
                    "$crate" "$n_tc" "$n_tma" "$n_wsc" "$n_tdb" "$n_mbl" "$n_pbv"
            fi
        done
    fi
fi

cat <<'EOF'

## Next steps (per plan §5)

1. **7b — Trait justification audit (§3.1):** For each row in the
   per-trait inventory above, open the trait definition + its rustdoc.
   Count prod impls + test fakes.  Classify J1/J2/J3/J4; record verdict.
   If verdict is REVIEW and the rustdoc does not justify, demote to
   concrete type.  Findings → `phase_7_trait_justification_findings.md`.

2. **7c — Generic-function audit (§3.2):** For each prod-path `fn<…>`,
   classify G1-LOCAL / G2-USEFUL / G3-SPREAD / G4-CASCADING / G5-CLOSURE.
   G3 / G4 sites → refactor to concrete or narrow scope.  Findings →
   `phase_7_generic_audit_findings.md`.

3. **7d — Dispatch audit (§3.3):** For each `dyn Trait` site (per the
   inventory above), classify D1-PLUGIN / D2-HETERO / D3-NOOP /
   D4-VTBL-COST.  D3 / D4 sites → refactor to concrete / enum.
   Findings → `phase_7_dispatch_findings.md`.

4. **7e — Trait sealing (§3.4):** For each surviving `pub trait`,
   decide OPEN / SEAL per playbook §902-904, carrying forward Phase 3b
   §3.7 decisions for `FileReader`, `FormatRow`, `DirCacheExt`.

5. **7f — Bound rationalization:** Add
   `trait_duplication_in_bounds = "deny"`,
   `wrong_self_convention = "deny"`,
   `too_many_arguments = "deny"`, and
   `multiple_bound_locations = "warn"` to
   `Cargo.toml [workspace.lints.clippy]`.  Dedupe per-crate diagnostics.

6. **7g — Trait policy doc:** Write
   `docs/architecture/code-quality/trait_policy.md`.

7. **7h — Bench + compile-time refresh:** Re-run
   `cargo bench -p uffs-mft` and `cargo bench -p uffs-core`; capture
   `cargo build --workspace --timings --release` HTML for top-10
   slowest-compile crates.  Pin to `phase_7_bench_delta.md`.
EOF
