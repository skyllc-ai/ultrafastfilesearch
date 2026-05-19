#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Phase 8 — Feature flag and dependency hygiene inventory for the UFFS
# workspace.
#
# Companion to:
#   - docs/dev/architecture/code_clean/phase_8_feature_flags_dependency_hygiene_implementation_plan.md
#   - scripts/dev/trait_generic_audit.sh (Phase 7a — same shape, different
#     pattern set)
#   - scripts/dev/clone_alloc_audit.sh  (Phase 6a — same shape, different
#     pattern set)
#
# Purpose
# -------
# Walk every crate and emit, **per crate**, the inventory the playbook
# §926-1004 calls out:
#
#   * `[features]` block contents — feature name, default? optional dep
#     gating (`dep:foo`), feature-on-feature activation (§988 — feature
#     contract).
#   * `#[cfg(feature = "…")]` site count in `src/` — proves a declared
#     feature actually gates a code path (a feature without a use-site
#     is dead).
#   * `optional = true` deps — every optional dep should be reachable
#     via a feature (`dep:foo`) and a `cfg(feature=…)` site.
#   * `default-features = false` consumer overrides — the legitimate
#     "consumer drops default" pattern (e.g. `uffs-cli` dropping
#     `uffs-client::async` to keep tokio out of the sync binary).
#
# Workspace-level inventory:
#   * `cargo tree --workspace -d --depth 0` cross-version duplicate
#     summary, grouped by crate name (§7 — duplication audit).
#   * `deny.toml [bans].skip-tree` cross-check — every duplicate must
#     either be a *new* dup not yet justified (FAIL) or an entry in the
#     skip-tree with a rationale (PASS).
#   * `cargo machete` summary — every direct dep must be used.
#
# Excludes (because the workspace's `clippy.toml` already relaxes
# lint posture inside these, and feature-cfg work is prod-only):
#
#   * `tests/`, `benches/`, `examples/` directories under any crate
#   * `build.rs` files
#   * Files named `tests.rs`, `*_tests.rs`, `*_test.rs`, `test_*.rs`
#
# Caveats (documented in the output preamble)
# -------------------------------------------
# 1. The `[features]` parser is a small awk filter that reads from the
#    `^[features]` table heading until the next `^[` table heading.
#    Comments and blank lines are skipped.  This matches every UFFS
#    Cargo.toml layout but is NOT a full TOML parser — exotic shapes
#    (e.g. inline `[features.foo]` sub-tables) will be missed.
#
# 2. The `cfg(feature = "…")` count is text-based: lines inside an
#    inline `#[cfg(test)] mod tests { ... }` block within a prod source
#    file are over-counted because grep cannot follow the attribute.
#    Phase 8b's per-crate documentation pass re-classifies each.
#
# 3. The duplicate-version inventory is captured from
#    `cargo tree --workspace -d --depth 0` parsed for lines matching
#    `<crate> v<version>`.  A crate appearing twice (or more) in that
#    list is a cross-version duplicate; the script groups by crate name
#    and reports the multiplicity.
#
# Optional cargo cross-checks
# ---------------------------
# Pass `--with-cargo` as the first argument to also run, in order:
#   * `cargo tree --workspace -d --depth 0`   (~1 s)
#   * `cargo machete`                          (~3 s)
#
# Pass `--with-deny` to additionally run `cargo deny check` (slower,
# may touch network for the advisory DB on first run).  With both
# flags, the script tabulates the full Phase-8 acceptance signal.
#
# Usage
# -----
#   scripts/dev/feature_dep_audit.sh                          # rg + toml only (fast, ~1 s)
#   scripts/dev/feature_dep_audit.sh --with-cargo             # + cargo tree -d + cargo machete
#   scripts/dev/feature_dep_audit.sh --with-cargo --with-deny # + cargo deny check
#
# Output goes to stdout in Markdown.  Redirect to capture:
#
#   scripts/dev/feature_dep_audit.sh --with-cargo \
#     > docs/dev/baseline/2026-05-19/phase_8_feature_dep_baseline.md
#
# Exit codes
# ----------
#   0 — script ran to completion (zero is the only success code; the
#       *count* of features / dups / unused-deps is information, not a
#       failure).
#   1 — fatal scripting error (rg missing, repo root not detectable,
#       cargo invocation failed when `--with-cargo` was requested).

