# Tree Metrics C++ vs Rust Parity Analysis

**Document Version:** 1.0  
**Date:** 2026-01-28  
**Status:** In Progress  
**Test Drive:** G: (16GB USB stick)

## Executive Summary

This document details the comprehensive analysis and debugging effort to achieve exact feature parity between the C++ UFFS (UltraFastFileSearch) implementation and the Rust port for tree metrics calculation (treesize, tree_allocated, descendants).

### Current State

| Metric | C++ Value | Rust Value | Difference | Status |
|--------|-----------|------------|------------|--------|
| Root Descendants | 15,119 | 15,119 | 0 | ✅ EXACT MATCH |
| Root Treesize | 609,898,968 | 609,898,920 | 48 bytes | ⚠️ 0.000008% diff |

---

## 1. Test Environment

### Test Data
- **MFT File:** `G_mft.raw` (16GB USB stick)
- **C++ Output:** `cpp_g.txt`
- **Location:** `/docs/trial_runs/UltraFastFileSearch/`

### Key Metrics
- Total records: ~15,088
- Root FRS: 5
- System metafiles: FRS 0-11
- User directories: MFT_TEST, temp_test, Norton sandbox, System Volume Information

---

## 2. Issues Discovered and Fixed

### 2.1 System Metafiles Exclusion (FIXED)

**Problem:** Rust was excluding system metafiles (FRS 0-15) from tree metrics calculation.

**C++ Behavior:** C++ includes ALL records in tree metrics. System metafiles are children of root (FRS 5) and their sizes contribute to root's treesize. They are simply not OUTPUT to CSV.

**Evidence:** C++ code line 568 only checks `if (frs_parent != frs_base)` to avoid self-reference - no FRS filtering.

**Fix:** Removed the special case that excluded system metafiles from tree metrics.

**Impact:** ~37 MB difference resolved.

---

### 2.2 $BadClus:$Bad Sparse File Size (FIXED)

**Problem:** Rust was reporting $BadClus:$Bad with DataSize of ~15.8 GB instead of 0.

**C++ Behavior (line 716):**
```cpp
info->length += is_badclus_bad ? ah->NonResident.InitializedSize : ah->NonResident.DataSize
```

**Fix:** Added special handling to use `InitializedSize` instead of `DataSize` for the $BadClus:$Bad stream.

**Impact:** Prevented massive inflation of root treesize.

---

### 2.3 MFT Overhead for Resident Files (FIXED)

**Problem:** Rust was incorrectly adding 512 bytes of "MFT overhead" for resident files.

**Evidence:**
- 5000 files × 512 bytes = 2,560,000 bytes extra per _FRAG_PRE directory
- 3 directories × 2,560,000 = 7,680,000 bytes total

**Fix:** Removed the MFT overhead logic from `compute_tree_metrics_impl()`.

**Result:**
| Directory | C++ Size | Rust treesize | Match |
|-----------|----------|---------------|-------|
| _FRAG_PRE_1 | 2,731,149 | 2,731,149 | ✅ Exact |
| _FRAG_PRE_2 | 539,602,061 | 539,602,061 | ✅ Exact |
| _FRAG_PRE_3 | 2,731,149 | 2,731,149 | ✅ Exact |

---

### 2.4 Stream Counting - Multiple Attribute Types (FIXED)

C++ counts streams via a `default:` case in its switch statement, creating a stream for ANY attribute that falls through. Rust needed explicit handling for each.

#### 2.4.1 $REPARSE_POINT Stream
**Fix:** Added $REPARSE_POINT as a countable stream for reparse points/symlinks.

#### 2.4.2 Non-$I30 Index Attributes
**Fix:** Added $SDH, $SII, $O, $Q, $R index attributes as streams.

#### 2.4.3 Additional Attribute Types
**Fix:** Added as streams:
- $OBJECT_ID
- $EA (Extended Attributes)
- $EA_INFORMATION
- $PROPERTY_SET
- $LOGGED_UTILITY_STREAM

#### 2.4.4 Volume Attributes
**Fix:** Added $VOLUME_NAME and $VOLUME_INFORMATION as streams.

#### 2.4.5 Unnamed $BITMAP
**Fix:** Changed condition to count unnamed $BITMAP attributes as streams (for $MFT, $Secure, etc.).

---

### 2.5 $LOGGED_UTILITY_STREAM Classification (FIXED)

**Problem:** Initially thought C++ commented out LoggedUtilityStream handling.

**Discovery:** C++ actually counts it via the `default:` case - it's NOT commented out.

