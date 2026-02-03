# ONLINE Flow Tree Metrics Parity Fix Plan

**Date:** 2026-02-03
**Status:** ✅ IMPLEMENTED - Pending CI Verification
**Scope:** C++ algorithm paths only (cpp_port parse, cpp tree, cpp I/O pipeline)

## Executive Summary

The OFFLINE MFT processing flow achieves ~100% tree metrics parity with C++. The ONLINE (LIVE) flow does NOT because the `CppMftIndex.into_mft_index()` conversion in `cpp_types.rs` fails to build the internal stream linked list required for per-stream delta distribution.

---

## 1. Chronological Summary of OFFLINE Fixes

### Fix 1: $SECURITY_DESCRIPTOR Stream Counting
**File:** `SECURITY_DESCRIPTOR_STREAM_FIX.md`  
**Problem:** Attribute type 0x50 (`$SECURITY_DESCRIPTOR`) was being skipped in Rust parsing.  
**Solution:** Added 0x50 to the stream-creating attribute list in `parse.rs`.

### Fix 2: $ATTRIBUTE_LIST Stream Counting  
**File:** `feedback_review_and_next_fix_attribute_list.md`  
**Problem:** Attribute type 0x20 (`$ATTRIBUTE_LIST`) was being skipped in Rust parsing.  
**Solution:** Added 0x20 to the stream-creating attribute list in `parse.rs`.

### Fix 3: Extension Record Stream Merging
**Problem:** Extension record streams were being SKIPPED instead of ADDED to base record sizes.  
**Solution:** Fixed merge logic to ADD sizes from extension records.

### Fix 4: Two-Channel Model Implementation
**Files:** `MFT_tree_metrics_parity_deep_dive.md`, `cpp_tree_two_channel_patched.rs`, `tree_metrics_cpp_parity_deep_dive_fix.md`  
**Problem:** C++ uses two different metric channels:
- **Channel A (propagation):** Values returned by recursion, counts ALL streams including internal
- **Channel B (printed):** Values stored for output, only counts directory stream (`$I30`)

**Solution:** Store Channel B values (directory stream only) for printed metrics while propagating Channel A values (all streams) up the tree.

### Fix 5: Per-Stream Delta for Internal Streams
**Files:** `uffs_tree_metrics_parity_remaining_gap_and_fix.md`, `cpp_tree_internal_stream_delta_fix.rs`  
**Problem:** The `delta()` function uses integer division and is NOT linear:
```
delta(a + b, i, n) != delta(a, i, n) + delta(b, i, n)
```
Pre-summing internal stream sizes caused ±1-4 byte tree-size skews.

**Solution:** Added `InternalStreamInfo` struct and `internal_streams` vector to `MftIndex`. Added `first_internal_stream` field to `FileRecord`. The tree algorithm now iterates through internal streams individually and applies `delta()` per stream.

---

## 2. ONLINE Code Path Analysis (C++ Algorithms Only)

### Data Flow
```
CppIoPipeline::run()           [cpp_io_pipeline.rs]
    ↓
CppParsePipeline::load()       [cpp_types.rs]
    ↓
CppMftIndex                    [cpp_types.rs]
    ↓
CppMftIndex::into_mft_index()  [cpp_types.rs:1194-1328]
    ↓
MftIndex                       [index.rs]
    ↓
MftIndex::compute_tree_metrics() → cpp_tree::CppTreeTraversal
```

### Key Files
| File | Role |
|------|------|
| `cpp_io_pipeline.rs` | IOCP sliding window I/O with inline parsing |
| `cpp_types.rs` | `CppMftIndex`, `CppParsePipeline`, `into_mft_index()` |
| `cpp_tree.rs` | Tree metrics computation with per-stream delta |
| `index.rs` | `MftIndex`, `FileRecord`, `InternalStreamInfo` |

### How C++ Stores Streams
C++ stores ALL streams (including internal ones like `$REPARSE_POINT`, `$OBJECT_ID`, `$SECURITY_DESCRIPTOR`) in:
- `Record.first_stream` (inline, first stream)
- `CppMftIndex.streaminfos` (overflow, additional streams)

The C++ tree algorithm iterates through ALL streams and applies `delta()` to each.

