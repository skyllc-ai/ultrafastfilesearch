<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.

UFFS - Ultra Fast File Search
-->

# Changelog

All notable changes to UFFS will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.74] - 2026-04-25

### Fixed
- **macOS arm64 release binaries SIGKILLed at launch** under macOS 26+
  (`SIGKILL (Code Signature Invalid)` / `namespace=CODESIGNING` /
  `"Taskgated Invalid Signature"`).  `[profile.release].strip = "symbols"`
  in `Cargo.toml` strips the Mach-O symbol table **after** the linker has
  emitted an ad-hoc (linker-signed) `CodeDirectory`, leaving the embedded
  hash inconsistent with the on-disk file.  macOS 26+'s hardened
  taskgated then refuses to launch the binary.  In v0.5.72 this hit
  `uffsmcp` and `uffsd` deterministically; `uffs` and `uffs_mft` survived
  by binary-layout chance — a fragile guarantee that wouldn't hold on
  the next rebuild.

  Fix: add a `Re-codesign macOS binaries (post-strip)` step to
  `release.yml` that re-stamps the ad-hoc signature with `codesign
  --force --sign -` on every shipping `apple-darwin` binary after
  `cargo build --release` finishes.  The step is gated on
  `contains(matrix.target, 'apple-darwin')` so Windows / Linux artifact
  paths are untouched.  Each re-signed binary is then verified with
  `codesign --verify --verbose=2` so a regression here fails the
  workflow loudly instead of shipping broken artifacts.

  Workaround for users still on a v0.5.72 download:

  ```bash
  codesign --force --sign - ~/bin/uffsmcp ~/bin/uffsd
  ```

  Re-signs in place; macOS picks up the refreshed `CodeDirectory` on the
  next exec and the binaries launch normally.

  No code changes; release-only fix.  Recommended upgrade for every Mac
  user on macOS 26+.

## [0.5.72] - 2026-04-25

### Changed
- **W2–W5: Windows MSVC clippy strict-gate cleanup** (40 commits across
  `uffs-mft`, `uffs-broker`, `uffs-daemon`, `uffs-cli`, `uffs-client`,
  `uffs-core`, `uffs-mcp`, `uffs-diag`).  Brings the workspace to
  clippy-clean on the Windows MSVC strict gate (`cargo xwin clippy
  --workspace --target x86_64-pc-windows-msvc --all-targets -- -D
  warnings`) without weakening any lints, while preserving the existing
  macOS host clippy contract.  Highlights:
  - **`uffs-mft`** — exhaustive `indexing_slicing` cleanup across all
    IOCP / parallel / sliding-window readers, the `to_index` /
    `to_index_parallel` pipelines, and `multi_volume.rs`; per-call-site
    fixes only (no module-level allows).  Adopted `&raw mut`/`&raw
    const` for FFI call sites (Win32 IOCP + USN), eliminated all
    `borrow_as_ptr` lints, moved `?` outside `unsafe` blocks, and
    converted `u32`↔`u64` LCN casts to explicit
    `cast_signed`/`cast_unsigned`.  Replaced `default_numeric_fallback`
    via explicit type annotations across reader / IO / stats paths.
    Renamed all single-character bindings (closures, match arms,
    pattern destructuring) to descriptive names.  Adopted
    `Duration::div_ceil`, `u64::cast_signed()`, and `if let Some(x) =
    &foo` over `if let Some(ref x) = foo`.  Added `# Errors` sections
    to every public `Result`-returning MFT reader / volume / USN API.
    Backticked common Win32/NTFS identifiers in doc comments.
  - **`uffs-daemon`** — refactored nine functions over the
    `cognitive_complexity` threshold without weakening the lint:
    `ensure_drives_loaded`, `run_ipc_server` (unix), `handle_search`,
    `refresh`, `load_single_mft_file`, `load_from_data_dir`,
    `run_aggregations` (also dropped from 9-arg to 4-arg via new
    `AggregationRequest` struct), `run_idle_timer`, and the
    215-line `run_daemon` (97/25 → ≤25, split into thirteen named
    helpers covering panic-hook install, lifecycle bootstrap, MFT
    file gathering, drive-list resolution, IPC + stats spawn, load
    task, zero-drive shutdown guard, and graceful shutdown).
    Added unit tests pinning the contracts of `infer_drive_letter`,
    `is_live_drive_marker`, and `drive_letter_matches`.
    `resolve_refresh_mft_source` no longer needs an `anyhow::Result`
    wrapper — non-Windows guard moved to the `spawn_blocking`
    closure where `?` propagates a real error.
  - **`uffs-broker`** — surgical clippy cleanup; `broker.rs` is now
    0 lints under the Windows strict gate.
  - **Eliminated all transient `#[expect]`s introduced by the
    refactor**: the only suppressions remaining in the daemon crate
    are pre-existing maintainer-approved ones (FFI safety, JSON-RPC
    float arithmetic for stats, unstable-`error_in_core`,
    unstable-`Duration::from_mins`, the `[diag]` block tagged for
    removal after the D: drive issue is resolved).

### Removed
- **Stale file-size-policy exceptions** for `crates/uffs-cli/src/main.rs`
  and `crates/uffs-core/src/search/sorting.rs` — both are back under
  the 800-LOC cap after the args-extraction and dataframe-convert
  splits respectively.

### Added
- **`crates/uffs-daemon/src/lifecycle.rs`** added to the file-size
  exception list (827 LOC) with a documented rationale: the
  `LifecycleManager` + `LifecycleHandle` + `run_idle_timer` state
  machine forms a single cohesive unit (active-connection guard,
  load-stall heartbeat, session-tier deadline extension); splitting
  fragments shutdown semantics across files.

### Changed
- **Close stale `ci-failure-tier-1` issue notifications**
  (2026-04-24 — GitHub issues #44 and #19).  Housekeeping
  follow-up to the Phase 4 cutover: both issues were auto-
  generated by the retired `ci.yml` "🧪 UFFS Tier 1 Nightly CI"
  workflow.  #44 was the `cargo vet check --locked` red on PR
  #43's first run (`pastey:0.2.2` and `rustls:0.23.39` missing
  `safe-to-deploy`), resolved in PR #45 (`780c1dbb1`) by adding
  `[[audits.pastey]]` and `[[audits.rustls]]` entries to
  `supply-chain/audits.toml`; PR #43 subsequently went green on
  its later commits and landed as `6a4d572e0`.  #19 was the
  shared-bucket Tier-1 failure issue on the pre-org-move fork
  (`githubrobbi/UltraFastFileSearch`) that accumulated 117
  auto-comments before the repo moved to the `skyllc-ai` org.
  Since PR #48 (`6f99b86aa`) deleted `.github/workflows/ci.yml`
  and the surviving `pr-fast.yml` / `tier-2.yml` / `release.yml`
  notify-failure jobs use distinct `ci-failure-pr-fast` /
  `ci-failure-tier-2` / `ci-failure-release` labels and query
  `listForRepo` by those per-workflow labels, no future workflow
  can append to or reopen the `ci-failure-tier-1`-labelled
  issues.  Both closed as `completed` with comments linking the
  fix PRs.
