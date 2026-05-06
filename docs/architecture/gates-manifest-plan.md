<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.

UFFS - Ultra Fast File Search
-->

# Gates Manifest — Single-Source-of-Truth for Quality Gates (Plan)

> Implementation companion to
> [`dev-flow-implementation-plan.md` §2.7](dev-flow-implementation-plan.md)
> ("machine-readable gate manifest").  That section sketches the goal
> at ~30 lines; this doc is the full implementation plan, schema spec,
> generator interface, golden-file verification strategy, per-phase
> migration order, and risk analysis.

## Status (2026-05-06)

| Phase | Description | Status |
|---|---|---|
| 0 | Plan + schema design | 🟡 in flight (this doc) |
| 1 | Manifest + drift detector (no consumer changes) | ⏭ pending |
| 2 | Codegen for `_lint_pre_push.sh` | ⏭ pending |
| 3 | Codegen for `_lint_fast.sh` + `pr-fast.yml` + doc tables | ⏭ pending |

Closing all four phases brings the workspace to **zero hand-maintained
gate-set drift** between local hooks, CI workflows, and contributor
docs.  PR #138 (Phase W5/L1 closure of
`windows-clippy-and-linux-cross-plan.md`) was the proximate motivation:
the windows-check → windows-lint rename touched **6 files** in 5
distinct surfaces (workflow YAML, pre-push hook, pre-commit hook
comments, CONTRIBUTING.md table, dev-flow doc gate matrix,
supply-chain doc reference list) plus a CHANGELOG entry, and it took
two follow-up commits to catch every reference.  That class of pain
is what this manifest erases.

## 1. Problem

The current gate set is hand-maintained in **six** places that all need
to stay in lockstep:

| Surface | What it encodes | Drift modes |
|---|---|---|
| `scripts/hooks/_lint_fast.sh` | pre-commit gate set | adding a gate without updating pre-push or CI; budget regressions |
| `scripts/hooks/_lint_pre_push.sh` | pre-push gate set, bucket assignments, fail-fast order | same as above plus bucket misclassification |
| `.github/workflows/pr-fast.yml` | CI job graph + `required` aggregator's `needs:` list | required-check name drift breaks branch protection silently |
| `just/test.just` (`lint-*` recipes) | manual invocations the hooks delegate to | recipe rename without hook update |
| `CONTRIBUTING.md` four-layer table | contributor-facing doc | usually the LAST thing updated; lies for weeks |
| `docs/architecture/dev-flow.md` §3.2 gate matrix | architectural reference | same as above |

Drift is **mathematically inevitable** because:

1. Each new gate requires editing 4-6 files in a specific order.
2. Each rename / flag flip requires the same.
3. There is no automated check that the local hook actually runs the
   same set of gates that CI runs (only that each individually exits 0).
4. Reviewers cannot easily diff "what does pre-push catch?" against
   "what does CI catch?" — they have to mentally cross-reference 6 files.

**Recent evidence**:
- PR #138 W5.5/W5.6 flip: 6 file references to `windows-check` had to
  be updated to `windows-lint`.  Two follow-up commits caught the
  ones missed in the first pass (`dev-flow.md`, `supply-chain-posture.md`,
  `dev-flow-implementation-plan.md` design-rationale bullets).
- Tier 2 `windows-check` removal (W5 follow-on, same PR): 5 surface
  points across `tier-2.yml` alone (job block + 2 `needs:` lists +
  summary conditional + summary table line).  Plus 4 doc surfaces.
- Phase 2 (long ago) removed Windows xwin from pre-commit.  CONTRIBUTING.md
  still claimed it ran there until PR #138 caught the lie.

## 2. Goal

Single source of truth: **`scripts/ci/gates.toml`** defines every
gate (id, command, env, classification, expected runtime, tier
membership).  All consumers — pre-commit hook, pre-push hook, CI
workflow, contributor docs — are **generated from** this file.  CI
verifies generators are in sync (drift detector).

