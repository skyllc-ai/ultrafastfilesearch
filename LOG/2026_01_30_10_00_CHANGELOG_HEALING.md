# CHANGELOG_HEALING - 2026-01-30

## CI Pipeline Run #1

**Started:** 2026-01-30 ~10:00
**Command:** `rust-script scripts/ci-pipeline.rs go -v`
**Branch:** `feature/cpp-tree-algorithm-port`

### Pre-run Status
- Previous commit: `feat(benchmark): accurate timing instrumentation and three-way comparison`
- Changes include: ReadParseTiming struct, read_all_parallel_with_timing(), benchmark updates, trial_run.ps1 three-way comparison

### Issues Found

1. **clippy::doc_markdown** (5 errors):
   - `crates/uffs-mft/src/index.rs:135` - `tree_allocated` missing backticks
   - `crates/uffs-mft/src/index.rs:5338` - `MftIndex` missing backticks
   - `crates/uffs-mft/src/index.rs:5338` - `IndexBuildTiming` missing backticks
   - `crates/uffs-mft/src/reader.rs:223` - `tree_allocated` missing backticks
   - `crates/uffs-mft/src/main.rs:506` - `tree_allocated` missing backticks (found in run #2)

2. **clippy::cast_possible_truncation** (3 errors):
   - `crates/uffs-mft/src/index.rs:5358` - `as_millis() as u64` truncation
   - `crates/uffs-mft/src/index.rs:5371` - `as_millis() as u64` truncation
   - `crates/uffs-mft/src/index.rs:5373` - `as_millis() as u64` truncation

### Fixes Applied

1. **Doc markdown fixes**: Added backticks around `tree_allocated`, `MftIndex`, `IndexBuildTiming` in doc comments

2. **Cast truncation fixes**: Changed `as_millis() as u64` to `u64::try_from(...).unwrap_or(u64::MAX)` for safe saturating conversion (overflow impossible for realistic durations, but satisfies clippy)

## CI Pipeline Run #3

### Issues Found (Windows cross-compilation)

1. **E0599**: `MftError::IoError` variant doesn't exist - should be `MftError::Io`
   - `crates/uffs-mft/src/reader.rs:1076`

2. **unused_imports warning**: `IndexBuildTiming` imported but not used
   - `crates/uffs-mft/src/reader.rs:2679`

### Fixes Applied

1. Changed `MftError::IoError(...)` to `MftError::Io(...)` at line 1076
2. Removed unused `IndexBuildTiming` import at line 2679

## CI Pipeline Run #4

### Issues Found (Windows cross-compilation)

1. **E0432**: `IndexCache` doesn't exist in `cache` module
   - `crates/uffs-mft/src/main.rs:3832`

2. **E0063**: Missing fields `index_build_ms` and `tree_metrics_ms` in `PhaseTimings` initializer
   - `crates/uffs-mft/src/main.rs:1974`

### Fixes Applied

1. Replaced `IndexCache::new()` with `load_cached_index()` function from cache module
2. Added missing `index_build_ms` and `tree_metrics_ms` fields to `PhaseTimings` initialization

---