---

## 3. Gap Analysis: The Missing Piece

### Location: `cpp_types.rs:1266-1291`

```rust
// C++ stores all streams, so total_stream_count = stream_count
total_stream_count: cpp_record.stream_count,
// C++ stores all streams inline, so no internal streams are filtered
first_internal_stream: NO_ENTRY,  // ← THE GAP
// ...
// C++ stores all streams, so no internal streams are filtered
internal_streams_size: 0,         // ← THE GAP
internal_streams_allocated: 0,    // ← THE GAP
```

### What's Missing
The `into_mft_index()` conversion does NOT:
1. Identify which streams are "internal" (names starting with `$` + uppercase letter)
2. Build the `internal_streams` linked list with `InternalStreamInfo` entries
3. Populate `first_internal_stream` pointer in `FileRecord`
4. Calculate `internal_streams_size` / `internal_streams_allocated` totals

### Why This Breaks Tree Metrics
The `cpp_tree.rs` algorithm (lines 309-321) expects to iterate through `first_internal_stream`:

```rust
let mut internal_idx = record.first_internal_stream;
while internal_idx != NO_ENTRY {
    let st = &index.internal_streams[internal_idx as usize];
    let internal_length_delta = delta(st.size.length, name_info, total_names);
    // ... apply delta per stream
    internal_idx = st.next_entry;
}
```

For ONLINE flow, `first_internal_stream = NO_ENTRY`, so this loop never executes and internal streams are not included in tree metrics.

---

## 4. Proposed Fix

### Option A: Modify `into_mft_index()` (Recommended)

During the conversion from `CppMftIndex` to `MftIndex`, iterate through all streams and identify internal ones:

```rust
// In into_mft_index(), after creating the FileRecord:

// Identify internal streams from first_stream and overflow streams
let mut internal_streams_size = 0_u64;
let mut internal_streams_allocated = 0_u64;
let mut first_internal_stream = RUST_NO_ENTRY;
let mut last_internal_stream = RUST_NO_ENTRY;

// Check first_stream
if is_internal_stream(&cpp_record.first_stream, &self.names) {
    let new_idx = index.internal_streams.len() as u32;
    index.internal_streams.push(InternalStreamInfo {
        size: SizeInfo {
            length: cpp_record.first_stream.size.length.as_u64(),
            allocated: cpp_record.first_stream.size.allocated.as_u64(),
        },
        next_entry: RUST_NO_ENTRY,
        flags: 0,
    });
    first_internal_stream = new_idx;
    last_internal_stream = new_idx;
    internal_streams_size += cpp_record.first_stream.size.length.as_u64();
    internal_streams_allocated += cpp_record.first_stream.size.allocated.as_u64();
}

// Check overflow streams (iterate through streaminfos linked list)
// ... similar logic for each overflow stream

// Update the FileRecord
record.first_internal_stream = first_internal_stream;
record.internal_streams_size = internal_streams_size;
record.internal_streams_allocated = internal_streams_allocated;
```

### Helper Function Needed
```rust
fn is_internal_stream(stream: &StreamInfo, names: &[u8]) -> bool {
    let name_offset = stream.name.offset();
    let name_len = stream.name.length();
    if name_len == 0 {
        return false; // Default stream, not internal
    }
    // Decode name and check if it starts with $UPPERCASE
    // ...
}
```

### Option B: Modify `cpp_tree.rs` to Iterate ALL Streams

Instead of using the internal stream linked list, modify the tree algorithm to iterate through ALL streams (first_stream + overflow) and apply delta to each. This would work because C++ stores all streams without filtering.

**Pros:** No changes to `into_mft_index()`, simpler conversion
**Cons:** Different code path than OFFLINE, harder to maintain parity

---

## 5. Implementation Checklist

### Files Modified
- [x] `crates/uffs-mft/src/cpp_types.rs` - `into_mft_index()` function

### Changes Completed (2026-02-03)
1. [x] Add helper function to identify internal streams from C++ `StreamInfo`
   - Added `is_internal_stream()` method to `CppMftIndex` (lines 1590-1635)
   - Handles both ASCII and UTF-16LE encoded names from C++ names buffer
   - Checks if name starts with `$` followed by uppercase letter