Adding a new gate becomes **one TOML edit + one regen command**.
Renaming a gate becomes the same.  The contributor docs auto-update.
The required-check name in CI's branch protection cannot drift from
what the workflow actually emits.

## 3. Manifest schema (`scripts/ci/gates.toml`)

```toml
# Top-level metadata
[manifest]
version = 1
generator_version = "..."  # bumps when the schema or generator output changes
generated_files = [        # files the generators own; regen rewrites these
    "scripts/hooks/_lint_fast.sh",
    "scripts/hooks/_lint_pre_push.sh",
    ".github/workflows/pr-fast.yml",
    # Optional: also-generated doc tables, see Phase 3
]

# Change-classification regexes used by the pre-push hook to gate
# bucket-2 jobs.  Hooks AND CI's classify job both consume these so
# they stay in lockstep.
[classification]
rust    = '\.rs$'
dep     = '^(.*Cargo\.toml$|Cargo\.lock$|supply-chain/)'
infra   = '^(\.github/|scripts/|\.cargo/|\.config/|just/|rust-toolchain|clippy\.toml$|rustfmt\.toml$|deny\.toml$|REUSE\.toml$|codecov\.yml$)'
docs    = '\.md$'

# One [[gate]] entry per logical check.
[[gate]]
id       = "fmt"                          # stable kebab-case identifier
label    = "cargo fmt --check"            # human-readable name
command  = ["cargo", "fmt", "--all", "--", "--check"]
tiers    = ["pre-commit", "pre-push", "pr-fast"]   # where this gate runs
gate_when = "always"                      # always | rust_changed | dep_changed | infra_changed | code_changed
hard     = true                           # hard-fail vs soft-skip-with-hint
tool     = "cargo"                        # missing-tool detection key
expected_runtime_secs = 1
bucket   = "bg"                           # pre-push bucket: "bg" (parallel) or "seq" (sequential fail-fast)
order    = 10                             # within-bucket ordering hint
notes    = """
Always-on; staged scope on pre-commit, workspace scope on pre-push and CI.
"""

[[gate]]
id       = "lint-ci"
label    = "cargo clippy --all-targets -D warnings (CI mirror)"
command  = ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--no-deps", "--", "-D", "warnings"]
tiers    = ["pre-commit", "pre-push", "pr-fast"]
gate_when = "rust_changed"
hard     = true
tool     = "cargo"
expected_runtime_secs = 10
bucket   = "seq"
order    = 20

[[gate]]
id       = "lint-prod"
label    = "cargo clippy --lib --bins (ultra-strict)"
command  = ["cargo", "clippy", "--workspace", "--lib", "--bins", "--all-features", "--locked", "--no-deps", "--", "{{prod_flags}}"]
tiers    = ["pre-commit", "pre-push"]    # NOT in pr-fast (lint-ci already covers --all-targets there)
gate_when = "rust_changed"
hard     = true
tool     = "cargo"
expected_runtime_secs = 8
bucket   = "seq"
order    = 30
flag_template = "prod_flags"             # which shared.just flag bag to expand

# ... etc for every gate currently defined in:
#   - scripts/hooks/_lint_fast.sh
#   - scripts/hooks/_lint_pre_push.sh
#   - .github/workflows/pr-fast.yml
```

### Field semantics

- **`id`**: kebab-case identifier; primary key.  Stable across renames
  (renaming an `id` is a breaking change → manifest version bump).
- **`label`**: human-readable name surfaced in CI logs + hook output.
  Free-form; can change without breaking anything downstream.
- **`command`**: array form for safety (no shell-injection on
  whitespace-bearing args).  Generator quotes per target language.
- **`tiers`**: any subset of `{"pre-commit", "pre-push", "pr-fast", "tier-2"}`.
  Drives which generated files include this gate.
- **`gate_when`**: triggers the gate against a change-classification
  bit.  `always` = unconditional; `rust_changed` / `dep_changed` /
  `infra_changed` = exact match against the manifest's `[classification]`
  regexes; `code_changed` = OR of `rust|dep|infra` (matches the existing
  `_lint_pre_push.sh` semantics).