- **Phase 4 CI cutover — `ci.yml` retired, `pr-fast.yml` is now
  the sole required lane** (2026-04-23 —
  `.github/workflows/ci.yml` deleted,
  `docs/architecture/dev-flow-implementation-plan.md` §4
  branch-protection checklist).  Completes the shift-left rollout
  scoped by the dev-flow implementation plan:
  - **Before**: two parallel CI lanes.  `ci.yml` (legacy Tier 1)
    with 6 required `Tier 1 / *` checks — Format, Clippy, Rustdoc,
    Security, File Size Policy, and the tests matrix — ran on
    every push to `main` / `develop` and on every PR to `main`
    regardless of what files changed.  `pr-fast.yml`
    (bucket-ordered PR-fast) was added in PR #45 and ran in
    parallel with `ci.yml` to validate equivalence.
  - **After**: single required lane.  `pr-fast.yml` reports exactly
    one required status check — `PR Fast CI / required` — which
    aggregates the 8 classify-gated downstream jobs (fmt, sanity,
    clippy, docs, test-build, tests, security, windows-check) plus
    the unconditional `file-size` job via `success|skipped` logic.
    Docs-only / dep-only / pure-infra-only PRs skip the heavy jobs
    and still report green, saving ~5-7 min of runner time per
    non-code PR.  The classify-aggregation branch explicitly
    depends on the classify job's own result, so a classify
    failure flips `required` red even though every downstream job
    would otherwise be `skipped` (validated live on PR #45 via
    broken-classify simulation — classify=red 4s, 8 downstream
    skipped, `required`=red 4s).
  - **Branch-protection ruleset** (`main-protection`, ID
    `11889528`) updated in the same window via the rulesets API
    (classic `/branches/main/protection` is 404 on this repo):
    required-checks list goes from the 7-entry parallel-window
    shape (6 `Tier 1 / *` + `PR Fast CI / required`) to a single
    `PR Fast CI / required` entry.  The context string is the
    job's `name:` attribute (`PR Fast CI / required`), NOT the
    UI-displayed `<workflow> / <job>` concatenation — a gotcha
    first hit on 2026-04-23 and documented in §4.4 of the plan
    doc.
  - **Bake-in evidence**: PR #45 (mixed rust + dep + infra,
    code=true) ran the full PR-fast matrix alongside `ci.yml` and
    both stayed green; PR #46 (docs-only, code=false) exercised
    the classify skip branch — downstream skipped 8 jobs,
    `required` green in ~4 s, `ci.yml` path-filter correctly
    didn't fire; PR #47 (infra-only Phase 4b retrofit) ran every
    PR-fast gate and stayed green.  Combined with the live broken-
    classify simulation on PR #45 (required=failure propagated
    correctly through 8 skipped downstream jobs), all four
    classification paths (mixed-code, docs-only, infra-only, and
    the broken-classify failure mode) are validated.  Zero
    disagreements between `pr-fast.yml` and `ci.yml` observed.
    The dev-flow implementation plan §10.3 originally scheduled
    a 7-day parallel-window bake; the cutover was brought forward
    because the confidence budget was already exhausted by the
    same-day evidence above — continuing to run `ci.yml` on every
    PR would burn ~5-7 min of runner time per PR with no
    additional signal.  See plan §10.5 "Deviations from the plan
    v1" for the decision log.
  - **Follow-ups NOT in this commit**: (1) stale `ci.yml`-
    referencing comments in `pr-fast.yml`, `release.yml`,
    `dependabot-review.yml`, and `scripts/hooks/_lint_pre_push.sh`
    — tracked as a separate housekeeping PR so this cutover
    commit stays minimal and reviewable; (2) Phase 4b release.yml
    workflow-level permissions refactor (workflow-level
    `contents: write` → per-job grants on `create-github-release`
    only) — still deferred per the Phase 4b PR's scope note;
    (3) plan-doc §10.2 / §10.3 reconciliation (tick the Phase 4
    "cutover" checkbox, flip the dashboard status to ✅) — handled
    as a docs-only follow-up PR per the pattern PR #45 → PR #46.
  - **Rollback**: `git revert` this commit restores `ci.yml`
    verbatim (full history preserved; no squash-merge loss), AND
    the ruleset needs to be PUT back to the 7-entry shape.  The
    revert alone is not sufficient — the ruleset change is
    separate state.  See §4.3 of the plan doc for the exact
    reverse sequence.

