# CHANGELOG_HEALING - 2026-01-24 14:00

## Summary

Fixed CI pipeline errors in `uffs-mft` crate that were causing cross-compilation failures for Windows.

## What Failed

The CI pipeline failed during Phase 2 (Build & Deploy) with multiple errors:

### 1. Errors in `main.rs` (binary target) - 14 errors

- **Method name mismatches**: `read_to_index()` should be `read_all_index()`, `get_ntfs_volume_data()` should be `volume_data()`
- **Type mismatches**: `apply_usn_changes()` expected `&[FileChange]` but received `HashMap<u64, FileChange>`
- **Error type conversion**: `MftIndex::load_from_file()` returns `Box<dyn Error>` which doesn't implement `Send + Sync`
- **Unnecessary qualifications**: `anyhow::Result<()>` and `std::path::Path` when already imported
- **Unused variables**: `idx` variable not used in loop

### 2. Warnings in `io.rs`

- **Unused field**: `volume_idx` field in `InFlightOp` struct was never read
- **Unnecessary mut**: `buffer` variable didn't need to be mutable

### 3. Warnings in `main.rs`

- **Unused import**: `MftReader` was imported but not used

## Why It Failed

The `main.rs` binary was using outdated API names and patterns that didn't match the refactored library code:

1. **API Evolution**: The library was refactored to use cleaner method names (`read_all_index()` instead of `read_to_index()`, `volume_data()` instead of `get_ntfs_volume_data()`)
2. **Return Type Changes**: `volume_data()` now returns a reference `&NtfsVolumeData` instead of `Result<NtfsVolumeData>`
3. **Type Signature Changes**: `apply_usn_changes()` expects a slice, but the USN aggregation returns a HashMap
4. **Dead Code**: The `InFlightOp` struct had a `volume_idx` field that was set but never read

## How It Was Fixed

### main.rs fixes:

1. **Lines 3577, 3792, 4039**: Changed `read_to_index()` → `read_all_index()`
2. **Lines 3588, 3803, 3986, 4050**: Changed `get_ntfs_volume_data()?` → `volume_data()` (removed `?` since it returns a reference)
3. **Line 3635-3637**: Added `.map_err(|e| anyhow::anyhow!("{}", e))?` to convert `Box<dyn Error>` to `anyhow::Error`
4. **Lines 3962-3963**: Converted HashMap to Vec using `changes_map.into_values().collect()`
5. **Lines 3426, 3470, 3564, 3626, 3679, 3739, 3836, 3874, 4026**: Removed unnecessary `anyhow::` qualifications
6. **Lines 3564, 3626**: Removed unnecessary `std::path::` qualifications
7. **Lines 3351-3357**: Removed unused empty closure code block
8. **Line 3377**: Changed `idx` to `_idx` to indicate intentionally unused
9. **Line 3870**: Removed unused `MftReader` import

### io.rs fixes:

1. **Line 6817**: Removed unused `volume_idx` field from `InFlightOp` struct
2. **Lines 6862, 6998**: Removed `volume_idx: vol_idx` from struct initialization
3. **Line 6845**: Changed `let mut buffer` to `let buffer`

## Verification

- ✅ Local `cargo check -p uffs-mft --bin uffs_mft` passes
- ✅ Full CI pipeline `rust-script scripts/ci-pipeline.rs go -v` passes
- ✅ All tests pass
- ✅ Cross-compilation for Windows succeeds
- ✅ No warnings in final build

## Commits

- `c61c3682e`: chore: development v0.2.67 - comprehensive testing complete [auto-commit]
- `a872454d5`: chore: development v0.2.68 - comprehensive testing complete [auto-commit]