- **`hard`**: `true` = exit non-zero on missing tool; `false` =
  soft-skip + print install hint.  Mirrors existing hook behaviour.
- **`tool`**: the binary the hook checks for via `command -v`.  Used
  only for missing-tool detection.
- **`expected_runtime_secs`**: documentary; surfaced in generated
  comments for the budget table.  Not enforced.
- **`bucket`** (pre-push only): `"bg"` = bucket 1 (cheap, parallel,
  fire-and-forget); `"seq"` = bucket 2 (cargo-heavy, sequential
  fail-fast).  Mirrors existing `_lint_pre_push.sh` design.
- **`order`** (pre-push only, within bucket): integer ordering hint;
  bucket-2 cheapest-to-most-expensive.
- **`flag_template`**: optional reference to a shared flag bag
  (`prod_flags` / `test_flags` / `common_flags`) defined in
  `just/shared.just`.  Generators substitute the actual flag list at
  emit time so the strict-flag stack stays single-sourced.
- **`notes`**: free-form Markdown surfaced in the generated comment
  blocks.  Carries the rationale that lives today in long bash comment
  prefixes.

### Optional sections

- `[[platform_override]]`: per-target tweaks (e.g. `lint-ci-windows`
  uses `cargo xwin clippy` cross-compile, `lint-ci-linux-zig` uses
  `cargo-zigbuild clippy`).  Modeled as overrides on a base `[[gate]]`
  to keep the strict-flag stack single-sourced.

## 4. Generator interface

Three small generators, one per consumer.  Each is a Rust binary
under `scripts/ci/gen-*` (Rust because TOML parsing in bash is
fragile, and we already require a Rust toolchain for everything else).

### 4.1 `scripts/ci/gen-hooks` (Phase 2)

```
USAGE: gen-hooks [--check]

Reads scripts/ci/gates.toml.
Writes (or checks) scripts/hooks/_lint_pre_push.sh
and scripts/hooks/_lint_fast.sh.

EXIT:
  0  on emit (without --check), or on no-op (with --check)
  1  on diff (with --check) — file is out-of-sync with manifest
  2  on schema error (manifest invalid)

FLAGS:
  --check           Diff mode: do not write files; exit 1 if regen
                    would change them.  Used by CI drift detector.
  --tier <tier>     Restrict emit to one tier (pre-commit | pre-push).
  --verbose         Print per-gate emit decisions to stderr.
```

### 4.2 `scripts/ci/gen-workflow` (Phase 3)

```
USAGE: gen-workflow [--check]

Reads scripts/ci/gates.toml.
Writes (or checks) the generated job blocks within
.github/workflows/pr-fast.yml.

EXIT: same as gen-hooks.

The workflow has hand-written + generated sections.  Only the
job blocks for individual gates are generated; the `classify`,
`required` aggregator, and `notify-failure` jobs stay
hand-maintained but are validated against the manifest's
generated `needs:` list.  Generator markers in the YAML:

    # >>> generated:gates-manifest <<<
    [generated content here]
    # <<< generated:gates-manifest >>>

Hand edits between the markers fail CI.
```

### 4.3 `scripts/ci/gen-docs` (Phase 3, optional)

```
USAGE: gen-docs [--check]

Reads scripts/ci/gates.toml.
Writes (or checks) the gate-matrix tables in:
  - CONTRIBUTING.md (four-layer quality gates table)
  - docs/architecture/dev-flow.md (§3.2 gate matrix)
  - docs/architecture/dev-flow-implementation-plan.md (§1.3.1 hard gates table)

EXIT: same as gen-hooks.

Same marker convention as gen-workflow.
```

### 4.4 Idempotency contract

- Running any generator without `--check` is **idempotent**: running
  it twice in a row produces no diff on the second run.
- Output is **deterministic**: byte-for-byte stable across:
  * macOS / Linux / Windows hosts (no host-dependent strings)
  * Different working directories (no absolute paths in output)
  * Different rust toolchain patches (within the pinned major)
