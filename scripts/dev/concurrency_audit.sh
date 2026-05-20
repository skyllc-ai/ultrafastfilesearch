#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Phase 10 — Async, concurrency, and shared state baseline for the UFFS
# workspace.
#
# Companion to:
#   - docs/dev/architecture/code_clean/phase_10_async_concurrency_shared_state_implementation_plan.md
#   - scripts/dev/build_codegen_audit.sh    (Phase 9a — same shape, different patterns)
#   - scripts/dev/feature_dep_audit.sh      (Phase 8a — same shape)
#   - scripts/dev/trait_generic_audit.sh    (Phase 7a — same shape)
#   - scripts/dev/clone_alloc_audit.sh      (Phase 6a — same shape)
#
# Purpose
# -------
# Walk every workspace member and emit, **per crate**, the inventory the
# playbook §1082-1146 calls out, covering all 7 audit dimensions from
# the Phase 10 plan §7:
#
#   1. `tokio::spawn` / detached tasks — every call-site + classification
#      (bound to a `JoinHandle` / bare-expression-statement / inside a
#      `JoinSet` / inside a named spawner function).
#   2. Locks held across `.await` — every `.read().await` / `.write().await`
#      / `.lock().await` site, listed for hand-audit (no auto-classifier
#      can read the surrounding control flow correctly).
#   3. Blocking IO inside async tasks — files that contain BOTH `async fn`
#      AND `std::fs::*` / `std::thread::sleep`; hand-audit confirms each
#      `std::fs::*` is either (a) inside a `spawn_blocking` / `block_in_place`,
#      or (b) inside a sync helper called only from sync contexts.
#   4. `Arc<Mutex<…>>` nesting — every double/triple-Arc-Mutex pattern.
#   5. Missing timeouts — every `.connect().await` / `.read_exact().await`
#      / `.write_all().await` / `.recv().await` / `.accept().await` site
#      not preceded by `tokio::time::timeout(` within 20 lines above
#      (heuristic — hand-audit confirms).
#   6. Missing cancellation handling — every `tokio::spawn(` that does
#      NOT contain `select!` / `CancellationToken` / shutdown-channel in
#      its closure body (heuristic — hand-audit confirms).
#   7. Unbounded channels — every `unbounded_channel(…)` / `broadcast::channel(…)`
#      call-site, listed for cross-check against the policy registry.
#
# Workspace-level inventory:
#   * Per-crate concurrency surface area table (async fn / spawn / spawn_blocking
#     / std::sync::* / tokio::sync::* / Arc<Mutex> / channels / timeouts).
#   * Total tokio::spawn count + per-site list.
#   * Total async-lock count + per-site list.
#   * Lock-across-await candidate set.
#   * Channel inventory (bounded / unbounded / oneshot / broadcast / watch).
#   * Timeout coverage (sites with `tokio::time::timeout` enclosing).
#   * Blocking-IO-in-async candidate files.
#   * Cancellation / shutdown infrastructure inventory.
#
# Excludes (because concurrency hazards in test harnesses use different
# patterns — test code is allowed to hold locks across awaits when
# stress-testing, for example):
#
#   * `tests/`, `benches/`, `examples/` directories under any crate.
#   * Files named `tests.rs`, `*_tests.rs`, `*_test.rs`, `test_*.rs`.
#
# Caveats (documented in the output preamble)
# -------------------------------------------
# 1. Lock-across-await detection is a literal `.read().await` /
#    `.write().await` / `.lock().await` match.  A site like
#    `let g = self.lock(); g.foo(); other.await; drop(g);` will NOT be
#    detected by the literal regex but IS a hazard.  Phase 10b's
#    hand-audit reads each candidate's surrounding context.
#
# 2. Blocking-IO-in-async detection emits a candidate FILE list (files
#    containing BOTH `async fn` AND `std::fs::*`).  It does NOT prove
#    the `std::fs::*` is reachable from the `async fn`; Phase 10f's
#    hand-audit confirms each.
#
# 3. Missing-timeout detection uses a `rg --context-before=20` heuristic.
#    Some sites legitimately have no timeout (e.g. a 24-h periodic
#    heartbeat that explicitly waits forever).  Phase 10e's hand-audit
#    confirms each.
#
# 4. Missing-cancellation detection uses a per-spawn-call-site closure-body
#    scan with a 50-line window.  Some spawned tasks legitimately ignore
#    cancellation (one-shot fire-and-forget setup tasks).  Phase 10c's
#    hand-audit confirms each.
#
# 5. `Arc<Mutex<…>>` matching does NOT recurse into type aliases.  A
#    `pub type Shared<T> = Arc<Mutex<T>>;` plus uses of `Shared<…>` will
#    miss the underlying pattern.  No such alias exists workspace-wide
#    as of 2026-05-19; audit re-runs flag the gap if it appears.
#
# Optional cargo cross-checks
# ---------------------------
# Pass `--with-cargo` to also run, in order:
#   * `cargo build --workspace --tests`                          (~30 s warm)
#   * `cargo clippy --workspace --tests -- -W clippy::await_holding_lock` (~45 s warm)
#
# The default mode (no flag) is rg+awk only and runs in < 5 s.
#
# Usage
# -----
#   scripts/dev/concurrency_audit.sh                  # fast (~3 s)
#   scripts/dev/concurrency_audit.sh --with-cargo     # + cargo + clippy lock-await lint
#
# Output goes to stdout in Markdown.  Redirect to capture:
#
#   scripts/dev/concurrency_audit.sh \
#     > docs/dev/baseline/2026-05-19/phase_10_concurrency_baseline.md
#
# Exit codes
# ----------
#   0 — script ran to completion.  The *counts* of spawns / locks /
#       channels are information, not a failure signal.
#   1 — fatal scripting error (rg missing, repo root not detectable,
#       cargo invocation failed when `--with-cargo` was requested).

