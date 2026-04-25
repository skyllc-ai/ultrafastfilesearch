<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.

UFFS — Release Automation: Baseline Metrics (Phase R0)
-->

# Release Automation Baseline Metrics

> Captured at the start of Phase R0 of
> [`release-automation-plan.md`](release-automation-plan.md).  These
> are the **before** numbers against which every subsequent phase is
> measured.  Re-measure at the start of R5 (cutover) and R9 (live
> publishing) to make regressions / improvements visible.

**Capture date**: 2026-04-25 (UTC-07:00)
**Captured by**: Phase R0 PR (`chore/release-auto-r0`)
**Workspace HEAD at capture**: `bf8681cb0` (`fix(release): re-codesign macOS binaries after strip — v0.5.73 (#63)`)

## 1. Versioning

| Metric | Value | Notes |
|---|---|---|
| Workspace version | `0.5.73` | `[workspace.package].version` in root `Cargo.toml` |
| Versioning model | Single-workspace (all 12 publishable crates inherit via `version.workspace = true`) | See plan §2.1 |
| Bumping mechanism | `build/update_all_versions.rs` (rust-script, ~1080 lines after R0 lockfile patch) invoked by `just ship` via `scripts/ci-pipeline/src/version.rs` | Targeted for retirement in R5 |
| `Cargo.lock` drift bug | **MITIGATED in R0** via `refresh_cargo_lock()` helper | Fully resolved in R5 (release-plz native) |

## 2. Changelog

| Metric | Value | Notes |
|---|---|---|
| `CHANGELOG.md` line count | **907** | Hand-maintained, Keep-a-Changelog format |
| `## [Unreleased]` section maintained | Yes, by hand | Drift-prone; observed misses on PR #51 and #52 (per plan §2.7) |
| Sections used | Added / Changed / Fixed / Removed / Performance / Security | Should map cleanly to `cliff.toml` parser groups in R2 |

## 3. Workflow invocations (last 30 days)

Captured via `gh run list --workflow <workflow> --limit 30 --json conclusion,createdAt,status`.

### `auto-tag-release.yml`

| Metric | Value |
|---|---|
| Total invocations (last 30d) | 6 |
| Successful | 6 (100%) |
| Failed | 0 |
| First successful run | 2026-04-22T18:30:37Z |
| Most recent run | 2026-04-25T21:18:33Z (v0.5.73) |
| Steady-state success rate | 100% — workflow is mature and reliable |

### `release.yml`

| Metric | Value |
|---|---|
| Total invocations (last 30d) | 30 |
| Successful | 5 |
| Failed | 25 |
| Headline rate | 16.7% |
| Steady-state rate (excluding stabilization period) | **100% (4/4)** for v0.5.69, v0.5.71, v0.5.72, v0.5.73 |

The headline rate is misleading.  The `release.yml` workflow underwent a **major stabilization push** between 2026-04-19 and 2026-04-22.  Failures break down as:

- **19 consecutive failures** v0.5.50 through v0.5.68 (2026-04-19 → 2026-04-22) — stabilization period, fixed iteratively
- **4 deliberate test runs** v99.x.x (2026-04-22) — testing release.yml's own logic via fake-version dispatches; 1 of 4 succeeded
- **2 failures** v0.5.67 / v0.5.67-Release_Test (2026-04-21) — same stabilization
- **5 successes** at the end (v0.5.69, v0.5.71, v0.5.72, v0.5.73, v99.99.99) — current steady state

The post-stabilization rate (the only one relevant to this baseline) is **100% over 4 consecutive real releases**.

### Time-to-binaries

End-to-end measurement: `Cargo.toml` version commit on `main` → release assets visible in GitHub Releases UI.

Computed from `auto-tag-release.yml` start time + `release.yml` duration for the 4 successful releases since steady state began (2026-04-22):

| Release | auto-tag start | release.yml duration | total wall time |
|---|---|---|---|
| v0.5.69 | 2026-04-22T22:03:29Z | 42m05s | ~42m |
| v0.5.71 | 2026-04-23T09:04:40Z | 33m16s | ~33m |
| v0.5.72 | 2026-04-25T18:42:07Z | 31m09s | ~31m |
| v0.5.73 | 2026-04-25T21:18:33Z | 32m28s | ~32m |

**Median**: ~32 minutes from version-bump commit to GitHub Release with binaries.  **p99**: ~42 minutes.  Dispatch overhead from `auto-tag-release.yml` to `release.yml` start is consistently ~7-8 seconds.

R5 target: release-plz creates the tag synchronously after release-PR merge, eliminating the dispatch hop entirely.  Expected wall-time improvement: ~10 sec on average (negligible) but reduces the failure surface (one fewer workflow-to-workflow handoff).

## 4. Conventional commits adherence

Sampled via:
```bash
git log --since=<date> --pretty=format:"%s" main \
  | grep -cE '^(feat|fix|perf|refactor|docs|test|build|ci|chore)(\([a-z0-9-]+\))?!?: '
```

### Last 30 days (broad sample, 2026-04-19 → 2026-04-25)