- Generators consume `RUSTFLAGS` / `CARGO_*` env vars when emitting
  documentary expected-runtime values, so the comments don't drift
  with sccache cache state.

## 5. Phase-by-phase migration

### Phase 1 — Manifest + drift detector (`feat/gates-manifest-phase-1`)

**Scope**:

1. NEW `scripts/ci/gates.toml` — populated by reading the existing
   `_lint_fast.sh` + `_lint_pre_push.sh` + `pr-fast.yml` and
   transcribing every gate.  Hand-written for this PR; the generators
   come in Phase 2/3.
2. NEW `scripts/ci/check_gates_drift.sh` — small bash that:
   - Reads `gates.toml`
   - For each `[[gate]]` with the appropriate tier:
     * Greps the corresponding consumer file (`_lint_pre_push.sh` etc.)
       for the gate's `id` or `command`
     * Fails if a manifest entry is not present in the consumer
     * Fails if a consumer entry (matched against a known-form regex)
       is not present in the manifest
   - Pass = manifest and consumers agree on the gate set.
3. NEW pre-push hook step + NEW CI job (`gates-drift`) that runs the
   drift detector.  Both hard-fail on mismatch.
4. NEW NPM-style version field in `gates.toml` (`manifest.version = 1`).
   Bumping breaks downstream tools — surfaced in CI as a separate
   `manifest-version` job that requires a `version-bump` PR label.

**No existing-consumer changes.**  `_lint_fast.sh`, `_lint_pre_push.sh`,
and `pr-fast.yml` continue to be hand-maintained.  The manifest is
purely informational + a drift safety net.

**Acceptance criteria**:

- [ ] `scripts/ci/gates.toml` exists; passes `taplo check`.
- [ ] Manifest covers every gate currently defined in
      `_lint_fast.sh`, `_lint_pre_push.sh`, `pr-fast.yml`.
- [ ] `scripts/ci/check_gates_drift.sh` exits 0 against current `main`.
- [ ] Mutating the manifest (e.g. dropping a gate) → drift detector
      exits 1 with a clear "gate `foo` in manifest, missing from
      `_lint_pre_push.sh`" message.
- [ ] Mutating a hook (e.g. removing `lint-ci`) → drift detector
      exits 1 with a symmetric message.
- [ ] `pr-fast.yml` `gates-drift` job runs and gates the `required`
      aggregator (added to `needs:`).
- [ ] Pre-push hook runs the drift check as a Bucket-1 job
      (cheap, parallel, fire-and-forget — same tier as `fmt` /
      `file-size`).
- [ ] `actionlint` exit=0 on the modified `pr-fast.yml`.
- [ ] All existing CI checks still pass (no behavioural change).

**Risk**: Near-zero.  No consumer-file logic changes; only ADD a
manifest + a drift checker that fires loudly on mismatch.  Worst case:
drift detector has a false positive that blocks a PR — easy to disable
with the `manifest.version` bump escape hatch documented above, plus
a `BYPASS_GATES_DRIFT=1 git push` env-var bypass mirroring the
existing `COMMIT_SUBJECT_BYPASS=1` pattern in `_lint_pre_push.sh`.

**Verification before merge**:

```bash
# Drift detector behaves correctly
just gates-drift                                    # exit 0
sed -i.bak 's/lint-ci/lint-ci-removed/' scripts/ci/gates.toml
just gates-drift                                    # exit 1, helpful message
mv scripts/ci/gates.toml.bak scripts/ci/gates.toml  # restore

# Workflows still parse
actionlint .github/workflows/pr-fast.yml            # exit 0

# Hooks still work
just lint-pre-push                                  # exit 0, drift-check shows in output

# CI green on the PR (the gates-drift job is itself the new check)
```

**Verification after merge**:

- Watch the next 1-2 PRs that add or rename a gate (e.g. a
  Dependabot rust-toolchain bump that adds a new clippy lint).
  Drift detector should fire if any consumer was updated without the
  manifest, or vice versa.

### Phase 2 — Codegen for `_lint_pre_push.sh` (`feat/gates-manifest-phase-2`)

