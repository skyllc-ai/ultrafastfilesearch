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

## 9. R3 addendum — release-plz shadow mode validation (2026-04-25)

Captured at the close of Phase R3 (PR forthcoming on `chore/release-auto-r3`).

### What was validated

`release-plz 0.3.157` was installed via `cargo install release-plz --locked` and run against the workspace with the new `release-plz.toml`:

```bash
release-plz update --config release-plz.toml
```

Exit code 0.  Output enumerates all 13 publishable workspace members at version `0.5.73` (matching the current tag) with no proposed bumps — correct, because every commit on `main` since the v0.5.73 tag has been a `chore:` (suppressed type).

### Two structural issues surfaced and fixed (R0-pattern)

Both are the same gitignore-mismatch class of bug R0 fixed for `build/update_all_versions.rs`: a file is intentionally tracked but `.gitignore` accidentally ignores it, causing release-plz to abort with `the working directory has uncommitted changes`.

1. **Three committed-but-gitignored binary assets**: `crates/uffs-text/src/upcase_default.bin` (`include_bytes!` compile-time dependency), `crates/uffs-text/src/upcase_windows_c.bin` (Windows-captured upcase test fixture), and `tests/fixtures/drive_g/G_mft.bin` (20 MB MFT capture for `uffs-daemon` integration tests).  Resolved by adding `!crates/uffs-text/src/*.bin` and `!tests/fixtures/**/*.bin` carve-outs to `.gitignore`.

2. **`cliff.toml` footer template variable unresolved under release-plz**: the footer references `{{ remote.github.owner }}` and `{{ remote.github.repo }}`.  Standalone `git-cliff` populates these from cliff.toml's `[remote.github]` section; release-plz's embedded git-cliff doesn't (it derives the remote from `Cargo.toml`'s `repository` field instead, and only populates the variables when a GitHub API token is available).  Resolved by adding the Tera `default(value="...")` filter so the template works in both contexts.

### Output observation: per-crate changelogs by default

A successful `release-plz update` invocation produced 13 per-crate `CHANGELOG.md` files (one in each `crates/uffs-*/` directory) populated with full history from `v0.5.1` onwards, NOT the existing single workspace-root `CHANGELOG.md`.  This is release-plz's default behaviour — `changelog_path` is a per-package field, not a workspace-level field, and each package writes to a `CHANGELOG.md` in its own directory unless overridden.

This is **observation, not a defect**.  R3 deliberately ships with workspace defaults only — per-crate `[[package]]` configuration is R6 scope.  However, R4 active mode will surface this as a structural decision before the first real release PR opens:

- **Option A — keep per-crate** (release-plz default).  Each `crates/uffs-*/Cargo.toml` accompanied by its own `CHANGELOG.md`.  Discoverable from crates.io detail pages.  Existing workspace-root `CHANGELOG.md` becomes a hand-maintained "release notes / migration guide" superset.
- **Option B — flatten to workspace** (matches existing UFFS convention).  Add 13 `[[package]]` blocks to `release-plz.toml`, each with `changelog_path = "CHANGELOG.md"`.  Single source of truth.  Requires testing whether release-plz handles concurrent writes from 13 crates correctly (untested at R3 close).

Decision deferred to R4 PR opening; will be made before flipping to active mode.

### Output observation: per-crate tag scheme by default

Per release-plz docs, multi-package workspaces produce per-crate tags (`uffs-cli-v0.5.74`, `uffs-core-v0.5.74`, ...) rather than the single workspace-level `vX.Y.Z` tags `auto-tag-release.yml` currently emits.  The existing `release.yml` triggers on tag pattern `v*` which would NOT match `uffs-cli-v*`.

This is the second R4-blocking observation.  Resolution paths:
- Configure release-plz to emit a single workspace-level tag (via `git_tag_name` template override at workspace level).
- Or: update `release.yml`'s tag-trigger pattern to match the per-crate scheme.

Decision deferred to R4 PR opening.

### Three-layer dormancy verified

The R3 workflow (`.github/workflows/release-plz.yml`) was authored with three independent layers of "no production state change":

1. **Config**: `publish = false` workspace-wide in `release-plz.toml`.
2. **Workflow command**: `release-plz update` (local-only) instead of `release-pr` / `release` (which open PRs / publish).
3. **Workflow permissions**: `contents: read`, `pull-requests: read` (not `write`).

