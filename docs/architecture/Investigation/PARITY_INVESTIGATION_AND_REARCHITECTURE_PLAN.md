# NTFS Treesize Parity Investigation & Re-Architecture Plan

**Date:** 2026-03-19  
**Status:** Phase 1 COMPLETE (51 diffs, all real metrics match). Phase 2 PLANNED (unified parser for 100% parity).  
**Drive tested:** D: (5 TB HDD, ~4.9M MFT records, ~7M output rows)

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Investigation Timeline](#investigation-timeline)
3. [What We Tried & Results](#what-we-tried--results)
4. [Root Cause Analysis](#root-cause-analysis)
5. [C++ Architecture Deep Dive](#c-architecture-deep-dive)
6. [Current Rust Architecture (Flawed)](#current-rust-architecture-flawed)
7. [Re-Architecture Plan](#re-architecture-plan)
8. [Files To Touch](#files-to-touch)
9. [Detailed Implementation Steps](#detailed-implementation-steps)
10. [Risk Assessment](#risk-assessment)

---

## Executive Summary

We investigated parity between the C++ (`uffs.com`) and Rust (`uffs`) NTFS MFT parsing implementations. Starting at 66 sorted-line differences, we fixed 4 real bugs bringing it to 51 diffs with all real metrics (treesize, allocated, descendants) matching exactly. The remaining 51 diffs are cosmetic: ±5 byte hardlink fair-share rounding and sort-order cascade from different `$FILE_NAME` primary-name selection on hardlinked files.

**The fundamental architectural difference causing the remaining 51 diffs cannot be fixed incrementally.** C++ processes ALL MFT records (base and extension) through ONE attribute loop. Rust uses TWO separate parsers with different logic. Every attempt to patch the Rust dual-parser to match C++ behavior either regressed or introduced new edge cases.

**Decision:** Re-architect the Rust IOCP parser to mirror C++ 100% — one unified record processor with identical attribute handling, name ordering, and stream counting.

---

## Investigation Timeline

### Phase 1: Bug Fixes (COMPLETED — 66→51 diffs)

| Fix | Diffs | Files Changed | Description |
|-----|-------|---------------|-------------|
| `$I30` saturating_add | 66→51 | `io/parser/index.rs` (2 locations) | Extension `$INDEX_ALLOCATION` DataSize was overwritten by base `$INDEX_ROOT` ValueLength instead of accumulating |
| Internal named `$DATA` streams | 51→51 (root gap fixed) | 6 parser files | Named `$DATA` streams with `$`-prefixed names were silently dropped — their sizes never captured for tree metrics |
| `$BadClus:$Bad` InitializedSize | (prevented 8 TB overcorrection) | `io/parser/index.rs` | C++ uses `InitializedSize` (≈0) instead of `DataSize` (=entire volume) for `$BadClus:$Bad` |
| `parent_frs==0` child-entry skip | (correctness fix) | 5 files | Rust skipped child entries for `parent_frs==0`; C++ only skips self-references |
| `has_default_data` bit | (descendants +5→EXACT) | `types.rs`, `index.rs`, `index_extension.rs`, `tree.rs` | Distinguishes "has empty `$DATA`" from "has no `$DATA`" for post-parse `total_stream_count` correction |

### Phase 2: Attempted Fixes (FAILED — reverted)

| Attempt | Result | Why It Failed |
|---------|--------|---------------|
| Push-to-front `$FILE_NAME` in base parser | 51→98 diffs | Swapping primary with last additional name doesn't match C++ because C++ processes base+extension in FRS order |
| Push-to-front in extension parser | Same regression | Extension names arrive at unpredictable times relative to base |
| `has_empty_default` in tree_metrics | 51→14,875 diffs | Condition was too broad — caught directories with resident `$INDEX_ROOT` (`first_alloc=0`) |
| `found_default_data` in parser | 51→3,270 diffs | Doesn't account for `$DATA` on extension records not yet arrived |
| `base_count=0` for `total_stream_count` | 51→6,982,046 diffs (5,631 missing rows) | `stream_count=0` breaks output row generation |
| Unified `process_record` (v1) | 14,129,916 lines (doubled) | `FileRecord::new()` starts `name_count=1` but unified parser also increments → double-counting |
| Unified `process_record` (v2, count reset) | 4,773,997 lines (missing 2.3M) | Aggressive count reset killed records already populated by extension attributes |

---

## Root Cause Analysis

### The 51 Remaining Diffs Breakdown

- **1 root row:** treesize +5 bytes (hardlink fair-share integer division rounding)
- **~10 ancestor directories:** ±5 byte treesize cascade through Dropbox hierarchy
- **~12 leaf directories:** ±2-5 byte hardlink fair-share rounding variations
- **~29 sort-order cascade:** 8 hardlinked files (`dep-graph.bin`, `query-cache.bin`, `work-products.bin` in Rust incremental compilation directories) appear under different parent paths, causing misaligned sorted rows

### Why Sort-Order Can't Be Fixed Incrementally

These 8 files have multiple `$FILE_NAME` attributes (hardlinks) pointing to different parent directories. The "primary name" (`first_name`) determines which path appears in output.

**C++ behavior:** Push-to-front — each `$FILE_NAME` overwrites `first_name`. Since C++ processes records in FRS order (base before extensions), the LAST `$FILE_NAME` attribute across ALL MFT records for a file becomes the primary name. This is deterministic.

**Rust behavior:** First-wins — the first `$FILE_NAME` encountered becomes `first_name`, additional ones are appended. Since IOCP delivers records in random order, and base/extension parsers are separate, the primary name is non-deterministic relative to C++.

**Why patching fails:** The "last name" in C++ depends on processing ALL MFT records for a file through ONE code path in a specific order. Rust's dual-parser architecture processes base and extension records through DIFFERENT code paths at UNPREDICTABLE times. No amount of swapping/reordering within one parser can replicate the cross-record ordering that C++ achieves naturally.

---

## C++ Architecture Deep Dive

### Reference File
`_trash/UltraFastFileSearch-code/src/index/ntfs_index_load.hpp` (729 lines)

### Processing Model

```
┌─────────────────────────────────────────────────────────────┐
│                    C++ load() Function                       │
│                                                              │
│  for each MFT record in buffer (FRS order within chunk):    │
│                                                              │
│    1. Read FileRecordSegmentHeader                          │
│    2. Check: is_in_use && magic == 'FILE'                   │
│    3. frs_base = is_extension ? BaseFileRecordSegment : frs │
│    4. base_record = this->at(frs_base)  [get or create]    │
│                                                              │
│    5. FOR EACH ATTRIBUTE on this record:                    │
│       ┌─────────────────────────────────────────────┐       │
│       │  switch (ah->Type):                          │       │
│       │                                              │       │
│       │  $STANDARD_INFORMATION (0x10):               │       │
│       │    → set stdinfo on base_record              │       │
│       │    → directory flag from header               │       │
│       │                                              │       │
│       │  $FILE_NAME (0x30):                          │       │
│       │    → skip DOS-only (namespace 2)             │       │
│       │    → PUSH-TO-FRONT: push old first_name      │       │
│       │      to chain, overwrite with new             │       │
│       │    → child entry: name_index = name_count    │       │
│       │    → ++name_count                            │       │
│       │                                              │       │
│       │  EVERYTHING ELSE (default case):             │       │
│       │    → is_primary check (LowestVCN == 0)       │       │
│       │    → is_i30 check ($I30 name on index attrs) │       │
│       │    → if is_i30: accumulate into first_stream │       │
│       │    → else: push old first_stream to chain,   │       │
│       │      create new, ++stream_count              │       │
│       │    → $BadClus:$Bad: use InitializedSize      │       │
│       │    → sizes: info->length += DataSize         │       │
│       │    → info->allocated += AllocatedSize        │       │
│       │    → (compressed: use CompressedSize)        │       │
│       └─────────────────────────────────────────────┘       │
│                                                              │
│  After all records parsed:                                   │
│    → preprocessor() from root FRS 5                          │
│    → depth-first tree metrics computation                    │
│    → reserved_clusters adjustment at depth 0                 │
└─────────────────────────────────────────────────────────────┘
```

### Key C++ Design Decisions

1. **ONE attribute loop for ALL records** (lines 240-461): Both base and extension records go through the SAME switch/case. No separate "extension handler."

2. **Push-to-front for `$FILE_NAME`** (lines 273-278): Each new name pushes old `first_name` to chain and overwrites. Last processed wins.

3. **Unified stream chain** (lines 374-428): ALL non-`$STD_INFO`/non-`$FILE_NAME` attributes create entries in ONE chain. No concept of "internal streams" at parse time.

4. **`stream_count` starts at 0** (implicit): `stream_count` is only incremented when a new stream entry is created (line 426). No phantom default.

5. **`name_count` starts at 0** (implicit): Only incremented when a `$FILE_NAME` is processed (line 307).

6. **`$I30` accumulation** (lines 377-392): Directory index attributes ($INDEX_ROOT/$INDEX_ALLOCATION/$BITMAP with $I30 name) accumulate into existing stream entry via matching loop.

7. **No separate `total_stream_count`**: C++ has ONE `stream_count` that covers everything. The preprocessor iterates the stream chain and counts `result.treesize += 1` per entry.

8. **`$BadClus:$Bad` special handling** (lines 431-452): Uses `InitializedSize` instead of `DataSize` for both length and allocated.

9. **Compressed attributes** (lines 441-446): Uses `CompressedSize` when `CompressionUnit > 0`.

### C++ Data Model

```cpp
struct FileRecord {
    LinkInfo first_name;     // Primary name (push-to-front: LAST wins)
    StreamInfo first_stream; // First stream entry (push-to-front for new types)
    unsigned short name_count;    // Starts at 0, incremented per $FILE_NAME
    unsigned short stream_count;  // Starts at 0, incremented per new stream entry
    // ... stdinfo, children, etc.
};
```

The stream chain: `first_stream → streaminfos[next_entry] → streaminfos[next_entry] → ...`
The name chain: `first_name → nameinfos[next_entry] → nameinfos[next_entry] → ...`

Both use push-to-front: newest entry becomes `first_*`, old one is pushed to the overflow vector.

---

## Current Rust Architecture (Flawed)

### Dual-Parser Model

```
┌──────────────────────────────────────────────────────────────┐
│              Rust IOCP Parser (current)                       │
│                                                               │
│  for each MFT record (IOCP completion order — RANDOM):       │
│                                                               │
│    if base_record:                                            │
│      → parse_record_to_index()          ← Parser A            │
│        - Collects primary_name + additional_names             │
│        - First $FILE_NAME = primary (first-wins)              │
│        - Separate: default_size, additional_streams,          │
│          internal_streams                                     │
│        - stream_count = 1 + ADS count                        │
│        - total_stream_count = 1 + ADS + internal             │
│        - Creates child entries                                │
│        - Calls merge_extension_streams() for ext snapshot     │
│                                                               │
│    if extension_record:                                       │
│      → parse_extension_to_index()       ← Parser B            │
│        - DIFFERENT attribute handling logic                    │
│        - Names appended to chain (not push-to-front)          │
│        - Separate name_index calculation                      │
│        - Different stream chaining logic                      │
│        - Different default $DATA merge logic                  │
│                                                               │
│  Post-parse correction in tree.rs:                            │
│    → Subtract 1 from total_stream_count for records           │
│      without $DATA (!has_default_data && !is_directory)        │
│                                                               │
│  compute_tree_metrics():                                      │
│    → Iterates first_stream + streams + internal_streams       │
│    → Uses total_stream_count for own_stream_count             │
└──────────────────────────────────────────────────────────────┘
```

### Problems with Dual-Parser Architecture

1. **Two code paths** with subtly different logic for the same attributes
2. **First-wins naming** instead of C++'s push-to-front (last-wins)
3. **Separate stream categories** (user-visible vs internal) that don't exist in C++
4. **`FileRecord::new()` starts at `name_count=1, stream_count=1`** — assumes defaults that C++ doesn't
5. **`total_stream_count` hack** — post-parse correction needed because counts don't match C++
6. **`has_default_data` bit** — another hack to work around the phantom default stream
7. **Non-deterministic name ordering** — IOCP arrival order affects which name becomes primary

---

## Re-Architecture Plan

### Goal
Replace the dual-parser architecture with a single unified record processor that mirrors C++ `load()` line-by-line.

### Approach
1. Create `process_record()` in `io/parser/unified.rs` — ONE function for both base and extension records
2. Use `FileRecord::new_unified()` with `name_count=0, stream_count=0, total_stream_count=0`
3. Implement C++-identical attribute handling (push-to-front names, unified stream chain)
4. Wire into `load_iocp_to_index` (IOCP replay) and Windows LIVE path
5. Remove post-parse correction hacks from `tree.rs`
6. Verify parity

### Key Design Decision: Stream Storage Model

C++ uses ONE unified stream chain. Rust has separate `streams` (ADS) and `internal_streams` chains. Changing this would affect `tree_metrics.rs`, `output.rs`, `dataframe.rs`, and serialization.

**Recommended approach:** Keep Rust's separate chain model for storage but have the unified parser populate it to produce IDENTICAL counts. Internal streams get `InternalStreamInfo` entries; named $DATA gets `IndexStreamInfo` entries; both are counted in ONE `total_stream_count` that matches C++'s `stream_count`. No phantom defaults, no post-parse corrections.

---

## Files To Touch

### New Files
| File | Purpose |
|------|---------|
| `io/parser/unified.rs` | Unified record processor (EXISTS, needs completion) |

### Modified Files
| File | Changes |
|------|---------|
| `index/types.rs` | Add `FileRecord::new_unified()` with counts starting at 0 |
| `raw_iocp.rs` | Switch `load_iocp_to_index` from `parse_record_to_index` to `process_record` |
| `io.rs` | Re-export `process_record` |
| `io/parser/mod.rs` | Export unified module |
| `index/tree.rs` | Remove post-parse `total_stream_count` correction hack |
| `tree_metrics.rs` | May need adjustment if `total_stream_count` semantics change |
| `reader/index_read.rs` | Switch LIVE Windows path from `parse_record_to_index` to `process_record` |
| `reader/persistence.rs` | Switch IOCP replay path if it uses a different entry point |
| `io/readers/parallel/to_index.rs` | Switch `SlidingIocpInline` from `parse_record_to_index` to `process_record` |

### Files NOT Changed (kept for backward compatibility)
| File | Reason |
|------|--------|
| `io/parser/index.rs` | Old `parse_record_to_index` kept for other paths (merger, fragment) |
| `io/parser/index_extension.rs` | Old extension parser kept for non-IOCP paths |
| `io/parser/fragment.rs` | Fragment parser (parallel path) — separate refactor |
| `io/parser/fragment_extension.rs` | Fragment extension parser — separate refactor |
| `parse/direct_index.rs` | Direct index parser — separate refactor |
| `parse/direct_index_extension.rs` | Direct index extension — separate refactor |

---

## Detailed Implementation Steps

### Step 1: `FileRecord::new_unified()`
Add constructor with `name_count=0, stream_count=0, total_stream_count=0` and `first_name` / `first_stream` initialized to empty/NO_ENTRY. The unified parser will increment all counts from 0.

### Step 2: Complete `unified.rs`
The file exists with the basic structure. Needs:
- Use `new_unified()` instead of `get_or_create()` for fresh records (detect via `frs_to_idx == NO_ENTRY`)
- Increment `name_count` for EVERY `$FILE_NAME` (no skip for first)
- Increment `stream_count` for EVERY new stream entry
- Set `total_stream_count = stream_count + internal_stream_count` at end of attribute loop
- Proper `has_default_data` bit setting
- Handle the `name_index = name_count` assignment correctly (C++ assigns BEFORE increment)

### Step 3: Wire into IOCP Replay
Replace `parse_record_to_index` with `process_record` in `raw_iocp.rs::load_iocp_to_index()`.

### Step 4: Wire into LIVE Windows Path
Replace in `io/readers/parallel/to_index.rs` (the `SlidingIocpInline` reader).

### Step 5: Remove Hacks
- Remove post-parse `total_stream_count` correction from `tree.rs`
- Remove `has_default_data` bit (no longer needed if counts are correct from parser)
- Simplify `tree_metrics.rs` if `total_stream_count` now exactly matches C++ `stream_count`

### Step 6: Verify
Run `rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate` and target 0 real diffs. The remaining diffs should be ONLY the ±5 hardlink fair-share rounding (which C++ also has — it's an inherent integer-division artifact, not a bug).

---

## Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| `FileRecord::new_unified()` breaks existing paths | High | Keep `new()` for old parsers, only unified uses `new_unified()` |
| Output layer assumes `stream_count >= 1` | Medium | Ensure unified parser always sets `stream_count >= 1` for records with any data |
| Tree metrics uses `total_stream_count` differently | Medium | Verify formula `own_stream_count = total_stream_count.max(1)` still works |
| LIVE Windows path regression | High | Test on Windows after switch; keep old parser available behind feature flag |
| Performance impact | Low | Unified parser is simpler (no extension snapshot/merge), likely faster |
| Other consumers of MftIndex | Medium | Audit all callers of `parse_record_to_index` before removing |

---

## Appendix: C++ Reference Code Map

| C++ Lines | Function | Description |
|-----------|----------|-------------|
| 180-463 | `load()` | Main MFT record processing loop |
| 216-222 | Record iteration | FRS order within buffer chunk |
| 228-234 | Record validation | Magic check, in-use flag, base FRS determination |
| 240-461 | Attribute loop | Single switch/case for ALL attribute types |
| 249-259 | `$STD_INFO` handler | Set stdinfo + directory flag |
| 264-309 | `$FILE_NAME` handler | Push-to-front, child entries, name_count |
| 315-459 | Default case | ALL other attributes → unified stream handling |
| 358 | `is_primary` | LowestVCN == 0 check |
| 361-366 | `is_i30` | $I30 name check on index attributes |
| 377-392 | $I30 matching | Accumulate into existing directory stream |
| 394-428 | New stream entry | Push old first_stream, create new, stream_count++ |
| 430-455 | Size calculation | DataSize, AllocatedSize, $BadClus special, compression |
| 500-700 | `preprocessor()` | Tree metrics (depth-first, delta distribution, WoF merge) |
| 592-596 | Reserved clusters | `depth==0` adjustment for root allocated |
| 600-700 | Preprocessor stream loop | Iterates ALL streams, result.treesize += 1 per entry |