- Total commits with subject lines: **90**
- Strict-CC matches (the 9 standard types): **75**
- **Adherence: 83.3%**

Non-conforming commits fall into three patterns:

1. **Custom types** (10 commits): `security:`, `bench:`, `shmem:`, `stream-stress:`, `cross-tool-benchmark:`, `gitignore:`.  These follow the `prefix:` pattern but use project-internal categories not in the standard CC type list.
2. **Space in scope** (1 commit): `chore(dev + ci):` — the standard CC scope regex `[a-z0-9-]+` rejects whitespace.
3. **Pre-discipline-tightening commits** (4 commits): older entries from before the §2.8 conventional-commits norm settled in.

### Last 3 days (current steady state, 2026-04-22T22:00 → 2026-04-25)

- Total commits: **24**
- Strict-CC matches: **24**
- **Adherence: 100%**

The 24-commit window matches the §2.8 plan claim of "8/8 recent merges follow conventional-commit format" and confirms the current discipline level is sufficient for release-plz to consume immediately at R3.

### Implications for Phase R1

The advisory commitlint workflow added in R1a should be tuned to:

- Match the standard 9 CC types (the `83.3%` baseline)
- Soft-warn (not fail) on the custom types observed in the last 30 days; if the project owner intends them to remain valid, R1b's hard-gate either (a) extends the regex to include them, or (b) requires migration to standard types.
- Reject scopes containing whitespace (the `chore(dev + ci):` pattern is a typo, not a deliberate choice).

R1a will gather data on real-PR friction; the R1a → R1b transition (advisory → mandatory) should be informed by that data, not by today's snapshot.

## 5. Source-of-truth files (state at R0 capture)

Files this plan references by path, with their state at HEAD `bf8681cb0`:

| File | Lines | State at R0 |
|---|---|---|
| `Cargo.toml` (root) | 635 (post-R0) / 665 (pre-R0) | R0 deletes `[workspace.metadata.release-plz]` (9 lines) and `[workspace.metadata.dist]` (13 lines) plus the now-empty `Workspace Metadata` section header (8 lines including blank lines). Net: 30-line deletion. |
| `build/update_all_versions.rs` | 1073 (newly tracked) | **Promoted into git in R0.** Was previously gitignored via the blanket `build/` rule, which left a 1073-line script invoked from 4 callsites (`just/build.just`, `just/dev.just`, `scripts/ci-pipeline/src/version.rs:90`, `scripts/ci-pipeline/src/version.rs:113-117`) untracked locally and absent from any clone. R0 carves a `.gitignore` exception (`!build/update_all_versions.rs`) so the script becomes versioned alongside the lockfile-drift patch.  Marked for full deletion in R5; the `.gitignore` exception is removed at the same time. |
| `.gitignore` | 133 (post-R0) / 130 (pre-R0) | R0 replaces the blanket `build/` ignore with `build/*` + `!build/update_all_versions.rs` exception, plus a 7-line block comment explaining the carve-out and pointing to R5 for the eventual cleanup. Other `build/` artifacts (`.uffs-workflow-state.json`, `logs/`) stay ignored. |
| `.github/workflows/auto-tag-release.yml` | 169 | Untouched in R0. Targeted for deletion in R5. |
| `.github/workflows/release.yml` | ~780 | Untouched in any phase of this plan. Stays as the binary-build pipeline. |
| `CHANGELOG.md` | 907 | Untouched in R0. R2 will produce a `cliff.toml` template that targets this format. R3-R4 will start letting `git-cliff` generate sections. |
| `crates/uffs-mft/Cargo.toml.bak` | (deleted) | **DELETED in R0** as drive-by cleanup. Stale v0.4.106 auto-commit artifact. |

## 6. Phase progression context

This baseline file is the **first artifact created by Phase R0**.  Subsequent phases will append addenda below this line documenting:

- **R2**: result of running `git cliff --config cliff.toml --unreleased` against the post-v0.5.73 commit history.  Expected: changelog content matching the existing `## [Unreleased]` section to within stylistic differences.
- **R3**: per-run release-plz dry-run outputs — version bump proposed vs. judged-correct, changelog diff vs. judged-correct.
- **R5**: re-measurement of all metrics in this file.  Expected outcomes:
  - `CHANGELOG.md` line count: stable or slightly growing (release-plz appends, never deletes)
  - `auto-tag-release.yml` invocations: zero (workflow deleted)
  - `release.yml` median wall time: unchanged (the binary build itself is the bottleneck)
  - Conventional-commit adherence: ≥95% (R1b mandatory gate enforces it)
- **R9**: addendum recording the first crates.io publish event with crate names, versions, and `docs.rs` build outcomes.

## 7. Decisions confirmed by R0 data

The two decisions recorded in `release-automation-plan.md` §8 (settled 2026-04-24) are validated by this baseline:

