# UFFS parity: proposed fixes (live + offline) — 2026-02-06

This document proposes **incremental, code-level fixes** to restore/achieve parity between the C++ scanner output and the Rust scanner output, based on the artifacts you posted:

- `scripts/analyze_trial_parity.rs` output for:
  - **H:** drive (0% path match regression + root tree metrics wrong)
  - **F:** drive (100% path match but **20 directory tree-size mismatches** in both live and offline)
- The uploaded Rust source files:
  - `cpp_types.rs` (C++-port MFT parsing)
  - `cpp_tree.rs` (C++-port tree metrics)
  - plus `index.rs`, `reader.rs`, `cpp_io_pipeline.rs` for context

The goal is to get back to:

- **CSV parse parity** (the parity analyzer must be able to parse Rust output as CSV)
- **Path-set parity** (100% common paths)
- **Row-count parity** (no duplicates / no missing)
- **Tree metrics parity** (directory `Size` and `Descendants` match exactly)
- **Tripwire verification** present in *logs/binary* without breaking CSV

---

## 0. Symptoms recap (what the evidence says)

### 0.1 H: drive regression is *not* a real path regression — it’s a CSV contamination regression

Your `rust_live_h.txt` is **not pure CSV**:

- It begins with a comment line:
  - `# TRIPWIRE: UFFS_CPP_TREE_FIX_...`
- It ends with non-CSV footer text (e.g.):
  - `Drives? 1 H:`
  - `MMMmmm that was FAST ...`
  - `'Search path...'` usage/help text

That alone is sufficient to make a strict CSV reader (like your parity analyzer) mis-parse columns and conclude **0% path match** even if the CSV portion is correct.

You can see it directly in the file:

```text
# TRIPWIRE: UFFS_CPP_TREE_FIX_2026_02_05_195
"Path","Name",...
...
Drives? 1 H:
MMMmmm that was FAST ...
Search path. E.g. 'C:/' ...
```

✅ After stripping the comment + footer, the **paths align** with C++ on H:.

**Action:** make Rust `--csv` output *CSV-only* (Fix 1).

---

### 0.2 H: drive live scan root tree metrics are still wrong

After filtering to valid CSV rows, **every row matches** C++ *except* the root row:

- C++ `H:\` Size: `42168722`, Descendants: `57`
- Rust live `H:\` Size: `31065729314`, Descendants: `54`

That suggests one of these is happening in *live mode only*:

- orphan/self-heal attachment is inflating root but not producing printable paths
- system-file records are included in the tree sum in live mode but not in offline/C++
- a live-only parse bug is producing a few records with absurd sizes that get folded into the root

**Action:** tighten “what counts toward the tree” to the Win32-visible namespace and/or stop counting unprintable orphans in parity mode (Fix 5).

---

### 0.3 F: drive live scan has 20 directory tree-size mismatches, descendants match

Example from your latest parity run (live):

- `...StartMenu\assets\` Size: C++=65744, Rust=65752 (**+8**)
- `...credprov.resources...\` Size: C++=48, Rust=56 (**+8**)
- `...eventlog...\` Size: C++=336, Rust=184 (**-152**)
- `...aarsvc...\` Size: C++=196800, Rust=131272 (**-65528**)

Key pattern:

- **Descendants match**, so child linking / stream counting is already in the right ballpark.
- Only **Size** is drifting, often by:
  - **+8** (classic “bitmap/8-byte” signature)
  - **~65536** scale (classic “allocated vs data size / cluster rounding” signature)
  - **152** (classic “a resident attribute blob was included/excluded” signature)

This strongly points to **stream size semantics** mismatching, not structural tree errors.

**Action:** adjust `$I30` component semantics + directory “printed size” semantics (Fix 2 + Fix 3).

---

### 0.4 F: drive live row-count mismatch (+11 rows) but path-set parity is 100%

Live counts:

- C++ rows: 2,221,315
- Rust live rows: 2,221,326 (**+11**)
- Common paths: 2,221,315
- Rust-only paths: 0

That’s the signature of **duplicate rows for the same path** in Rust output.

The most likely culprit (common NTFS edge-case):

- parsing two FILE_NAME attributes that represent the same Win32-visible name (e.g. Win32 + Win32&DOS) and emitting both

**Action:** filter + dedupe FILE_NAME namespace handling in the C++-port parser (Fix 4).

---

## 1) Fix 1 — Make `--csv` output *strict CSV* (no tripwire/comments/footers on stdout)

### Why this is necessary

Your parity analyzer expects files like `cpp_*.txt` and `rust_*.txt` to be machine-parseable CSV.

Right now Rust live output is polluted with:

- a `# TRIPWIRE:` comment line at top
- human-readable footer/help text at end