### Security
- **CI / release supply-chain hardening batch** (2026-04-22 —
  `.github/workflows/*.yml`, `SECURITY.md`,
  `docs/architecture/security/supply-chain-posture.md`).  Closes
  the gaps + nits from the 2026-04-22 supply-chain review:
  - **Concurrency groups** on `ci.yml` and `release.yml`.  Tier 1
    now cancels superseded PR runs (but queues on `main` pushes so
    branch-protection required checks stay stable); `release.yml`
    queues instead of cancelling, so a half-signed release asset
    can never ship.
  - **`optimized-ci.yml` → `tier-2.yml`** rename for clarity; the
    filename now matches the workflow's advertised "Tier 2" identity.
  - **Tier 2 / Windows Compile Check** runs
    `cargo check --workspace --all-features --all-targets` natively
    on `windows-latest` weekly.  Previously Windows-only build
    regressions only surfaced 10-15 minutes into a `just ship`
    release build; the earlier Linux-hosted MSVC cross-check was
    removed because ubuntu has no MSVC linker.  Tier 2 summary +
    `notify-failure` are now wired to this job AND to the
    pre-existing `file-size-policy` job (which was dangling before
    this change, so a file-size-policy Tier 2 failure used to be
    silent).
  - **CycloneDX 1.5 SBOMs on every release** via `cargo-cyclonedx`,
    emitted as `sbom-<crate>.cdx.json` into `final-release/` BEFORE
    `CHECKSUMS.txt` is regenerated and BEFORE the SLSA attestation
    step, so the SBOMs are in the checksum manifest AND covered by
    the Sigstore OIDC attestation.  Verify with the same
    `gh attestation verify` flow that already exists for binaries.
  - **CodeQL (Rust SAST)** workflow
    (`.github/workflows/codeql.yml`) on every PR and weekly
    Tuesday 06:30 UTC baseline.  Pinned to
    `github/codeql-action` v4.35.2.  Uses `build-mode: none` (the
    only mode Rust currently supports) so the extractor parses
    source directly without a cargo build — run budget is ~5-10 min
    rather than the 15-25 min a compiled-extraction pipeline would
    need.  Rust is in CodeQL's public
    preview since CodeQL 2.22.1 (July 2025) — findings are
    informational until a clean baseline settles, so this is NOT
    yet a required branch-protection gate.
  - **Narrowly-scoped Dependabot auto-merge**
    (`.github/workflows/dependabot-auto-merge.yml`) — only
    `version-update:semver-patch` bumps with no active security
    advisory queue for auto-merge, and only once every required
    check is green (`cargo-deny`, `cargo vet check --locked`,
    clippy, tests, doc-tests, file-size policy).  Branch
    protection (signed commits, required reviews) is NOT
    bypassed — this just saves the "merge when green" clickwork.
    Minor / major / security-advisory bumps keep the existing
    manual-review flow.  Updates
    `docs/architecture/security/supply-chain-posture.md` +
    `SECURITY.md` to reflect the narrowed policy.
  - **Free-up-disk-space step on the clippy job** matching the
    other heavy Tier 1 jobs, so future dep-tree fan-out does not
    tip `--all-features` clippy past ubuntu-22.04's default disk
    budget.
  - **Per-workflow `notify-failure` labels**
    (`ci-failure-tier-1`, `ci-failure-tier-2`,
    `ci-failure-release`) so a release failure is never buried
    as a comment on a week-old Tier 2 flake issue.  Keeps the
    legacy `ci-failure` label as a secondary label for
    backwards-compatible issue queries.
  - **Updated threat-model + layered-defences tables** in
    `docs/architecture/security/supply-chain-posture.md` with
    rows for SBOM, SAST, Windows regression check, and the split
    between manual-review (minor/major) vs gated auto-merge
    (patch) on Dependabot PRs.

### Added
- **Brand identity pass** (chore, 2026-04-21) — publishing-grade brand
  and trademark layer:
  - `assets/brand/` with logos (ICO, ICNS, 7 hicolor PNG sizes),
    wordmark, hero mark, web assets (favicons, Apple / Android touch
    icons, Safari pinned-tab SVG, web manifest), and source SVGs —
    23 files, ~600 KB.
  - `LICENSES/LicenseRef-UFFS-Brand.txt` and a second `REUSE.toml`
    annotation block carving `assets/brand/**` out of the MPL-2.0
    default under `LicenseRef-UFFS-Brand`. Trademark and copyright
    stay cleanly separated and machine-readable for REUSE lint.
  - `TRADEMARK.md` at the repo root — canonical policy separating the
    UFFS name and logo from the MPL-2.0-licensed source, modeled on
    the Rust Foundation and CNCF trademark policies.
  - README hero banner, centered header + 5-badge row, new
    "License & Trademarks" section, and new "Maintainership &
    Commercial" section crediting [Sky, LLC](https://github.com/skyllc-ai)
    as the maintaining organization and outlining commercial UFFS
    frontends currently in development.
  - `CONTRIBUTING.md` gets a one-line contribution-agreement note
    covering MPL-2.0 and TRADEMARK.md, plus a Contact section so
    `TRADEMARK.md`'s "contact in CONTRIBUTING.md" pointer resolves.
  - **Windows binary icon + `app.manifest`** (Phase 2 —
    `crates/uffs-cli/build.rs`, `crates/uffs-cli/Cargo.toml`,
    `crates/uffs-cli/app.manifest`).  The existing MSVC `/DELAYLOAD`
    build-script block is now augmented with a `winresource` resource
    embed on the same MSVC gate: icon from
    `assets/brand/icons/uffs.ico`, plus `ProductName` /
    `FileDescription` / `CompanyName` / `LegalCopyright` /
    `OriginalFilename` version-info fields.  The manifest declares
    `asInvoker`, `PerMonitorV2` DPI awareness, and long-path support.
    Critical: the manifest stays `asInvoker` — elevation policy lives
    in `uffs_client::daemon_ctl::ElevationPolicy` (v0.5.36 refactor);
    a `requireAdministrator` manifest would pop UAC on every
    `uffs <pattern>` invocation and defeat that work.  `winresource`
    added as a build-dep (MSVC-only effect; compiles inertly on
    other targets).  New `cargo:rerun-if-changed=app.manifest` +
    `cargo:rerun-if-changed=../../assets/brand/icons/uffs.ico` so
    edits retrigger the resource embed.  Clippy lint gate satisfied
    with `#![allow(clippy::expect_used, reason = "…")]` scoped to the
    build script — runtime code stays panic-free.
  - **UFFS wordmark on user manual landing** (Phase 6 —
    `docs/user-manual/index.md`).  Centred `uffs-wordmark.png` at
    560 px above the H1 so the published docs carry the brand
    consistently with the README.
  - **macOS `.app` bundle layout** (Phase 3 —
    `packaging/macos/Info.plist.in`,
    `packaging/macos/bundle.sh`, `just/packaging.just`,
    `justfile`).  New `just dist-macos` recipe: builds the release
    binary and wraps it in `dist/UFFS.app` with the UFFS icns,
    `LSUIElement` CLI-mode plist, and `CFBundleIdentifier =
    com.skyllc.uffs`.  `Info.plist.in` carries a `@@VERSION@@`
    placeholder that `bundle.sh` sed-substitutes from
    `cargo pkgid -p uffs-cli`, so the bundle version can never drift
    from `Cargo.toml`.  Output goes to the gitignored `dist/` tree;
    packaging configuration lives under the tracked `packaging/`
    root (new top-level folder).  End-to-end verified on macOS:
    `dist/UFFS.app/Contents/MacOS/uffs --version` returns
    `uffs 0.5.71` with the plist version fields templated correctly.
  - **Linux `.desktop` + installer** (Phase 4 —
    `packaging/linux/uffs.desktop`,
    `packaging/linux/install.sh`, `just/packaging.just`).  New
    `just install-linux` recipe (wraps `sudo
    packaging/linux/install.sh`): builds the release binary,
    drops it at `$PREFIX/bin/uffs` (default `/usr/local`), installs
    the freedesktop entry under
    `$PREFIX/share/applications/uffs.desktop`, and lays out the
    full hicolor icon tree under
    `$PREFIX/share/icons/hicolor/{16..512}/apps/uffs.png`.  `install.sh`
    uses a portable `mkdir -p` + `install -m` helper (GNU `install
    -D` is a GNU-only extension; the helper also works with BSD
    `install` on macOS, which lets the script smoke-test from a mac
    dev box).  `gtk-update-icon-cache` and `update-desktop-database`
    run best-effort; absent tools fail silently.  End-to-end smoke-
    tested from macOS against `PREFIX=/tmp/uffs-linux-install-test`:
    9 files installed, binary runs, `.desktop` fields correct.
  - **`just/packaging.just`** — new module imported from the root
    `justfile` alongside the existing `build` / `bench_ci` /
    `analysis` modules.  Keeps packaging concerns isolated so
    `just --list` groups them and `build.just` stays focused on
    compilation rather than distribution.
  - **Release workflow bundles brand assets with every tag** (Phase 8
    — `.github/workflows/release.yml`).  New `Stage release bundle`
    step runs per matrix target after the existing binary build,
    staging binaries + `README` / `LICENSE` / `TRADEMARK.md` /
    `CHANGELOG.md` + platform-specific brand assets + packaging
    helpers into `release-staging/<artifact-name>/`, then zipping
    into `release-artifacts/<artifact-name>.zip` (7z on Windows,
    `zip` on macOS / Linux).  macOS additionally runs
    `packaging/macos/bundle.sh` so the ZIP ships a ready-to-run
    `UFFS.app` — end users don't need to run the bundler themselves.
    Linux ZIP embeds the full `assets/brand/icons/hicolor/` tree and
    `packaging/linux/install.sh` so `sudo
    packaging/linux/install.sh` works from the unzipped directory
    with no extra downloads.  `Organize release assets` step updated
    to copy per-platform ZIPs into `final-release/` as-is (the
    platform suffix is already baked into
    `matrix.artifact-name`); raw-binary platform-suffix loop kept
    intact so existing automation that `wget`s a single binary keeps
    working.  `CHECKSUMS.txt` covers every asset (ZIPs + raw
    binaries).  Release notes rewritten to front the ZIP bundles as
    the recommended path with raw-binary URLs documented as the
    automation alternative.

