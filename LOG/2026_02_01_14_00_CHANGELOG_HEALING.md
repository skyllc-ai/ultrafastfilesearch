# Changelog Healing - 2026-02-01 14:00

## Summary

Fixed the 2x rows bug in the LIVE path by adding stream filtering to match C++ behavior.

## Changes Made

### 1. Added `is_output_stream()` method to `IndexStreamInfo`

**File:** `crates/uffs-mft/src/index.rs`

**What:** Added helper method to check if a stream should be included in output.

**Why:** C++ filters out non-`$DATA` streams during output expansion (`ntfs_index.hpp` lines 1388-1392). The Rust LIVE path was missing this filter, causing ~2x the expected rows.

**How:** 
```rust
pub const fn is_output_stream(&self) -> bool {
    let tid = self.type_name_id();
    // type_name_id == 0: directory index ($I30)
    // type_name_id == 8: $DATA (0x80 >> 4)
    tid == 0 || tid == 8
}
```

### 2. Modified `IndexQuery::collect()` to filter streams

**File:** `crates/uffs-core/src/index_search.rs`

**What:** Changed stream expansion from `.map()` to `.filter_map()` with `is_output_stream()` check.

**Why:** The LIVE path uses `IndexQuery::collect()` for output expansion. Without filtering, internal Windows attributes like `$OBJECT_ID` (size=44) and `$EA_INFORMATION` (size=8) were being output as separate rows.

**How:**
```rust
(0..stream_count).filter_map(move |stream_idx| {
    let stream_info = index.get_stream_at(record, stream_idx)?;
    if !stream_info.is_output_stream() {
        return None;  // Skip non-$DATA streams
    }
    // ... rest of logic
})
```

### 3. Fixed doc comment lint warning

**File:** `crates/uffs-mft/src/index.rs`

**What:** Added backticks around `ntfs_index.hpp` in doc comment.

**Why:** Clippy `doc_markdown` lint requires code identifiers to be in backticks.

## Expected Results

| Output | Before Fix | After Fix |
|--------|------------|-----------|
| C++ (baseline) | 7,058,035 | 7,058,035 |
| Rust offline | 7,057,994 | 7,057,994 |
| Rust LIVE | 14,741,028 | ~7,058,000 |

## CI Pipeline Status

- [ ] Initial run pending

