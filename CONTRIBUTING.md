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
| **PR CI** | on PR to `main` | `.github/workflows/ci.yml` | minutes | Tier 1 matrix: Format, Clippy, Rustdoc, Security, Test Build, Tests, Doc Tests, File Size Policy (all on `ubuntu-22.04` — the pre-push Windows gate is the only pre-PR check that exercises `#[cfg(windows)]` code) |
| **Release** | manual | `just ship` | minutes | version bump + `release/vX.Y.Z` PR + signed commit + auto-tag + binary build |

The ultra-strict flag stack — `common_flags` / `prod_flags` / `test_flags` — is defined in `@/Users/rnio/Private/Github/UltraFastFileSearch/just/shared.just:21-26` and pulled in identically by the local hooks and by `just ship` Phase 1, so **the rules a commit is checked against locally are the exact rules CI enforces**.

### Cross-platform coverage

CI today runs only on `ubuntu-22.04`, so Windows-specific compile errors (e.g. `#[cfg(windows)]` tests / benches, `std::os::windows::*` usage, type drift between platforms) would not surface until a user tried to build on Windows. The pre-commit / pre-push hooks run `cargo xwin check` against `x86_64-pc-windows-msvc` as a first-class gate to close that gap:

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

## Architecture guardrails

- Do not depend on `polars` directly; use `uffs-polars`.
- Preserve the crate layering: `uffs-polars` ← `uffs-mft` ← `uffs-core` ← `uffs-cli`.
- Prefer fixture-, golden-, or saved-MFT/index-based tests for portable validation.
- Update docs when contributor-facing workflow or user-visible behavior changes.

## Docs map

- Root overview: `README.md`
- Documentation map: `docs/README.md`
- Developer docs landing page: `docs/dev/README.md`
- Architecture docs: `docs/architecture/README.md`

If you are changing behavior that depends on raw NTFS access, call out the Windows/Admin requirement in the relevant docs and tests.