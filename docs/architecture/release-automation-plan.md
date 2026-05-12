<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.

UFFS — Release Automation Implementation Plan
-->

# Release Automation Implementation Plan (v1)

> Sibling document to
> [`dev-flow-implementation-plan.md`](dev-flow-implementation-plan.md).
> Where `dev-flow-implementation-plan.md` describes the **CI / gate**
> architecture (what it takes to merge a PR safely), this document
> describes the **release / versioning** architecture (what it takes
> to ship a versioned release safely and, eventually, to publish
> crates to `crates.io`).

## 0. TL;DR

| Aspect | Today | Target |
|---|---|---|
| Version source of truth | `[workspace.package].version` in root `Cargo.toml` (correct — all 12 crates inherit via `version.workspace = true`) | **Unchanged** |
| Bumping mechanism | `build/update_all_versions.rs` (1028-line hand-rolled rust-script) invoked by `just ship` via `scripts/ci-pipeline/src/version.rs` | **`release-plz` GitHub Action** computing bumps from conventional commits |
| Changelog | `CHANGELOG.md` `## [Unreleased]` section maintained **by hand** | **Generated** by `git-cliff` via `cliff.toml` template, assembled into `CHANGELOG.md` by release-plz |
| Release PR | None — version bump lands directly on `main` via `just ship` → commit → push | **Release-plz PR**: `chore(release): prepare vX.Y.Z` opened automatically, human-reviewed, merged manually |
| Tag creation | `auto-tag-release.yml` watches for `Cargo.toml` diff, dispatches `release.yml` | **release-plz** creates the tag directly after the release PR merges; `release.yml` keeps its tag-triggered path |
| Binary distribution | `release.yml` (existing, ~780 lines, works well) | **Unchanged** — `release.yml` remains the binary-producing workflow |
| crates.io publishing | `publish = true` accidentally set in `[workspace.metadata.release-plz]`, no workflow actually running | **Scaffolding only**: `publish = false` workspace-wide, dry-run validation in CI, docs.rs metadata per crate, trusted-publishing (OIDC) prep docs. **Actual publish: deferred to a separate plan.** |
| Commit convention | Conventional commits used consistently by habit, not enforced | Advisory PR comment from day 1; mandatory gate after 1 month of observation |

**Central mental shift**:

> *"Stop bumping versions. Start describing changes. The version
> computes itself."*

## 1. Goals and non-goals

### 1.1 Goals (in scope for this plan)

1. **Eliminate bespoke version tooling.** Delete
   `build/update_all_versions.rs` and the version-bump path of
   `scripts/ci-pipeline/src/version.rs`.  Replace with
   `release-plz` + `git-cliff` + conventional commits.
2. **Automate changelog generation** so `CHANGELOG.md` is
   derivable from commit history, not a hand-maintained artifact
   that drifts.
3. **Human-reviewable release cadence**: every release goes through
   a PR (`chore(release): prepare vX.Y.Z`) that a human reviews
   before merging.  No silent publish-on-push.
4. **Preserve the existing `release.yml` binary pipeline** as-is.
   Release-plz integrates **upstream** of it (opens the version PR
   and eventually creates the tag); `release.yml` continues to do
   what it does well: build signed binaries, emit SLSA attestations,
   publish GitHub releases.
5. **Scaffold crates.io publishing end-to-end** so that when the
   project decides to publish, the switch is a **configuration
   flip** (`publish = false` → `publish = true` per crate), not
   weeks of setup:
   - Per-crate metadata complete (`description`, `keywords`,
     `categories`, `readme`, `license`)
   - `[package.metadata.docs.rs]` per crate for docs.rs
   - `cargo publish --dry-run` in CI catches metadata drift
   - Trusted publishing (OIDC) path documented; secrets slots
     named but empty
   - Publish order (dependency DAG) documented
   - First-publish checklist written
6. **Formalize conventional commits** in `CONTRIBUTING.md` with
   concrete examples, soft-enforced by a PR-level advisory bot
   initially, hard-enforced once the discipline is proven stable.
7. **Every step reversible.** No phase in this plan is committed
   such that the next phase can't be rolled back to the previous
   state with a single `git revert`.

### 1.2 Non-goals (explicitly out of scope)

1. **Actually publishing to crates.io.**  We're building the
   ramp; taking off is a separate decision recorded in a separate
   plan.  When that happens, the publish plan will reference this
   plan's §5 ("crates.io scaffolding deep-dive") as its prerequisite
   state.
2. **`cargo-dist` adoption.**  The workspace already has
   `[workspace.metadata.dist]` config (probably leftover from prior
   exploration), but `release.yml` already does everything cargo-dist
   would: builds for 5 targets, signs artifacts, emits SLSA
   attestations, uploads to GitHub Releases.  Adding cargo-dist would
   create a second, parallel binary pipeline.  **Explicitly declined.**
   See §12 "Non-goals deep-dive" for the comparison and rationale.
3. **Independent per-crate versioning.**  Today all 12 crates share
   the workspace version `0.5.71`.  Independent versions (each crate
   evolving on its own SemVer trajectory) is the pattern for mature
   published libraries.  UFFS is currently a binary app; per-crate
   versions don't add value.  Deferred to Phase R9, which is itself
   deferred until approaching v1.0.
4. **Breaking-change detection via API diffing** (`cargo-semver-checks`,
   `public-api`).  Worth revisiting once publishing is live and the
   stable-API contract becomes observable.  Not needed before then.
5. **Automated pre-release / nightly publish to crates.io** (`-alpha`,
   `-rc` channel tooling).  Stable releases only for the foreseeable
   future; pre-release infrastructure added on-demand.
6. **Signing the `Cargo.lock` / SBOM-driven release.**  The existing
   `release.yml` already handles SLSA provenance attestation and
   SHA256 manifests; this plan doesn't touch artifact signing.
7. **Monorepo-style versioning tools from non-Rust ecosystems**
   (changesets, semantic-release, standard-version).  Rust-native
   tooling (release-plz + git-cliff) is purpose-built for Cargo
   workspaces and integrates with crates.io semantics.  Cross-ecosystem
   tools always miss Rust-specific concerns (workspace inheritance,
   `cargo publish` ordering, cross-crate dependency version updates).
8. **Cross-target strict-lint convergence.**  Upgrading the Windows
   lint gate from `cargo check` to `cargo clippy -- -D warnings`
   (1,346-lint backlog) and adding a native macOS → Linux cross-check
   path are the concern of
   [`windows-clippy-and-linux-cross-plan.md`](windows-clippy-and-linux-cross-plan.md).
   Release-automation honours that work by running the post-R5
   release pipeline against a Windows-clippy-clean `main`; the two
   plans share no workflows or tooling surface.

### 1.3 Success criteria

The plan is complete when **all** of the following hold
simultaneously, without regression:

- Opening a PR with a `feat:` commit and merging it causes release-plz
  to open a release PR within 10 min of `main` landing the commit,
  with:
  - `Cargo.toml` workspace version bumped (minor increment)
  - `CHANGELOG.md` updated with the new version header and the
    commit's subject line categorized under "Features"
  - `Cargo.lock` refreshed
  - PR body containing the full changelog-style diff
- Merging the release PR creates the `vX.Y.Z` tag on `main`, which
  fires `release.yml` via its `push: tags: v*` trigger, producing
  a GitHub Release with signed binaries within 45 min.
- No step in the above flow touches `build/update_all_versions.rs`
  (which is deleted) or `scripts/ci-pipeline/src/version.rs`'s
  version-bump code path (which is deleted).
- Running `cargo publish --dry-run --workspace` in CI succeeds on a
  scheduled nightly job, proving per-crate metadata is valid for
  crates.io.
- `publish = false` explicitly set in every crate's `Cargo.toml`
  and `[workspace.metadata.release-plz]`, preventing accidental
  publish.