- **Regex alternation → ExtensionIndex fast path** (Phase 4, 2026-04-21 —
  `crates/uffs-core/src/search/dispatch.rs`,
  `crates/uffs-client/src/protocol/cli_args_helpers.rs`,
  `crates/uffs-client/src/protocol/cli_args.rs`).  New
  `extract_extensions_from_regex` helper recognises the narrow regex
  shape `>^?(?i)?.*?\.(e1|e2|...)$` and rewrites it to
  `pattern="*" + extensions=[e1, e2, ...]` so the query routes through
  the same `ExtensionIndex` CSR fast path that `--ext e1,e2,e3` uses.
  Requires a trailing `$` anchor so the rewrite is semantically
  lossless (without `$` the regex matches `.ext` anywhere in the
  name, which the ext-index cannot replicate).  Rejects multi-segment
  extensions, wildcards, character classes, and literal-prefixed
  regex (so path-anchored forms stay on the regex scan path).
  Added as dispatch-time safety net #3 in `apply_dispatch_safety_nets`
  and as parse-time sugar in `RawCliArgs::into_search_params`.
  **Projected**: `>.*\.(jpg|png|heic)$` on a 3.5 M-record C: drive
  drops from 298 ms → ~95 ms (matches the equivalent
  `--ext jpg,png,heic` glob path).  **28 new regression tests** pin
  the accepted / rejected shapes across both layers.

### Changed
- **`--sort path_only` parallelised on the ext-index fast path**
  (Phase 4, 2026-04-21 —
  `crates/uffs-core/src/search/query/path_only_top_n.rs`,
  `crates/uffs-core/src/search/sorting.rs`).  Three targeted changes
  in the daemon's path_only pipeline:
  - **`collect_path_only_via_ext_index`** — per-candidate path
    resolution rewritten from a single-threaded `for` loop to
    `par_chunks(4096)` with per-worker `DirCache`, mirroring the
    pattern already used in
    `numeric_top_n::collect_global_top_n_numeric`.  Includes an
    explicit `(drive_idx, rec_idx)` locality re-sort upfront so
    multi-extension queries (e.g.
    `>.*\.(jpg|png|heic)$`) preserve MFT-adjacent DirCache hits.
  - **`sort_rows_with_fold`** — the Schwartzian decorate pass
    (`String`-alloc-per-row for each needed folded key) now runs on
    `into_par_iter` with a per-worker `fold_buf`, and the resulting
    sort uses `par_sort_unstable_by` when `rows.len() >= 16_384`
    (same threshold as the numeric fast path).
  - **`PhaseTimings` instrumentation** — the path_only fast path
    now populates `scan_ms`, `sort_ms`, `path_resolve_ms`,
    `path_candidates`, and `path_cache_entries`, so `--profile` no
    longer reports `scan=0 sort=0 path_resolve=0` on
    `--sort path_only` queries.  `collect_path_only_sorted_top_n`
    now returns `(Vec<DisplayRow>, Option<PhaseTimings>)` — the
    tree-walk branch still returns `None` because its single
    traversal interleaves every phase.

  **Projected**: `*.dll --sort path_only` on a 167 K-row C: drive
  drops from 221 ms → ~60 ms daemon-side (closes the 172 ms gap
  vs the default Modified sort observed during the v0.5.62 validation run; full capture in
  [`docs/benchmarks/raw/2026-04-v0.5.62_aggregate-baseline.txt`](docs/benchmarks/raw/2026-04-v0.5.62_aggregate-baseline.txt) and related internal logs).

### Fixed
- **`ext_rare` 543 ms outlier on drives with zero matching extensions**
  (Run 12, 2026-04-21 — `crates/uffs-core/src/search/query/numeric_top_n.rs`,
  `crates/uffs-core/src/search/filters/mod.rs`,
  `crates/uffs-core/src/search/filters/apply.rs`,
  `crates/uffs-core/src/search/sorting.rs`).  Two compounding bugs
  in the `*.<extension>` pipeline:
  - **Bug A (perf)** — `numeric_top_n::search_index` fell through to
    a full-drive scan when `resolve_ext_ids_for_drive` produced an
    empty ID set.  On a 3.5 M-record drive with zero `.dbt` files,
    `C:*.dbt --hide-system --hide-ads` cost 543 ms of pure scan
    plus a spurious row from Bug B.  **Fixed** by adding an explicit
    short-circuit arm that skips the drive entirely when the
    resolved-ID set is empty.
  - **Bug B (correctness)** — the `matches_record` /
    `row_passes_filters` fallback extracted extensions via
    `name.rsplit('.').next().unwrap_or("")`, which returns the
    whole name for dotless inputs.  A directory literally named
    `dbt` therefore matched `--ext dbt` even though the MFT
    indexer's `intern_extension` had already assigned it
    `extension_id = 0` (no extension bucket).  **Fixed** by adding
    a shared `extract_extension_after_dot` helper that matches
    `intern_extension` semantics exactly (dotless, dotfile, and
    trailing-dot names all return `""`), and replacing the buggy
    extraction in `matches_record`, `row_passes_filters`, and the
    `search::sorting` sort-key builder.  The sort-key fix closes
    a latent data-leak where dotless names leaked between
    extension groups on extension-sorted result sets.
  - **11 new regression tests** pin the fixes:
    `search::filters::tests::extract_extension_after_dot_*` (5 —
    helper semantics), `filter_extension_fallback_*` (4 —
    end-to-end `matches_record` fallback), and
    `search::backend::tests::search_index_ext_rare_*` (2 —
    end-to-end `*.dbt` on a drive with zero `.dbt` files).