**Scope**:

1. NEW `scripts/ci/gen-hooks/` — Rust workspace member implementing
   `gen-hooks` per §4.1.  Single binary; no library exposure (the
   manifest model is internal to this binary).
2. NEW `tests/ci/golden/_lint_pre_push.sh` — byte-for-byte snapshot
   of the pre-Phase-2 hand-maintained file.  Updated only via
   explicit "golden update" PRs.
3. MODIFIED `scripts/hooks/_lint_pre_push.sh` — now generated.
   Header banner: `# AUTO-GENERATED by scripts/ci/gen-hooks; manual
   edits will be overwritten.  Source: scripts/ci/gates.toml.`
4. NEW `just gen-hooks` recipe (alias `just gen` if no other generator
   exists yet) — runs the generator non-`--check`.
5. NEW CI job `hooks-drift` — runs `gen-hooks --check`; fails if
   regen would change the file.
6. NEW pre-push step (Bucket-1) — same drift check.

**Pre-commit hook stays hand-written.**  Reason: pre-commit's logic
is small (3 clippy spawns + a few bash conditionals); generating it
is more code than the file it replaces.  Phase 3 picks it up if/when
we extract enough common machinery.

**Acceptance criteria**:

- [ ] `cargo build -p gen-hooks` succeeds.
- [ ] `cargo test -p gen-hooks` covers schema parsing + emit
      determinism (same input → same output across runs).
- [ ] Running `just gen-hooks` produces `_lint_pre_push.sh` that is
      **byte-for-byte equivalent** to the golden snapshot.
- [ ] `diff -u tests/ci/golden/_lint_pre_push.sh scripts/hooks/_lint_pre_push.sh`
      → empty.
- [ ] `bash -n scripts/hooks/_lint_pre_push.sh` (syntax check).
- [ ] `just lint-pre-push` exits 0 with **identical** observable
      behaviour to pre-PR (same gate list, same bucket assignment,
      same fail-fast order, same wall-clock within ±10%).
- [ ] Manually trigger a `git push` (e.g. push a no-op commit on a
      throwaway branch) → hook fires correctly.
- [ ] Phase-1 drift detector still passes.
- [ ] CI's new `hooks-drift` job runs and gates the `required`
      aggregator.

**Risk**: Medium.  Generator bugs could silently drop a gate, change
fail-fast order, or shuffle bucket assignments.  The byte-for-byte
golden-file diff is the primary safety net: any unintentional change
shows up as a diff in the PR; intentional changes require updating
the golden in the same commit (visible in review).

**Verification before merge**:

```bash
# Codegen + drift loop
just gen-hooks                                      # writes file
diff -u tests/ci/golden/_lint_pre_push.sh \
        scripts/hooks/_lint_pre_push.sh             # empty
just hooks-drift                                    # exit 0
just gates-drift                                    # exit 0 (Phase 1 still passes)

# Behavioural equivalence
just lint-pre-push                                  # exit 0, same gates
git push --dry-run origin HEAD:refs/heads/test     # hook runs, exit 0

# Mutation testing
echo '# garbage' >> scripts/hooks/_lint_pre_push.sh
just hooks-drift                                    # exit 1 (catches manual edit)
git checkout scripts/hooks/_lint_pre_push.sh       # revert
```

**Rollback**: Revert the PR.  `_lint_pre_push.sh` is byte-for-byte
identical to the pre-Phase-2 file (the golden), so reverting yields
the previous hand-maintained version exactly.

### Phase 3 — Codegen for `_lint_fast.sh` + `pr-fast.yml` + doc tables (`feat/gates-manifest-phase-3`)

**Scope**:

1. EXTEND `gen-hooks` to also emit `_lint_fast.sh`.
2. NEW `scripts/ci/gen-workflow/` — Rust binary per §4.2.  Emits
   the per-gate job blocks in `pr-fast.yml` between the `# >>>
   generated:gates-manifest <<<` markers.  Hand-written sections
   (classify, required, notify-failure) stay hand-maintained.