- `CONTRIBUTING.md` has a "Commit convention" section citing
  [Conventional Commits 1.0.0](https://www.conventionalcommits.org/)
  with concrete UFFS examples, and a commit-msg CI check that posts
  advisory comments on non-conforming PR title commits.
- Reverting **any** of R0-R7 in isolation restores the project to a
  functional state (versioning still works, releases still ship).

## 2. Current state audit

### 2.1 The version source of truth

`@/Users/rnio/Private/Github/UltraFastFileSearch/Cargo.toml:46-47`

```@/Users/rnio/Private/Github/UltraFastFileSearch/Cargo.toml:46-47
[workspace.package]
version = "0.5.71"
```

All 12 member crates inherit correctly:

```toml
# Pattern repeated across every crate/*/Cargo.toml:
[package]
name = "uffs-<name>"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
```

Verified via grep on 2026-04-24: every crate in `crates/` plus
`scripts/ci-pipeline/` uses workspace inheritance.  **The
inheritance mechanism itself is correct and stays.**

### 2.2 The bespoke bumper

`@/Users/rnio/Private/Github/UltraFastFileSearch/build/update_all_versions.rs`
is a **1028-line rust-script** (`#!/usr/bin/env rust-script`) that:

1. Reads `[workspace.package].version` via hand-rolled TOML scanning
2. Parses the SemVer triple (`MAJOR.MINOR.PATCH`)
3. Increments patch / minor / major based on CLI arg (default: patch)
4. Rewrites `Cargo.toml` using **4 spacing-variant patterns** to
   match whatever formatting was on disk:

```@/Users/rnio/Private/Github/UltraFastFileSearch/build/update_all_versions.rs:674-679
    let patterns = [
        format!("version = \"{}\"", current),           // Standard spacing (most common)
        format!("version       = \"{}\"", current),     // Aligned spacing (formatted)
        format!("version=\"{}\"", current),             // No spaces (compact)
        format!("version\t= \"{}\"", current),          // Tab spacing (legacy)
    ];
```

5. Updates `README.md` with **5 regex-like patterns** for version
   badges, git tags, prose mentions, dependency declarations, and
   TOML examples.
6. Updates `CHANGELOG.md` version references.
7. Updates `docs/*.md` files.

The rust-script approach was pragmatic at inception (single file,
no external deps, runnable directly) but has grown into a
**maintenance liability**:

- 4 variant patterns for a single Cargo.toml field — every patterns
  list is a place for new Cargo.toml formatting styles to silently
  not match.
- 5 README patterns — a prose-heavy README makes it easy to forget
  a reference; the script won't flag unmatched mentions.
- No validation that the NEW version string is itself a valid
  SemVer (it parses the OLD version then formats the new one, but
  no round-trip check).
- The script is duplicated in Rust in
  `scripts/ci-pipeline/src/version.rs` (which shells out to the
  rust-script rather than reimplementing — reasonable, but means
  two files claim authority over versioning).

**Observed bugs in the bespoke bumper** (informing the decision
to retire rather than patch it):

- **`Cargo.lock` drift after bump** — the script edits
  `[workspace.package].version` in `Cargo.toml` but does NOT run
  `cargo check` / `cargo generate-lockfile` afterwards, so
  `Cargo.lock`'s `[[package]]` entries for the 12 internal crates
  keep the OLD version string.  Observed on `origin/main` at
  `v0.5.69`-era: workspace `Cargo.toml` = `0.5.69`, `Cargo.lock`
  internal entries = `0.5.68`.  Self-heals intermittently (any
  subsequent `cargo` invocation that touches the lockfile —
  Dependabot dep bump, CI cold run, `just check` locally —
  silently rewrites the internal versions), which is worse than a
  hard failure because **the drift escapes notice for multiple
  releases**.  Breaks the "tagged release is exactly reproducible
  from its `Cargo.lock`" invariant for whichever releases shipped
  before the self-heal fired.  The one-line fix on the bespoke
  side is to run `cargo generate-lockfile --offline` (or a plain
  `cargo check`) after the Cargo.toml edits; see R0 for the
  interim-patch option.  **Phase R5 retires the entire script so
  this bug disappears structurally**: release-plz's release PR
  always includes the `Cargo.lock` diff alongside the `Cargo.toml`
  diff because release-plz invokes `cargo update --workspace`
  (via `dependencies_update = true`) as part of preparing the PR.
- **Hardcoded patch-bump regardless of commit types** — see §2.3.
  `feat:` commits silently become patch bumps, violating SemVer
  expectations for any future library consumer.  Release-plz's
  commit-driven bump computation fixes this structurally.
- **No verification that the README / CHANGELOG / doc pattern
  sweeps actually matched** — if one of the 5 README patterns
  silently misses (because the README got reformatted), the
  script exits 0 and the stale version string stays in prose.
  Caught by eyeballing; not caught by CI.  Release-plz doesn't
  touch prose files, so this failure mode disappears by
  construction (README prose version references should be
  removed in R6 README rewrites).

### 2.3 The `just ship` flow

```@/Users/rnio/Private/Github/UltraFastFileSearch/scripts/ci-pipeline/src/version.rs:88-101
pub(crate) async fn increment_version() -> Result<()> {
    println!("📈 Incrementing version...");
    let output = Command::new("./build/update_all_versions.rs")
        .arg("patch")
        .output()
        .await
        .context("Failed to execute version update script")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Version bump failed: {stderr}");
    }
    println!("✅ Version incremented successfully");
    Ok(())
}
```

The `just ship` command (via the `ci-pipeline` binary) runs a
**pipelined flow**: version bump → commit → push to a release
branch → open PR.  The version bump is **hardcoded to `patch`**
regardless of what the commits since the last release actually
did — `feat:` commits silently become patch bumps, breaking SemVer
expectations for any future consumer.

### 2.4 The tag/release handoff

`@/Users/rnio/Private/Github/UltraFastFileSearch/.github/workflows/auto-tag-release.yml`
(169 lines) watches for `Cargo.toml` changes on `main` and, if
`[workspace.package].version` differs from `HEAD~1`, dispatches
`release.yml` via `gh workflow run` with the new version.  It does
NOT push the tag itself — `release.yml` creates and pushes the tag
after a successful build, making the tag a "release succeeded"
marker rather than an intention.

This two-workflow split is **actually well-designed**: the tag
means "a release was built and published", nothing weaker.
Release-plz can participate without disturbing this invariant —
see §3 for how.

### 2.5 The existing release-plz config hint

`@/Users/rnio/Private/Github/UltraFastFileSearch/Cargo.toml:641-649`

```@/Users/rnio/Private/Github/UltraFastFileSearch/Cargo.toml:641-649
# Workspace-level release-plz configuration
[workspace.metadata.release-plz]
git_release_enable = true
git_tag_enable = true
changelog_update = true
dependencies_update = true
allow_dirty = false
publish = true
git_release_draft = false
```

This section is **dead code** (no release-plz workflow exists) but
shows prior thinking about this migration.  The `publish = true`
setting is the most dangerous residue: if release-plz were
activated with the current config, it would attempt to publish all
12 crates to crates.io immediately.  **Must be flipped to
`publish = false` as the first scaffolding step.**

### 2.6 The cargo-dist config hint

`@/Users/rnio/Private/Github/UltraFastFileSearch/Cargo.toml:652-664`

```@/Users/rnio/Private/Github/UltraFastFileSearch/Cargo.toml:652-664
# Workspace-level cargo-dist configuration
[workspace.metadata.dist]
cargo-dist-version = "0.30.0"
ci = ["github"]
installers = ["shell", "powershell"]
targets = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
]
pr-run-mode = "plan"
allow-dirty = ["ci"]
```

Another dead-code block.  `cargo-dist` competes directly with
`release.yml` — both build cross-platform binaries and emit
GitHub Releases.  `release.yml` is **already in production** and
handles things cargo-dist does NOT natively do (SLSA provenance
attestation, sccache integration, per-target rustflags baselines,
Windows-specific cross-compile via xwin).  **Declined in §12.**
Block will be deleted as part of Phase R0 pre-flight.

### 2.7 The changelog

`@/Users/rnio/Private/Github/UltraFastFileSearch/CHANGELOG.md` uses
the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format
with a manually-edited `## [Unreleased]` section at the top.  789
lines as of 2026-04-24, with high-quality prose but clearly
maintained by hand — recent entries describe CI cutover work in
fine-grained detail that came from the PR descriptions, not an
automated harvest.

**High information density per entry, but also high
maintenance cost and drift risk**: if a PR merges without someone
editing CHANGELOG.md, the change silently omits from the next
release's notes.  Observed at least twice in the last 10 PRs
(PR #51 and #52 landed without `## [Unreleased]` updates; caught
only by tribal memory when compiling release notes later).

### 2.8 Conventional commits adoption

Scanning recent merges on `main`:

```
0e811d0bb fix(tests): correct stale expected values in Windows-gated unit tests (#55)
ea65c5f84 docs(dev-flow): flip Phase 5 dashboard row 🟡 → ✅ + add sub-status (#56)
2d3a7f5b3 fix(preview): complete Phase 5 re-bake — windows-latest move + RC_PATH fix (#52)
b9a67f2dc docs(dev-flow): Phase 5 live bake ticks + preview-artifacts.yml robustness fixes (#51)
294057973 chore(ci): retire stale ci.yml references across workspace (#50)
1edf12ff1 docs(dev-flow): reconcile plan with post-cutover live state (v3.2) (#49)
6f99b86aa feat(ci): cutover to pr-fast.yml + ruleset — retire ci.yml (#48)
eef3359b2 chore(ci): actions hardening retrofit across workflows (#47)
```

**8/8 recent merges follow conventional-commit format** — `fix:`,
`docs:`, `chore:`, `feat:`, all with appropriate scope.  The
discipline is already there; the tooling just doesn't consume it.

This is the single most important enabler for the entire plan:
**release-plz works immediately** against a history like this.
No backfill of commit messages needed.

### 2.9 Per-crate metadata readiness for crates.io

Cross-referencing crate Cargo.toml files against crates.io's
publication requirements:

| Field | Workspace source | Status |
|---|---|---|
| `name` | per-crate (unique) | ✅ all unique, all present |
| `version` | `workspace.package` | ✅ inherited |
| `edition` | `workspace.package` | ✅ inherited (edition = 2024) |
| `rust-version` | `workspace.package` | ✅ inherited (1.91) |
| `license` | `workspace.package` | ✅ MPL-2.0 (valid SPDX identifier, crates.io accepts) |
| `repository` | `workspace.package` | ✅ `https://github.com/skyllc-ai/UltraFastFileSearch` |
| `authors` | `workspace.package` | ✅ `Robert Nio <…@users.noreply.github.com>` |
| `description` | `workspace.package` (has one) + per-crate override | ⚠️ per-crate has it; workspace-level description is for the CLI, may not fit library crates |
| `documentation` | `workspace.package` | ⚠️ points at `https://docs.rs/uffs` which doesn't exist yet |
| `readme` | `workspace.package` = `"README.md"` | ⚠️ per-crate READMEs missing; the root README is app-focused, not library-crate-focused |
| `keywords` | `workspace.package` | ⚠️ workspace has 5; library crates each have max 5; may need per-crate overrides |
| `categories` | `workspace.package` | ⚠️ same: per-crate overrides may differ (e.g. `uffs-text` is `text-processing`, not `filesystem`) |
| `publish` | Cargo.toml workspace root has `publish = true` (in release-plz config), crates themselves have NO explicit `publish =` key | ⚠️ cargo defaults to `publish = true` — **AN ACCIDENTAL `git tag` + CI trigger today could publish!**  Must be addressed in R0. |

`Cargo.toml:[workspace.package]`:

```@/Users/rnio/Private/Github/UltraFastFileSearch/Cargo.toml:54-60
repository = "https://github.com/skyllc-ai/UltraFastFileSearch"
authors = ["Robert Nio <50460704+githubrobbi@users.noreply.github.com>"]
description = "UFFS - Ultra Fast File Search using direct NTFS MFT reading and Polars DataFrames"
documentation = "https://docs.rs/uffs"
readme = "README.md"
keywords = ["mft", "ntfs", "file-search", "windows", "polars"]
categories = ["filesystem", "command-line-utilities"]
```

Library-crate metadata gaps will be filled in Phase R6.

### 2.10 Licensing posture

- `@/Users/rnio/Private/Github/UltraFastFileSearch/LICENSE` — MPL-2.0 full text at root.
- `@/Users/rnio/Private/Github/UltraFastFileSearch/LICENSES/MPL-2.0.txt` — REUSE-compliant license file.
- `@/Users/rnio/Private/Github/UltraFastFileSearch/LICENSES/LicenseRef-UFFS-Brand.txt` — custom trademark license for the UFFS brand (not software).
- `@/Users/rnio/Private/Github/UltraFastFileSearch/TRADEMARK.md` — trademark policy.
- All source files carry an MPL-2.0 SPDX license header (verified by `reuse lint` in PR Fast CI).

**crates.io implications**:

- MPL-2.0 is an [OSI-approved license](https://opensource.org/license/mpl-2-0/)
  and accepted by crates.io without special handling.
- The `LicenseRef-UFFS-Brand` applies only to the brand, not the
  software — non-blocking for publishing.
- The per-file SPDX headers satisfy the "source traceability"
  expectation but are not required by crates.io.

**Verdict**: licensing is clean.  No changes needed for crates.io
readiness.

## 3. Target architecture

### 3.1 End-state flow diagram

```
Contributor                        main branch                GitHub Actions
─────────────                      ──────────                ──────────────

 writes feat: /                       ┌────────────┐
 fix: commits                         │  commits   │         (PR Fast CI runs per-PR,
      │                               │  on main   │          conventional-commit
      ▼                               │            │          lint as advisory)
 opens PR  ─── PR Fast CI ─────────▶  │            │
      │    (required; blocks merge)   │            │
      ▼                               │            │
 PR merges                            │            │
                                      └─────┬──────┘
                                            │
                                            ▼
                                     release-plz workflow
                                     (runs on every push to main)
                                            │
                                            ├── Reads conventional commits
                                            │   since last vX.Y.Z tag
                                            │
                                            ├── Computes next version:
                                            │     feat!: → MAJOR
                                            │     feat:  → MINOR
                                            │     fix:   → PATCH
                                            │     other: no release
                                            │
                                            ├── git-cliff assembles
                                            │   CHANGELOG.md sections
                                            │   from cliff.toml template
                                            │
                                            ▼
                              ┌──────────────────────────┐
                              │  release PR opened:      │
                              │  chore(release):         │
                              │    prepare v0.5.72       │
                              │                          │
                              │  files changed:          │
                              │    Cargo.toml (version)  │
                              │    Cargo.lock (refresh)  │
                              │    CHANGELOG.md (generated) │
                              └──────────┬───────────────┘
                                         │
                 human reviews ◀─────────┤
                 changelog, decides ──── ┤
                 to merge / amend        │
                                         ▼
                              release PR merges into main
                                         │
                                         ▼
                              release-plz workflow
                              (triggered by merge of
                               release PR)
                                         │
                                         ├── Creates + pushes tag
                                         │   v0.5.72 on merge commit
                                         │
                                         ├── Creates GitHub Release
                                         │   (draft or published)
                                         │
                                         ├── (if publish = true per crate)
                                         │   cargo publish each crate
                                         │   in dependency-DAG order
                                         │
                                         ▼
                              release.yml workflow
                              (triggered by push: tags: v*)
                                         │
                                         ├── Builds binaries for 5 targets
                                         ├── Signs + attests (SLSA)
                                         ├── Uploads to GitHub Release
                                         └── Done.
```

### 3.2 Which workflows exist in the target state

| Workflow | Purpose | Target status |
|---|---|---|
| `pr-fast.yml` | Per-PR gate (format, clippy, tests, etc.) | ✅ unchanged |
| `preview-artifacts.yml` | Label-gated Windows preview binaries | ✅ unchanged |
| `release.yml` | Tag-triggered binary build + GH Release | ✅ unchanged |
| `tier-2.yml` | Weekly cron (coverage, miri, udeps) | ✅ unchanged |
| `codeql.yml` | Security analysis | ✅ unchanged |
| `dependabot-review.yml` | Dependabot PR validation | ✅ unchanged |
| `dependabot-auto-merge.yml` | Auto-merge dependabot PRs | ✅ unchanged |
| `cargo-vet-refresh.yml` | Cargo-vet upkeep | ✅ unchanged |
| `auto-tag-release.yml` | **Version-diff → release.yml dispatcher** | 🟡 **RESTORED** post-R5 rollback (PR [#160](https://github.com/skyllc-ai/UltraFastFileSearch/pull/160), 2026-05-09) to keep the bespoke `just ship` flow operational while release-plz stays deferred (§R5 rollback deviation row).  Will be deleted again when R5 re-lands (i.e. once `cargo package -p uffs-polars` succeeds end-to-end and `release-plz.yml`'s `push:` trigger is re-enabled). |
| `release-plz.yml` | **NEW** — release PR generator + release cutter | ➕ created in Phase R3 (shadow) → R4 (active) |
| `commitlint.yml` | **NEW** — advisory conventional-commits check on PR titles | ➕ created in Phase R1 |
| `crates-io-dry-run.yml` | **NEW** — scheduled `cargo publish --dry-run --workspace` | ➕ created in Phase R6 |

### 3.3 Which config files exist in the target state

| File | Purpose | Phase |
|---|---|---|
| `release-plz.toml` | Release-plz workspace config (supersedes `[workspace.metadata.release-plz]`) | R3 |
| `cliff.toml` | git-cliff template for CHANGELOG.md sections | R2 |
| `.github/workflows/release-plz.yml` | Release-plz action invocation | R3 / R4 |
| `.github/workflows/commitlint.yml` | Commit convention check | R1 |
| `.github/workflows/crates-io-dry-run.yml` | Metadata-drift detection | R6 |
| `docs/publishing.md` | First-time crates.io publish runbook (dormant) | R6 |

### 3.4 Which files get deleted

| File | Lines | Reason |
|---|---|---|
| `build/update_all_versions.rs` | 1028 | Replaced by release-plz | R5 |
| `scripts/ci-pipeline/src/version.rs` (bump-related functions) | ~140 | Replaced by release-plz | R5 |
| `.github/workflows/auto-tag-release.yml` | 169 | Replaced by release-plz's tag step | R5 (deleted in R5, **restored** post-rollback via PR [#160](https://github.com/skyllc-ai/UltraFastFileSearch/pull/160) — re-deleted when R5 re-lands) |
| `[workspace.metadata.dist]` section in `Cargo.toml` | 13 | Unused cargo-dist config | R0 |

Gross deletion: **~1350 lines of bespoke tooling removed.**

## 4. Phase-by-phase plan

Each phase is designed to be:

- **Single-PR-merge-sized**: each phase is ONE PR that can be
  reviewed in one sitting.
- **Reversible**: `git revert` on the phase's merge commit
  restores the prior state with no manual cleanup.
- **Independently shippable**: subsequent phases depend on
  earlier ones, but skipping or pausing the plan between phases
  leaves the project functional (just partially modernized).
- **Bake-in-on-main**: each phase lands on `main` and runs for
  at least one real release cycle before the next phase
  activates irreversible changes.

### Phase R0 — Pre-flight audit and dangerous-config removal

**Scope**: remove the two dead-code blocks in `Cargo.toml` that
could bite us if release-plz or cargo-dist were ever accidentally
activated, and record baseline metrics.

**Steps**:

1. Delete the `[workspace.metadata.release-plz]` block from
   `Cargo.toml` — we'll reintroduce it in Phase R3 as a proper
   standalone `release-plz.toml` with `publish = false`.  Keeping
   it in place with `publish = true` is a footgun.
2. Delete the `[workspace.metadata.dist]` block from `Cargo.toml`
   — cargo-dist is explicitly non-goal (see §12).
3. Capture baseline metrics into a new file
   `docs/architecture/release-automation-baseline.md`:
   - Current workspace version: `0.5.71`
   - Number of merges since last release-worthy commit (for future
     calibration of "release cadence")
   - `auto-tag-release.yml` invocation count in last 30 days
     (`gh run list --workflow auto-tag-release.yml --limit 30`)
   - `release.yml` invocation count + success rate in last 30 days
   - Average time from "version bump commit on main" to "release
     assets visible in GitHub Releases UI"
   - Hand-maintained `CHANGELOG.md` line count: **789** as of
     2026-04-24.
4. Add a `Phase R0` subsection to §10 of this plan's progress
   dashboard (appended at the end of this file).
5. **Optional interim lockfile-drift patch** — apply only if R5
   is expected to be more than ~4 weeks out and at least one more
   release will ship through `just ship` before R5.  In that
   window, add a final `cargo generate-lockfile --offline` (or a
   plain `cargo check --workspace --locked`) step to the bumper so
   `Cargo.lock` tracks `Cargo.toml` on every bump.  See §2.2 for
   the bug description.  Skip this if R5 is landing soon — the
   one-line patch is throwaway work; R5's deletion subsumes it
   and release-plz refreshes the lockfile natively.  One-line
   change inside `build/update_all_versions.rs` (appended after
   the Cargo.toml writes, before the script exits):
   ```rust
   // Ensure Cargo.lock tracks the new workspace version.  Without
   // this, internal-crate `[[package]]` entries drift until some
   // later `cargo` invocation self-heals.  See
   // docs/architecture/release-automation-plan.md §2.2.
   Command::new("cargo")
       .args(["generate-lockfile", "--offline"])
       .status()
       .context("Failed to refresh Cargo.lock")?;
   ```

**Validation**:

- `cargo check --workspace --all-features --locked` still passes
  (the deleted TOML blocks are pure metadata, not consumed by
  cargo itself).
- `cargo publish --dry-run` run locally for any single crate to
  confirm the `publish` default is still `true` at the Cargo level
  (demonstrating that R0 reduced but did not eliminate the risk;
  R6 will finish the job by adding explicit `publish = false`).
- If the interim lockfile patch (step 5) was applied: run
  `just ship` dry-run locally and verify `git diff Cargo.lock`
  shows the internal-crate version bump alongside `Cargo.toml`.
  Skip this validation step if step 5 was skipped.

**PR shape** (as executed on `chore/release-auto-r0`, 2026-04-25):

- `Cargo.toml`: ~30 lines deleted (the two metadata blocks + section header).
- `build/update_all_versions.rs`: **1073 lines newly tracked** + ~52 lines of in-file additions (lockfile patch).  R0 discovered the script was gitignored despite being invoked by 4 callsites — the .gitignore carve-out below brings it under version control.  Both the script and the carve-out are scheduled for deletion in R5.
- `.gitignore`: replace blanket `build/` with `build/*` + `!build/update_all_versions.rs` + 7-line block comment explaining the R5 sunset.
- `crates/uffs-mft/Cargo.toml.bak`: 123 lines deleted (drive-by — stale v0.4.106 auto-commit artifact, never used by any current code path).
- `docs/architecture/release-automation-baseline.md`: ~150 lines, new file.
- `docs/architecture/release-automation-plan.md`: dashboard row R0 → 🟡 in-progress; PR-shape addendum (this section).

Net: ~1280 LOC tracked into git (mostly the previously-gitignored script), ~155 LOC deleted.

The PR is larger than the plan's original ~85-LOC estimate because the `.gitignore` discovery was unforeseen.  None of the additional scope is gold-plating: every change is required either by Decision 1 (lockfile patch needs the script tracked to be visible) or by structural correctness (a script invoked from 4 callsites should not be untracked).

**Rollback**: `git revert` the merge commit.  No runtime behavior
changes.

### Phase R1 — Commit convention formalization (advisory)

**Scope**: document the project's already-practiced conventional
commits discipline in `CONTRIBUTING.md`, and add an **advisory**
(non-blocking) PR check that comments when commit titles don't
match the convention.

**Rationale**: UFFS is already at ~100% adherence by habit (see
§2.8), but new contributors will write `Update README` or
`fix typo` etc. if we don't tell them the rule.  Release-plz's
version inference fails silently on non-conforming commits
(treats them as "no release" — they simply don't appear in the
changelog).  Better to fail loudly at PR time.

**Steps**:

1. Add a "Commit message convention" section to `CONTRIBUTING.md`:
   - Cite [Conventional Commits 1.0.0](https://www.conventionalcommits.org/)
   - List the **allowed types** (chosen to match what UFFS already uses):
     - `feat`: user-facing new feature → minor bump
     - `fix`: user-facing bug fix → patch bump
     - `perf`: performance improvement → patch bump
     - `refactor`: code change without behavior change → no bump
     - `docs`: documentation only → no bump
     - `test`: test-only change → no bump
     - `build`: build-system change (`Cargo.toml`, `Cargo.lock`, `rust-toolchain`) → no bump
     - `ci`: CI/workflows change → no bump
     - `chore`: everything else → no bump
   - Explain **breaking changes**:
     - `feat!: redesign IPC wire format` or
     - a `BREAKING CHANGE: <description>` footer
     - → major bump
   - Give UFFS-specific examples drawn from recent real merges
     (see §2.8 list).
   - Explain that **only `feat`, `fix`, `perf` produce releases**;
     all other types accumulate between releases but don't by
     themselves trigger a release.
2. Add `.github/workflows/commitlint.yml`:
   - Runs on `pull_request: [opened, synchronize, edited]`
   - Checks **only** the PR **title** (the squash-merge subject
     line) since UFFS uses squash-merge exclusively, not individual
     commit messages
   - Uses a self-contained regex check (no external action needed —
     the regex is ~200 bytes):
     ```yaml
     - name: Check PR title
       run: |
         TITLE="${{ github.event.pull_request.title }}"
         if ! echo "$TITLE" | grep -qE '^(feat|fix|perf|refactor|docs|test|build|ci|chore)(\([a-z0-9-]+\))?!?: .{1,}$'; then
           gh pr comment "${{ github.event.pull_request.number }}" \
             --body "⚠️ PR title does not match the Conventional Commits convention.  Expected: \`type(scope): subject\`.  See [CONTRIBUTING.md](CONTRIBUTING.md#commit-message-conventions).  **This check is advisory — it will NOT block merge during Phase R1.**"
           exit 0  # advisory: don't fail
         fi
     ```
   - Logs `✓` or `⚠ non-conforming, advisory only` to the Actions
     summary.  Exits 0 either way.
3. Do **not** add commit-msg hook to `scripts/hooks/` yet — the PR
   title is what gets squashed into `main`, not individual commit
   messages, so local enforcement would be noise.

**Validation**:

- Open a test PR with title `Update CONTRIBUTING.md`.  Commitlint
  posts the advisory comment; check does not block merge.
- Open a test PR with title `docs: update CONTRIBUTING.md`.
  Commitlint logs ✓ silently; no comment.
- Merge both PRs as squashes; observe that the `main` log contains
  properly-formatted subject lines for the conformant one and a
  raw-subject line for the non-conformant one (the latter is what
  we're preventing).

**PR shape**: 2 files changed (`CONTRIBUTING.md`, new
`.github/workflows/commitlint.yml`), ~100 lines added.

**Rollback**: `git revert`.  The advisory check simply stops
running.

### Phase R2 — git-cliff standalone adoption

**Scope**: introduce `git-cliff` with a `cliff.toml` template,
generate a fresh `CHANGELOG.md` from commit history, compare against
the hand-maintained one, and commit the result if quality is
acceptable.

**Rationale**: we want the commit → changelog pipeline working and
proven **before** hooking it into release-plz.  If the generated
changelog is worse than the hand-maintained one, we catch that here
in isolation and iterate on `cliff.toml` until it's right.
Release-plz delegates to `git-cliff` natively when `cliff.toml` is
present, so getting it right here means release-plz inherits it.

**Steps**:

1. Add `cliff.toml` at workspace root with a template that produces
   Keep-a-Changelog-compatible output matching the existing
   `CHANGELOG.md` style.  Template outline:
   - Header: unchanged from current
   - Version section format: `## [X.Y.Z] - YYYY-MM-DD`
   - Subsections by commit type:
     - `feat:` → `### Added` (for new features) + `### Changed` (for behavior changes)
     - `fix:` → `### Fixed`
     - `perf:` → `### Performance`
     - `BREAKING CHANGE:` → `### Breaking Changes`
     - `docs:`, `test:`, `build:`, `ci:`, `chore:`, `refactor:` → suppressed (not shown in changelog)
   - Commit formatting: `- **[scope]** subject (#PR)` where available
   - Unreleased section preserved for WIP
2. Install `git-cliff` locally for the initial generation:
   - `cargo install git-cliff --locked`
3. Generate the changelog to a scratch file first:
   - `git cliff --config cliff.toml --unreleased -o /tmp/generated-CHANGELOG.md`
   - Manually diff against current `CHANGELOG.md`
   - Iterate on `cliff.toml` until the output captures all
     release-worthy changes at acceptable prose quality.
4. **Do not overwrite `CHANGELOG.md` yet** — that happens in
   Phase R3/R4 when release-plz takes over changelog updates.
   The goal of R2 is just to prove the template works.
5. Document the verification in a short
   `docs/architecture/release-automation-baseline.md` followup.

**Validation**:

- `git cliff --config cliff.toml --unreleased` runs cleanly with no
  errors.
- The generated output contains entries for every merge since the
  last `v0.5.71` equivalent (which doesn't exist yet — so: since
  the last `chore: release` commit, or since the initial commit,
  whichever is shorter).
- Categorization looks right: `feat:` → Added, `fix:` → Fixed, etc.

**PR shape**: 1 file added (`cliff.toml`), ~80 lines.  1 file
touched (`docs/architecture/release-automation-baseline.md` addendum),
~30 lines.  `CHANGELOG.md` **not touched**.

**Rollback**: `git revert`.  git-cliff installation is per-developer
and orthogonal.

### Phase R3 — release-plz in shadow (comment-only) mode

**Scope**: install release-plz as a GitHub Action that runs on
every push to `main` and **only posts a comment** saying what it
WOULD do, without opening PRs or creating tags.  Observe for 1-2
weeks / 3-5 merges.

**Rationale**: release-plz's behavior depends on cliff.toml, the
commit history, the workspace structure, and the `release-plz.toml`
config.  Getting any of those wrong produces surprising output.
Shadow mode lets us see the output on **real commits** without any
blast radius.

**Steps**:

1. Add `release-plz.toml` at workspace root.  Minimum required
   content:
   ```toml
   [workspace]
   # Single version for the whole workspace (matches current layout).
   # Can be flipped to per-crate later (Phase R9).
   dependencies_update = true

   # CRITICAL: dormant publishing.  Explicit and audited.
   publish = false

   # Git release created by release-plz; actual binary upload stays
   # in release.yml (triggered by the tag push).
   git_release_enable = true
   git_tag_enable = true

   # Changelog generation via git-cliff (cliff.toml at workspace root).
   changelog_update = true

   # Shadow mode: do not open PRs, do not create tags, do not release.
   # This is a release-plz-specific flag set via the GitHub Action
   # workflow below (release_always: false + dry_run semantics).
   ```
2. Add `.github/workflows/release-plz.yml`:
   ```yaml
   name: 🔮 Release-plz (shadow)
   on:
     push:
       branches: [main]
   permissions:
     contents: read
     pull-requests: read
     issues: read
   jobs:
     dry-run:
       name: Dry-run release plan
       runs-on: ubuntu-latest
       timeout-minutes: 10
       steps:
         - uses: actions/checkout@<pinned-sha>
           with:
             fetch-depth: 0
         - uses: dtolnay/rust-toolchain@<pinned-sha>
           with:
             toolchain: stable
         - name: release-plz plan
           uses: release-plz/action@<pinned-sha>
           with:
             command: release-pr  # subcommand
             config: release-plz.toml
             # dry-run via NOT providing token write permissions above;
             # the action will fail to open a PR but succeed at computing
             # the plan, which it emits to the workflow summary.
   ```
   Note: release-plz's actual dry-run story evolved across versions;
   if the above approach doesn't produce the desired output, pivot
   to calling `release-plz release-pr --dry-run` via a raw step
   (the CLI supports `--dry-run` since 0.3.x).
3. Run on `main` naturally via subsequent merges — no synthetic
   test PRs.
4. After each run, record in
   `docs/architecture/release-automation-baseline.md`:
   - What release-plz proposed (version bump + changelog diff)
   - What you would have proposed manually
   - Discrepancies and their causes (e.g. "commit was `fix:` but
     was actually a minor feature — upgrade to `feat:` in future")

**Exit criteria** (all must hold before advancing to R4):

- ≥3 consecutive release-plz runs where the proposed version bump
  matches the manual judgement.
- ≥1 run that includes a `feat:` commit, producing a minor bump
  (validates the MINOR path, which is the commonest real-world
  release driver).
- ≥1 run that includes only `chore:` / `docs:` / `test:` commits,
  producing **no release** (validates the suppression path).
- The generated changelog entries are human-acceptable without
  heavy editing — one-sentence edits are fine, multi-paragraph
  rewrites are a signal that `cliff.toml` needs tuning.

**PR shape**: 2 files added (`release-plz.toml`, new workflow),
~80 lines total.

**Rollback**: `git revert`.  The shadow workflow just stops
running.

### Phase R4 — release-plz active (release PR mode)

**Scope**: flip release-plz from shadow to active.  It now opens
release PRs on `main` pushes.  Humans review and merge.  Merging the
release PR causes release-plz (on its next `main` run) to create the
tag, which triggers `release.yml`.

**Keep `auto-tag-release.yml` running in parallel during R4.**  Belt
and suspenders: if release-plz's tag creation fails for any reason,
`auto-tag-release.yml` still catches the Cargo.toml version change
and dispatches `release.yml`.  The idempotency guard in
`release.yml` (tag-exists check) prevents double-fire.

**Settled-pre-execution decisions** (recorded 2026-05-07 before R4
opened — mirrors the §8 settled-decisions block for R0):

1. **Workspace-style tags** — single `v{{ version }}` tag per
   release.  Override of release-plz's default
   `{{ package }}-v{{ version }}` per-crate scheme.  Honors UFFS as
   one product (12 publishable crates moving in lockstep, sharing
   `[workspace.package].version`) and keeps the existing
   `release.yml` `on: push: tags: [v*]` trigger working with zero
   migration.  Same shape as `cargo` and `rustls` workspaces.

2. **Workspace-style CHANGELOG** — all 12 publishable crates point
   at the workspace-root `CHANGELOG.md` via per-package
   `changelog_path` overrides in `release-plz.toml`.  Per-crate
   crates.io detail pages link back to this single file.  Diverges
   from `tokio` (per-crate CHANGELOGs) because UFFS releases lockstep,
   tokio doesn't.

3. **`git_only = true` baseline** — UFFS is unpublished through R8,
   so the crates.io registry has no version data to diff against.
   release-plz uses git tags (the existing `v0.5.x` series) as the
   baseline instead.  Flips back in R8 once ≥1 crate is published.

4. **`release_commits` filter** — only `feat:`, `fix:`, `perf:`,
   `security:` commit subjects trigger a release PR.  `chore`,
   `ci`, `build`, `docs`, `refactor`, `test`, `style`, `revert` are
   silently ignored even when they land on `main`.  Mirrors the
   suppression list in `cliff.toml`'s `commit_parsers`.  Without
   this, every push to `main` (including infra-only commits) would
   re-open the release PR with a no-op preview, producing churn.

5. **Two-job workflow structure** — `release-plz/action` does NOT
   have a single "do both" command.  Per release-plz's own recommended
   workflow shape, R4 ships with two parallel jobs in
   `.github/workflows/release-plz.yml`: `release-plz-pr` (runs
   `command: release-pr` on every push) and `release-plz-release`
   (runs `command: release` on every push but no-ops unless HEAD is
   the merge of the release PR).  Mirrors
   <https://github.com/release-plz/release-plz/blob/main/.github/workflows/release-plz.yml>.

6. **Default `GITHUB_TOKEN` for R4 — NOT a GitHub App** — the
   first-cut R4 PR uses the workflow-provided `GITHUB_TOKEN`, NOT
   a GitHub App or PAT.  Trade-off:
     - **Pro**: zero new infra (no app creation, no secret rotation).
     - **Con**: tags created by release-plz via `GITHUB_TOKEN` do
       NOT trigger downstream workflows (per GitHub's anti-loop
       policy).  `release.yml` won't auto-fire after release-plz
       creates the tag.
   For the first release after R4 lands, the maintainer manually
   pushes the tag (or the version-bump commit) — that's a user-
   driven push which triggers `release.yml` normally.  A follow-up
   PR (tracked as informal "R4.5") sets up Option A (GitHub App)
   from the §8.1 deviations log, after R4 itself bakes in.

7. **First-release bootstrap is OUT OF SCOPE for the R4 PR** —
   the v0.5.90 git tag's worktree predates the R3.5 fix (it lacks
   `version =` on internal deps), so release-plz's `git_only`
   baseline check fails when comparing HEAD against v0.5.90.  This
   surfaces as "no version bump proposed" in the first few R4 runs.
   The maintainer manually cuts v0.5.91 from current `main` (which
   has the R3.5 fix) — bumping `[workspace.package].version` and
   writing the CHANGELOG entry by hand — to bootstrap a working
   baseline.  Subsequent releases proceed automatically.

**Steps**:

1. Update `release-plz.toml` with the four R4 workspace-level
   settings — `git_only = true`, `git_tag_name = "v{{ version }}"`,
   `git_release_name = "v{{ version }}"`,
   `release_commits = "^(feat|fix|perf|security)(\\(.+\\))?:"`.
2. Add 12 per-package `[[package]]` blocks (one per publishable
   crate) with `changelog_path = "CHANGELOG.md"` to flatten the
   per-crate changelog into the workspace-root file.  These are
   in addition to the existing 3 R6 `release = false` blocks for
   internal CI tools.
3. Replace the R3 shadow-mode workflow body in
   `.github/workflows/release-plz.yml` with the two-job active
   structure (decision 5 above).  Use
   `release-plz/action@<pinned-sha>` with `command: release-pr`
   and `command: release` respectively.
4. **Do NOT delete `auto-tag-release.yml` yet** — that's Phase R5.
5. Land R4 PR.  Observe behaviour for `≥` 1 push to `main` after
   merge:
   - The `release-plz-pr` job runs and either opens a release PR
     (if any qualifying commits since the latest tag), or no-ops
     (if not).
   - The `release-plz-release` job runs and silently no-ops
     because HEAD is not yet a release-PR merge.
6. Maintainer (out-of-band): bootstrap the first release per
   decision 7.  Concretely:
   - Bump `[workspace.package].version` 0.5.90 → 0.5.91 in `Cargo.toml`.
   - Add the `## [0.5.91] - <date>` block to `CHANGELOG.md` by hand.
   - Open + merge the bootstrap PR (or commit directly to a feature
     branch + PR through normal review).
   - Tag the resulting commit `v0.5.91` (push triggers
     `release.yml` because it's a user push, not GITHUB_TOKEN).
7. After bootstrap: future `feat:`/`fix:` merges into `main`
   trigger the `release-plz-pr` job, which opens the release PR
   automatically.  Maintainer reviews + merges → `release-plz-release`
   creates the v0.5.92 tag → ... but NOTE the GITHUB_TOKEN limitation
   in decision 6: the tag won't fire `release.yml` automatically
   until the R4.5 follow-up PR sets up a GitHub App.
8. Write a deviations-log entry in this plan's §8.1 if the
   GITHUB_TOKEN limitation manifests differently than expected, or
   if any of decisions 1-7 turn out wrong in practice.

**Exit criteria** (all must hold before advancing to R5):

- ≥1 complete release via release-plz flow (release PR opened →
  reviewed → merged → tag created → GitHub Release page exists).
  The first release MAY be the bootstrapped v0.5.91 done by hand
  per decision 7 — that still counts because the workflow's
  `release-plz-pr` and `release-plz-release` jobs are observed
  to run cleanly even if they no-op for that specific cut.
- Both release-plz's tag step and `auto-tag-release.yml` fire on
  the release PR merge, and the tag-exists guard correctly
  deduplicates.
- Generated `CHANGELOG.md` is in the `main` history, replacing the
  hand-maintained `## [Unreleased]` workflow.  (For the bootstrap
  release this means the bootstrap PR's hand-written entry is
  followed by automated entries on subsequent releases.)

**PR shape (this PR)**: 4 files modified
(`release-plz.toml` ~120 LOC added,
`.github/workflows/release-plz.yml` rewritten,
`docs/architecture/release-automation-plan.md` §R4 updated,
`docs/architecture/release-automation-baseline.md` §11 R4 addendum
appended), ~400-line net diff.

**Rollback**: `git revert` flips the workflow back to shadow mode.
Any already-created tag from a shadow run stays (harmless; tags
are idempotent).  The `release-plz.toml` R4 additions can be
reverted independently — they're additive (no R3 settings removed).

### Phase R5 — Retire bespoke tooling

**Scope**: delete all the code and workflows that release-plz now
replaces.

**Prerequisites**:

- ≥2 full releases successfully cut via release-plz (Phase R4).
- `auto-tag-release.yml` has been observed to no-op after release-plz
  creates the tag (confirming release-plz wins the race reliably and
  the tag-exists guard is the correct invariant).

**Steps**:

1. Delete `build/update_all_versions.rs` (1073+ lines, version-tracked
   in R0).  Also remove the `.gitignore` carve-out (`build/*` +
   `!build/update_all_versions.rs` block) added in R0; restore the
   blanket `build/` ignore.  After deletion, `build/` returns to its
   pre-R0 state: a fully gitignored directory of generated artifacts.
2. Delete the thin wrapper `scripts/ci/ci-pipeline.rs` (49 lines)
   — dev-flow Phase 7 kept it as a deprecation shim with header
   marker `REMOVE-AFTER: v0.5.73`.  R5 retires it alongside the
   bespoke bumper; coordinating here avoids a follow-up cleanup PR.
   If the current workspace version is still ≤ `0.5.73`, note in
   the PR body that the REMOVE-AFTER marker was met by R5's
   landing rather than by a version threshold.
3. Delete version-bump functions from
   `scripts/ci-pipeline/src/version.rs`:
   - `increment_version()` — deleted
   - `version_bump()` — deleted
   - Keep `get_current_version()` and
     `extract_version_from_cargo_toml()` — they're still useful
     for `just ship`'s push step (constructing release branch
     names).
   - Keep `update_polars_git()` — unrelated to versioning,
     updates the polars git dep pin.
4. Remove the version-bump step from the `ship` pipeline in
   `scripts/ci-pipeline/src/`.  `just ship` is now **only**:
   check → lint → test → push.  Version bumping happens via
   release-plz on `main`, not via local ship commands.
5. Delete `.github/workflows/auto-tag-release.yml` (169 lines).
6. Update `dev-flow-implementation-plan.md`:
   - Remove references to the `auto-tag-release.yml` bridge
     workflow.
   - Flip the final `[ ]` bake-in item in Phase 7 dashboard
     (§10.3) to `[x]` — the R4 release that preceded this PR
     satisfied it.  Cross-reference this PR's SHA in the tick.
   - Add cross-reference to
     `release-automation-plan.md` in §1 or §2 intro.
7. Update `CONTRIBUTING.md`:
   - Remove any mention of `./build/update_all_versions.rs`.
   - Explain the new "just write conventional commits, releases
     are automatic" flow.
8. Update `justfile`:
   - Remove the version-bump step from `just ship` (if present as a
     distinct target).
   - Keep any `just release` target that dispatches `release.yml`
     manually, if such a target exists — it's a useful escape
     hatch.

**Validation**:

- `just ship` runs cleanly end-to-end on a feature branch without
  invoking any deleted script.
- No workflow references `./build/update_all_versions.rs`.
- `grep -r 'update_all_versions' .github/ scripts/ justfile` returns
  zero matches.
- `grep -r 'auto-tag-release' .github/ docs/ scripts/` returns zero
  matches except historical entries in CHANGELOG.md and the
  dev-flow plan's deviations log (retained as history).
- Next release after Phase R5 lands cleanly — the release PR
  opens, merges, tag creates, `release.yml` fires.

**PR shape**: 4-5 files deleted (bespoke bumper + thin wrapper +
auto-tag workflow + CI-pipeline version functions), 3-4 files
modified (dev-flow plan Phase 7 bake-in tick + CONTRIBUTING +
justfile + version.rs trim).  Net diff: **~1400 lines removed**,
~30 lines added.

**Rollback**: `git revert` restores everything.  This is where the
"reversibility" discipline earns its keep — reverting Phase R5 is
the only way back to the bespoke-tooling world, and it works
because nothing else was restructured simultaneously.

### Phase R6 — Crates.io metadata audit + dry-run publish CI

**Scope**: audit every `crates/*/Cargo.toml` for crates.io readiness
and add a scheduled GitHub Action that runs `cargo publish --dry-run
--workspace` to catch metadata drift early.  **No actual publishing
happens; `publish = false` remains set workspace-wide.**

**Rationale**: `cargo publish` has ~30 distinct failure modes
(missing `description`, `license` mismatch, `readme` not found,
path dependencies without `version =`, feature-flag typos, lockfile
divergence, crate-size > 10 MB, disallowed registry, missing
`repository` link, docs.rs build failures, ...).  You don't want
to discover any of them during a real go-live release.  A nightly
dry-run on the full workspace surfaces every failure class against
the current `main` tip, so by the time Phase R9 flips publishing on,
the `cargo publish` step is boring.

This phase also sets up the **dependency ordering** discipline — the
only publish order that works is a topological sort of the workspace
internal-dependency DAG, and that order must be encoded in whatever
invokes `cargo publish`.  Release-plz handles this automatically
(it walks the DAG), but the dry-run workflow still needs to agree
with release-plz's walk, or debugging becomes an archaeological
expedition.

**Steps**:

1. **Audit each publishable crate** using the matrix in §5.1.  For
   each crate, confirm:
   - `name` is unique on crates.io (run
     `cargo search <crate-name> --limit 1` for each; reserve names
     NOW before someone else takes them — see §5.7).
   - `description` exists and is ≤ 280 chars, ≥ 30 chars,
     descriptive (not `"TODO"`, not the crate name echoed).
   - `license` or `license-file` resolves.  The workspace sets
     `license = "MPL-2.0"`; each crate inherits.  `MPL-2.0` is
     an SPDX identifier crates.io accepts natively.
   - `repository` URL is reachable (workspace-inherited).
   - `homepage` set if a product landing page exists; otherwise
     omit (do NOT repeat `repository`).
   - `documentation` field — either omit (crates.io defaults to
     `docs.rs/<crate-name>`) or set to a specific docs.rs URL per
     crate.  **Recommend omitting** — simpler and always correct.
   - `readme` field: each crate needs a `README.md` adjacent to
     its `Cargo.toml` (or it inherits the workspace one, which is
     generic and undesirable per-crate).  File an issue for each
     missing per-crate README; don't block the phase on it but
     track it.
   - `keywords`: max 5, lowercase, hyphenated.  Workspace-inherited
     value is reasonable starting point; individual crates may
     override (e.g. `uffs-polars` might prefer `["polars", "facade",
     "isolation"]`).
   - `categories`: must match one of the ~50 crates.io
     categories exactly.  See https://crates.io/category_slugs.
     Workspace default `["filesystem", "command-line-utilities"]`
     is correct for `uffs-cli`; library crates like `uffs-core`
     should use `["filesystem", "data-structures"]` or similar.
     Audit per crate.
   - `rust-version` is honest — matches what CI actually tests.
     Currently set to `1.91` workspace-wide.  Honest.
2. **Explicitly set `publish = false`** at the workspace level in
   `release-plz.toml` (already done in R3) **and** per-crate in
   the `[package]` section of any crate that is a private
   dev tool — specifically:
   - `scripts/ci-pipeline` → `publish = false` (internal tool;
     never goes on crates.io)
   - `crates/uffs-diag` → `publish = false` (diagnostic tools;
     not user-facing)
   - `crates/uffs-broker` → **TBD** — it's the Windows elevated
     handle broker, useful standalone?  Decide in Phase R8 dress
     rehearsal.
   This "publish = false at the crate level" is different from
   "publish = false in release-plz.toml" — the former blocks
   `cargo publish` locally too, the latter blocks just the
   release-plz workflow.  Belt and suspenders.
3. **Add docs.rs metadata** to each publishable crate's `Cargo.toml`:
   ```toml
   [package.metadata.docs.rs]
   # Which feature flags to build with on docs.rs
   all-features = true
   # Target-specific build (docs.rs builds on Linux by default;
   # this forces a specific target so platform-gated items render)
   targets = ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
   # Show cfg(docsrs) content (the `#[cfg_attr(docsrs, doc(cfg(...)))]`
   # pattern that makes platform gating visible in docs)
   rustdoc-args = ["--cfg", "docsrs"]
   ```
   For `uffs-mft`, `uffs-security`, `uffs-broker` (Windows-gated),
   the `targets` list must include `x86_64-pc-windows-msvc` or
   the Windows API surface disappears from docs.
4. **Add `.github/workflows/crates-io-dry-run.yml`**:
   ```yaml
   name: 📦 crates.io dry-run
   on:
     schedule:
       - cron: '0 6 * * 1'   # Mondays 06:00 UTC, weekly
     workflow_dispatch:
   permissions:
     contents: read
   jobs:
     dry-run:
       name: cargo publish --dry-run (workspace)
       runs-on: ubuntu-latest
       timeout-minutes: 30
       steps:
         - uses: actions/checkout@<pinned-sha>
         - uses: dtolnay/rust-toolchain@<pinned-sha>
           with:
             toolchain: stable
         - name: Dry-run publish every workspace member
           run: |
             # cargo publish refuses path-deps without version =
             # (we already set version = in workspace.dependencies).
             # --no-verify skips the expensive full build; we just
             # want metadata + tarball assembly to succeed.
             # For a richer signal, run without --no-verify but
             # be prepared for ~20 min runtime.
             for crate in $(cargo metadata --no-deps --format-version 1 \
                 | jq -r '.packages[] | select(.publish != []) | .name'); do
               echo "::group::dry-run $crate"
               cargo publish --dry-run -p "$crate"
               echo "::endgroup::"
             done
   ```
   Notes on the `jq` filter: `.publish != []` picks crates whose
   `publish` field is either absent (default: publishable) or a
   registry list.  Crates with `publish = false` serialize to
   `.publish == []` in cargo metadata and get filtered out.
5. **Document first-publish runbook** at `docs/publishing.md` —
   template in §5.7.  Marked clearly as "DORMANT — do not execute
   without explicit release decision recorded in
   release-automation-plan.md §9 status dashboard".
6. **Reserve crate names on crates.io** (critical):
   - Publish **empty stub 0.0.0 versions** of each crate name you
     intend to own: `uffs`, `uffs-core`, `uffs-mft`, `uffs-polars`,
     `uffs-security`, `uffs-text`, `uffs-time`, `uffs-format`,
     `uffs-client`, `uffs-daemon`, `uffs-cli`, `uffs-mcp`,
     `uffs-broker`.
   - The stub is a single-file crate with `description = "Name
     reservation for UFFS — see https://github.com/skyllc-ai/
     UltraFastFileSearch"` and no code.
   - **This is the one exception to "no actual publishing in
     this plan"** — name squatting is a real risk and
     reservations are irreversible (crates.io doesn't allow
     name transfers except for abuse).
   - Do the name reservations from a **throwaway dedicated
     workspace** (not from the UFFS repo), so the actual project
     still has `publish = false` everywhere.  The reserved 0.0.0
     can be yanked later without side effects; the name stays
     owned.
   - Track reserved names in §5.1 audit matrix.

**Exit criteria** (all must hold before advancing to R7):

- Every publishable crate has complete, correct metadata per §5.1
  matrix.
- `crates-io-dry-run.yml` runs green for 2 consecutive weeks
  (so we observe at least one post-release `main`).
- All 13 intended crate names are reserved on crates.io under
  the project owner's account.
- `docs/publishing.md` exists and is reviewed by at least one
  maintainer for accuracy.
- `[package.metadata.docs.rs]` present in every publishable
  crate.

**PR shape**: ~13 files modified (per-crate Cargo.toml + metadata
additions), 2 files added (workflow + docs/publishing.md),
1 companion PR for name-reservation stubs (from the throwaway
workspace, **not** in the UFFS repo).  Net diff in UFFS repo:
~400 lines added.

**Rollback**: `git revert` reverses metadata and deletes the
workflow.  The crates.io name reservations stay (they're
external state); the stubs can be `cargo yank`ed if the project
is abandoned.

### Phase R7 — Trusted publisher (OIDC) scaffolding

**Scope**: configure GitHub's OIDC federation with crates.io's
trusted-publisher feature.  This replaces the legacy pattern of
"long-lived crates.io API token stored as a GitHub secret" with
short-lived, audience-scoped OIDC tokens minted per workflow run.

**Rationale**: crates.io trusted publishing shipped in mid-2024 and
is now the recommended posture for CI publishing.  Benefits:
- No long-lived secrets in GitHub repo settings (one less
  rotation burden, one less exfiltration target)
- Token audience restricted to a specific workflow + branch +
  environment (narrow blast radius if compromised)
- Server-side auditability: crates.io logs which GitHub Actions
  run minted each publish
- No human-in-the-loop credential handling during onboarding

**Prerequisites**: Phase R6 complete (names reserved, dry-run
green).

**Steps**:

1. **Create a GitHub Environment** in the repo settings:
   - Name: `crates-io-production`
   - Protection rule: required reviewers = 1 (maintainer list)
   - Deployment branch rule: only `main` branch
   - Environment secrets: **none** (trusted publishing doesn't
     need a secret; the OIDC token is minted at runtime)
2. **Register the trusted publisher on crates.io** for each
   reserved crate:
   - Log in to crates.io as the project owner.
   - For each of the 13 reserved crate names, go to
     Crate Settings → Trusted Publishers → Add.
   - Form fields:
     - Repository owner: `skyllc-ai`
     - Repository name: `UltraFastFileSearch`
     - Workflow filename: `release-plz.yml` (relative to
       `.github/workflows/`)
     - Environment: `crates-io-production`
   - This binds the crate name to the exact GHA workflow that
     is authorized to publish it.
3. **Add the publish-eligible job to `release-plz.yml`** (but
   leave it gated off via `if: false`):
   ```yaml
   publish:
     name: Publish to crates.io (gated)
     needs: release
     if: false   # Gate flipped in Phase R9
     runs-on: ubuntu-latest
     environment: crates-io-production
     permissions:
       contents: read
       id-token: write   # MANDATORY for OIDC
     steps:
       - uses: actions/checkout@<pinned-sha>
       - uses: rust-lang/crates-io-auth-action@<pinned-sha>
         id: auth
       - uses: dtolnay/rust-toolchain@<pinned-sha>
         with:
           toolchain: stable
       - name: cargo publish (workspace, OIDC)
         env:
           CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}
         run: |
           # release-plz will eventually drive this; the shell
           # loop is only for the pre-release-plz smoke test.
           for crate in $(cargo metadata --no-deps --format-version 1 \
               | jq -r '.packages[] | select(.publish != []) | .name'); do
             cargo publish -p "$crate" --locked
           done
   ```
   The `id-token: write` permission is what enables the workflow
   to request an OIDC token.  Without it, the OIDC mint fails with
   an opaque `"Unable to mint OIDC token"` error.
4. **Document the trusted-publisher setup** in `docs/publishing.md`
   with screenshots / exact form field values captured.  Rotation
   and revocation instructions live here too: if trust breaks or
   the repository is forked/renamed, the publisher registrations
   need updating on crates.io.
5. **Verify OIDC flow without publishing**: flip `if: false` to
   `if: github.event_name == 'workflow_dispatch'` temporarily.
   Trigger `workflow_dispatch` manually.  The auth step should
   succeed and print a redacted token.  Keep the `cargo publish`
   step dry-run only in this test (`--dry-run`).  Revert the `if`
   gate after observation.
6. **Do NOT flip `if: false` to `if: true` in this phase.**  That
   cutover is Phase R9.

**Exit criteria** (all must hold before advancing to R8):

- Trusted-publisher registration done for all 13 reserved crate
  names.
- `crates-io-production` GitHub Environment exists with correct
  protection rules.
- OIDC dry-run via manual `workflow_dispatch` succeeds end-to-end
  (token minted, cargo auth succeeds, `--dry-run` publish
  succeeds) and result is documented in `docs/publishing.md`.
- Regular `release-plz.yml` runs continue unaffected (the publish
  job is `needs: release` + `if: false`, so it never runs on
  scheduled or push-triggered invocations).

**PR shape**: 1 file modified (`release-plz.yml` — publish job
added), 1 file modified (`docs/publishing.md` — OIDC section),
0 files deleted.  Net diff: ~80 lines added.  External state:
13 crate registrations + 1 GitHub Environment (not in repo).

**Rollback**: `git revert` removes the workflow job.  Trusted-
publisher registrations on crates.io can be removed via crate
settings; the GitHub Environment can be deleted.  No lingering
tokens, no credential cleanup.

### Phase R8 — First publish dress rehearsal

**Scope**: execute **one** real publish against crates.io — for a
**single, benign, low-surface crate** — to validate the entire
chain end-to-end.  The publish target is intentionally the smallest,
most leaf-like crate: `uffs-time` (pure NTFS FILETIME arithmetic,
zero internal deps, no unsafe, no platform gating).

**Rationale**: the dry-run workflow (R6) catches metadata errors but
NOT:
- Interaction with crates.io's actual registry backend
- docs.rs rendering (only observable after a real publish)
- The OIDC auth handshake with a real token
- Network flakiness, registry throttling, 5xx errors
- Post-publish verification (crate page renders, readme
  displays, docs.rs build succeeds within its 2-hour window)

A single-crate rehearsal against a real publishable surface
shakes out everything the dry-run can't.  `uffs-time` is chosen
because:
- It has no internal dependencies → publish ordering irrelevant
- It's pure compute with zero unsafe → minimal security surface
- Its failure modes are purely "didn't render well on docs.rs",
  not "accidentally shipped CVE"
- If we decide to unpublish, yanking it affects nothing
  downstream (it has no published users on day 1 by construction)

**Prerequisites**:
- Phase R7 complete (OIDC verified end-to-end in dry-run).
- Workspace version has been bumped via release-plz at least
  once (so we're publishing `uffs-time = "0.5.72"` or similar,
  not `0.5.71`, which would collide with the reserved `0.0.0`
  stub).

**Steps**:

1. **Temporarily gate the publish job to `uffs-time` only**:
   ```yaml
   - name: cargo publish (uffs-time only — R8 rehearsal)
     env:
       CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}
     run: cargo publish -p uffs-time --locked
   ```
2. **Flip the `if: false` to `if: true`** on the publish job
   (keeping it `needs: release`, so it only runs after the
   release job succeeds, i.e. after a release tag is created).
3. **Trigger a release** through the normal release-plz flow:
   - Merge a `fix:` or `feat:` commit to `main`
   - Wait for release PR
   - Review, merge
   - Observe release-plz → tag → `release.yml` (binaries) AND
     the new publish job firing in parallel
4. **Post-publish verification checklist**:
   - `cargo search uffs-time --limit 1` shows the new version
   - `crates.io/crates/uffs-time` page renders correctly
   - `docs.rs/uffs-time` build starts within 5 min, succeeds
     within 2 hours
   - The published crate can be consumed from a throwaway
     project: `cargo add uffs-time` in an empty `cargo new`
     project, then `cargo build`.  This catches the "tarball
     was missing a file" class of bugs that dry-run can't
     detect.
5. **Restore the publish job** to its full-workspace form after
   rehearsal succeeds:
   ```yaml
   - name: cargo publish (workspace)
     env:
       CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}
     run: |
       for crate in $(cargo metadata --no-deps --format-version 1 \
           | jq -r '.packages[] | select(.publish != []) | .name'); do
         cargo publish -p "$crate" --locked
       done
   ```
   But **flip `if: true` back to `if: false`** for the next
   release cycle.  R8 is a one-off rehearsal; live publishing
   is R9.

**Exit criteria** (all must hold before advancing to R9):

- `uffs-time` v0.5.72+ appears on crates.io via the GHA publish
  path (not via manual `cargo publish`).
- The crate builds green on docs.rs.
- A throwaway downstream project can `cargo add uffs-time`
  and build.
- The publish job successfully authenticated via OIDC (the
  crates.io publish log, accessible via crate settings, shows
  the GitHub Actions run ID).
- No cleanup actions pending (no accidentally-published
  crates, no leaked tokens, no confused Dependabot).
- Deviations log entry written in dev-flow-implementation-plan.md
  §10.5 or release-automation-plan.md §9 for any surprises.

**PR shape**: 1 file modified (`release-plz.yml` — gate flip +
scope restrict + restore), 0 files deleted.  Net diff over the
phase lifetime: ~10 lines (flip, restore).  External state:
1 crate version published to crates.io (irreversible — can be
yanked but not deleted).

**Rollback**:
- Flip `if: true` back to `if: false` (future releases won't
  publish).
- `cargo yank -p uffs-time --vers <published-version>` (crates.io
  hides the version from new resolves; existing lockfiles still
  work).
- **Cannot unpublish** — crates.io's no-delete policy is
  absolute except for the 72-hour grace window on accidental
  pushes.  Within the grace window: `cargo owner --add` to give
  the crates.io support team the ability to delete on your
  behalf, then file a support ticket.
- Document the yank rationale in `docs/publishing.md` "yank
  decisions" log.

### Phase R9 — Live publishing (deferred cutover)

**Scope**: flip the publish job from `uffs-time`-only to
full-workspace, and from `if: false` to `if: true`, on a
maintainer's explicit go-ahead (recorded in the status dashboard,
§8).  This is the "we are a published project now" milestone.

**Explicit gate**: this phase is **not executed as part of the
initial automation rollout**.  It is a separate decision gated
on:

1. API stability: the project has reached at least v0.6.0 or
   v1.0.0, with a stated SemVer stability contract.
2. Library API review: `cargo public-api` or
   `cargo-semver-checks` integrated into PR Fast CI for at
   least one release cycle.
3. Documentation: every publishable crate has a usable
   `README.md` adjacent to its `Cargo.toml` (not the
   workspace-inherited one) and at least one working
   `cargo add <crate> && use <crate>::...` example.
4. Downstream readiness: a maintainer has written a short
   "who is this crate for" blurb on each crate page.  Drive-by
   adoption is a commitment; you don't get to unpublish when
   it's inconvenient.
5. Explicit maintainer approval recorded in this plan's §8
   dashboard row R9, with date + maintainer handle + PR link.

**Steps** (to be executed at go-live, not now):

1. Flip the publish job's `if:` guard to `if: true`.
2. Restore full-workspace scope (from R8 restore step).
3. Add `[workspace.metadata.release-plz] publish = true` in
   `release-plz.toml` (or per-crate via the release-plz config),
   triggering release-plz's own publish step in addition to the
   OIDC workflow job.  **Do not dual-path** — pick one:
   - Option A: release-plz drives publishing (simpler, fewer
     moving parts)
   - Option B: dedicated `publish` workflow job drives
     publishing, release-plz only does tag + GH Release
   Recommend Option A for simplicity.  The workflow job from
   R7/R8 then becomes a dry-run integration test, not the
   publishing path.
4. Lock the publishing workflow behind the `crates-io-production`
   GitHub Environment with required reviewers.  Every published
   version needs a human click.  The review should be "yes, the
   release notes are accurate and the version bump is
   appropriate", not "yes, this compiles" (CI already said it
   compiles).
5. Update `docs/publishing.md` from "DORMANT" to "LIVE" with
   the cutover date.
6. Update `release-automation-plan.md §8 dashboard` R9 row with
   the cutover commit SHA, PR link, and first-live-published
   crate version.

**Exit criteria**: N/A — this is the terminal phase for the
release automation plan.  Post-R9, the project is in a steady
state and further evolution moves to operational docs rather
than a migration plan.

**PR shape**: ~3 lines modified in `release-plz.yml`, ~2 lines
in `release-plz.toml`, ~20 lines in `docs/publishing.md`.

**Rollback**: the nuclear option is flipping `publish` back to
`false` everywhere and `cargo yank`ing whatever was published
in the 24-48 hours before rollback.  Partial rollback (e.g.
"publish these 3 crates but not those 10") is supported by
setting `publish = false` per-crate in individual `Cargo.toml`
files and tagging another release — release-plz respects the
per-crate flag.

## 5. Crates.io publishing scaffolding — deep-dive

The phases R6-R9 above are the **execution steps**.  This section
is the **reference** for the artifacts those phases produce.  Read
this when you're:
- Auditing a crate's metadata in R6 (§5.1)
- Debugging publish ordering (§5.2)
- Rendering docs.rs for the first time (§5.4)
- Writing the runbook (§5.7)

### 5.1 Per-crate metadata audit matrix

Target state table.  Fill in the `Action` column during R6.

| Crate | Publishable? | `publish` | `description` present | Per-crate `README.md`? | `keywords` override? | `categories` override? | `docs.rs` features | `[[bin]]`? | Action |
|---|---|---|---|---|---|---|---|---|---|
| `uffs-polars` | Yes (lib) | inherit (publishable) | Yes, tune | Missing — write | `["polars", "facade", "isolation"]` | `["rust-patterns", "caching"]` | `all-features` | No | write README, tune description |
| `uffs-security` | Yes (lib) | inherit | Yes, tune | Missing — write | `["security", "filesystem", "crypto"]` | `["cryptography", "filesystem", "os::windows-apis"]` | `all-features` + Windows target | No | write README, tune description, add docs.rs Windows target |
| `uffs-text` | Yes (lib) | inherit | Yes, tune | Missing — write | `["unicode", "text", "i18n"]` | `["text-processing", "internationalization"]` | `all-features` | No | write README |
| `uffs-time` | Yes (lib) | inherit | Yes, tune | Missing — write | `["ntfs", "filetime", "windows", "time"]` | `["date-and-time", "os::windows-apis"]` | `all-features` | No | write README — **this is the R8 rehearsal crate, prioritize quality** |
| `uffs-mft` | Yes (lib) | inherit | Yes, tune | Missing — write | `["ntfs", "mft", "polars", "filesystem"]` | `["filesystem", "os::windows-apis", "parser-implementations"]` | `all-features` + Windows target | No | write README, add docs.rs Windows target |
| `uffs-format` | Yes (lib) | inherit | Yes, tune | Missing — write | `["csv", "format", "polars"]` | `["encoding", "filesystem"]` | `all-features` | No | write README |
| `uffs-core` | Yes (lib) | inherit | Yes, tune | Missing — write | `["polars", "query", "search", "mft"]` | `["filesystem", "data-structures"]` | `all-features` | No | write README |
| `uffs-client` | Yes (lib) | inherit | Yes, tune | Missing — write | `["uffs", "client", "ipc"]` | `["filesystem", "network-programming"]` | `all-features` | No | write README |
| `uffs-daemon` | **Decide** — bin-only? | — | Yes, tune | Missing — write | — | — | — | Yes | **R8** decision: publishable (end-users install `cargo install uffs-daemon`) or private (`publish = false`) |
| `uffs-mcp` | **Decide** — bin-only? | — | Yes, tune | Missing — write | — | — | — | Yes | **R8** decision: publishable (MCP stdio adapter has standalone value) or private |
| `uffs-broker` | **Decide** — Windows-only bin? | — | Yes, tune | Missing — write | — | — | — | Yes | **R8** decision: Windows elevated handle broker — useful standalone? |
| `uffs-cli` | Yes (bin) | inherit | Yes, tune | Missing — write | `["uffs", "cli", "search", "filesystem"]` | `["command-line-utilities", "filesystem"]` | `all-features` + Windows target | Yes | write README, add `[[bin]]` docs pattern |
| `uffs-diag` | No | `publish = false` | N/A | N/A | N/A | N/A | N/A | Yes (internal) | Explicit `publish = false` — internal diagnostic tool |
| `scripts/ci-pipeline` | No | `publish = false` | N/A | N/A | N/A | N/A | N/A | Yes (internal) | Explicit `publish = false` — CI driver |
| (meta) `uffs` | **Decide** | — | — | — | — | — | — | No | **R6** decision: register a meta crate named `uffs` that re-exports the public API, similar to how `serde` works?  Or leave `uffs` as an unused reserved name?  See §5.8. |

**Rust convention reminder**: crate names on crates.io are
`kebab-case`; Rust identifiers are `snake_case`.  The mapping is
automatic (`uffs-core` crate ↔ `uffs_core` module).  No aliasing
needed.

### 5.2 Dependency DAG and publish order

The workspace internal dependency graph determines publish order.
A crate can only publish when every crate it depends on is already
published (or publishing in the same `cargo publish` invocation,
which cargo handles for workspaces).

From the 12 publishable crates (excluding `uffs-diag`,
`scripts/ci-pipeline`):

```
Level 0 (leaves, no internal deps):
  uffs-polars, uffs-text, uffs-time, uffs-security

Level 1 (depends on level 0):
  uffs-format     → uffs-polars, uffs-text
  uffs-mft        → uffs-polars, uffs-security, uffs-time
                    (re-exports some uffs-time types)

Level 2 (depends on levels 0-1):
  uffs-core       → uffs-polars, uffs-text, uffs-time,
                    uffs-mft, uffs-format, uffs-security

Level 3 (depends on levels 0-2):
  uffs-client     → uffs-core
  uffs-daemon     → uffs-core, uffs-client  (IPC server side)

Level 4 (top-level):
  uffs-cli        → uffs-core, uffs-client, uffs-daemon
  uffs-mcp        → uffs-core, uffs-client
  uffs-broker     → uffs-security (Windows-gated broker)
```

**Publish order for `cargo publish --workspace` (release-plz
handles this automatically)**:

1. `uffs-polars`, `uffs-text`, `uffs-time`, `uffs-security`  (parallel-eligible)
2. `uffs-format`, `uffs-mft`  (parallel-eligible)
3. `uffs-core`
4. `uffs-client`, `uffs-broker`  (parallel-eligible)
5. `uffs-daemon`
6. `uffs-cli`, `uffs-mcp`  (parallel-eligible)

crates.io **rate-limits** publishes to roughly 5/min per account
(historically; check current quotas via the
`https://crates.io/api/v1/crates/...` headers during R8 rehearsal).
12 crates serialize conservatively at ~3 min total publish time.

**DAG validation**: run `cargo metadata --no-deps --format-version
1 | jq '.packages[] | {name, deps: .dependencies[] |
select(.kind == null) | .name}'` and cross-check against the levels
above.  Deviations indicate a dependency has been added/removed
since this plan was written.

### 5.3 Feature flag audit

Each crate that has features needs those features documented for
docs.rs and for downstream consumers.  The `[package.metadata.docs.rs]
all-features = true` config makes docs.rs build with every feature
on, which is the right default unless features conflict
(mutually-exclusive features).

Known feature flags in the workspace (as of this plan):

| Crate | Feature | Purpose | Default? |
|---|---|---|---|
| `uffs-client` | `async` | Async client API (tokio) | Yes — `uffs-cli` overrides with `default-features = false` to drop tokio from the hot-path binary |
| `uffs-mcp` | `streamable-http` | HTTP gateway transport (axum + tower + rmcp HTTP transport).  Required by the `uffs-mcp-http` binary | Yes |
| `uffs-cli` | `mcp-http-probe` | Active `/status` probe for the HTTP MCP gateway in `system-status` output | No (off by default to keep the CLI binary lean) |

> **2026-05-12**: the `uffs-mft.zstd` feature was retired (PR #175).
> The flag was declared optional with `default = ["zstd"]` but every
> workspace consumer pinned `features = ["zstd"]` and recent code
> paths added uses of `zstd::` / `crate::cache::compress_zstd_mt`
> without `#[cfg(feature = "zstd")]` gating.  `cargo hack --workspace
> --each-feature` (now wired into `tier-2.yml::hack` via PR #177)
> surfaced three un-gated sites; the rest of the codebase carried
> ~26 cfg-gates that never fired in practice.  The feature was
> promoted to a hard dependency to match reality.

> **2026-05-12**: `cargo-semver-checks check-release` joined this
> workflow as a second pre-publish guard (Tier R3-05).  Runs against
> the same enumerated publishable-crate set as the `cargo publish
> --dry-run` step, but answers a different question — "does the
> current API match SemVer expectations vs the latest crates.io
> release?" instead of "would crates.io accept this package?".  Pre-
> R8 the guard is informational (no baseline to compare against);
> post-R8 (first publish lands), flip `FAIL_ON_SEMVER_BREAK=true` in
> `crates-io-dry-run.yml` to convert it to a hard gate that catches
> non-major-version bumps containing breaking changes.

For each remaining feature flag, the crate should have a brief docs paragraph:

```rust
//! ## Feature flags
//!
//! - `async` (default): enables the tokio-based `UffsClient`.
//!   Disabling drops the tokio dependency and `ws2_32.dll` (Windows)
//!   from the call-site binary.
```

Audit once in R6.  Add rustdoc-level feature docs to each crate
before R9.

### 5.4 docs.rs rendering — what breaks, what to check

docs.rs builds every published version on push.  Failures are
**silent from crates.io's perspective** (the publish succeeds; only
docs.rs shows a "build failed" badge).  Known failure modes:

1. **Platform-gated items vanish**: docs.rs builds on
   `x86_64-unknown-linux-gnu` by default.  Any `#[cfg(windows)]`
   item gets dropped from the docs.  Fix: set
   `[package.metadata.docs.rs] targets = ["x86_64-unknown-linux-gnu",
   "x86_64-pc-windows-msvc"]` so docs.rs builds twice and the
   Windows surface renders.
2. **Unstable feature gates in docs.rs stable builds**: docs.rs
   uses **nightly** by default, so `#![feature(...)]` works.  But
   if a crate declares `rust-version = "1.91"`, docs.rs may still
   complain.  Set the MSRV honestly; docs.rs handles it.
3. **Missing `cfg(docsrs)` annotations** make feature-gated items
   invisible.  Pattern:
   ```rust
   #[cfg(feature = "async")]
   #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
   pub mod async_api { ... }
   ```
   The `doc(cfg(...))` line requires `#![feature(doc_cfg)]` at
   crate root — but docs.rs enables this flag automatically
   under the `docsrs` cfg.  On stable builds, the line is a
   no-op.
4. **README with relative image/link paths breaks** once rendered
   on crates.io (which serves from a different domain).  Fix: use
   absolute GitHub blob URLs for images, or embed them via the
   `#![doc = include_str!("../README.md")]` pattern with relative
   paths that work in both contexts.
5. **Build timeout (2 hours on docs.rs)** is rarely hit, but
   `uffs-mft` with polars features enabled is a risk.  Monitor
   build times in R8 rehearsal; if polars-ops rebuild exceeds
   90 min, reduce `docs.rs.features` to a minimal set for
   documentation.

### 5.5 Yanking and version deletion policy

Crates.io follows an immutable-version policy except:
- A version can be **yanked** (hidden from new resolves, kept for
  existing lockfiles).  `cargo yank -p <crate> --vers X.Y.Z`.
- A version can be **unyanked** (undo yank).  `cargo yank -p <crate>
  --vers X.Y.Z --undo`.
- A version **cannot be deleted** except via support request
  within 72 hours of publish (for accidental pushes with
  credentials in code, etc.).

Project policy (target state post-R9):

1. **Never delete.  Always yank.**  Deletion breaks downstream
   lockfiles that reference the deleted version; yanking only
   prevents new adoption.
2. **Yank triggers**:
   - Security vulnerability in the published version → yank + release
     patched version same-day
   - Accidentally-published debug/test content → yank within
     24 hours
   - License violation discovered post-publish → yank immediately,
     rebuild and republish under correct license
3. **Do not yank**:
   - Because a later version is "better" — users need the ability
     to pin to older stable versions
   - Because of an API regression you want to hide — release a
     new version with the API restored, don't erase history
4. **Log yanks** in `docs/publishing.md` "yank decisions" section
   with date, version, rationale, replacement version.

### 5.6 Trusted publisher (OIDC) — step-by-step

See §R7 for the phase-level steps.  This section is the reference
for the exact crates.io UI form fields.

crates.io Trusted Publisher form (as of 2026):

| Field | Value | Notes |
|---|---|---|
| Repository owner | `skyllc-ai` | GitHub org or user |
| Repository name | `UltraFastFileSearch` | Exact case |
| Workflow filename | `release-plz.yml` | Relative to `.github/workflows/` |
| Environment name | `crates-io-production` | Must match the GHA `environment:` value exactly |

The workflow must request `id-token: write` in its permissions
block to mint the OIDC token.  The OIDC audience crates.io expects
is `crates.io` (not `api.crates.io` or anything else).  The
`rust-lang/crates-io-auth-action` action handles the audience
negotiation transparently; only break glass if building a custom
auth step.

**If the workflow filename changes**, ALL trusted-publisher
registrations break and must be re-registered.  This is why
`release-plz.yml` was chosen as the registered filename — it's
the natural name, unlikely to be renamed.

### 5.7 First-publish runbook (template for `docs/publishing.md`)

```markdown
# UFFS Publishing Runbook

**STATUS**: DORMANT — publishing is not yet live.  See
[release-automation-plan.md §8](architecture/release-automation-plan.md#8-status-dashboard)
for the R9 go-live decision.

## When do we publish?

Never automatically.  Every publish is a maintainer decision made
in the release PR review step.  Release-plz opens a release PR; a
maintainer reviews the changelog, confirms the version bump, merges,
and at that point the OIDC publish job fires (from R9 onward).

## Pre-publish checklist (one-time, per go-live decision)

- [ ] All 13 crate names reserved on crates.io under the project
      account
- [ ] Trusted-publisher registrations complete for all 13 names
- [ ] `crates-io-production` GitHub Environment exists
- [ ] `release-plz.yml` publish job has `if: true`
- [ ] `release-plz.toml` has `publish = true` at workspace level
- [ ] First-release communication drafted (blog post, release notes,
      Twitter/Mastodon announcement)

## Per-release checklist

- [ ] Release-plz release PR opened
- [ ] Changelog entries reviewed for accuracy
- [ ] Version bump reviewed (feat → minor, fix → patch, feat! →
      major — all consistent)
- [ ] Breaking changes called out in changelog Migration section
- [ ] Release PR merged → tag created → release.yml fires
- [ ] Binaries visible on GitHub Release page
- [ ] Publish job succeeds for all eligible crates (check run logs)
- [ ] Each published crate appears on crates.io within 60 sec
- [ ] docs.rs builds succeed for all published crates within 2 hours
- [ ] Announcement posted (if major release)

## Yank decisions log

| Date | Crate | Version | Rationale | Replacement |
|---|---|---|---|---|
| (none yet) | | | | |

## Post-publish checks

- [ ] `cargo search <crate>` returns new version
- [ ] Throwaway `cargo new test-pub && cargo add <crate> && cargo
      build` succeeds
- [ ] crates.io crate page renders readme correctly
- [ ] docs.rs renders without errors (green build badge)
```

### 5.8 Meta-crate decision: `uffs` top-level crate

Open question for Phase R6: publish a meta crate named `uffs` that
re-exports the public API?

**Pros**:
- Users install ONE crate (`cargo add uffs`), get everything
- Ergonomic for library consumers (`use uffs::query::...`)
- Matches the pattern of `tokio`, `serde`, `clap` — which are
  technically workspaces but expose a single public umbrella

**Cons**:
- Duplication of public API — the meta crate has to keep its
  re-exports in sync with the underlying crates' APIs
- Additional maintenance surface during refactoring
- Potential confusion: is `uffs::mft::...` the same as
  `uffs_mft::...`?  (Yes, via re-export; but users might not
  know that.)

**Recommendation**: defer to Phase R8 decision point.  If UFFS is
primarily a binary app (users install `uffs-cli` via
`cargo install uffs-cli`), the meta crate adds complexity without
value.  If UFFS becomes a library for downstream embedding (MCP
host, external tooling), the meta crate becomes the canonical
API surface.  Today UFFS is 80% binary app, so the meta crate is
likely unnecessary.  But reserve the name anyway — reservations
are cheap, and owning `uffs` on crates.io prevents confusion.

## 6. Risks and open questions

### 6.1 Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | **Release-plz misinterprets commit history**, producing wrong version bump on first active release | Medium | High (confusing user-facing version jump) | Phase R3 shadow mode observes ≥3 runs before activation; manual override always available via editing the release PR before merging |
| 2 | **git-cliff template drift**: changelog sections categorize new commit prefixes incorrectly | Medium | Low (easy to fix in PR review) | R2 validation against last ~6 months of commits; PR review step catches drift |
| 3 | **Concurrent tag creation** from release-plz + `auto-tag-release.yml` during R4 overlap period | Low | Medium (confused CI state) | Tag-exists idempotency guard in `release.yml`; one of the two no-ops cleanly |
| 4 | **Merge conflicts in release PR** when main advances between PR creation and merge | High (for active projects) | Low (release-plz rebases automatically) | release-plz auto-updates the release PR on main advances; manual rebase available |
| 5 | **Commit convention violations** by contributors who bypass commitlint | Medium | Low → High over time (version accuracy degrades) | Advisory phase (R1a) gathers data; mandatory gate (R1b) blocks violators |
| 6 | **crates.io rate limits** hit during full-workspace publish in R9 | Low | Medium (partial publish, DAG disrupted) | Release-plz handles rate limit backoff; publish order (§5.2) respects DAG so partial publish fails closed, not open |
| 7 | **OIDC misconfiguration** in R7 blocks publishing in R9 | Medium | Medium (blocks release) | R7 dry-run validation catches this; rollback is trivial (reset `if: false`, investigate offline) |
| 8 | **Premature publishing** — someone flips `publish = true` before R9 gates are satisfied | Low | Very high (irreversible — cannot delete crates.io versions) | `if: false` gate + `crates-io-production` env required-reviewer + `publish = false` at workspace level + per-crate `publish = false` — **four independent locks**, all must be explicitly defeated |
| 9 | **SemVer regression** — patch release accidentally breaks API, violating SemVer contract | Medium | High (ecosystem trust damage) | Pre-R9 gate: `cargo-semver-checks` in PR Fast CI for ≥1 release cycle |
| 10 | **Orphaned meta-crate** (`uffs`) reserved but never maintained | Low | Low (name squatting for one's own project) | Reserve as stub 0.0.0 with clear pointer to canonical crates; yank if decision is "no meta crate" |
| 11 | **Release-plz tooling breakage** (upstream bug, API change, action deprecation) | Low per cycle, High over 2 years | Medium (blocks releases until fixed) | Action SHA pinning; fall back to manual `cargo publish` via runbook (§5.7); monitor release-plz changelog |
| 12 | **Fork PR poisoning release-plz**: malicious contributor opens a PR that manipulates commit messages to force an unexpected version bump | Low | Medium (wrong version, recoverable via amend) | release-plz only runs on `main`, not on PRs; PR commit messages are scanned but not acted upon until the PR merges; merge requires review |
| 13 | **Interim `Cargo.lock` drift** between R0 and R5 — if the optional R0 step-5 patch is skipped, any releases cut via `just ship` during the R1-R4 window continue to ship with drifted lockfiles (see §2.2) | Medium (certain if step skipped) | Low (binaries still build; self-heals intermittently) | Either apply the R0 step-5 one-line patch, or accept that releases during the transition are not byte-reproducible from their `Cargo.lock`.  Fully resolved at R5 (release-plz refreshes lockfile natively via `dependencies_update = true`). |

### 6.2 Open questions (resolve during phases)

1. **`uffs-daemon`, `uffs-mcp`, `uffs-broker`: publishable or
   private?**  Decide in R6/R8 dress rehearsal.  Default: publish
   `uffs-mcp` (clear standalone value: MCP stdio adapter), keep
   `uffs-daemon` and `uffs-broker` private unless there's
   downstream demand.
2. **Meta crate `uffs`: publish or reserve-only?**  See §5.8.
   Default: reserve as 0.0.0 stub, decide on content after R8.
3. **Release cadence: per-merge or batched?**  Release-plz defaults
   to per-merge (every meaningful merge to main opens a release
   PR).  Alternative: weekly batch via `release-plz.toml
   release_pr_schedule`.  Default: per-merge; revisit if noise
   becomes painful.
4. **Pre-release channel (`-alpha.N`, `-rc.N`)?**  Not needed until
   approaching v1.0.  Revisit as part of R9 go-live planning.
5. **Changelog format**: Keep a Changelog (current) vs. commit-
   subject list (git-cliff default).  Default: git-cliff default
   with `cliff.toml` sections shaped to approximate Keep a
   Changelog (Added, Changed, Fixed, Removed).  Decide in R2.
6. **Advisory commitlint runner**: own workflow or step in
   pr-fast.yml?  Default: separate `commitlint.yml` workflow —
   easier to disable / toggle independently of PR Fast CI.
7. **Per-crate versioning (R9-beta)**: when does the single-
   workspace-version approach become painful?  Probably around
   v1.0 when `uffs-polars` stabilizes ahead of `uffs-cli`.
   Defer.

## 7. Rollback playbook

Per-phase rollback is documented in each phase's **Rollback**
block.  This section is the cross-cutting playbook for
"something went wrong across multiple phases."

### 7.1 Rollback triggers

Any of these conditions warrants rollback consideration:

1. A release-plz run produces a changelog or version bump that
   cannot be corrected by editing the release PR (i.e. the output
   is structurally wrong, not cosmetically wrong)
2. `release.yml` stops triggering on tags (tag/workflow trigger
   divergence)
3. A published crates.io version cannot be consumed
4. OIDC authentication fails consistently, blocking releases
5. Commitlint false-positive rate > 20% of PRs (hard-gate phase)

### 7.2 Rollback order (latest phase first)

Always roll back in **reverse execution order**.  Never "leap back"
to an earlier phase skipping intermediate rollbacks.

Example: if R7 breaks, roll back R7 first; do not roll back to
R5 while R6 remains in the forward state.  This preserves the
reversibility invariant.

| Forward phase | Rollback command | Post-rollback state |
|---|---|---|
| R9 | Revert the `if: false` flip PR; `cargo yank` any versions published in the rollback window | Publishing disabled; historical versions remain on crates.io |
| R8 | Revert the scope/flip PR; `cargo yank -p uffs-time --vers <rehearsal-version>` | OIDC scaffolding in place but dormant |
| R7 | Revert the OIDC job PR; remove GitHub Environment | Dry-run CI still running; no OIDC trust registrations needed (but they stay on crates.io harmlessly) |
| R6 | Revert per-crate metadata + `crates-io-dry-run.yml` | Back to R5 steady state; reserved names stay owned |
| R5 | Revert the deletion PR → restores `build/update_all_versions.rs`, `auto-tag-release.yml`, and `scripts/ci-pipeline/src/version.rs` | Both the bespoke flow and release-plz flow present concurrently |
| R4 | Revert the permissions/release-job PR | Release-plz back to shadow mode |
| R3 | Revert the release-plz.toml + workflow PR | Release-plz not installed; git-cliff still present |
| R2 | Revert the cliff.toml PR | `git-cliff` uninstalled-ish (the tool doesn't mind; the config just disappears) |
| R1 | Revert the commitlint workflow PR | No commit convention checks; contributors continue by habit |
| R0 | Revert the baseline / cleanup PR | Back to pre-plan state |

### 7.3 Partial rollback patterns

- **Rollback only the "active" bit of R4 without losing R3
  shadow observability**: edit `release-plz.yml` to remove the
  `release` job (keep `release-pr` in shadow mode).  Do not delete
  the whole workflow.
- **Rollback only R9's publishing without affecting R5's
  deletion**: flip `if: false`, flip `publish = false` in
  `release-plz.toml`, but do not un-delete `build/
  update_all_versions.rs`.  The bespoke tooling stays deleted;
  we just stop publishing new versions.
- **Partial crate publishing after R9**: set `publish = false`
  on individual `Cargo.toml` per crate, release-plz skips those.

### 7.4 Communication plan for rollback

If a rollback is executed after a public release has shipped:

1. Open a GitHub issue with `rollback` label explaining which
   phase was rolled back and why.
2. Post an announcement in release notes or repo README if the
   rollback affects user-facing behavior (binary distribution,
   published crates).
3. Update this plan's §8 status dashboard with the rollback
   date and rationale.
4. Write a deviations-log entry in `dev-flow-implementation-plan.md`
   §10.5 if the rollback reveals a systemic issue.

## 8. Status dashboard

Single source of truth for phase progress.  Mirror the format of
`dev-flow-implementation-plan.md §5 status dashboard`.

**Settled decisions** (recorded 2026-04-24 before R0 opened):

1. **R0 step-5 lockfile patch**: **INCLUDE**.  Adds the one-line `cargo generate-lockfile --offline` step to `build/update_all_versions.rs` so interim releases during R1-R4 are byte-reproducible from `Cargo.lock`.  Eliminates risk #13 during the transition window.
2. **Dev-flow Phase 7 sequencing**: R5 lands AFTER dev-flow Phase 7's final bake-in observation.  Automatically satisfied by the plan's natural order — dev-flow Phase 7 needs one `just ship` bake (its only remaining `[ ]` item); release-auto R3/R4 also need ≥1 live release for shadow-mode observation.  **The same release cycle satisfies both.**  R5 is the first phase where the two plans mutually unblock; no explicit wait needed.

| # | Phase | Status | Commit | Date | PR | Notes |
|---|---|---|---|---|---|---|
| R0 | Baseline & cleanup (remove dead cargo-dist + release-plz metadata, add lockfile-drift patch per decision 1, new baseline doc) | 🟢 | `873a668c4` | 2026-04-25 | [#64](https://github.com/skyllc-ai/UltraFastFileSearch/pull/64) | Final landed PR shape: 6 files, +1258 / −160 LOC. Includes: lockfile patch; **promotion of `build/update_all_versions.rs` into version control** via `.gitignore` carve-out (1073-line script was previously gitignored despite 4 callsites depending on it); drive-by deletion of stale `crates/uffs-mft/Cargo.toml.bak` (v0.4.106 auto-commit artifact); baseline metrics in `release-automation-baseline.md`. |
| R1a | Conventional commits (advisory) | 🟢 | `966f09c5f` | 2026-04-25 | [#65](https://github.com/skyllc-ai/UltraFastFileSearch/pull/65) | Final landed PR shape: 1 file added (commitlint workflow), 224 LOC.  Workflow self-validated by running on its own opening PR (3-second pass).  Sticky-comment mechanism via `gh api PATCH/DELETE` confirmed working.  CONTRIBUTING.md "Commit message conventions" section already landed pre-R0 (lines 150-187). |
| R1b | Conventional commits (mandatory gate) | ⬜ | | | | After ≥1 month of advisory observation |
| R2 | `git-cliff` + `cliff.toml` (local validation) | 🟢 | `d49a778d6` | 2026-04-25 | [#66](https://github.com/skyllc-ai/UltraFastFileSearch/pull/66) | Final landed PR shape: 3 files (1 new, 2 modified), +495 / −3 LOC.  `cliff.toml` template iterated against full history until output matches Keep-a-Changelog spacing; type → section mapping mirrors `commitlint.yml` regex (11 types).  Validation captured in `release-automation-baseline.md` §8.  Two iteration issues caught + fixed during template tuning (extra blank line after `## [version]`, duplicate `(#NN)` PR links). |
| R3 | release-plz shadow mode | 🟢 | `1b0aa55b7` | 2026-04-25 | [#67](https://github.com/skyllc-ai/UltraFastFileSearch/pull/67) | Final landed PR shape: 2 files (1 new workflow, 1 new release-plz.toml) + ~370 LOC.  Workflow runs `release-plz update` (local-only by design) on every `push: main` and posts the proposed diff to the workflow summary.  Three layers of dormancy: `publish = false` in config, missing `CARGO_REGISTRY_TOKEN`, read-only workflow permissions. **Post-merge observation** revealed shadow output stayed empty across ≥12 days because `release-plz update` failed silently inside the workflow on `cargo package`'s "dependency `uffs-X` does not specify a version" error — fixed in R3.5 below by adding `version = ` requirements to internal `[workspace.dependencies]` entries. |
| R3.5 | Internal-dep `version = ` requirements + polars git-pin version annotation | 🟢 | `cccf4f111` | 2026-05-07 | [#145](https://github.com/skyllc-ai/UltraFastFileSearch/pull/145) | Bundled into the R6 PR (see §8.1 deviations log first row).  Adds `version = "0.5.90"` to all 8 internal workspace.dependencies, to the 2 direct path-deps in `uffs-cli/Cargo.toml`, and `version = "0.53.0"` to the polars git dep.  Updates `just polars` to keep the polars version pin in lockstep with the resolved git rev.  Without these, every `cargo package` invocation (release-plz `update` and any future `release-pr`) fails with "dependency `<name>` does not specify a version".  Verified locally: `release-plz update --config release-plz.toml` now lists all 12 publishable crates without error. |
| R4 | release-plz active (release PR mode) | 🟡 | | 2026-05-08 | (this PR) | Active-mode workflow flip (`update` → `release-pr` + `release`).  Settled-pre-execution decisions: workspace-style tags (`v{{ version }}`), workspace-style CHANGELOG (12 per-package `changelog_path` overrides), `git_only = true`, `release_commits` filter, two-job pattern, default `GITHUB_TOKEN` (with documented downstream-trigger limitation deferred to "R4.5"), first-release v0.5.91 bootstrap done out-of-band by maintainer (v0.5.90 worktree predates the R3.5 dep-version fix).  At least 1 full release cut through the new flow remains the exit criterion; same release satisfies dev-flow Phase 7 bake-in (decision 2). |
| R5 | Retire bespoke tooling (incl. `scripts/ci/ci-pipeline.rs` thin wrapper per its `REMOVE-AFTER: v0.5.73` marker) | 🔴 ROLLBACK | `779c14fb1` (landed); reverted same-day; `auto-tag-release.yml` subsequently restored in `a7bdeb6a3` | 2026-05-08 → 2026-05-09 | [#153](https://github.com/skyllc-ai/UltraFastFileSearch/pull/153) (landed) → revert PR → [#160](https://github.com/skyllc-ai/UltraFastFileSearch/pull/160) (auto-tag-release.yml re-instated) | Landed via PR #153 then rolled back same-day pending polars-upstream chrono-compat release.  The R5 rollback's original "EXCEPTION: auto-tag-release.yml stays deleted" clause was superseded next day by PR #160 when the bespoke `just ship` flow needed the version-diff → release.yml dispatcher back to keep automatic binary builds working — see `R5 rollback post-script` deviation row in §8.  Re-application is a single-PR forward step once `cargo package -p uffs-polars` succeeds end-to-end (at which point auto-tag-release.yml is deleted again and the `release-plz.yml` → `release.yml` `workflow_dispatch` bridge lands together). |
| R6 | crates.io metadata audit + dry-run CI | 🟢 | `cccf4f111` | 2026-05-07 | [#145](https://github.com/skyllc-ai/UltraFastFileSearch/pull/145) | Adds: `[package.metadata.docs.rs]` to all 12 publishable crates with appropriate `targets`/`default-target` per crate's platform surface; explicit `publish = false` to `crates/uffs-diag/Cargo.toml`; per-package `release = false` blocks for the 3 internal CI tools in `release-plz.toml`; `.github/workflows/crates-io-dry-run.yml` (advisory weekly + workflow_dispatch); `docs/publishing.md` DORMANT runbook.  R6 step 6 (crate name reservations on crates.io) is intentionally **deferred** — those happen from a throwaway external workspace per plan §R6 step 6, not from this repo. |
| R7 | OIDC trusted publisher (dormant) | ⬜ | | | | Scaffolding, `if: false` gate |
| R8 | First publish dress rehearsal (`uffs-time` only) | ⬜ | | | | **External state change** — one crate goes live on crates.io |
| R9 | Live publishing (full workspace) | ⬜ | | | | **DEFERRED** — explicit maintainer decision, separate plan |

Legend: ⬜ pending · 🟡 in progress · 🟢 complete · 🔴 blocked · ⏸️ paused

### 8.1 Deviations log

Mirror the format of
`dev-flow-implementation-plan.md §10.5 deviations log`.

| Phase | Date | Anomaly | Root cause | Resolution | Plan impact |
|---|---|---|---|---|---|
| R3 → R4 readiness | 2026-05-07 | Shadow-mode workflow ran 12+ days with empty output; `release-plz update` failed silently inside the runner. | The R3 plan did not anticipate that internal `[workspace.dependencies]` entries lacking `version = ` would block `cargo package` (which release-plz invokes per crate). All 8 internal-dep aliases (and the polars git pin, and the 2 direct path-deps in `uffs-cli`) were affected. | Bundled into the R6 PR as **phase R3.5** (see dashboard row above): added `version = "0.5.90"` to all internal-dep aliases + `version = "0.53.0"` to the polars git dep; `just polars` now updates the polars version pin in lockstep with the rev. | None — R3 stays 🟢, R3.5 closes the gap, R4 advances on schedule. |
| R6 step 6 | 2026-05-07 | Crate-name reservations on crates.io explicitly NOT performed in this PR. | Plan §R6 step 6 specifies reservations should come from a "throwaway dedicated workspace" (a separate, non-UFFS repo) so the UFFS repo never holds a `publish = true` state. | Reservations deferred to a separate out-of-band operation. The R6 PR documents the prerequisite in `docs/publishing.md` "Pre-publish checklist" and the `crates-io-dry-run.yml` workflow header. | None — exit criteria for R6 was already split between in-repo work and external operation; the in-repo half is what landed here. |
| R6 → R8 publishability | 2026-05-07 | `cargo publish --dry-run -p uffs-polars` (and any crate transitively depending on it) fails with `failed to select a version for chrono` because the published-form `polars = "0.53.0"` resolution against crates.io conflicts with the workspace-pinned chrono. | Our git-pinned polars rev uses a different feature mix than crates.io polars 0.53.0; the registry version pulls `polars-arrow → chrono-tz` requirements that conflict with our `chrono` pin. | Recorded as a known-expected failure in `crates-io-dry-run.yml` (advisory mode, `FAIL_ON_DRY_RUN_ERROR=false` initially). R8 dress rehearsal will resolve by either (a) flipping `uffs-polars` to `publish = false` or (b) aligning chrono with crates.io polars expectations. | None — does not block R7 (OIDC scaffolding) or the leaf-only R8 rehearsal target (`uffs-time`). |
| R4 baseline | 2026-05-08 | First R4 active-mode workflow run on R4 merge commit `6790a8162` (workflow run [25532369301](https://github.com/skyllc-ai/UltraFastFileSearch/actions/runs/25532369301)) fails the `release-plz-pr` job with `failed to update packages` → `failed to determine next versions` → `cargo package failed: ... dependency `uffs-security` does not specify a version`.  ORIGINAL prediction in this row said "release-plz silently treats as no-baseline" — the ACTUAL behaviour is HARD-FAIL of the workflow run.  Misprediction corrected here. | `git_only = true` (R4 decision 3) tells release-plz to check out the v0.5.90 tag's worktree for baseline comparison; that worktree pre-dates R3.5 (`cccf4f111`) so its internal-dep aliases lack `version =`.  release-plz invokes `cargo package` per crate to compute baseline; that fails hard, propagates up the call stack, and exits the action non-zero.  Self-healing transient: once a fresh tag (v0.5.91 — see resolution column) is cut from current `main` (which has the R3.5 fix), all subsequent baseline checks succeed. | First-release v0.5.91 bootstrap performed by maintainer via the existing `just ship` bespoke flow (PR [#149](https://github.com/skyllc-ai/UltraFastFileSearch/pull/149), squash-merged to `5ff321b04`).  `auto-tag-release.yml` detected the Cargo.toml version diff and dispatched `release.yml`, which created the `v0.5.91` tag at the merge commit + built binaries.  Bootstrap NOT done via release-plz (release-plz's release-pr job kept failing on the v0.5.90 baseline through the bootstrap window).  Post-bootstrap: `git_only` baseline check now uses v0.5.91 worktree (which has R3.5 fix), so `release-plz-pr` succeeds on subsequent pushes. | None — R4 stays 🟢; the workflow run that failed on the R4 merge commit is a known harmless one-time race captured by this entry; no rollback. |
| R4 downstream-trigger | 2026-05-08 | Tags created by release-plz via the workflow-provided `GITHUB_TOKEN` do NOT trigger `release.yml` (which has `on: push: tags: [v*]`). | GitHub's anti-loop policy: actions triggered by `GITHUB_TOKEN` can create refs but those refs do not fire downstream workflows. | R4 ships with default `GITHUB_TOKEN` (decision 6 above) and documents the limitation in both the workflow header and this entry. The maintainer pushes the v0.5.91 bootstrap tag manually for the first release. A follow-up PR (informally "R4.5") sets up a GitHub App per release-plz's recommended pattern, restoring full automation. | None — non-blocking; future R4.5 PR resolves it. |
| R4 release-job race | 2026-05-08 | First R4 active-mode workflow run on PR [#149](https://github.com/skyllc-ai/UltraFastFileSearch/pull/149)'s merge commit `5ff321b04` failed the `release-plz-release` job with `failed to create ref refs/tags/v0.5.91 with sha 113f188...` → GitHub 422 “Reference update failed”.  release-plz interpreted the 422 as “commit not pushed”, but the actual cause was a tag-creation race against `auto-tag-release.yml` → `release.yml`.  release-plz computed a synthetic local commit (`113f188...`) containing a regenerated CHANGELOG (because the bespoke `update_all_versions.rs` rewrites `## [0.5.90]` → `## [0.5.91]` in place rather than producing a cliff-style entry, leaving the CHANGELOG state inconsistent with what release-plz expects), then tried to tag that local SHA — which was never pushed and which conflicted with `release.yml`'s tag at the actual merge SHA. | `release_always = true` (release-plz default) makes `release-plz release` attempt a tag-creation on EVERY push to `main`, regardless of whether the merge came from a release-plz-PR review path or from another route (here: the bespoke `just ship` `release/v0.5.91` PR).  Two tag-creators racing for the same `v0.5.91` ref — only one can win; release-plz lost. | Set `release_always = false` workspace-level in `release-plz.toml` (PR [#151](https://github.com/skyllc-ai/UltraFastFileSearch/pull/151)).  release-plz `release` job now only fires when the latest commit on `main` is the merge of a PR whose branch starts with `pr_branch_prefix` (`release-plz-`).  Bespoke `just ship` cycles use `release/vX.Y.Z` branch names — NOT `release-plz-*` — so the `release` job no-ops cleanly on those merges, leaving `auto-tag-release.yml` + `release.yml` as the sole tag-creator during the R4 → R5 transition window.  Post-R5 (when bespoke flow is deleted), only the `release-plz-*` path remains, and `release_always = false` continues gating correctly without modification.  Same setting recommended by [release-plz docs](https://release-plz.ieni.dev/docs/config) for any project using PR-gated releases. | None — R4 stays 🟢; surgical config-only fix; no workflow changes. |
| R5 rollback (polars-upstream wait) | 2026-05-08 | PR [#153](https://github.com/skyllc-ai/UltraFastFileSearch/pull/153) (R5) deleted the bespoke version-bump tooling (`build/update_all_versions.rs`, `STEP_VERSION_INCREMENT` in `scripts/ci-pipeline/src/workflow.rs`, `version_bump`/`increment_version` in `version.rs`, `auto-tag-release.yml`, `version-bump` recipe in `just/build.just`, and the version-bump step in `quick-deploy`).  Same-day, PR [#157](https://github.com/skyllc-ai/UltraFastFileSearch/pull/157) deferred release-plz's `push: branches: [main]` auto-trigger because the chrono ≤0.4.41 vs 0.4.44 conflict in `cargo package --workspace` (release-plz's hardcoded baseline step) cannot be sidestepped by `publish = false` flags alone.  Combined effect: release-plz is dormant AND the bespoke fallback is deleted — `just ship-fresh` fails at step `10-git-commit` with "nothing to commit, working tree clean" because no step bumps the workspace version anymore.  The release pipeline has no working version-bump path. | R5 prerequisites required `≥ 2` release-plz-driven releases before deleting the bespoke fallback (plan §R5 exit criteria); the `R5-before-R4-bakein` deviation row above acknowledged we shipped R5 with **0** observed cycles.  When R6→R8 Path A and the release-plz upstream-source bug (release_plz_core/src/next_ver.rs hardcodes `cargo package --workspace` regardless of `release = false`) jointly forced the deferral in PR #157, R5's premise — "release-plz is the sole version-bump driver" — went away.  No driver, no bumps. | Revert PR [#153](https://github.com/skyllc-ai/UltraFastFileSearch/pull/153) on top of current `main` (post-#157), restoring `update_all_versions.rs` + `STEP_VERSION_INCREMENT` + `just ship` step 07 + the `version-bump` recipe + `.gitignore` carve-out + `quick-deploy` bump + R5's removed docstrings.  **EXCEPTION:** `.github/workflows/auto-tag-release.yml` stays deleted (operator decision: `release-plz.yml`'s dormant `gh workflow run release.yml` bridge plus `release.yml`'s native `on: push: tags: [v*]` trigger jointly cover tag→build dispatch when re-enabled or when the maintainer pushes a signed tag manually).  Preserve PR [#154](https://github.com/skyllc-ai/UltraFastFileSearch/pull/154) (CC-regex convergence), PR [#155](https://github.com/skyllc-ai/UltraFastFileSearch/pull/155) (`publish = false` / `release = false` flips on the 8 polars-tainted crates), and PR [#157](https://github.com/skyllc-ai/UltraFastFileSearch/pull/157) (release-plz deferral header + commented `push:` block).  R5 dashboard row flips back from 🟢 to 🔴 ROLLBACK with this PR; will return to 🟢 only after polars upstream ships a chrono-compat release that lets `cargo package -p uffs-polars` succeed end-to-end (tracked weekly via `crates-io-dry-run.yml`). | None for the in-flight workflow — bespoke `just ship` is back to its pre-R5 working shape and the next manual version bump runs through it.  Public release sequence resumes via the bespoke flow: `just ship` → `release/vX.Y.Z` PR → squash-merge to `main` → maintainer pushes signed `vX.Y.Z` tag → `release.yml` builds + publishes.  Re-applying R5 once polars upstream lands becomes a single-PR forward step: re-delete the same files this revert restores, re-add `STEP_VERSION_INCREMENT`, uncomment `push: branches: [main]` in `release-plz.yml`, optionally re-delete `auto-tag-release.yml` (which this rollback already keeps deleted). |
| v0.5.91 immutable-release lockout | 2026-05-08 | `release.yml` finalize step on the v0.5.91 bootstrap merge (run [25549839464](https://github.com/skyllc-ai/UltraFastFileSearch/actions/runs/25549839464)) failed with `error finalizing release: HttpError: Validation Failed: pre_receive Repository rule violations found ... Cannot create ref due to creations being restricted ... tag_name was used by an immutable release`.  The partial release was deleted to recover, but the **`v0.5.91` tag name became permanently locked** by GitHub's *immutable releases* feature: subsequent attempts to recreate the ref via `git push origin v0.5.91` (signed annotated tag at `5ff321b04`) and via the REST API both rejected with the same `gh013: Cannot create ref due to creations being restricted` error.  Confirmed via direct push attempts and via the live ruleset matrix (`gh api repos/.../rules/branches/v0.5.91`); the `tag-protection-v-prefix` ruleset only blocks `deletion`/`update`, so the actual block is the immutable-release pre-receive hook. | Once a release is published with the repo's *immutable releases* feature engaged, the tag-name → release association is permanent.  Even after the release object is deleted, the tag name remains burned and pre-receive rejects any future ref creation under that name — there is no UI surface for reusing it short of contacting GitHub support. | Skipped v0.5.91 entirely; bootstrapped v0.5.92 instead via PR [#152](https://github.com/skyllc-ai/UltraFastFileSearch/pull/152) (manual `release/v0.5.92` branch — bespoke flow's last cycle).  v0.5.92 published cleanly on 2026-05-08T12:13:39Z with all 29 assets (1 CHECKSUMS + 13 SBOMs + 15 binaries) and a signed `v0.5.92` annotated tag.  CHANGELOG.md carries an explicit *Note on the v0.5.91 gap* explaining the discontinuity in the public release sequence (`v0.5.90` → `v0.5.92`).  All intended v0.5.91 changes are rolled forward into v0.5.92. | None — public sequence skips v0.5.91 forever; that single tag name is now reserved and unusable. |
| R5-before-R4-bakein | 2026-05-08 | Plan §R5 prerequisites call for `≥ 2` full releases successfully cut via release-plz before deleting the bespoke fallback.  This PR lands R5 with **0** release-plz-driven releases observed (v0.5.92 was bootstrapped via the bespoke flow, same as v0.5.85 through v0.5.91).  The exit criterion will land naturally on the next `feat:`/`fix:`/`perf:`/`security:` commit to `main`, which will trigger release-plz to open a `release-plz-vX.Y.Z` PR; merging that PR will produce v0.5.93 end-to-end through the new flow. | Pragmatic acceleration: (1) the v0.5.91 immutable-release lockout demonstrated the bespoke flow has its own failure modes; the "wait for ≥2 release-plz cycles" prerequisite no longer trades meaningful additional safety against the cost of keeping ~1430 LOC of dual-driver code on `main`.  (2) The R5 implementation is fully reversible (`git revert` of a single PR restores everything); the rollback discipline that made earlier phases safe applies here.  (3) v0.5.92's success ground-truths the build path that R5's bridge dispatches into. | This PR.  R4 stays 🟡 in the dashboard until the first release-plz-driven release lands (per its original exit criterion); R5 transitions to 🟢 immediately because its OWN exit criteria (workspace builds, no orphan references to deleted scripts) are met by this PR.  Once v0.5.93 ships through release-plz, R4 flips to 🟢 in a follow-up commit (single-row dashboard edit). | If the next release-plz-driven release fails for any reason, `git revert` of this PR restores `auto-tag-release.yml` + `update_all_versions.rs` + the `version-bump` recipes; the bespoke flow is back operational with one merge.  No data has moved. |
| R5 cache-warm short-circuit | 2026-05-08 | `release-cache-warm.yml` and `release.yml` were firing on the same version-bump push to `main`, racing to build identical Polars-heavy dep graphs in parallel.  `release.yml` saved its own cache at job end, overwriting whatever cache-warm produced — pure waste of ~165 runner-min/release (Linux ~45 + macOS ~45 + Windows ~75). | Cache-warm fires on every Cargo-touching push to `main` (its `paths:` filter matches `Cargo.toml`/`Cargo.lock`/`crates/**`).  Version-bump merges always touch `Cargo.toml`, so cache-warm fires alongside `release.yml`'s `auto-tag-release.yml` dispatch (pre-R5) or `release-plz-release`'s bridge dispatch (post-R5). | Add a cheap (~5s) `detect-release-bump` job that diffs `[workspace.package].version` between `HEAD` and `HEAD~1`; the warm matrix is gated on `needs.detect-release-bump.outputs.is_release_bump != 'true'`.  Pure docs / scripts / CHANGELOG-only commits never reach this job because the `paths:` filter already excludes them; only Cargo-touching commits run the diff. | None — net positive; observable on the next release as cache-warm completing in `~15s` instead of `~165min`. |
| R1b CC-type convergence (early) | 2026-05-08 | Four regexes governed the allowed Conventional Commits type list: (a) `scripts/ci/check_commit_subjects.sh` (local commit-msg + pre-push hook); (b) `.github/workflows/commitlint.yml` (PR-title advisory check); (c) `cliff.toml` `commit_parsers`; (d) `release-plz.toml` `release_commits`.  (a) and (b) accepted only the 11 standard CC types; (c) and (d) ALSO accepted top-level `security:` / `security(<scope>):` as a tolerance for PRs #31, #33, #34 (early-project commits that landed before the local hook was installed).  Plan §R1b explicitly deferred this convergence ("Phase R1b decides whether to keep this exception or migrate those commits to `chore(security):`") to a future ≥1-month-after-R1a window. | The `security:` carve-out was a one-time historical accommodation, not a forward convention.  Since the commit-msg hook landed (well before this PR), no future commit can use the `security:` prefix on `main`, so the cliff.toml + release-plz.toml carve-outs were preemptively allowing something that can no longer reach the codebase.  The dedicated `^fix\(security\)` and `^chore\(security\)` parsers in `cliff.toml` already route security work to the **### Security** changelog section without requiring a non-CC top-level type. | Drop `security` from `release_commits` in `release-plz.toml` (regex collapses to `^(feat\|fix\|perf)(\(.+\))?:`).  Drop the `^security(\([a-z0-9-]+\))?:` parser line + carve-out comment from `cliff.toml`.  Add a "Security commits" paragraph to `CONTRIBUTING.md` § "Commit message conventions" codifying `fix(security):` (patch + Security section) and `chore(security):` (no bump + Security section) as the canonical encodings; explicitly state that top-level `security:` is not allowed.  All four regexes now agree on the 11 standard CC types. | Brings the R1b enforcement decision forward by ~1 phase.  Does not change Phase R1a → R1b advisory→required scheduling for the *commitlint workflow itself*; only resolves the orthogonal "should `security:` be a top-level type" sub-question.  Historical PRs #31/#33/#34 remain in CHANGELOG and `release-automation-baseline.md` §4; their entries are unaffected. |
| R5 downstream-trigger bridge | 2026-05-08 | Resolution of the prior `R4 downstream-trigger` deviation row.  release-plz creates the workspace tag (`vX.Y.Z`) using the workflow-provided `GITHUB_TOKEN`; that tag push does NOT fire `release.yml`'s `on: push: tags: [v*]` trigger because of GitHub's anti-loop policy on GITHUB_TOKEN-pushed refs. | Same root cause as the R4 row.  R4 deferred the workaround to a follow-up PR (GitHub App / PAT setup); R5 inlines an alternative that does not require new secrets. | After the `release-plz/action` step in the `release-plz-release` job, capture `releases_created` + `releases` outputs and call `gh workflow run release.yml --ref main -f version=$tag -f commit_sha=$sha -f triggered_by=release-plz[$tag]`.  `workflow_dispatch` events are explicitly carved out of the GITHUB_TOKEN anti-loop policy ([GitHub docs](https://docs.github.com/en/actions/using-workflows/triggering-a-workflow#triggering-a-workflow-from-a-workflow)), so the dispatch fires `release.yml` reliably from the default identity.  Permission delta: `release-plz-release` job grants `actions: write` (in addition to its existing `contents: write`).  Also flips `git_release_enable = false` in `release-plz.toml` so release-plz only creates the tag — `release.yml` owns the GitHub Release page (avoids the body-overwrite race that would otherwise occur when softprops/action-gh-release updates a release-plz-created Release with `body_path: release-notes.md`).  **STATUS (2026-05-12):** reverted along with the rest of R5 (PR [#153](https://github.com/skyllc-ai/UltraFastFileSearch/pull/153) rollback); the current `release-plz.yml` on `main` is dormant (`workflow_dispatch` only) and does NOT contain the bridge step.  The R4 downstream-trigger problem is therefore CURRENTLY RESOLVED by a different route: `auto-tag-release.yml` is back (PR [#160](https://github.com/skyllc-ai/UltraFastFileSearch/pull/160)) and continues to dispatch `release.yml` from user-driven `Cargo.toml` version pushes.  The workflow-dispatch bridge lands again when R5 re-lands, at which point `auto-tag-release.yml` is re-deleted. | None — replaces the deferred R4 workaround in code without secrets; manual-tag escape hatch (maintainer-pushed `vX.Y.Z`) still works because user-driven pushes are not subject to the anti-loop policy.  Post-R5-rollback, the R4 downstream-trigger problem reverts to being covered by `auto-tag-release.yml` (see adjacent `R5 rollback post-script` row). |
| R5 rollback post-script (auto-tag-release.yml restored) | 2026-05-09 | The `R5 rollback` deviation row (one day earlier) prescribed keeping `auto-tag-release.yml` deleted as an "EXCEPTION" to the revert on the theory that the deferred-but-imminent `release-plz.yml` → `release.yml` workflow-dispatch bridge (the `R5 downstream-trigger bridge` row's mechanism) would cover tag-creation.  But with release-plz itself deferred to `workflow_dispatch`-only in PR [#157](https://github.com/skyllc-ai/UltraFastFileSearch/pull/157), no automated path now dispatches `release.yml` from a `Cargo.toml`-bump push; the bespoke `just ship` flow has no way to reach `release.yml` except via a maintainer-pushed signed tag.  Observable consequence: the v0.5.93 bespoke bump merge (post-rollback) produced no automatic binary build. | Partial-revert sequencing error in the R5 rollback.  The "EXCEPTION" was written assuming the `workflow_dispatch` bridge would be in place; when both are absent (bridge reverted, auto-tag-release deleted) the release path has a silent hole. | PR [#160](https://github.com/skyllc-ai/UltraFastFileSearch/pull/160) (commit `a7bdeb6a3`, 2026-05-09): single-file restoration of `.github/workflows/auto-tag-release.yml` from its pre-R5 shape.  The restored workflow watches `Cargo.toml` → `[workspace.package].version` diffs on `push: main` and dispatches `release.yml` with the new version.  Bespoke flow (v0.5.93, v0.5.94) immediately resumed producing automatic binary builds.  The R5 §3.2/§3.4 "auto-tag-release.yml DELETED in Phase R5" cells updated in-line (2026-05-12 cleanup PR) to reflect the restoration. | Dashboard R5 row `Notes` + `Commit` columns extended to cite PR #160 as the post-rollback repair.  When R5 re-lands (polars upstream releases chrono-compat artefacts), the "re-apply R5" PR re-deletes `auto-tag-release.yml` **and** re-lands the `workflow_dispatch` bridge in `release-plz.yml` in the same merge — the two are tightly coupled and must move together. |
| R6 → R8 publishability resolution (Path A) | 2026-05-08 | Resolution of the prior `R6 → R8 publishability` deviation row.  Probed option (b) of the original row's resolution column ("aligning chrono with crates.io polars expectations") and found it infeasible.  Probe details: dropped the polars `git/rev` pin in `crates/uffs-polars/Cargo.toml` and switched to `polars = "=0.53.0"` from crates.io.  Workspace `chrono` pinned to `=0.4.41` to satisfy crates.io polars-arrow 0.53.0's `<=0.4.41` constraint.  `cargo update` succeeded.  But `cargo build --workspace` then hard-failed in two independent places: (1) `polars-arrow-0.53.0/src/bitmap/bitmask.rs:2` — `use std::simd::{LaneCount, SupportedLaneCount, …}` against current nightly (`nightly-2026-05-08`) reports "no `LaneCount` in `simd`" because the upstream `std::simd` API has moved post-0.53.0-release; (2) `polars-ops-0.53.0/src/chunked_array/strings/case.rs:79` — `use core::unicode::{Case_Ignorable, Cased}` reports "no `Cased` in `unicode`" and "function `Case_Ignorable` is private" against the same nightly.  Both code paths are gated by `polars/nightly`, but `polars/nightly` is also pulled transitively through `polars-stream`/`polars-lazy`/`polars-expr`/`polars-plan` even when the top-level `nightly` feature is disabled in our config.  Conclusion: the in-workspace polars `git/rev` pin (`1e9a63b9...`) was NOT opportunistic.  It carries upstream nightly-API patches that the published 0.53.0 release lacks, and dropping it breaks the build.  Path B-i abandoned. | The git-rev / published-version skew is fundamental: the same crate version (`0.53.0`) ships TWO different sets of source contents.  The git rev's `polars-arrow` declares `chrono ^0.4.42`; the published `polars-arrow 0.53.0` declares `chrono <=0.4.41` — no chrono version satisfies both.  The git rev is necessary for the workspace to build on current nightly, so it cannot be dropped.  An older nightly pin would also break unrelated workspace deps (Tokio/`std::simd`/`tracing` API drift over the same window), so a Path C ("regress nightly") was rejected without probing. | Executed option (a) of the original row.  Added `publish = false` to the 8 polars-tainted crates' Cargo.toml: `uffs-polars`, `uffs-mft`, `uffs-format`, `uffs-core`, `uffs-daemon`, `uffs-client`, `uffs-mcp`, `uffs-cli` (the user's table called out 6; the actual chain is 8 because `uffs-client` inherits polars via `uffs-format → uffs-mft → uffs-polars`, and `uffs-mcp` via `uffs-client → …`).  Replaced the corresponding `[[package]]` blocks in `release-plz.toml` from `changelog_path = "CHANGELOG.md"` to `release = false` so release-plz skips them entirely (no version bump computation, no `cargo package` step).  The 4 polars-free crates (`uffs-time`, `uffs-text`, `uffs-security`, `uffs-broker`) remain release-eligible with their original `changelog_path` entries.  Retired the `just polars` recipe (PR-internal — replaced with a deprecation stub in `just/test.just` that points users at `cargo update -p polars` plus a chrono-pin compat checklist) since bumping the rev now risks pulling in MORE nightly-API drift faster than upstream polars publishes patches.  Removed the `just polars` line from `just/help.just`. | R6 PARTIALLY RESOLVED — the publishability invariant is now scoped to 4 of 12 crates.  R7 (OIDC scaffolding) unaffected — the dormancy gate doesn't care which crates are publishable.  R8 dress rehearsal still feasible on its originally-chosen leaf target (`uffs-time` is polars-free).  R9 (full publish) DEFERRED until polars upstream publishes a release containing the nightly-API patches our `git/rev` carries (track via `crates-io-dry-run.yml` weekly).  When that release ships: (1) flip the 8 × `publish = false` to unset (or remove the line), (2) flip the 8 × `release = false` back to `changelog_path = "CHANGELOG.md"` in `release-plz.toml`, (3) drop the `git/rev` pin in `uffs-polars/Cargo.toml` in favor of the new published version, (4) re-evaluate the workspace `chrono = "=0.4.41"` pin (likely loosen if polars-arrow relaxes its upper bound), (5) restore the `just polars` recipe (or replace with a `just bump-polars` that takes a version arg).  `crates-io-dry-run.yml`'s ADVISORY mode comments at lines 19-39 and 245-251 reference the original deviation by old framing — those will be refreshed in the same future PR that re-enables the polars subtree. |

## 9. Cross-references

This plan is a sibling to
[`dev-flow-implementation-plan.md`](dev-flow-implementation-plan.md);
both depend on and complement each other.

### 9.1 What this plan owns

- Conventional commits policy
- Version bumping mechanism (release-plz)
- Changelog generation (git-cliff)
- Release PR process
- crates.io scaffolding and eventual publishing
- Retirement of bespoke version tooling

### 9.2 What `dev-flow-implementation-plan.md` owns

- Per-PR CI gate composition (pr-fast.yml jobs)
- Preview artifact workflow
- Tier 1 vs Tier 2 split
- `ci-pipeline` / `just ship` pipeline driver
- Branch protection rulesets
- `gates.toml` machine-readable manifest (Phase 8, deferred)

### 9.3 Shared concerns (touched by both plans)

- `scripts/ci-pipeline/src/version.rs` is touched by R5 in this
  plan; its broader shape (check/lint/test pipeline) belongs to
  dev-flow.
- `release.yml` is referenced as "unchanged" in this plan; its
  continued correctness is validated by tier-2 and preview flows
  in dev-flow.
- Branch protection: after R5, the `auto-tag-release` workflow is
  deleted; any branch protection check referring to it must be
  removed in the same PR.  Coordinate via dev-flow's branch-
  protection ruleset tracking.

### 9.4 Reading order for new maintainers

1. `README.md` — what UFFS is
2. `CONTRIBUTING.md` — how to contribute (includes commit
   conventions from R1)
3. `dev-flow-implementation-plan.md` §1-§4 — the CI architecture
4. **This document §1-§3** — the release architecture
5. Either plan's §5 onward — phase-by-phase detail, only if
   executing the migration

## 10. Non-goals deep-dive

### 10.1 Why not `cargo-dist`?

`cargo-dist` (now `dist`) is a strong tool for projects starting
fresh.  It generates release.yml-equivalent workflows from
declarative config and handles binary builds, signing, installer
generation.  **Why decline for UFFS:**

1. `release.yml` already does everything cargo-dist would:
   5-target matrix build, SLSA provenance, sha256 manifests,
   GitHub Release upload.  ~780 lines of tested workflow.
2. cargo-dist's generated workflow would be ~1000 lines, much of
   which would mirror `release.yml`'s existing logic.
3. cargo-dist's install scripts (curl|sh) introduce a new
   distribution surface that UFFS doesn't currently support.
   Adding that is a product decision, not a tooling decision.
4. The `[workspace.metadata.dist]` block in `Cargo.toml` (removed
   in R0) was exploratory; adopting cargo-dist requires
   regenerating `release.yml` from scratch, which would lose
   hand-tuned behavior (specific winresource handling, cargo-xwin
   invocation, polars-specific flags).

**Re-evaluate if**: cargo-dist ships a migration tool that can
import existing `release.yml` workflows.  Until then, cargo-dist
adoption is a blank-slate rewrite of release infrastructure, which
doesn't pencil.

### 10.2 Why not non-Rust ecosystem tools?

- **semantic-release** (Node.js): cross-ecosystem, but doesn't
  understand Cargo workspaces, dependency DAG, or crates.io
  publish semantics.  Would require ~500 lines of shell glue
  to translate.
- **changesets** (originally for npm): requires every contributor
  to write a `.changeset/<random>.md` file per change — adds
  friction.  Doesn't compute versions from commits.
- **standard-version** / **release-it**: JS-centric, same DAG
  and workspace blind spots.

Rust-native tooling (release-plz + git-cliff) integrates with
Cargo workspace inheritance, `[workspace.dependencies]` version
updates, and the `cargo publish` DAG walk.  Using ecosystem-
native tools is a strict win for correctness and maintenance
cost.

### 10.3 Why not single-PR mega-migration?

Temptation: do R0-R5 in one big PR to minimize churn.  Decline
because:

1. Release infrastructure changes touch the critical path; one
   broken piece breaks all releases.  Small reversible PRs limit
   blast radius to a single phase.
2. Shadow-observation phases (R3) require real commits flowing
   through main to validate — impossible in a mega-PR.
3. Rollback granularity: if R4 breaks, reverting R4 should not
   also revert R0-R3.  Mega-PR rollback is all-or-nothing.
4. Review burden: each phase is ~30-400 lines; reviewable in a
   sitting.  A combined PR would be ~2000 lines of structural
   change, review-fatigue-prone.

## 11. FAQ

**Q: Why not just adopt release-plz in one shot and delete the old
tooling the same day?**
A: Because release-plz's behavior depends on the commit history and
`cliff.toml`.  Getting either wrong produces surprising versions.
Phase R3 (shadow mode) observes that release-plz would produce the
right output on real commits, without creating tags or PRs.  Same-
day cutover would trade observability for speed, which is the wrong
trade when the feedback loop is "did we ship the wrong version
number to users".

**Q: Do conventional commits mean every commit message has to be
perfect?**
A: No.  Only merge commit messages (the title of the squashed PR)
need to follow the convention.  Intermediate commits on feature
branches can be whatever the author prefers.  The squash-merge
pattern (which UFFS already uses for most PRs) means the PR title
becomes the merge commit subject — so "the PR title must follow
conventional commits" is the effective rule.

**Q: What if a PR genuinely doesn't fit any conventional commit
type?**
A: Use `chore:` as the catch-all.  `chore:` commits don't trigger
a release.  If a PR is "`chore: refactor the whole world" that
also fixes a user-visible bug, split the PR — one `chore:` for
the refactor, one `fix:` for the bug.

**Q: What about reverts and hotfixes — do they bump versions?**
A: `revert:` commits follow the same semantics as the reverted
commit.  Reverting a `feat:` bumps minor; reverting a `fix:` bumps
patch.  Hotfixes are just `fix:` commits on main.  Urgent hotfix
cadence (releases within hours) is supported because release-plz
opens a release PR on every push to main — merge it ASAP for a
fast release.

**Q: Can we skip release-plz and use git-cliff + a hand-rolled
version bump script?**
A: Yes, but you'd be re-implementing release-plz.  The value of
release-plz is the PR-review-based release cadence + tag creation
+ GitHub Release integration + dependency version walking, all in
one tool.  Git-cliff alone gets you 30% of the way; release-plz
adds the remaining 70%.

**Q: What happens to the existing hand-maintained `## [Unreleased]`
section of `CHANGELOG.md`?**
A: In R2 we document the current format; in R3 we start letting
git-cliff generate sections.  In R4 release-plz generates a new
`## [0.5.72]` (or similar) header before the `## [Unreleased]`
marker, and the `## [Unreleased]` block becomes vestigial.  R5
deletes the vestigial block.  All historical `## [X.Y.Z]` entries
stay — they're valuable.

**Q: What if we later decide to publish cargo-dist's release
format alongside our own?**
A: Re-evaluated in the release-automation plan v2 (post-R9).  The
current plan deliberately does not bind the project to one binary
distribution path.

## 12. Appendix A — Minimal `cliff.toml` reference

```toml
[changelog]
header = """
# Changelog

All notable changes to this project are documented in this file.
Format based on Keep a Changelog; versions follow SemVer.
"""
body = """
{% if version %}\
  ## [{{ version | trim_start_matches(pat="v") }}] - {{ timestamp | date(format="%Y-%m-%d") }}
{% else %}\
  ## [Unreleased]
{% endif %}\
{% for group, commits in commits | group_by(attribute="group") %}
  ### {{ group | upper_first }}
  {% for commit in commits %}
    - {{ commit.message | split(pat="\n") | first | upper_first | trim }}\
      {% if commit.breaking %} **BREAKING**{% endif %}
  {% endfor %}
{% endfor %}
"""
footer = ""
trim = true

[git]
conventional_commits = true
filter_unconventional = false
commit_parsers = [
    { message = "^feat",     group = "Added"   },
    { message = "^fix",      group = "Fixed"   },
    { message = "^perf",     group = "Performance" },
    { message = "^refactor", group = "Changed" },
    { message = "^docs",     group = "Docs"    },
    { message = "^test",     group = "Tests"   },
    { message = "^chore",    skip = true       },
    { message = "^ci",       skip = true       },
    { message = "^revert",   group = "Reverted" },
    { body    = ".*BREAKING", group = "Breaking" },
]
protect_breaking_commits = true
filter_commits = false
tag_pattern = "v[0-9]*"
skip_tags = "v0.1.0-beta.1"
ignore_tags = ""
topo_order = false
sort_commits = "oldest"
```

## 13. Appendix B — Minimal `release-plz.toml` reference

```toml
[workspace]
# All crates share the same version (today's state).  Flip to
# per-crate versioning (independent) post-R9 if needed.
dependencies_update = true

# Publishing gate: OFF at workspace level until R9.  Individual
# crates also have `publish = false` in their Cargo.toml.
publish = false

# Release artifacts (GitHub Release + tag) driven by release-plz.
# Actual binary upload still happens in release.yml (tag-triggered).
git_release_enable = true
git_tag_enable = true
git_tag_name = "v{{ version }}"

# Changelog generation via git-cliff (cliff.toml).
changelog_update = true
changelog_path = "CHANGELOG.md"

# PR title and body format.
pr_name = "chore(release): prepare v{{ version }}"
pr_body = """
Automated release prepared by release-plz.

Please review:
- Version bump correctness
- Changelog entries
- Cargo.lock refresh

Merging this PR triggers:
1. Tag creation (v{{ version }})
2. release.yml (binary builds + GitHub Release)
3. crates.io publish (post-R9 only; gated today)
"""

# Semver-checks integration (once cargo-semver-checks is in PR Fast CI)
# semver_check = true

[[package]]
name = "uffs-diag"
publish = false
release = false  # Never cut a release for this crate

[[package]]
name = "ci-pipeline"
publish = false
release = false
```

## 14. Execution order summary

Read-through reminder:

1. R0 (baseline cleanup) — ~30 LOC churn
2. R1a (advisory commitlint) — ~60 LOC, new workflow
3. R1b (mandatory commitlint) — 1-line change after observation window
4. R2 (cliff.toml) — ~80 LOC, new config
5. R3 (release-plz shadow) — ~80 LOC, new workflow + config
6. R4 (release-plz active) — ~30 LOC change (permissions + release job)
7. R5 (retire bespoke tooling) — ~1350 LOC deletion, ~30 LOC additions
8. R6 (crates.io metadata + dry-run CI) — ~400 LOC additions
9. R7 (OIDC scaffolding) — ~80 LOC additions
10. R8 (publish rehearsal) — ~10 LOC, 1 crate published
11. R9 (live publishing) — **deferred, separate decision**

Total migration churn: **~2100 LOC added, ~1350 LOC removed**,
spanning R0-R8.  Median PR size: ~100 LOC.  Max PR size: R5 (the
deletion), which is net -1320 LOC.
