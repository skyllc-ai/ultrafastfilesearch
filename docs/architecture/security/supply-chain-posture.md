# Supply-Chain Posture

<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Copyright (c) 2025-2026 SKY, LLC. -->

Status as of **2026-04-22** · Maintainer: `@githubrobbi` · Review cadence: monthly

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
| Tag integrity | `main-protection` + `tag-protection-v-prefix` rulesets | Required reviews + signed commits on main; `v*` tag deletion/update blocked | Always | GitHub enforces |
| Build-provenance | `actions/attest-build-provenance` | Every release asset signed with Sigstore OIDC | Every `v*` release | **Yes** — release.yml |
| Commit ancestry check | Custom step in `release.yml` | `workflow_dispatch` `commit_sha` must be ancestor of main | Every release dispatch | **Yes** — release.yml |
| Dep-tree growth | `dependabot-review.yml` | Cargo.lock crate-count delta on Dependabot PRs | Every Dependabot PR | Annotation only |
| Structural audit | `cargo-geiger` via `just geiger` | unsafe / build.rs / proc-macro footprint | On-demand (monthly) | No |
| Human review | Dependabot PRs are NOT auto-merged | Every dependency bump | Every Dependabot PR | GitHub enforces |

## Threat model and coverage matrix

| Threat | Severity | Current control | Residual risk |
|---|---|---|---|
| Known CVE in a dep | High | `cargo-deny` RustSec DB on every PR | Low — CI gates |
| Unknown (zero-day) CVE | High | None specific; detection delayed | Medium — industry-wide |
| Typo-squatted dep added via PR | Medium | `deny.toml` `[sources]` allow-list + `unknown-git = warn` | Low |
| Maintainer-account compromise → silent minor-version malicious bump | **High** | Dependabot-manual-merge + `dependabot-review.yml` tree-growth annotation + human review of diffs | **Medium** — per-bump human judgment |
| Malicious `build.rs` / proc-macro executing in CI | **High** | Dependabot PRs run with **read-only `GITHUB_TOKEN`** + no repo secrets (GitHub default); `permissions:` block denies writes elsewhere | **Medium** — blast radius is bounded (runner has no sensitive tokens), but can still exfil public source / waste CI minutes |
| Release binary swapped on GitHub Release page | Medium | SHA256 `CHECKSUMS.txt` + SLSA build-provenance attestation via `gh attestation verify` | Low — requires attacker to also swap attestation, which is stored in GitHub Attestations API (separate audit trail) |
| Rollback attack (release an older vulnerable commit) | Medium | `commit_sha` ancestor-of-main guard in `release.yml` | Low |
| Rogue `v*` tag push by write-access user | Low | `tag-protection-v-prefix` ruleset blocks deletion + update | **Medium on creation** — GitHub API does not support `Integration` bypass for user-owned repos, so `creation` rule not enforced (bot couldn't push tags if it were).  Partial protection only. |
| Compromised runner (GitHub Actions infra itself) | Low | None — SLSA L2 attestation trusts the runner | Industry-wide; accept |

## Explicitly deferred (with rationale)

### cargo-vet initialisation

**Status**: Deferred to PR #33b (queued).

**What it does**: Requires every resolved crate-version to have an
audit record (either our own, or an imported one from Mozilla /
Google / Bytecode Alliance).  Dependabot PRs that bump to an
un-audited version fail CI until we audit the delta.

**Why deferred**: One-shot grandfather-in cost (~2 h) is low, but the
ongoing per-Dependabot-PR audit burden (~15-30 min/month at current
cadence of ~1-2 Dependabot PRs/month) needs a dedicated habit.  The
value compounds over time as the exemption list converts to real
audits via scheduled import-refresh from upstream.

**Promotion trigger**: When the project starts publishing
`uffs-core` as a library (others will depend on us transitively) or
when a second long-term contributor joins.

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
3. `gh pr diff <N>` and scan `Cargo.lock` for unexpected additions.
4. If the bump is a known crate and the diff looks clean, merge.
5. If anything looks off — typo-squat, sudden fan-out, unexplained
   git source change — ask the maintainer upstream before merging.

### Incident response (suspected compromise)

1. Disable auto-merge on all open PRs: `gh pr list --state open
   --json number --jq '.[].number' | xargs -I {} gh pr merge {}
   --disable-auto`.
2. Pin the suspected crate to its last-known-good version via
   `[patch.crates-io]` in workspace `Cargo.toml`.
3. Run `cargo audit` and `cargo deny check` locally for
   cross-confirmation.
4. Run `just geiger` and diff against the last monthly baseline —
   look for unexpected unsafe-count spikes in the suspected crate.
5. Rotate the repo's `GITHUB_TOKEN` (via GitHub settings) if any
   workflow had `write`-level scopes during the suspected window.

## References

- `deny.toml` — `cargo-deny` policy
- `.github/workflows/ci.yml` §Security — CI enforcement
- `.github/workflows/release.yml` — SLSA attestation + ancestor check
- `.github/workflows/dependabot-review.yml` — dep-tree growth annotation
- `.github/workflows/auto-tag-release.yml` — tagging bridge
- `just/analysis.just` — `just geiger` / `just geiger-summary` recipes
- GitHub [Attestations API](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)
- SLSA [build-provenance v1](https://slsa.dev/spec/v1.0/provenance)