3. NEW `scripts/ci/gen-docs/` — Rust binary per §4.3 (optional —
   may be deferred to a Phase 3b sub-PR if scope grows too large).
4. NEW `tests/ci/golden/_lint_fast.sh` + `tests/ci/golden/pr-fast.yml`
   — byte-for-byte snapshots.
5. MODIFIED `scripts/hooks/_lint_fast.sh` + `.github/workflows/pr-fast.yml`
   — both now generated (with hand-written sections clearly marked).
6. NEW CI jobs: `workflow-drift`, optional `docs-drift`.

**Critical constraint**: `pr-fast.yml`'s `required` job's `name:` field
MUST stay as `PR Fast CI / required` — that exact string is in the
repo's branch-protection ruleset's `required_status_checks`.  Changing
it silently breaks merge protection.  The generator MUST hard-code
that string and surface a comment explaining why.

**Acceptance criteria**:

- [ ] All Phase 2 acceptance items still hold for `_lint_pre_push.sh`.
- [ ] Generated `_lint_fast.sh` is byte-for-byte equivalent to the
      golden snapshot.
- [ ] Generated `pr-fast.yml` is byte-for-byte equivalent to the
      golden snapshot **between the markers**; hand-written sections
      (above + below + classify + required + notify-failure)
      unchanged.
- [ ] `actionlint .github/workflows/pr-fast.yml` exit=0.
- [ ] `bash -n scripts/hooks/_lint_fast.sh`.
- [ ] `just lint-fast` + `just lint-pre-push` exit 0 with identical
      observable behaviour to pre-PR.
- [ ] **Branch protection still works**: the PR itself depends on
      `PR Fast CI / required` (same name as before); auto-merge
      gates correctly.
- [ ] Drift detectors (Phase 1 `gates-drift`, Phase 2 `hooks-drift`,
      new `workflow-drift`) all pass.

**Risk**: High.  `pr-fast.yml` is the highest-blast-radius file in the
repo — every PR depends on it.  A generator bug that emits invalid
YAML, drops a job, or renames the `required` job breaks merge
protection silently for every subsequent PR.

**Mitigation**:

- Land `gen-workflow` behind the marker convention so hand-written
  sections (especially `required` + branch-protection-facing names)
  are **not** generated.
- Run the generator in dry-run mode in a separate `validate-workflow`
  PR (zero behavioural change) to flush out any emit bugs before
  flipping `pr-fast.yml` to generated mode.
- Phase 3 can be split into 3a (`_lint_fast.sh` only) + 3b
  (`pr-fast.yml`) + 3c (`gen-docs`) sub-PRs if reviewer load demands.

**Verification before merge**: same matrix as Phase 2 plus:

```bash
# Workflow YAML still parses + matches golden
just gen-workflow
diff -u tests/ci/golden/pr-fast.yml \
        .github/workflows/pr-fast.yml               # empty within markers
actionlint .github/workflows/pr-fast.yml            # exit 0

# Required-check name is preserved
grep -n 'name: PR Fast CI / required' \
    .github/workflows/pr-fast.yml                   # exactly one match
```

**Rollback**: Revert the PR.  All three generated files are
byte-for-byte identical to their goldens (= the pre-Phase-3 hand-
maintained versions), so revert yields the previous state exactly.
Branch protection's required-check name is preserved across the
rollback.

## 6. Golden-file verification strategy

Each generated file gets a sibling under `tests/ci/golden/` with the
same basename.  The golden is committed alongside the generator
change that introduces it.  Updating a golden is a deliberate review
event:

- **Routine regen** (e.g. add a new gate to the manifest): the
  generator output changes → the golden must be updated in the same
  commit → reviewer sees the diff explicitly and ack's it.
- **Generator bug fix**: same flow.  Golden update is the visible
  evidence that the bug is fixed.
- **Schema bump** (`manifest.version`): may produce a wholesale
  golden rewrite.  Encouraged to land in a dedicated PR with the
  `manifest-version` label.

### CI step: `golden-diff`