That breaks strict CSV parsing and produces false “0% path match” diagnostics.

### What to change

**Rule:** When `--csv` is enabled, **stdout must contain only**:

1) the header row  
2) the data rows  

Everything else must go to **stderr** (or be disabled unless `--verbose`).

### Implementation sketch

You didn’t upload the CLI/output module, so below is a “search-and-patch” guide:

1) Find the CSV writer function:
   - search for the header string `"Path","Name","Path Only"`
2) Find any post-run printing like:
   - `MMMmmm that was FAST`
   - drive lists
   - “Search path …” usage
   - “stats” summary lines

Then gate them:

```rust
if args.csv {
    // CSV header + rows to stdout only
    write_csv(&mut stdout, ...)?;
    // optional: diagnostics go to stderr
    eprintln!("[INFO] wrote {} rows", n);
} else {
    // human output mode
    println!("MMMmmm that was FAST ...");
}
```

### Tripwire placement (don’t put it in CSV)

Instead of `# TRIPWIRE:` in the CSV file, place tripwire in one or more of:

- **binary string table**: a referenced `const TRIPWIRE: &str = "TRIPWIRE_...";`
- **log file**: `log::info!("TRIPWIRE: ...")` to the log sink
- `--version` output: already shown in your `uffs_version.log`

(See Fix 6 for concrete tripwire tactics.)

### Validation

- `head -n 3 rust_live_h.txt` should show:
  - header row as line 1
  - data rows starting line 2
- `tail -n 3 rust_live_h.txt` should still be CSV rows, **not** “MMMmmm …” or help text
- rerun parity analyzer for H:  
  - **path matching should jump back to 100%**

---

## 2) Fix 2 — Correct `$I30` directory index size semantics in `cpp_types.rs`

This one targets the **+8** and **~65536-scale** deltas you’re seeing in live F:.

### 2.1 What the deltas strongly imply

From your samples:

- **+8 bytes** deltas on directories that otherwise match:
  - strongly consistent with **including the `$I30` $BITMAP length** in Rust when C++ excludes it
- **65536-ish** deltas (one cluster) on directories:
  - strongly consistent with Rust using **IndexAllocation `data_size`** while C++ uses **IndexAllocation `allocated_size`** for the directory index contribution

A single directory example matches *perfectly* with that model:

- Rust: 131272 = 131072 + 192 + 8  
  (IndexAllocation data_size + IndexRoot + Bitmap)
- C++: 196800 = 196608 + 192  
  (IndexAllocation allocated_size + IndexRoot, bitmap excluded)

### 2.2 Where to implement (in your uploaded code)

File: `cpp_types.rs`

Relevant functions and current line numbers (from your uploaded file):

- `parse_record` around **line ~2017**
- `parse_stream` around **line ~2273**
- `update_stream_sizes` around **line ~2397**

### 2.3 Proposed change

Pass **type_code** and **is_dir_index** into `update_stream_sizes`, and special-case:

- `$I30` **IndexAllocation**: add **allocated_size** to `length` (not data_size)
- `$I30` **Bitmap**: add **0** to `length` (exclude), but keep `allocated/bulkiness` behavior if you want disk-usage correctness

