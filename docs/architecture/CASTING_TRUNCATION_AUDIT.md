# Casting & Truncation Audit

> **Goal**: Remove ALL `clippy::cast_possible_truncation`, `cast_sign_loss`,
> `cast_possible_wrap`, `cast_precision_loss`, and `cast_lossless` suppression
> attributes by fixing the root cause — using correct types from the start.
>
> **Audit date**: 2026-03-23 | **Total instances**: 105 across 4 crates

---

## Classification Summary

| Class | Count | Fix Strategy | Risk |
|-------|-------|-------------|------|
| **A — FRS `u64 as usize`** | ~28 | Introduce `Frs` newtype or use `usize` from the start | Medium |
| **B — `Vec::len() as u32/u16`** | ~22 | Use `u32::try_from().unwrap_or()` or carry the correct index type | Low |
| **C — Display-only `as f64`** | ~18 | Legitimate — use `@expect` with reason (already done) or helper fn | None |
| **D — NTFS parser casts** | ~15 | Module-level blanket — replace with targeted inline `from()`/`into()` | Medium |
| **E — Diagnostic tool blankets** | ~8 | Leave as-is — diagnostic binaries, not shipped library code | None |
| **F — DateTime math** | 2 | Legitimate — bounded arithmetic, keep `@expect` | None |
| **G — NTFS boot sector / data runs** | 5 | Legitimate — hardware values with pre-checks | None |
| **H — Test code** | ~7 | Use explicit conversions or `try_from` | Low |

---

## Class A — FRS `u64 as usize` (THE dominant pattern)

### Root Cause

NTFS File Record Segment numbers are `u64` in the on-disk spec. But the index
uses `Vec<FileRecord>` addressed by `usize`. On 64-bit systems this cast is
lossless, but Clippy flags it because on 32-bit it would truncate.

**This project targets 64-bit only** (Windows NTFS, macOS cross-compile). A
32-bit MFT would never exceed `usize::MAX` records anyway (each record is 1KB,
so `u32::MAX` records = 4TB of MFT data alone).

### Affected Sites (28 instances)

| File | Line(s) | Cast | Context |
|------|---------|------|---------|
| `index/base.rs` | 195, 230, 275, 305, 438 | `frs as usize` | `get_or_create`, `find`, `frs_to_idx_opt` |
| `index/merge.rs` | 72, 129, 203, 357, 378, 399, 414 | `frs as usize`, `len() as u32` | Fragment merge offsets |
| `index/builder.rs` | 41 | `frs as usize` | Index building |
| `index/child_order.rs` | 21, 101 | `frs as usize` | Child sorting |
| `index/usn.rs` | 40 | `frs as usize` | USN journal |
| `index/fragment.rs` | 79 | `frs as usize` | Fragment building |
| `parse/merger.rs` | 84, 114 | `frs as usize` | Record merging |
| `path_resolver/fast.rs` | 75, 147, 168, 242 | `frs as usize` | DataFrame path resolver |
| `commands/load.rs` | 646 | `total_records as usize` | IOCP capacity |
| `reader/persistence.rs` | 173, 485 | `record_count as usize` | Raw MFT loading |

### Recommended Fix: `frs_to_usize()` helper + centralized assertion

```rust
// In index/types.rs or a shared location:

/// Convert an FRS number to a `usize` index.
///
/// # Panics (debug only)
///
/// Debug-asserts that the FRS fits in `usize`. On 64-bit platforms
/// this is always true. On hypothetical 32-bit targets, this would
/// catch overflow during development.
#[inline(always)]
pub const fn frs_to_usize(frs: u64) -> usize {
    debug_assert!(
        frs <= usize::MAX as u64,
        "FRS exceeds usize::MAX — are you running on 32-bit?"
    );
    frs as usize
}
```

**Alternative (stronger)**: Use `usize` for FRS throughout the index layer.
The NTFS spec uses u64, but the *index* never needs more than `usize` because
it's a `Vec` index. Change `FileRecord.frs` from `u64` to `usize` and convert
at the parse boundary. This eliminates ALL class-A casts at once.

**Recommendation**: Start with the `frs_to_usize()` helper (1 PR, mechanical).
Consider the `usize` migration later if the helper feels like a bandaid.

---

## Class B — `Vec::len() as u32/u16` (index offsets)

### Root Cause