```bash
# Run by every drift detector job
for f in tests/ci/golden/*; do
    target=$(basename "$f")
    case "$target" in
        _lint_pre_push.sh|_lint_fast.sh)
            actual="scripts/hooks/$target" ;;
        pr-fast.yml)
            actual=".github/workflows/$target" ;;
    esac
    if ! diff -u "$f" "$actual" >/dev/null; then
        echo "❌ $target diverges from golden:"
        diff -u "$f" "$actual"
        exit 1
    fi
done
```

This is the load-bearing piece of the whole plan.  As long as the
golden diff is part of CI's `required` aggregator, no generator
regression can ship undetected.

## 7. Open questions

### 7.1 Generator language

**Decision**: Rust.

Considered: bash (too fragile for TOML parsing — would need `dasel`
or `taplo` as a hard runtime dep), Python (drags Python into the
build path), Rust (already on every contributor's machine).

Rust binaries are compiled once and cached by `Swatinem/rust-cache`
in CI; local generator runs are sub-second after first build.  Cost:
~15 s of cold compile per fresh checkout.

### 7.2 Manifest format

**Decision**: TOML.

Considered: YAML (footguns around quoting + bool coercion), JSON (no
comments, ugly diffs), TOML (existing project standard — `Cargo.toml`,
`deny.toml`, `rust-toolchain.toml`, `clippy.toml`, `rustfmt.toml`,
`REUSE.toml`).  The `taplo` formatter is already installed by
`just install-dev-tools` and runs at pre-commit so manifest
formatting drift is auto-caught.

### 7.3 Should `just` recipes be generated too?

**Decision**: No.

The lint recipes (`lint-prod`, `lint-tests`, `lint-ci`,
`lint-ci-windows`, `lint-ci-linux`, `lint-ci-linux-zig`) are
human-facing entry points whose ergonomics matter (printf colour
prefixes, clear error messages, helpful install hints).  Generating
them costs more readability than it saves.

The hooks call these recipes via `just lint-ci` etc., so the recipe
NAMES (which the generator references) are part of the manifest's
implicit contract.  A `recipe = "lint-ci"` field on each `[[gate]]`
documents the binding.

### 7.4 What about Tier 2 (`tier-2.yml`)?

**Out of scope.**

Tier 2 is the deep-assurance lane (coverage, miri, udeps); each job
is uniquely shaped (different toolchains, different timeouts,
different cache keys).  Forcing them into the gate-manifest schema
loses fidelity.  Tier 2 stays hand-maintained.

### 7.5 What about `release.yml` / `tier-2.yml` / `codeql.yml` / etc.?

**Out of scope.**

Those workflows have orthogonal concerns (release artifact signing,
weekly cron deep-checks, third-party SAST orchestration).  The
manifest's mandate is the **PR-time gate set** — what blocks merge.
Other workflows are opaque to it.

## 8. Cross-references

- **Source spec**: `docs/architecture/dev-flow-implementation-plan.md` §2.7
- **Companion plan (closing W5/L1)**: `docs/architecture/windows-clippy-and-linux-cross-plan.md`
- **Existing pre-commit hook**: `scripts/hooks/_lint_fast.sh`
- **Existing pre-push hook**: `scripts/hooks/_lint_pre_push.sh`
- **Existing PR-fast workflow**: `.github/workflows/pr-fast.yml`
- **Strict-flag stack**: `just/shared.just:21-30`
- **Contributor-facing gate table**: `CONTRIBUTING.md` (four-layer quality gates)
- **Architectural gate matrix**: `docs/architecture/dev-flow.md` §3.2
- **TOML formatter pre-commit**: `taplo` (already in `just install-dev-tools`)

## 9. Action log

| Date | Event | PR |
|---|---|---|
| 2026-05-06 | Plan drafted | (this PR, target: `docs(plan): gates manifest`) |
| TBD | Phase 1 lands (manifest + drift detector) | TBD |
| TBD | Phase 2 lands (`gen-hooks` for pre-push) | TBD |
| TBD | Phase 3 lands (full migration) | TBD |