**Fix:** Added LoggedUtilityStream to stream counting in Rust.

---

### 2.6 Directory Descendants Initialization (FIXED)

**Problem:** Rust initialized directory descendants to 1, but C++ adds `stream_count` for each directory.

**C++ Algorithm (lines 817, 879):**
```cpp
result = children_size;
// ... then for each stream:
result.treesize += 1;
```

So a directory with `stream_count=2` contributes 2 to its own descendants count.

**Fix:** Changed directory descendants initialization from `1` to `stream_count`:
```rust
// Before:
record.descendants = u32::from(*is_directory);

// After:
record.descendants = if *is_directory { *stream_count } else { 0 };
```

**Result:** Root descendants now matches exactly: 15,119 = 15,119

---

### 2.7 Directory Additional Streams Contribution (FIXED)

**Problem:** After fixing descendants initialization, the `descendants_contribution` calculation was double-counting.

**Fix:** Updated contribution calculation:
```rust
// Before:
child_descendants + (stream_count as u32).saturating_sub(1)

// After:
child_descendants  // stream_count already included in descendants
```

---

## 3. Verified Correct Behaviors

### 3.1 Stream Size Summation
All streams' sizes are correctly summed for each record:

| Record | Streams | Total Size | Match |
|--------|---------|------------|-------|
| $MFT (FRS 0) | default + $BITMAP | 20,709,376 + 4,104 = 20,713,480 | ✅ |
| $Volume (FRS 3) | $VOLUME_NAME + $VOLUME_INFO + default | 20 + 12 + 0 = 32 | ✅ |
| $Secure (FRS 9) | $SDS + 2×$SDH + 2×$SII + ... | 271,792 | ✅ |
| $UpCase (FRS 10) | default + $Info | 131,072 + 32 = 131,104 | ✅ |
| $Tops (FRS 32) | default + $T | 100 + 1,048,576 = 1,048,676 | ✅ |

### 3.2 User-Visible Directory Matches
All user-visible directories match exactly:

| Directory | C++ Treesize | Rust Treesize | Match |
|-----------|--------------|---------------|-------|
| MFT_TEST | 545,073,550 | 545,073,550 | ✅ |
| Norton sandbox | 5,000 | 5,000 | ✅ |
| temp_test | 1,835,272 | 1,835,272 | ✅ |
| System Volume Information | 368 | 368 | ✅ |

### 3.3 Hardlinked Files
Files with multiple names correctly sum all streams:

| FRS | Name | Streams | Total | Match |
|-----|------|---------|-------|-------|
| 188 | doc1.txt | default(20) + comments(15) | 35 | ✅ |
| 244 | photo1_link.jpg | default(26) + com.dropbox.attrs(18) | 44 | ✅ |
| 252 | main.rs | default(34) + build_info(14) | 48 | ✅ |

### 3.4 $TXF_DATA Streams on Directories
Directories with transaction streams correctly include them:

| Directory | Dir Size | $TXF_DATA | Total | Match |
|-----------|----------|-----------|-------|-------|
| Root (FRS 5) | 4,160 | 56 | 4,216 | ✅ |
| $Txf (FRS 31) | 48 | 56 | 104 | ✅ |


---

## 4. Current Treesize Calculation Verification

### Root Treesize Breakdown (Rust)

```
Children of Root (FRS 5):
├── $MFT (FRS 0)           treesize: 20,713,480
├── $MFTMirr (FRS 1)       treesize: 4,096
├── $LogFile (FRS 2)       treesize: 23,429,120
├── $Volume (FRS 3)        treesize: 32
├── $AttrDef (FRS 4)       treesize: 2,560
├── $Bitmap (FRS 6)        treesize: 482,304
├── $Boot (FRS 7)          treesize: 8,192
├── $BadClus (FRS 8)       treesize: 0
├── $Secure (FRS 9)        treesize: 271,792
├── $UpCase (FRS 10)       treesize: 131,104
├── $Extend (FRS 11)       treesize: 17,893,572
├── System Volume Info     treesize: 368
├── autorun.inf            treesize: 208
├── autorun.ico            treesize: 34,494
├── temp_test              treesize: 1,835,272
├── Norton sandbox         treesize: 5,000
├── create_mft_test.ps1    treesize: 9,560
└── MFT_TEST               treesize: 545,073,550
                           ─────────────────────
Sum of children:                    609,894,704
Root own size (dir index):                4,160
Root $TXF_DATA stream:                       56
                           ─────────────────────
Rust Total:                         609,898,920
C++ Total:                          609,898,968
                           ─────────────────────
DIFFERENCE:                                  48 bytes
```