The index uses `u32` for linked-list `next_entry` fields (to save memory —
`u32` is 4 bytes vs `usize`'s 8 bytes on 64-bit). When pushing to a `Vec` and
storing the new index, code does `self.links.len() as u32`.

This is safe in practice (MFT indexes never exceed 4 billion entries), but
Clippy flags it because `Vec::len()` returns `usize`.

### Affected Sites (22 instances)

| File | Line(s) | Cast | Context |
|------|---------|------|---------|
| `index/merge.rs` | 72-75, 231, 271, 366, 387, 403, 418 | `len() as u32` | Fragment merge offset adjustments |
| `index/base.rs` | 205 | `len() as u32` | `get_or_create` new record index |
| `index/extensions.rs` | 55, 112, 134, 206 | `len() as u16`, `len() as u32` | Extension ID allocation |
| `index/dataframe.rs` | 39 | Various | DataFrame column building |
| `index/storage/serialize.rs` | 146, 155, 167 | `len() as u32` | Serialization |
| `raw_iocp.rs` | 298, 302, 308, 326, 349 | `len() as u32` | Chunk recording |
| `raw/mod.rs` | 228, 489 | Various | Raw MFT loading |
| `path_resolver/arena.rs` | 28 | `len() as u32`, `len() as u16` | Name arena offsets |
| `parse/types.rs` | 79, 89 | `len() as u16` | Name/stream counts |

### Recommended Fix: `u32::try_from().unwrap_or()` or saturating helper

For **safety-critical** paths (index building):
```rust
/// Convert a Vec length to a u32 index, saturating at u32::MAX.
#[inline(always)]
fn len_as_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}
```

For **known-bounded** paths (extension IDs where we check `< u16::MAX`):
```rust
// Already correct pattern in extensions.rs:55
let id = self.names.len() as u16;
if id == u16::MAX { return 0; }
```
→ Replace with:
```rust
let id = u16::try_from(self.names.len()).ok()?;  // or unwrap_or(0)
```

For **`parse/types.rs`** (`name_count`, `stream_count`):
```rust
pub fn name_count(&self) -> u16 {
    u16::try_from(self.names.len()).unwrap_or(u16::MAX)
}
```

---

## Class C — Display-only `as f64` (cast_precision_loss)

### Root Cause

Converting `u64` or `usize` to `f64` for human-readable display (percentages,
throughput, file sizes). Precision loss beyond 2^53 is irrelevant for display.

### Affected Sites (18 instances)

| File | Context | Verdict |
|------|---------|---------|
| `display.rs:52` | `format_bytes()` — human file sizes | **Keep expect** |
| `reader/stats.rs:51,126,141` | Progress display (MB/s, %) | **Keep expect** |
| `commands/load.rs:131` | Compression ratio display | **Keep expect** |
| `commands/windows/save.rs:138,148` | Compression ratio display | **Keep expect** |
| `index/base.rs:486` | `display_stats()` | **Keep expect** |
| `commands.rs:80` | `format_file_size()` | **Keep expect** |
| `commands/search/dispatch.rs:368` | Throughput display | **Keep expect** |
| `uffs-tui/src/main.rs:370` | TUI file size display | **Keep expect** |
| `uffs-diag/src/parity/stats.rs:29,41` | Statistical % and mean | **Keep expect** |

### Recommended Fix: Centralized `DisplayFloat` helper (optional)

These are all legitimate. The current `#[expect(...)]` with reason strings is
the correct Rust approach. **No action needed.**

Optionally: create a `display_utils` module with `fn to_display_f64(v: u64) -> f64`
to centralize the expect in one place. But this is cosmetic, not a correctness fix.

---

## Class D — NTFS Parser Module-Level Blankets

### Root Cause

`io/parser/index.rs` and `parse.rs` use module-level `#![expect(...)]` for
`cast_lossless`, `cast_sign_loss`, and `cast_possible_truncation`. These
blankets cover the entire file (~800 LOC of dense NTFS parsing).

### Affected Sites

| File | Lint | Context |
|------|------|---------|
| `io/parser/index.rs:22` | `cast_lossless` | `u32 as u64` for NTFS field widening |
| `io/parser/index.rs:26` | `cast_sign_loss` | `i64.max(0) as u64` for timestamps |
| `io/parser/index.rs:73` | `cast_possible_truncation` | NTFS field sizes bounded by u16/u32 |
| `io/parser/index_extension.rs:44` | `cast_possible_truncation` | Same pattern |
| `io/parser/fragment.rs:33` | `cast_possible_truncation` | Same pattern |
| `io/parser/fragment_extension.rs:29` | `cast_possible_truncation` | Same pattern |
| `io/parser/unified.rs:90` | `cast_possible_truncation` | Same pattern |
| `parse.rs:27` | `cast_sign_loss` | NTFS signed fields |
| `parse.rs:31` | `cast_lossless` | Widening casts |
| `parse/index_helpers.rs:8` | `cast_possible_truncation` | Helper functions |
| `parse/direct_index.rs:78` | `cast_possible_truncation` | Offline parser |
| `parse/direct_index_extension.rs:71` | `cast_possible_truncation` | Offline parser |

### Recommended Fix: Replace `as` with `From`/`Into` for widening; targeted inline for narrowing

**For `cast_lossless`** (`u32 as u64`):
Replace all `x as u64` with `u64::from(x)` where `x: u32`. This is a
mechanical search-and-replace within parser files. Zero risk.

**For `cast_sign_loss`** (`i64.max(0) as u64`):
Replace with a helper:
```rust
#[inline(always)]
fn nonneg_to_u64(val: i64) -> u64 {
    val.max(0) as u64  // or: u64::try_from(val.max(0)).unwrap_or(0)
}
```
Or use `.try_into().unwrap_or(0)`.

**For `cast_possible_truncation`** in parsers:
These are reading NTFS struct offsets (u16/u32) from byte buffers. The values
are always bounded by the record size (1024 or 4096 bytes). The casts are:
- `offset as usize` where offset is `u16` or `u32` — actually lossless on 64-bit
- `header_field as usize` — same

→ Replace `offset as usize` with `usize::from(offset)` for u16, or
`offset as usize` is actually fine for u32→usize on 64-bit. Use
`usize::try_from(offset).unwrap_or(0)` if paranoid.

**Execution**: Remove module-level blankets, add targeted inline expects only
where truly needed. This is 2-3 PRs touching parser files.

---

## Class E — Diagnostic Tool Blankets

### Affected Files

| File | Blankets |
|------|----------|
| `uffs-diag/src/bin/compare_scan_parity.rs` | `cast_precision_loss`, `cast_possible_truncation`, `cast_possible_wrap`, `cast_sign_loss` |
| `uffs-diag/src/bin/analyze_diff.rs` | `cast_precision_loss` |
| `uffs-diag/src/bin/compare_raw_mft.rs` | `cast_precision_loss` |
| `uffs-diag/src/bin/dump_mft_records.rs` | `cast_possible_truncation` |
| `uffs-diag/src/bin/verify_iocp_capture.rs` | `cast_precision_loss`, `cast_possible_truncation` |

### Verdict: **Low priority — leave as-is**

These are diagnostic/development tools, not shipped library code. The blanket
allows are acceptable for tools that do statistical comparisons with floating
point. Fix last (or never).

---

## Class F — DateTime Math (`append_datetime`)

### Affected Site

`output/row_writer.rs:551-556` — `cast_sign_loss` + `cast_possible_truncation`

### Analysis

The `append_datetime` function does civil time decomposition from Unix
microseconds. The `rem_euclid(86_400)` result is guaranteed non-negative by
the mathematical definition of `rem_euclid`. The `doe` variable is bounded by
the era calculation (max 146,096). These casts are **provably correct**.

### Verdict: **Keep `#[expect]` — mathematically bounded**

---

## Class G — NTFS Boot Sector / Data Runs

### Affected Sites

| File | Line | Cast | Analysis |
|------|------|------|----------|
| `ntfs/boot_sector.rs:83` | `clusters_per_file_record as u32` | Checked positive on line 82 |
| `ntfs/boot_sector.rs:87` | `(-negative) as u32` | Negated negative is positive |
| `ntfs/boot_sector.rs:97` | `mft_start_lcn as u64` | MFT LCN always non-negative |
| `ntfs/data_runs.rs:34` | `lcn as u64` | Checked positive on line 36 |
| `ntfs/data_runs.rs:92` | `cast_possible_wrap` | Cluster counts in i64 range |

### Verdict: **Keep `#[expect]` — hardware-checked values**

These are reading hardware-defined NTFS structures where the sign/range is
guaranteed by the filesystem specification and validated before the cast.

---

## Class H — Test Code

### Affected Sites

| File | Count |
|------|-------|
| `index.rs` (test cfg) | 2 (`cast_possible_truncation`, `cast_sign_loss`) |
| `parse/tests.rs` | 5 (`cast_possible_truncation`) |
| `index/tests_merge.rs` | 1 (`cast_possible_truncation`) |
| `raw/tests.rs` | 2 (`cast_possible_truncation`) |
| `ntfs/tests.rs` | 1 (`cast_possible_truncation`) |
| `core/tree/mod.rs` test | 1 (`cast_possible_truncation`) |

### Recommended Fix

Use explicit `u32::try_from(x).unwrap()` or typed constants in test code.
Low priority — test code clarity matters less than production code.

---

## Execution Progress

### ✅ COMPLETED — Helper Infrastructure (2026-03-23)

Created three centralized casting helpers in `index/types.rs`, re-exported
from `lib.rs` so ALL targets (lib, bins, downstream crates) use the same
single-source-of-truth functions:

```rust
pub const fn frs_to_usize(frs: u64) -> usize;  // FRS → Vec index
pub const fn len_to_u32(len: usize) -> u32;     // Vec::len() → linked-list index
pub const fn len_to_u16(len: usize) -> u16;     // Vec::len() → name/stream count
```

Each includes a `debug_assert!` that catches overflow in debug builds.

### ✅ COMPLETED — Phase A: FRS + Vec::len Casts (42 suppressions removed)

Replaced all `frs as usize` and `len() as u32/u16` casts with helper calls
in **21 files**:

| File | Casts Fixed |
|------|-------------|
| `index/base.rs` | 5 (3× `frs as usize`, 2× `len() as u32`) |
| `index/merge.rs` | 7 (all `len() as u32` offset adjustments) |
| `index/builder.rs` | ~15 (FRS, len→u32, len→u16, name counts) |
| `index/child_order.rs` | 3 (`frs as usize`, `len() as u32`, name index) |
| `index/fragment.rs` | 2 (`frs as usize`, `len() as u32`) |
| `index/usn.rs` | 5 (`frs as usize`, `len→u32`, `len→u16`) |
| `index/extensions.rs` | 4 (`len→u16`, ext_id casts) |
| `index/dataframe.rs` | 1 (removed blanket) |
| `index/storage/serialize.rs` | 3 (`len→u32`, `len→u16`) |
| `index/stats.rs` | 2 (replaced `f64.log2() as usize` with integer `leading_zeros`) |
| `parse/merger.rs` | 2 (`frs as usize`) |
| `parse/types.rs` | 2 (`len→u16` for name/stream counts) |
| `commands/load.rs` | 3 (`len→u16`, `frs_to_usize` for capacity) |
| `reader/persistence.rs` | 2 (`frs_to_usize` for record_count) |
| `raw_iocp.rs` | 5 (`len→u32`, `frs_to_usize`) |
| `raw/mod.rs` | 2 (removed blankets) |
| `uffs-core/path_resolver/fast.rs` | 4 (`frs_to_usize` for FRS lookups) |

**Status**: `cargo check --workspace` ✅ zero warnings, zero errors.
**Status**: `cargo test --workspace` ✅ all tests pass.

---

## Remaining Work (63 suppressions across 4 categories)

### Category 1 — Parser Module-Level Blankets (8 files, ~11 suppressions)

These files use `#![expect(clippy::cast_possible_truncation)]` at module level
to cover hundreds of `as` casts inside dense NTFS record-parsing loops.

| File | Raw `as` Casts | Blanket Covers |
|------|---------------|----------------|
| `io/parser/index.rs` | 35 | `cast_possible_truncation`, `cast_lossless`, `cast_sign_loss` |
| `io/parser/index_extension.rs` | 70 | `cast_possible_truncation` |
| `io/parser/fragment.rs` | 33 | `cast_possible_truncation` |
| `io/parser/fragment_extension.rs` | 38 | `cast_possible_truncation` |
| `io/parser/unified.rs` | 29 | `cast_possible_truncation` |
| `parse/direct_index.rs` | 35 | `cast_possible_truncation` |
| `parse/direct_index_extension.rs` | 61 | `cast_possible_truncation` |
| `parse/index_helpers.rs` | 17 | `cast_possible_truncation` |
| **Total** | **318** | |

**Strategy — THREE sub-phases:**

**1a. Remove `cast_lossless` blankets** (2 files: `io/parser/index.rs`, `parse.rs`)
- Replace every `u32_value as u64` → `u64::from(u32_value)` in these files.
- Replace every `u16_value as u32` → `u32::from(u16_value)`.
- These are widening casts, always safe. Purely mechanical, zero risk.
- Estimated: ~40 replacements per file.

**1b. Remove `cast_sign_loss` blankets** (2 files: `io/parser/index.rs`, `parse.rs`)
- The pattern is `i64_value.max(0) as u64` — clamp-then-cast for NTFS timestamps.
- Create a helper: `fn nonneg_to_u64(val: i64) -> u64 { val.max(0) as u64 }`
  in `index/types.rs`, or use `u64::try_from(val.max(0)).unwrap_or(0)`.
- Replace all ~15 occurrences, remove the 2 module-level blankets.

**1c. Keep `cast_possible_truncation` blankets in parser files** (8 files)
- These are NTFS struct field reads: `u16` offsets and `u32` sizes that are
  mathematically bounded by the MFT record size (1024–4096 bytes).
- Converting 318 individual casts to `usize::from()` would:
  - Add 318 lines of noise to performance-critical hot paths
  - Not improve correctness (the values are already bounded)
  - Hurt readability of the C++-parity parser code
- **Decision**: Keep `#![expect(cast_possible_truncation)]` in parser files,
  but update reason strings to: `"NTFS struct field reads bounded by record
  size (1024–4096 bytes)"`.

### Category 2 — Easy Stragglers (3 suppressions)

| File | Cast | Fix |
|------|------|-----|
| `uffs-core/path_resolver/arena.rs` | `buffer.len() as u32`, `name.len() as u16` | Use `len_to_u32()` / `len_to_u16()` via `uffs_mft::` |
| `uffs-core/path_resolver/fast.rs` | `get_entry` method | Already uses `frs as usize` — replace with `frs_to_usize` |
| `uffs-cli/commands/raw_io.rs` | 1× `cast_possible_truncation` | Inspect and apply helper |

### Category 3 — Legitimate Keeps (30+ suppressions, no code changes needed)

| Type | Count | Reason |
|------|-------|--------|
| `cast_precision_loss` (display floats) | 17 | `u64 as f64` for human-readable sizes/percentages — precision loss past 2^53 is irrelevant for display |
| `cast_sign_loss` (NTFS boot sector) | 3 | Hardware values with pre-validation checks |
| `cast_possible_wrap` | 4 | Bounded arithmetic (benchmark overhead, ADS diff, cluster counts) |
| `cast_sign_loss` + `cast_possible_truncation` (datetime) | 2 | Mathematically proven bounded by `rem_euclid` |

**Action**: Standardize reason strings for consistency. No code changes.

### Category 4 — Test Code + Diagnostic Tools (14 suppressions)

| File | Count | Priority |
|------|-------|----------|
| `parse/tests.rs` | 5 | Low — test data constants |
| `raw/tests.rs` | 2 | Low |
| `ntfs/tests.rs` | 1 | Low |
| `index/tests_merge.rs` | 1 | Low |
| `index.rs` (test cfg) | 2 | Low |
| `uffs-core/tree/mod.rs` test | 1 | Low |
| `uffs-diag` binaries (3 files) | 3+ blankets | Low — diagnostic tools |

**Action**: Replace `as` casts with `u32::try_from(x).unwrap()` in test code
for consistency. Diagnostic tool blankets can stay.

---

## Scorecard

| Phase | Suppressions | Status |
|-------|-------------|--------|
| Helper infrastructure | — | ✅ Done |
| Phase A: FRS + Vec::len | 42 removed | ✅ Done |
| Category 1a: `cast_lossless` | ~2 blankets (80 casts) | ⏳ Next |
| Category 1b: `cast_sign_loss` | ~2 blankets (15 casts) | ⏳ Next |
| Category 1c: Parser `cast_possible_truncation` | 8 blankets | 📌 Keep (update reasons) |
| Category 2: Easy stragglers | 3 | ⏳ Quick wins |
| Category 3: Legitimate keeps | ~30 | 📌 Standardize reasons only |
| Category 4: Test + diag | ~14 | ⏳ Low priority |
| **Total removed so far** | **42 of 105** | **40%** |
| **Expected final total** | **~60 removed** | **57%** |
| **Remaining legitimate** | **~45 kept with reasons** | Standards-compliant |