1. **Decision 1 (R0 step-5 lockfile patch: INCLUDE)** — vindicated.  Two real releases (v0.5.72 and v0.5.73) shipped between the decision date (2026-04-24) and the R0 PR landing.  Both used the OLD bumper without the lockfile-refresh step, meaning their `Cargo.lock` files may have drifted before the next `cargo` invocation self-healed.  The lockfile patch now ensures v0.5.74+ ships with a deterministic lockfile.
2. **Decision 2 (Dev-flow Phase 7 sequencing)** — automatically satisfied.  Dev-flow Phase 7's only remaining `[ ]` item ("real-world bake-in on a live `just ship` run") was implicitly satisfied by v0.5.72 + v0.5.73 shipping cleanly via `uffs-ci-pipeline` (the workspace-binary form of the ship driver).  No coordination wait needed for R5.

## 8. R2 addendum — git-cliff template validation (2026-04-25)

Captured at the close of Phase R2 (PR forthcoming on `chore/release-auto-r2`).

### What was validated

`git-cliff 2.12.0` was installed via `cargo install git-cliff --locked` and run against the workspace history with the new `cliff.toml`:

```bash
git-cliff --config cliff.toml -o /tmp/uffs-cliff-full.md         # full history
git-cliff --config cliff.toml --unreleased                        # next release preview
```

Both commands exit 0.  `--unreleased` correctly reports an empty placeholder section because every conforming commit since v0.5.73 has been a `chore:` (which the parsers intentionally suppress).

### Output statistics

| Metric | Value |
|---|---|
| `git-cliff` version | `2.12.0` |
| `cliff.toml` line count | 196 |
| Generated full-history changelog | 508 lines |
| Hand-maintained `CHANGELOG.md` line count | 907 |
| Versions captured by git-cliff | 18 (every tag from `v0.5.1` to `v0.5.73`) |
| Versions in hand-maintained `CHANGELOG.md` | 9 (curated milestone subset) |
| Footer comparison links generated | 18 (initial release `v0.5.1` correctly uses `releases/tag/`, all others use `compare/{prev}...{ver}`) |

The line-count delta (508 vs. 907) is the intended divergence — the hand-maintained file uses **multi-paragraph mini-essays** for each entry (rationale, before/after, rollback, bake-in evidence) while git-cliff uses **single-line bullets from the squash subject**.  The expected loss of prose richness is a known trade-off of automation.  Phase R3+ release-plz will narrow this gap by ingesting PR descriptions into the changelog body, but the bullets-from-subjects baseline established here is the floor.

### Type → section mapping verified

Spot-checked against real commits in the generated output:

| Source commit type | Example merge | Rendered section |
|---|---|---|
| `feat:` | `Shift-left dev-flow rollout (phases 1-7) (#45)` | `### Added` ✓ |
| `fix:` | `Re-codesign macOS binaries after strip — v0.5.73 (#63)` | `### Fixed` ✓ |
| `perf:` | `Parallelize drive scan + ext-index fast path for --sort path (#38)` | `### Performance` ✓ |
| `chore(security):` | (none in current history) | (parser registered) |
| `security:` non-standard | `Cargo-vet init...weekly refresh workflow (#34)`, `supply-chain hardening...(#33)` | `### Security` ✓ (caught via the `^security(...)?:` parser added for R1a observation tolerance) |
| Suppressed types | `chore:`, `docs:`, `test:`, `build:`, `ci:`, `refactor:`, `style:`, `revert:` | omitted ✓ |
| Malformed (space in scope) | `chore(dev + ci): pre-commit ultra-strict lints... (#41)` | dropped ✓ (via `filter_unconventional = true`) |

### Whitespace + duplication issues caught and resolved during iteration

1. **Spacing on first generation**: extra blank line after `## [version]` header, missing blank line between releases.  Fixed by tightening Tera whitespace controls (`{%- ... -%}`) on the loop wrappers.

2. **Duplicate PR link**: first generation produced `Subject (#63) ([#63](URL))` because the squash subject already carries `(#63)` and the template was also adding an explicit markdown link.  Resolved by removing the explicit link block from the body template — GitHub-flavored markdown auto-links the trailing `(#NN)` natively.  `cliff.toml` now documents this decision in a leading comment block.

### Forward-compatibility check

The `tag_pattern = "v[0-9]+\\.[0-9]+\\.[0-9]+"` matches what `auto-tag-release.yml` currently emits AND what release-plz will emit in Phase R4.  No tag-format migration required for the R2 → R3 → R4 transition.

The 11-type list in `commit_parsers` is identical (modulo regex syntax) to the one in `.github/workflows/commitlint.yml`'s PR-title check.  These three files (`cliff.toml`, `commitlint.yml`, `CONTRIBUTING.md`) are now a tightly-coupled trio; adding a new type means updating all three in the same PR.

### What R2 deliberately does NOT do

- **Does NOT touch `CHANGELOG.md`**.  The hand-maintained file stays exactly as-is.  Phase R3-R4's release-plz will write the **next** release section above the existing `## [0.5.73]` header, leaving all prior entries intact.
- **Does NOT install git-cliff in CI**.  R2 is a per-developer tool right now (used to preview `--unreleased` before pushing).  R3+'s release-plz embeds git-cliff natively as a library, so no CI install step is added until then.
