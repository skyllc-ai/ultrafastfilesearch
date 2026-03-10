# UFFS: Why Rust LIVE Can’t Match C++ Tree Metrics (Yet) — Deep Dive + Fix Plan

**Context:** you have **perfect parity** between C++ and Rust in **offline** mode, but **LIVE** scans still show **tree-metrics** mismatches (directory `Size`, `Descendants`) for a tiny set of paths — notably the **volume root** and some **junction/reparse directories**. fileciteturn0file1 fileciteturn0file0

This doc explains what’s *actually* happening, why it looks like “symlinks/junctions”, and what to change so the **Rust “C++ tree” mode** achieves **100% output parity** with the legacy baseline.

---

## 1) The key observation: Offline is perfect; Live is “selectively wrong”

### G: drive run
* LIVE mismatches: **3 directories** (root + 2 junctions) fileciteturn0file1  
* OFFLINE mismatches: **none** (perfect parity) fileciteturn0file1  

The LIVE mismatches are:

- `G:\` shows **C++ Size=609,893,968 / Desc=15,106** but **Rust Size=0 / Desc=0** fileciteturn0file1
- `...\PhotosJunction\` shows **C++ Desc=1** but **Rust Desc=0** (size matches at 48) fileciteturn0file1
- `...\ReportsJunction\` shows **C++ Desc=1** but **Rust Desc=0** (size matches at 48) fileciteturn0file1

### H: drive run
* LIVE mismatches: **1 directory** (root) fileciteturn0file0  
* OFFLINE mismatches: **none** (perfect parity) fileciteturn0file0  

The LIVE mismatch is:

- `H:\` shows **C++ Size=42,168,722 / Desc=57** but **Rust Size=0 / Desc=0** fileciteturn0file0

---

## 2) What this rules out (and why)

### 2.1 It’s *not* basic parsing correctness
Paths match **100%** in both LIVE and OFFLINE comparisons. That means:
- name decoding is consistent
- parent chain resolution is consistent (you can’t produce correct full paths without correct parent linkage)
- ADS counts match too, so stream enumeration is consistent at least at the “row emission” layer fileciteturn0file1 fileciteturn0file0

If the problem were “NTFS is weird” or “symlinks/junction targets”, you would typically see:
- missing/extra paths
- divergent path canonicalization
- lots of subtree size drift, not a tiny set of rows

But you don’t — everything matches except the *printed* tree metrics for a couple of special cases.

### 2.2 It’s *not* “C++ follows reparse targets and Rust doesn’t”
If C++ were following junction targets and Rust wasn’t, you would see:
- many directories with different descendant counts (whole subtrees differ)
- lots of drift, not just `Desc: 1 vs 0` on the junction directory itself

Instead, the junction directories differ only in whether they are treated like a **directory leaf** (`Desc=1` in C++) or like a **file** (`Desc=0` in Rust LIVE). fileciteturn0file1

### 2.3 It’s *not* the Rust C++-tree algorithm failing to execute
Your LIVE trace shows the Rust side entering the C++-port pipeline and calling the tree computation, including the “CppPort” dispatch. fileciteturn0file2

That means the issue is **downstream of “tree metrics were computed”** — i.e., the values are being **lost, overridden, or not wired into the printed row** for those special cases.

---

## 3) The real issue: **Row emission is bypassing (or overriding) the computed metrics for special rows**

There are **two distinct (but related) failure modes visible in your runs:

### Failure mode A — Volume root row is not using the computed root record metrics
In LIVE outputs, the volume root row (`G:\` / `H:\`) prints `Size=0` and `Descendants=0`, while many other directories print non-zero descendants.

That strongly indicates one of these implementation patterns in the LIVE row writer:

1. **Synthetic “volume root” row**  
   The UI layer often adds a “drive root row” explicitly (because NTFS root has weird naming rules), and that row is *not backed by the real FRS=5 record* — or it is backed, but you never fill its tree metrics.

2. **A “root special-case” that zeroes stats**  
   E.g. code like:  
   `if path == "X:\" { descendants = 0; size = 0; }`  
   (usually done to avoid confusing users when stats are “not ready yet”)

