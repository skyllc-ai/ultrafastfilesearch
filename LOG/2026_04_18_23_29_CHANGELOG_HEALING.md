# CHANGELOG — Healing Run 2026-04-18 23:29 PDT

## Goal

Ship the auto-concurrency formula bump + Windows sweep + MCP
docstring refresh accumulated in this session via `just ship -v`,
cutting a new release (expected `v0.5.46`).

Commits awaiting the next release from the prior v0.5.45 tag:

* `b67608cbb` — **daemon:** bump auto-concurrency formula to
  `max(2, (cpus × 26) / (drives × 10))` — roughly **2.6× cpus / drives**,
  i.e. 30 % more permits than the v0.5.45 default of
  `2 × cpus / drives`.  Calibrated against the 24/7 Windows box
  (2026-04-18 `LOG/Output` sweep): wall time collapses from 21.7 s to
  12.0 s with only ~16 % per-query latency cost.  Formula extracted
  into `IndexManager::auto_concurrency_target(cpus, drives)` pure
  helper and locked in with 4 unit tests covering the calibration
  target, common box shapes, the drives=0 clamp, and the floor-of-2
  edge cases.
* `697952f63` — **sweep:** set `UFFS_LOG_DIR` in the Windows
  concurrency-sweep script so the retune-line verification can
  actually find `uffsd.log`.
* `090f452c5` — **sweep:** new `scripts/windows/concurrency-sweep.rs`
  harness that drives the semaphore across `N ∈ {2, 3, 4, 6, 8, 12, 16, 24}`
  and measures wall + per-query latency, plus a refresh of the MCP
  `ClientSlot` docstring so the "capacity is owned by the daemon"
  rationale block quotes the new formula.

## Pre-pipeline state

Local advisory checks (macOS, workspace):

* Unit tests for the new formula pass:
  `cargo test -p uffs-daemon --lib auto_concurrency_target`
  → 4 / 4 pass.
* Working tree has one unstaged cosmetic whitespace tweak in
  `crates/uffs-daemon/src/index/tests.rs` (user alignment of `//`
  column on the test-case tuples).  Will be captured by the release
  commit.

Files touched since `v0.5.45`:

* `crates/uffs-daemon/src/index/mod.rs` — new
  `auto_concurrency_target(cpus, drives)` const pure fn,
  `tune_concurrency` delegates to it, docstrings refreshed.
* `crates/uffs-daemon/src/index/tests.rs` — 4 new unit tests.
* `crates/uffs-daemon/src/index/search.rs` — permit-acquire comment
  refreshed to quote the new formula.
* `crates/uffs-mcp/src/handler/mod.rs` — `ClientSlot` rationale
  block refreshed.
* `scripts/windows/concurrency-sweep.rs` — **new** harness (+ env-var
  plumbing for `UFFS_LOG_DIR`).

## Operating rules

* Baseline + final validation: `just ship -v`.  Local `cargo` checks
  advisory only.
* No suppression hacks (`#[allow]`, disabled lints, skipped tests).
* Surgical, idiomatic fixes targeted at root cause.
* Preserve public API and observable behaviour unless the pipeline
  proves otherwise.
* Strengthen tests, do not dodge them.
* One atomic commit per healed issue with `fix:` prefix and root
  cause summary.  This document stays current throughout the run and
  is part of the final commit.

## Run log

### Run 1 — `just ship -v`

**Result:** ❌ Phase 1 clippy gate failed in the doctest-combined step.

```text
error: `drives` is shadowed
   --> crates/uffs-daemon/src/index/mod.rs:217:13
    |
217 |         let drives = if drives == 0 { 1 } else { drives };
    |             ^^^^^^
    |
note: previous binding is here
   --> crates/uffs-daemon/src/index/mod.rs:216:62
    |
216 |     pub(crate) const fn auto_concurrency_target(cpus: usize, drives: usize) -> usize {
    |                                                              ^^^^^^
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#shadow_reuse
    = note: requested on the command line with `-D clippy::shadow-reuse`

error: could not compile `uffs-daemon` (lib) due to 1 previous error
```

**Root cause.** In `auto_concurrency_target(cpus, drives)` I clamped
the zero-drive edge case with `let drives = if drives == 0 { 1 } else { drives };`
— which rebinds the parameter under the same name.  The workspace
enables `clippy::shadow-reuse` as a deny-level gate, so this fails the
build.

**Fix (surgical, non-suppression).** Rename the clamp binding to
`effective_drives` so it no longer shadows the parameter, and update
the helper's body + denominator accordingly.  Added a two-sentence
comment explaining the clamp intent and the rename motive.

```rust
// Clamp drives=0 → 1 so the pre-load admission window (before any
// drive has registered) still returns a usable target instead of
// dividing by zero.  Rename vs. the parameter so we don't trip
// `clippy::shadow_reuse`.
let effective_drives = if drives == 0 { 1 } else { drives };
let numerator = cpus.saturating_mul(26);
let denominator = effective_drives.saturating_mul(10);
```

Same math, same public contract, same 4 locked-in unit tests still
pass.

### Run 2 — `just ship -v`

**Result:** ✅ All 12 phases green.

```text
00-toolchain-ensure     0s
01-update-polars-git    0s
02-clean-artifacts      2s
03-format-code          0s
04-coverage-tests      31s
05-parallel-validation 13s
06-format-check         0s
07-version-increment    0s
08-build-release     2m 13s
09-deploy-binary     3m  1s
10-git-commit           0s
11-git-push             2s

📦 Binary uploaded to GitHub Release
📤 Changes committed and pushed
```

Release cut as `v0.5.46`.  `Cargo.toml` bumped from `0.5.45` to
`0.5.46`.  Auto-commit `628343a6f` includes the shadow-reuse fix plus
the user's cosmetic whitespace tweak in `tests.rs`.

## Post-run state

* `git log --oneline -1` → `628343a6f chore: development v0.5.46 - comprehensive testing complete [auto-commit]`
* `grep ^version Cargo.toml` → `version = "0.5.46"`
* `ls dist/v0.5.46/` populated with the four Windows + macOS binaries.
* Working tree clean after the healing changelog commit that follows.

## Lessons for next time

* `clippy::shadow-reuse` is a workspace deny gate — never rebind a
  parameter under the same name, even for a one-liner clamp.  Default
  to a descriptive new binding (`effective_*`, `clamped_*`, etc.).
* The local advisory run `cargo test -p uffs-daemon --lib auto_concurrency_target`
  passed 4 / 4 but did **not** catch this because plain `cargo test`
  without `--quiet` and without `-D warnings` does not run clippy with
  the shipping gate.  The only reliable baseline is `just ship -v`
  itself — exactly the operating rule we're following.

