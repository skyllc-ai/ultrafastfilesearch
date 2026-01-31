# Changelog Healing - 2026-01-30 22:57

## Context

Branch: `feature/cpp-parsing-algorithm-port`

## Changes Made

### Fix: `MftIndex::add_child_entry()` - C++ Parity

**File**: `crates/uffs-mft/src/index.rs`

**Problem**: The `add_child_entry()` method was silently skipping child entries when the parent record didn't exist. This caused child entries to be lost when chunks were processed out of order.

**Root Cause**: The method used `frs_to_idx_opt()` which returns `None` if the parent doesn't exist, and then returned early without creating the child entry.

**Fix**: Changed to create a placeholder parent record on-demand, matching C++ `at(frs_parent)` behavior in `ntfs_index.hpp` lines 568-579.

**Before**:
```rust
let Some(parent_idx) = self.frs_to_idx_opt(parent_frs) else {
    return;  // Child entry lost!
};
```

**After**:
```rust
// Expand lookup table if needed
if parent_frs_usize >= self.frs_to_idx.len() {
    self.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
}

// Get or create parent record index
let parent_idx = if self.frs_to_idx[parent_frs_usize] == NO_ENTRY {
    // Create placeholder parent record
    let new_idx = self.records.len() as u32;
    self.frs_to_idx[parent_frs_usize] = new_idx;
    self.records.push(FileRecord::new(parent_frs));
    new_idx as usize
} else {
    self.frs_to_idx[parent_frs_usize] as usize
};
```

### Fix: Test Code Clippy Errors

**File**: `crates/uffs-mft/src/index.rs`

**Problem**: CI pipeline failed with 6 clippy errors in test code:
- 4x `clippy::similar_names` - variable names too similar (`name_a_ref`, `name_b_ref`, `name_c_ref`)
- 2x `clippy::doc_markdown` - `get_name_at` missing backticks in doc comments

**Fix**:
1. Renamed variables to be more distinct:
   - `name_b_ref` → `ext_hardlink_b`
   - `name_c_ref` → `ext_hardlink_c`
   - `name_a_ref` → `base_original`
   - `name_b_offset` → `ext_b_offset`
   - `name_c_offset` → `ext_c_offset`
   - `name_a_offset` → `base_offset`
2. Added backticks around `get_name_at` in doc comments

## CI Pipeline Results

### Run 1: FAILED - 6 clippy errors in test code (similar_names, doc_markdown)
### Run 2: [PENDING]