set -uo pipefail

WITH_CARGO=0
for arg in "$@"; do
    case "$arg" in
        --with-cargo) WITH_CARGO=1 ;;
        --help | -h)
            sed -n '1,103p' "$0"
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument '$arg' (expected --with-cargo | --help)" >&2
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

# ── rg filter (prod-only — excludes test code) ────────────────────────
# Note: the `**/` recursive prefix is required for the directory excludes
# because UFFS has in-tree test modules under `src/.../tests/` (the
# canonical Rust pattern) in addition to top-level `crates/*/tests/`
# integration-test directories.  `!tests/**` (without `**/`) only
# matches the top-level pattern.
RG_PROD_GLOBS=(
    -g '*.rs'
    -g '!**/tests/**'
    -g '!**/benches/**'
    -g '!**/examples/**'
    -g '!**/tests.rs'
    -g '!**/*_tests.rs'
    -g '!**/*_test.rs'
    -g '!**/test_*.rs'
)

# Count occurrences of a fixed-string pattern across a directory.
count_fixed() {
    local dir="$1"
    local pattern="$2"
    rg "${RG_PROD_GLOBS[@]}" -F --no-heading --no-filename --count-matches \
        "$pattern" "$dir" 2>/dev/null \
        | awk 'BEGIN{s=0} {s+=$1} END{print s+0}'
}

# Count occurrences of a regex pattern across a directory.
count_regex() {
    local dir="$1"
    local pattern="$2"
    rg "${RG_PROD_GLOBS[@]}" --no-heading --no-filename --count-matches \
        "$pattern" "$dir" 2>/dev/null \
        | awk 'BEGIN{s=0} {s+=$1} END{print s+0}'
}

# Count `#[tokio::test]` sites in a directory (INCLUDES tests/, since
# those are precisely where #[tokio::test] lives).
count_tokio_tests() {
    local dir="$1"
    rg -g '*.rs' -F --no-heading --no-filename --count-matches \
        '#[tokio::test' "$dir" 2>/dev/null \
        | awk 'BEGIN{s=0} {s+=$1} END{print s+0}'
}

# ── Per-dimension extractors ──────────────────────────────────────────

# Dimension 1 — list every `tokio::spawn(` call-site as `path:line`.
list_spawn_sites() {
    local dir="$1"
    rg "${RG_PROD_GLOBS[@]}" -F -n --no-heading \
        'tokio::spawn(' "$dir" 2>/dev/null \
        | cut -d: -f1-2
}

# Dimension 2 — list every `.read().await` / `.write().await` /
# `.lock().await` site as `path:line:snippet`.
list_lock_await_sites() {
    local dir="$1"
    rg "${RG_PROD_GLOBS[@]}" -n --no-heading \
        '\.(read|write|lock)\(\)\.await\b' "$dir" 2>/dev/null
}