2. [x] In `into_mft_index()`, iterate through `first_stream` and check if internal
   - Added check at lines 1269-1284
3. [x] In `into_mft_index()`, iterate through overflow `streaminfos` and check if internal
   - Added loop at lines 1286-1316
4. [x] Build `InternalStreamInfo` entries for internal streams
   - Creates entries with `RustSizeInfo { length, allocated }` and `next_entry`
5. [x] Link internal streams into linked list via `next_entry`
   - Properly maintains `first_internal_stream` and `last_internal_stream` pointers
6. [x] Set `first_internal_stream` in `FileRecord`
   - Set at line 1333
7. [x] Calculate and set `internal_streams_size` / `internal_streams_allocated`
   - Set at lines 1354-1355

### Testing
- [ ] Run CI pipeline to verify compilation and tests pass
- [ ] Run `analyze_trial_parity.rs` on LIVE scan to verify tree metrics match
- [ ] Compare ONLINE vs OFFLINE results for same drive
- [ ] Verify no regression in OFFLINE flow

---

## 6. Code Reference: OFFLINE Internal Stream Handling

The OFFLINE flow correctly handles internal streams in `index.rs:5946-6018`:

```rust
// Filter out:
//   - Empty name (default stream)
//   - Internal Windows streams (names starting with `$UPPERCASE`)
let mut internal_streams_size = 0_u64;
let mut internal_streams_allocated = 0_u64;
let mut first_internal_stream = NO_ENTRY;
let mut last_internal_stream = NO_ENTRY;

for st in &parsed.streams {
    if st.name.is_empty() {
        continue;
    }

    let is_internal = st
        .name
        .strip_prefix('$')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|ch| ch.is_ascii_uppercase());

    if is_internal {
        internal_streams_size = internal_streams_size.saturating_add(st.size);
        internal_streams_allocated =
            internal_streams_allocated.saturating_add(st.allocated_size);

        let flags = u8::from(st.is_sparse) | (u8::from(st.is_resident) << 1_u8);

        let new_idx = index.internal_streams.len() as u32;
        index.internal_streams.push(InternalStreamInfo {
            size: SizeInfo {
                length: st.size,
                allocated: st.allocated_size,
            },
            next_entry: NO_ENTRY,
            flags,
        });

        if last_internal_stream == NO_ENTRY {
            first_internal_stream = new_idx;
        } else {
            index.internal_streams[last_internal_stream as usize].next_entry = new_idx;
        }
        last_internal_stream = new_idx;
        continue;
    }
    // ... handle non-internal streams
}

record.first_internal_stream = first_internal_stream;
record.internal_streams_size = internal_streams_size;
record.internal_streams_allocated = internal_streams_allocated;
```

This same logic needs to be applied in `into_mft_index()` for the ONLINE flow.

---

## 7. Summary

| Aspect | OFFLINE Flow | ONLINE Flow (BEFORE) | ONLINE Flow (AFTER) |
|--------|--------------|----------------------|---------------------|
| **Parsing** | `parse_record_full()` → `ParsedRecord` | `CppParsePipeline` → `CppMftIndex` | `CppParsePipeline` → `CppMftIndex` |
| **Internal Stream Detection** | ✅ In `from_parsed_records()` | ❌ Missing in `into_mft_index()` | ✅ `is_internal_stream()` helper |
| **Internal Stream Linked List** | ✅ Built correctly | ❌ Always `NO_ENTRY` | ✅ Built in `into_mft_index()` |
| **Per-Stream Delta** | ✅ Works correctly | ❌ Loop never executes | ✅ Loop executes correctly |
| **Tree Metrics Parity** | ✅ ~100% | ❌ Missing internal stream sizes | ✅ Expected ~100% |

**Root Cause (FIXED):** The `into_mft_index()` conversion assumed "C++ stores all streams, so no internal streams are filtered" but failed to build the internal stream tracking that `cpp_tree.rs` requires for per-stream delta distribution.

**Fix Applied:** Added `is_internal_stream()` helper and modified `into_mft_index()` to identify internal streams, build the linked list, and populate `first_internal_stream`, `internal_streams_size`, and `internal_streams_allocated` in each `FileRecord`.

