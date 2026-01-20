# CHANGELOG_HEALING - 2026-01-20 14:00

## Issue: MFT Reading Incomplete - Missing Records

### Symptoms
- Rust implementation reading significantly fewer MFT records than C++ (16.5M vs 25.8M)
- 3.28M paths unresolved (showing `<unknown:xxxxx>`)
- Some drives had very low match rates:
  - F: 2.9%
  - M: 17.8%
  - C: 40.6%
  - E: 41.5%
  - D: 48.3%
  - S: 84.5%

### Root Cause Analysis

Compared C++ implementation (`reference/uffs/UltraFastFileSearch-code/file.cpp`) with Rust implementation (`crates/uffs-mft/src/io.rs`).

**C++ behavior (file.cpp line 2369):**
```cpp
if (frsh->MultiSectorHeader.Magic == 'ELIF' && !!(frsh->Flags & ntfs::FRH_IN_USE))
```
- Only checks: Magic == FILE and IN_USE flag
- Does NOT skip records without $FILE_NAME attribute
- All in-use records are added to the lookup table

**Rust behavior (io.rs lines 1033-1036) - BEFORE FIX:**
```rust
// For base records, require at least one name
if primary_name.is_empty() {
    return ParseResult::Skip;
}
```
- Skipped records without $FILE_NAME attribute
- This caused parent directories to be missing from the lookup table
- Child files couldn't resolve their paths → `<unknown:xxxxx>`

### Fix Applied

Modified `parse_record_full()` in `crates/uffs-mft/src/io.rs`:

**BEFORE:**
```rust
// For base records, require at least one name
if primary_name.is_empty() {
    return ParseResult::Skip;
}
```

**AFTER:**
```rust
// For base records without a name, use a placeholder
// This ensures all in-use records are included in the DataFrame for path resolution
// (matching C++ behavior which does NOT skip records without $FILE_NAME)
if primary_name.is_empty() {
    primary_name = format!("<unnamed:{frs}>");
}
```

### Verification

- `cargo check --package uffs-mft` - ✅ Compiles
- `cargo test --package uffs-mft` - ✅ All 15 tests pass
- `cargo test --workspace` - ✅ All tests pass
- `cargo clippy --package uffs-mft` - ✅ No warnings

### Expected Impact

This fix should:
1. Include all in-use MFT records in the DataFrame
2. Ensure parent directories are in the path resolver lookup table
3. Significantly reduce `<unknown:xxxxx>` paths
4. Improve match rate between Rust and C++ output

### Next Steps

1. Rebuild profiling binaries with this fix
2. Run on Windows to generate new output
3. Compare with C++ output to verify improvement