#### Patch-style diff (illustrative)

```diff
diff --git a/cpp_types.rs b/cpp_types.rs
@@ fn parse_stream(...)
-        Self::update_stream_sizes(&mut record.first_stream, attr_data, is_non_resident);
+        Self::update_stream_sizes(
+            &mut record.first_stream,
+            attr_data,
+            is_non_resident,
+            attr_header.type_code,
+            is_dir_index,
+        );

@@
-    fn update_stream_sizes(stream: &mut StreamInfo, attr_data: &[u8], is_non_resident: bool) {
+    fn update_stream_sizes(
+        stream: &mut StreamInfo,
+        attr_data: &[u8],
+        is_non_resident: bool,
+        type_code: u32,
+        is_dir_index: bool,
+    ) {
         if is_non_resident {
             if attr_data.len() >= std::mem::size_of::<NonResidentAttributeData>() {
                 let non_res = unsafe {
                     std::ptr::read_unaligned(attr_data.as_ptr() as *const NonResidentAttributeData)
                 };
                 stream.size.allocated += non_res.allocated_size;
-                stream.size.length += non_res.data_size;
                 stream.size.bulkiness += non_res.allocated_size;
+
+                // C++ parity: for $I30 IndexAllocation, treat "length" as allocated_size.
+                const ATTR_INDEX_ALLOCATION: u32 = 0xA0;
+                if is_dir_index && type_code == ATTR_INDEX_ALLOCATION {
+                    stream.size.length += non_res.allocated_size;
+                } else {
+                    stream.size.length += non_res.data_size;
+                }
             }
         } else {
             if attr_data.len() >= std::mem::size_of::<ResidentAttributeData>() {
                 let res = unsafe {
                     std::ptr::read_unaligned(attr_data.as_ptr() as *const ResidentAttributeData)
                 };
-                stream.size.length += res.value_length as u64;
+
+                // C++ parity: exclude $I30 $BITMAP from "Size" (but keep it in allocated/bulkiness if desired)
+                const ATTR_BITMAP: u32 = 0xB0;
+                if !(is_dir_index && type_code == ATTR_BITMAP) {
+                    stream.size.length += res.value_length as u64;
+                }
             }
         }
     }
```

### 2.4 If you still see “exactly 152 bytes” size gaps after Fix 2+3: handle `$ATTRIBUTE_LIST` without changing descendants

If the **-152** class of mismatches persists *after*:

- `$I30` bitmap exclusion (+8 disappears), and  
- IndexAllocation length uses allocated_size (65536-scale deltas disappear), and  
- directory printed size includes all streams (Fix 3),

then the remaining -152 gaps are very likely coming from an attribute that:

- C++ **counts into Size**, but
- C++ **does not count as an extra “stream”** (so descendants remain unchanged),
- and Rust is currently skipping it.

The #1 suspect is the resident `$ATTRIBUTE_LIST` (type code **0x20**). In your current `parse_record`, unnamed resident attributes only get parsed as streams if `is_non_resident || name_length > 0`, so `$ATTRIBUTE_LIST` (resident + unnamed) is silently ignored.

**Important constraint:** If you simply turn `$ATTRIBUTE_LIST` into a new stream entry, you will probably change `stream_count` and therefore directory **Descendants**, which may *create new mismatches* if C++ folds `$ATTRIBUTE_LIST` into existing accounting.

#### Safer parity approach: fold `$ATTRIBUTE_LIST.value_length` into the directory’s existing stream-length accounting

Implementation sketch inside `parse_record`:

1) add a local accumulator:
   - `let mut attr_list_len: u64 = 0;`
2) when you see `type_code == 0x20` (attribute list):
   - parse `ResidentAttributeData.value_length` and add to `attr_list_len`
3) after the attribute loop:
   - if `attr_list_len > 0` and `record.stream_count > 0`:
     - add it into `record.first_stream.size.length` (or into the merged `$I30` stream if that’s your “directory main”)

