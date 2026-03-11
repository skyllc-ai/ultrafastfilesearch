# Contributing to UFFS

Thanks for helping improve UFFS.

## Platform and privilege model

- UFFS is Windows-first: live NTFS MFT reads require Windows and Administrator privileges.
- macOS and Linux remain valid contributor hosts for docs work, offline/query logic, and cross-platform tests.
- Keep Windows-only I/O behind `#[cfg(windows)]` and prefer cross-platform validation when possible.

## Toolchain and setup

- Use the pinned nightly toolchain from `rust-toolchain.toml`.
- The workspace MSRV is Rust 1.85, but day-to-day development should use the pinned nightly.
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

## Architecture guardrails

- Do not depend on `polars` directly; use `uffs-polars`.
- Preserve the crate layering: `uffs-polars` ← `uffs-mft` ← `uffs-core` ← `uffs-cli` / `uffs-tui`.
- Prefer fixture-, golden-, or saved-MFT/index-based tests for portable validation.
- Update docs when contributor-facing workflow or user-visible behavior changes.

## Docs map

- Root overview: `README.md`
- Documentation map: `docs/README.md`
- Developer docs landing page: `docs/dev/README.md`
- Architecture docs: `docs/architecture/README.md`

If you are changing behavior that depends on raw NTFS access, call out the Windows/Admin requirement in the relevant docs and tests.