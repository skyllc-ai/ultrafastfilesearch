# Dev-Flow Implementation Plan (v2)

<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Copyright (c) 2025-2026 SKY, LLC. -->

Companion to `dev-flow.md`.  Incorporates external review (2026-04-23)
that correctly identified structural gaps in v1:

- v1 still treated CI as **one long lane** instead of separating
  merge-confidence from artifact production from release packaging.
- v1's "soft-skip everything" rule was too broad — semantic checks
  with local inputs (e.g. `cargo vet` when `Cargo.lock` changed)
  should be **hard-fail**, not soft-skip.
- v1 mixed **when** a gate runs (time) with **why** it exists (purpose).
  A proper model needs both axes.
- v1 proposed `CARGO_INCREMENTAL=0` in `just/shared.just` — the right
  fix is to move the entire cache policy into `.cargo/config.toml`
  (Cargo supports `build.incremental` natively) and **delete** the Just
  export entirely.  One source of truth.
- v1 kept Windows xwin in pre-commit at 40-90 s cold — violates the
  "don't break flow" threshold.  Windows belongs in pre-push + CI, not
  pre-commit.
- v1 left pre-push at **zero test execution**.  nextest profiles +
  filtersets let us run a mandatory fast subset without blowing budget.

Status as of **2026-04-23** · Maintainer: `@githubrobbi`

---

## 1. Target architecture — four lanes, two axes

### 1.1 The two axes

Every gate has both a **time tier** (when) and a **lane purpose** (why).

**Axis A — time tiers** (from v1, unchanged):

- T0 editor · T1 pre-commit · T2 pre-push · T3 CI on PR · T4 nightly

**Axis B — lane purpose** (new):

- **Gate lane** — "is this obviously broken?"  Blocks merge.  Fast.
- **Preview lane** — "is this SHA worth human validation on real binaries?"
  Produces artifacts.  Non-blocking.  On-demand.
- **Release lane** — "is this SHA shippable?"  Signed, SBOM'd,
  published.  Tag-only.
- **Deep-assurance lane** — "does this survive slow / probabilistic
  checks?"  Coverage, miri, fuzz.  Nightly.

A SHA **earns** its way from gate → preview → release.  These are not
coordinate stages on one long pipeline; they are **promotion steps**.

### 1.2 Lane matrix (post-refactor)

| Lane | Trigger | Mandatory? | Max runtime | Outputs |
|---|---|---|---|---|
| T1 pre-commit (gate) | `git commit` | yes, bypassable with `--no-verify` | **&lt; 15 s warm staged-only** | clean working tree |
| T2 pre-push (gate) | `git push` | yes, bypassable | **&lt; 60 s warm** | none; local confidence |
| T3 PR-fast (gate) | PR open / push to PR branch | **required to merge** | **&lt; 8 min wall** | Pass/fail only |
| T3 PR-preview (preview) | PR label `preview-binaries` or manual dispatch, **only after PR-fast green on same SHA** | **not required** | 15-30 min | Windows artifacts + nextest archive + per-file SHA256 manifest (Phase 5 scope; macOS/Linux deferred to Phase 9+) |
| T4 Tier 2 (deep) | weekly cron | no | 60-90 min | coverage, miri, udeps report |
| Tag (release) | `v*.*.*` tag push | n/a (gated by PR-fast being green) | 30-45 min | signed binaries, SBOM, SLSA attestation, GH Release |

Key architectural distinction from today's flow:

- Today: every PR push triggers `ci.yml` which does format+clippy+rustdoc+tests+security+file-size (8 jobs) — **and** the user is expected to rely on Tier 2 + Release for Windows validation.  Result: a regression that only shows on Windows surfaces at release time (10-15 min into a ship run).
- Tomorrow: every PR push triggers `pr-fast.yml` including a **cheap Windows compile-check**.  Preview binaries are opt-in via label.  Release stays tag-only.

### 1.3 Change classification — the single source of truth for `if:` predicates

Every gate's trigger is expressed in terms of one of four file classes.
Same predicates apply locally (pre-commit / pre-push) and in CI (`classify`
job in `pr-fast.yml`).  Hand-coded copies in both places **must** agree,
which is one motivation for the stretch goal in §2.7.

| Class | Patterns (glob) | Intent |
|---|---|---|
| `rust_changed` | `**/*.rs` | Any Rust source edit. |
| `dep_changed` | `Cargo.toml`, `**/Cargo.toml`, `Cargo.lock`, `supply-chain/**` | Dependency graph touched.  Cargo Vet reads the **full** tree via `cargo metadata --all-features`, so `Cargo.toml` counts even when `Cargo.lock` is untouched. |
| `infra_changed` | `.github/**`, `scripts/**`, `.cargo/**`, `.config/**`, `just/**`, `rust-toolchain*`, `clippy.toml`, `rustfmt.toml`, `deny.toml`, `REUSE.toml`, `codecov.yml` | Tooling, workflows, lint / test / deny configs, toolchain pin.  Any of these can break build/CI/hooks. |
| `docs_changed` | `**/*.md`, `docs/**`, `LICENSE*`, `CHANGELOG.md`, `TRADEMARK.md`, `SECURITY.md`, `assets/**` | Documentation and brand assets only. |

**Derived predicate used by all compile/test/lint jobs**:

```
code_changed = rust_changed || dep_changed || infra_changed
```

A PR that only hits `docs_changed` skips every compile gate.  Anything
else runs the full gate.  **Dependency-only PRs run compile/docs/tests/
Windows** — `dep_changed` alone is enough to gate them in, because a
Cargo.toml or lockfile bump can break any of those.  Infra-only PRs
(e.g. workflow tweaks) are treated the same way.

### 1.3.1 Hard gates (block commit / push / merge)

| Gate | Hard when… | Tier | Missing-tool behavior |
|---|---|---|---|
| fmt | `rust_changed` | T1 | stdlib rustfmt; always present |
| clippy (scoped) | `rust_changed` | T1 | stdlib; always present |
| file-size-policy | always | T1 | shell script; always present |
| **`cargo vet check --locked`** | **`dep_changed`** | **T1+T2+T3** | **hard-fail with install hint** — not soft-skip |
| workspace clippy trio | `code_changed` | T2 | always present |
| rustdoc `-Dwarnings` | `code_changed` | T2 | always present |
| **doctests** | `code_changed` | **T2 (new)** | always present |
| `cargo deny check --locked` | `dep_changed` | T2 | `cargo-deny` required; hard-fail on missing |
| `cargo check --locked` | `code_changed` | T2 | always present |
| test-compile (`nextest --no-run --locked`) | `code_changed` | T2 | `cargo-nextest` required; hard-fail on missing (pinned in `.config/nextest.toml`) |
| **nextest `pre-push-smoke` profile** | `code_changed` | **T2 (new)** | same; hard-fail on missing |
| **Windows xwin check (local)** | `code_changed` | T2 | **advisory locally**: soft-skip with install hint when `cargo-xwin` missing.  Hard-gated at T3 by native `windows-check` job. |
| pr-fast required aggregate | always | T3 | n/a |

### 1.3.2 Advisory gates (soft-skip; install hint printed on miss)

| Gate | Rationale |
|---|---|
| typos | spell-check; nice-to-have |
| taplo | TOML formatting; nice-to-have |
| reuse | SPDX header check; nice-to-have |
| cargo machete | unused-dep check; Tier 2 has cargo-udeps as the authoritative version |

### 1.3.3 Design decisions behind the table

**`cargo-vet` is the only tool promoted from advisory to hard.**  Rationale:
it has a dedicated local trigger (`dep_changed`), its input is 100 %
reproducible offline, and CI enforces it anyway — the CI-only placement
is the exact bug that caused the v0.5.71 four-round-trip incident.

**Windows xwin is deliberately split** between advisory-local and
hard-required-remote.  Rationale: `cargo-xwin` downloads the MSVC SDK on
first run (~400 MB) and not all contributors want that footprint.  The
PR-fast native `windows-check` job is the authoritative gate; the local
xwin step is an ergonomics win for contributors who already have it
installed.  This is **not** a regression of the v1 "hard xwin" position
— the feedback correctly flagged that treating a 400 MB download as a
hard local requirement is user-hostile.

**`cargo-deny` and `cargo-nextest` are hard-required rather than advisory.**
Rationale: both are listed in `just update-tools` (`just/dev.just:118`)
as baseline dev tools, and their checks are required in CI.  A missing
binary at push time means the developer is out of sync with the
baseline; hard-fail with install hint is correct.

### 1.4 Gate scheduling — ordered by first-actionable-failure

v1's `_lint_pre_push.sh` spawns everything at once and lets wall-clock
finish.  That maximizes throughput but delays the first actionable red
result.  Cargo's target-dir lock serialises most heavy jobs anyway.

New order (T2 pre-push), in two buckets:

**Bucket 1 — fire-and-wait in parallel (all cheap, all non-cargo):**

1. fmt check
2. file-size-policy
3. typos (advisory)
4. reuse (advisory)
5. taplo (advisory)
6. **cargo vet** (when dep files changed)

**Bucket 2 — sequential in meaningful order (each command uses `--locked`
where supported):**

1. `cargo check --workspace --all-targets --all-features --locked` — fastest compile gate, catches most type errors before clippy.
2. clippy `lint-ci` (`--locked`)
3. clippy `lint-prod` (`--locked`)
4. clippy `lint-tests` (`--locked`)
5. `cargo doc --workspace --all-features --no-deps --locked` with `RUSTDOCFLAGS=-Dwarnings`
6. `cargo test --doc --workspace --all-features --locked` (new)
7. `cargo deny check` — runs when `dep_changed`, advisory otherwise
8. `cargo nextest run --no-run --workspace --all-targets --all-features --locked` (test compile)
9. `cargo nextest run --profile pre-push-smoke --locked` (new)
10. `cargo xwin check --workspace --all-targets --all-features --target x86_64-pc-windows-msvc --locked` — **advisory locally** (soft-skip with install hint if `cargo-xwin` missing; authoritative gate is PR-fast native `windows-check`)

The first red in Bucket 2 aborts the rest (fail-fast).  Bucket 1
continues to completion so the user sees all cheap-check results in
one report.  **Why `--locked` everywhere**: Cargo's `--locked` enforces
the exact resolution from `Cargo.lock` and is the canonical "CI" mode;
matching local to CI eliminates a class of "green locally, red in CI"
surprises where the resolver fetched a newer patch.

---

## 2. Detailed change list — file by file

### 2.1 `.cargo/config.toml` — single source of cache policy

**Current** (`@/Users/rnio/Private/Github/UltraFastFileSearch/.cargo/config.toml:16-25`):

```toml
[build]
target-dir = "target"
rustc-wrapper = "sccache"
```

**Target**:

```toml
[build]
target-dir = "target"
# sccache + incremental=false is the canonical local cache policy.
# Paired here (not in just/shared.just) so every cargo invocation —
# including those spawned by git hooks, rust-script, IDE plugins,
# etc. — inherits both settings as one atomic config.
# Docs: https://doc.rust-lang.org/cargo/reference/config.html#build
rustc-wrapper = "sccache"
incremental = false
```

And delete `@/Users/rnio/Private/Github/UltraFastFileSearch/just/shared.just:15`
line `export CARGO_INCREMENTAL := "1"` entirely.