This changes **Size** without changing **Descendants**.

Pseudo-code:

```rust
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;

let mut attr_list_len: u64 = 0;

...
// in the attribute loop:
if attr_header.type_code == ATTR_ATTRIBUTE_LIST && !is_non_resident {
    if attr_data.len() >= size_of::<ResidentAttributeData>() {
        let res = unsafe { ptr::read_unaligned(attr_data.as_ptr() as *const ResidentAttributeData) };
        attr_list_len += res.value_length as u64;
    }
    continue; // do NOT call parse_stream
}

...
// after loop:
if attr_list_len > 0 && record.stream_count > 0 {
    record.first_stream.size.length += attr_list_len;
}
```

That keeps the stream-count model stable while matching C++ “bytes counted” behavior.

### 2.5 Validation

Re-run the F: parity case and specifically re-check the sample problem dirs:

- `...assets\` should go from **65752 → 65744**
- `...credprov.resources...\` should go from **56 → 48**
- `...aarsvc...\` should go from **131272 → 196800** (assuming index allocation allocated_size is 3 clusters)

If those shift in exactly those directions, you’ve confirmed the hypothesis and fixed the bulk of the “big deltas”.

---

## 3) Fix 3 — Directory `Size` must include *all* of the directory record’s streams (not only the “first”)

This one targets the **-152** class of mismatches, and any case where a directory has multiple meaningful metadata streams (attribute blobs, reparse data, etc.).

### 3.1 Why the current Rust logic is likely too narrow

In `cpp_tree.rs`, the directory case currently does:

```rust
rec.treesize = children.length + first_len;
```

That means:

- directory’s own size contribution is **only** the first stream
- any other streams for that directory (overflow streams, internal streams) are excluded from directory Size,
  even though they *do* contribute to the parent aggregation via `agg.length`

This is a common source of “directory is too small by exactly the size of some resident blob”.

### 3.2 Where to change (in your uploaded code)

File: `cpp_tree.rs`

Directory branch is around **lines 264–269** in your upload.

### 3.3 Proposed change

For directories, make the “printed” own-size include **the delta-shared sum of all streams**, while still keeping the “main” stream full (non-delta) like the file case.

You already have the correct ingredients in preprocess:

- `own_len` = sum of *delta* sizes for all own streams (first + internal + overflow)
- `first_len` = full first-stream length
- `delta(first_len)` = delta share of first stream already computed (it’s exactly the value you used to seed `own_len`)

So define:

- `printed_own_len = own_len - delta(first_len) + first_len`

Then:

- `rec.treesize = children.length + printed_own_len`

#### Patch-style diff (illustrative)

```diff
diff --git a/cpp_tree.rs b/cpp_tree.rs
@@ fn preprocess(...)
-        if is_dir {
-            // Directory: printed metrics exclude internal streams and overflow streams from own row,
-            // but still propagate through aggregation.
-            rec.treesize = children.length + first_len;
-            rec.tree_allocated = children.allocated + first_alloc;
-            rec.descendants = children.treesize + 1;
-        } else {
+        if is_dir {
+            // Directory: include *all* directory-record streams in Size (delta-shared),
+            // but keep the main stream full-sized (non-delta) like the file case.
+            let delta_first_len = Self::delta(first_len, name_info, total_names);
+            let printed_own_len = own_len
+                .saturating_sub(delta_first_len)
+                .saturating_add(first_len);
+
+            let delta_first_alloc = Self::delta(first_alloc, name_info, total_names);
+            let printed_own_alloc = own_alloc
+                .saturating_sub(delta_first_alloc)
+                .saturating_add(first_alloc);
+
+            rec.treesize = children.length + printed_own_len;
+            rec.tree_allocated = children.allocated + printed_own_alloc;
+            rec.descendants = children.treesize + 1;
+        } else {
             // File: "Size" should reflect only the first stream (main data stream)
             rec.treesize = agg.length - own_len + first_len;
             rec.tree_allocated = agg.allocated - own_alloc + first_alloc;
             rec.descendants = 1;
         }
```

### 3.4 Why this is safe

- Files remain unchanged: file row `Size` stays as “main stream only”
- Directories become “tree-of-children + full main stream + other streams (delta-shared)”
- This tends to match how the C++ output behaves when directory metadata blobs are present

### 3.5 Validation

This change should specifically help cases like:

- C++ size is larger by a fixed resident blob size (e.g., 152) and descendants already match.

After Fix 2+3, the “mystery -152” directories should shrink toward 0 mismatches.

---

## 4) Fix 4 — Remove duplicate paths and fix hardlink name counting in `parse_file_name`

This targets the **row-count mismatch** (+11 duplicate rows) and also helps the **small ±1..11 byte** mismatches from remainder distribution.

### 4.1 Why duplicates happen

NTFS can store multiple FILE_NAME attributes for a record with different namespaces:

- 0x00 POSIX
- 0x01 Win32
- 0x02 DOS-only
- 0x03 Win32+DOS

For Win32-visible enumeration parity, you generally want **only**:

- 0x01 and 0x03

If you also include POSIX (0x00), or you accept both 0x01 and 0x03 when they refer to the same Win32 name, you can:

- generate duplicate child edges
- inflate `name_count`
- change the hardlink delta rounding distribution
- produce duplicate output rows for identical paths

### 4.2 Where to change

File: `cpp_types.rs`

Function: `parse_file_name` around **line ~2166**.

### 4.3 Proposed change

- Skip namespaces that are not Win32-visible (`0x00`, `0x02`)
- Deduplicate `(parent_directory, name_string)` within a record

#### Patch-style diff (illustrative)

```diff
diff --git a/cpp_types.rs b/cpp_types.rs
@@ fn parse_file_name(...)
-        // Skip DOS-only names
-        if file_name.name_space == 0x02 {
-            return;
-        }
+        // Parity with Win32-visible enumeration:
+        // keep only Win32 (0x01) and Win32+DOS (0x03) names.
+        match file_name.name_space {
+            0x01 | 0x03 => {}
+            _ => return, // skip POSIX (0x00) and DOS-only (0x02)
+        }

@@
-        let name_string = Self::parse_utf16_name(&file_name.name);
-        let name_string_idx = index.names.len() as u32;
-        index.names.push(name_string);
+        let name_string = Self::parse_utf16_name(&file_name.name);
+
+        // Deduplicate exact same (parent_id, name) within this record.
+        // (Prevents duplicate paths when both 0x01 and 0x03 carry the same Win32 name.)
+        if Self::record_already_has_name(index, record, parent_id, &name_string) {
+            return;
+        }
+
+        let name_string_idx = index.names.len() as u32;
+        index.names.push(name_string);

@@
         record.name_count += 1;
```

And add a helper (still in `cpp_types.rs`):

```rust
fn record_already_has_name(
    index: &CppMftIndex,
    record: &Record,
    parent_id: u64,
    name: &str,
) -> bool {
    if record.name_count == 0 {
        return false;
    }

    // Check first_name
    let first = &record.first_name;
    if first.parent_id == parent_id {
        let s = &index.names[first.name_string as usize];
        if s == name {
            return true;
        }
    }

    // Check overflow names chain
    let mut next = record.first_name.next_entry;
    while next != 0 {
        let ni = &index.nameinfos[next as usize];
        if ni.parent_id == parent_id {
            let s = &index.names[ni.name_string as usize];
            if s == name {
                return true;
            }
        }
        next = ni.next_entry;
    }

    false
}
```

### 4.4 Validation

- live row counts should match exactly (your +11 should disappear)
- the remaining F: offline ±1..11 byte directory size mismatches should reduce significantly (hardlink rounding is very sensitive to name_count)

---

## 5) Fix 5 — Root-only mismatch in H: live scan: stop counting unprintable orphans in parity mode

This is the “root is huge but everything else matches” issue.

### Likely mechanism

Your trace mentions:

- `orphan count: 4`
- “Added 4 orphans”

If those orphans have:

- missing/invalid Win32 path reconstruction, so they aren’t printed as rows
- but they *are* included in tree aggregation

Then:

- root Size becomes inflated
- root Descendants changes
- but the visible path set (rows) looks normal

### Proposed parity behavior

For parity (matching C++ Win32 enumeration), only include:

- records reachable from the root via **Win32-visible FILE_NAME edges**
- do *not* attach or count orphans that cannot produce a Win32 path

This pairs naturally with Fix 4 (namespace filtering).

### Implementation sketch

Wherever you do orphan sweep / self-heal (not in uploaded files):

- add a flag: `--include-orphans` (default false in CSV parity mode)
- in parity mode:
  - do not attach orphans
  - or attach them but exclude them from `tree_size` aggregation unless they also get a printable path

### Validation

After Fix 4 + Fix 5:

- H: root row should match C++ exactly.

---

## 6) Fix 6 — Tripwire that doesn’t break CSV and is verifiable from logs

### Problem

Putting the tripwire in the CSV output breaks machine parsing and causes false parity failures.

### Better tripwire options

**Option A: binary string table**

Add a referenced constant:

```rust
pub const TRIPWIRE: &str = "TRIPWIRE_UFFS_CPP_TREE_FIX_2026_02_06";
pub fn touch_tripwire() {
    std::hint::black_box(TRIPWIRE);
}
```

Call `touch_tripwire()` from startup / main.

Then:

- `strings uffs.exe | grep TRIPWIRE` works
- CSV remains clean

**Option B: log file**

Emit to the log sink used by `*_mft_save.log`:

```rust
log::info!("TRIPWIRE: TRIPWIRE_UFFS_CPP_TREE_FIX_2026_02_06");
```

Make sure it is emitted on the code path that produces the diagnostic log file that your analyzer reads.

### Validation

- `strings uffs.exe | grep TRIPWIRE`
- parity analyzer “Tripwire verification” should turn ✅

---

## 7) Suggested validation sequence (fast feedback loop)

1) **CSV sanity**
   - Generate rust live output
   - Confirm file is CSV-only:
     - no leading `#`
     - no footer/help text

2) **Run parity analyzer** for H:
   - expect: common paths 100%
   - if root differs: apply Fix 4 + Fix 5

