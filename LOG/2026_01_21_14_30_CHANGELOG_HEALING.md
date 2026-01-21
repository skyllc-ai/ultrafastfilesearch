# Healing Log – 2026-01-21 14:30

## Context

The CI pipeline (`rust-script scripts/ci-pipeline.rs go -v`) was failing in the **Production linting** stage due to `clippy` errors in the `uffs-cli` crate's diagnostic compatibility stubs:

- `crates/uffs-cli/src/bin/scan_mft_magic.rs`
- `crates/uffs-cli/src/bin/dump_mft_records.rs`
- `crates/uffs-cli/src/bin/analyze_mft_parents.rs`

These binaries are intentionally tiny shims that inform users the real implementations have moved to `uffs-diag`.

## Symptoms

Running the CI pipeline or a focused clippy invocation produced errors such as:

- `extern crate anyhow is unused in crate 'scan_mft_magic'`
- `extern crate chrono is unused in crate 'dump_mft_records'`
- ... (similar for `clap`, `dirs_next`, `indicatif`, `tokio`, `tracing`,
  `tracing_appender`, `tracing_subscriber`, `uffs_core`, `uffs_mft`, `uffs_polars`)
- `clippy::print_stderr` for the `eprintln!`-based shims
- `missing-docs` / `missing-docs-in-private-items` for the crate roots

Because `-D warnings` and `-D clippy::cargo` are enabled for production linting, these
`unused-crate-dependencies` findings were hard errors.

## Root Cause

The three compatibility binaries live in the `uffs-cli` crate so that the old tool
names remain discoverable, but their current implementations only print a short
message and exit. The main `uffs-cli` library and primary binaries depend on a
substantial set of crates (error handling, async runtime, logging, progress bars,
core search logic, MFT/Polars helpers), and those dependencies are declared at the
crate level.

For these tiny stub binaries, that means `cargo` sees a large dependency set that is
never referenced inside the module, triggering `unused-crate-dependencies`.

Separately, the stubs use `eprintln!` to direct users to the new `uffs-diag` tools,
which conflicted with `clippy::print_stderr`, and they did not have crate-level
`//!` docs explaining their purpose.

## Changes

### 1. Documented `analyze_mft_parents` stub and allowed `print_stderr`

**File:** `crates/uffs-cli/src/bin/analyze_mft_parents.rs`

- Replaced header comments with crate-level documentation explaining the stub's
  role and why it exists.
- Added a narrow crate-level allow for `clippy::print_stderr`, since the whole
  binary is intentionally just a small UX shim that prints to stderr.
- Normalized the `eprintln!` message to use an explicit `\n` with continuation
  for readability.

Example:

```rust
//! Compatibility stub for the old `analyze_mft_parents` binary location.
//!
//! This crate used to host the real implementation, but the tool has been
//! moved to `crates/uffs-diag` ...

#![allow(clippy::print_stderr)]

fn main() {
    eprintln!(
        "analyze_mft_parents has moved to the uffs-diag crate.\n\
         Please run it via `cargo run -p uffs-diag --bin analyze_mft_parents -- <args>`",
    );
}
```

### 2. Wired dependencies for all three compatibility stubs

**Files:**

- `crates/uffs-cli/src/bin/scan_mft_magic.rs`
- `crates/uffs-cli/src/bin/dump_mft_records.rs`
- `crates/uffs-cli/src/bin/analyze_mft_parents.rs`

For each stub, added explicit `_`-aliased uses of the dependencies that are required
by the `uffs-cli` crate but are otherwise unused inside these tiny shims.

This is the narrow, explicit fix recommended by Clippy itself (rather than a broad
`#[allow(unused_crate_dependencies)]`), and it keeps the crate graph honest while
preserving the minimal behavior of the stubs.

Example (pattern shared across all three stub files):

```rust
#![allow(clippy::print_stderr)]

// Keep these dependencies wired up so that this compatibility stub reflects
// the same crate graph as the main CLI, satisfying `unused_crate_dependencies`
// while remaining a tiny binary.
use anyhow as _;
use chrono as _;
use clap as _;
use dirs_next as _;
use indicatif as _;
use tokio as _;
use tracing as _;
use tracing_appender as _;
use tracing_subscriber as _;
use uffs_core as _;
use uffs_mft as _;
use uffs_polars as _;

fn main() {
    eprintln!("...");
}
```

This directly addresses the `extern crate X is unused` errors while keeping the
solution fully explicit and localized to the affected binaries.

## Verification

### Focused local lint

Ran a focused clippy invocation for `uffs-cli` binaries with the same strict flags
as CI production linting:

```bash
cargo clippy -p uffs-cli --bins --all-features -- \
  -D warnings \
  -D clippy::pedantic \
  -D clippy::nursery \
  -D clippy::cargo \
  -A clippy::multiple_crate_versions \
  -W clippy::panic -W clippy::todo -W clippy::unimplemented
```

Result:

- Exit code: 0
- No remaining warnings or errors in `scan_mft_magic`, `dump_mft_records`, or
  `analyze_mft_parents` stubs.

### Full CI pipeline

Finally, ran the full CI pipeline as required:

```bash
rust-script scripts/ci-pipeline.rs go -v
```

Result:

- Production linting: passes for `uffs-cli`, `uffs-diag`, and the rest of the workspace.
- Documentation tests, test linting, and dependency security (`cargo deny`) all
  complete successfully.
- Overall pipeline status: success (exit code 0).

## Impact and Notes

- The behavior of the diagnostic tools has **not** changed: real functionality
  still lives in `uffs-diag`, and the `uffs-cli` binaries remain tiny shims that
  print clear guidance.
- The changes are narrowly scoped to satisfy `clippy` under strict CI settings
  (`-D warnings`, `-D clippy::cargo`) without introducing broad allow attributes
  or weakening tests.
- If additional legacy diagnostic entry points are added in the future, they
  should follow the same pattern: documented stub, targeted `print_stderr` allow
  if needed, and explicit `_`-aliased `use` statements for crate-level
  dependencies to keep `unused-crate-dependencies` satisfied.