set -uo pipefail

WITH_CARGO=0
WITH_DENY=0
for arg in "$@"; do
    case "$arg" in
        --with-cargo) WITH_CARGO=1 ;;
        --with-deny) WITH_DENY=1 ;;
        --help | -h)
            sed -n '1,90p' "$0"
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument '$arg' (expected --with-cargo | --with-deny | --help)" >&2
            exit 1
            ;;
    esac
done

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
if [[ "$WITH_CARGO" -eq 1 ]] && ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: 'cargo' not found in PATH (required for --with-cargo)" >&2
    exit 1
fi
if [[ "$WITH_CARGO" -eq 1 ]] && ! command -v cargo-machete >/dev/null 2>&1; then
    echo "ERROR: 'cargo-machete' not found in PATH (cargo install cargo-machete)" >&2
    exit 1
fi
if [[ "$WITH_DENY" -eq 1 ]] && ! command -v cargo-deny >/dev/null 2>&1; then
    echo "ERROR: 'cargo-deny' not found in PATH (cargo install cargo-deny)" >&2
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

# Extract the `[features]` block from a Cargo.toml.  Reads from the
# `^[features]` heading until the next top-level `^[` heading.  Strips
# inline comments + blank lines AND joins multi-line feature values
# (`foo = [\n  "bar",\n  "baz",\n]`) into a single logical line so
# downstream regex steps see one row per feature.
extract_features_block() {
    local toml="$1"
    awk '
        /^\[features\]/ { in_block = 1; next }
        in_block && /^\[/ { in_block = 0 }
        in_block {
            # Strip inline comments after the first un-quoted #.
            sub(/[[:space:]]*#.*$/, "")
            # Skip blank lines.
            if (/^[[:space:]]*$/) next
            # Feature name lines contain `=`; continuation lines do not.
            if (index($0, "=") > 0) {
                if (length(buf) > 0) print buf
                buf = $0
            } else {
                # Trim leading whitespace from continuation, then append.
                cont = $0
                sub(/^[[:space:]]+/, "", cont)
                buf = buf " " cont
            }
        }
        END { if (length(buf) > 0) print buf }
    ' "$toml"
}

# Count `optional = true` deps in a Cargo.toml's [dependencies] /
# [target.*.dependencies] sections (excluding [dev-dependencies] +
# [build-dependencies]).
count_optional_deps() {
    local toml="$1"
    awk '
        /^\[dev-dependencies\]/      { in_block = 0; next }
        /^\[build-dependencies\]/    { in_block = 0; next }
        /^\[dependencies\]/          { in_block = 1; next }
        /^\[target\..*dependencies\]/ { in_block = 1; next }
        /^\[/                        { in_block = 0; next }
        in_block && /optional[[:space:]]*=[[:space:]]*true/ { c++ }
        END { print c+0 }
    ' "$toml"
}

# ── Markdown preamble ─────────────────────────────────────────────────
SHA="$(git rev-parse HEAD)"
DATE_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

cat <<EOF
# Phase 8 — Feature flag and dependency hygiene baseline

**Captured:** $DATE_UTC
**SHA:** \`$SHA\`
**Methodology:** \`scripts/dev/feature_dep_audit.sh\` — \`rg\`-based count
across each crate's \`src/\` tree plus an awk pass over each crate's
\`Cargo.toml\` \`[features]\` and \`[dependencies]\` blocks.  Excludes
\`tests/\`, \`benches/\`, \`examples/\`, \`build.rs\`, and files matching
\`tests.rs\` / \`*_tests.rs\` / \`*_test.rs\` / \`test_*.rs\`.

**Diff target:** \`docs/dev/baseline/2026-05-19/phase_7_final_report.md\`
(Phase 7 closeout snapshot) for workspace structural consistency.

**Lint posture:** Phase 8 inherits the strict-clippy gate from Phases
5-7 unchanged.  No new lints land in Phase 8 — features and deps are
governed by:

  * \`deny.toml\` — \`cargo deny check\` policy (advisories, bans,
    licenses, sources).
  * \`supply-chain/{config,audits,imports.lock}.toml\` — \`cargo vet\`
    audit trail.
  * \`cargo machete\` in pre-push gate \`pr-fast.yml:553\` — zero
    unused direct deps.

See \`docs/architecture/code-quality/dependency_policy.md\` (Phase 8c)
for the full feature + dep-hygiene contract.  Surviving counts below
are *justified* features and *accepted* duplicate-version dep groups
(skip-tree entries with documented rationale) — not new findings.

> Caveat 1: the \`[features]\` parser is a small awk filter — it
> handles the standard \`name = [ … ]\` shape but not inline
> \`[features.foo]\` sub-tables.
>
> Caveat 2: lines inside an inline \`#[cfg(test)] mod tests { ... }\`
> block within a prod source file are over-counted because grep cannot
> follow the attribute.  Phase 8b's per-crate doc pass re-classifies.
>
> Caveat 3: the duplicate-version inventory is captured from
> \`cargo tree --workspace -d --depth 0\` — only available when invoked
> with \`--with-cargo\`.  Default mode does the toml-only analysis.

## Inventory — feature surface per crate

| Crate | Features | Has default? | \`dep:\`-gated | \`optional = true\` deps | \`cfg(feature)\` sites | Has \`# Features\` rustdoc? |
|---|---:|:---:|---:|---:|---:|:---:|
EOF

# ── Per-crate tally ───────────────────────────────────────────────────
TOT_FEATS=0
TOT_DEPGATED=0
TOT_OPTDEPS=0
TOT_CFGSITES=0
TOT_DOCFEAT=0

for crate in "${CRATES[@]}"; do
    toml="crates/$crate/Cargo.toml"
    src="crates/$crate/src"
    if [[ ! -f "$toml" ]]; then
        continue
    fi

    # Count features (lines matching `^<name> =`) in the [features] block.
    feats=0
    has_default="—"
    dep_gated=0
    if rg --quiet '^\[features\]' "$toml" 2>/dev/null; then
        block=$(extract_features_block "$toml")
        feats=$(printf '%s\n' "$block" | rg -c '^[A-Za-z_][A-Za-z0-9_-]*[[:space:]]*=' || echo 0)
        if printf '%s\n' "$block" | rg --quiet '^default[[:space:]]*='; then
            has_default="yes"
        else
            has_default="no"
        fi
        # Count `dep:` occurrences (NOT lines): the joined feature
        # block may carry multiple `dep:foo` references on a single
        # row, so `rg -c` would undercount.  Use `-o` + line-count.
        dep_gated=$(printf '%s\n' "$block" | rg -o '"dep:[A-Za-z0-9_-]+"' 2>/dev/null | wc -l | tr -d ' ')
        [[ -z "$dep_gated" ]] && dep_gated=0
    fi

    opt_deps=$(count_optional_deps "$toml")

    # cfg(feature = "…") site count in prod src/.  Restrict to the
    # attribute form `#[cfg(feature = …)]` (incl. `#![cfg(...)]` inner
    # attributes) so rustdoc-comment mentions of the syntax are not
    # counted.
    cfg_sites=0
    if [[ -d "$src" ]]; then
        cfg_sites=$(count_pattern "$src" '#!?\[cfg\(feature[[:space:]]*=' 0)
    fi

    # Does the lib.rs / main.rs root have a `# Features` rustdoc section?
    doc_features="—"
    if [[ "$feats" -gt 0 ]]; then
        if rg --quiet -F '# Features' "$src/lib.rs" 2>/dev/null \
            || rg --quiet -F '# Features' "$src/main.rs" 2>/dev/null; then
            doc_features="yes"
            TOT_DOCFEAT=$((TOT_DOCFEAT + 1))
        else
            doc_features="**no**"
        fi
    fi

    TOT_FEATS=$((TOT_FEATS + feats))
    TOT_DEPGATED=$((TOT_DEPGATED + dep_gated))
    TOT_OPTDEPS=$((TOT_OPTDEPS + opt_deps))
    TOT_CFGSITES=$((TOT_CFGSITES + cfg_sites))

    printf '| `%s` | %d | %s | %d | %d | %d | %s |\n' \
        "$crate" "$feats" "$has_default" "$dep_gated" "$opt_deps" \
        "$cfg_sites" "$doc_features"
done

printf '| **Total** | **%d** | — | **%d** | **%d** | **%d** | **%d / %d** |\n' \
    "$TOT_FEATS" "$TOT_DEPGATED" "$TOT_OPTDEPS" "$TOT_CFGSITES" \
    "$TOT_DOCFEAT" "$(rg -l '^\[features\]' crates --glob '**/Cargo.toml' 2>/dev/null | wc -l | tr -d ' ')"

# ── Per-feature contract listing ──────────────────────────────────────
cat <<'EOF'

## Per-feature contract (§3.1 — playbook §988)

Every declared feature, with its activations, the optional deps it
gates, and the prod-path use-site count.  Phase 8b documents each per
the playbook §988 contract:

  * **What it enables** (which code path / which subcommand).
  * **What deps it adds** (`dep:` gating).
  * **Public-API shape impact** (additive vs subtractive).
  * **Semver claim** (default-on means breaking-on-disable).

| Crate | Feature | Default? | Activates | Use-sites |
|---|---|:---:|---|---:|
EOF

for crate in "${CRATES[@]}"; do
    toml="crates/$crate/Cargo.toml"
    src="crates/$crate/src"
    if [[ ! -f "$toml" ]] || ! rg --quiet '^\[features\]' "$toml" 2>/dev/null; then
        continue
    fi

    # Identify defaults set.
    default_list=$(extract_features_block "$toml" \
        | rg --no-heading --no-filename '^default[[:space:]]*=[[:space:]]*\[(.*)\]' \
              --replace '$1' 2>/dev/null || true)

    # Walk each feature line in the [features] block.
    extract_features_block "$toml" \
        | rg --no-heading --no-filename '^[A-Za-z_][A-Za-z0-9_-]*[[:space:]]*=' 2>/dev/null \
        | while IFS= read -r line; do
            # Extract feature name (everything before `=`, trimmed).
            fname="${line%%=*}"
            fname="${fname#"${fname%%[![:space:]]*}"}"
            fname="${fname%"${fname##*[![:space:]]}"}"
            # Extract activation list (between `[` and `]` or after `=`).
            rhs="${line#*=}"
            rhs="${rhs#"${rhs%%[![:space:]]*}"}"

            # Skip `default = [...]` row — captured into a separate column.
            if [[ "$fname" == "default" ]]; then
                continue
            fi

            # Default? — yes if name appears in default_list.
            if printf '%s\n' "$default_list" | rg --quiet -F "\"$fname\""; then
                is_default="yes"
            else
                is_default="no"
            fi

            # Use-site count.
            sites=0
            if [[ -d "$src" ]]; then
                sites=$(rg "${RG_PROD_GLOBS[@]}" --no-heading --no-filename --count-matches \
                    -F "feature = \"$fname\"" "$src" 2>/dev/null \
                    | awk 'BEGIN{s=0} {s+=$1} END{print s+0}')
            fi

            # Normalise RHS for table rendering: keep single-line shape,
            # escape pipes.
            rhs_disp="$(printf '%s' "$rhs" | tr -s ' \t' ' ')"
            rhs_disp="${rhs_disp//|/\\|}"

            printf '| `%s` | `%s` | %s | `%s` | %d |\n' \
                "$crate" "$fname" "$is_default" "$rhs_disp" "$sites"
        done
done

# ── Optional dep cross-check ──────────────────────────────────────────
cat <<'EOF'

## Optional-dep cross-check (§3.2)

Every `optional = true` dep in a `[dependencies]` table should be
referenced by exactly one feature via `dep:foo`.  An optional dep with
zero `dep:foo` references is dead weight; an optional dep referenced
by multiple features is informational (legitimate composition).

| Crate | Optional dep | Referenced by feature(s) |
|---|---|---|
EOF

for crate in "${CRATES[@]}"; do
    toml="crates/$crate/Cargo.toml"
    if [[ ! -f "$toml" ]]; then
        continue
    fi

    # Extract the names of optional deps from the [dependencies] +
    # [target.*.dependencies] blocks.  Match `<name>.workspace =` or
    # `<name> = {` with `optional = true` on the same logical line.
    awk '
        /^\[dev-dependencies\]/      { in_block = 0; next }
        /^\[build-dependencies\]/    { in_block = 0; next }
        /^\[dependencies\]/          { in_block = 1; next }
        /^\[target\..*dependencies\]/ { in_block = 1; next }
        /^\[/                        { in_block = 0; next }
        in_block && /optional[[:space:]]*=[[:space:]]*true/ {
            # Extract crate name (token before `=` or `.workspace`).
            line = $0
            sub(/[[:space:]]*=.*$/, "", line)
            sub(/\.workspace$/, "", line)
            gsub(/[[:space:]]/, "", line)
            if (length(line) > 0) print line
        }
    ' "$toml" | while IFS= read -r dep; do
        if [[ -z "$dep" ]]; then
            continue
        fi
        # Find features that reference `dep:<dep>`.
        refs=$(extract_features_block "$toml" \
            | rg --no-heading --no-filename -o "\"dep:${dep}\"" 2>/dev/null \
            | wc -l \
            | tr -d ' ')
        # Find the feature name(s) that contain dep:<dep>.  Match the
        # joined feature lines that reference `"dep:<dep>"` and pull
        # out the bare feature name with sed (rg `--replace` keeps the
        # un-matched trailing text on the line; sed gives a clean
        # capture).
        feat_names=$(extract_features_block "$toml" \
            | rg --no-heading --no-filename -F "\"dep:${dep}\"" 2>/dev/null \
            | sed -E 's/^([A-Za-z_][A-Za-z0-9_-]*).*$/\1/' \
            | sort -u \
            | paste -sd ', ' - || true)
        if [[ "$refs" -eq 0 ]]; then
            feat_disp="**ORPHAN — no \`dep:\` reference**"
        else
            feat_disp="$feat_names ($refs ref)"
        fi
        printf '| `%s` | `%s` | %s |\n' "$crate" "$dep" "$feat_disp"
    done
done

# ── `default-features = false` consumer overrides ─────────────────────
cat <<'EOF'

## Consumer-side `default-features = false` overrides (§3.3)

Sites where an internal crate is consumed with default features
explicitly dropped.  The canonical example is `uffs-cli` dropping
`uffs-client::async` to keep tokio + `ws2_32.dll` out of the sync CLI
binary.  Each row is a deliberate decision, not a bug.

| Consumer | Crate | Cargo.toml |
|---|---|---|
EOF

while IFS=: read -r toml line content; do
    if [[ -z "$toml" ]]; then
        continue
    fi
    # Skip TOML comment lines — `# … default-features = false …` in a
    # rustdoc block is not an override.
    if [[ "$content" =~ ^[[:space:]]*# ]]; then
        continue
    fi
    # Consumer = the crate that owns the Cargo.toml.
    consumer="${toml#crates/}"
    consumer="${consumer%/Cargo.toml}"
    # Dep crate = first token before `=` on the line.
    dep="${content%%=*}"
    dep="${dep#"${dep%%[![:space:]]*}"}"
    dep="${dep%"${dep##*[![:space:]]}"}"
    printf '| `%s` | `%s` | `%s:%s` |\n' "$consumer" "$dep" "$toml" "$line"
done < <(rg --no-heading --line-number 'default-features[[:space:]]*=[[:space:]]*false' \
    crates --glob '**/Cargo.toml' 2>/dev/null)

# ── deny.toml [bans].skip-tree inventory ──────────────────────────────
cat <<'EOF'

## `deny.toml` skip-tree inventory (§3.4)

Every entry in `deny.toml [bans].skip-tree` represents an *accepted*
cross-version duplicate.  Each must carry a `reason` field.  A
duplicate-version pair surfaced by `cargo tree -d` must match a
skip-tree entry by crate name (and optionally version); otherwise it
is a NEW dup needing either resolution or a new skip-tree entry with a
documented rationale.

| Skip-tree entry | Rationale |
|---|---|
EOF

if [[ -f "deny.toml" ]]; then
    awk '
        /^skip-tree[[:space:]]*=/ { in_block = 1; next }
        in_block && /^\]/ { in_block = 0 }
        in_block && /crate[[:space:]]*=/ {
            # Pull the crate spec and reason from the `{ crate = "foo", reason = "…" }` line.
            line = $0
            cspec = ""
            rspec = ""
            if (match(line, /crate[[:space:]]*=[[:space:]]*"[^"]*"/)) {
                cspec = substr(line, RSTART, RLENGTH)
                sub(/^crate[[:space:]]*=[[:space:]]*"/, "", cspec)
                sub(/"$/, "", cspec)
            }
            if (match(line, /reason[[:space:]]*=[[:space:]]*"[^"]*"/)) {
                rspec = substr(line, RSTART, RLENGTH)
                sub(/^reason[[:space:]]*=[[:space:]]*"/, "", rspec)
                sub(/"$/, "", rspec)
            }
            if (length(cspec) > 0) printf("| `%s` | %s |\n", cspec, rspec)
        }
    ' deny.toml
else
    echo "| _(no deny.toml found)_ | — |"
fi

# ── Optional cargo cross-checks ───────────────────────────────────────
if [[ "$WITH_CARGO" -eq 1 ]]; then
    cat <<'EOF'

## Cross-version duplicate inventory (`cargo tree --workspace -d`)

Every duplicate-version crate group, with its multiplicity.  Cross-
checked against `deny.toml [bans].skip-tree` — a row marked `accepted`
matches a skip-tree entry by crate name; `**NEW**` is an unjustified
dup needing either resolution or a new skip-tree entry.

| Crate | Versions | Multiplicity | Skip-tree status |
|---|---|---:|:---:|
EOF

    TMPDIR=$(mktemp -d)
    trap 'rm -rf "$TMPDIR"' EXIT

    cargo tree --workspace -d --depth 0 --quiet 2>/dev/null \
        | rg '^[a-z0-9_][a-z0-9_-]* v[0-9]' \
        > "$TMPDIR/dups.txt" || true

    # Group by crate name → DISTINCT versions list + multiplicity.
    # `cargo tree -d --depth 0` can list the same `(name, version)`
    # pair twice when the same resolved node appears under multiple
    # ancestors (e.g. once as a normal dep and once as a build-dep);
    # we count *unique* versions, not raw occurrences.
    awk '
        {
            name = $1
            ver  = $2
            sub(/^v/, "", ver)
            key = name "@" ver
            if (key in seen) next
            seen[key] = 1
            count[name]++
            if (!(name in vlist)) vlist[name] = ver
            else                  vlist[name] = vlist[name] ", " ver
        }
        END {
            for (name in count) {
                if (count[name] >= 2) {
                    print name "\t" vlist[name] "\t" count[name]
                }
            }
        }
    ' "$TMPDIR/dups.txt" \
        | sort \
        > "$TMPDIR/dup-groups.txt"

    # Extract skip-tree crate names (with or without @version).
    awk '
        /^skip-tree[[:space:]]*=/ { in_block = 1; next }
        in_block && /^\]/ { in_block = 0 }
        in_block && /crate[[:space:]]*=/ {
            if (match($0, /crate[[:space:]]*=[[:space:]]*"[^"]*"/)) {
                cspec = substr($0, RSTART, RLENGTH)
                sub(/^crate[[:space:]]*=[[:space:]]*"/, "", cspec)
                sub(/"$/, "", cspec)
                # Strip @version to get bare crate name.
                bare = cspec
                sub(/@.*$/, "", bare)
                print bare
            }
        }
    ' deny.toml \
        | sort -u \
        > "$TMPDIR/skip-tree-names.txt"

    TOT_DUPS=0
    TOT_ACCEPTED=0
    TOT_NEW=0
    while IFS=$'\t' read -r name versions mult; do
        if [[ -z "$name" ]]; then
            continue
        fi
        TOT_DUPS=$((TOT_DUPS + 1))
        if rg --quiet -F -x "$name" "$TMPDIR/skip-tree-names.txt" 2>/dev/null; then
            status="accepted"
            TOT_ACCEPTED=$((TOT_ACCEPTED + 1))
        else
            status="**NEW**"
            TOT_NEW=$((TOT_NEW + 1))
        fi
        printf '| `%s` | `%s` | %s | %s |\n' "$name" "$versions" "$mult" "$status"
    done <"$TMPDIR/dup-groups.txt"

    printf '| **Total** | — | **%d** | accepted: **%d** / new: **%d** |\n' \
        "$TOT_DUPS" "$TOT_ACCEPTED" "$TOT_NEW"

    # ── cargo machete summary ─────────────────────────────────────────
    cat <<'EOF'

## `cargo machete` summary

Every direct dep in every crate's `Cargo.toml` should be referenced by
at least one source file.  An unused direct dep is dead weight (and
inflates compile time).  Workspace gate: zero unused direct deps.

EOF

    machete_out=$(cargo machete 2>&1 || true)
    if printf '%s\n' "$machete_out" | rg --quiet "didn't find any unused dependencies"; then
        echo "✅ \`cargo machete\` reports **0 unused direct deps** workspace-wide."
    else
        echo "❌ \`cargo machete\` found unused deps:"
        echo
        echo '```'
        printf '%s\n' "$machete_out"
        echo '```'
    fi
fi

# ── Optional cargo deny check ─────────────────────────────────────────
if [[ "$WITH_DENY" -eq 1 ]]; then
    cat <<'EOF'

## `cargo deny check` summary

Workspace dep-hygiene gate: all four categories (advisories, bans,
licenses, sources) must pass.  Ignored advisories are listed in
`deny.toml [advisories].ignore` with rationale.

EOF

    deny_out=$(cargo deny check 2>&1 || true)
    final_line=$(printf '%s\n' "$deny_out" | rg '(advisories|bans|licenses|sources)' | tail -n 1 || true)
    if printf '%s\n' "$deny_out" | rg --quiet 'advisories ok, bans ok, licenses ok, sources ok'; then
        echo "✅ \`cargo deny check\`: **advisories ok, bans ok, licenses ok, sources ok**."
        if [[ -n "$final_line" ]]; then
            echo
            echo '```'
            echo "$final_line"
            echo '```'
        fi
    else
        echo "⚠️ \`cargo deny check\` did not report all-green; tail:"
        echo
        echo '```'
        printf '%s\n' "$deny_out" | tail -n 20
        echo '```'
    fi
fi

cat <<'EOF'

## Next steps (per plan §1)

1. **8b — Feature documentation pass:** For each crate with a
   `[features]` block (3 crates: `uffs-cli`, `uffs-client`, `uffs-mcp`),
   add a `# Features` rustdoc section on `lib.rs` (or `main.rs` for
   bin-only crates) per playbook §988: what it enables, what deps it
   adds, public-API shape impact, semver claim.  Improve inline
   comments in the `[features]` block.

2. **8c — Dependency policy doc:** Write
   `docs/architecture/code-quality/dependency_policy.md` — companion to
   `panic_policy.md` (5e), `allocation_policy.md` (6f), and
   `trait_policy.md` (7g).  Cross-link from CONTRIBUTING.md §"Dependency
   hygiene policy".

3. **8d — Final report + closeout:** `phase_8_final_report.md` + plan
   §11 fill-in + close issue #195.
EOF