Plus a belt-and-suspenders verification step at the end of the workflow that asserts `git rev-parse HEAD` is unchanged from `${{ github.sha }}`, surfacing any accidental mutation as a workflow failure.

### Local validation summary

| Check | Result |
|---|---|
| `release-plz update --config release-plz.toml` exit code | 0 |
| All 13 workspace members enumerated | ✓ |
| `cliff.toml` footer renders without template error | ✓ (after `default(value="...")` fix) |
| Working tree clean check | ✓ (after `.gitignore` `.bin` carve-outs) |
| `actionlint .github/workflows/release-plz.yml` | passes |
| Standalone `git-cliff --config cliff.toml` still works post-fix | ✓ (footer URLs still correct) |

### What R3 deliberately does NOT do

- **Does NOT open a release PR**.  Shadow mode only.  R4 flips to active mode.
- **Does NOT publish anything to crates.io**.  Triple-locked dormancy through R8.
- **Does NOT add `[[package]]` per-crate config**.  Workspace defaults only.  R6 adds per-crate `publish = true` overrides for the 13 publishable members.
- **Does NOT install `cargo-semver-checks`**.  Disabled via `semver_check = false`.  R6 enables it as part of the metadata audit.
- **Does NOT add a schedule trigger**.  Push-to-main + workflow_dispatch is sufficient — scheduled re-runs of the same history would produce no new observations.

## 10. R3.5 + R6 addendum — `version = ` requirements + crates.io metadata audit (2026-05-07)

Captured at the close of Phase R6 (PR `feat/release-auto-r6-publishability`).

### Trigger: silent shadow-mode failure

R3 landed on 2026-04-25 and was expected to start producing meaningful per-merge `release-plz update` summaries within the next 1–2 weeks.  After 12 days the workflow had run successfully on every push to `main` but **every summary was empty**: `release-plz proposed no version bump and no changelog entry`.

A local reproduction surfaced the root cause:

```
$ release-plz update --config release-plz.toml
[ERROR] failed to determine next versions
Caused by:
    failed to verify manifest at .../crates/uffs-broker/Cargo.toml
Caused by:
    dependency `uffs-security` does not specify a version
```

`release-plz update` invokes `cargo package` per crate (to validate that the manifest can produce a tarball acceptable to crates.io).  Cargo refuses to package any crate whose `[dependencies]` entries lack a `version =` requirement, even when the entry resolves through a `path =`.  All 8 of UFFS's internal `[workspace.dependencies]` aliases were `path =` only, plus 2 direct path-deps in `crates/uffs-cli/Cargo.toml`, plus the polars git pin in `crates/uffs-polars/Cargo.toml`.

Inside the workflow the failure was swallowed by `tee /tmp/release-plz-output.log` — the `if` chain in `.github/workflows/release-plz.yml` captured the non-zero exit but the empty diff meant the summary's "no proposed changes" branch fired, masking the diagnostic from any reviewer who didn't expand the step log.

### Fix (R3.5, bundled into the R6 PR)

`Cargo.toml` (workspace root):

```diff
 [workspace.dependencies]
-uffs-polars = { path = "crates/uffs-polars" }
-uffs-security = { path = "crates/uffs-security" }
-uffs-text = { path = "crates/uffs-text" }
-uffs-time = { path = "crates/uffs-time" }
-uffs-mft = { path = "crates/uffs-mft", features = ["zstd"] }
-uffs-format = { path = "crates/uffs-format" }
-uffs-core = { path = "crates/uffs-core" }
-uffs-client = { path = "crates/uffs-client" }
+uffs-polars = { path = "crates/uffs-polars", version = "0.5.90" }
+uffs-security = { path = "crates/uffs-security", version = "0.5.90" }
+uffs-text = { path = "crates/uffs-text", version = "0.5.90" }
+uffs-time = { path = "crates/uffs-time", version = "0.5.90" }
+uffs-mft = { path = "crates/uffs-mft", version = "0.5.90", features = ["zstd"] }
+uffs-format = { path = "crates/uffs-format", version = "0.5.90" }
+uffs-core = { path = "crates/uffs-core", version = "0.5.90" }
+uffs-client = { path = "crates/uffs-client", version = "0.5.90" }
```

