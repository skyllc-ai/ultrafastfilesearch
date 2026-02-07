# UFFS Trial Run Parity – Next Fixes (based on 2026-02-06 11:23 analysis output)

This document is based on the **updated parity analysis output you pasted** (the run where:

- **C++ live** rows: **2,221,315**
- **Rust live** rows: **871,025**
- **Rust offline** rows: **2,221,315**
- Live match rate: **39.2121%**
- Live ADS mismatch: **C++ 97,308 vs Rust 39**
- Tree metrics misaligned: **53,795 (live)** and **124,722 (offline)**

The goal here is to translate those numbers into **specific, code-level fixes** (and the minimum diagnostics needed to confirm each fix worked).

---

## 0) Triage: what the new numbers imply

### 0.1 LIVE scan is missing ~1.35M entries (this is now the #1 problem)

From your LIVE scan comparison:

- Common paths: **871,025**
- C++ only: **1,350,290**
- Rust only: **0**

This is not “small drift” — it’s a **systematic omission** in the Rust *live* pipeline.

**Very high-signal arithmetic check:**

- `2,221,315 − 871,025 = 1,350,290`

That’s *exactly* the “C++ only” count.

This strongly suggests the Rust live output is a **strict subset** of the C++ output (not a normalization mismatch), and the subset size is suspiciously plausible as **“directories-only” output** (or “directories + very little else”).

**Fast confirmation to add (analysis tool or quick one-off):**

- Count Rust-live rows that are files vs directories.
- If Rust-live has ~0 files, the bug is in **selection/emission**, not “a few missing records”.

Until live rows match offline/C++ again, tree metrics and ADS deltas are mostly downstream noise.

### 0.2 OFFLINE scan is complete (path & ADS match), so your base parsing can be correct

Offline:

- Path match: **100.0000%**
- ADS: **97,308 vs 97,308** ✅

This tells us the system *can* produce full parity, and the big mismatch is isolated to the **LIVE pipeline** (I/O + chunk processing + emission).

### 0.3 Tree metric mismatch count is large, but samples look like small systematic deltas

Your samples show many diffs like:

- `48 vs 56` (**+8**)
- `65712 vs 65776` (**+64**)
- `131424 vs 131432` (**+8**)

Those are *exactly what you’d expect* if Rust is counting `$I30:$BITMAP` bytes into directory sizes while the C++ reference does not (bitmap resident sizes are often 8 bytes, 64 bytes, etc depending on number of index blocks).

So: the mismatch *count* looks scary, but it’s likely one systematic rule difference.

---

So: the **count** of mismatches is large, but it’s likely one systematic rule difference, not 100k random bugs.

---

## 1) Fix #1: Directory size semantics — EXCLUDE `$I30:$BITMAP` from directory “size”

### 1.1 Why this fix is strongly indicated

The delta patterns (**+8**, **+64**) match typical index bitmap sizes:

- `$I30:$BITMAP` resident value length can be **8 bytes** for small indexes,
- or **64 bytes** (512 bits) for indexes with more blocks,
- and so on.

If Rust adds bitmap bytes to directory size and C++ does not, then:

- **Every directory with a bitmap** mismatches,
- and **tree size** mismatches “add up” across subtrees.

That matches what you’re seeing (large mismatch counts, small per-node deltas).

### 1.2 Where to implement (you likely need both)

You likely need this in **both** pipelines:

1. **The “current” parser → `ParsedRecord.size` / `ParsedRecord.allocated_size`**
   - This is used by your **offline scan**.
   - In `index.rs`, `MftIndex::from_parsed_records()` currently treats directory size as:
     
     > `$INDEX_ROOT + $INDEX_ALLOCATION + $BITMAP` (comment currently says this is “C++ parity”)

     Based on your mismatch signatures, that comment is almost certainly wrong. Update parsing so directory size is:

     > `$INDEX_ROOT + $INDEX_ALLOCATION` (**exclude `$BITMAP`**)

2. **The C++ port parser (`cpp_types.rs`)**
   - If/when your live pipeline uses the C++ port parsing, it must also exclude bitmap or it will reintroduce the same mismatch.

---

### 1.3 Concrete code change in `cpp_types.rs` (C++ port parser)

File: `cpp_types.rs`  
Function: `fn parse_stream(...)`

You already detect `$I30` streams here:

```rust
let is_dir_index = matches!(
    type_code,
    AttributeType::Bitmap | AttributeType::IndexRoot | AttributeType::IndexAllocation,
) && stream.name.as_str() == "$I30";
```

**Add an early return to ignore `$I30:$BITMAP`:**

```diff
diff --git a/cpp_types.rs b/cpp_types.rs
@@ fn parse_stream(index: &mut CppMftIndex, frs_base: u32, attr_data: &[u8], is_non_resident: bool) {
     let is_dir_index = matches!(
         type_code,
         AttributeType::Bitmap | AttributeType::IndexRoot | AttributeType::IndexAllocation,
     ) && stream.name.as_str() == "$I30";
+
+    // Parity: C++ does NOT include $I30:$BITMAP bytes in directory size.
+    // Evidence: many mismatches are exactly +8 or +64, typical bitmap resident sizes.
+    if is_dir_index && type_code == AttributeType::Bitmap {
+        return;
+    }
 
     // existing logic continues...
 }
```

**Expected outcome:**

- The huge counts of tree-size mismatches that are only off by +8/+64 should largely disappear.

---

### 1.4 Fix the same issue in the “current” parser path (offline)

Because your **offline** output matches on paths/ADS but not on tree size, the current parser path is almost certainly counting bitmap bytes.

Recommended implementation approach:

- Fix it at parse time where directory size is accumulated.
- Then update the misleading comment in `index.rs`:

`index.rs` (around `MftIndex::from_parsed_records`), current comment:

```rust
// ... directories: $INDEX_ROOT + $INDEX_ALLOCATION + $BITMAP
```

Change it to:

```rust
// ... directories: $INDEX_ROOT + $INDEX_ALLOCATION (exclude $BITMAP)
```

(That comment is currently reinforcing the wrong behavior.)

---

### 1.5 Add a focused regression test (prevents backsliding)

Add a unit/integration test that parses a directory record containing:

- `$I30:$INDEX_ROOT` (resident)
- `$I30:$INDEX_ALLOCATION` (non-resident)
- `$I30:$BITMAP` (resident, e.g. value length 8)

Assert:

```rust
assert_eq!(dir_size_with_bitmap, dir_size_without_bitmap);
```

Even better: assert exact expected sizes when you know the test record’s values.

---

## 2) Fix #2: LIVE scan is outputting only ~39% of entries

This is the “stop everything else until fixed” issue.

### 2.1 Add one diagnostic that immediately classifies the failure mode

In the **live scan path**, print (to stderr/log, not CSV):

- total emitted rows
- emitted dirs
- emitted files
- emitted ADS rows
- parsed stats: `file_count` / `dir_count`

If `file_count` is normal but emitted files are ~0 ⇒ bug is in **emission/filtering**.  
If `file_count` is ~0 ⇒ bug is in **live parsing / chunk pipeline**.

Suggested minimal log at end of scan:

```rust
log::warn!(
  "LIVE SUMMARY: rows={} files={} dirs={} ads_rows={}",
  rows_total, files_total, dirs_total, ads_total
);
```

If you can also log parsed stats:

```rust
log::warn!(
  "INDEX STATS: records={} in_use={} names={} streams={}",
  index.stats.record_count,
  index.stats.in_use_records,
  index.stats.name_count,
  index.stats.stream_count,
);
```

---

### 2.2 Very likely cause: an experimental env setting (`cpp_port`) is still active

Your env selectors in `index.rs`:

- `UFFS_PARSE_ALGO` → `current | cpp_port`
- `UFFS_IO_ALGO` → `current | cpp_port`
- `UFFS_CHUNK_ALGO` → `current | cpp_port` (**comment says TODO**)

**Critical point:** `ChunkAlgorithm::CppPort` is annotated “TODO”.

If `UFFS_CHUNK_ALGO=cpp_port` is set and that path is incomplete, you can absolutely end up emitting only a subset of entries.

✅ **Hardening fix (strongly recommended):**

Require an explicit “I know this is experimental” opt-in before enabling cpp_port chunk algo.

In `index.rs`:

```rust
"cpp_port" => {
    if std::env::var_os("UFFS_EXPERIMENTAL").is_some() {
        Self::CppPort
    } else {
        log::warn!("UFFS_CHUNK_ALGO=cpp_port ignored (experimental). Using current.");
        Self::Current
    }
}
```

Do the same guard for `UFFS_IO_ALGO` / `UFFS_PARSE_ALGO` if you want to prevent accidental “global shell env” poisoning.

**Expected outcome:** you can’t silently run a half-implemented pipeline and think parity regressed.

---

### 2.3 Add an always-on “parity safety fallback” for live scans

If you detect:

- emitted rows ≪ expected (or)
- parsed names ≪ `bitmap.in_use_records`

then auto-fallback:

- switch to `ChunkAlgorithm::Current`, or
- disable trimming in the cpp IO pipeline, or
- rerun parsing with the current parser.

Example condition:

```rust
if names_parsed < (in_use_records as f64 * 0.95) as u64 {
    log::error!("LIVE parse produced too few names; falling back to safe pipeline");
    // fallback path
}
```

This prevents “release builds that produce incomplete datasets” even if the experimental path is chosen.

---

### 2.4 If the live issue is in the C++ IO pipeline trimming: add a kill-switch

Even if trimming is correct in theory, a bug can skip clusters containing in-use records.

**Isolation switch:** disable trimming completely (keep record-level skipping), and compare parity.

In `cpp_io_pipeline.rs`, inside `compute_skip_ranges`:

```rust
if std::env::var_os("UFFS_CPP_DISABLE_TRIM").is_some() {
    log::warn!("UFFS_CPP_DISABLE_TRIM set: disabling cluster trimming");
    return;
}
```

Then rerun live scan:

- If rows jump back to ~2.22M ⇒ trimming logic is the culprit.
- If still ~0.87M ⇒ output filtering or another pipeline issue.

---

### 2.5 Make trimming “safe”: verify skipped clusters are truly unused

If you keep trimming, make it impossible to skip an in-use record:

After `(skip_begin, skip_end)` for a chunk, verify via bitmap:

- each skipped cluster at the beginning is all-unused
- same for skipped clusters at the end

If verification fails:

- clamp skip_begin/skip_end to 0 for that chunk
- log vcn/lcn

This turns trimming bugs into performance loss (acceptable) instead of correctness loss (unacceptable).

---

## 3) Fix #3: Improve the parity analysis tool to surface “what’s missing” instantly

Your new analyzer is already much better. Two additions make it *surgical*:

### 3.1 Output composition counts

For each output:

- total rows
- directory rows
- file rows
- ADS rows

This will confirm/disprove “directories-only live output” immediately.

### 3.2 “Missing path prefixes” histogram

Take the first N missing C++ paths (e.g. 10k) and group by top-level prefix:

- `f:/windows/...`
- `f:/program files/...`

If everything missing is “files under directories that exist”, it supports the “files not emitted” hypothesis.

---

## 4) Fix #4: Tripwire verification — align the analyzer with the new tripwire

Your analyzer says:

> Tripwire NOT found → binary may not have fixed cpp_tree code

…but you now have runtime `[TRIP]` logs.

That usually means the analyzer is searching for an older exact string.

Recommendation:

- Treat tripwire as present if **either**:
  - the binary contains `UFFS_TRIPWIRE_*` (via `strings`), **or**
  - the log contains runtime `[TRIP]` markers

Also consider printing the tripwire string once at startup to stderr/log.

---

## 5) “Do these in order” checklist

### Step 1 — fix live completeness first

1. Add live summary counts (rows/files/dirs/ads).
2. Guard against accidental `cpp_port` selection (require `UFFS_EXPERIMENTAL=1`).
3. Add `UFFS_CPP_DISABLE_TRIM` to isolate trimming.

✅ Success condition: Rust live rows return to ~2.22M and ADS rows return to ~97k.

### Step 2 — fix directory size semantics

4. Exclude `$I30:$BITMAP` from directory size (cpp_types **and** current parser).
5. Re-run parity: misaligned tree entries should drop massively.

### Step 3 — handle the remaining real deltas

6. Whatever is left after bitmap removal is likely “real” mismatch, not systematic +8/+64.

---

## Appendix A: Why `$I30:$BITMAP` is the best-fit explanation for the +8/+64 deltas

Directory indexes need a bitmap tracking used index blocks. It is typically stored as a resident `$BITMAP` attribute. Common resident bitmap sizes are 8 bytes, 64 bytes, etc.

Your mismatches are frequently Rust = C++ + 8 or + 64, which is the signature of “bitmap counted on one side but not the other”.

---

## Appendix B: Why the line-count arithmetic screams “directories-only live output”

From live:

- C++: 2,221,315
- Rust:   871,025

Difference: 1,350,290.

If Rust emitted only directories (871,025) and C++ emitted dirs + files + ADS, then:

- `files + ADS = 1,350,290`
- with ADS = 97,308, files ≈ 1,252,982

That breakdown is extremely plausible, which is why “directories-only live emission” is currently the #1 hypothesis to confirm.

