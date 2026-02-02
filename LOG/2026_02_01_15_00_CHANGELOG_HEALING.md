# Changelog Healing - 2026-02-01 15:00

## Issue: Timestamp 1-Hour Offset Between C++ and Rust Output

### Problem
When comparing C++ and Rust offline output, timestamps showed a consistent +1 hour difference:

| Field | C++ | Rust | Difference |
|-------|-----|------|------------|
| Created | 2024-05-20 **19:49:45** | 2024-05-20 **20:49:45** | +1 hour |
| Modified | 2024-05-20 **19:49:45** | 2024-05-20 **20:49:45** | +1 hour |
| Accessed | 2024-05-20 **19:49:45** | 2024-05-20 **20:49:45** | +1 hour |

### Root Cause Analysis

**C++ behavior** (`time_utils.hpp` lines 30-66):
1. `get_time_zone_bias()` calls `GetSystemTimeAsFileTime()` to get current UTC time
2. Calls `FileTimeToLocalFileTime()` to convert to local time
3. Returns `ft_local - ft` (the current timezone offset in 100ns intervals)
4. This offset is calculated **ONCE** and applied to **ALL** timestamps

**Key insight:** Windows' `FileTimeToLocalFileTime()` uses the **CURRENT** DST status, not the historical DST status for the timestamp's date. So if you're in PST (winter) and convert a timestamp from May (when PDT was active), Windows still applies the PST offset.

**Rust behavior** (`output.rs` lines 584-588, before fix):
1. Uses `chrono::DateTime::from_timestamp()` to create UTC datetime
2. Uses `Local.from_utc_datetime()` which applies **HISTORICAL** DST rules
3. So a May timestamp gets PDT offset, a January timestamp gets PST offset

This caused a 1-hour difference for timestamps from dates when DST status differed from current.

### Fix Applied

Modified `crates/uffs-core/src/output.rs`:

1. Added `timezone_offset_secs: i32` field to `OutputConfig` struct
2. In `Default::default()`, compute the current timezone offset once using `chrono::Local::now().offset().local_minus_utc()`
3. In `format_value()`, use `chrono::FixedOffset::east_opt(self.timezone_offset_secs)` instead of `Local.from_utc_datetime()`

This matches C++ behavior: the same timezone offset is applied to ALL timestamps, regardless of the timestamp's date.

### Verification

After fix, timestamps match exactly:
- **C++:** `2024-05-20 19:49:45`
- **Rust:** `2024-05-20 19:49:45` ✅

Full comparison shows 100% path parity with 7,058,030 common paths.

### Files Changed
- `crates/uffs-core/src/output.rs` - Added fixed timezone offset handling

### Tests
- All 145 uffs-core tests pass
- Offline comparison shows timestamp parity

---

## Issue: C++ Port Tree Algorithm Storing Stream Count Instead of File Sizes

### Problem
When comparing C++ and Rust tree metrics with `--tree-algo cpp`, the "Size" column for directories showed wildly different values:

| Field | C++ | Rust | Notes |
|-------|-----|------|-------|
| **Size** | 5,159,847,006,297 | 7,058,053 | C++ = treesize (sum of file sizes), Rust = stream count |
| **Size on Disk** | 5,027,573,407,744 | 5,093,351,227,392 | ~66GB difference |
| **Descendants** | 14,754,843 | 4,768,552 | ~10M difference |

The Rust "Size" value of 7,058,053 was suspiciously close to the row count (~7M paths), indicating it was counting streams rather than summing file sizes.

### Root Cause Analysis

In `crates/uffs-mft/src/cpp_tree.rs`, the `preprocess_recursive()` function was incorrectly storing stream count in `record.treesize`:

**Before (buggy):**
```rust
if is_directory {
    record_mut.descendants = children_size.descendants;
    record_mut.treesize = u64::from(children_size.treesize) + u64::from(stream_count);  // STREAM COUNT!
    record_mut.tree_allocated = children_size.allocated + first_stream_allocated;
} else {
    record_mut.descendants = 0;
    record_mut.treesize = u64::from(stream_count);  // STREAM COUNT!
    record_mut.tree_allocated = first_stream_allocated;
}
```

The C++ algorithm uses `PreprocessResult.treesize` internally as a stream count for the delta formula, but the **output** "Size" column for directories comes from the accumulated `length` field (sum of file sizes) stored in the default stream.

### Fix Applied

Modified `crates/uffs-mft/src/cpp_tree.rs`:

1. Added computation of `own_total_length` and `own_total_allocated` (sum of all streams for the record)
2. Changed directory storage to use `children_size.length + first_stream_length` in `record.treesize`
3. Changed file storage to use `first_stream_length` in `record.treesize`

**After (fixed):**
```rust
if is_directory {
    record_mut.descendants = children_size.descendants;
    // C++ stores accumulated length (sum of file sizes) in the default stream's
    // length field, which becomes the directory's "Size" in output.
    // We store this in treesize for Rust output compatibility.
    record_mut.treesize = children_size.length + first_stream_length;
    record_mut.tree_allocated = children_size.allocated + first_stream_allocated;
} else {
    record_mut.descendants = 0;
    // Files: treesize = own size (not stream count)
    record_mut.treesize = first_stream_length;
    record_mut.tree_allocated = first_stream_allocated;
}
```

### Files Changed
- `crates/uffs-mft/src/cpp_tree.rs` - Fixed treesize storage to use sum of file sizes

### Tests
- All 116 uffs-mft tests pass
- Awaiting user verification with offline comparison