`crates/uffs-polars/Cargo.toml`:

```diff
-polars = { git = "...", rev = "1e9a63b9...", default-features = false, features = [...] }
+polars = { git = "...", rev = "1e9a63b9...", version = "0.53.0", default-features = false, features = [...] }
```

`crates/uffs-cli/Cargo.toml`:

```diff
-uffs-client = { path = "../uffs-client", default-features = false }
-uffs-format = { path = "../uffs-format" }
+uffs-client = { path = "../uffs-client", version = "0.5.90", default-features = false }
+uffs-format = { path = "../uffs-format", version = "0.5.90" }
```

`just/test.just` (`polars` recipe): added a post-`cargo update` step that re-derives the polars version from `cargo tree` output and edits the `version =` field in lockstep with the new `rev =`, so future polars major-version bumps don't drift the version requirement.

### Validation

Local re-run after the fix:

```
$ release-plz update --config release-plz.toml
* `uffs-polars`: 0.5.90
* `uffs-security`: 0.5.90
* `uffs-text`: 0.5.90
* `uffs-time`: 0.5.90
* `uffs-mft`: 0.5.90
* `uffs-format`: 0.5.90
* `uffs-core`: 0.5.90
* `uffs-client`: 0.5.90
* `uffs-daemon`: 0.5.90
* `uffs-mcp`: 0.5.90
* `uffs-broker`: 0.5.90
* `uffs-cli`: 0.5.90
```

12 publishable crates enumerated cleanly (no errors; uffs-diag excluded by its own `publish = false`; uffs-ci-pipeline / uffs-gen-hooks / uffs-gen-workflow excluded by their `release = false` blocks added in this PR).

### R6 deliverables in this PR

1. `[package.metadata.docs.rs]` blocks added to all 12 publishable crates.  Three platform tiers:
    - **Single-target Linux**: `uffs-time`, `uffs-text`, `uffs-format`, `uffs-polars` (no platform-gated items).
    - **Multi-target Linux + Windows**: `uffs-mft`, `uffs-core`, `uffs-daemon`, `uffs-client`, `uffs-cli`, `uffs-mcp` (all carry `#[cfg(windows)]` / `#[cfg(unix)]` divergent code paths).
    - **Multi-target Linux + Windows + macOS**: `uffs-security` (Keychain on macOS, DACL on Windows, flock on Unix — three distinct surfaces).
    - **Windows-only**: `uffs-broker` (`default-target = "x86_64-pc-windows-msvc"` because the entire crate is a Windows handle broker).
2. `crates/uffs-diag/Cargo.toml` — explicit `publish = false` (per plan §R6 step 2).  Belt-and-suspenders: blocks both release-plz and any local `cargo publish -p uffs-diag` from a developer machine.
3. `release-plz.toml` per-package overrides — `[[package]] release = false` for `uffs-ci-pipeline`, `uffs-gen-hooks`, `uffs-gen-workflow`.  These are internal CI tools (already `publish = false` at the crate level); the `release = false` is the surgical fix per plan §R6 step 2 to keep them out of release-plz's per-package iteration.
4. `.github/workflows/crates-io-dry-run.yml` — weekly + workflow_dispatch scheduled job that runs `cargo publish --dry-run -p <crate>` for every publishable crate and posts a per-crate status table to the workflow summary.  Currently runs in **ADVISORY mode** (`FAIL_ON_DRY_RUN_ERROR=false`) because two known-expected failure classes exist:
    - **Crate-name reservations not yet pushed** (R6 step 6, deferred): every dry-run fails with `no matching package named uffs-X found` until 0.0.0 stub versions are reserved on crates.io from a throwaway external workspace.
    - **polars / chrono publishability gap**: `uffs-polars` (and any crate that transitively depends on it) fails with `failed to select a version for chrono`.  Our git-pinned polars rev uses different feature ergonomics than the published `polars = "0.53.0"`; the published-form resolution pulls a `chrono-tz` chain that conflicts with our workspace `chrono` pin.  Resolution is R8 dress rehearsal scope — either flip `uffs-polars` to `publish = false` (cascades to its transitive dependents), or align chrono with crates.io polars expectations.
