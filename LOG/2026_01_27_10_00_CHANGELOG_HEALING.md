# CHANGELOG_HEALING - 2026-01-27 10:00

## Summary
Fixed CI pipeline failures after implementing P3 forensic fields for MFT field enhancements.

## Context
Implementing Priority 3 (P3) forensic fields from `docs/architecture/uffs-mft-field-enhancements.md`:
- `is_deleted` - Deleted file records
- `is_corrupt` - Corrupted records
- `is_extension` - Extension records
- `base_frs` - Base FRS for extension records

## Issues Fixed

### 1. Test `test_file_record_size` Failure
**File:** `crates/uffs-mft/src/index.rs`
**What failed:** Test expected `FileRecord` size ≤ 216 bytes, but adding `base_frs: u64` field increased it to 224 bytes.
**Root cause:** The `base_frs` field (8 bytes) was added to `FileRecord` struct for P3 forensic support.
**Fix:** Updated test assertion from 216 to 224 bytes to reflect the new struct size.

### 2. Clippy `doc_markdown` Warnings (4 occurrences)
**Files:** `crates/uffs-mft/src/raw.rs`, `crates/uffs-mft/src/main.rs`
**What failed:** Doc comments referenced field names like `is_deleted`, `is_corrupt`, `is_extension`, `base_frs` without backticks.
**Root cause:** Clippy requires code identifiers in doc comments to be wrapped in backticks.
**Fix:** Wrapped all field names in backticks in doc comments.

### 3. Clippy `struct_excessive_bools` Warning
**File:** `crates/uffs-mft/src/parse.rs`
**What failed:** `ParsedRecord` struct has more than 3 boolean fields.
**Root cause:** P3 forensic fields added `is_deleted`, `is_corrupt`, `is_extension` booleans to the struct.
**Fix:** Added targeted `#[allow(clippy::struct_excessive_bools)]` with justification - these are distinct semantic flags from MFT record parsing, not a state machine.

### 4. Clippy `cognitive_complexity` Warning
**File:** `crates/uffs-mft/src/parse.rs`
**What failed:** `parse_record_forensic` function exceeded complexity threshold.
**Root cause:** Function handles multiple forensic parsing paths with necessary branching.
**Fix:** Added targeted `#[allow(clippy::cognitive_complexity)]` - the complexity is inherent to MFT record parsing requirements.

### 5. Clippy `too_many_lines` Warning
**File:** `crates/uffs-mft/src/index.rs`
**What failed:** `to_dataframe` function exceeded line count threshold.
**Root cause:** Function builds DataFrame with many columns including new forensic fields.
**Fix:** Added targeted `#[allow(clippy::too_many_lines)]` - the function is a single logical unit for DataFrame construction.

### 6. Clippy `undocumented_unsafe_blocks` Warnings (2 occurrences)
**File:** `crates/uffs-mft/src/parse.rs`
**What failed:** Unsafe blocks lacked SAFETY comments.
**Root cause:** Unsafe pointer operations for MFT record parsing were missing documentation.
**Fix:** Added SAFETY comments explaining why each unsafe operation is sound.

### 7. Compilation Error - Missing `NameInfo` Fields
**File:** `crates/uffs-mft/src/reader.rs`
**What failed:** `NameInfo` struct initialization missing `fn_created`, `fn_modified`, `fn_accessed`, `fn_mft_changed` fields.
**Root cause:** P2 added `$FILE_NAME` timestamp fields to `NameInfo` but `to_dataframe` wasn't updated.
**Fix:** Added missing timestamp fields with `None` values in the struct initialization.

### 8. Compilation Error - Missing `StreamInfo` Field
**File:** `crates/uffs-mft/src/reader.rs`
**What failed:** `StreamInfo` struct initialization missing `is_resident` field.
**Root cause:** P2 added `is_resident` field to `StreamInfo` but `to_dataframe` wasn't updated.
**Fix:** Added `is_resident: false` to the struct initialization.

## Verification
- ✅ `cargo check -p uffs-mft` - passed
- ✅ `cargo clippy --all-targets --all-features -- -D warnings` - passed
- ✅ `rust-script scripts/ci-pipeline.rs go -v` - passed (exit code 0)
- ✅ All binaries built successfully (uffs, uffs_mft, uffs_tui, uffs_gui)
- ✅ Cross-platform build for Windows x64 completed
- ✅ Git push to remote successful

## Version
v0.2.109

