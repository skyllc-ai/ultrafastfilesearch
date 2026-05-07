<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.

UFFS Publishing Runbook
-->

# UFFS Publishing Runbook

> **STATUS**: **DORMANT** — publishing is not yet live.
>
> Do not execute any of the steps in this runbook until the **R9 go-live
> decision** has been recorded in
> [`docs/architecture/release-automation-plan.md` §8 status dashboard][dashboard].
> Until then this document is a **forward-looking specification** of how
> UFFS will eventually ship to crates.io, captured here so the steps can
> be reviewed, audited, and refined while still safe to do so.
>
> [dashboard]: architecture/release-automation-plan.md#8-status-dashboard

## When do we publish?

**Never automatically.** Every publish is a maintainer decision made
in the release-PR review step. Release-plz opens a release PR; a
maintainer reviews the changelog, confirms the version bump, merges,
and at that point the OIDC publish job fires (from R9 onward).

The four-layer dormancy stack that protects us until R9 is live:

1. **`publish = false`** at the workspace level in `release-plz.toml`
   — release-plz never invokes `cargo publish` while this is `false`.
2. **`publish = false`** per-package in selected `Cargo.toml`s
   (`crates/uffs-diag`, `scripts/ci-pipeline`, `scripts/ci/gen-hooks`,
   `scripts/ci/gen-workflow`) — even a manual `cargo publish` from a
   developer machine refuses these.
3. **`if: false`** on the OIDC publish job (added in R7) — the
   workflow step never runs even when triggered.
4. **No `CARGO_REGISTRY_TOKEN`** secret in the repository — even a
   misconfigured workflow has no credential to authenticate with.

All four must be defeated independently to ship a crate. There is no
single accidental flip that can leak.

## Phase status as of this document's last update