In both cases, the fix is the same: **the row for `X:\` must draw `treesize/descendants` from the actual root directory record (FRS=5) after tree metrics are computed.**

The evidence for a wiring/override problem (not a compute problem) is that the pipeline logs show tree metric computation happening, but the printed CSV row still shows zeros. fileciteturn0file2

---

### Failure mode B — Reparse (junction) directories are being printed “file-like”
For junctions in LIVE mode you get:

- **C++:** `Desc=1`  
- **Rust LIVE:** `Desc=0`  
- **Size matches** (48)

This is the signature of:
- treating a directory row as a file row when formatting:  
  - files print `Desc=0`
  - size prints “own stream size” (and for a leaf junction dir, “own stream size” often happens to match the tree result)

So the junction mismatch is almost certainly caused by a live-mode formatting rule like:

```text
if record.is_reparse { descendants = 0; }     // ❌ breaks parity
// or:
if record.is_reparse { treat_as_leaf_file(); } // ❌ breaks parity
```

That’s a *UI policy*, not an NTFS fact. The legacy baseline output is clearly treating the junction record as a **directory leaf**, not a file. fileciteturn0file1

---

## 4) What “C++ tree metrics” actually mean (so we match them exactly)

This section is important because it affects what you should (and should not) special-case.

### 4.1 Descendants are **not** “number of child nodes”
In your G: run:
- total output rows: 15,063
- root descendants in C++: 15,106 (slightly *larger* than row count) fileciteturn0file1

That can only happen if “Descendants” is closer to a **stream-count–style** measure (records + streams + internal streams), not a pure node-count. This is consistent with the C++-port algorithm behavior: it counts streams, and internal streams can push the value above “row count”.

### 4.2 Junction directories should still be directories
A junction/reparse-point directory is still a directory record in the MFT, and the legacy baseline output is clearly treating it that way: it prints `Desc=1` (leaf directory: “self only”). fileciteturn0file1

Crucially:
- “Not following reparse targets” ≠ “printing it as a file”
- To match C++, you do **not** have to follow the target; you just must not **override** the directory’s printed metrics.

---

## 5) The fix: Make LIVE output use the same post-tree metrics as OFFLINE

This is a *wiring* fix.

### Fix #1 — Root row must map to FRS=5 record metrics
Wherever LIVE output constructs the `X:\` row, do this:

- Look up the real root directory record:
  - `root_idx = index.frs_to_idx_opt(5)`
- Use:
  - `size = records[root_idx].treesize`
  - `desc = records[root_idx].descendants`
  - `allocated = records[root_idx].tree_allocated` (if you print it)

If you currently build the root row without a record reference (common), then after the index is built you *patch* the emitted root row using this lookup.

#### Minimal patch sketch (pseudo-Rust)

```rust
// In your "row builder / CSV exporter" (LIVE path)
if is_volume_root_row(path) {
    if let Some(root_idx) = index.frs_to_idx_opt(5) {
        let root = &index.records[root_idx];

        row.size = root.treesize;
        row.descendants = root.descendants;
        row.size_on_disk = root.tree_allocated; // if applicable
    }
}
```

**Why this is correct:** OFFLINE already matches the legacy baseline (so these fields are computed correctly); LIVE must simply emit them for the root row as well. fileciteturn0file0 fileciteturn0file1

---

### Fix #2 — Never “file-ify” a directory just because it is reparse
Remove (or gate) any logic that sets `descendants=0` or `size=0` based on `is_reparse`.

To keep safety (avoid cycles) while still matching C++:
- **Traversal**: do not follow targets (you already aren’t; you’re using MFT relationships)
- **Printing**: always print the directory’s computed metrics

#### Minimal patch sketch (pseudo-Rust)

```rust
let rec = &index.records[rec_idx];

