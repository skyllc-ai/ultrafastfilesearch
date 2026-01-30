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

### Run 3 - ✅ PASSED
- All tests passed
- Clippy passed
- Windows cross-compilation succeeded
- Committed and pushed as v0.2.144

---

## Additional Fix - 2026-01-29 (Continued)

### What Failed

After the extension record name merging fix, 99.8% of missing paths were recovered. However, ~120 paths remained different:
- 50 directories showing as files (no trailing backslash)
- 94 ADS entries missing
- 26 files missing

**Root Cause**: When the base record has NO `$FILE_NAME` attribute (it's entirely in the extension record), the base record parsing returned early without storing `stdinfo` (including `is_directory` flag).

This caused:
1. `is_directory = false` (default) even for directories
2. No trailing backslash added to directory paths
3. Timestamps showing as epoch (1969-12-31)

### The Fix

In `parse_record_to_index` and `parse_record_to_fragment`:

**Before**:
```rust
// Skip records without a filename
let (name, parent_frs, _namespace) = match primary_name {
    Some(n) => n,
    None => return false,  // <-- BUG: Never stores stdinfo!
};

// Set directory flag in std_info
if is_directory {
    std_info.set_directory(true);
}
```

**After**:
```rust
// Set directory flag in std_info BEFORE checking for filename
if is_directory {
    std_info.set_directory(true);
}

// Handle records without a filename in the base record
let (name, parent_frs, _namespace) = match primary_name {
    Some(n) => n,
    None => {
        // No $FILE_NAME in base record - store stdinfo anyway
        let record = index.get_or_create(frs);
        record.stdinfo = std_info;
        record.first_stream.size = SizeInfo {
            length: default_size,
            allocated: default_allocated,
        };
        return false;
    }
};
```

### Files Changed

- `crates/uffs-mft/src/io.rs`: Fixed `parse_record_to_index` and `parse_record_to_fragment`

### Expected Impact

- Fixes 50 directories showing as files (now have correct `is_directory` flag)
- Trailing backslash will be added to directory paths
- Timestamps will be correct (not epoch)

### CI Pipeline Status

- Run 5: v0.2.145 - PASSED ✅

---

## Fix 3: Remove Debug Prints from Normal Flow

### What Failed

Debug `eprintln!` statements were polluting stdout/stderr during normal operation:
```
[DEBUG] read_all_index: ENTER volume=D
[DEBUG] read_all_index: INSIDE spawn_blocking volume=D
[DEBUG] read_all_index: read_mft_index_internal done
[DEBUG] search_dataframe: before load_or_build_dataframe_cached drive=D
[DEBUG] search_dataframe: after load_or_build_dataframe_cached
```

These were left over from debugging and should only appear when tracing is enabled.

### The Fix

Converted `eprintln!("[DEBUG] ...")` to proper `tracing::trace!()` calls:
- `crates/uffs-mft/src/reader.rs`: 4 debug prints in `read_all_index`
- `crates/uffs-cli/src/commands.rs`: 2 debug prints in `search_dataframe`

---

## Fix 4: Match C++ Output Format Exactly

### What Was Different

The Rust CLI output didn't match C++ output format exactly:
1. Missing "Drives?" line AFTER the CSV data (not before)
2. Missing "MMMmmm that was FAST" message when search completes in <= 1 second
3. "Finished" message format was slightly different

### C++ Output Format (stdout)

```
"Path","Name","Path Only",...,"Attributes"

"G:\","","G:\",609893968,...
...
"G:\last\file.txt",...

Drives? 	1	G:

MMMmmm that was FAST ... maybe your searchstring was wrong?	*
Search path. E.g. 'C:/' or 'C:\Prog**'
```

### C++ Output Format (stderr)

```
Finished 	in 0 s

```

### The Fix

Modified `crates/uffs-cli/src/commands.rs`:
1. Added "Drives?" line AFTER the CSV data (to stdout, format: `\nDrives? \t{count}\t{drive_list}\n\n`)
2. Added "MMMmmm that was FAST" message when elapsed <= 1 second (to stdout)
3. Updated "Finished" message format to match C++ exactly (to stderr: `\nFinished \tin {secs} s\n`)

### CI Pipeline Status

