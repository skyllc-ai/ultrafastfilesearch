# CHANGELOG_HEALING - 2026-02-01

## CI Pipeline Run #1

**Started:** 2026-02-01 12:00 UTC

**Branch:** `feature/chunk-processing-investigation`

**Purpose:** Validate diagnostic logging additions for live vs offline path comparison investigation.

### Changes Being Validated

1. Added extensive diagnostic logging to `CppParsePipeline` in `cpp_types.rs`:
   - Chunk handoff logging
   - Record boundary handling
   - preload_concurrent timing
   - USA fixup success/failure counts
   - Records not in-use counts
   - Parallel sync (lock acquisition)

2. Added diagnostic logging to offline path in `reader.rs`

3. Added `--chunk-algo` CLI scaffolding for future chunk processing investigation

4. Updated `trial_run.ps1` for live data collection with diagnostic logging

### Errors Found (Run #1)

31 clippy errors in `crates/uffs-mft/src/cpp_types.rs`:

1. **`std_instead_of_core`** (26 occurrences) - Using `std::sync::atomic::AtomicU64` instead of `core::sync::atomic::AtomicU64`
2. **`partial_pub_fields`** (1 occurrence) - Mixed usage of pub and non-pub fields in diagnostic counters struct
3. **`significant_drop_tightening`** (1 occurrence) - Temporary with significant Drop can be early dropped (line 1762)
4. **`missing_asserts_for_indexing`** (1 occurrence) - Indexing into a slice multiple times without an assert (lines 1945-1950)

### Fixes Applied (Run #1)

1. **Changed all `std::sync::atomic` to `core::sync::atomic`** - Replaced all 26 occurrences in struct fields, constructors, and function bodies
2. **Made diagnostic counter fields public** - Changed from private to `pub` to fix `partial_pub_fields` lint (all fields in struct are now consistently public)
3. **Merged lock acquisition with single usage** - Changed `let mut index = self.index.lock().unwrap(); index.get_or_create(max_frs - 1);` to `self.index.lock().unwrap().get_or_create(max_frs - 1);`
4. **Added assert before indexing** - Added `assert!(record_data.len() >= 48, ...)` before indexing into `record_data[0..3]` and `record_data[22..23]` to elide bounds checks

---

## CI Pipeline Run #2

**Started:** 2026-02-01 12:15 UTC

### Errors Found (Run #2)

1 clippy error in `crates/uffs-cli/src/commands.rs`:

1. **`cognitive_complexity`** (30/25) - The `search` function has cognitive complexity of 30, exceeding the limit of 25

### Root Cause Analysis

The `search` function in `commands.rs` had four repetitive algorithm configuration blocks (tree_algo, parse_algo, io_algo, chunk_algo) that each:
- Parse the algorithm string to an enum
- Check if it's `CppPort` variant
- Set an environment variable if so
- Log info messages

The `--chunk-algo` scaffolding added in the investigation pushed the complexity over the limit.

### Fixes Applied (Run #2)

1. **Extracted algorithm configuration into helper function** - Created `configure_algorithms()` function that handles all four algorithm configurations:
   - Parses each algorithm string
   - Sets environment variables when `CppPort` is selected
   - Logs configuration info
   - Reduces `search()` cognitive complexity by moving 4 if-blocks out

**Location:** `crates/uffs-cli/src/commands.rs` lines 320-376

---

## CI Pipeline Run #3

**Started:** 2026-02-01 12:25 UTC

### Errors Found (Run #3)

8 clippy errors in `crates/uffs-cli/src/commands.rs`:

1. **`undocumented_unsafe_blocks`** (4 occurrences) - Unsafe blocks missing SAFETY comments
2. **`semicolon_outside_block`** (4 occurrences) - Semicolons should be outside unsafe blocks (test linting)
3. **`single_call_fn`** (1 occurrence) - Function only called once

### Fixes Applied (Run #3)

1. **Added SAFETY comments** - Added `// SAFETY: Called once at CLI startup in main thread before spawning workers.` before each unsafe block
2. **Moved semicolons outside blocks** - Changed `unsafe { ... ; }` to `unsafe { ... };`
3. **Added allow attribute for single_call_fn** - Added `#[allow(clippy::single_call_fn)]` since the function is intentionally extracted for cognitive complexity reduction

---

## CI Pipeline Run #4

**Started:** 2026-02-01 12:30 UTC

### Errors Found (Run #4)

4 clippy errors in `crates/uffs-cli/src/commands.rs`:

1. **`semicolon_inside_block`** (4 occurrences) - Production linting wants semicolons INSIDE blocks

### Root Cause Analysis

There's a conflict between two clippy lints:
- Production linting uses `-D clippy::semicolon-inside-block` (wants `;` inside)
- Test linting uses `-D clippy::semicolon-outside-block` (wants `;` outside)

These are mutually exclusive lints that cannot both be satisfied.

### Fixes Applied (Run #4)

1. **Added allow for both semicolon lints** - Added `#[allow(clippy::semicolon_inside_block, clippy::semicolon_outside_block)]` to the `configure_algorithms` function to silence both conflicting lints

---

## Final Status

**Phase 1 (Testing & Validation):** ✅ PASSED
**Phase 2 (Build & Deploy):** 🔄 In Progress (building release binaries)

All clippy errors have been fixed. The CI pipeline is now in the build phase.

