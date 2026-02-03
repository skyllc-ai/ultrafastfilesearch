# CHANGELOG_HEALING - 2026-02-03 14:00

## Session Goal
Apply ONLINE flow parity fix for internal stream tracking in `into_mft_index()`.

## Background
The OFFLINE flow was fixed to track internal streams (like `$REPARSE_POINT`, `$OBJECT_ID`, 
`$SECURITY_DESCRIPTOR`) for per-stream delta distribution in tree metrics. However, the 
ONLINE/LIVE flow (which uses `CppMftIndex.into_mft_index()`) was not updated, causing 
tree metrics discrepancies.

## Changes Made

### 1. Added `is_internal_stream()` helper function
**File:** `crates/uffs-mft/src/cpp_types.rs` (lines 1590-1635)

Added a helper function to check if a C++ `StreamInfo` represents an internal NTFS stream
by checking if the stream name starts with `$` followed by an uppercase letter.

Handles both ASCII and UTF-16LE encoded names from the C++ names buffer.

### 2. Modified `into_mft_index()` to build internal stream linked list
**File:** `crates/uffs-mft/src/cpp_types.rs` (lines 1256-1316)

Modified the conversion function to:
- Check if `first_stream` is internal and add to internal streams linked list
- Iterate through overflow streams and identify internal ones
- Build `InternalStreamInfo` entries with proper linked list structure
- Set `first_internal_stream`, `internal_streams_size`, `internal_streams_allocated` in `FileRecord`

This ensures the tree algorithm's internal stream loop executes correctly for ONLINE flow,
matching the behavior of the OFFLINE flow.

## CI Pipeline Status
- [x] CI pipeline passed ✅ (v0.2.179)

## Fixes Applied During CI

### Clippy lint fix: `useless_let_if_seq`
**File:** `crates/uffs-mft/src/cpp_types.rs` (lines 1263-1284)

The initial implementation triggered a clippy lint error because we initialized
`last_internal_stream = RUST_NO_ENTRY` and then conditionally modified it in an if block.

**Fix:** Refactored to use tuple destructuring with an if-else expression:
```rust
let (mut first_internal_stream, mut last_internal_stream) =
    if self.is_internal_stream(&cpp_record.first_stream) {
        // ... build first entry ...
        (new_idx, new_idx)
    } else {
        (RUST_NO_ENTRY, RUST_NO_ENTRY)
    };
```

This is more idiomatic Rust and satisfies the clippy lint while maintaining the same logic.