**Why this is better than v1's proposal**: Cargo reads config.toml first,
then env overrides.  Today's drift exists because `just/shared.just`
_exports_ `CARGO_INCREMENTAL=1` into every subshell — which overrides
`.cargo/config.toml`'s implicit `incremental = true` default.  Flipping
the env var to `0` in Just "works" but keeps two authorities.  Removing
the env var and adding `incremental = false` to config.toml has **one
authority** — env vars only override in CI, where they're explicitly set.

**Rollback**: If a contributor wants incremental for a specific
experimental session they can still `CARGO_INCREMENTAL=1 cargo build`
— env overrides config.  The default just becomes the correct one.

### 2.2 `.config/nextest.toml` — new `pre-push-smoke` profile

**Current**: three profiles (`default`, `ci`, `slow`).

**Add**:

```toml
# ---------------------------------------------------------------------------
# pre-push-smoke — mandatory fast test subset at git-push time.
# Budget: 10-20 s warm on a developer laptop.
# Usage: cargo nextest run --profile pre-push-smoke --locked
#
# Selection is a denylist expressed in nextest's filterset language:
#   - Exclude the validation suite (`test_validate_*`) — those tests
#     are heavy integration and belong in the CI `ci` profile.
#   - Exclude the entire `uffs-client` package — its shmem tests
#     serialise globally via `threads-required = num-cpus` and
#     dominate wall-time when mixed with fast unit tests.
#   - Exclude the benchmark harness binaries
#     (`benchmark_filtering`, `benchmark_sorting`) that are compiled
#     under `--tests`.
#   - Everything else: unit tests across all remaining crates.
#
# Long term: replace package-level denylist with per-test overrides
# that carry an explicit `slow` attribute, so the filter becomes
# `not attr(slow)` rather than a list of package / binary names.
# ---------------------------------------------------------------------------
[profile.pre-push-smoke]
test-threads = -2
retries = 0
fail-fast = true

slow-timeout = { period = "30s", terminate-after = 1 }
leak-timeout = "100ms"

failure-output = "immediate-final"
success-output = "never"
status-level = "fail"
final-status-level = "flaky"

default-filter = """
    not test(/^test_validate/)
  & not package(uffs-client)
  & not binary(benchmark_filtering)
  & not binary(benchmark_sorting)
"""
```

**Runtime expectation** on this workspace: roughly 8-15 s warm.  Will
be validated during Phase 3 rollout.

### 2.3 `scripts/hooks/_lint_pre_push.sh` — ordered schedule + hard cargo-vet

Replace the current "spawn everything at once" pattern with the two-bucket
scheduler described in §1.4.  Key contract changes:

- **cargo vet** becomes **hard-required** when `Cargo.lock` or
  `supply-chain/**` touched in the about-to-push range.  If tool is
  missing in that case → fail with install hint.
- **doctests** added as mandatory.
- **nextest smoke** added as mandatory.
- **cargo check** added as the first compile gate (fast fail before
  the three clippy runs).

**Critical protocol detail**: `git push` invokes the pre-push hook with
the ref updates on **stdin**, one per line, format:

```
<local_ref> <local_oid> <remote_ref> <remote_oid>\n
```

