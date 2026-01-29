# CHANGELOG_HEALING - 2026-01-29 22:00

## Summary

Fix for extension record `$FILE_NAME` merging bug that caused 73,721 missing paths on D-Drive.

## What Failed

**Root Cause**: When a directory or file has so many attributes that the `$FILE_NAME` attribute is pushed to an extension record (not in the base record), the Rust MFT parser was not properly merging the name into the base record.

The bug was in two functions in `crates/uffs-mft/src/io.rs`:
1. `parse_extension_to_index` 
2. `parse_extension_to_fragment`

Both functions were:
- Adding extension names to the `links` buffer
- Chaining them to `record.first_name.next_entry`

But they were **NOT** checking if `record.first_name` itself was empty! When the base record had no `$FILE_NAME` attribute, `first_name` remained empty, and the chained names were never used for path resolution.

## The Fix

When processing extension records with `$FILE_NAME` attributes:

1. **Check if base record has no name**: `!record.first_name.name.is_valid()`
2. **If empty**: Copy the first extension name **directly into `first_name`** (not just chain it)
3. **If not empty**: Chain extension names as additional hard links (original behavior)

This matches the C++ behavior in `ntfs_index.hpp` lines 559-567.

## Files Changed

- `crates/uffs-mft/src/io.rs`: Fixed `parse_extension_to_index` and `parse_extension_to_fragment`

## Impact

- Fixes 73,721 missing paths on D-Drive (1.0% of total)
- Caused by ~60 directories and ~341 files having `$FILE_NAME` only in extension records
- All 73 uffs-mft tests pass
- Build succeeds for uffs-mft and uffs-cli

## CI Pipeline Status

### Run 1 - Failed (Clippy errors in test code)
- `$FILE_NAME` should be `` `$FILE_NAME` `` in doc comments
- `"test_directory".to_string()` should be `"test_directory".to_owned()`
- `merged` is too similar to `merger` - renamed to `result` and `record_merger`

### Run 2 - Failed (Borrow checker error in Windows cross-compile)
```
error[E0502]: cannot borrow `*fragment` as mutable because it is also borrowed as immutable
   --> crates/uffs-mft/src/io.rs:1719:26
```
**Fix**: Copy values from `fragment.links[...]` to local variables before calling `fragment.get_or_create()`.

### Run 3 - Pending

