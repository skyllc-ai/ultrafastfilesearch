# IOCP Tree Size Bug Investigation - 2026-03-18

## Executive Summary

After fixing the `StandardInfo::from_extended()` attribute flag bugs (Bug 2 & 3), we're left with **66 parity differences** on Drive D, all related to **directory tree sizes (`treesize`)**.

The most dramatic case: `D:\temp_test\JG6eTsoCnK\` shows:
- **Baseline (C++)**: `treesize = 19,137,152` bytes (~19MB)
- **Rust (IOCP path)**: `treesize = 640` bytes

Yet `tree_allocated` is **identical** in both: `19,136,512` bytes.

## Current Status

| Metric | Before Fixes | After Fixes | Target |
|--------|--------------|-------------|--------|
| Total Differences | 437 | **66** | 0 |
| Attribute Flag Mismatches | ~350 | 0 ✅ | 0 |
| Tree Size Mismatches | ~87 | **66** | 0 |

## Bug Symptoms

### 1. Tree Size vs Tree Allocated Mismatch

For directory `D:\temp_test\JG6eTsoCnK\`:
```
BASELINE: treesize=19137152, tree_allocated=19136512, descendants=50001
RUST:     treesize=640,      tree_allocated=19136512, descendants=50001
```

Key observations:
- `descendants` count is **IDENTICAL** (50,001) - children ARE being linked
- `tree_allocated` is **IDENTICAL** (~19MB) - allocated sizes ARE being summed
- `treesize` is **WRONG** (640 bytes vs 19MB) - logical sizes NOT being summed

### 2. Individual Files Have Size=0 in Both

```bash
$ grep "JG6eTsoCnK" /Users/rnio/uffs_data/drive_d/cpp_d.txt | head -5
"D:\temp_test\JG6eTsoCnK\file_46748.txt","file_46748.txt","D:\temp_test\JG6eTsoCnK\",0,0,...

$ grep "JG6eTsoCnK" /Users/rnio/uffs_data/drive_d/verify_rust_d.txt | head -5  
"D:\temp_test\JG6eTsoCnK\file_9210.txt","file_9210.txt","D:\temp_test\JG6eTsoCnK\",0,0,...
```

Both C++ and Rust report individual files as `Size=0, Allocated=0`. Yet C++ sums to 19MB treesize somehow.

### 3. Math Analysis

```
tree_allocated = 19,136,512 bytes
descendants = 50,001
Per-file average = 19,136,512 / 50,001 ≈ 383 bytes

