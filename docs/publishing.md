<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.

UFFS Publishing Runbook
-->

# UFFS Publishing Runbook

> **STATUS**: **BOOTSTRAP DONE — automated publishing still DORMANT.**
>
> R8 bootstrap complete (2026-06-10): `uffs-time` and `uffs-text`
> v0.5.120 were published to crates.io **once, by hand**, using a
> maintainer `CARGO_REGISTRY_TOKEN` (the chicken-and-egg below).
> Automated, ongoing publishing via the `release-plz.yml` OIDC job is
> still **dormant** and must not be activated until the **R9 go-live
> decision** is recorded in
> [`docs/architecture/release-automation-plan.md` §8 status dashboard][dashboard].
>
> **Chicken-and-egg (why the bootstrap is manual):** crates.io Trusted
> Publishing (OIDC) is configured per-crate on the crate's settings
> page, which only exists *after* the crate is published. There is no
> pending-publisher flow. So the **first** publish of each crate must
> use a token; OIDC is wired up afterwards and every subsequent publish
> is tokenless.
>
> [dashboard]: architecture/release-automation-plan.md#8-status-dashboard

## When do we publish?

**Never automatically.** Every publish is a maintainer decision made
in the release-PR review step. Release-plz opens a release PR; a
maintainer reviews the changelog, confirms the version bump, merges,
and at that point the OIDC publish job fires (from R9 onward).

The dormancy stack that keeps *automated* publishing off until R9:

1. **`ENABLE_CRATES_IO_PUBLISH` repo variable unset** — the
   `crates-io-publish` OIDC job in `release-plz.yml` is gated on
   `if: vars.ENABLE_CRATES_IO_PUBLISH == 'true'`, so it never runs
   until a maintainer deliberately sets the variable. (Replaces the
   former `if: false`, which actionlint rejected as a constant
   condition.)
2. **No trusted-publisher registration** on crates.io for the
   `crates.io-publish` environment — even if the job ran, the OIDC
   token mint would fail with no registered publisher to match.
3. **No `crates.io-publish` GitHub Environment** with reviewers — the
   `environment:` reference would not resolve to an approval gate.
4. **No long-lived `CARGO_REGISTRY_TOKEN`** secret in the repository
   — the bootstrap token lived only on the maintainer's machine and
   is revoked after R9 registration; CI never stores it.

All must be defeated independently to ship a crate via CI. There is no
single accidental flip that can leak.

## Phase status as of this document's last update

| Phase | What it adds | Status |
|---|---|---|
| R3   | Shadow-mode `release-plz update` workflow                     | ✅ landed |
| R3.5 | `version = ` requirements on internal & polars deps           | ✅ landed |
| R4   | Active-mode release-PR generator                              | ✅ landed |
| R5   | Retire bespoke `build/update_all_versions.rs` tooling         | ✅ landed |
| R6   | Per-crate metadata + dry-run CI workflow                      | ✅ landed |
| R6 step 6 | Crate-name reservations on crates.io                     | ✅ via bootstrap (2 crates) |
| R7   | OIDC trusted-publishing scaffolding (repo-variable gated, wired) | 🟡 wired, dormant |
| R8   | Bootstrap publish — `uffs-time` + `uffs-text` v0.5.120        | ✅ done (token, manual) |
| R9   | Live OIDC publishing for the publishable set                  | ⬜ pending |

See the canonical dashboard at
[`docs/architecture/release-automation-plan.md` §8][dashboard].

## Pre-publish checklist (one-time, per go-live decision)

These must all be ✅ before the R9 go-live PR opens:

- [x] Publishable crate names exist on crates.io under the project
      owner's account (`uffs-time`, `uffs-text` — published in the
      2026-06-10 bootstrap; R6 step 6 satisfied for the 2-crate set).
