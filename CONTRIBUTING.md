# Contributing to UFFS

Thanks for helping improve UFFS.

> By contributing you agree your contribution is licensed under [MPL-2.0](LICENSE) and that the UFFS name and logo remain governed by [TRADEMARK.md](TRADEMARK.md).

## Contact

- **Bug reports, feature requests, questions:** open a GitHub issue on [skyllc-ai/UltraFastFileSearch](https://github.com/skyllc-ai/UltraFastFileSearch/issues).
- **Brand / trademark questions:** open an issue tagged `brand`, or email [`uffs@nios.net`](mailto:uffs@nios.net).
- **Commercial / partnership inquiries:** [`uffs@nios.net`](mailto:uffs@nios.net), or a [discussion](https://github.com/skyllc-ai/UltraFastFileSearch/discussions) with the `commercial-interest` label.
- **Organization:** [Sky, LLC](https://github.com/skyllc-ai).

## Platform and privilege model

- UFFS is Windows-first: live NTFS MFT reads require Windows and Administrator privileges.
- macOS and Linux remain valid contributor hosts for docs work, offline/query logic, and cross-platform tests.
- Keep Windows-only I/O behind `#[cfg(windows)]` and prefer cross-platform validation when possible.

## Toolchain and setup

- Use the pinned nightly toolchain from `rust-toolchain.toml`.
- The workspace MSRV is Rust 1.91, but day-to-day development should use the pinned nightly.
- `just` is the primary workflow entry point.

Recommended setup:

1. Install the nightly toolchain: `rustup toolchain install nightly`
2. Install `just`: `cargo install just`
3. Install the common contributor toolchain: `just setup`
4. List available workflows any time with `just`

For cross-compilation from macOS/Linux hosts:

- `just setup-cross` — install cross targets used by the workspace
- `just check-cross` — run the CI-style cross-compilation validation
- See `docs/xwin-msvc-rlib-size-root-cause-and-workarounds.md` for `cargo xwin` / MSVC-specific notes

## Preferred validation workflow

Prefer the smallest command that proves your change:

- `just check` — quick workspace validation (`cargo check`, formatting check, file-size policy)
- `just fmt` — format the workspace
- `just test` — workspace tests via nextest/llvm-cov
- `just test-doc` — documentation tests
- `just lint-prod` — strict production Clippy
- `just lint-tests` — test-target Clippy
- `just build` — workspace build
- `just go` — full fast-fail workflow when you want the whole pipeline

Focused examples from the current workflow canon:

- `cargo nextest run -p uffs-mft -- tree`
- `cargo test -p uffs-core -- path_resolver --nocapture`
- `cargo test -p uffs-mft --lib -- --nocapture`

Most tests run cross-platform. Tests that need live MFT access are typically `#[ignore]` and should only be run on Windows with elevation.

## Four-layer quality gates

UFFS uses a shift-left pipeline: cheap checks fire close to the keystroke, expensive ones move rightward into CI. Each layer is fully opt-in via `just install-hooks` and can be bypassed with `--no-verify` for a single commit or push when you need to.

| Layer | Trigger | Recipe | Budget | What it runs |
|------|--------|--------|--------|-------------|
| **IDE save** | On save | `rust-analyzer` | instant | type-check-on-save, clippy-on-save |
| **pre-commit** | `git commit` | `just lint-fast` | sub-2 s (docs-only) / 15–25 s warm (`*.rs` staged) | `fmt --check`, **`lint-prod`** (ultra-strict: pedantic + nursery + cargo + unwrap_used + missing_docs_in_private_items), **`lint-tests`** (same base + unwrap allowed), **`lint-ci`** (CI-mirror `-D warnings --all-targets`), **`check-windows`** (`cargo xwin check` of Windows-only `#[cfg(windows)]` paths) — all when `*.rs` staged; plus `taplo fmt --check` (if `*.toml` staged), `typos`, `reuse lint`, file-size policy — all in parallel; missing optional tools soft-skip |
| **pre-push** | `git push` | `just lint-pre-push` | 25–40 s warm | Same three ultra-strict clippy passes + **Windows `cargo xwin check`** + `fmt --check` + `rustdoc -D warnings` + `cargo deny check` + `nextest run --no-run` (test-binary link check) + file-size policy + `typos` + `reuse lint` — all in parallel. Full parity with `just ship` Phase 1 lint surface plus cross-platform Windows coverage; only the full test runtime (`nextest run`) is deferred to CI. |
| **PR CI** | on PR to `main` | `.github/workflows/pr-fast.yml` | minutes | PR-blocking matrix (classify → file-size, fmt, sanity, clippy, docs, test-build, tests, security, windows-check, required).  The `classify` job short-circuits docs-only / dep-only / infra-only PRs so heavy jobs only run when code actually changed.  Linux jobs run on `ubuntu-22.04`; `windows-check` runs natively on `windows-latest` (so `#[cfg(windows)]` compile errors now surface at PR time, not release time).  Tier 2 weekly workflow (`.github/workflows/tier-2.yml`) runs coverage + udeps + miri out of the critical path. |
| **Release** | manual today; automated after release-automation R4 | `just ship` today; release-plz release PR after R4 | minutes | today: version bump + `release/vX.Y.Z` PR + signed commit + auto-tag + binary build.  **Target state** (see [`docs/architecture/release-automation-plan.md`](docs/architecture/release-automation-plan.md)): release-plz opens `chore(release): prepare vX.Y.Z` PR automatically from conventional commits on `main`; merging that PR creates the tag; `release.yml` builds binaries as today. |

The ultra-strict flag stack — `common_flags` / `prod_flags` / `test_flags` — is defined in `@/Users/rnio/Private/Github/UltraFastFileSearch/just/shared.just:21-26` and pulled in identically by the local hooks and by `just ship` Phase 1, so **the rules a commit is checked against locally are the exact rules CI enforces**.

### Cross-platform coverage

As of the 2026-04-23 Phase 4 cutover, `pr-fast.yml`'s `windows-check` job runs natively on `windows-latest` so Windows compile errors surface at PR time.  The pre-commit / pre-push hooks still run `cargo xwin check` against `x86_64-pc-windows-msvc` as an advisory pre-PR gate (local fast-feedback); the CI `windows-check` is the authoritative one.  Breakdown:

- **Windows** — `cargo xwin check --workspace --all-targets --all-features` via `cargo-xwin` (provisions the MSVC SDK under `~/Library/Caches/xwin/`). Runs in **2–5 s warm** once the SDK is cached. Mandatory at pre-commit (when `*.rs` is staged) and pre-push.
- **Linux** — covered by CI matrix (`ubuntu-22.04`); a local Docker-based gate is available via `just lint-ci-linux` for conscious cross-platform sweeps but is **not** run at pre-push (minutes-scale, disproportionate for commit-time).
- **macOS / native host** — covered by the three native clippy passes (`lint-ci` / `lint-prod` / `lint-tests`) at both pre-commit and pre-push.

For a full sweep across all three targets, run `just check-all-targets` (native + xwin + Docker Linux). The recipe soft-skips tools that are not installed and reports which targets were exercised.

### First-time setup

```bash
just install-hooks         # sets core.hooksPath → scripts/hooks/
just install-dev-tools     # installs typos-cli + taplo-cli + cargo-xwin + x86_64-pc-windows-msvc target; prints pipx hint for `reuse`
```

Re-run `just install-hooks` after any rebase that touches `scripts/hooks/` — it's idempotent. The first time `cargo xwin check` runs it will download the MSVC SDK into `~/Library/Caches/xwin/` (~1–2 GB); subsequent runs reuse the cache.

### Running gates manually

```bash
just lint-fast             # the pre-commit bundle on demand
just lint-pre-push         # the pre-push bundle on demand
just lint-ci               # the single clippy gate that CI runs (`--all-targets --all-features --no-deps`)
just lint-ci-linux         # same clippy gate under a Linux x86_64 Docker image (catches macOS↔Linux drift)
just check-windows         # cargo xwin check against x86_64-pc-windows-msvc (Windows cross-platform gate)
just check-all-targets     # full sweep: native + Windows (xwin) + Linux (Docker)
just phase1-test           # the full `just ship` Phase-1 validation (pre-ship rehearsal)
```

### Bypass escape hatches

```bash
git commit --no-verify     # skip pre-commit
git push   --no-verify     # skip pre-push
```

Use them for work-in-progress commits on a feature branch. CI will still enforce the same gates on the PR.

### Keeping hook output fast

The hooks are tuned for an sccache-warm workspace. If you notice cold-cache slowness, verify:

- `cargo install sccache` is installed and `.cargo/config.toml` has `rustc-wrapper = "sccache"` (default).
- `sccache --show-stats` shows a healthy cache-hit rate after a few rebuilds.
- The shared `target/` directory is not being wiped by unrelated tools.

See `@scripts/hooks/_lint_fast.sh` and `@scripts/hooks/_lint_pre_push.sh` for the shared parallel runners both the hooks and the `just` recipes call into — edit there when adjusting the gate set, not in the hooks themselves.

## Target-dir hygiene

`just test` runs `cargo llvm-cov nextest` which writes source-instrumented artifacts into `$CARGO_TARGET_DIR/llvm-cov-target/` — a tree that is entirely separate from regular `cargo build`'s `target/debug/` and `target/release/`, and which can grow to **100 GB+** over a long session of coverage runs (each profile bump recompiles everything; every `.profraw` from a failed / killed run stays cached).  On a near-full disk (Dropbox sync volume, small SSD, external drive) this is the top cause of two otherwise-mysterious test failures:

- `handler::paths_blob_tests::try_pack_paths_blob_offloads_large_blob_to_shmem`
- `handler::csv_blob_tests::try_pack_csv_blob_offloads_large_blob_to_shmem`

Both pin the daemon's **shmem-offload** transport for large search blobs.  When the underlying `write_paths_blob` hits `ENOSPC` / `ERROR_DISK_FULL` during shmem file creation the daemon correctly degrades to inline JSON (see `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-daemon/src/handler_blob.rs:149-159`), which is exactly what the test asserts against — so the test panics with "*must be offloaded to SearchPayload::ShmemBlob; got InlineBlob(...)*".  This is working as designed: silently skipping under disk pressure would hide a real `write_paths_blob` regression on a healthy host.

Run this on the host that surfaced the failure:

```bash
just clean-cov
```

The recipe (`@/Users/rnio/Private/Github/UltraFastFileSearch/just/cache.just`) prunes:

- `$CARGO_TARGET_DIR/llvm-cov-target/` — the instrumented build tree
- `$CARGO_TARGET_DIR/llvm-cov/` — the HTML coverage report directory
- `$CARGO_TARGET_DIR/**/*.profraw` — leftover instrumentation output from killed / crashed runs
- `$LOCALAPPDATA\uffs\shmem\` (Windows) / `$XDG_DATA_HOME/uffs/shmem/` (Linux) / `~/Library/Application Support/uffs/shmem/` (macOS) — orphan shmem files from aborted daemon sessions

Leaves regular `cargo build` artifacts, the sccache wrapper cache, and the Cargo registry alone, so a subsequent `cargo build` stays incremental.  Empirically frees 5–30 GB on an active dev box and up to 100+ GB after a week of heavy coverage work.

If `just clean-cov` finishes and `just test` still fails with the same shmem-offload assertion, the issue is in `write_paths_blob` itself (a real regression) — check the daemon's stderr for a `tracing::warn!` line beginning `paths_blob shmem write failed; falling back to inline JSON` and open an issue with that error message.

## Commit message conventions

UFFS is migrating to [Conventional Commits](https://www.conventionalcommits.org/) to drive automated versioning and changelog generation via `release-plz` + `git-cliff`.  For a one-page **"what to do this week"** cheatsheet (PR conventions, what's automatic, FAQs), see [`docs/architecture/release-automation-current-flow.md`](docs/architecture/release-automation-current-flow.md).  The full multi-phase migration plan lives in [`docs/architecture/release-automation-plan.md`](docs/architecture/release-automation-plan.md).

**What matters for you today**:

- The **PR title** (which becomes the squash-merge commit subject) should follow conventional commits.  Intermediate commits on a feature branch don't need to — only what lands on `main`.
- During the **advisory phase** (release-automation Phase R1a, ongoing), non-conforming titles trigger a bot comment but do **not** block merge.  Treat the comment as a nudge, not a gate.
- During the **mandatory phase** (release-automation Phase R1b, scheduled after ≥1 month of advisory observation), non-conforming titles will hard-fail the required PR Fast CI check.

**Recognized types and their release impact**:

| Type | Meaning | Triggers release? | Version bump |
|---|---|---|---|
| `feat:` | User-visible new feature | Yes | Minor (0.X.0) |
| `fix:` | User-visible bug fix | Yes | Patch (0.0.X) |
| `perf:` | Performance improvement | Yes | Patch |
| `feat!:` / `fix!:` | Breaking change (note the `!`) | Yes | Major (X.0.0, or minor pre-1.0) |
| `refactor:` | Code restructure, no behavior change | No | — |
| `docs:` | Documentation only | No | — |
| `test:` | Test additions / changes | No | — |
| `chore:` | Tooling, config, catch-all | No | — |
| `ci:` | CI / workflow change | No | — |
| `build:` | Build system / dependency change | No | — |
| `style:` | Formatting, whitespace | No | — |
| `revert:` | Reverts a previous commit | Inherits reverted type | Inherits |

**Examples**:

- `feat(mft): add zstd-compressed MFT archive support` — minor bump, appears under "Features" in changelog
- `fix(security): correct FNV-1a pipe hash for empty inputs` — patch bump, appears under "Bug Fixes"
- `feat(cli)!: rename --query to --filter; drop deprecated --q shorthand` — major/minor bump (depending on current v0.x vs v1.x), appears under "BREAKING CHANGES"
- `chore: bump dependabot grouping window to weekly` — no release
- `docs(architecture): clarify pipeline stage naming` — no release

**Scopes** (optional, in parentheses after type): prefer the crate name or a short area tag.  Examples: `mft`, `cli`, `core`, `security`, `polars`, `ci`, `build`, `architecture`.  Omit the scope if the change is workspace-wide.

**If in doubt**, use `chore:` — it never triggers a release.  If a PR genuinely has both a fix and a feature, split it into two PRs.

## Architecture guardrails

- Do not depend on `polars` directly; use `uffs-polars`.
- Preserve the crate layering: `uffs-polars` ← `uffs-mft` ← `uffs-core` ← `uffs-cli`.
- Prefer fixture-, golden-, or saved-MFT/index-based tests for portable validation.
- Update docs when contributor-facing workflow or user-visible behavior changes.
- **New Windows-gated code must pass `just lint-ci-windows` before PR.**  Strict clippy on `#[cfg(windows)]` paths is advisory during the Phase W2–W5 backlog cleanup (see `docs/architecture/windows-clippy-and-linux-cross-plan.md`) and becomes a hard gate at PR CI after W5.  New lints introduced into the backlog trigger an immediate reviewer ask.

## Docs map

- Root overview: `README.md`
- Documentation map: `docs/README.md`
- Developer docs landing page: `docs/dev/README.md`
- Architecture docs: `docs/architecture/README.md`
- **CI / gate architecture**: `docs/architecture/dev-flow-implementation-plan.md`
- **Release / versioning architecture**: `docs/architecture/release-automation-plan.md`
- **Cross-target strict-lint architecture**: `docs/architecture/windows-clippy-and-linux-cross-plan.md`

If you are changing behavior that depends on raw NTFS access, call out the Windows/Admin requirement in the relevant docs and tests.