- **`--profile` per-drive match counts rewritten O(rows×drives) →
  O(rows)** (`crates/uffs-daemon/src/index/search.rs:282-310`).
  The previous implementation nested `filter(|row| row.drive == D)
  .count()` inside a per-drive loop, producing quadratic work in
  the result cross product.  Single-pass `HashMap<char, usize>` tally
  then projects back over `drive_info` to preserve the existing
  (drive, count) ordering contract.  Cuts `--profile` overhead on
  wide result sets (e.g. 100 K rows × 4 drives) from ~400 K
  predicate evaluations to ~100 K hash inserts.

### Changed
- **`scripts/windows/cross-tool-benchmark.rs` no longer hard-codes
  `--profile`** in the default UFFS invocation (Run 12, 2026-04-21).
  The bench now measures the exact command shape a normal user
  types; `daemon_ms` is still captured on an opt-in basis via
  `UFFS_EXTRA_ARGS="--profile"` (environment variable).  Previous
  runs paid <0.2% overhead from `--profile`, so summary numbers
  remain comparable — change is primarily methodological
  cleanliness for public-facing benchmarks.

### Added
- **Phase 3 — `--columns parity` / `--parity-compat` and `--format custom`
  now take the daemon pre-format fast path**
  (`crates/uffs-daemon/src/handler.rs::RequestHandler::try_pack_csv_blob`).
  Both exclusions from Phase 2 are lifted; the daemon now produces the
  full 25-column legacy parity layout and the `Drives? … / MMMmmm …`
  drive footer server-side, leaving the CLI a pure `write_all` on
  the received blob.  Specifically:
  - `--columns parity` and `--parity-compat` both route through
    `uffs_format::write_rows` with `parity_compat=true` — the new
    behaviour that `build_output_config` auto-promotes `columns ==
    "parity"` into `parity_compat = true` keeps the CLI's
    `write_parity` (always rewrites dir rows) and the daemon's
    `write_rows` (rewrites only when flag is set) emitting
    byte-identical output even for `--columns parity` queries that
    omit `--parity-compat`.
  - `--format custom` accepts the CSV body through the shared
    writer, then appends the legacy footer via
    `uffs_format::write_legacy_drive_footer`.  The drive letters
    come from the new `SearchParams::output_drive_targets` wire
    field; empty targets skip the footer entirely, matching the
    CLI's baseline behaviour.
  - Parity always emits the 25-column header even when
    `output_header=false`: the daemon explicitly overrides
    `cfg.header=true` when `parity_compat` is active so the CLI's
    hand-rolled `write_parity` (which ignores the header flag) and
    the daemon fast path stay byte-identical on
    `--parity-compat --noheader` queries.
- **`uffs-format::footer` module — canonical legacy drive footer writer**
  (`crates/uffs-format/src/footer.rs`).  Carves the
  `write_legacy_drive_footer` + `DriveFooterContext` +
  `is_full_scan_pattern` helpers out of the CLI-private `parity.rs`
  into the shared crate so the CLI slow path
  (`write_native_results("custom", …)`) and the daemon fast path
  (`try_pack_csv_blob` with `output_format == "custom"`) share a
  single implementation.  Includes a self-test suite
  (`uffs_format::footer::tests::*`) that pins the CRLF shape, the
  `"MMMmmm that was FAST"` heuristic, the row-count threshold
  (`FAST_SCAN_ROW_LIMIT = 20 000`), the pipe-joined drive-letter
  formatting, and the full-scan pattern classifier.  Re-exported
  from `uffs-client::output` so the CLI preserves its thin-client
  invariant of depending only on `uffs-client`.
- **`SearchParams::output_drive_targets` wire field**
  (`crates/uffs-client/src/protocol/mod.rs`).  Carries the CLI's
  local `targets: Vec<char>` computation (from `--drive`,
  `--drives`, and the thin-client passthrough `--mft-file` path) to
  the daemon so `try_pack_csv_blob` can reproduce the footer
  exactly.  Intentionally separate from `SearchParams::drives`
  because "drives to search" and "drives to show in footer" are
  semantically distinct — e.g. `--mft-file D.mft` targets D for the
  footer but leaves `drives` empty.  Absent / empty → footer
  omitted (matches `uffs_format::write_legacy_drive_footer`'s
  empty-targets short-circuit).
- **CLI `write_columnar` now emits canonical byte-parity output**
  (`crates/uffs-cli/src/commands/output/mod.rs`).  The slow path
  that runs when the daemon returns `InlineRows` has been aligned
  with `uffs_format::write_rows` in three places so the CLI
  fallback and the daemon fast path cannot drift:
  - **Quote policy:** only string-shaped columns (`Path` / `Name` /
    `PathOnly` / `Type` / `Extension`) get quote-wrapped; numeric,
    datetime, and boolean-flag columns emit raw.  Matches the match
    arms in `uffs_format::writer::write_row` — the new helper
    `is_quoted_column` is the single authority both sites check.
  - **Timezone:** `extract_field` now takes a `tz_offset_secs`
    parameter fed from the parity context, and
    `format_filetime_with_tz` mirrors
    `uffs_format::append_datetime_native` exactly.  The older
    `format_filetime_local` is retained for the `--format table`
    human-display path (intentionally host-local for that surface).
  - **Header terminator:** the header row is now closed with
    `\n\n` (header + blank separator line) instead of a single
    `\n`, matching `uffs_format::write_rows` and the legacy
    baseline that `uffs-core::output::tests::format_parity_*`
    already pin.
- **Datetime zero-sentinel alignment across CLI and daemon**
  (`crates/uffs-cli/src/commands/output/{mod.rs,parity.rs}`).  Both
  `format_filetime_local` and `append_datetime_tz` now emit
  `"0000-00-00 00:00:00"` on an unset FILETIME (zero ticks, for
  which `uffs_time::filetime_to_calendar` returns `None`).  The
  previous empty-string behaviour diverged from
  `uffs_format::append_datetime_native` and silently produced
  different bytes between the CLI slow path and the daemon fast
  path on rows with zero Created/Modified/Accessed values — a
  latent Phase 2 inconsistency.