| Phase | What it adds | Status |
|---|---|---|
| R3   | Shadow-mode `release-plz update` workflow                     | ✅ landed |
| R3.5 | `version = ` requirements on internal & polars deps (this PR) | 🟡 in progress |
| R4   | Active-mode release-PR generator                              | ⬜ pending |
| R5   | Retire bespoke `build/update_all_versions.rs` tooling         | ⬜ pending |
| R6   | Per-crate metadata + dry-run CI workflow (this PR's R6 work)  | 🟡 in progress |
| R6 step 6 | Crate-name reservations on crates.io                     | ⬜ deferred |
| R7   | OIDC trusted-publishing scaffolding (`if: false` gated)       | ⬜ pending |
| R8   | Dress rehearsal — publish `uffs-time` (foundation crate)      | ⬜ pending |
| R9   | Live publishing for the full publishable set                  | ⬜ pending |

See the canonical dashboard at
[`docs/architecture/release-automation-plan.md` §8][dashboard].

## Pre-publish checklist (one-time, per go-live decision)

These must all be ✅ before the R9 go-live PR opens:

- [ ] All publishable crate names reserved on crates.io under the
      project owner's account (R6 step 6, deferred).
- [ ] **Known-blocker resolution**: the `uffs-polars` git pin's
      published-form `polars = "0.53.0"` resolves cleanly against
      the workspace `chrono` pin OR `uffs-polars` is converted to
      `publish = false`. (Tracked in
      [release-automation-plan.md §6.1 risk #13][r6-known-blockers]
      and the `crates-io-dry-run.yml` workflow header comment.)
- [ ] Trusted-publisher (OIDC) registrations complete for every
      publishable crate name (R7).
- [ ] `crates-io-production` GitHub Environment exists with the
      required-reviewer rule active.
- [ ] `release-plz.yml` publish job has `if: true` (currently
      `if: false` per the four-layer dormancy stack above).
- [ ] `release-plz.toml` has `publish = true` at the workspace
      level (currently `publish = false`).
- [ ] First-release communication drafted (blog post, release
      notes, social-media announcement).

[r6-known-blockers]: architecture/release-automation-plan.md#61-risks

## Per-release checklist (every release, post-R9)

- [ ] Release-plz release PR opened against `main`.
- [ ] Changelog entries reviewed for accuracy against the actual
      commits since the previous tag.
- [ ] Version bump reviewed (feat → minor, fix → patch, feat! /
      `BREAKING CHANGE` → major — verify all crates bump
      consistently).
- [ ] Breaking changes called out in the changelog `Migration`
      section.
- [ ] Release PR merged on `main` → release-plz creates the tag
      → `release.yml` builds + uploads binaries.
- [ ] Binaries visible on the GitHub Release page (15 binaries,
      1 CHECKSUMS, 13 SBOMs — see `release.yml` for the asset
      manifest).
- [ ] Publish job succeeds for all eligible crates (check Actions
      run logs; expect 12 successful per-crate publish steps).
- [ ] Each published crate appears on crates.io within 60 sec
      (`cargo search uffs-time`, `uffs-text`, etc.).
- [ ] docs.rs builds succeed for all published crates within
      2 hours (look for green build badge on each crate's docs.rs
      landing page).
- [ ] Announcement posted (only required for major releases).

## Yank decisions log

Yanks are recorded here, latest first. A yank does **not** delete
the version from crates.io — it prevents new resolutions from
selecting it, but existing `Cargo.lock` files keep working.

| Date | Crate | Version | Rationale | Replacement |
|------|-------|---------|-----------|-------------|
| (none yet) |  |  |  |  |

If we ever need to yank, document the rationale here AND open a
GitHub issue with the `yank` label so downstream consumers see the
notice.

## Post-publish smoke checks

After every successful publish, verify:

- [ ] `cargo search <crate>` returns the new version (allow
      ~60 sec for crates.io index propagation).
- [ ] In a throwaway scratch directory:
      `cargo new test-pub-<crate> && cd test-pub-<crate> &&
      cargo add <crate> && cargo build` succeeds.
- [ ] crates.io crate page renders the README correctly (image
      paths, badge URLs, table layouts).
- [ ] docs.rs page renders without errors. The build log lives at
      `https://docs.rs/crate/<crate>/<version>/builds`. A failed
      build shows a red banner; for those, check the build log
      and adjust `[package.metadata.docs.rs]` in the next release.

## Manual fallback (release-plz unavailable)

If release-plz is broken / mis-configured / unreachable and a
release MUST ship, fall back to manual `cargo publish` in dependency
order. **Use this only as an emergency lever — the standard path is
release-plz.**

The publish order is the topological sort of the internal-dep DAG.
For UFFS as of v0.5.90:

```
1. uffs-time         (zero internal deps)
2. uffs-text         (zero internal deps)
3. uffs-security     (zero internal deps)
4. uffs-polars       (zero internal deps; external git pin)
5. uffs-mft          (deps: polars, text, security)
6. uffs-format       (deps: time, mft)
7. uffs-core         (deps: polars, text, time, mft, format, security)
8. uffs-client       (deps: security, format)
9. uffs-broker       (deps: security)
10. uffs-mcp         (deps: client)
11. uffs-daemon      (deps: security, mft, core, client)
12. uffs-cli         (deps: client, format, time)
```

Per-crate command:

```bash
cargo publish -p <crate> --token "$CARGO_REGISTRY_TOKEN"
```

After each `cargo publish`, wait ~30 sec before the next step so
the index has time to update — otherwise the next crate's
`cargo publish` may fail with "no matching package named `<dep>`
found".

## Trusted publishing (OIDC) configuration — to be filled in during R7

This section will document:

- Each crate's trusted-publisher form-field values on crates.io
  (repository, workflow filename, environment name).
- The `crates-io-production` GitHub Environment's required-reviewer
  list.
- Rotation procedure if the OIDC trust breaks (e.g., the workflow
  filename changes or the repository is renamed).
- Revocation procedure if a maintainer leaves or a credential
  is suspected compromised.

Until R7 lands, the section is intentionally empty.

## References

- [`docs/architecture/release-automation-plan.md`](architecture/release-automation-plan.md)
  — the canonical multi-phase migration plan (R0 → R9).
- [`docs/architecture/release-automation-baseline.md`](architecture/release-automation-baseline.md)
  — pre-migration baseline metrics + per-phase observations.
- [`release-plz.toml`](../release-plz.toml) — release-plz workspace
  configuration (per-package overrides documented inline).
- [`cliff.toml`](../cliff.toml) — git-cliff changelog template
  shared between `release-plz update` and `git cliff` developer
  iteration.
- [`.github/workflows/release-plz.yml`](../.github/workflows/release-plz.yml)
  — shadow-mode release-PR generator.
- [`.github/workflows/crates-io-dry-run.yml`](../.github/workflows/crates-io-dry-run.yml)
  — weekly metadata-drift detection job (R6 step 4).
- crates.io documentation:
  [trusted publishing][cratesio-tp] · [package metadata][cratesio-pkg]
- docs.rs documentation:
  [build configuration][docsrs-config]

[cratesio-tp]: https://crates.io/docs/trusted-publishing
[cratesio-pkg]: https://doc.rust-lang.org/cargo/reference/manifest.html
[docsrs-config]: https://docs.rs/about/builds#package-set-up