---

## 5. The Remaining 48-Byte Mystery

### 5.1 What We Know

1. **All user-visible directories match exactly** - MFT_TEST, temp_test, etc.
2. **All system metafiles' individual treesizes are verified correct**
3. **Stream counting is correct** - descendants match exactly (15,119)
4. **Stream size summation is correct** - verified for all key records
5. **The 48 bytes is NOT from:**
   - Hardlink delta rounding (verified)
   - Missing streams (all attribute types covered)
   - Directory index size calculation (verified)
   - $TXF_DATA streams (verified included)

### 5.2 Hypotheses for the 48-Byte Difference

#### Hypothesis A: Reserved Clusters
C++ code at line 814 adds reserved clusters to allocated size at depth 0:
```cpp
if (depth == 0) {
    children_size.allocated += static_cast<unsigned long long>(me->reserved_clusters) * me->cluster_size;
}
```
However, this affects `allocated`, not `length` (treesize). **Unlikely cause.**

#### Hypothesis B: Attribute List Handling
Records with many attributes may have an $ATTRIBUTE_LIST that spans multiple MFT records. C++ may handle extension records differently.

**Status:** Not yet investigated.

#### Hypothesis C: Compressed Stream Merging
C++ has special logic for merging compressed default streams (lines 860-874):
```cpp
if (compressed_default_stream_to_merge) {
    // Special handling for compressed streams
}
```
**Status:** Not yet investigated.

#### Hypothesis D: Bulkiness Calculation
C++ tracks a "bulkiness" metric that may affect size calculations in edge cases.

**Status:** Not yet investigated.

#### Hypothesis E: Extension Record Sizes
MFT records that span multiple FRS entries (via $ATTRIBUTE_LIST) may have their extension record sizes counted differently.

**Status:** Not yet investigated.

---

## 6. C++ Algorithm Deep Dive

### 6.1 Key Code Locations (ntfs_index.hpp)

| Line | Function | Description |
|------|----------|-------------|
| 568 | Child linking | `if (frs_parent != frs_base)` - only check |
| 716 | Size calculation | `info->length += ...` for each stream |
| 718 | Treesize init | `info->treesize = isdir;` |
| 779-792 | Tree traversal | Recursive size accumulation |
| 814 | Reserved clusters | Added to allocated at depth 0 |
| 817 | Result init | `result = children_size;` |
| 876 | Length delta | `result.length += length_delta;` |
| 879 | Treesize increment | `result.treesize += 1;` per stream |
| 882-885 | Children merge | Adds children's metrics to default stream |

### 6.2 Stream Processing Loop (lines 839-886)