3) **Run parity analyzer** for F:
   - after Fix 2 + Fix 3:
     - the +8 and -65528 examples should disappear
   - after Fix 4:
     - row counts should match
   - remaining ±1..11 byte diffs should drop (hardlink rounding stabilized)

4) **Tripwire**
   - confirm analyzer finds tripwire in log and/or `strings`.

---

## 8) Optional: add a one-off diagnostic to confirm `$I30` component math

For one mismatching directory record (in live mode), dump:

- IndexRoot resident value_length
- IndexAllocation data_size and allocated_size
- Bitmap value_length
- computed merged `$I30` stream length (your code)
- expected C++ formula (IndexRoot + IndexAllocation.allocated, bitmap excluded)

This will let you *prove* the model in minutes and avoid guessing.

---

## Appendix: concrete code touchpoints (uploaded files)

- `cpp_types.rs`
  - `parse_record` (~2017): where attribute types are dispatched into `parse_stream`
  - `parse_file_name` (~2166): where namespaces/dedup should be implemented
  - `parse_stream` (~2273): where `$I30` detection and size semantics are applied
  - `update_stream_sizes` (~2397): where allocated/data/bitmap rules are enforced

- `cpp_tree.rs`
  - preprocess directory branch (~264): change directory printed size to include all own streams (Fix 3)

---

If you want, I can also turn the illustrative diffs above into an actual `git apply` patch file once you paste the surrounding CLI/output code for the CSV writer + the orphan sweep path.