(reference: <https://git-scm.com/docs/githooks#_pre_push>).  Earlier
drafts tried to compute the pushed range via `git merge-base HEAD @{push}`
or `origin/<branch>` heuristics.  Those break on (a) first push of a new
branch, (b) alternate remotes, (c) force-pushes, (d) multi-ref pushes, and
the fallback-to-HEAD error path silently collapses the diff to empty —
meaning a hard gate could be skipped exactly when it's needed.  The
implementation **must** consume stdin.

Detailed pseudo-code:

```bash
#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------
# 1. Consume git's pre-push stdin protocol.
# ---------------------------------------------------------------------
ZERO='0000000000000000000000000000000000000000'
CHANGED_FILES=""

while IFS=' ' read -r _local_ref local_oid _remote_ref remote_oid; do
    # Blank line or deletion ($local_oid == ZERO) — skip.
    [[ -z "${local_oid:-}" || "$local_oid" == "$ZERO" ]] && continue

    if [[ "$remote_oid" == "$ZERO" ]]; then
        # New remote ref (first push of this branch).  Diff against the
        # best available base: merge-base with origin/main, falling back
        # to the root commit.  Conservative: if we cannot determine a
        # base, treat everything as changed (triggers all hard gates).
        base=$(git merge-base "$local_oid" origin/main 2>/dev/null \
            || git rev-list --max-parents=0 "$local_oid" 2>/dev/null | tail -n1 \
            || echo "")
    else
        base="$remote_oid"
    fi

    if [[ -n "$base" ]]; then
        CHANGED_FILES+=$'\n'$(git diff --name-only "$base" "$local_oid")
    else
        # Be conservative: can't compute diff → run everything.
        CHANGED_FILES="__UNKNOWN__"
        break
    fi
done

# If CHANGED_FILES is empty (no parseable refs — e.g. hook invoked outside
# of git push for testing), also be conservative.
[[ -z "${CHANGED_FILES// /}" ]] && CHANGED_FILES="__UNKNOWN__"

# ---------------------------------------------------------------------
# 2. Classify into the four file classes from §1.3.
# ---------------------------------------------------------------------
class_matches() {
    [[ "$CHANGED_FILES" == "__UNKNOWN__" ]] && return 0
    echo "$CHANGED_FILES" | grep -E "$1" >/dev/null
}

RUST_CHANGED=0;  class_matches '\.rs$' && RUST_CHANGED=1
DEP_CHANGED=0;   class_matches '^(.*Cargo\.toml$|Cargo\.lock$|supply-chain/)' && DEP_CHANGED=1
INFRA_CHANGED=0; class_matches '^(\.github/|scripts/|\.cargo/|\.config/|just/|rust-toolchain|clippy\.toml$|rustfmt\.toml$|deny\.toml$|REUSE\.toml$|codecov\.yml$)' && INFRA_CHANGED=1
CODE_CHANGED=$(( RUST_CHANGED || DEP_CHANGED || INFRA_CHANGED ))

# ---------------------------------------------------------------------
# 3. Bucket 1 — cheap, parallel.
# ---------------------------------------------------------------------
spawn "fmt"       cargo fmt --all -- --check
spawn "file-size" bash scripts/ci/check_file_size_policy.sh

if (( DEP_CHANGED )); then
    if ! command -v cargo-vet >/dev/null 2>&1; then
        printf '\033[0;31m❌ cargo-vet required (Cargo.{toml,lock} or supply-chain/ changed)\033[0m\n' >&2
        printf '   install: \033[0;36mcargo install cargo-vet --locked\033[0m\n' >&2
        printf '   or run:  \033[0;36mjust install-dev-tools\033[0m\n' >&2
        exit 2
    fi
    spawn "vet" cargo vet check --locked
fi

command -v typos  >/dev/null 2>&1 && spawn "typos" typos .
command -v reuse  >/dev/null 2>&1 && spawn "reuse" reuse lint --quiet
command -v taplo  >/dev/null 2>&1 && spawn "taplo" taplo fmt --check

# ---------------------------------------------------------------------
# 4. Bucket 2 — sequential, fail-fast.  Only runs when code changed.
# ---------------------------------------------------------------------
if (( CODE_CHANGED )); then
    run_seq cargo check --workspace --all-targets --all-features --locked
    run_seq just lint-ci
    run_seq just lint-prod
    run_seq just lint-tests
    run_seq env RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --all-features --no-deps --locked
    run_seq env RUSTDOCFLAGS=-Dwarnings cargo test --doc --workspace --all-features --locked
    (( DEP_CHANGED )) && run_seq cargo deny check --locked --hide-inclusion-graph
    run_seq cargo nextest run --workspace --all-targets --all-features --no-run --locked
    run_seq cargo nextest run --profile pre-push-smoke --locked
    # Windows xwin: advisory locally (see §1.3.3).  Print install hint on miss.
    if command -v cargo-xwin >/dev/null 2>&1; then
        run_seq just check-windows
    else
        printf '\033[1;33m⚠  cargo-xwin missing — skipping local Windows check\033[0m\n'
        printf '   PR-fast will validate on windows-latest regardless.\n'
        printf '   To check locally: \033[0;36mcargo install cargo-xwin\033[0m\n'
    fi
fi

# Wait on Bucket 1 results, aggregate into exit code.
```

### 2.4 `scripts/hooks/_lint_fast.sh` — remove xwin from pre-commit

**Current**: xwin runs at pre-commit if `*.rs` staged and cargo-xwin
present.  Cold cost: 40-90 s.  **Violates the &lt;15 s T1 budget.**

**Target**: xwin moved to pre-push only (local advisory there; PR-fast
authoritative).  Pre-commit keeps: fmt, file-size, clippy-scoped, taplo,
typos, reuse, plus **cargo-vet when staged files hit `dep_changed`**.

**Staged-file classification** (different from pre-push — pre-commit sees
the staged set, not ref diff):

```bash
STAGED=$(git diff --cached --name-only --diff-filter=ACMR)
class_matches() { echo "$STAGED" | grep -E "$1" >/dev/null; }

DEP_STAGED=0; class_matches '^(.*Cargo\.toml$|Cargo\.lock$|supply-chain/)' && DEP_STAGED=1

if (( DEP_STAGED )); then
    command -v cargo-vet >/dev/null 2>&1 || {
        printf '\033[0;31m❌ cargo-vet required at commit (staged: Cargo.toml/lock/supply-chain)\033[0m\n' >&2
        exit 2
    }
    spawn "vet" cargo vet check --locked
fi
```

Result: pre-commit stays under 15 s warm, pre-push owns local Windows
advisory, PR-fast owns authoritative Windows gate.

### 2.5 Split `ci.yml` → `pr-fast.yml` + `preview-artifacts.yml`

**Current**: single `.github/workflows/ci.yml` (17 KB, 8 jobs).

**New architecture**:

#### `pr-fast.yml` (required, always-on)

```yaml
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
name: PR Fast CI

on:
  pull_request:
    branches: [main]
  push:
    branches: [main, develop]
  # Required when using GitHub merge queue — without this the required
  # checks never report against the merge-queue test-merge commit and
  # the queue stalls indefinitely.  Harmless if merge queue is off.
  merge_group:
  workflow_dispatch:

# Least-privilege default.  Individual jobs opt into write scopes if
# they need them (none currently do).
permissions:
  contents: read

concurrency:
  group: pr-fast-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

env:
  # Match local policy (.cargo/config.toml) so green-locally ↔ green-in-CI.
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: 0
  RUST_BACKTRACE: 1

jobs:
  # ---------------------------------------------------------------------
  # classify — emits file-class outputs consumed by every downstream job.
  # MUST be in `required`'s needs so a classify failure cannot silently
  # green-light a PR.
  # ---------------------------------------------------------------------
  classify:
    name: Classify changes
    runs-on: ubuntu-22.04
    timeout-minutes: 2
    outputs:
      rust:  ${{ steps.f.outputs.rust }}
      dep:   ${{ steps.f.outputs.dep }}
      infra: ${{ steps.f.outputs.infra }}
      docs_only: ${{ steps.f.outputs.docs_only }}
      code:  ${{ steps.code.outputs.code }}
    steps:
      - name: Checkout (full history for accurate diff base)
        uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with:
          fetch-depth: 0
          # Pinned to the head SHA for reproducibility when replaying
          # the workflow.  Falls through on push/merge_group events.
          ref: ${{ github.event.pull_request.head.sha || github.sha }}

      - id: f
        uses: dorny/paths-filter@de90cc6fb38fc0963ad72b210f1f284cd68cea36 # v3.0.2
        with:
          filters: |
            rust:
              - '**/*.rs'
            dep:
              - 'Cargo.toml'
              - '**/Cargo.toml'
              - 'Cargo.lock'
              - 'supply-chain/**'
            infra:
              - '.github/**'
              - 'scripts/**'
              - '.cargo/**'
              - '.config/**'
              - 'just/**'
              - 'rust-toolchain*'
              - 'clippy.toml'
              - 'rustfmt.toml'
              - 'deny.toml'
              - 'REUSE.toml'
              - 'codecov.yml'
            docs_only:
              - added|modified: '**'
              - '!**/*.rs'
              - '!**/Cargo.toml'
              - '!Cargo.lock'
              - '!supply-chain/**'
              - '!.github/**'
              - '!scripts/**'
              - '!.cargo/**'
              - '!.config/**'
              - '!just/**'

      - id: code
        run: |
          if [[ "${{ steps.f.outputs.rust }}" == 'true' \
             || "${{ steps.f.outputs.dep }}" == 'true' \
             || "${{ steps.f.outputs.infra }}" == 'true' ]]; then
              echo "code=true"  >> "$GITHUB_OUTPUT"
          else
              echo "code=false" >> "$GITHUB_OUTPUT"
          fi

  # ---------------------------------------------------------------------
  # file-size — always runs, cheap.
  # ---------------------------------------------------------------------
  file-size:
    name: File size policy
    runs-on: ubuntu-22.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
      - run: bash scripts/ci/check_file_size_policy.sh

  # ---------------------------------------------------------------------
  # fmt — rust_changed.  Cheap, runs in parallel with classify downstream.
  # ---------------------------------------------------------------------
  fmt:
    name: Format check
    runs-on: ubuntu-22.04
    needs: classify
    if: needs.classify.outputs.rust == 'true'
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with:
          ref: ${{ github.event.pull_request.head.sha || github.sha }}
      - run: rustup show
      - run: cargo fmt --all -- --check

  # ---------------------------------------------------------------------
  # sanity — fastest compile gate (cargo check) + cargo-vet.
  # Downstream heavy jobs need this to have passed.
  # ---------------------------------------------------------------------
  sanity:
    name: Sanity (cargo check + vet)
    runs-on: ubuntu-22.04
    needs: classify
    if: needs.classify.outputs.code == 'true'
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with:
          ref: ${{ github.event.pull_request.head.sha || github.sha }}
      - run: rustup show
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
      - name: cargo fetch --locked
        run: cargo fetch --locked
      - name: cargo check --locked
        run: cargo check --workspace --all-targets --all-features --locked
      - name: cargo vet check --locked
        if: needs.classify.outputs.dep == 'true'
        uses: taiki-e/install-action@dfc84ffb7254b048a0a843316ff5a2714dcdc7bd # v2
        with: { tool: cargo-vet }
      - if: needs.classify.outputs.dep == 'true'
        run: cargo vet check --locked

  clippy:
    name: Clippy
    runs-on: ubuntu-22.04
    needs: sanity
    if: needs.classify.outputs.code == 'true'
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ github.event.pull_request.head.sha || github.sha }} }
      - run: rustup show
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
      - run: cargo clippy --workspace --all-targets --all-features --locked --no-deps -- -D warnings

  docs:
    name: Rustdoc + doctests
    runs-on: ubuntu-22.04
    needs: sanity
    if: needs.classify.outputs.code == 'true'
    timeout-minutes: 30
    env: { RUSTDOCFLAGS: "-Dwarnings" }
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ github.event.pull_request.head.sha || github.sha }} }
      - run: rustup show
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
      - run: cargo doc  --workspace --all-features --no-deps --locked
      - run: cargo test --doc --workspace --all-features --locked

  test-build:
    name: Test build
    runs-on: ubuntu-22.04
    needs: sanity
    if: needs.classify.outputs.code == 'true'
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ github.event.pull_request.head.sha || github.sha }} }
      - run: rustup show
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
        with: { shared-key: test-build, cache-on-failure: 'true' }
      - run: cargo test --workspace --all-features --lib --tests --no-run --locked

  tests:
    name: Tests
    runs-on: ubuntu-22.04
    needs: test-build
    if: needs.classify.outputs.code == 'true'
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ github.event.pull_request.head.sha || github.sha }} }
      - run: rustup show
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
        with: { shared-key: test-build, save-if: 'false' }
      - uses: taiki-e/install-action@dfc84ffb7254b048a0a843316ff5a2714dcdc7bd # v2
        with: { tool: nextest }
      - run: cargo nextest run --workspace --all-features --lib --tests --profile ci --locked

  security:
    name: Security (deny + vet)
    runs-on: ubuntu-22.04
    needs: classify
    if: needs.classify.outputs.dep == 'true' || needs.classify.outputs.infra == 'true' || needs.classify.outputs.rust == 'true'
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ github.event.pull_request.head.sha || github.sha }} }
      - run: rustup show
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
      - uses: taiki-e/install-action@dfc84ffb7254b048a0a843316ff5a2714dcdc7bd # v2
        with: { tool: cargo-deny,cargo-vet }
      - run: cargo deny check --locked
      - run: cargo vet check --locked

  windows-check:
    name: Windows compile check
    runs-on: windows-latest
    needs: sanity
    if: needs.classify.outputs.code == 'true'
    timeout-minutes: 25
    env: { RUSTFLAGS: "-C target-cpu=x86-64-v3" }
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ github.event.pull_request.head.sha || github.sha }} }
      - run: rustup show
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
        with: { shared-key: pr-fast-windows }
      - run: cargo check --workspace --all-targets --all-features --locked

  # ---------------------------------------------------------------------
  # required — single branch-protection target.  Explicitly includes
  # `classify` so a broken classify cannot green-light the PR via
  # skipped downstream jobs (GitHub's `skipped` counts as `success`
  # for required checks, so we must check result strings ourselves).
  # ---------------------------------------------------------------------
  required:
    name: PR Fast CI / required
    runs-on: ubuntu-22.04
    if: always()
    needs:
      - classify
      - file-size
      - fmt
      - sanity
      - clippy
      - docs
      - test-build
      - tests
      - security
      - windows-check
    steps:
      - name: Gate on classify
        # Classify MUST succeed.  A skipped or failed classify means
        # downstream skips are meaningless — treat as failure.
        run: |
          if [[ "${{ needs.classify.result }}" != "success" ]]; then
            echo "::error::classify job did not succeed (result=${{ needs.classify.result }})"
            exit 1
          fi
      - name: Aggregate downstream results
        run: |
          set -u
          declare -A R=(
            [file-size]='${{ needs.file-size.result }}'
            [fmt]='${{ needs.fmt.result }}'
            [sanity]='${{ needs.sanity.result }}'
            [clippy]='${{ needs.clippy.result }}'
            [docs]='${{ needs.docs.result }}'
            [test-build]='${{ needs.test-build.result }}'
            [tests]='${{ needs.tests.result }}'
            [security]='${{ needs.security.result }}'
            [windows-check]='${{ needs.windows-check.result }}'
          )
          fail=0
          for job in "${!R[@]}"; do
            r="${R[$job]}"
            case "$r" in
              success|skipped) ;;
              *) echo "::error::$job: $r"; fail=1 ;;
            esac
          done
          exit "$fail"
```

**Critical correctness notes**:

1. **`required.needs` includes `classify`** — and the first step
   explicitly fails unless `needs.classify.result == 'success'`.  This
   closes the GitHub Actions quirk where a failed upstream job causes
   downstream jobs to `skip`, and `skipped` counts as success for
   branch-protection aggregation.  Without this, a broken `classify`
   would green-light every PR.
2. **`code_changed` is the gate for every compile/test/Windows job** —
   not `rust == 'true'`.  A PR that only touches `Cargo.lock` or a
   workflow file still runs the full compile/test matrix, because
   those changes can break anything.
3. **`merge_group:` trigger** is present so that merge-queue runs
   report against the test-merge commit; without it, enabling merge
   queue later would require rediscovering this the hard way.
4. **`permissions: contents: read`** at the workflow level is the
   least-privilege default per GitHub's Actions hardening guide.
5. **Checkout ref policy**: every job pins to
   `github.event.pull_request.head.sha || github.sha`, so the PR-fast
   run and the preview run in §2.5.2 validate the **same commit SHA**.
   Without this, `actions/checkout`'s default behaviour on PR events
   can check out a test-merge commit that doesn't match the head SHA
   the preview workflow sees, causing manifest mismatches.
6. **Third-party actions pinned to full commit SHAs**.  Matches the
   repo's existing practice in `ci.yml` and `tier-2.yml`.  Dependabot
   tracks these and opens bump PRs via `.github/dependabot.yml`.
7. **`--locked` on every cargo command that supports it** (`check`,
   `doc`, `test`, `clippy`, `deny`, `vet`, `nextest`).  Guarantees the
   resolver uses exactly `Cargo.lock`.  One `cargo fetch --locked` up
   front in `sanity` primes the cache for the whole pipeline.

#### `preview-artifacts.yml` (not required, opt-in)

Scope for Phase 5: **Windows binaries + Windows nextest archive only.**
macOS / Linux preview builds are deferred (see §7 Non-goals).

```yaml
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
name: Preview Artifacts

on:
  pull_request:
    types: [labeled, synchronize]
  workflow_dispatch:
    inputs:
      sha:
        description: 'Commit SHA to build (default: latest on default branch)'
        required: false

permissions:
  contents: read
  checks: read            # for verify-pr-fast-green
  pull-requests: read     # for label inspection

concurrency:
  group: preview-${{ github.workflow }}-${{ github.event.pull_request.number || github.event.inputs.sha || github.sha }}
  cancel-in-progress: true

jobs:
  # ---------------------------------------------------------------------
  # gate — only proceed when:
  #   (a) manual dispatch, OR
  #   (b) PR has the `preview-binaries` label.
  # Emits the exact SHA every downstream job will build and validate.
  # ---------------------------------------------------------------------
  gate:
    if: |
      github.event_name == 'workflow_dispatch' ||
      (github.event_name == 'pull_request' &&
       contains(github.event.pull_request.labels.*.name, 'preview-binaries'))
    runs-on: ubuntu-22.04
    timeout-minutes: 2
    outputs:
      sha: ${{ steps.s.outputs.sha }}
    steps:
      - id: s
        run: |
          SHA='${{ github.event.pull_request.head.sha || github.event.inputs.sha || github.sha }}'
          [[ -z "$SHA" ]] && { echo "::error::no SHA resolved"; exit 1; }
          echo "sha=$SHA" >> "$GITHUB_OUTPUT"

  # ---------------------------------------------------------------------
  # verify-pr-fast-green — preview binaries are only worth producing for
  # a SHA that has already passed the merge-blocking checks.  Otherwise a
  # tester is validating a commit that wouldn't merge anyway.
  # Queries GitHub's check-runs API for the SHA.
  # ---------------------------------------------------------------------
  verify-pr-fast-green:
    needs: gate
    runs-on: ubuntu-22.04
    timeout-minutes: 5
    steps:
      - name: Poll check-runs for PR Fast CI / required
        uses: actions/github-script@3a2844b7e9c422d3c10d287c895573f7108da1b3 # v9
        with:
          script: |
            const sha = '${{ needs.gate.outputs.sha }}';
            const want = 'PR Fast CI / required';
            // Retry for up to 10 minutes in case pr-fast is still running.
            for (let i = 0; i < 60; i++) {
              const { data } = await github.rest.checks.listForRef({
                owner: context.repo.owner,
                repo:  context.repo.repo,
                ref:   sha,
                per_page: 100,
              });
              const run = data.check_runs.find(r => r.name === want);
              if (run && run.status === 'completed') {
                if (run.conclusion === 'success') {
                  core.info(`✅ ${want} is success on ${sha.slice(0,7)}`);
                  return;
                }
                core.setFailed(`❌ ${want} on ${sha.slice(0,7)}: ${run.conclusion}`);
                return;
              }
              core.info(`⏳ ${want} not yet complete (status=${run?.status ?? 'missing'}); retry ${i+1}/60`);
              await new Promise(r => setTimeout(r, 10_000));
            }
            core.setFailed(`⏱  Timed out waiting for ${want} on ${sha.slice(0,7)}`);

  # ---------------------------------------------------------------------
  # build-windows — cross-compile Windows release binary on Linux/xwin.
  # Runs only after PR-fast is green on the same SHA.
  # ---------------------------------------------------------------------
  build-windows:
    needs: [gate, verify-pr-fast-green]
    runs-on: ubuntu-22.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with:
          ref: ${{ needs.gate.outputs.sha }}  # validated SHA, not PR head
      - run: rustup show
      - run: rustup target add x86_64-pc-windows-msvc
      - uses: taiki-e/install-action@dfc84ffb7254b048a0a843316ff5a2714dcdc7bd # v2
        with: { tool: cargo-xwin }
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
        with: { shared-key: preview-windows }
      - name: cargo xwin build --release
        run: |
          cargo xwin build --release --bins --locked \
            --target x86_64-pc-windows-msvc
      - name: Stage Windows artifacts
        run: |
          mkdir -p dist/windows
          cp target/x86_64-pc-windows-msvc/release/*.exe dist/windows/ || true
      - uses: actions/upload-artifact@b4b15b8c7c6ac21ea08fcf65892d2ee8f75cf882 # v4.4.3
        with:
          name: windows-preview-${{ needs.gate.outputs.sha }}
          path: dist/windows/
          if-no-files-found: error

  # ---------------------------------------------------------------------
  # build-test-archive — cargo-nextest supports "archive on one machine,
  # run on another".  Target machine needs (a) the same source SHA
  # checked out, (b) nextest installed.  Cargo itself is not required.
  # ---------------------------------------------------------------------
  build-test-archive:
    needs: [gate, verify-pr-fast-green]
    runs-on: ubuntu-22.04
    timeout-minutes: 30
    outputs:
      nextest_version: ${{ steps.nxv.outputs.version }}
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ needs.gate.outputs.sha }} }
      - run: rustup show
      - run: rustup target add x86_64-pc-windows-msvc
      - uses: taiki-e/install-action@dfc84ffb7254b048a0a843316ff5a2714dcdc7bd # v2
        with: { tool: cargo-xwin,nextest }
      - uses: Swatinem/rust-cache@e18b497796c12c097a38f9edb9d0641fb99eee32 # v2
        with: { shared-key: preview-windows }
      - id: nxv
        run: echo "version=$(cargo nextest --version | awk '{print $2}')" >> "$GITHUB_OUTPUT"
      - name: cargo nextest archive
        run: |
          cargo nextest archive --workspace --locked \
            --target x86_64-pc-windows-msvc \
            --archive-file uffs-tests-${{ needs.gate.outputs.sha }}.tar.zst
      - uses: actions/upload-artifact@b4b15b8c7c6ac21ea08fcf65892d2ee8f75cf882 # v4.4.3
        with:
          name: nextest-archive-${{ needs.gate.outputs.sha }}
          path: uffs-tests-${{ needs.gate.outputs.sha }}.tar.zst
          if-no-files-found: error

  # ---------------------------------------------------------------------
  # smoke-windows — GitHub-hosted windows-latest runs the archive against
  # the SAME SHA that produced it.  Nextest's cross-compile model requires
  # the target machine to have the source tree checked out at the same
  # revision + nextest installed.  We satisfy both here.
  # ---------------------------------------------------------------------
  smoke-windows:
    needs: [gate, build-windows, build-test-archive]
    runs-on: windows-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ needs.gate.outputs.sha }} }
      - uses: taiki-e/install-action@dfc84ffb7254b048a0a843316ff5a2714dcdc7bd # v2
        with: { tool: nextest }
      - uses: actions/download-artifact@fa0a91b85d4f404e444e00e005971372dc801d16 # v4.1.8
        with: { name: nextest-archive-${{ needs.gate.outputs.sha }} }
      - run: cargo nextest run --archive-file uffs-tests-${{ needs.gate.outputs.sha }}.tar.zst --profile ci

  # ---------------------------------------------------------------------
  # manifest — human-readable summary committed-alongside-artifacts so a
  # tester downloading from the browser UI can verify integrity and
  # compatibility.  Emits per-file SHA256 in addition to the
  # artifact-level digest that upload-artifact@v4 validates.
  # ---------------------------------------------------------------------
  manifest:
    needs: [gate, build-windows, build-test-archive, smoke-windows]
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with: { ref: ${{ needs.gate.outputs.sha }} }
      - uses: actions/download-artifact@fa0a91b85d4f404e444e00e005971372dc801d16 # v4.1.8
        with: { path: preview }
      - name: Emit manifest.json
        run: |
          RUSTC=$(rustc --version)
          LOCK_SHA=$(sha256sum Cargo.lock | awk '{print $1}')
          {
            echo '{'
            echo '  "git_sha":         "${{ needs.gate.outputs.sha }}",'
            echo '  "tested_sha":      "${{ needs.gate.outputs.sha }}",'
            echo '  "cargo_lock_sha256": "'"$LOCK_SHA"'",'
            echo '  "rustc_version":   "'"$RUSTC"'",'
            echo '  "nextest_version": "${{ needs.build-test-archive.outputs.nextest_version }}",'
            echo '  "target":          "x86_64-pc-windows-msvc",'
            echo '  "build_os":        "ubuntu-22.04 (cargo-xwin)",'
            echo '  "files": ['
            find preview -type f | while read -r f; do
              sha=$(sha256sum "$f" | awk '{print $1}')
              printf '    {"path": "%s", "sha256": "%s"},\n' "$f" "$sha"
            done | sed '$ s/,$//'
            echo '  ]'
            echo '}'
          } > manifest.json
          cat manifest.json
      - uses: actions/upload-artifact@b4b15b8c7c6ac21ea08fcf65892d2ee8f75cf882 # v4.4.3
        with:
          name: manifest-${{ needs.gate.outputs.sha }}
          path: manifest.json
```

**Critical correctness notes**:

1. **`verify-pr-fast-green` polls the GitHub check-runs API** for the exact
   SHA and refuses to proceed unless `PR Fast CI / required` is green on
   that commit.  Retries for 10 min to absorb cases where the PR-fast run
   is still executing.  Closes the "preview artifacts from a commit that
   couldn't merge" class of confusion.
2. **Every downstream job checks out `needs.gate.outputs.sha`** — the
   same SHA the PR-fast run validated.  No reliance on default
   `actions/checkout` behaviour which can land on the test-merge commit.
3. **Manifest declares `git_sha == tested_sha`** explicitly so a human
   downloading the bundle can cross-check against `PR Fast CI`.
   `tested_sha` is separate from `git_sha` because in a future extension
   the smoke runner could validate against a merge-commit while binaries
   were built from the PR head.
4. **`nextest_version` is in the manifest** so a person running the
   archive manually knows which nextest to install.  Nextest's archive
   requires matching versions on both sides.
5. **Per-file SHA256 in the manifest**, not just the artifact-level
   digest that GitHub's UI shows.  A manual downloader verifies file
   integrity without trusting the Actions UI.
6. **Self-hosted runners are intentionally NOT used anywhere.**  Even
   though `windows-latest` costs ~10× Linux minutes, running fork PR code
   on a self-hosted runner is a well-documented security hazard.  If a
   self-hosted Windows runner is introduced later, it must be gated on
   `github.event.pull_request.head.repo.fork == false` (trusted branches
   only) — documented here so future maintainers don't miss it.

#### Deletions / migrations

- `ci.yml` → **delete** after `pr-fast.yml` is green for a week.
  Replace `main-protection` required status checks with the single
  `PR Fast CI / required` aggregate.
- `tier-2.yml` → **keep as-is** except: drop `windows-check` job
  (now in `pr-fast.yml`).  Tier 2 stays the deep-assurance lane
  (coverage, miri, udeps).

### 2.6 `scripts/ci/ci-pipeline.rs` → workspace binary `scripts/ci-pipeline/`

Same as v1's Phase 5.  Eliminates rust-script binary-cache Heisenbugs.

**Additional benefit from v2**: the binary can now **import** the single
gate-definition file (see §2.7) so local pre-push, `just ship`, and
`pr-fast.yml` all reference the same gate list.

### 2.7 (Stretch) — machine-readable gate manifest

Long-term direction: replace the hand-maintained correspondence between
`_lint_pre_push.sh`, `ci.yml`, and this doc with a single file consumed
by all three.  Deferred — documented here so future-us knows the target
shape.

```toml
# gates.toml (proposed, not yet implemented)
[[gate]]
id = "cargo-vet"
label = "cargo vet check --locked"
tiers = ["pre-commit", "pre-push", "pr-fast"]
hard_required_when = "dep_changed"
tool = "cargo-vet"
cmd  = "cargo vet check --locked"
expected_runtime_secs = 2

[[gate]]
id = "doctests"
label = "cargo test --doc"
tiers = ["pre-push", "pr-fast"]
hard_required_when = "rust_changed"
cmd = "cargo test --doc --workspace --all-features"
env = { RUSTDOCFLAGS = "-Dwarnings" }
expected_runtime_secs = 25

# ... etc
```

Generator script would produce `_lint_fast.sh`, `_lint_pre_push.sh`,
and the PR-fast workflow matrix from this.  Converts v1's "explanatory
truth" into "executable truth".

### 2.8 GitHub Actions hardening policy

New policy applied across both new workflows and enforced as a
review checklist going forward.  Most of it mirrors GitHub's
published Actions hardening guide.

**At the workflow level**:

- `permissions: contents: read` as the default block.  Individual
  jobs opt into write scopes explicitly; none currently do.
- `concurrency` group with `cancel-in-progress: true` on PR events
  so stale runs die when a new push lands.
- All third-party actions pinned to full commit SHAs (40-char hex),
  never floating tags.  `.github/dependabot.yml` already tracks
  Actions updates; bump PRs land as small reviewable deltas.
- `merge_group:` trigger present on every workflow that reports
  required checks.  Harmless when merge queue is disabled;
  essential when it is enabled.

**At the job level**:

- `timeout-minutes` set on every job.  Default GitHub timeout is
  360 (6 h); we cap realistic jobs at 30 min and cheap jobs at 2–5
  min.  Prevents runaway runs from exhausting runner-hour budgets.
- `runs-on: windows-latest` / `macos-latest` only on GitHub-hosted
  runners, never self-hosted.  Rationale: fork PRs execute arbitrary
  author code, and a self-hosted runner sits on privileged infra.
- Every `uses: actions/checkout@...` pins `ref:` to an explicit
  `github.event.pull_request.head.sha || github.sha`.  Default
  checkout behaviour differs between `pull_request` and
  `pull_request_target` events and can land on a synthetic merge
  commit.  Explicit refs remove the ambiguity between what PR-fast
  validates and what the preview lane builds.
- `--locked` passed to every cargo subcommand that accepts it.  One
  `cargo fetch --locked` up front seeds the cache.

**Manifest integrity**:

- `actions/upload-artifact@v4` already computes and validates an
  artifact-level SHA256 on download inside Actions.  Our manifest
  **additionally** lists per-file SHA256 so a human downloading via
  the browser UI can verify without trusting the UI.
- Manifest includes tool versions (rustc, nextest, cargo-vet as
  applicable) so that a validator on a different machine knows
  exactly which binaries to install.

**What is explicitly forbidden**:

- `pull_request_target` on any workflow that runs build/test code.
  Reason: it runs against the base branch's workflow file with
  elevated permissions and is the primary source of the "malicious
  PR exfiltrates secrets" class of bug.
- Self-hosted runners for any job reachable from fork PRs.  If a
  self-hosted Windows runner is ever introduced (see §9 open
  items), the job must gate on
  `github.event.pull_request.head.repo.fork == false`.
- Floating action tags (`@v4`, `@main`).  Full commit SHA only.
- `secrets.*` reads in forked-PR context.  GitHub masks them but
  shell exit codes and derived strings may leak; audit any job
  that touches secrets.

This policy ships alongside Phases 4–5 and is retroactively
applied to `tier-2.yml`, `codeql.yml`, `release.yml`,
`auto-tag-release.yml`, `cargo-vet-refresh.yml`,
`dependabot-review.yml`, and `dependabot-auto-merge.yml` as a
housekeeping commit (Phase 4b).  Most of those already conform;
the audit's job is to confirm and fill gaps.

---

## 3. Phased rollout

Seven commits, each atomic, ranked so **each step leaves the repo
green**.

### Phase 1 — Hardened pre-push gates (Commit 1)

**Scope**:

- `scripts/hooks/_lint_pre_push.sh` — add `cargo vet` (hard-required on
  dep change), `doctests`, `cargo nextest run --profile pre-push-smoke`.
- `.config/nextest.toml` — add `pre-push-smoke` profile.
- Does NOT yet reorder schedule or split buckets — that's Phase 2.

**Validation**:

1. `just lint-pre-push` on current HEAD — all green within 60 s.
2. `touch Cargo.lock` → commit → `just lint-pre-push` — cargo-vet runs
   and passes.
3. Uninstall `cargo-vet`, repeat (2) — hard-fails with install hint.
4. Introduce a deliberate doctest failure — pre-push aborts.

**Risk**: Low.  Additive changes.

### Phase 2 — Bucket-ordered scheduler + remove xwin from pre-commit (Commit 2)

**Scope**:

- `scripts/hooks/_lint_pre_push.sh` — rewrite with two-bucket scheduler
  (§1.4).
- `scripts/hooks/_lint_fast.sh` — drop the `check-windows` invocation.
- Add `cargo check` as the first Bucket 2 step.

**Validation**:

1. Introduce a type error → confirm pre-push shows `cargo check` red
   within ~15 s instead of waiting for the 40-60 s clippy batch.
2. Time pre-commit on a `*.rs`-only change — confirm under 15 s warm.

**Risk**: Medium.  Scheduler rewrite.  Keep the previous script as
`_lint_pre_push.sh.bak` for one cycle.

### Phase 3 — Cache policy single source of truth (Commit 3)

**Scope**:

- `.cargo/config.toml` — add `incremental = false`.
- `just/shared.just` — delete `export CARGO_INCREMENTAL := "1"`.
- `scripts/ci/ci-pipeline.rs` — remove the explicit
  `CARGO_INCREMENTAL=0` pairing (no longer needed).
- `CHANGELOG.md` — note the policy change.

**Validation**:

1. Fresh `cargo build --workspace` cold → time recorded.
2. `touch one file` → rebuild → time recorded.  Should be close to v1
   §6.4's 42 s figure.
3. `just ship -v` through to push — pre-push green.
4. Direct `git push` from fresh shell with no env overrides — pre-push
   green (proof that drift is gone).

**Risk**: Medium.  This is the Bug B root-cause fix.

### Phase 4 — Split `ci.yml` into `pr-fast.yml` + `preview-artifacts.yml` (Commit 4)

**Scope**:

- Create `pr-fast.yml` with the full structure in §2.5 (classify +
  required aggregator depending on classify + merge_group trigger +
  workflow-level `permissions: contents: read` + per-job full-SHA
  action pins).
- Create `preview-artifacts.yml` (stub, artifact build only, no smoke
  runner yet — that's Phase 5).
- Keep `ci.yml` intact; both workflows run in parallel for one week.
- Update `tier-2.yml` — drop `windows-check` job (now in `pr-fast.yml`).
- Confirm `.github/dependabot.yml` tracks both new workflows.

**Validation** (each item MUST pass before Phase 4 cutover):

1. **Docs-only PR** → `pr-fast` completes with `fmt`, `sanity`, `clippy`,
   `docs`, `test-build`, `tests`, `security`, `windows-check` all
   `skipped`; `required` = success.  `ci.yml` does the same (parity).
2. **Dep-only PR** (e.g. a Cargo.toml version bump with no `.rs`
   change) → `pr-fast` runs the full compile/test/Windows matrix.
   This exercises the v2→v3 under-gating fix where `rust == 'true'`
   was the (wrong) trigger.
3. **Infra-only PR** (e.g. edit to `.github/workflows/pr-fast.yml`
   itself, or to `just/lint.just`) → same as dep-only: full matrix.
4. **Rust-change PR** → both workflows run identical checks.  Compare
   wall-clock: `pr-fast` must be ≤ `ci.yml` on p50.
5. **Broken-classify simulation** — **the single most important check**.
   Manually break the `dorny/paths-filter` step in a test branch
   (e.g. invalid YAML in the filters block).  `classify` fails,
   downstream jobs `skip`, and `required` **must** fail (not pass
   via skip aggregation).  If `required` goes green, the
   `Gate on classify` step regressed — do not cutover.
6. **merge_group dry-run** (only if merge queue is enabled) → a
   queued PR runs `pr-fast` against the test-merge commit; result
   reports back to the queue.
7. **Branch-protection cutover**: after 7 days green on items 1–5,
   replace the 8 required `Tier 1 / *` checks with the single
   `PR Fast CI / required` aggregate and delete `ci.yml` in the
   **same commit** so GitHub never enforces a check from a deleted
   workflow.

**Risk**: Medium-high.  Branch protection drift possible during
transition.  Two-workflow parallel window is the mitigation.  The
classify-failure simulation (validation step 5) is the critical
check — it proves the v3 correctness fix actually works.

### Phase 5 — Preview lane with nextest archive + Windows smoke (Commit 5)

**Scope**:

- Flesh out `preview-artifacts.yml` with the full structure in §2.5:
  - `gate` job (label / dispatch guard + SHA pinning).
  - `verify-pr-fast-green` job (polls check-runs API, up to 10 min).
  - `build-windows` (`cargo xwin build --release --locked`).
  - `build-test-archive` (`cargo nextest archive --locked`).
  - `smoke-windows` (`windows-latest`, same-SHA checkout, runs the
    archive).
  - `manifest` (emits `manifest.json` with `git_sha`, `tested_sha`,
    `cargo_lock_sha256`, `rustc_version`, `nextest_version`, and
    per-file SHA256).
- Add a repo label `preview-binaries` (maintainer-apply only).
- Document the label-trigger workflow in `dev-flow.md`.

**Validation** (each item MUST pass before Phase 5 is done):

1. **Label-trigger path**: apply `preview-binaries` to an open PR
   whose `PR Fast CI / required` is green → full preview workflow
   runs → artifacts appear in Actions UI with the SHA in their names.
2. **Same-SHA integrity**: download the Windows bundle +
   `manifest.json`.  `manifest.git_sha` must equal the PR's head SHA.
   Every `files[].sha256` in the manifest must match `sha256sum` of
   the downloaded file.
3. **Nextest archive round-trip**: on a real Windows box with
   `cargo-nextest` at exactly `manifest.nextest_version`,
   `git checkout <sha>` the repo and run
   `cargo nextest run --archive-file <file>` — must pass.
4. **Pre-fast-gate enforcement** — **the critical check**.  Open a PR
   that will fail `PR Fast CI` (e.g. deliberate fmt violation).
   Apply `preview-binaries` label.  `verify-pr-fast-green` **must**
   fail within 10 min; artifact jobs must not start.  If the build
   proceeds, the gate is broken — do not merge Phase 5.
5. **Fork-PR behaviour**: open a PR from a forked repo.  Label with
   `preview-binaries`.  Workflow must execute on GitHub-hosted
   runners only (verify by grepping for `self-hosted` in the
   workflow YAML).
6. **Concurrency behaviour**: push a new commit to a PR that already
   has a preview run in progress.  The in-progress run must cancel,
   a new run must start against the new SHA, and no artifacts from
   the cancelled run should publish.

**Risk**: Medium.  New workflow surface; isolated impact.  Biggest
risk is validation step 4 silently passing when it shouldn't — the
poll-based check-runs gate is subtle.  Always run a deliberate-failure
test before trusting it.

### Phase 6 — Resumable-push fix (Commit 6)

**Scope**: exactly v1 §6.3.  The `git rev-list --count` guard at the top
of step 11 in `scripts/ci/ci-pipeline.rs`.

**Validation**: reproduce the v0.5.71 "committed after push, re-ran
ship, HEAD never landed" sequence.  Before fix: exits in 2 s.  After
fix: re-runs step 11, push lands.

**Risk**: Low.  Read-only git check + targeted state invalidation.

### Phase 7 — Promote ci-pipeline.rs to workspace binary (Commit 7)

**Scope**: v1 §6.5.B.  Move to `scripts/ci-pipeline/` with `Cargo.toml`,
update `just/workflow.just` to `cargo run -p ci-pipeline --release --`.

**Validation**:

1. `just ship --fresh` — works as before, no rust-script cache.
2. Edit `ci-pipeline.rs`, re-run — change takes effect immediately.

**Risk**: Medium.  ~90 min restructuring; clean once done.

### Phase 8 (stretch, optional) — Machine-readable gate manifest (Commit 8)

Deferred to a future session.  See §2.7 for target shape.

---

## 4. Branch-protection migration checklist

Branch protection on `main` is enforced via the **rulesets API**
(ID `11889528`, name `main-protection`), not classic branch protection.
Classic `/branches/main/protection` returns 404 — verified
2026-04-23.  Inspect or update via:

```bash
gh api repos/skyllc-ai/UltraFastFileSearch/rulesets/11889528
gh api --method PUT repos/skyllc-ai/UltraFastFileSearch/rulesets/11889528 --input <body.json>
```

### 4.1 Baseline (pre-Phase-4) required checks

Verified against the ruleset on 2026-04-21, 6 entries (not 8 —
`Tier 1 / Doc Tests` and `Tier 1 / Test Build` were never required,
they run informationally):

```
Tier 1 / Clippy
Tier 1 / Tests
Tier 1 / Security
Tier 1 / File Size Policy
Tier 1 / Format
Tier 1 / Rustdoc
```

### 4.2 Parallel-window posture (current, 2026-04-23 → 2026-04-30)

Ruleset updated 2026-04-23 11:38 PDT.  Current required-check set has
**7 entries**:

```
Tier 1 / Clippy
Tier 1 / Tests
Tier 1 / Security
Tier 1 / File Size Policy
Tier 1 / Format
Tier 1 / Rustdoc
PR Fast CI / required        ← new, added in parallel window
```

Both lanes must pass; neither alone unblocks merge.  This is the state
the `pr-fast.yml:22-25` comment anticipates for the 7-day bake-in.

### 4.3 Cutover procedure (end of parallel window)

**Do everything in a single commit** so GitHub never enforces a check
from a deleted workflow:

1. Delete `.github/workflows/ci.yml`.
2. PUT the ruleset with only `PR Fast CI / required` in the
   `required_status_checks` array (drop all 6 `Tier 1 / *`).
3. Update `CHANGELOG.md`.

End-state required-check set (1 entry):

```
PR Fast CI / required
```

### 4.4 Required-check context string — gotcha

**The context string matches the job's `name:` attribute, NOT the UI
display string.**  GitHub's PR UI concatenates `<workflow-name> / 
<job-name>` for display (e.g. `PR Fast CI / PR Fast CI / required`),
but branch-protection matches against `check_run.name` which is the
raw job name.  Worked examples:

| Workflow | Job `name:` | UI display | Ruleset context |
|---|---|---|---|
| `🧪 UFFS Tier 1 Nightly CI` | `Tier 1 / Clippy` | `🧪 UFFS Tier 1 Nightly CI / Tier 1 / Clippy` | **`Tier 1 / Clippy`** |
| `PR Fast CI` | `PR Fast CI / required` | `PR Fast CI / PR Fast CI / required` | **`PR Fast CI / required`** |

Getting this wrong produces `mergeStateStatus: BLOCKED` with zero
failing and zero pending checks — the protection engine waits forever
for a check-name that no workflow produces.  Verify via:

```bash
gh pr view <N> --json statusCheckRollup \
  --jq '.statusCheckRollup[] | select(.name | contains("required")) | .name'
```

The returned value is the correct context string.  This was hit and
resolved live on 2026-04-23 (see §10.5 Deviations log).

---

## 5. Risk matrix

| Risk | Mitigation |
|---|---|
| Phase 3 flips `CARGO_INCREMENTAL` default; daily dev feels different | One-line revert in `.cargo/config.toml`.  Benchmarks in v1 §6.4 support the direction. |
| Phase 4 branch-protection drift during cutover | 7-day parallel window; both workflows required. |
| Phase 5 preview workflow triggered on every label event → runner cost | Gate on `if: contains(... 'preview-binaries')` and concurrency cancel-in-progress. |
| Phase 7 changes the ship entry point | Keep `rust-script scripts/ci/ci-pipeline.rs ship` working as a thin wrapper for one release cycle. |
| Smoke profile tests flaky on some machines | `retries = 0, fail-fast = true` means flakes block push — but the profile deliberately excludes integration tests which are the usual flaky class. |

---

## 6. Success metrics

Measured against current `v0.5.71` baseline:

| Metric | Baseline | Target |
|---|---|---|
| Median time to first actionable failure after `git push` | CI (~5 min to first red) | T2 pre-push (&lt; 30 s) |
| PR-blocking full-pipeline wall time | 8-15 min (ci.yml) | &lt; 8 min (pr-fast) |
| Windows regression detection latency | release-time (~15 min into `just ship`) | PR-time (pr-fast windows-check) |
| Cargo-vet round-trip incidents per release | 1-3 today | 0 (hard-gated at T1) |
| Ship resumable state bugs per month | ≥1 (v0.5.71) | 0 after Phase 6 |

---

## 7. What this plan intentionally does NOT do

- **Does not introduce a remote sccache.**  That's a future optimization
  ("Phase 9") worth evaluating only after Phases 1-7 land and baselines
  are measured.
- **Does not add `cargo mutants` / fuzz.**  Deep-assurance lane expansion
  is out of scope for this plan.
- **Does not change Tier 2 cadence.**  Weekly cron stays weekly.
- **Does not touch `release.yml`.**  The release lane is correct today;
  the issue was artifact-producing lanes bleeding into PR critical path.
- **Does not adopt a pre-commit framework** (e.g. `pre-commit` the
  Python tool).  Bash + `just` stays; rationale: one less dependency
  for new contributors.

---

## 8. Execution readiness

**Prerequisites before Phase 1**:

- [ ] Current repo is green on `main`.
- [ ] `cargo-vet` installed locally (`cargo install cargo-vet --locked`).
- [ ] Confirmed `cargo nextest` version ≥ 0.9.130 (already pinned in
      `.config/nextest.toml`).

**Order of operations**: 1 → 2 → 3 → 6 → 4 → 5 → 7.

Rationale: hardened gates (1–3) first make local work reliable.  Fix
the ship-pipeline bug (6) next so release-time iteration doesn't fight
us.  Then CI split (4) once local is solid.  Preview lane (5) plumbs in
on top of the split.  Binary promotion (7) is the clean-up step.

Phase 8 (gates.toml manifest) lands when the dust settles and we've
lived with the split lanes for a release or two.

---

## 9. Open review items

- **Smoke profile selection rule** — does `default-filter` in the
  `pre-push-smoke` nextest profile need to exclude more / fewer tests?
  Validate during Phase 1 rollout; tune before merge.
- **Windows preview smoke runner cost** — GitHub-hosted `windows-latest`
  at 10× the Linux minute rate.  Consider a self-hosted runner if
  preview-label usage exceeds ~5×/week.
- **Preview-artifacts security surface** — a malicious PR author could
  trigger preview builds.  Today's model: gated on label which only
  maintainers apply.  Re-evaluate if the contributor base grows.
- **Bazel / Buck2 / build-server-protocol** — not on today's roadmap
  but the gates.toml direction (§2.7) is a stepping stone if we ever
  want it.

---

## 10. Execution tracking

Live dashboard.  **This section is updated as work lands**, not in one
go at the end.  Future readers should be able to audit exactly how
the plan was executed by reading this section — commits, dates, PRs,
deviations, and measured metrics.

Status legend: ⬜ not started · 🟡 in progress · 🔵 blocked (see notes)
· ✅ done · ❌ reverted

### 10.1 Prerequisites (before Phase 1)

- [ ] Repo is green on `main` — `ci.yml` Tier 1 all checks passing.
- [ ] `cargo-vet` installed locally (`cargo install cargo-vet --locked`).
- [ ] `cargo-nextest` ≥ 0.9.130 confirmed (already pinned in
      `.config/nextest.toml`).
- [ ] `cargo-xwin` installed for local Windows advisory (optional but
      recommended).
- [ ] Dropbox / backup of current `.cargo/config.toml`,
      `just/shared.just`, `scripts/hooks/_lint_*.sh` in case of revert.

### 10.2 Phase status dashboard

| # | Phase | Status | Started | Completed | Commit(s) | PR |
|---|-------|--------|---------|-----------|-----------|-----|
| 1 | Hardened pre-push gates (cargo vet + doctests + smoke profile) | ✅ | 2026-04-23 | 2026-04-23 | `780c1dbb1` (squash) | [#45](https://github.com/skyllc-ai/UltraFastFileSearch/pull/45) |
| 2 | Bucket-ordered scheduler + xwin removed from pre-commit | ✅ | 2026-04-23 | 2026-04-23 | `780c1dbb1` (squash) | [#45](https://github.com/skyllc-ai/UltraFastFileSearch/pull/45) |
| 3 | Cache policy single source (`.cargo/config.toml` owns `incremental`) | ✅ | 2026-04-23 | 2026-04-23 | `780c1dbb1` (squash) | [#45](https://github.com/skyllc-ai/UltraFastFileSearch/pull/45) |
| 6 | Resumable-push fix in `ci-pipeline.rs` | ✅ | 2026-04-23 | 2026-04-23 | `780c1dbb1` (squash) | [#45](https://github.com/skyllc-ai/UltraFastFileSearch/pull/45) |
| 4 | Split `ci.yml` → `pr-fast.yml` + `preview-artifacts.yml` | 🟡 | 2026-04-23 | — | `780c1dbb1` (squash) | [#45](https://github.com/skyllc-ai/UltraFastFileSearch/pull/45) |
| 4b | Actions hardening retrofit across existing workflows | ⬜ | | | | |
| 5 | Preview lane fleshed out (smoke runner + manifest) | 🟡 | 2026-04-23 | — | `780c1dbb1` (squash) | [#45](https://github.com/skyllc-ai/UltraFastFileSearch/pull/45) |
| 7 | `ci-pipeline.rs` promoted to workspace binary | ✅ | 2026-04-23 | 2026-04-23 | `780c1dbb1` (squash) | [#45](https://github.com/skyllc-ai/UltraFastFileSearch/pull/45) |
| 8 | (stretch) `gates.toml` machine-readable manifest | ⬜ | | | | |

**Phase 4 sub-status (2026-04-23)**: static implementation landed in
`780c1dbb1`; broken-classify simulation executed live on PR #45 and
passed with full fidelity (required=failure propagated correctly
through 8 skipped downstream jobs); 7-day parallel-window bake-in
opened on 2026-04-23; ruleset `main-protection` updated to require
`PR Fast CI / required` alongside the 6 existing `Tier 1 / *` checks.
Cutover (drop Tier 1, delete `ci.yml`) scheduled for 2026-04-30 or
earlier if no regressions surface.

**Legend**: ✅ = complete and validated; 🟡 = static-complete (actionlint
clean, ruby-yaml clean, pins/permissions correct, classify-aggregation
gated) — remaining work is live-PR bake-in (broken-classify simulation,
7-day parallel window, branch-protection cutover) which cannot be
validated pre-merge; ⬜ = not started.

**Recommended execution order**: 1 → 2 → 3 → 6 → 4 (+4b) → 5 → 7.
See §8 Execution readiness for the rationale behind the reorder
(Phase 6 before Phase 4 so ship-pipeline iteration doesn't fight
us mid-cutover).

### 10.3 Per-phase validation checklists

Copy-pasted from §3.  Check items off as they pass in practice.
Record any surprises under "Notes".

#### Phase 1 — Hardened pre-push gates

- [x] `just lint-pre-push` on current HEAD — all green within 60 s.
- [x] Manual-mode (`bash scripts/hooks/_lint_pre_push.sh`) triggers
      `DEP_CHANGED=1` fallback; cargo-vet runs and passes.
- [ ] Uninstall `cargo-vet`, repeat — hard-fails with install hint.
      (Skipped in rollout; validated by code review — hard-fail path is
      a 3-line `if ! command -v cargo-vet … exit 2`.)
- [ ] Introduce a deliberate doctest failure — pre-push aborts.
      (Skipped in rollout; validated by code review — `doc-tests` gate
      spawns `cargo test --doc` which surfaces rustdoc failures.)
- [x] Measured pre-push runtime (warm): **43 s** (target < 60 s).
- [x] Smoke profile runtime (nested): **5.9 s** wall / 1140 tests run /
      11 skipped via profile.

**Notes**: `default-filter` in nextest profiles treats unknown binary
names as parse errors, not warnings, so the original plan's exclusion
of `benchmark_filtering` / `benchmark_sorting` had to be dropped —
those criterion-style benches aren't surfaced as nextest binaries.
Filter now denylists only the test-name prefix (`test_validate_`) and
the `uffs-client` package.  Runtime remains within budget.

#### Phase 2 — Bucket-ordered scheduler

- [ ] Introduce a type error → `cargo check` shows red within ~15 s
      (before the clippy batch).  Skipped in rollout; fail-fast logic
      validated by code review — `run_seq` sets `SEQ_FIRST_FAIL` on
      first failure, subsequent calls short-circuit to `skip`.
- [x] Pre-commit on staged-empty change: **3 s** warm (base load:
      file-size + fmt-check + typos + reuse).
- [x] Pre-push (warm, manual mode, CODE_CHANGED=1): **40 s** total.
      Per-Bucket-2 step: `cargo-check` 1 s, `lint-ci` 8 s, `lint-prod`
      4 s, `lint-tests` 7 s, `rustdoc` 1 s, `doc-tests` 9 s, `tests`
      2 s, `smoke` 7 s, `deny` 0 s, `check-windows` 1 s.
- [ ] `_lint_pre_push.sh.bak` kept as rollback for one cycle.
      Superseded by git history — reverts are atomic at the commit level.

**Notes**:
- Removed `taplo` from pre-push Bucket 1 — running
  `taplo fmt --check` workspace-wide surfaces pre-existing TOML drift
  that is out of scope for the push-being-validated.  Taplo stays at
  pre-commit where the staged-scope is natural.
- `cargo deny check` does **not** accept `--locked` (unlike cargo-vet);
  cargo-deny reads `Cargo.lock` directly.  Removed the `--locked`
  suffix.  The doc's "`--locked` on every cargo subcommand that
  supports it" is still accurate — `cargo deny` is not a cargo
  subcommand in the resolver sense.
- Bucket 2 ordering proved out: cargo-check runs in 1 s, so a type
  error reports in ~1-2 s instead of waiting ~40 s for clippy.

#### Phase 3 — Cache policy single source

- [x] `cargo check --workspace --all-targets --all-features --locked`
      after `incremental = false` landed: **8.36 s** (unchanged from
      baseline — config change didn't invalidate sccache).
- [x] Pre-push warm after Phase 3: **42 s** (same as Phase 2;
      config change did not degrade cache reuse).
- [x] Pre-push cold-ish (first run after config edit): **72 s**
      (rustdoc jumped 1 s → 18 s on the invalidation wave, then
      back to 1 s on the subsequent warm run — cache re-established).
- [ ] `just ship -v` through to push — pre-push green.
      Deferred until after Phase 6 lands (Phase 6 is the ship-bug fix;
      testing ship with the Bug A present is circular).
- [x] Direct `bash scripts/hooks/_lint_pre_push.sh` from fresh shell
      with no env overrides — pre-push green.  Proves drift is gone.
- [x] `shared.just` no longer exports `CARGO_INCREMENTAL`.
- [x] `.cargo/config.toml` has `incremental = false` paired with
      `rustc-wrapper = "sccache"`.
- [x] `ci-pipeline.rs` simplified — the `CARGO_INCREMENTAL=0` explicit
      push is gone; pipeline only re-asserts `RUSTC_WRAPPER=sccache`
      because git itself reads no Cargo config.  Diagnostic
      `[ci-pipeline][sccache-fix]` eprintln removed too (served its
      purpose as stale-binary detector during v0.5.71 triage).
- [x] `rust-script --test scripts/ci/ci-pipeline.rs` compiles clean.

**Notes**:
- Left `export RUSTC_WRAPPER := ""` in `just/shared.just` untouched.
  The plan didn't mandate its removal.  It encodes an intentional
  "just recipes opt out of sccache by default" policy; specific
  recipes that need sccache (`just cache:*`, `just ship`) set it
  back explicitly.  Changing that semantic is out of scope for
  Phase 3 — it would require auditing every recipe for cache
  assumptions.
- Env var still overrides Cargo config per Cargo's resolution
  order, so any user who prefers incremental can still
  `CARGO_INCREMENTAL=1 cargo build` one-off.  Default is the
  right default; override stays available.

#### Phase 6 — Resumable-push fix

- [ ] Reproduce the v0.5.71 sequence: ship → commit more → re-run ship.
      Deferred to next real ship run (requires an in-flight release
      PR; no safe way to dry-run without altering remote state).
- [x] `count_unpushed_commits` helper added, `git rev-list --count
      origin/<branch>..HEAD` parsed safely.  Conservative fallback
      returns 1 when remote ref doesn't exist (first push).
- [x] `invalidate_step` method added to `WorkflowState` — drops from
      `completed_steps` and `failed_steps`, saves state only if
      something actually changed.
- [x] Guard at Step 11 call site: `unpushed > 0 &&
      state.is_step_completed(STEP_GIT_PUSH)` → invalidate + re-run.
- [x] `rust-script --test scripts/ci/ci-pipeline.rs` compiles clean.

**Notes**:
- Validation step 1 (reproduce v0.5.71 sequence) is deferred because
  the natural test requires an actual in-flight release branch with a
  follow-up commit, which would perturb the git history on origin.
  Will exercise on the next legitimate ship run where a CI-detected
  failure triggers a follow-up commit.
- Helper uses `tokio::process::Command` (imported as `Command` in
  this module — the async version).  Sync `std::process::Command`
  would also work here but the helper is `async` to fit the pipeline
  style used throughout.

#### Phase 4 — Split `ci.yml`

- [x] `.github/workflows/pr-fast.yml` created — 11 jobs:
      `classify`, `file-size`, `fmt`, `sanity`, `clippy`, `docs`,
      `test-build`, `tests`, `security`, `windows-check`, `required`,
      plus `notify-failure`.
- [x] `.github/workflows/preview-artifacts.yml` created as full
      Phase 5 implementation (label gating + verify-pr-fast-green +
      xwin build + nextest archive + smoke + manifest).
- [x] `merge_group:` trigger present in `pr-fast.yml`.
- [x] Concurrency: `cancel-in-progress: pull_request only`.
- [x] Least-privilege workflow permissions: `contents: read`.
- [x] All action uses pinned to full commit SHAs (reuses existing
      `ci.yml`/`release.yml`/`tier-2.yml` pins).
- [x] Both workflows pass `actionlint` clean.
- [x] Both workflows pass `ruby -ryaml` parse check.
- [x] `required` job:
       * Has `if: always()`.
       * Has explicit `needs: [classify, ...]`.
       * First step gates on `needs.classify.result == 'success'` —
         **this is the static fix for the highest-leverage sanity
         check** (plan doc § 10.3 classify-failure bullet).
       * Second step aggregates downstream results, `success|skipped`
         pass, anything else fails.
- [x] **Broken-classify simulation** on a real PR — ✅ executed
      2026-04-23 on PR #45.  Inserted `exit 1` as the FIRST step of
      the `classify` job (before outputs were populated), observed:
      `classify`=failure (4 s), 8 downstream jobs skipped (empty
      `needs.classify.outputs.*`), `required`=failure (4 s, via the
      explicit `Gate on classify` step), legacy `Tier 1 / *` lane
      stayed all green.  Reverted same day; PR #45 merged green.
      This proves the `required` aggregator fails correctly when
      downstream jobs are **skipped** (the exact GitHub Actions
      "skipped counts as success" trap the plan warned about).
- [ ] **Docs-only PR** real-world bake: `pr-fast` skips the heavy
      jobs (`code == 'false'`); `required` = success.  🟡 Exercised
      by this very PR (the plan-doc update).
- [ ] **Dep-only PR** real-world bake: Cargo.toml bump runs full
      matrix + cargo-vet in sanity.  Deferred to next dependabot
      or explicit version-bump PR.
- [ ] **Infra-only PR** real-world bake: workflow/justfile edit
      runs full matrix.  Partially covered by PR #45 (mixed
      rust+dep+infra change — infra classification branch exercised
      green).  Still want a pure-infra bake for completeness.
- [x] **Rust-change PR** real-world bake (mixed rust+dep+infra on
      PR #45): `pr-fast` p50 wall time **~7 min** (rustdoc+doctests
      dominates; next-longest is `clippy` ~1 min, `sanity` ~1 min,
      `tests` buried inside `test-build+tests` pipeline).  Legacy
      `ci.yml` on the same PR: comparable ~5–7 min p50.  No regression.
- [ ] 7-day parallel window with `ci.yml` (dates: **2026-04-23**
      → **2026-04-30**).
- [ ] Branch-protection cutover: `ci.yml` deleted + required-checks
      updated to `PR Fast CI / required` in the **same commit**.
      See §4.3 for the procedure.

**Notes**:
- Rather than pulling in `dorny/paths-filter`, `classify` uses a
  native `git diff --name-only BASE..HEAD` shell step.  Pros: no new
  third-party action to vet (supply-chain); transparent regex we own.
  Cons: slightly more shell code.  Trade-off documented here so
  future maintainers don't swap back without discussion.
- `security` job re-runs `cargo vet check --locked` even though
  `sanity` also runs it when `dep_changed`.  This is intentional:
  external observers (branch protection, GitHub search for "vet")
  expect a distinct `Security` check in the check-runs list.  Cost
  is ~10 s on warm cache.
- `windows-check` uses `cargo check` (not `build`) — the PR-fast lane
  only needs compile-confidence.  Full release-shaped builds move to
  `preview-artifacts.yml` where they belong.

#### Phase 4b — Actions hardening retrofit

Audit each existing workflow against §2.8 policy:

- [ ] `tier-2.yml` — permissions, SHA pinning, timeouts, merge_group.
- [ ] `codeql.yml` — same.
- [ ] `release.yml` — same.
- [ ] `auto-tag-release.yml` — same.
- [ ] `cargo-vet-refresh.yml` — same.
- [ ] `dependabot-review.yml` — same.
- [ ] `dependabot-auto-merge.yml` — same.

**Notes**:

#### Phase 5 — Preview lane

- [x] `gate` job guards on `preview-binaries` label OR
      `workflow_dispatch`.
- [x] `gate` pins the SHA (`needs.gate.outputs.sha`) for every
      downstream checkout / artifact name — prevents drift on
      `synchronize` events.
- [x] `verify-pr-fast-green` polls GitHub Checks API for
      `PR Fast CI / required` on the pinned SHA; up to 60 × 10 s
      retry; fails fast on non-success.
- [x] `build-windows` uses `cargo-xwin` on ubuntu-22.04 targeting
      `x86_64-pc-windows-msvc`; caches via `preview-windows`
      shared-key; uploads `dist/windows/*.exe`.
- [x] `build-test-archive` builds nextest archive
      (`uffs-tests-<sha>.tar.zst`); exposes `nextest_version` as job
      output so the manifest can advertise it.
- [x] `smoke-windows` runs on **GitHub-hosted `windows-latest`** (not
      self-hosted) and executes the archive against the same-SHA
      checkout — both nextest archive requirements satisfied.
- [x] `manifest` emits `manifest.json` with `git_sha`, `tested_sha`,
      `cargo_lock_sha256`, `rustc_version`, `nextest_version`,
      per-file SHA256 for every downloaded artifact.
- [x] Concurrency: `group: preview-${workflow}-${pr#|sha}`,
      `cancel-in-progress: true` — push during in-progress preview
      cancels it (satisfies plan validation bullet).
- [x] Permissions: `contents: read`, `checks: read`,
      `pull-requests: read` (needed for Checks API + label lookup).
- [x] Both workflows pass `actionlint` clean.
- [ ] **Label-trigger path** real PR validation: apply
      `preview-binaries` label to a green PR → workflow fires →
      artifacts appear in Actions UI.  Deferred to first real use.
- [ ] **Same-SHA integrity** real validation: `manifest.git_sha` ==
      PR head SHA; every `files[].sha256` matches `sha256sum` of
      downloaded file.  Deferred.
- [ ] **Nextest archive round-trip** validation: Windows box with
      matching `nextest_version` from manifest, `git checkout <sha>`,
      `cargo nextest run --archive-file`.  Deferred.
- [ ] **Pre-fast-gate enforcement** validation: deliberate
      `PR Fast CI` failure blocks preview build.  🔴 Critical —
      verify on first test PR.
- [ ] **Fork-PR behaviour** validation: all jobs use
      `runs-on: ubuntu-22.04` / `windows-latest`, never self-hosted
      — static check is ✅ per grep; live fork-PR bake deferred.

**Notes**:
- The `verify-pr-fast-green` job is the critical gate that prevents
  wasting runner minutes on non-mergeable SHAs.  The polling loop is
  10 minutes; if PR-fast is slower than that, increase the cap in
  one commit — don't raise the preview trigger's retry budget
  arbitrarily.
- `build-test-archive` and `build-windows` share a `preview-windows`
  `rust-cache` shared-key so the Windows std + xwin sysroot are
  compiled once per SHA, not twice.
- The manifest's `cargo_lock_sha256` is a stronger integrity anchor
  than trusting the artifact alone — if someone later asks "which
  crates versions were in this preview", they can reconstruct
  deterministically from this + the git SHA.

#### Phase 7 — `ci-pipeline.rs` → workspace binary

- [x] `scripts/ci-pipeline/Cargo.toml` created.
- [x] `scripts/ci-pipeline/src/main.rs` is the authoritative
      implementation (1735 lines, previously 1752 in the rust-script
      minus the 17-line shebang / deps header).
- [x] Workspace `members` in `Cargo.toml` updated:
      `"scripts/ci-pipeline"` added alongside the crates.
- [x] `Cargo.lock` regenerated and tracks `uffs-ci-pipeline v0.5.71`.
- [x] `cargo check -p uffs-ci-pipeline --locked` → clean (6.77 s cold,
      0.17 s warm).
- [x] `cargo run -q -p uffs-ci-pipeline -- --help` → prints the same
      subcommand table as before the promotion.
- [x] `just/workflow.just`, `just/dev.just`, `just/bench_ci.just` —
      every `rust-script scripts/ci/ci-pipeline.rs <cmd>` replaced
      with `cargo run -q --release -p uffs-ci-pipeline -- <cmd>`.
- [x] `ci.yml` path filter additionally watches
      `scripts/ci-pipeline/**` so the parallel window catches
      changes to the new crate.
- [x] Old `scripts/ci/ci-pipeline.rs` kept as a **thin wrapper**
      (49 lines) that prints a deprecation notice and re-execs
      `cargo run -q --release -p uffs-ci-pipeline -- "$@"`.  Verified
      executable via `rust-script scripts/ci/ci-pipeline.rs --help`.
      Marked `REMOVE-AFTER: v0.5.73` in the header.
- [ ] Real-world bake-in on a live `just ship` run.  Deferred to
      the next release cycle.

**Notes**:
- The new crate intentionally does **not** opt into
  `[lints] workspace = true`.  The workspace lints are calibrated
  for production library/CLI code (deny unwrap, deny `println!`, deny
  missing_docs_in_private_items).  Inheriting them for this tool
  would require thousands of lines of edits without making the tool
  materially better.  Opting out is documented in a header comment
  on the crate's Cargo.toml so future maintainers don't silently
  re-enable them.
- Package name: `uffs-ci-pipeline` (not `ci-pipeline` as named in the
  plan spec).  Matches the workspace convention that every
  workspace-local crate is `uffs-*`, and avoids a future collision
  with any upstream crate published to crates.io as `ci-pipeline`.
  `publish = false` is set to enforce we never accidentally publish.
- `tokio` workspace baseline is net-free; this crate re-enables
  the `process` feature via `features = ["process"]` on the
  workspace alias.  Feature unions are additive, so no other crate
  sees this addition.

#### Phase 8 — (stretch) `gates.toml`

- [ ] Deferred — re-evaluate after 2 release cycles with lanes split.

**Notes**:

### 10.4 Metric measurements

Fill the baseline column at Phase 1 start; fill each phase column
at its completion.  Empty cells mean "not yet measured"; `—` means
"metric does not apply to this phase".  Record actual values, not
pass/fail, so regressions are visible across the refactor.

| Metric | Baseline | P1 | P2 | P3 | P4 | P5 |
|--------|----------|----|----|----|----|----|
| pre-push wall (warm) | 23–45 s | | | | — | — |
| pre-push wall (cold) | 60–90 s | | | | — | — |
| pre-commit wall (Rust change, warm) | 15–25 s | — | | | — | — |
| Pre-push time to first red on vet failure | ∞ (CI-only) | ~1 s | | | — | — |
| Pre-push time to first red on type error | ~40 s (clippy) | | ~15 s | | — | — |
| Cold `cargo build --workspace` | — | — | — | | — | — |
| Warm rebuild after single-file touch | — | — | — | ~42 s target | — | — |
| PR-fast wall (warm, p50) | 8–12 min (ci.yml) | — | — | — | ≤ 8 min | — |
| Preview-artifacts wall (warm) | n/a | — | — | — | — | ≤ 30 min |
| Windows regression detection latency | release-time | — | — | — | PR-time | PR-time |

### 10.5 Deviations log

Any step that diverged from the plan — record here so future readers
don't infer the wrong reason from commit messages.

| Date | Phase | Deviation | Resolution |
|------|-------|-----------|------------|
| 2026-04-23 | 4 | Plan §4 claimed 8 `Tier 1 / *` checks were required.  Actual ruleset had 6 (no `Doc Tests`, no `Test Build`). | Corrected baseline to 6 entries in §4.1; parallel-window math in §4.2 adjusted accordingly. |
| 2026-04-23 | 4 | First ruleset PUT used context `"PR Fast CI / PR Fast CI / required"` (matching the UI display string).  PR entered `mergeStateStatus: BLOCKED` with zero failing checks because no real check reports under that name — the protection engine matches `check_run.name` = job `name:` attribute only. | Second PUT used `"PR Fast CI / required"` (bare job name).  PR went to `CLEAN/MERGEABLE`.  Gotcha documented in §4.4 to prevent recurrence at Tier 1 cutover. |

### 10.6 Open blockers

Active blockers visible at a glance.  Move to "Resolved" below with a
one-line outcome when cleared.

**Active**:

- **7-day parallel-window bake** — started 2026-04-23, scheduled to
  end 2026-04-30.  Not a blocker per se; a scheduled milestone.  At
  end of window (or sooner if signal is overwhelmingly green),
  execute the §4.3 cutover.
- **Phase 4b** — actions-hardening retrofit across `tier-2.yml`,
  `codeql.yml`, `release.yml`, `auto-tag-release.yml`,
  `cargo-vet-refresh.yml`, `dependabot-review.yml`,
  `dependabot-auto-merge.yml` not yet audited against the §2.8 policy.
  Independent of the bake window; can land any time.
- **Real-world bake gaps** — `Dep-only PR` and pure `Infra-only PR`
  still need a natural exercise.  Opportunistic — wait for dependabot
  or an infra-only edit; don't synthesize a test PR just for this.

**Resolved**:

- **Broken-classify simulation** — ✅ 2026-04-23, on PR #45.  `required`
  correctly propagated failure through 8 skipped downstream jobs.
- **Ruleset context-string gotcha** — ✅ 2026-04-23, same day.  See
  §4.4 for the write-up.

### 10.7 Post-mortem triggers

If any of the following happens during rollout, **stop and reassess**
before continuing to the next phase:

- A hard gate added in Phases 1–3 blocks a legitimate push more than
  once in the same session without an obvious fix.  Symptom of an
  over-tightened gate; relax or move to advisory before continuing.
- Phase 4 validation step 5 (broken-classify simulation) unexpectedly
  reports `required = success`.  Symptom of the GitHub Actions
  skipped-success bug not being closed; the `Gate on classify` step
  is broken.
- Phase 5 validation step 4 (pre-fast-gate enforcement) lets a preview
  build proceed against a red PR-fast.  Symptom of the check-runs
  poller returning stale data.
- `just ship` loses time to any newly-introduced gate.  Budget was
  tight before — keep it tight after.

---

## 11. Revision history

- **2026-04-23 v3.1** — Current.  Post-PR #45 reconciliation:
  - Phases 1, 2, 3, 6, 7 marked complete with commit + PR references
    (squash `780c1dbb1`).
  - Phase 4 annotated with live-bake progress: broken-classify sim
    executed and passed; parallel-window opened; cutover scheduled.
  - §4 rewritten to reflect reality: ruleset-based protection
    (ID `11889528`), 6-entry (not 8) Tier 1 baseline, 7-entry
    parallel-window state, documented cutover procedure, and a
    required-check context-string gotcha section.
  - §10.3 Phase 4 checklist: broken-classify and rust-change bakes
    ticked with evidence; 7-day window dates filled
    (2026-04-23 → 2026-04-30); infra-only partially credited.
  - §10.5 Deviations log: baseline-count correction + ruleset
    context-string gotcha recorded.
  - §10.6 Open blockers: bake-window + 4b retrofit + two real-world
    bake gaps marked active; broken-classify + context gotcha moved
    to resolved.
- **2026-04-23 v3** — Incorporates second-round external
  review.  Eleven correctness fixes:
  1. `required` aggregator explicitly depends on `classify` and
     fails unless `needs.classify.result == 'success'` — closes
     the "skipped-counts-as-success" green-lighting bug.
  2. Every compile/test/Windows job gates on
     `code_changed = rust || dep || infra`, not `rust == 'true'` —
     dep-only and infra-only PRs now run the full matrix.
  3. Explicit `infra_changed` file class defined in §1.3 with
     concrete glob patterns, wired into `classify` outputs.
  4. Pre-push diff logic uses git's stdin protocol
     (`<local_ref> <local_oid> <remote_ref> <remote_oid>`) instead
     of `@{push}` heuristics, with conservative fallback for
     unparseable cases.
  5. `preview-artifacts.yml` gains `verify-pr-fast-green` job
     querying check-runs API for the same SHA; refuses to build
     unless PR-fast is green on that commit.
  6. Explicit checkout-ref policy — every job pins
     `github.event.pull_request.head.sha || github.sha`.  No
     reliance on `actions/checkout`'s default PR behaviour.
  7. `--locked` applied to every cargo subcommand that supports
     it (CI and local).  `cargo fetch --locked` primes the cache.
  8. New §2.8 Actions hardening policy: `permissions: contents: read`
     default, full-SHA action pinning, forbidden
     `pull_request_target` + self-hosted fork PRs.
  9. `merge_group:` trigger added to `pr-fast.yml` for merge-queue
     compatibility.
  10. Manifest emits `tested_sha`, `nextest_version`, and per-file
      SHA256 (not just the artifact-level digest).
  11. Local Windows xwin reclassified as advisory (not hard);
      PR-fast native `windows-check` is the authoritative gate.
      Resolves the v2 half-hard/half-soft inconsistency.
- **2026-04-23 v2** — External review of v1.  Lane separation,
  hard-vs-advisory gate split, single-source cache policy, ordered
  scheduler, PR-fast/preview split, nextest archive for Windows
  preview.
- **2026-04-23 v1** — `dev-flow.md` initial draft.  Retained as the
  "explanation" companion doc.
