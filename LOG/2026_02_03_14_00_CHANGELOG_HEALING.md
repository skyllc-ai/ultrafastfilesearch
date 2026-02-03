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
- [ ] Running CI pipeline...

## Fixes Applied During CI
(To be updated if any fixes are needed)

