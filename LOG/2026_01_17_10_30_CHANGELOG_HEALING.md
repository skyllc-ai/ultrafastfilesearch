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

---

## Issue 4: WMI crate 0.18.0 breaking API changes

### What Failed
Compilation errors in 16 files in `uffs-legacy` crate:
```
error[E0432]: unresolved import `wmi::COMLibrary`
error[E0061]: this function takes 1 argument but 2 arguments were supplied
```

### Root Cause
The `wmi` crate version 0.18.0 introduced breaking changes:
1. `COMLibrary` was removed from the public API
2. `WMIConnection::with_namespace_path` now takes only 1 argument (namespace path) instead of 2 (namespace path + COM library)

### Fix Applied

**16 files in `crates/uffs-legacy/src/modules/disk/`:**
- `drive_info.rs`, `wim_defrag_analysis.rs`, `wim_disk_quota.rs`, `wmi_disk_drive.rs`
- `wmi_disk_partition.rs`, `wmi_encryptable_volume.rs`, `wmi_logical_disk.rs`, `wmi_mount_point.rs`
- `wmi_msft_disk.rs`, `wmi_msft_partition.rs`, `wmi_perf_disk_physical_disk.rs`, `wmi_physical_media.rs`
- `wmi_quota_setting.rs`, `wmi_shadow_copy.rs`, `wmi_volume.rs`, `wmi_volume_quota.rs`

For each file:
- Changed `use wmi::{COMLibrary, WMIConnection}` to `use wmi::WMIConnection`
- Removed COM library initialization: `let com_con = COMLibrary::new()?;`
- Changed `WMIConnection::with_namespace_path("...", com_con.into())?` to `WMIConnection::with_namespace_path("...")?`

---

## Issue 5: Noop `.clone()` call on `&OsStr`

### What Failed
Warning in `drive_info.rs`:
```
warning: call to `.clone()` on a reference in this situation does nothing
   --> crates/uffs-legacy/src/modules/disk/drive_info.rs:419:41
    |
419 |         &mut drive.root_path.as_os_str().clone(),
```

### Root Cause
`OsStr` does not implement `Clone`, so calling `.clone()` on `&OsStr` just copies the reference, which is a no-op.

### Fix Applied

**File: `crates/uffs-legacy/src/modules/disk/drive_info.rs`**
- Changed `&mut drive.root_path.as_os_str().clone()` to `drive.root_path.as_os_str()`

---

## Issue 6: Progress bar enhancement with environment variable control

### What Changed
Added fancy progress bars for MFT reading with:
- Spinner, drive letter, elapsed time, progress bar, bytes read/total
- Environment variable control: `UFFS_NO_PROGRESS=1` disables progress bars

### Implementation

**File: `crates/uffs-cli/src/commands.rs`**
- Added `#[cfg(windows)]` helper functions:
  - `is_progress_disabled()` - checks `UFFS_NO_PROGRESS` env var
  - `create_multi_progress()` - creates multi-progress container
  - `add_drive_progress()` - adds drive progress bar to container
  - `create_save_raw_progress()` - creates progress bar for saving raw MFT
- Updated progress bar styles to show bytes, elapsed time, and ETA
- Made `MultiProgress` import conditional on Windows

### Verification
- `cargo clippy --workspace --all-targets -- -D warnings` passes
- `cargo xwin check --target x86_64-pc-windows-msvc` passes
- `cargo test --workspace` passes

---

## Issue 7: CI pipeline doesn't clean cargo-xwin SDK cache

### What Failed
The CI pipeline failed with a linker error during Windows cross-compilation:
```
lld-link: error: truncated or malformed archive (string table at long name offset 210not terminated)
```

The error occurred when linking against Windows SDK libraries in `~/Library/Caches/cargo-xwin/xwin/`.

### Root Cause
The cargo-xwin tool downloads Windows SDK and CRT files to `~/Library/Caches/cargo-xwin/xwin/`. These files can become corrupted (truncated archives) due to:
- Interrupted downloads
- Disk space issues
- Network problems during download