5. `docs/publishing.md` — DORMANT runbook covering: pre-publish checklist (per go-live decision), per-release checklist (every release post-R9), yank decisions log, post-publish smoke checks, manual fallback ordering (when release-plz is broken), and an OIDC/trusted-publisher section to be filled in during R7.

### What R6 deliberately does NOT do

- **Does NOT reserve crate names on crates.io**.  Plan §R6 step 6 explicitly mandates that reservations happen from a *throwaway* external workspace, not the UFFS repo, so the UFFS repo never carries a `publish = true` state.  The reservation operation is documented in `docs/publishing.md`'s pre-publish checklist as a prerequisite for R8.
- **Does NOT install `cargo-semver-checks`**.  Plan §R6 mentions this as part of the metadata audit; deferred to a follow-up because the CI integration is non-trivial (semver-checks needs a baseline crate to compare against, and we don't have a published v0 yet).
- **Does NOT add OIDC trusted-publisher scaffolding**.  Plan §R7 scope.
- **Does NOT flip the workspace-level `publish = false`** in `release-plz.toml`.  Plan §R8 scope.
- **Does NOT enable `FAIL_ON_DRY_RUN_ERROR=true`** in the dry-run workflow.  Toggled only when crate-name reservations and the polars-chrono gap are both resolved (post-R6 + post-R8).

### Forward-compat assertion

When R5 deletes `build/update_all_versions.rs`, the `version = "0.5.90"` strings added in this PR will need to be kept in sync with `[workspace.package].version` by release-plz (which already handles workspace-version synchronization natively, including dependency version bumps).  No manual coordination required: release-plz reads `[workspace.package].version` and propagates the new value to every internal-dep `version =` field as part of its release-PR generation.  Verified by reading release-plz source (`crates/release_plz_core/src/version.rs`).

## 11. R4 addendum — release-plz active mode + workspace-style decisions (2026-05-08)

Captured at the close of Phase R4 (PR `feat/release-auto-r4-active-mode`).

### Decisions settled before R4 opened

The R3 addendum (§9) flagged two structural decisions deferred to the R4 PR opening: per-crate vs flattened CHANGELOG.md, and per-crate vs workspace tag scheme.  Both are settled WORKSPACE-STYLE in this R4 PR.  Recorded here for the durability of the rationale.

#### D1.  Single workspace tag (`v{{ version }}`), not per-crate tags

**Default release-plz behaviour for multi-package workspaces**: `{{ package }}-v{{ version }}` per crate.  For UFFS that produces 12 tags per release (`uffs-cli-v0.5.91`, `uffs-core-v0.5.91`, …).

**Override**: workspace-level `git_tag_name = "v{{ version }}"` (and matching `git_release_name`) in `release-plz.toml`.  Single tag per cut.

**Rationale**:

1. UFFS is one product, not 12 independent crates.  All 12 publishable crates share `[workspace.package].version` (R3.5).  Per-crate tags would imply independent release cadences — a property UFFS doesn't have and isn't pursuing.
2. The existing `release.yml` workflow uses `on: push: tags: [v*]` — the v0.5.90 series.  Per-crate tags would require re-architecting `release.yml`'s trigger filter AND its asset-upload logic to deduplicate the 12 simultaneous tag pushes.
3. The existing `CHANGELOG.md` uses `## [0.5.71] - 2026-04-19` per-version sections, not per-crate-per-version.  Single tag aligns with single CHANGELOG.

**Ecosystem precedent**: matches `cargo` (one tag, one CHANGELOG, multi-crate workspace) and `rustls` (same shape).  Diverges from `tokio` (per-crate tags + per-crate CHANGELOG) because UFFS releases lockstep, tokio doesn't.

**Reversibility**: low cost.  If we ever need per-crate tags (e.g. R5+ era when one crate needs an out-of-band security fix), flip `git_tag_name` back to default + update `release.yml`'s trigger filter in the same PR.

#### D2.  Single workspace-root CHANGELOG.md, not per-crate CHANGELOGs

**Default release-plz behaviour**: each crate gets its own `<crate>/CHANGELOG.md` written by release-plz.

**Override**: 12 per-package `[[package]]` blocks in `release-plz.toml` with `changelog_path = "CHANGELOG.md"` (relative to workspace root).  All 12 publishable crates write to the same workspace-root `CHANGELOG.md`.  `changelog_path` cannot be set at workspace level — release-plz docs explicitly forbid it ("This field cannot be set in the [workspace] section").

**Rationale**:

1. UFFS has had a single hand-maintained `CHANGELOG.md` since v0.4.x.  Splitting into 12 per-crate files would scatter user-facing release notes across the workspace and require crates.io detail pages to point at different files per crate.
2. Per-crate CHANGELOGs make sense when crates have independent release cadences — `tokio`'s pattern.  UFFS doesn't.
3. The cliff.toml template (R2) renders per-version sections (`## [0.5.91]`) with subsections grouped by type (Added / Fixed / Performance / Security / Breaking).  When release-plz iterates the 12 publishable crates and asks git-cliff to render each crate's slice, all 12 produce the same `## [0.5.91]` block (template is package-agnostic).  release-plz writes the same content 12 times to the same file — idempotent in practice.

**Trade-off acknowledged**: per-crate crates.io detail pages will link to the workspace-root CHANGELOG (covering all 12 crates' history) rather than a crate-specific changelog.  This is the pattern `polars`, `cargo`, and `rustls` use, so it's familiar to crates.io readers.

**Reversibility**: medium cost.  Removing 12 `[[package]]` blocks reverts to per-crate CHANGELOGs, but the per-crate files would not auto-populate from history — release-plz only writes new entries going forward.  Would require either (a) hand-curating 12 per-crate CHANGELOGs at split time, or (b) accepting empty per-crate CHANGELOGs that grow only post-split.

### Decisions settled at the same time

#### D3.  `git_only = true` workspace baseline

**Why**: UFFS is unpublished through R8.  release-plz's default behaviour queries crates.io for the previous published version per crate.  With nothing published, that's empty → release-plz silently treats every crate as "initial release" and proposes no bump even when the conventional-commit history would warrant one.  Symptom in CI: PR #145 merge ran `release-plz update` on `cccf4f111`, run [25528382935](https://github.com/skyllc-ai/UltraFastFileSearch/actions/runs/25528382935) — proposed `next version is 0.5.90` for all 12 crates despite ≥1 `fix(daemon):` commit since v0.5.90.

**Override**: `git_only = true` workspace-level.  release-plz uses git tags as the baseline instead of crates.io.

**Forward-compat note for R8**: `git_only = true` and `publish = true` cannot both be true on the same package — release-plz refuses that combination by design.  When R8 publishes the first crate, the R8 PR will either flip `git_only = false` workspace-wide (once ≥1 crate is published) OR carry per-package `git_only` overrides for the remaining unpublished crates during the staggered rollout.

#### D4.  `release_commits` regex filter

**Why**: every push to `main` (including `chore:`, `docs:`, `ci:`, `build:` housekeeping) would re-open the release PR with a fresh version-bump preview, producing churn and noise in the PR list.

**Override**: `release_commits = "^(feat|fix|perf|security)(\\(.+\\))?:"` workspace-level.

**Single source of truth**: this regex matches the same set of commit types that `cliff.toml`'s `commit_parsers` maps to changelog sections (Added / Fixed / Performance / Security / Breaking).  All other types (`chore`, `docs`, `test`, `build`, `ci`, `refactor`, `style`, `revert`) are skipped as both changelog entries (cliff.toml) AND release-trigger commits (here).

**Branch protection note**: the regex INTENTIONALLY excludes the `^build(\(release-automation\))?:` infra commits (R0 through R6) so the release-automation refactor itself doesn't trigger phantom release PRs while it's still in flight.  After R4+R5 land, those commits stop being typical anyway.

#### D5.  Two-job workflow structure

**Why**: `release-plz/action` does NOT have a single "do both" command.  The action's `command:` input takes EITHER `release-pr` OR `release`, never both at once.  release-plz docs and the release-plz repo's [own release-plz.yml](https://github.com/release-plz/release-plz/blob/main/.github/workflows/release-plz.yml) use TWO separate jobs.

**Implementation**: `.github/workflows/release-plz.yml` ships with two parallel jobs:
- `release-plz-pr` (`command: release-pr`, `permissions: { contents: write, pull-requests: write }`) — runs on every push, opens or updates the release PR.
- `release-plz-release` (`command: release`, `permissions: { contents: write }`) — runs on every push, no-ops unless HEAD is the merge of the release PR.

#### D6.  Default `GITHUB_TOKEN`, not GitHub App / PAT (DOCUMENTED LIMITATION)

**Why minimal infra**: R4 ships with workflow-provided `GITHUB_TOKEN`, NOT a GitHub App or PAT.  Zero new secrets, zero new infra.

**Cost**: tags created by release-plz via `GITHUB_TOKEN` do NOT trigger downstream workflows (per GitHub's anti-loop policy).  `release.yml` (`on: push: tags: [v*]`) won't auto-fire after release-plz creates a tag.

**Workaround for the bootstrap**: maintainer manually pushes the v0.5.91 bootstrap tag — that's a user-driven push, NOT a GITHUB_TOKEN push, so `release.yml` fires normally.

**Workaround for steady-state**: a follow-up PR (informally "R4.5") sets up a GitHub App per release-plz's recommended pattern, restoring full automation.  Three options ranked in `release-plz.yml`'s header comment:
- **A. GitHub App** (canonical fix per release-plz docs).  Requires `APP_ID` + `APP_PRIVATE_KEY` secrets, app installation on the repo.  ~30 minutes of one-time setup.
- **B. PAT** stored as `RELEASE_PLZ_TOKEN` secret.  Simpler but ties release authority to a person.
- **C. `release.yml` adds `on: workflow_run` trigger**.  Fully automatic but requires `release.yml` changes outside R4 scope.

Option A is recommended.  Tracked but not blocked by R4 itself.

### Maintainer bootstrap procedure (out of scope for R4 PR, scoped here for clarity)

The R4 PR does NOT bump any crate version.  The first release-plz active-mode run on `main` after R4 lands will propose `no version bump` for all 12 crates because release-plz's `git_only` baseline check fails on the v0.5.90 worktree (which predates the R3.5 fix).  This is a self-healing transient — once a fresh tag exists, future runs work normally.

To bootstrap v0.5.91 manually:

1. From a fresh `git checkout main && git pull` on a clean working tree:
   ```bash
   git checkout -b chore/bootstrap-v0.5.91
   ```
2. Bump `[workspace.package].version` from `0.5.90` to `0.5.91` in the **workspace root `Cargo.toml`** (one line change).  All 12 publishable crates inherit via `version.workspace = true`.
3. Add a `## [0.5.91] - <today>` block to `CHANGELOG.md` near the top (above the existing `## [0.5.90]` block).  Hand-curate sections (Added / Fixed / Performance) summarizing changes since v0.5.90 — git-cliff can preview the auto-generated content via:
   ```bash
   git cliff --config cliff.toml --unreleased --tag v0.5.91
   ```
4. `cargo update -w` to refresh `Cargo.lock`.
5. `cargo check --workspace --all-targets` to confirm nothing broke.
6. Commit + open PR + merge through normal review.
7. After PR merges, locally fetch the merged `main` and tag it:
   ```bash
   git fetch origin main && git checkout main && git pull --ff-only
   git tag -s -m "Release v0.5.91" v0.5.91
   git push origin v0.5.91
   ```
   The tag push (user-driven, not GITHUB_TOKEN) triggers `release.yml`.
8. Wait for `release.yml` to complete and produce the GitHub Release with binaries.
9. From this point on, future `feat:` / `fix:` merges auto-trigger the `release-plz-pr` job, opening the release PR for v0.5.92 automatically.  Maintainer reviews + merges → `release-plz-release` creates the v0.5.92 tag.  **But note D6**: until R4.5 lands, the v0.5.92 tag won't auto-trigger `release.yml` — maintainer pushes the tag manually OR re-runs `release.yml` with `workflow_dispatch`.

### What R4 deliberately does NOT do

- **Does NOT bump any crate version**.  Bootstrap is out-of-band.
- **Does NOT delete `auto-tag-release.yml`**.  R5 scope (after ≥2 R4-flow releases bake in).
- **Does NOT set up a GitHub App / PAT**.  R4.5 follow-up.
- **Does NOT publish anything to crates.io**.  Workspace `publish = false` + missing `CARGO_REGISTRY_TOKEN` are unchanged from R3 → R6.
- **Does NOT modify `release.yml`**.  The downstream binary-build workflow stays exactly as-is; the trigger contract is preserved.