# Dimension 3 — files that contain BOTH `async fn` AND `std::fs::*`
# (candidates for the blocking-IO-in-async hand-audit).
list_blocking_io_async_candidates() {
    local dir="$1"
    # Files with `async fn`.
    local async_files
    async_files=$(rg "${RG_PROD_GLOBS[@]}" -l 'async fn' "$dir" 2>/dev/null | sort -u)
    # Files with `std::fs::*` or `std::thread::sleep`.
    local blocking_files
    blocking_files=$(rg "${RG_PROD_GLOBS[@]}" -l \
        'std::fs::|std::thread::sleep' "$dir" 2>/dev/null | sort -u)
    # Intersection.
    comm -12 <(echo "$async_files") <(echo "$blocking_files")
}

# Dimension 4 — `Arc<Mutex<…>>` / `Arc<RwLock<…>>` nesting (including
# multi-layer-share patterns like `Arc<Mutex<Arc<…>>>`).
list_arc_mutex_sites() {
    local dir="$1"
    rg "${RG_PROD_GLOBS[@]}" -n --no-heading \
        'Arc<(Mutex|RwLock)<' "$dir" 2>/dev/null
}

# Dimension 5 — await sites on IO/network primitives that COULD need a
# timeout.  We list the candidates; Phase 10e hand-audits each for
# `tokio::time::timeout(` enclosure within 20 lines above.
list_timeout_candidate_awaits() {
    local dir="$1"
    rg "${RG_PROD_GLOBS[@]}" -n --no-heading \
        '\.(connect|read_exact|write_all|read_to_end|recv|accept|read_buf)\(\)\.await\b' \
        "$dir" 2>/dev/null
}

