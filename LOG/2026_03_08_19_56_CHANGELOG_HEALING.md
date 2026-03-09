# CHANGELOG_HEALING - 2026-03-08 19:56 UTC

## Session Goal
Run CI pipeline (`rust-script scripts/ci-pipeline.rs go -v`) and fix all errors following the surgical, correct fixes policy.

## Rules Applied
1. No suppression hacks
2. Surgical, correct fixes (prefer minimal, idiomatic Rust changes)
3. Preserve behavior & contracts
4. Improve tests, don't dodge them
5. Document & commit well

---

## Pipeline Run 1

**Status:** FAILED

**Error:**
```
error: unsafe block missing a safety comment
   --> crates/uffs-diag/src/bin/scan_mft_magic.rs:134:45
```

**Root Cause:** The `// SAFETY:` comment was placed before the `#[expect(unsafe_code)]` attribute rather than immediately before the `unsafe { ... }` block. Clippy's `undocumented_unsafe_blocks` lint requires the safety comment to be on the line immediately preceding the unsafe block.

**Fix:** Moved the safety comment to be immediately before the `unsafe { ... }` block (after the `#[expect]` attribute).

---

## Pipeline Run 2

**Additional Fixes Applied:**
1. `MftReader::open_sync` → `MftReader::open` (4 occurrences in cache.rs and reader.rs)
2. Added `#[derive(Debug)]` to `MftRecordMerger` struct in parse.rs

**Status:** FAILED (cross-compile step)

**Additional Errors Found:**
1. `read_with_progress_sync` → `read_with_progress`
2. `read_all_sync` → `read_all`
3. Unused imports in io.rs (removed VecDeque, Pin, Windows imports)
4. Unnecessary qualifications: `crate::parse::MftRecordMerger` → `MftRecordMerger` (2 places in io.rs)
5. Unused variable: `estimated_records` → `_estimated_records` in io.rs
6. Added local `use std::time::Instant;` in cross-platform function `load_raw_to_index_with_options`

---

## Pipeline Run 3

**Additional Fixes Applied:**
1. Removed `.await` from `MftReader::open()` calls in commands.rs (2 places) - method is now sync
2. Removed unfulfilled `#[expect(unused_imports)]` attributes in io.rs (2 places)
3. Removed unfulfilled `#[expect(unsafe_code)]` from functions that call unsafe but don't contain unsafe directly:
   - `read_all_sliding_window_iocp_to_index_cpp_port` in io.rs
   - `read_all_sliding_window_iocp_to_index_parallel` in io.rs
   - `read_all_streaming` in io.rs
   - `read_all_prefetch` in io.rs
   - `read_all_pipelined` in io.rs
   - `read_all_pipelined_parallel` in io.rs
   - `get_mft_bitmap` in platform.rs
   - `get_mft_bitmap_verbose` in platform.rs

**Status:** PASSED native clippy, failed cross-compile step

---

## Pipeline Run 4

**Additional Fixes Applied:**
1. Re-added `#[expect(unsafe_code)]` to `read_all_sliding_window_iocp_to_index_parallel` - it DOES contain unsafe blocks
2. Removed unfulfilled `#[expect(unsafe_code)]` from `as_overlapped_ptr` - creating raw pointers is safe
3. Removed `.await` from `MftReader::open()` calls in main.rs (9 places) - method is sync
4. Removed `.await` from `read_all()` calls (3 places at lines ~1130, ~1431, ~3553)
5. Removed `.await` from `read_with_timing()` calls (2 places at lines ~1963, ~2338)
6. Removed `.await` from `save_raw_to_file()` call (line ~2652)
7. REVERTED: `read_all_index()` IS async (uses spawn_blocking + await internally), re-added `.await`

**Status:** Running full CI pipeline...

---