- **Six new byte-parity regression tests across CLI writers**
  (`crates/uffs-cli/src/commands/output/output_tests.rs`).  Pin
  every axis the Phase 3 lift depends on:
  - `parity_byte_parity_basic_file_zero_filetime` — datetime
    sentinel agreement.
  - `parity_byte_parity_directory_rewrite` — Path / Name /
    `PathOnly` / Size / `SizeOnDisk` parity-dir rewrite for
    directory rows.
  - `parity_byte_parity_all_flag_bits` — 15-column flag dispatch
    and `ParityAttributes` final column agree for every
    `PARITY_MASK` bit.
  - `parity_byte_parity_multi_row` — row ordering and header /
    blank-separator structure.
  - `columnar_byte_parity_zero_filetime_date_columns` +
    `columnar_byte_parity_nonzero_filetime` — pins the
    `write_columnar` ↔ `uffs_format::write_rows` alignment
    (quote policy, TZ, `\n\n` header) end-to-end.
- **Six new daemon regression tests for the Phase 3 gate lift**
  (`crates/uffs-daemon/src/handler.rs::tests`).  Replace the
  Phase 2 `skips_columns_parity` / `skips_parity_compat_flag`
  tests (which pinned the old exclusions) with positive-assertion
  coverage of the new behaviour:
  - `accepts_columns_parity` — `--columns parity` lands on
    `InlineBlob`, header matches the canonical 25-column legacy
    layout + `\n\n`, and the sample directory row gets the
    parity-dir rewrite (`\"C:\\\\Program Files\\\\app\\\\\",\"\",`).
  - `accepts_parity_compat_flag` — `--parity-compat` on a
    non-parity projection still rewrites dir rows (Path gets
    trailing `\`, Size swapped to `treesize`).
  - `parity_forces_header_when_disabled` — parity overrides
    `output_header=false` so the fast/slow paths agree on
    `--parity-compat --noheader`.
  - `custom_appends_footer_when_drives_set` — `--format custom`
    with non-empty `output_drive_targets` produces a blob whose
    tail contains the CRLF `Drives? \t1\tC:\r\n` footer and the
    `MMMmmm that was FAST` warning for a full-scan pattern under
    the row threshold.
  - `custom_omits_footer_when_no_drives` — empty
    `output_drive_targets` skips the footer entirely (matches the
    CLI's baseline).
  - `skips_non_csv_format` (updated) — `"json"` / `"table"` /
    `"CSV "` (trailing-space garbage) still skip; the old
    `"custom"` entry is removed because it is now accepted.
- **Daemon-side multi-column CSV pre-format fast path**
  (`crates/uffs-daemon/src/handler.rs::RequestHandler::try_pack_csv_blob`).
  Extends the existing path-only blob fast path (`try_pack_paths_blob`)
  to every multi-column CSV projection the daemon's formatter can
  reproduce byte-for-byte.  When the gate accepts the request, the
  handler consumes the inline `Vec<SearchRow>`, feeds it through
  `uffs_format::write_rows` with the same `OutputConfig` the
  `--out=file` path uses (via the newly `pub(crate)`
  `uffs_daemon::index::search::build_output_config`), and replaces
  `SearchResponse::payload` with `SearchPayload::InlineBlob` for
  payloads ≤ 512 KB or `SearchPayload::ShmemBlob` above that
  threshold.  The CLI then writes the buffer verbatim with a single
  `write_all`, skipping per-row JSON deserialisation, the
  client-side `extract_field` dispatch, and the `write_columnar`
  per-column render loop on the medium-to-large result sets where
  that dispatch dominates end-to-end latency.
- **New `SearchParams::output_format` wire field**
  (`crates/uffs-client/src/protocol/mod.rs`).  Carries the CLI's
  `--format` value (`"csv"`, `"json"`, `"custom"`, `"table"`) to
  the daemon so `try_pack_csv_blob` can gate correctly — the
  pre-format path only runs when the CLI will actually consume CSV
  output, and defers to the local formatter for JSON / table /
  `custom` (which appends a legacy drive footer the daemon does not
  emit).  Filled from `CliArgs::format` in `from_cli_args` and
  handled everywhere else by serde defaults — the field is optional
  and absent means "CLI default (csv)".
- **Nine new regression tests for `try_pack_csv_blob`**
  (`crates/uffs-daemon/src/handler.rs::tests::try_pack_csv_blob_*`).
  Mirror the path-only test layout:
  - **`happy_path_multi_column`** — pins the default CSV projection
    case (`output_format: None`, multi-column projection) lands on
    `InlineBlob` with the expected header + separator + row
    structure.
  - **`accepts_explicit_csv_format`** — `output_format =
    Some("csv")` in every case combination (lowercase, uppercase,
    mixed) is accepted.
  - **`skips_json_response_mode`**, **`skips_non_csv_format`**,
    **`skips_aggregations`**, **`skips_when_output_file_set`**,
    **`skips_columns_parity`**, **`skips_parity_compat_flag`**,
    **`skips_empty_response`** — each gate bullet in the method
    docstring has a dedicated test that keeps the payload as
    `InlineRows` instead of pre-formatting.
  - **`offloads_large_blob_to_shmem`** — 5 000-row fixture with
    padded paths produces a >512 KB blob, verifies the handler
    lands on `ShmemBlob`, streams the file back via
    `stream_paths_blob_into`, and compares the streamed bytes
    against a fresh in-memory `uffs_format::write_rows` reference
    call.  The file is deleted after the stream, mirroring the
    `try_pack_paths_blob` shmem test's lifecycle check.
- **`uffs-format` crate — unified CSV/columnar output formatter**
  (`crates/uffs-format/`).  Carves the shared CSV writer that both the
  daemon's `--out=file` path (`DisplayRow`) and the thin CLI's stdout
  path (`SearchRow`) now delegate to, so the two sites are byte-identical
  by construction rather than by accident.  The crate is polars-free,
  tokio-free, and depends only on `uffs-time` + `uffs-mft` + `itoa` +
  `rayon` + `serde` + the narrow `chrono` `clock` feature, preserving
  the thin-client binary-size invariant.  The public surface is
  `FormatRow` (trait abstracting over `DisplayRow` / `SearchRow`),
  `OutputConfig` (builder), `OutputColumn` (narrow enum mirroring the
  subset of `FieldId` the formatter needs), and `write_rows` (the
  entry point).  `uffs-client::output::write_search_rows` is a thin
  re-export used by CLI consumers that already depend on `uffs-client`.
- **Byte-parity regression tests for the formatter unification**
  (`crates/uffs-core/src/output/tests.rs::format_parity_*`).  Four
  tests — basic file row, parity-compat directory row, `--columns all`
  baseline, and 20 000-row parallel branch — pin that
  `uffs_format::write_rows(&[DisplayRow], …)` emits byte-identical
  output to the legacy `OutputConfig::write_display_rows(&[DisplayRow], …)`.
  Any future drift in either implementation trips at least one test
  before it reaches end-to-end parity suites.
- **`FieldId` ↔ `OutputColumn` drift-guard tests**
  (`crates/uffs-core/src/search/field/field_tests.rs::field_id_matches_output_column_*`).
  Three tests pin that every `FieldId` variant has a matching
  `uffs_format::OutputColumn` variant with identical `canonical_name`
  and `display_name`.  The `field_id_to_format_column` bridge in
  `uffs_core::output::display_rows_format_bridge` is an exhaustive
  `const fn` match, so variant-set drift trips at compile time; these
  tests cover the remaining metadata-drift surface at run time.
- **Phase 3 output-path optimization** (`docs/research/perf-phase3-output-optimization.md`)
  - **3.1 NUL fast path** — CLI detects `> NUL` / `> /dev/null` via the new
    `uffs_client::stdout_kind` module (Unix `fstat` + `/dev/null` device-id
    match; Windows `GetFileType` + `GetConsoleMode`) and auto-injects
    `--no-output`.  The daemon gates `SearchRow` materialisation on
    `include_rows`, so `paths_blob` packing, shmem offload, and IPC row
    transfer all no-op on suppressed queries.  Expected saving: 20–30 ms
    on medium result sets piped to NUL.
  - **3.2 Single-buffer multi-column console render** — the console branch
    of `write_native_results` now renders CSV / JSON / table / parity output
    into a `Vec<u8>` and issues one `stdout.lock().write_all`, replacing the
    previous `BufWriter<StdoutLock>` + per-row `writeln!` pattern.  Guarded
    by a 50 MiB cap via the pure `choose_console_strategy(row_count, cap,
    est)` helper — falls back to streaming on pathological result sets.
  - **3.3 Windows `WriteConsoleW` direct path** — when stdout is a real
    console on Windows, `uffs_client::stdout_kind::write_stdout_buffer`
    transcodes the rendered buffer to UTF-16 once and issues chunked
    `WriteConsoleW` calls, bypassing the narrow-CRT codepage translation
    that otherwise mangles non-ASCII output on legacy conhost.
- **Async `UffsClient` wire-protocol test coverage**
  (`crates/uffs-client/src/connect_tests.rs`) — six behavioural regression
  pins mirroring the sync suite (`status`-method contract,
  `ConnectionFailed` remediation text, `cached_status` short-circuit in
  both directions).  Drives the client through in-memory tokio
  `AsyncRead`/`AsyncWrite` doubles — no real socket, no daemon.

### Changed
- **Bulkiness sort-key eliminates per-candidate `DisplayRow` allocation**
  (`crates/uffs-core/src/search/query/numeric_top_n.rs`).  Added
  `bulkiness_for_record(&CompactRecord)` as a sibling of `bulkiness_for_row`;
  both forward to a shared private `bulkiness_from_sizes` so they cannot
  drift.  On the numeric top-N hot path this shaves an 18-line
  `DisplayRow::new(..., String::new(), ...)` dance — ~μs per candidate —
  measured impact ≈ 45 ms on a 45K-row `--sort bulkiness *.dll` query.

### Fixed
- **`shmem::tests` race** — `concurrent_writes_get_unique_paths` and
  `gc_cleans_orphaned_bins_and_preserves_non_bins` shared the global
  `shmem_dir()`; the GC test's `cleanup_stale_shmem_files()` sweep could
  wipe in-flight files written by the concurrent-writes test when cargo's
  threadpool scheduled both in parallel.  Serialised via a file-local
  `Mutex<()>`.  Production never hit this — GC only runs at daemon
  startup, and the PID file prevents overlap in real usage.
- **Two miswritten `#[expect(clippy::cognitive_complexity)]` reason strings**
  in `crates/uffs-daemon/src/index/mod.rs` had been copy-pasted from
  unrelated functions (`load_single_mft_file` tagged as "multi-drive
  search"; `ensure_drives_loaded` as "tree metrics computation").
  Replaced with accurate per-function justifications.

## [0.5.71] - 2026-04-19

### Added
- **Phase 2 performance measurement series** (closed): 11 instrumented
  runs comparing UFFS to Everything / UltraSearch / ES across cold-warm-hot
  phases.  Shipped `docs/research/perf-phase2-measurement-plan.md` as
  the permanent record.
- **`paths_blob` single-buffer fast path (v0.5.35)** — daemon packs
  path-only projections into a newline-terminated UTF-8 buffer; CLI
  writes with one `write_all`, skipping per-row JSON deserialisation.
  Inline for ≤ `SHMEM_THRESHOLD` rows; large results fall back to the
  shmem transport.
- **UAC refactor (v0.5.36)** — `ElevationPolicy::RequireExistingElevation`
  default, `--elevate` opt-in, `UFFS_ELEVATE=1` session override, plus an
  actionable error surface listing all three recovery paths (elevated
  shell, explicit UAC, broker install).
- **Deep health check (Run 10 Part B)** — `UffsClientSync` /
  `UffsClient` consolidate the connect-time liveness probe and
  pre-search readiness poll into a single `status` RPC, with a
  `cached_status` short-circuit in `await_ready`.  ~5–10 ms saved per
  CLI invocation on Windows named pipes.
- **Shared-memory transport for bulk results** (`uffs-client::shmem`) —
  results beyond `SHMEM_THRESHOLD` bypass JSON and memory-map a temp
  file.  Includes format v2 binary header, best-effort GC of stale
  `.bin` files on daemon startup.
- **Cross-tool benchmark harness**
  (`scripts/windows/cross-tool-benchmark.rs`) — drives UFFS, Everything,
  UltraSearch, and the legacy `uffs.com` C++ build through an
  apples-to-apples workload with cold/warm/hot phases and per-drive
  isolation.

### Changed
- **`cli_args.rs` refactored** — 11 stateless parsers extracted to
  `cli_args_helpers.rs`; `search.rs` tests moved to `search_tests.rs`
  via `#[path]` module re-attach.  Keeps both files under the 800-LOC
  file-size policy with no suppression.
- **Startup profiling** — `UFFS_PROFILE_STARTUP=1` prints per-phase
  wall-clock from `main()` through first `write_all`, driving the
  Phase 2 + Phase 3 measurement work.

### Fixed
- **`*.<ext>` and `<letter>:*` CLI sugar** — parse-time promotion to
  `pattern="*" + ext=<ext>` (and drive-prefix extraction) was briefly
  regressed during the fat→thin CLI split; restored plus a dispatch-time
  safety net in `uffs_core::search::backend::search_index` for direct
  JSON-RPC callers.
- **PathOnly sort** — now matches Windows Folder-column semantics
  (directories compare before files at equal path prefix; case-folded
  via the drive's upcase table).
- **Lifecycle / PID file** — stale-PID detection, `--no-retire` flag
  for long-running CI sessions, and session-tier upgrades (TUI / GUI /
  MCP at tier 1 get 3× the idle timeout of CLI at tier 0).

## [0.5.0] - 2026-03-15

Major architectural milestone — daemon-first CLI, MCP adapter, and
aggregate engine all ship together.

### Added
- **Aggregate engine** (`uffs_core::aggregate`) — Stages 0-5 complete:
  scaffolding + `AggregateMeta`; protocol + daemon + CLI integration;
  rollup, duplicates, parser, presets; pagination + CSV/TSV export;
  cache; `--agg` flag surface; MCP aggregation tools; 10-test
  validation suite (T119–T128).
- **MCP (Model Context Protocol) gateway** (`uffs-mcp`) — stdio adapter
  that bridges Claude, Cursor, Windsurf, and other AI agents to the
  daemon via JSON-RPC.  D3.4.5 notifications, D4.3 E2E tests, MCP
  resources + prompts.
- **Security hardening** — S1 cache DACL / file permissions, S2.2.2
  Windows DPAPI keystore, S4 daemon IPC hardening (peer credentials,
  input validation, limit caps), S4.3 client-side daemon identity
  verification (macOS codesign / Windows Authenticode), S4.4 rate
  limiting + idle timeout + shutdown nonce, S5 Access Broker hardening.
- **`uffs-broker`** — optional Windows service providing elevated MFT
  handles so the daemon itself can run `asInvoker` with no UAC prompt.
- **Scenario M** — incremental MFT hot-load validation scenario; exercises
  the daemon's `load_drive` + `info` + `refresh` paths against live
  drives.

### Changed
- **`--parity-compat` mode** — `CPP_COLUMN_ORDER` for exact C++-binary
  output shape, `parity_attributes()` mask for the 15 baseline NTFS
  flag bits.  Lets the Rust daemon drop into legacy automation with
  zero ini changes.

## [0.4.0] - 2026-02-12

Daemon-first architecture lands — CLI / TUI / GUI / MCP are now all
thin clients over a unified `uffsd` process.

### Added
- **Daemon foundation (D2)** — `IndexManager` holds the compact index +
  trigrams; `IpcServer` over Unix domain socket (macOS/Linux) or named
  pipe (Windows); RPC handler; lifecycle manager with idle auto-retire.
- **Client library (D3)** — `UffsClient` (async, tokio) and
  `UffsClientSync` (blocking, tokio-free) with auto-start, keepalive,
  reconnect, structured error types.
- **MCP adapter scaffolding (D4)** — stdio bridge, initial tool
  definitions, handler dispatch.
- **Windows Access Broker scaffold (D7)** — `uffs-broker` service,
  client, shared handle passing via Win32 named pipes; unblocks the
  "no UAC prompt for search" target posture.
- **Thin-client CLI / TUI / GUI** — `uffs`, `uffs_tui`, `uffs_gui` now
  delegate all heavy lifting to the daemon.  TUI drops from ~7 GiB
  peak RSS to < 50 MB.

## [0.3.0] - 2026-02-01

### Added
- **Compact index** — 72 bytes/record `CompactRecord` (`repr(C)`,
  `bytemuck::Pod/Zeroable`) replaces the full `MftIndex` after cache
  build.  ~72% memory reduction (7.5 GB → 2.1 GB for 25.9M records
  across 7 drives).
- **TUI** (`uffs_tui`) with ratatui — search box, paginated table,
  multi-tier sort (seven columns), file/dir/all filter, drive colour
  palette.  Wave 1 (trigram index, textarea, devicons) and Wave 2
  (table, sort, filter) complete.
- **Tree-based path search** — children index + segment decomposition
  for `C:\foo\bar`-style queries; glob matching with `*`, `?`, `**`.
- **On-demand full record lookup** — 25-column max view via seek+read
  from the `.uffs` cache, no need to keep full records in memory.
- **`.uffs` cache on macOS** — mirrors the Windows cache flow so MFT
  files captured on Windows can be searched on macOS.
- **Persistent search history** (`Ctrl+P` / `Ctrl+N`) — platform
  config dir, deduplicated, survives restarts.
- **Keymap system** — `~/.config/uffs/keys.toml`, embedded
  `PRESET_WINDOWS` and `PRESET_EMACS`, `--keys emacs` CLI override.

### Fixed
- **NTFS flags refactor** — `StandardInfo.flags` now stores raw
  `FILE_ATTRIBUTE_*` bits matching Windows semantics (`IS_READONLY=0x0001`,
  `IS_HIDDEN=0x0002`, etc.) instead of an internal remapping.  Cache
  format v9 (v8 auto-converts via `v8_flags_to_raw_ntfs()`).  Unblocks
  downstream parity work.

## [0.2.208] - 2026-01-27

### Added
- Baseline CI validation for modernization effort
- Windows cross-compilation for all binaries (uffs, uffs_mft, uffs_tui, uffs_gui)
- Modernization tracker and wave guides

### Changed
- Updated Polars to commit 8b99db82

## [0.2.114] - 2026-01-26

### Added
- Initial UFFS Rust implementation
- MFT reading and parsing with Polars DataFrames
- Path resolution during MFT digestion
- Hard link expansion (default on)
- Multi-drive parallel indexing support
- Cache architecture with zstd compression

### Fixed
- Various MFT parsing edge cases

[Unreleased]: https://github.com/skyllc-ai/UltraFastFileSearch/compare/v0.5.71...HEAD
[0.5.71]: https://github.com/skyllc-ai/UltraFastFileSearch/compare/v0.5.0...v0.5.71
[0.5.0]: https://github.com/skyllc-ai/UltraFastFileSearch/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/skyllc-ai/UltraFastFileSearch/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/skyllc-ai/UltraFastFileSearch/compare/v0.2.208...v0.3.0
[0.2.208]: https://github.com/skyllc-ai/UltraFastFileSearch/compare/v0.2.114...v0.2.208
[0.2.114]: https://github.com/skyllc-ai/UltraFastFileSearch/releases/tag/v0.2.114