if rec.stdinfo.is_directory() {
    // ✅ always use computed tree metrics, even if is_reparse
    row.size = rec.treesize;
    row.descendants = rec.descendants;
    row.size_on_disk = rec.tree_allocated;
} else {
    // files use their normal size
    row.size = rec.first_stream.size.length;
    row.descendants = 0;
    row.size_on_disk = rec.first_stream.size.allocated;
}
```

This alone should flip:
- junction `Desc=0` → `Desc=1` (parity) fileciteturn0file1

---

### Fix #3 — Ensure LIVE and OFFLINE go through the exact same exporter logic
Right now, OFFLINE output is correct while LIVE output is not. fileciteturn0file0 fileciteturn0file1

That implies you likely have:
- a “CLI/offline exporter” that reads `record.treesize/descendants`
- a “LIVE interactive exporter” that has special-cases (root, reparse) or uses pre-tree fields

**Best practice fix:** refactor so both modes call the same function, e.g.:

```rust
fn record_to_output_row(index: &MftIndex, rec_idx: usize, link_idx: Option<...>) -> Row
```

…and put *all* size/desc logic in that one place.

---

## 6) Validation plan (to get to “100% parity” reliably)

### 6.1 Build-time “am I running the right binary?”
Your parity analyzer warned about missing tripwires because it couldn’t find them in the *tiny* `.log` files. fileciteturn0file1 fileciteturn0file3

You already have richer traces (e.g., `rust_live_trace_h.txt`) that clearly show the tree algorithm running. fileciteturn0file2

**Actionable improvement:** make the parity tool scan:
- the “trace log” if present (preferred)
- or the main binary strings if available
- or fall back to `.log`

So you don’t chase ghosts caused by missing diagnostics.

### 6.2 Add a *post-export sanity check* for the two special cases
Right before writing rows:

1) Assert the root row:
- if `X:\` exists in output, it must have the same `Size/Desc` as record 5.

2) Assert reparse directories:
- if `is_directory && is_reparse`, descendants must be `>= 1` (in legacy-output parity mode)

This catches the bug at the exact layer it occurs (export), not 20 steps later.

### 6.3 Re-run parity for the exact failing drives
After patching:
- rerun the same trial harness that produced the reports
- expect **LIVE tree metrics issues = 0** for both G and H fileciteturn0file0 fileciteturn0file1

---

## 7) Why this fix gives you *true* 100% parity (not “close enough”)

Because:
- OFFLINE already proves the Rust data structures + tree computation can match C++ exactly fileciteturn0file1
- LIVE already proves you can parse and enumerate the same set of paths (100% match) fileciteturn0file1
- The remaining differences are in **how the final row is populated** for:
  - the “drive root” row
  - reparse directories

Once LIVE output uses the same post-tree fields as OFFLINE, parity falls out naturally.

---

## 8) Quick checklist (copy/paste for your PR)

- [ ] In LIVE exporter, `X:\` row pulls metrics from `FRS=5` record (`treesize/desc/tree_allocated`).
- [ ] Remove any exporter rule that sets `descendants=0` for `is_reparse` directories.
- [ ] Ensure exporter uses computed metrics for *all* directories (including reparse) once tree metrics are computed.
- [ ] Add post-export assertions:
  - root row matches record 5
  - `is_directory => descendants >= 1` in legacy-output parity mode
- [ ] Update parity analyzer to check trace logs for tripwires when `.log` files are tiny. fileciteturn0file3

---

## Appendix A — Evidence excerpts

### A.1 LIVE vs OFFLINE mismatch pattern (G:)
Tree metrics mismatch only in LIVE; OFFLINE matches perfectly. fileciteturn0file1

### A.2 LIVE trace shows tree metrics pipeline executed (H:)
Trace includes entering reader + compute_tree_metrics with CppPort dispatch. fileciteturn0file2

### A.3 LIVE root row prints zero metrics (H:)
`"H:\", ... Size=0 ... Descendants=0` while other directories show non-zero descendants. fileciteturn0file2
