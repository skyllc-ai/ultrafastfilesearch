# CHANGELOG HEALING - 2026-01-29 19:00

## Issue: MFT Extension Record Merging Regression

### Summary
The Rust MFT parser was missing ~88,879 paths (3.75%) compared to the C++ implementation after recent changes. This was a regression from a previously working state where we had achieved 100% parity.

---

## 1. Issues Faced

### 1.1 Primary Issue: Missing Paths in Rust Output
- **Symptom**: The `analyze_diff` diagnostic tool reported 96.25% match rate instead of 100%
- **Impact**: 88,879 paths were missing from Rust output compared to C++
- **Initial Error**: `Error: not found: "Path" not found` - the diagnostic tool couldn't even find the Path column

### 1.2 Secondary Issues (CI Pipeline Failures)

#### 1.2.1 Clippy Linting Errors
- Semicolon placement issues in `crates/uffs-mft/src/index.rs`
- Similar issues in `parse.rs` and `dump_mft_records.rs`

#### 1.2.2 Windows Cross-Compilation Errors
- Incorrect field names on `FileNameAttribute` struct
- Used `namespace` instead of `file_name_namespace`
- Used `name_length` instead of `file_name_length`

#### 1.2.3 Borrow Checker Errors (E0499, E0502)
- In `merge_extension_into_fragment` function
- Conflicting mutable borrows when accessing `record` while iterating `fragment.links`

---

## 2. What Made These Issues Hard to Analyze

### 2.1 Subtle Regression
- The code had previously worked correctly
- The regression was introduced when `load_raw_to_index_with_options` was modified
- It used the legacy `parse_record` function instead of `parse_record_full` with `MftRecordMerger`

### 2.2 Extension Records Are Rare
- Most MFT records fit in a single 1024-byte record
- Extension records only occur when a file has many attributes (many hard links, many ADS, long file names)
- This means ~96% of files worked correctly, masking the issue

### 2.3 Silent Data Loss
- The legacy `parse_record` function:
  - Returns `None` for extension records (correct)
  - Returns base records with empty names when `$FILE_NAME` is in an extension record (incorrect)
  - These records were silently dropped or had empty paths

### 2.4 Cross-Platform Field Naming
- The `FileNameAttribute` struct has fields with `file_name_` prefix
- Easy to forget the prefix when writing new code
- Only caught during Windows cross-compilation, not macOS native builds

---

## 3. Investigation Tools Used

### 3.1 `analyze_diff` Diagnostic Tool
```bash
cargo run --release -p uffs-diag --bin analyze_diff -- \
  docs/trial_runs/UltraFastFileSearch/cpp_f.txt \
  docs/trial_runs/UltraFastFileSearch/rust_f.txt
```
- Compares C++ and Rust output files
- Reports match rates, missing paths, extra paths
- Identifies ADS entries, system files, directory entries

### 3.2 `dump_mft_records` Diagnostic Tool
- Dumps raw MFT record details for specific FRS numbers
- Helps identify which records have extension records
- Shows attribute lists and their contents

### 3.3 CI Pipeline
```bash
rust-script scripts/ci-pipeline.rs go -v
```
- Full build/test/lint/deploy workflow
- Catches cross-compilation issues
- Validates all clippy lints pass

### 3.4 Reference Documentation
- `docs/architecture/C++_resources/` - C++ source code and documentation
- `docs/architecture/Investigation/` - Previous investigation notes
- `reference/Ultra-Fast-File-Search/CPP_TEAM_QUESTIONS.md` - Questions for C++ team

---

## 4. Root Cause Analysis

### The Problem
The `load_raw_to_index_with_options` function in `crates/uffs-mft/src/reader.rs` was using:
```rust
// WRONG: Legacy function that doesn't handle extension records
if let Some(parsed) = parse_record(record_data, frs as u64) {
    // ... process record
}
```

### Why This Failed
1. `parse_record` returns `None` for extension records
2. Base records with `$ATTRIBUTE_LIST` pointing to extension records get empty `$FILE_NAME`
3. These base records are either dropped or have empty paths
4. Result: ~3.75% of files missing

### The Solution
Use `MftRecordMerger` with `parse_record_full`:
```rust
// CORRECT: Full parsing with extension record merging
let mut merger = MftRecordMerger::new();
for (frs, record_data) in records {
    if let Some(parsed) = parse_record_full(record_data, frs as u64) {
        merger.add_record(parsed);
    }
}
// Merger automatically combines extension records into base records
for (frs, merged_record) in merger.drain() {
    // ... process complete record with all attributes
}
```

---

## 5. Fixes Applied

### 5.1 Primary Fix: Use MftRecordMerger
**File**: `crates/uffs-mft/src/reader.rs`
- Updated `load_raw_to_index_with_options` to use `MftRecordMerger`
- Ensures extension records are collected and merged into base records
- Base records now get their `$FILE_NAME` attributes from extension records

### 5.2 Field Name Fixes
**File**: `crates/uffs-mft/src/io.rs` (lines 919-920, 1512-1513)
```rust
// Changed from:
if fn_attr.namespace != 2 {
    let name_len = fn_attr.name_length as usize;
// To:
if fn_attr.file_name_namespace != 2 {
    let name_len = fn_attr.file_name_length as usize;
```

### 5.3 Borrow Checker Fix
**File**: `crates/uffs-mft/src/io.rs` (lines 1630-1708)
- Restructured `merge_extension_into_fragment` function
- Chain new links/streams together first
- Get chain end indices before modifying records
- Use separate borrows for each modification

### 5.4 Clippy Fixes
- Fixed semicolon placement in `index.rs`, `parse.rs`, `dump_mft_records.rs`
- Removed unnecessary `mut` from `last_link_idx` variable

---

## 6. Results

| Metric | Before Fix | After Fix |
|--------|------------|-----------|
| Match Rate | 96.25% | **100%** |
| Missing Paths | 88,879 | **0** |
| Total Paths (C++) | 2,369,730 | 2,369,730 |
| Total Paths (Rust) | 2,280,851 | **2,369,730** |
| ADS Entries | 97,302 | **97,308** |

---

## 7. Lessons Learned

1. **Always use `MftRecordMerger`** when processing MFT records that may have extension records
2. **Test with real-world MFT files** that have complex records (many hard links, ADS)
3. **Cross-compilation catches issues** that native builds miss (field names, platform-specific code)
4. **The `analyze_diff` tool is invaluable** for detecting parity regressions
5. **Extension records are rare but critical** - they contain data for the most complex files

---

## 8. Version

- **Version**: v0.2.141
- **Commit**: `chore: development v0.2.141 - comprehensive testing complete [auto-commit]`
- **CI Pipeline**: ✅ All steps passed (1353s total)