treesize = 19,137,152 bytes  
difference = 19,137,152 - 19,136,512 = 640 bytes (exactly the directory's own size!)
```

This suggests C++ is summing something ~383 bytes per file that Rust is not.

## Code Paths

### OFFLINE Path (WORKING - used for raw .bin files)
```
parse_record_full()
    → MftRecordMerger.merge()
    → MftIndex::from_parsed_records()
        → Uses StandardInfo::from_extended() [FIXED]
        → Child entries via builder.rs line 333-368
    → compute_tree_metrics()
```

### IOCP/LIVE Path (BUGGY - used for Windows LIVE and IOCP replay)
```
parse_record_to_index()  [crates/uffs-mft/src/io/parser/index.rs]
    → Uses StandardInfo::from_extended() [FIXED]
    → Child entries via add_child_entry() [line 819]
    → parse_extension_to_index() for extension records
        → Child entries [line 785-794]
compute_tree_metrics()  [crates/uffs-mft/src/tree_metrics.rs]
```

## Fixes Already Applied

### Fix 1: StandardInfo::from_extended() [COMPLETE]
**File**: `crates/uffs-mft/src/index/types.rs` lines 79-157

Created canonical two-step conversion:
1. `ExtendedStandardInfo::from_attributes()` - complete flag parsing
2. `StandardInfo::from_extended()` - single source of truth conversion

### Fix 2: Extension Record name_index Calculation [COMPLETE]
**File**: `crates/uffs-mft/src/io/parser/index_extension.rs` lines 777-783

Fixed off-by-one error:
```rust
// BEFORE (buggy):
existing_name_count - 1 + name_idx as u16

// AFTER (fixed):
existing_name_count + name_idx as u16
```

### Fix 3: Converted eprintln! to tracing::debug! [COMPLETE]
Production lint compliance - all debug statements now use proper tracing.

## Remaining Bug: Tree Metrics Aggregation

### The Core Mystery

In `tree_metrics.rs` lines 333-335:
```rust
children.length = children.length.saturating_add(child_agg.length);
children.allocated = children.allocated.saturating_add(child_agg.allocated);
```

Both use the **same** `child_agg` from `preprocess()`. If `children.allocated` is correct (19MB) but `children.length` is wrong (0), the only explanation is:

**`child_agg.length = 0` but `child_agg.allocated ≈ 383` for each child.**

This means the children have `first_stream.size.length = 0` but `first_stream.size.allocated > 0`.

### Hypothesis

The C++ code may be counting something in `length` that Rust is not:
1. MFT record overhead?
2. $INDEX_ROOT size for files?
3. Some other attribute's ValueLength?

## Key Files to Examine

### C++ Reference Implementation

**Documentation:**
- `/Users/rnio/Private/Github/UltraFastFileSearch/_trash/untracked/docs/`

**Source Code:**
- `/Users/rnio/Private/Github/UltraFastFileSearch/_trash/UltraFastFileSearch-code/`

**Critical C++ Files:**
- `_trash/UltraFastFileSearch-code/src/index/ntfs_index_impl.hpp` - Main index implementation
- `_trash/UltraFastFileSearch-code/src/index/ntfs_index_load.hpp` - MFT loading and tree building

**Key C++ Code Sections:**

1. **Child Entry Creation** (ntfs_index_load.hpp lines 291-307):
```cpp
// Build parent-child relationship
child_info->name_index = base_record->name_count;  // BEFORE increment
++base_record->name_count;  // AFTER adding child entry
```

2. **Size Calculation** (ntfs_index_load.hpp lines 441-452):
```cpp
info->allocated += ah->IsNonResident
    ? (ah->NonResident.CompressionUnit
        ? static_cast<file_size_type>(ah->NonResident.CompressedSize)
        : static_cast<file_size_type>(ah->NonResident.AllocatedSize))
    : 0;  // <-- Resident files get allocated=0

info->length += ah->IsNonResident
    ? static_cast<file_size_type>(ah->NonResident.DataSize)
    : ah->Resident.ValueLength;  // <-- Resident files get ValueLength
```

3. **Preprocessor name_info** (ntfs_index_impl.hpp lines 560-562):
```cpp
PreprocessResult const subresult = this->operator()(fr2,
    fr2->name_count - static_cast<size_t>(1) - i->name_index,  // name_info calculation
    fr2->name_count);
```

4. **Delta Distribution** (ntfs_index_impl.hpp lines 601-617):
```cpp
static unsigned long long delta(unsigned long long const value,
    unsigned short const i, unsigned short const n)
{
    return value * (i + 1) / n - value * i / n;
}
```

### Rust Implementation

**Tree Metrics:**
- `crates/uffs-mft/src/tree_metrics.rs` - Main tree aggregation logic

**IOCP/LIVE Path (Buggy):**
- `crates/uffs-mft/src/io/parser/index.rs` - `parse_record_to_index()`
- `crates/uffs-mft/src/io/parser/index_extension.rs` - `parse_extension_to_index()`
- `crates/uffs-mft/src/parse/index_helpers.rs` - `add_child_entry()`

**OFFLINE Path (Working):**
- `crates/uffs-mft/src/index/builder.rs` - `MftIndex::from_parsed_records()`
- `crates/uffs-mft/src/parse/merger.rs` - `MftRecordMerger`

**IOCP Replay:**
- `crates/uffs-mft/src/raw_iocp.rs` - `load_iocp_to_index()`

**Index Types:**
- `crates/uffs-mft/src/index/types.rs` - `StandardInfo`, `FileRecord`, `ChildInfo`
- `crates/uffs-mft/src/index/base.rs` - `MftIndex` core structure

## Testing & Verification Tools

### Parity Verification Script
```bash
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
```

This script:
1. Builds fresh release binary
2. Runs `uffs scan` on IOCP capture
3. Compares output to C++ baseline (`cpp_d.txt`)
4. Reports differences

### Test Data Location
- `/Users/rnio/uffs_data/drive_d/D_mft.iocp` - IOCP capture (407.9 MB)
- `/Users/rnio/uffs_data/drive_d/cpp_d.txt` - C++ baseline output (7,065,517 lines)
- `/Users/rnio/uffs_data/drive_d/verify_rust_d.txt` - Rust output for comparison

### Quick Grep Commands
```bash
# Check specific directory in Rust output
grep "JG6eTsoCnK" /Users/rnio/uffs_data/drive_d/verify_rust_d.txt | head -20

# Check same directory in C++ baseline
grep "JG6eTsoCnK" /Users/rnio/uffs_data/drive_d/cpp_d.txt | head -20

# Compare root directory metrics
head -10 /Users/rnio/uffs_data/drive_d/cpp_d.txt
head -10 /Users/rnio/uffs_data/drive_d/verify_rust_d.txt
```

## Tree Metrics Algorithm Deep Dive

### Rust tree_metrics.rs Key Sections

**Aggregation Loop** (lines 306-340):
```rust
let mut children = Agg::default();
if is_directory {
    let mut child_entry_idx = first_child;
    while child_entry_idx != NO_ENTRY {
        let (child_frs, child_name_idx, next_child_entry) = {
            let ce = &self.index.children[child_entry_idx as usize];
            (ce.child_frs, ce.name_index, ce.next_entry)
        };

        if let Some(child_idx) = self.index.frs_to_idx_opt(child_frs) {
            let child_total_names = u32::from(self.index.records[child_idx].name_count);
            let child_name_info = compute_name_info_checked(
                u32::from(child_name_idx),
                child_total_names,
                child_frs,
                self.debug,
            );

            let child_agg = self.preprocess(child_idx, child_name_info, child_total_names.max(1));

            children.length = children.length.saturating_add(child_agg.length);     // <-- BUG: This is 0
            children.allocated = children.allocated.saturating_add(child_agg.allocated); // <-- OK: This is correct
            children.treesize = children.treesize.saturating_add(child_agg.treesize);
        }
        child_entry_idx = next_child_entry;
    }
}
```

**Own Size Calculation** (lines 342-345):
```rust
let mut own_len = delta(first_len, name_info, total_names);
let mut own_alloc = delta(first_alloc, name_info, total_names);
```

**Where first_len and first_alloc come from** (lines 285-295):
```rust
let (
    is_directory, first_child, first_stream_next, first_internal_stream,
    total_stream_count, first_len, mut first_alloc, reparse_tag,
) = {
    let rec = &self.index.records[record_idx];
    (
        rec.stdinfo.is_directory(),
        rec.first_child,
        rec.first_stream.next_entry,
        rec.first_internal_stream,
        rec.total_stream_count,
        rec.first_stream.size.length,    // <-- first_len
        rec.first_stream.size.allocated, // <-- first_alloc
        rec.reparse_tag,
    )
};
```

**Directory Output** (lines 396-397):
```rust
rec.treesize = children.length.saturating_add(first_len);
rec.tree_allocated = children.allocated.saturating_add(first_alloc);
```

### The Bug Pattern

For files in `JG6eTsoCnK`:
- `first_stream.size.length = 0` (empty files, correct)
- `first_stream.size.allocated = ???` (should be 0 for empty resident files)

But C++ produces `treesize = 19MB` somehow, even though individual files show `Size=0`.

**Hypothesis**: C++ may be counting something in addition to `$DATA` stream size that Rust is not:
1. Some internal attribute's ValueLength?
2. MFT record slack space?
3. $INDEX_ALLOCATION for directories containing these files?

## Questions for Investigation

1. **What does C++ count in `info->length` that produces 19MB?**
   - Individual files show `Size=0` in output
   - But `treesize` sums to 19MB
   - Where does the 383 bytes/file average come from?

2. **Why is `tree_allocated` correct but `treesize` wrong?**
   - Both use the same aggregation loop
   - Both use the same `child_agg` from `preprocess()`
   - Only difference is input: `first_len` vs `first_alloc`

3. **Is there a difference in how resident vs non-resident files are handled?**
   - C++: `info->length += ah->Resident.ValueLength` for resident
   - Rust: `first_stream.size.length` from inline parser

4. **Is there something special about the `JG6eTsoCnK` directory?**
   - 50,001 empty test files
   - All have `Size=0, Allocated=0`
   - Yet C++ sums to 19MB treesize

## Reproduction Steps

1. Ensure release build is current:
   ```bash
   cargo build --release -p uffs-cli
   ```

2. Run parity verification:
   ```bash
   rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
   ```

3. Check specific differences:
   ```bash
   grep "JG6eTsoCnK" /Users/rnio/uffs_data/drive_d/verify_rust_d.txt | wc -l
   # Should show ~50,002 lines (directory + 50,001 files)
   ```

## Environment

- **Platform**: macOS (cross-compiling for Windows)
- **Rust Version**: See `rust-toolchain.toml`
- **MFT Source**: IOCP capture from Windows D: drive
- **Total Records**: ~7 million

## Related Documents

- `docs/architecture/Investigation/TREE_METRICS_PARITY_ANALYSIS.md`
- `docs/architecture/Investigation/UFFS_TREE_METRICS_PARITY_DEEP_DIVE.md`
- `docs/architecture/tree-metrics-algorithm-question.md`

## What We've Ruled Out

1. **Attribute flag parsing** - Fixed via `StandardInfo::from_extended()`, no longer a factor
2. **Extension record name_index calculation** - Fixed the off-by-one, reduced diffs from 89 to 66
3. **Child entry linking** - Descendant counts match exactly (50,001 vs 50,001)
4. **Tree walk logic** - Both `length` and `allocated` use the same loop, `allocated` is correct
5. **Delta distribution function** - Rust matches C++ exactly: `value * (i+1) / n - value * i / n`
6. **name_info calculation** - Rust matches C++: `name_count - 1 - name_index`

## What Still Needs Investigation

1. **Why do children have `first_stream.size.length = 0` but `first_stream.size.allocated > 0`?**
   - This is the direct cause of the bug
   - `parse_record_to_index` must be setting these differently than C++

2. **What are the 50,001 files in `JG6eTsoCnK`?**
   - Are they truly empty (0 bytes)?
   - Or do they have some content that Rust isn't capturing?

3. **Is there an internal stream or attribute being counted by C++ but not Rust?**
   - `$STANDARD_INFORMATION` has some size
   - `$FILE_NAME` attributes have size
   - These might contribute to tree aggregation in C++

## Appendix: Raw Parity Output Sample

```
=== SORTED SIDE-BY-SIDE COMPARISON (differences only) ===
  Baseline lines: 7065517
  Rust lines:     7065517
  Lines that differ: 66

--- FIRST 5 DIFFERENCES ---
  Line 5:
    BASELINE: "D:\","","D:\",5252335107531,5120051462144,...,14762408,...
    RUST:     "D:\","","D:\",5229396511000,5119999246336,...,14762404,...
  Line 76461:
    BASELINE: "D:\Dropbox\","","D:\Dropbox\",2808178863552,2728612118528,...,8305040,...
    RUST:     "D:\Dropbox\","","D:\Dropbox\",2808173063437,2728612118528,...,8305040,...
  ...
  Line 6857798:
    BASELINE: "D:\temp_test\JG6eTsoCnK\","","D:\temp_test\JG6eTsoCnK\",19137152,19136512,...,50001,...
    RUST:     "D:\temp_test\JG6eTsoCnK\","","D:\temp_test\JG6eTsoCnK\",640,19136512,...,50001,...
```

Note: Column format is `Path,Name,ParentPath,Size,Allocated,Created,Modified,Accessed,Descendants,...`

## Contact

Investigation conducted: 2026-03-18
Last parity run: 66 differences remaining (down from 437)