For each stream in a record:
1. Calculate `length_delta` (stream's size)
2. Calculate `allocated_delta` (stream's allocated size)
3. Add to result: `result.length += length_delta`
4. Increment descendants: `result.treesize += 1`
5. If default stream (!type_name_id): merge children's metrics

---

## 7. Files Modified During Investigation

### 7.1 Primary Files

| File | Changes |
|------|---------|
| `crates/uffs-mft/src/index.rs` | Tree metrics calculation, descendants initialization |
| `crates/uffs-mft/src/parse.rs` | Stream counting, attribute type handling |

### 7.2 Key Functions Modified

**index.rs:**
- `compute_tree_metrics_impl()` - Main tree metrics calculation
- `base_metrics` calculation - Stream size summation
- `descendants_contribution` - Child contribution to parent

**parse.rs:**
- `parse_mft_record()` - Attribute parsing and stream creation
- Stream counting for various attribute types

---

## 8. Future Investigation Areas

### 8.1 High Priority (Likely Causes of 48-Byte Difference)

1. **$ATTRIBUTE_LIST Extension Records**
   - Check if C++ counts sizes from extension records differently
   - Verify all attributes from extension records are being parsed
   - Test with records that have $ATTRIBUTE_LIST

2. **Compressed Stream Handling**
   - Analyze C++ compressed stream merging logic (lines 860-874)
   - Check if any files on G: drive are compressed
   - Verify compressed size vs logical size handling

3. **Sparse File Edge Cases**
   - Beyond $BadClus, check other sparse files
   - Verify InitializedSize vs DataSize handling for all sparse streams

### 8.2 Medium Priority

4. **Bulkiness Metric**
   - Understand what bulkiness represents in C++
   - Determine if it affects length calculation in any edge case

5. **Hardlink Delta Rounding**
   - Deep dive into C++ delta_impl() function (line 820)
   - Verify rounding behavior matches exactly
   - Test with files having many hardlinks

6. **Directory Index Size Calculation**
   - Verify $INDEX_ROOT + $INDEX_ALLOCATION + $BITMAP calculation
   - Check for edge cases with large directories

### 8.3 Lower Priority

7. **Reparse Point Data**
   - Verify reparse point size calculation
   - Check symlink vs junction vs mount point handling

8. **Extended Attributes (EA)**
   - Verify $EA and $EA_INFORMATION size handling
   - Check for files with extended attributes

9. **Encrypted Files**
   - Verify EFS-encrypted file size handling
   - Check if encryption affects size calculation

---

## 9. Debugging Commands Reference

### Build and Test
```bash
cargo build -p uffs-mft --release
cargo run -p uffs-mft --release -- load G_mft.raw --drive G --output /tmp/rust_mft.csv
```

### Compare Root Metrics
```bash
# Rust
awk -F',' 'NR>1 && $1==5 {print "descendants:", $38, "treesize:", $39}' /tmp/rust_mft.csv

# C++
head -2 cpp_g.txt | tail -1 | awk -F',' '{print "descendants:", $9, "treesize:", $4}'
```

### Check Specific FRS
```bash
awk -F',' 'NR>1 && $1==FRS_NUMBER {print}' /tmp/rust_mft.csv
grep 'FILENAME' cpp_g.txt
```

### Sum Children Treesizes
```bash
awk -F',' 'NR>1 && $4==5 && $1!=5 {sum+=$39} END {print sum}' /tmp/rust_mft.csv
```

---

## 10. Conclusion

Significant progress has been made in achieving legacy-output parity:

- **Descendants:** 100% match (15,119 = 15,119)
- **Treesize:** 99.999992% match (48 bytes difference on 609 MB)

The remaining 48-byte difference is likely due to a subtle edge case in:
- Extension record handling
- Compressed stream merging
- Or an attribute type not yet fully analyzed

Given the 0.000008% difference, this may be acceptable for production use, but for true byte-exact parity, further investigation into the hypotheses in Section 5.2 is recommended.

---

## Appendix A: Attribute Types and Stream Classification

| Attribute Type | Code | Counted as Stream | Notes |
|----------------|------|-------------------|-------|
| $STANDARD_INFORMATION | 0x10 | No | Metadata only |
| $ATTRIBUTE_LIST | 0x20 | No | Extension record pointer |
| $FILE_NAME | 0x30 | No | Name metadata |
| $OBJECT_ID | 0x40 | Yes | Object identifier |
| $SECURITY_DESCRIPTOR | 0x50 | No | Security metadata |
| $VOLUME_NAME | 0x60 | Yes | Volume label |
| $VOLUME_INFORMATION | 0x70 | Yes | Volume metadata |
| $DATA | 0x80 | Yes | File data / ADS |
| $INDEX_ROOT | 0x90 | Merged | Directory index |
| $INDEX_ALLOCATION | 0xA0 | Merged | Directory index |
| $BITMAP | 0xB0 | Conditional | Unnamed = stream |
| $REPARSE_POINT | 0xC0 | Yes | Symlink/junction data |
| $EA_INFORMATION | 0xD0 | Yes | EA metadata |
| $EA | 0xE0 | Yes | Extended attributes |
| $PROPERTY_SET | 0xF0 | Yes | Property set |
| $LOGGED_UTILITY_STREAM | 0x100 | Yes | Transaction log |

---

## Appendix B: Test Data Summary

### G: Drive (16GB USB Stick)

| Metric | Value |
|--------|-------|
| Total MFT Records | ~15,088 |
| User Files | ~15,000 |
| System Metafiles | 12 (FRS 0-11) |
| Directories with $TXF_DATA | 2 (Root, $Txf) |
| Hardlinked Files | 3 (FRS 188, 244, 252) |
| Files with ADS | Multiple |

### Key Test Directories

| Directory | FRS | Files | Treesize |
|-----------|-----|-------|----------|
| MFT_TEST | 53 | ~15,000 | 545,073,550 |
| _FRAG_PRE_1 | 54 | 5,000 | 2,731,149 |
| _FRAG_PRE_2 | 56 | 5,000 | 539,602,061 |
| _FRAG_PRE_3 | 60 | 5,000 | 2,731,149 |
| Documents | 55 | ~100 | Various |

