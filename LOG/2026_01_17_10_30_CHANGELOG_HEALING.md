# Changelog Healing - 2026-01-17

## Issue 1: CI pipeline succeeds despite cross-compilation failure

### What Failed
The CI pipeline (`rust-script scripts/ci-pipeline.rs go -v`) completed with exit code 0 even though the Windows cross-compilation failed with:
```
rustc-LLVM ERROR: 64-bit code requested on a subtarget that doesn't support it!
'apple-m4' is not a recognized processor for this target (ignoring processor)
```

### Root Cause
Two issues:

1. **`.cargo/config.toml`**: The Windows target had `target-cpu=native` which refers to the host CPU (Apple M4 on macOS ARM64), not the target CPU (x86_64). This is invalid for cross-compilation.

2. **`scripts/build-cross-all.rs`**: When `build_for_target()` returned `false` (build failed), the script continued to the next target and eventually exited with code 0, not propagating the failure.

### Fix Applied

**File: `.cargo/config.toml`**
- Changed `target-cpu=native` to `target-cpu=x86-64-v2` for the Windows target
- Added comment explaining why `native` breaks cross-compilation
- `x86-64-v2` provides good compatibility (SSE4.2, POPCNT - supported since ~2009)

**File: `scripts/build-cross-all.rs`**
- Changed to fail-fast: immediately `exit(1)` when any build fails
- Removed the pattern of collecting failures and checking at the end

---

## Issue 2: Benchmark binary killed during test enumeration (SIGKILL)

### What Failed
After fixing Issue 1, the CI failed with:
```
error: creating test list failed
for `uffs-core::bench/search_benchmarks`, command ... aborted with signal 9 (SIGKILL)
```

### Root Cause
The coverage test command used `--all-targets` which includes benchmarks. The benchmark binary (`search_benchmarks`) creates large DataFrames (up to 100,000 rows) during initialization. When `cargo nextest` runs `--list` to enumerate tests, it loads the benchmark binary which triggers this initialization, causing memory pressure or timeout leading to SIGKILL.

### Fix Applied

**File: `scripts/ci-pipeline.rs`**
- Changed `--all-targets` to `--lib --bins --tests` in three places:
  1. Line ~484: `phase1_testing()` function
  2. Line ~690: `coverage_report()` function
  3. Line ~807: `phase1_optimized()` function
- Added comments explaining why benchmarks are excluded

---

## Issue 3: Unused crate dependency warnings in uffs-mft

### What Failed
Compilation warnings about unused crate dependencies:
- Library (`lib.rs`): `anyhow`, `clap`, `indicatif`, `tokio`, `tracing`, `tracing_subscriber`
- Binary (`main.rs`): `bitflags`, `criterion`, `rayon`, `thiserror`, `uffs_mft`, `uffs_polars`, `zstd`, `indicatif`, `tracing`

### Root Cause
Cargo doesn't support per-binary dependencies. The `uffs-mft` crate has both a library and a binary:
- The library uses some dependencies (e.g., `bitflags`, `thiserror`)
- The binary uses other dependencies (e.g., `clap`, `indicatif`)
- Some dependencies are platform-gated with `#[cfg(windows)]`

On non-Windows platforms, the platform-gated code is not compiled, making those dependencies appear unused.

### Fix Applied

**File: `crates/uffs-mft/src/lib.rs`**
- Added `use X as _;` statements for binary-only dependencies
- Added `#[cfg(not(windows))]` gated suppressions for Windows-only dependencies

**File: `crates/uffs-mft/src/main.rs`**
- Added `use X as _;` statements for library-only dependencies
- Added `#[cfg(not(windows))]` gated suppressions for Windows-only dependencies

### Verification
Run: `cargo check --package uffs-mft`
- Should complete with no warnings

