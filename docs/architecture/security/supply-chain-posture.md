# Supply-Chain Posture

<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Copyright (c) 2025-2026 SKY, LLC. -->

Status as of **2026-04-22** · Maintainer: `@githubrobbi` · Review cadence: monthly

**Changelog**:
- 2026-04-22 — Initial baseline (PR #33a).  Added `cargo-geiger`
  on-demand, `dependabot-review.yml` dep-tree-growth annotation.
- 2026-04-22 — Added cargo-vet audit trail (PR #33b): 5 upstream
  imports (Mozilla, Google, Bytecode Alliance, ISRG, Zcash),
  `cargo vet check --locked` gate in CI, weekly
  `cargo-vet-refresh.yml` import-refresh workflow.
- 2026-04-22 — Committed `Cargo.lock` (PR #33b hotfix): switched
  from gitignored-lockfile to committed-lockfile for reproducibility,
  SLSA-attestation integrity, and cargo-vet stability.  See
  [Lockfile pinning](#lockfile-pinning-cargolock) below.
- 2026-04-22 — Documented known softprops old-commit 403 limitation
  in the release flow (PR #33c).  See [Known limitations](#known-limitations-in-the-release-flow) below.
- 2026-04-22 — Pipeline hardening batch:
  - Added concurrency groups to `ci.yml` and `release.yml` so
    superseded PR runs are cancelled and release dispatches queue
    cleanly.
  - Renamed `optimized-ci.yml` → `tier-2.yml` for clarity.
  - Added **Tier 2 / Windows Compile Check** on `windows-latest` so
    Windows regressions surface weekly, not 10-15 minutes into a
    `just ship` release run.
  - Added **CycloneDX 1.5 SBOMs** to every release: one
    `sbom-<crate>.cdx.json` per workspace crate, covered by the
    same SLSA build-provenance attestation as the binaries.
  - Added **CodeQL (Rust SAST)** workflow on PR + weekly
    schedule.  Rust support is in public preview (CodeQL ≥ 2.22.1)
    so the check is not a required gate yet; promoted after a
    few weeks of clean baselines.
  - Split `notify-failure` labels per workflow
    (`ci-failure-tier-1`, `ci-failure-tier-2`,
    `ci-failure-release`) so a release failure is never buried as a
    comment on a week-old Tier 2 flake issue.
  - **Narrow Dependabot auto-merge** (`dependabot-auto-merge.yml`)
    — patch-level bumps with no active security advisory queue for
    auto-merge once all required checks + branch-protection rules
    are satisfied.  Minor, major, and security-advisory bumps keep
    the existing manual-review path.  Net effect: reviewer time
    reclaimed for the bumps that actually carry review signal.

This document captures UFFS's current supply-chain defences, the threat
model they address, and the concrete gaps that are deferred (with
rationale).  It's a living reference — update it whenever a new
control lands or a deferred item is promoted.

---

## Layered defences in place

| Layer | Tool | Scope | Cadence | CI gate? |
|---|---|---|---|---|
| Known CVE detection | `cargo-deny` `[advisories]` | Full dep tree vs RustSec DB | Every PR | **Yes** — `ci.yml` Security job |
| License policy | `cargo-deny` `[licenses]` | Permitted list (MIT, Apache-2.0, MPL-2.0, …) | Every PR | **Yes** — same job |
| Source whitelist | `cargo-deny` `[sources]` | crates.io + pinned polars git | Every PR | **Yes** — same job |
| Workflow permissions | `permissions:` in every workflow | Minimal (`contents: read` default, `write` only where proven needed) | Reviewed per PR | N/A |
| Concurrency hygiene | `concurrency:` groups on every workflow | Cancel superseded PR runs; queue (never cancel) release / scheduled runs | Every workflow | N/A |
| Tag integrity | `main-protection` + `tag-protection-v-prefix` rulesets | Required reviews + signed commits on main; `v*` tag deletion/update blocked | Always | GitHub enforces |
| Build-provenance | `actions/attest-build-provenance` | Every release asset signed with Sigstore OIDC | Every `v*` release | **Yes** — release.yml |
| SBOM | `cargo-cyclonedx` → CycloneDX 1.5 JSON | One SBOM per workspace crate, shipped alongside binaries; covered by SLSA attestation | Every `v*` release | **Yes** — release.yml |
| Commit ancestry check | Custom step in `release.yml` | `workflow_dispatch` `commit_sha` must be ancestor of main | Every release dispatch | **Yes** — release.yml |
| Dep-tree growth | `dependabot-review.yml` | Cargo.lock crate-count delta on Dependabot PRs | Every Dependabot PR | Annotation only |
| Lockfile pinning | Committed `Cargo.lock` | Every resolved crate-version frozen across devs / CI / releases | Always | **Yes** — `cargo vet check --locked` would fail any drift |
| Audit trail | `cargo-vet check --locked` | Every resolved crate-version must have import / own audit / exemption | Every PR | **Yes** — `pr-fast.yml` security job |
| Import refresh | `cargo-vet-refresh.yml` | Weekly `cargo vet regenerate imports` → PR | Mondays 08:00 UTC | GitHub schedules |
| Structural audit | `cargo-geiger` via `just geiger` | unsafe / build.rs / proc-macro footprint | On-demand (monthly) | No |
| Semantic SAST | `codeql.yml` (Rust, public preview) | Dataflow-based bug patterns (path / SQL / regex injection, crypto misuse, unvalidated redirects) | PR + Tuesdays 06:30 UTC | Informational (not a required gate yet) |
| Windows regression check | `pr-fast.yml::windows-lint` job | `cargo clippy --workspace --all-targets --all-features --locked --no-deps -- -D warnings` on `windows-latest` | **Every PR** (was weekly Tier 2 pre-PR-#138; now PR-time) | GitHub-required-check |
| Human review for minor/major bumps | Dependabot PRs for minor + major + security advisories are NOT auto-merged | Minor / major / security-advisory bumps | Every Dependabot PR | GitHub enforces |
| Gated auto-merge for patch bumps | `dependabot-auto-merge.yml` | Only `version-update:semver-patch` bumps with NO active security advisory, gated on all required checks (cargo-deny, cargo-vet, clippy, tests, doc-tests, file-size policy) + branch-protection rules (signed commits, required reviews) | Every Dependabot PR | Gates enforced via required checks + branch protection |

## Threat model and coverage matrix

| Threat | Severity | Current control | Residual risk |
|---|---|---|---|
| Known CVE in a dep | High | `cargo-deny` RustSec DB on every PR | Low — CI gates |
| Unknown (zero-day) CVE | High | None specific; detection delayed | Medium — industry-wide |
| Typo-squatted dep added via PR | Medium | `deny.toml` `[sources]` allow-list + `unknown-git = warn` | Low |
| Maintainer-account compromise → silent minor-version malicious bump | **High** | `cargo-vet check` requires upstream or local audit for the new version; human review mandatory for minor+ / security-advisory bumps; `dependabot-review.yml` tree-growth annotation + human review of diffs.  Patch-level auto-merge is gated on cargo-vet + cargo-deny + the dep-tree-growth annotation, so a malicious patch still hits the same audit-trail wall as a manual-merge path | **Low-medium** — cargo-vet covers most crates via Mozilla/Google imports; residual is the first N days after a malicious version lands until upstream audits it (or we audit locally) |
| Malicious `build.rs` / proc-macro executing in CI | **High** | Dependabot PRs run with **read-only `GITHUB_TOKEN`** + no repo secrets (GitHub default); `permissions:` block denies writes elsewhere; `cargo-vet` imports from Mozilla/Google are the primary vetting signal for new build-script crates | **Medium** — blast radius bounded (runner has no sensitive tokens); new unaudited crate bumps are caught by `cargo vet check`, forcing a conscious decision |
| Release binary swapped on GitHub Release page | Medium | SHA256 `CHECKSUMS.txt` + SLSA build-provenance attestation via `gh attestation verify` | Low — requires attacker to also swap attestation, which is stored in GitHub Attestations API (separate audit trail) |
| SBOM swapped on GitHub Release page (misleading component inventory) | Medium | `sbom-*.cdx.json` files are covered by the same SLSA attestation as the binaries — `gh attestation verify` on the SBOM matches only if the bytes match this workflow run | Low — inherits the binary-swap residual |
| Windows-only build regression lands on main and only surfaces at release time | Low | `pr-fast.yml::windows-lint` job runs strict `cargo clippy -- -D warnings` natively on `windows-latest` on every PR (post-PR-#138 / Phase W5.5).  `cargo clippy` does a full type-check + executes every dep's `build.rs`, so any Windows-only build regression is caught at PR-time | Very low — minutes-scale detection latency, hard-gates merge to `main` |
| Semantic bug class (path / SQL / regex injection, crypto misuse) slipping past clippy | Medium | `codeql.yml` Rust SAST on every PR + weekly baseline | Medium — Rust support is in public preview; false-negative rate unknown |
| Rollback attack (release an older vulnerable commit) | Medium | `commit_sha` ancestor-of-main guard in `release.yml` | Low |
| Rogue `v*` tag push by write-access user | Low | `tag-protection-v-prefix` ruleset blocks deletion + update | **Medium on creation** — GitHub API does not support `Integration` bypass for user-owned repos, so `creation` rule not enforced (bot couldn't push tags if it were).  Partial protection only. |
| Compromised runner (GitHub Actions infra itself) | Low | None — SLSA L2 attestation trusts the runner | Industry-wide; accept |

## Explicitly deferred (with rationale)

### Reproducible builds + independent rebuild verifier

**Status**: Deferred indefinitely.

**What it would protect against**: Compromise of the GitHub-hosted
runner itself (the step the SLSA L2 attestation has to trust).

**Why deferred**: Requires pinning OS-image SHAs, `SOURCE_DATE_EPOCH`,
deterministic toolchain.  Worth ~3-5 days of work for L3.  The
threat is industry-wide and low-probability at UFFS's scale.

### Code signing (Authenticode / `codesign`)

**Status**: Deferred indefinitely.

**What it would protect against**: Users who rely on the OS's
SmartScreen / Gatekeeper reputation signal rather than
`gh attestation verify`.

**Why deferred**: $99-$200/yr cost + key-rotation discipline.  SLSA
attestation covers the technical threat for the audience who
verifies; deferring the UX-layer signing until there's enterprise
demand.

## Lockfile pinning (`Cargo.lock`)

As of 2026-04-22, `Cargo.lock` is committed to the repository
(previously in `.gitignore`).  This is the standard recommendation
for binary-crate workspaces per the [Cargo book](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html),
but for UFFS specifically it plays three supply-chain roles:

### Role 1 — Reproducible builds

Every developer clone, every CI run, every release build now resolves
the identical dep graph.  Before, a fresh CI runner without a
lockfile would `cargo generate-lockfile` on the spot, picking up
whatever versions crates.io had published in the meantime.  This
made the binary that got built from "commit X" non-deterministic,
which in turn weakened the SLSA attestation's "I built this
artifact from these sources" claim.

### Role 2 — `cargo-vet` stability

`cargo-vet` exemptions are keyed on specific `crate@version` pairs.
With a floating lockfile, any transitive dep getting a patch
release on crates.io (literally hundreds per week across our ~500
transitive deps) would fail CI's `cargo vet check` for reasons
unrelated to our PRs.  We hit this concretely on PR #33b when
`pastey 0.2.2` was published between `cargo vet init` and the
first CI run — the PR was blocked on an "unvetted" dep we hadn't
intentionally upgraded.  Committing the lockfile eliminates this
class of spurious failures; real bumps now only come via
deliberate `cargo update` / Dependabot PRs.

### Role 3 — Dependabot review surface

With the lockfile in-tree, every Dependabot PR visibly changes
`Cargo.lock` — the `dependabot-review.yml` workflow's tree-growth
annotation has a real artifact to compare against.  The ~500-line
`Cargo.lock` diff a bump produces is also a quick skim for
unexpected fan-out.

### Cost

- **Repo size**: one-time +300 KB (5512-line `Cargo.lock`).
- **Merge conflicts**: Dependabot PRs may conflict with each
  other on `Cargo.lock`.  Mitigation: merge Dependabot PRs one at
  a time (we already review them manually per
  [Operational playbook](#per-dependabot-pr-review-5-10-min),
  so the cost is zero).

## Known limitations in the release flow

### `softprops/action-gh-release` 403 on non-HEAD commits

**Symptom**: `Create GitHub Release` step fails with

```
GitHub release failed with status: 403
{"message":"Resource not accessible by integration"}
```

even though `permissions: contents: write` is set on the workflow.

**Root cause**: documented in
[softprops/action-gh-release README](https://github.com/softprops/action-gh-release):

> When creating a new tag for an older commit, `github.token` may
> not have permission to create the ref; use a PAT or another
> token with sufficient contents permissions if you hit
> `403 Resource not accessible by integration`.

This is a hardcoded restriction of the GitHub Apps token model:
`GITHUB_TOKEN` can create refs pointing at the default branch's
current HEAD, but not at arbitrary older commits.

**When this surfaces for UFFS**:

- Smoke tests dispatched against an old commit (e.g. we replay
  `release.yml` against a pre-fix commit to verify a fix works)
  — always fails.
- Real `just ship` flow where another commit lands on `main`
  during the ~10-15 min release build window — `commit_sha`
  stops matching `main` HEAD by the time softprops runs.
  Theoretically possible but rare at current solo-maintainer
  merge cadence (~1 PR/day).

**Mitigation**: accept and document.  If we ever hit the race in
practice, operator response is "re-dispatch `release.yml`, do not
merge during release window".  A PAT-based workaround would
re-introduce a supply-chain secret for a single edge-case — not
worth the trade.

**Separate from** the workflow-file-protection race that PR #32
fixed (which was `remote rejected: refusing to allow a GitHub App
to create or update workflow ... without 'workflows' permission`
at `git push origin v*` time).  That path is closed; softprops
now creates the tag via REST API, which has no workflow-file
guard.

## Baseline metrics

### cargo-geiger (2026-04-22, `uffs-cli` full feature set)

Aggregate unsafe footprint across the resolved dep tree:

```
Functions  Expressions  Impls  Traits  Methods
862/2072   73458/112403 1285/2290 106/112 2688/3519
```

Top-10 `unsafe`-heavy crates (by function count):

| Crate | Used/Total fns | Used/Total exprs | Role |
|---|---|---|---|
| `rustix 1.1.4` | 44/436 | 1560/7539 | syscall bindings |
| `objc2-core-foundation 0.3.2` | 66/185 | 1143/2829 | Apple platform bindings |
| `libc 0.2.185` | 1/92 | 10/725 | C library bindings |
| `portable-atomic 1.13.1` | 16/87 | 122/633 | atomics polyfill |
| `blake3 1.8.4` | 1/84 | 32/4365 | crypto (SIMD) |
| `sysinfo 0.37.2` | 12/73 | 1018/5802 | OS info |
| `polars-arrow 0.53.0` | 69/69 | 5226/5271 | columnar engine |
| `argminmax 0.6.3` | 60/60 | 2854/3391 | SIMD min/max |
| `zlib-rs 0.6.3` | 41/49 | 2594/3481 | compression (pure Rust) |
| `tokio 1.52.1` | 26/30 | 2270/2912 | async runtime |

**Interpretation**: every top-10 crate is either (a) platform bindings
that must use unsafe by definition, (b) SIMD-heavy performance code
in well-known widely-audited crates, or (c) foundational async/crypto
from the Rust ecosystem's top-tier maintainers.  **No surprises.**
None of the crates appearing here warrant a focused manual audit at
this time.

**How to refresh**: `just geiger` (writes dated report to
`target/geiger-YYYYMMDD.txt`).  Review monthly; flag any new top-20
entry that isn't obviously platform / SIMD / foundational.

### cargo-vet coverage (2026-04-22)

From `cargo vet check` immediately after init:

```
Vetting Succeeded (121 fully audited, 2 partially audited, 387 exempted)
```

**Interpretation**:

- **121 fully audited** — covered by an imported audit record from
  Mozilla / Google / Bytecode Alliance / ISRG / Zcash.  Zero
  maintenance on our side; refreshed weekly.
- **2 partially audited** — some criteria (`safe-to-deploy` vs
  `safe-to-run`) covered by imports, others exempted.
- **387 exempted** — grandfathered in at init time.  The weekly
  refresh workflow auto-prunes any exemption that becomes covered
  by a fresh upstream audit (numbers should trend down over
  months).

**Target trajectory**:

- 2026-04 (initial): 121 audited / 387 exempted (24% coverage)
- 2026-10 (6 months): aim for >40% coverage via upstream refreshes
- Long-term: >70% coverage via upstream; remaining exemptions are
  niche / rare-maintainer crates that are unlikely to be audited
  upstream.

**Ongoing cost**:

- Dependabot PR where the bump is covered by upstream: **0 min**
  (sails through CI).
- Dependabot PR where the bump isn't covered: **~2 min** to run
  `cargo vet regenerate exemptions && git commit --amend` locally
  and push.  Weekly refresh minimises how often this happens.
- Weekly refresh PR review: **~5 min** to skim the diff + merge.

### Cargo.lock crate count (2026-04-22)

<!-- Populate with actual count next time you run `grep -c '^name = ' Cargo.lock` -->
See the Dependabot-review workflow summary for the current value.  The
`dep-tree-growth` check compares each Dependabot PR against the value
on `main` at branch-open time.

## Operational playbook

### Monthly review (~30 min)

1. `just geiger` — regenerate report, diff against previous month's
   `target/geiger-YYYYMMDD.txt`.
2. Scan the top-20 for new entries; investigate any that aren't
   platform / SIMD / foundational.
3. Review any Dependabot PRs that are still open with
   `dep-tree-growth` warnings.
4. Glance at the GitHub Security tab for new Dependabot alerts.

### Per-Dependabot-PR review (~5-10 min)

1. Read the PR description (Dependabot summarises the changelog).
2. Check the `dep-tree-growth` annotation — if flagged, inspect the
   "Newly-resolved crate names" summary.
3. Check the `cargo vet check` CI status — if red, the bump isn't
   covered by any imported audit.  Fix locally:
   ```bash
   gh pr checkout <N>
   cargo vet regenerate imports     # try upstream first
   cargo vet check                  # still red?
   cargo vet regenerate exemptions  # grandfather in
   git commit -S -am "chore: refresh cargo-vet imports/exemptions"
   git push
   ```
4. `gh pr diff <N>` and scan `Cargo.lock` for unexpected additions.
5. If the bump is a known crate and the diff looks clean, merge.
6. If anything looks off — typo-squat, sudden fan-out, unexplained
   git source change — ask the maintainer upstream before merging.

### Incident response (suspected compromise)

1. Disable auto-merge on all open PRs: `gh pr list --state open
   --json number --jq '.[].number' | xargs -I {} gh pr merge {}
   --disable-auto`.
2. Pin the suspected crate to its last-known-good version via
   `[patch.crates-io]` in workspace `Cargo.toml`.
3. Record a `cargo-vet` violation for the compromised version so
   future runs fail loudly:
   ```bash
   cargo vet record-violation --criteria safe-to-deploy \
     <crate> <bad-version>
   ```
4. Run `cargo audit` and `cargo deny check` locally for
   cross-confirmation.
5. Run `just geiger` and diff against the last monthly baseline —
   look for unexpected unsafe-count spikes in the suspected crate.
6. Rotate the repo's `GITHUB_TOKEN` (via GitHub settings) if any
   workflow had `write`-level scopes during the suspected window.

## References

### In-repo

- `deny.toml` — `cargo-deny` policy
- `supply-chain/config.toml` — `cargo-vet` imports + exemptions
- `supply-chain/audits.toml` — our local audit records (starts empty)
- `supply-chain/imports.lock` — pinned upstream audit snapshots
- `.github/workflows/pr-fast.yml` — required per-PR gate (fmt, clippy, docs, tests, **windows-lint** [native `cargo clippy -- -D warnings` on `windows-latest` post-W5.5], `cargo-deny` + `cargo-vet check` in `security` job) (Tier 1)
- `.github/workflows/tier-2.yml` — weekly coverage / udeps / Miri (Windows compile check removed in PR #138 — superseded by `pr-fast.yml::windows-lint`'s per-PR strict clippy)
- `.github/workflows/release.yml` — SLSA attestation + ancestor check + CycloneDX SBOM
- `.github/workflows/codeql.yml` — Rust SAST (public preview)
- `.github/workflows/dependabot-review.yml` — dep-tree growth annotation
- `.github/workflows/dependabot-auto-merge.yml` — patch-level auto-merge gate
- `.github/workflows/cargo-vet-refresh.yml` — weekly imports refresh
- `.github/workflows/release-plz.yml` — release-PR generator + tag creator + `release.yml` dispatch bridge (post-R5; the pre-R5 `auto-tag-release.yml` shim was retired in `release-automation-plan.md` §R5)
- `just/analysis.just` — `just geiger` / `just geiger-summary` recipes

### External

- GitHub [Attestations API](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)
- SLSA [build-provenance v1](https://slsa.dev/spec/v1.0/provenance)
- cargo-vet [user guide](https://mozilla.github.io/cargo-vet/)
- Mozilla's [supply-chain audits](https://github.com/mozilla/supply-chain)
- Google's [supply-chain audits](https://github.com/google/supply-chain)
- CycloneDX [1.5 specification](https://cyclonedx.org/specification/overview/)
- [cargo-cyclonedx](https://github.com/CycloneDX/cyclonedx-rust-cargo)
- CodeQL [Rust public preview changelog](https://github.blog/changelog/2025-07-02-codeql-2-22-1-bring-rust-support-to-public-preview/)