# Dimension 6 — `tokio::spawn(` sites whose closure body in the next 50
# lines does NOT contain `select!` / `CancellationToken` / `cancel` /
# shutdown-related keywords.  Hand-audit confirms cancellation policy.
#
# Implementation: emit a `path:line` for each spawn site; cross-checking
# the closure body is done via `rg -A 50` filter at report time.
list_spawn_without_cancellation_candidates() {
    local dir="$1"
    # All spawn sites, with 50-line trailing context, no headings.
    local raw
    raw=$(rg "${RG_PROD_GLOBS[@]}" -A 50 -n --no-heading \
        'tokio::spawn(' -F "$dir" 2>/dev/null)
    # Group by file:line, mark sites whose context contains cancellation
    # keywords.  Keyword set uses word boundaries to avoid false-positive
    # matches on identifiers that contain the substring (e.g. `cancel_tx`
    # would falsely match bare `cancel`; we require either `CancellationToken`,
    # `cancellation_token`, `shutdown_token`, `abort_signal`, `select!`, or
    # a `recv_cancel` / `is_cancelled` / `.cancelled()` call).
    echo "$raw" | awk '
        /^--$/ { in_block=0; next }
        /tokio::spawn\(/ {
            if (block_text != "") {
                if (!cancel_seen) print site_header
                block_text=""; cancel_seen=0
            }
            site_header=$0
            in_block=1
            block_text=$0
            next
        }
        in_block {
            block_text = block_text "\n" $0
            if (match($0, /(select!|CancellationToken|cancellation_token|shutdown_token|abort_signal|is_cancelled|\.cancelled\(\)|recv_cancel)/)) {
                cancel_seen=1
            }
        }
        END {
            if (block_text != "" && !cancel_seen) print site_header
        }
    ' | sed -nE 's|^([^-][^:]+):([0-9]+):.*|\1:\2|p' | sort -u
}

# Dimension 7 — list every unbounded-channel construction site.
list_unbounded_channel_sites() {
    local dir="$1"
    rg "${RG_PROD_GLOBS[@]}" -n --no-heading \
        'unbounded_channel\(\)|broadcast::channel\(' "$dir" 2>/dev/null
}

# Bounded-channel construction sites (for the per-crate table).
list_bounded_channel_sites() {
    local dir="$1"
    rg "${RG_PROD_GLOBS[@]}" -n --no-heading \
        'mpsc::channel\(|watch::channel\(|oneshot::channel\(' "$dir" 2>/dev/null
}

# ── Markdown preamble ─────────────────────────────────────────────────
SHA="$(git rev-parse HEAD)"
DATE_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

cat <<EOF
# Phase 10 — Async, concurrency, and shared state baseline

**Captured:** ${DATE_UTC}
**SHA:** \`${SHA}\`
**Methodology:** \`scripts/dev/concurrency_audit.sh\` — \`rg\`-based count
across each crate's \`src/\` tree.  Excludes \`tests/\`, \`benches/\`,
\`examples/\`, and files matching \`tests.rs\` / \`*_tests.rs\` /
\`*_test.rs\` / \`test_*.rs\`.

**Companion plan:** \`docs/dev/architecture/code_clean/phase_10_async_concurrency_shared_state_implementation_plan.md\` (local-only).
**Tracking issue:** [#302](https://github.com/skyllc-ai/UltraFastFileSearch/issues/302).

---

## §1 — Per-crate concurrency surface area

| Crate | \`async fn\` | \`tokio::spawn\` | \`spawn_blocking\` | \`std::sync::*\` | \`tokio::sync::*\` | \`Arc<Mutex<>>\` | bounded ch. | unbounded ch. | \`timeout\` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
EOF

# Workspace accumulators.
TOTAL_ASYNC_FN=0
TOTAL_SPAWN=0
TOTAL_SPAWN_BLOCKING=0
TOTAL_STD_LOCK=0
TOTAL_TOKIO_LOCK=0
TOTAL_ARC_MUTEX=0
TOTAL_BOUNDED_CH=0
TOTAL_UNBOUNDED_CH=0
TOTAL_TIMEOUT=0

for c in "${CRATES[@]}"; do
    crate_dir="crates/$c"
    async_fn=$(count_regex "$crate_dir" 'async fn|async move')
    spawn=$(count_fixed "$crate_dir" 'tokio::spawn(')
    spawn_blk=$(count_fixed "$crate_dir" 'spawn_blocking')
    std_lock=$(count_regex "$crate_dir" 'std::sync::(Mutex|RwLock)|sync::Mutex<|sync::RwLock<')
    tokio_lock=$(count_regex "$crate_dir" 'tokio::sync::(Mutex|RwLock|Semaphore)')
    arc_mu=$(count_regex "$crate_dir" 'Arc<(Mutex|RwLock)<')
    bounded=$(count_regex "$crate_dir" 'mpsc::channel\(|watch::channel\(|oneshot::channel\(')
    unbounded=$(count_regex "$crate_dir" 'unbounded_channel\(\)|broadcast::channel\(')
    timeout=$(count_regex "$crate_dir" 'tokio::time::timeout\b|::timeout_at\(')

    TOTAL_ASYNC_FN=$((TOTAL_ASYNC_FN + async_fn))
    TOTAL_SPAWN=$((TOTAL_SPAWN + spawn))
    TOTAL_SPAWN_BLOCKING=$((TOTAL_SPAWN_BLOCKING + spawn_blk))
    TOTAL_STD_LOCK=$((TOTAL_STD_LOCK + std_lock))
    TOTAL_TOKIO_LOCK=$((TOTAL_TOKIO_LOCK + tokio_lock))
    TOTAL_ARC_MUTEX=$((TOTAL_ARC_MUTEX + arc_mu))
    TOTAL_BOUNDED_CH=$((TOTAL_BOUNDED_CH + bounded))
    TOTAL_UNBOUNDED_CH=$((TOTAL_UNBOUNDED_CH + unbounded))
    TOTAL_TIMEOUT=$((TOTAL_TIMEOUT + timeout))

    printf "| \`%s\` | %d | %d | %d | %d | %d | %d | %d | %d | %d |\n" \
        "$c" "$async_fn" "$spawn" "$spawn_blk" "$std_lock" "$tokio_lock" \
        "$arc_mu" "$bounded" "$unbounded" "$timeout"
done

cat <<EOF
| **Workspace total** | **${TOTAL_ASYNC_FN}** | **${TOTAL_SPAWN}** | **${TOTAL_SPAWN_BLOCKING}** | **${TOTAL_STD_LOCK}** | **${TOTAL_TOKIO_LOCK}** | **${TOTAL_ARC_MUTEX}** | **${TOTAL_BOUNDED_CH}** | **${TOTAL_UNBOUNDED_CH}** | **${TOTAL_TIMEOUT}** |

Plus **$(count_tokio_tests crates) \`#[tokio::test]\` sites** across the workspace (test code; not subject to Phase-10 hazards but useful baseline).

---

## §2 — \`tokio::spawn\` call-site inventory (dimension 1)

EOF

if [[ "$TOTAL_SPAWN" -eq 0 ]]; then
    echo "_No \`tokio::spawn(\` call sites in any crate._"
else
    cat <<EOF
**${TOTAL_SPAWN} call-site(s)** across the workspace.  Phase 10c hand-audits
each one for: who owns it / how it's shut down / how errors are observed /
what happens on cancellation.

| Crate | File:line |
|---|---|
EOF
    for c in "${CRATES[@]}"; do
        list_spawn_sites "crates/$c" | while IFS= read -r site; do
            [[ -z "$site" ]] && continue
            printf "| \`%s\` | \`%s\` |\n" "$c" "$site"
        done
    done
fi

cat <<EOF

---

## §3 — Lock-across-await candidates (dimension 2)

EOF

LOCK_AWAIT_TOTAL=0
for c in "${CRATES[@]}"; do
    n=$(list_lock_await_sites "crates/$c" 2>/dev/null | wc -l | tr -d ' ')
    LOCK_AWAIT_TOTAL=$((LOCK_AWAIT_TOTAL + n))
done

if [[ "$LOCK_AWAIT_TOTAL" -eq 0 ]]; then
    echo "_No \`.read().await\` / \`.write().await\` / \`.lock().await\` sites in any crate._"
else
    cat <<EOF
**${LOCK_AWAIT_TOTAL} candidate site(s)** across the workspace.  These are
literal \`.read().await\` / \`.write().await\` / \`.lock().await\` matches —
they are **candidates**, not confirmed hazards.  Phase 10b hand-audits
each one for "guard held across an inner \`.await\`" (the hazard) vs
"guard acquired with \`.await\` and dropped before the next \`.await\`"
(legitimate).

| Crate | File:line | Snippet |
|---|---|---|
EOF
    for c in "${CRATES[@]}"; do
        list_lock_await_sites "crates/$c" 2>/dev/null | while IFS= read -r line; do
            [[ -z "$line" ]] && continue
            path_line=$(echo "$line" | cut -d: -f1-2)
            snippet=$(echo "$line" | cut -d: -f3- | sed 's/^[[:space:]]*//' | head -c 120)
            printf "| \`%s\` | \`%s\` | \`%s\` |\n" "$c" "$path_line" "$snippet"
        done
    done
fi

cat <<EOF

---

## §4 — Async-lock surface (\`tokio::sync::*\`)

EOF

if [[ "$TOTAL_TOKIO_LOCK" -eq 0 ]]; then
    echo "_No \`tokio::sync::Mutex\` / \`RwLock\` / \`Semaphore\` use sites in any crate._"
else
    cat <<EOF
**${TOTAL_TOKIO_LOCK} \`tokio::sync::*\` site(s)** across the workspace.
These are the central async-lock audit targets for Phase 10b.

| Crate | File:line | Snippet |
|---|---|---|
EOF
    for c in "${CRATES[@]}"; do
        rg "${RG_PROD_GLOBS[@]}" -n --no-heading \
            'tokio::sync::(Mutex|RwLock|Semaphore)' "crates/$c" 2>/dev/null \
            | while IFS= read -r line; do
                [[ -z "$line" ]] && continue
                path_line=$(echo "$line" | cut -d: -f1-2)
                snippet=$(echo "$line" | cut -d: -f3- | sed 's/^[[:space:]]*//' | head -c 120)
                printf "| \`%s\` | \`%s\` | \`%s\` |\n" "$c" "$path_line" "$snippet"
            done
    done
fi

cat <<EOF

---

## §5 — \`Arc<Mutex<…>>\` / \`Arc<RwLock<…>>\` patterns (dimension 4)

EOF

if [[ "$TOTAL_ARC_MUTEX" -eq 0 ]]; then
    echo "_No \`Arc<Mutex<…>>\` / \`Arc<RwLock<…>>\` patterns in any crate._"
else
    cat <<EOF
**${TOTAL_ARC_MUTEX} site(s)** across the workspace.  Multi-layer-share
nesting (\`Arc<Mutex<Arc<…>>>\`) is flagged separately — the playbook
§1096 calls this "shared mutable state wrapped in layers of \`Arc<Mutex<…>>\`",
a structural smell that often indicates the wrong sharing primitive.

| Crate | File:line | Snippet |
|---|---|---|
EOF
    for c in "${CRATES[@]}"; do
        # Filter out doc-comment lines (`///`, `//!`) and block-comment
        # continuations (` * `) so that rustdoc prose referencing
        # `Arc<Mutex<...>>` in narrative text doesn't show up as a real
        # use site.
        list_arc_mutex_sites "crates/$c" 2>/dev/null \
            | grep -Ev '^[^:]+:[0-9]+:[[:space:]]*(///|//!|//[[:space:]]|/\*|\*[[:space:]])' \
            | while IFS= read -r line; do
                [[ -z "$line" ]] && continue
                path_line=$(echo "$line" | cut -d: -f1-2)
                snippet=$(echo "$line" | cut -d: -f3- | sed 's/^[[:space:]]*//' | head -c 120)
                printf "| \`%s\` | \`%s\` | \`%s\` |\n" "$c" "$path_line" "$snippet"
            done
    done

    echo
    nested=$(rg "${RG_PROD_GLOBS[@]}" -n --no-heading \
        'Arc<(Mutex|RwLock)<Arc<' crates 2>/dev/null \
        | grep -Ev '^[^:]+:[0-9]+:[[:space:]]*(///|//!|//[[:space:]]|/\*|\*[[:space:]])' \
        | wc -l | tr -d ' ')
    if [[ "$nested" -gt 0 ]]; then
        echo "**${nested} multi-layer \`Arc<Mutex<Arc<…>>>\` nesting site(s)** found — review for restructure."
    else
        echo "**0 multi-layer \`Arc<Mutex<Arc<…>>>\` nesting sites** — flat sharing only."
    fi
fi

cat <<EOF

---

## §6 — Channel inventory (dimension 7)

EOF

if [[ $((TOTAL_BOUNDED_CH + TOTAL_UNBOUNDED_CH)) -eq 0 ]]; then
    echo "_No \`mpsc\` / \`watch\` / \`oneshot\` / \`broadcast\` / \`unbounded_channel\` sites in any crate._"
else
    cat <<EOF
**${TOTAL_BOUNDED_CH} bounded** (mpsc / watch / oneshot) + **${TOTAL_UNBOUNDED_CH} unbounded**
(unbounded_channel / broadcast) construction sites.

### §6.1 Unbounded channels (Phase 10d audit targets)

Each unbounded site must have a documented "by-construction bounded"
rationale (e.g. one producer with finite message budget by lifecycle) OR
be converted to bounded with explicit capacity rationale.

| Crate | File:line | Snippet |
|---|---|---|
EOF
    for c in "${CRATES[@]}"; do
        list_unbounded_channel_sites "crates/$c" 2>/dev/null | while IFS= read -r line; do
            [[ -z "$line" ]] && continue
            path_line=$(echo "$line" | cut -d: -f1-2)
            snippet=$(echo "$line" | cut -d: -f3- | sed 's/^[[:space:]]*//' | head -c 120)
            printf "| \`%s\` | \`%s\` | \`%s\` |\n" "$c" "$path_line" "$snippet"
        done
    done

    cat <<EOF

### §6.2 Bounded channels (informational)

| Crate | File:line | Snippet |
|---|---|---|
EOF
    for c in "${CRATES[@]}"; do
        list_bounded_channel_sites "crates/$c" 2>/dev/null | while IFS= read -r line; do
            [[ -z "$line" ]] && continue
            path_line=$(echo "$line" | cut -d: -f1-2)
            snippet=$(echo "$line" | cut -d: -f3- | sed 's/^[[:space:]]*//' | head -c 120)
            printf "| \`%s\` | \`%s\` | \`%s\` |\n" "$c" "$path_line" "$snippet"
        done
    done
fi

cat <<EOF

---

## §7 — Timeout coverage (dimension 5)

**${TOTAL_TIMEOUT} \`tokio::time::timeout(\` / \`timeout_at(\` site(s)** across
the workspace.  Phase 10e cross-references each IO/network/IPC await site
against an enclosing timeout (within 20 lines above).

### §7.1 Timeout-candidate await sites (need hand-audit)

These are \`.connect/.read_exact/.write_all/.read_to_end/.recv/.accept/.read_buf\`
sites — each MUST either be inside a \`tokio::time::timeout(…)\` block or
have a documented "deliberately blocking forever" rationale.

| Crate | File:line | Snippet |
|---|---|---|
EOF

for c in "${CRATES[@]}"; do
    list_timeout_candidate_awaits "crates/$c" 2>/dev/null | while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        path_line=$(echo "$line" | cut -d: -f1-2)
        snippet=$(echo "$line" | cut -d: -f3- | sed 's/^[[:space:]]*//' | head -c 120)
        printf "| \`%s\` | \`%s\` | \`%s\` |\n" "$c" "$path_line" "$snippet"
    done
done

cat <<EOF

---

## §8 — Blocking-IO-in-async candidate files (dimension 3)

Files containing BOTH \`async fn\` AND \`std::fs::*\` / \`std::thread::sleep\`.
Phase 10f hand-audits each \`std::fs::*\` site in these files to confirm
it is either (a) inside a \`spawn_blocking\` / \`block_in_place\`, or
(b) inside a sync helper called only from sync contexts.

EOF

BLOCKING_IO_TOTAL=0
for c in "${CRATES[@]}"; do
    candidates=$(list_blocking_io_async_candidates "crates/$c" 2>/dev/null)
    # Use `grep -c '^.'` (not `grep -c .`) so that a single empty
    # newline from `echo ""` does not get counted as 1.
    [[ -z "$candidates" ]] && continue
    count=$(printf '%s\n' "$candidates" | grep -c '^.')
    BLOCKING_IO_TOTAL=$((BLOCKING_IO_TOTAL + count))
done

if [[ "$BLOCKING_IO_TOTAL" -eq 0 ]]; then
    echo "_No files contain BOTH \`async fn\` AND \`std::fs::*\` / \`std::thread::sleep\`._"
else
    cat <<EOF
**${BLOCKING_IO_TOTAL} candidate file(s)** across the workspace.

| Crate | File |
|---|---|
EOF
    for c in "${CRATES[@]}"; do
        list_blocking_io_async_candidates "crates/$c" 2>/dev/null | while IFS= read -r file; do
            [[ -z "$file" ]] && continue
            printf "| \`%s\` | \`%s\` |\n" "$c" "$file"
        done
    done
fi

cat <<EOF

---

## §9 — Cancellation / shutdown infrastructure (dimension 6)

EOF

CANCEL_TOKEN=$(count_regex crates 'CancellationToken|cancellation_token|shutdown_token')
CTRL_C=$(count_regex crates 'tokio::signal::ctrl_c|ctrl_c\(\)')
ABORT=$(count_regex crates '\.abort\(\)|abort_handle')
SELECT_BANG=$(rg "${RG_PROD_GLOBS[@]}" -F --no-heading --no-filename --count-matches \
    'tokio::select!' crates 2>/dev/null \
    | awk 'BEGIN{s=0} {s+=$1} END{print s+0}')
NOTIFY=$(count_regex crates 'tokio::sync::Notify\b')

cat <<EOF
| Mechanism | Count | Notes |
|---|---:|---|
| \`CancellationToken\` / \`shutdown_token\` | ${CANCEL_TOKEN} | \`tokio_util::sync::CancellationToken\` (cooperative cancel) |
| \`tokio::signal::ctrl_c\` | ${CTRL_C} | Process-signal-driven shutdown |
| \`.abort()\` / \`abort_handle\` | ${ABORT} | Hard task cancellation (forces drop at next \`.await\`) |
| \`tokio::select!\` | ${SELECT_BANG} | Multi-future racing (the canonical cancellation idiom) |
| \`tokio::sync::Notify\` | ${NOTIFY} | One-shot wakeups (often paired with shared state) |

### §9.1 Spawn sites without cancellation keywords nearby (heuristic)

Each \`tokio::spawn(\` whose closure body (next 50 lines) does NOT contain
\`select!\` / \`CancellationToken\` / \`cancel\` / \`shutdown_token\` / \`abort_signal\`.
Phase 10c hand-audits to confirm cancellation policy per site.

EOF

NO_CANCEL_TOTAL=0
for c in "${CRATES[@]}"; do
    sites=$(list_spawn_without_cancellation_candidates "crates/$c" 2>/dev/null)
    # Use `grep -c '^.'` (anchored, non-empty) instead of `grep -c .`,
    # because `echo "" | grep -c .` returns `1` (it counts the trailing
    # newline produced by `echo`).  This produced phantom counts of "24
    # spawn sites without cancellation" when the actual filtered list
    # was empty.
    [[ -z "$sites" ]] && continue
    n=$(printf '%s\n' "$sites" | grep -c '^.')
    NO_CANCEL_TOTAL=$((NO_CANCEL_TOTAL + n))
done

if [[ "$NO_CANCEL_TOTAL" -eq 0 ]]; then
    echo "_All \`tokio::spawn(\` sites have cancellation keywords within 50 lines._"
else
    cat <<EOF
**${NO_CANCEL_TOTAL} site(s) without cancellation keywords** within the next 50 lines.

| Crate | File:line |
|---|---|
EOF
    for c in "${CRATES[@]}"; do
        list_spawn_without_cancellation_candidates "crates/$c" 2>/dev/null | while IFS= read -r site; do
            [[ -z "$site" ]] && continue
            printf "| \`%s\` | \`%s\` |\n" "$c" "$site"
        done
    done
fi

cat <<EOF

---

## §10 — Workspace totals

- \`async fn\` + async blocks: **${TOTAL_ASYNC_FN}**
- \`tokio::spawn(\` call sites: **${TOTAL_SPAWN}**
- \`spawn_blocking\` call sites: **${TOTAL_SPAWN_BLOCKING}**
- \`std::sync::Mutex/RwLock\` sites: **${TOTAL_STD_LOCK}**
- \`tokio::sync::*\` sites: **${TOTAL_TOKIO_LOCK}**
- \`Arc<Mutex<>>\` / \`Arc<RwLock<>>\` sites: **${TOTAL_ARC_MUTEX}**
- Bounded channels (mpsc / watch / oneshot): **${TOTAL_BOUNDED_CH}**
- Unbounded channels (unbounded_channel / broadcast): **${TOTAL_UNBOUNDED_CH}**
- \`tokio::time::timeout(\` / \`timeout_at(\` sites: **${TOTAL_TIMEOUT}**
- \`.read/write/lock().await\` candidate sites: **${LOCK_AWAIT_TOTAL}**
- Blocking-IO-in-async candidate files: **${BLOCKING_IO_TOTAL}**
- \`tokio::spawn(\` sites without nearby cancellation keywords: **${NO_CANCEL_TOTAL}**
- \`#[tokio::test]\` sites (test code): **$(count_tokio_tests crates)**

---

EOF

if [[ "$WITH_CARGO" -eq 1 ]]; then
    cat <<EOF
## §11 — Cargo cross-check (\`--with-cargo\` mode)

> \`cargo build --workspace --tests\` + \`cargo clippy --workspace --tests -- -W clippy::await_holding_lock\`
> — only available when invoked with \`--with-cargo\`.

EOF
    echo '### Build'
    echo '```'
    cargo build --workspace --tests 2>&1 | tail -10
    echo '```'
    echo
    echo '### Clippy `await_holding_lock` lint (Phase 10b enforcement-mode preview)'
    echo '```'
    cargo clippy --workspace --tests -- -W clippy::await_holding_lock 2>&1 \
        | grep -E 'warning|error|^\s+-->|note: this lock|await_holding_lock' \
        | head -40
    echo '```'
    echo
fi

cat <<EOF

---

## Next steps (per plan §1)

1. **Phase 10b** — hand-audit each of the **${LOCK_AWAIT_TOTAL} lock-across-await candidate(s)**.  Add a verdict comment + rustdoc justification at each site, OR refactor to extract-then-await.
2. **Phase 10c** — hand-audit each of the **${TOTAL_SPAWN} \`tokio::spawn(\` site(s)**.  Document owner / shutdown / errors / cancellation at the call-site or in the wrapping spawner function's rustdoc.
3. **Phase 10d** — hand-audit each of the **${TOTAL_UNBOUNDED_CH} unbounded-channel site(s)**.  Justify "by-construction bounded" OR convert to bounded.
4. **Phase 10e** — hand-audit timeout coverage for IO/network/IPC await sites.  Add \`tokio::time::timeout(\` wrappers OR document the deliberate absence.
5. **Phase 10f** — hand-audit each of the **${BLOCKING_IO_TOTAL} blocking-IO-in-async candidate file(s)**.  Confirm every \`std::fs::*\` is sync-context OR \`spawn_blocking\`-wrapped.
6. **Phase 10g** — produce \`docs/architecture/code-quality/concurrency_policy.md\` + per-crate \`# Concurrency\` rustdoc sections.
EOF