- [x] **Known-blocker resolution**: `uffs-polars` now uses plain
      `polars = "0.54.4"` from crates.io (git pin dropped), so the
      chrono clash is gone. Moot for the publishable set anyway —
      `uffs-polars` resolves to `publish = false`. (History:
      [release-automation-plan.md §6.1 risk #13][r6-known-blockers].)
- [ ] Trusted-publisher (OIDC) registrations complete for every
      publishable crate name (`uffs-time`, `uffs-text`) with
      environment `crates.io-publish`.
- [ ] `crates.io-publish` GitHub Environment exists with the
      required-reviewer rule active.
- [ ] Repo variable `ENABLE_CRATES_IO_PUBLISH = true` set (this is
      the live gate; replaces the former `if: false`).
- [ ] Bootstrap `CARGO_REGISTRY_TOKEN` revoked once OIDC verified.
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
      run logs; expect one successful publish step per publishable
      crate — currently 2: `uffs-time`, `uffs-text`).
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

The publishable set as of v0.5.120 is exactly **two** dependency-free
leaves (`cargo metadata --no-deps` is authoritative; all other members
resolve to `publish = false`):

```
1. uffs-time         (zero internal deps)
2. uffs-text         (zero internal deps)
```

Order is interchangeable — neither depends on the other. No other crate
can join the set without first flipping its never-publish internal deps
(`uffs-polars`/`uffs-security`/`uffs-format` → blocking `uffs-mft` →
blocking `uffs-client`/`uffs-mcp`/`uffs-cli`), a deliberate architecture
decision (see `docs/refactor/crates-io-publishability-deep-dive.md`).

Per-crate command:

```bash
cargo publish -p <crate> --token "$CARGO_REGISTRY_TOKEN"
```

After each `cargo publish`, wait ~30 sec before the next step so
the index has time to update — otherwise the next crate's
`cargo publish` may fail with "no matching package named `<dep>`
found".

## Trusted publishing (OIDC) configuration

The `crates-io-publish` job in `release-plz.yml` mints a short-lived
crates.io token via `rust-lang/crates-io-auth-action` and runs a
dependency-ordered `cargo publish` loop. To activate it (R9), register
a trusted publisher on crates.io for **each** publishable crate with
these exact form-field values:

| Field | Value |
|---|---|
| Repository owner | `skyllc-ai` |
| Repository name | `UltraFastFileSearch` |
| Workflow filename | `release-plz.yml` |
| Environment | `crates.io-publish` |

**The environment name MUST be `crates.io-publish`** — it has to match
the `environment:` value in `release-plz.yml` exactly, or the OIDC
token mint fails. (An earlier draft of the plan said
`crates-io-production`; that was a doc bug, corrected 2026-06-10.)

Then create the `crates.io-publish` GitHub Environment (Settings →
Environments) with a required-reviewer rule, and set the repo variable
`ENABLE_CRATES_IO_PUBLISH = true`.

- **Rotation**: if the workflow filename changes or the repo is
  renamed, ALL trusted-publisher registrations break and must be
  re-registered with the new values.
- **Revocation**: revoke the bootstrap `CARGO_REGISTRY_TOKEN` (crates.io
  → Account Settings → API Tokens) once OIDC is verified working; after
  that, no long-lived credential exists anywhere.

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
  — active release-PR generator + tag/release bridge + the dormant
  `crates-io-publish` OIDC job.
- [`.github/workflows/crates-io-dry-run.yml`](../.github/workflows/crates-io-dry-run.yml)
  — weekly metadata-drift detection job (R6 step 4).
- crates.io documentation:
  [trusted publishing][cratesio-tp] · [package metadata][cratesio-pkg]
- docs.rs documentation:
  [build configuration][docsrs-config]

[cratesio-tp]: https://crates.io/docs/trusted-publishing
[cratesio-pkg]: https://doc.rust-lang.org/cargo/reference/manifest.html
[docsrs-config]: https://docs.rs/about/builds#package-set-up