The CI pipeline's clean step only cleaned `/tmp/rust-target/uffs` (cross-compilation target directory) but NOT the cargo-xwin SDK cache.

### Fix Applied

**File: `scripts/ci-pipeline.rs`**
- Added cargo-xwin SDK cache cleaning to the `01-clean-artifacts` step
- Now cleans `~/Library/Caches/cargo-xwin/xwin` in addition to `/tmp/rust-target/uffs`
- Added comment explaining why this cache needs to be cleaned

### Verification
Run: `rm -f build/.uffs-workflow-state.json && rust-script scripts/ci-pipeline.rs go -v`
- Should see "🧹 Cleaning cargo-xwin SDK cache: ..." in the output
- Cross-compilation should succeed without linker errors

---

## Issue 8: Cargo-xwin cache corruption between pipeline steps

### What Failed
Even after adding xwin cache cleaning to step 01 (clean-artifacts), the cross-compilation in step 08 (deploy-binary) still failed with:
```
lld-link: error: truncated or malformed archive (string table at long name offset 210not terminated)
```

### Root Cause
The xwin cache was only cleaned at the START of the pipeline (step 01), but by the time step 08 runs:
1. Step 03 (coverage-tests) runs native compilation
2. Step 04-07 run various validation steps
3. By step 08, the xwin cache may have been corrupted or partially downloaded

Additionally, there's a version mismatch in windows crates:
- `fs4` → `windows-sys v0.59.0` → `windows_x86_64_msvc v0.52.6`
- `socket2` → `windows-sys v0.60.2` → `windows_x86_64_msvc v0.53.1`

This mismatch (both 0.52.6 and 0.53.1 in the same link) increases the chance of edge-case linker issues.

### Fix Applied

**File: `scripts/ci-pipeline.rs`**
- Added xwin cache cleaning RIGHT BEFORE cross-compilation in step 08 (deploy-binary)
- This ensures a fresh xwin SDK download immediately before it's needed
- The cache is now cleaned in TWO places:
  1. Step 01 (clean-artifacts) - for fresh pipeline starts
  2. Step 08 (deploy-binary) - right before cross-compilation

### Verification
Run: `rm -f build/.uffs-workflow-state.json && rust-script scripts/ci-pipeline.rs go -v`
- Should see "🧹 Cleaning cargo-xwin SDK cache before cross-compilation: ..." in step 08
- Cross-compilation should succeed without linker errors

---

## Issue 9: Xwin cache cleaning not happening in build-cross-all.rs

### What Failed
The xwin cache cleaning in `ci-pipeline.rs` step 08 was conditional on the cache existing, but since step 01 already cleaned it, the cache didn't exist and the cleaning was skipped. The xwin SDK was then downloaded fresh during `cargo xwin build`, but the download got corrupted.

### Root Cause
The xwin cache cleaning logic was in `ci-pipeline.rs` but the actual `cargo xwin` command runs in `build-cross-all.rs`. The cleaning in `ci-pipeline.rs` happened before calling `build-cross-all.rs`, but since the cache was already cleaned in step 01, the conditional check `if xwin_cache.exists()` was false.

### Fix Applied

**File: `scripts/build-cross-all.rs`**
- Added xwin cache cleaning directly in `build-cross-all.rs` right before the build loop
- This ensures the cache is cleaned immediately before `cargo xwin` runs
- Added verbose command output showing exactly what cargo command is being executed
- Format: `→ binary (profile) → cargo args (target: triple)`

### Verification
Run: `rm -f build/.uffs-workflow-state.json && rust-script scripts/ci-pipeline.rs go -v`
- Should see "🧹 Cleaning cargo-xwin SDK cache: ..." in the build-cross-all.rs output
- Should see verbose command output like "→ uffs (debug) → cargo xwin build ..."
- Cross-compilation should succeed without linker errors

