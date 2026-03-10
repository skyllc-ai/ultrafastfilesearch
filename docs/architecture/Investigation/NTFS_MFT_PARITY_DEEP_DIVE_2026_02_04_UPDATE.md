# NTFS / MFT parity deep dive (Rust vs C++) — update for Drive **F** (2026‑02‑04)

This document is a follow‑up to the earlier analysis (root-path issue on small test volumes) and focuses on the remaining parity gaps you reported on **Drive F** (and the “LIVE/ONLINE” path in particular).

It’s written to be “drop‑in actionable”: it explains *what’s wrong*, *why it happens on NTFS*, and *exactly what to change* (with ready-to-copy replacement files in the accompanying zip).

---

## 1) What the parity data is telling us

From your parity run(s), the failure modes on **F:** split cleanly into two buckets:

### A. LIVE scan: a small set of directories end with **Size = 0** and **Descendants = 0**

These are directories that **exist** in the output list (paths match), but the **tree metrics never got stamped** for them.

This signature is extremely specific:

- It’s *not* a path resolver problem (paths exist and match).
- It’s *not* “missing children records” in the index globally (you’d see broad path mismatches).
- It’s the C++-port DFS traversal leaving some nodes **unvisited**, so their `treesize/tree_allocated/descendants` fields remain at their reset default (0).

On large/active volumes, LIVE scans are more likely to end up with:
- transient parent/child linkage gaps
- small cycles / broken edges from in-flight operations
- orphans (nodes not reachable from FRS 5 via child lists)

If the tree algorithm **only traverses from the root** and has **no orphan sweep**, those nodes never get visited and stay “0/0”.

### B. OFFLINE scan: descendants match, but directory sizes are off by **a few bytes**

When:
- **descendants match**
- **path match is 100%**
- but **directory `Size` differs by 1–80 bytes**

…that is a classic “hardlink delta rounding” signature.

On Windows system volumes, hardlinks are everywhere (WinSxS, System32, driver store, WindowsApps, etc.). The legacy baseline avoids double-counting hardlinked files in subtree size totals by splitting each file’s contribution across its N names using a **floor-division delta**.

If we match C++ “almost”, the only thing that tends to remain is **tiny per-directory differences** due to:
- applying delta to an aggregated sum (`delta(a+b)`), instead of per stream (`delta(a)+delta(b)`)
- using the wrong hardlink index (`name_index` vs C++’s reversed `name_info`)
- or using a “shortcut” remainder split instead of the exact floor formula

Those discrepancies are normally single-digit bytes — exactly what you’re seeing.

---

## 2) The three concrete fixes

### Fix #1 — Tree metrics: implement the exact C++ delta and **name_info mapping**, per-stream

**C++ uses:**

```
delta(v, i, n) = floor(v*(i+1)/n) - floor(v*i/n)
name_info = name_count - 1 - name_index
```

And it must be applied **per stream** (including internal streams), because delta is not linear:

```
delta(a+b) != delta(a) + delta(b)   // in general, due to floor rounding
```

If you pre-sum internal streams (or ADS) into one number before delta, you get 1–4 byte skews that propagate into directory sizes.

### Fix #2 — Tree metrics: add an **orphan sweep** after root traversal (LIVE stability)

After finishing DFS from FRS 5, walk all records:

- if a record wasn’t visited, run `preprocess()` on it as a “secondary root”

This guarantees **no directory remains with descendants=0** just because it was unreachable from root in the child graph at scan time.

This is especially important for LIVE scans, where the child graph can be transiently incomplete.

### Fix #3 — LIVE performance: remove the **O(n²)** conversion in `CppMftIndex::into_mft_index`

In the C++-pipeline live flow, the conversion from C++ packed index → Rust `MftIndex` did:

```
records_lookup.iter().position(...)
```

inside the loop over all records.

That’s **O(n²)** on real volumes (millions of records), which explains why Rust LIVE takes minutes while C++ takes seconds.

This isn’t “just performance”:

- A long scan window increases drift (files created/removed while scanning)
- Drift increases the chance of orphan/cycle edge cases
- Drift is consistent with “LIVE row count mismatch by a small number”

Precompute the inverse map once (`record_idx -> frs`) and the conversion becomes **O(n)**.

---

## 3) Drop-in file replacements provided

The accompanying zip contains drop‑in replacements for:

- `cpp_tree.rs`  
  - exact delta()  
  - correct `name_info = name_count - 1 - name_index` mapping  
  - per-stream delta distribution (internal streams and ADS)  
  - two-channel model (propagation vs printed row metrics)  
  - orphan sweep

- `index.rs`  
  - canonicalize root path to `X:\` in the path resolver  
  - avoid double separators when joining paths (robust `ends_with('\\')` join)

- `cpp_types.rs`  
  - O(n) precomputed inverse mapping for record index → FRS  
  - massive LIVE speedup

The zip also includes your other files unchanged (`reader.rs`, `cpp_io_pipeline.rs`) so you can copy the whole set into the same module folder if you prefer.

---

## 4) How to apply

1. Unzip the drop-in bundle.
2. Copy the files into your repo at the locations you currently keep them (your earlier notes suggest these live under `crates/uffs-mft/src/`).
3. Rebuild `uffs.exe`.
4. Re-run your trial harness:
   - H (LIVE + OFFLINE)
   - F (LIVE + OFFLINE)

---

## 5) What you should expect after these patches

### H:
- Root directory (`H:\`) should no longer show `Size=0, Desc=0` in LIVE output.

### F:
- OFFLINE: the “few bytes” directory size skews should drop to **0** (or near-zero) if the only remaining source was delta rounding.
- LIVE:
  - The “Size=0 / Desc=0” directory rows should disappear entirely (orphan sweep).
  - Row count mismatch should often reduce (because LIVE scan becomes much faster), though an exact match can still be defeated by real-time filesystem churn.

---

## 6) Regression tests worth adding (fast, deterministic)

### Test A — delta + name_info mapping correctness

Create a file of size 5 bytes and hardlink it into two directories:

- `A\file.bin`
- `B\file.bin`

Then verify:
- one directory gets 3 bytes and the other gets 2 bytes (C++ assigns the “extra” byte to the link with `name_info = 1` for n=2)

This catches:
- wrong delta formula
- wrong name_index/name_info mapping

### Test B — per-stream delta correctness

Create a file with:
- a default stream size not divisible by N
- plus one ADS size not divisible by N

Verify the directory totals match C++.

This catches:
- pre-summing streams before delta

### Test C — orphan sweep sanity

Inject a synthetic index with a disconnected subtree and ensure:
- every directory ends with `descendants >= 1`
- no record remains at desc=0 after tree metrics

---

## 7) Notes on NTFS specifics that matter here

- **Parent/child graph**: The parent pointer is stored in each `$FILE_NAME` attribute. Each hardlink corresponds to another `$FILE_NAME`. Building children lists must therefore add one child entry per `$FILE_NAME` (excluding DOS-only namespace if you choose).
- **Hardlinks**: A file record can have many names (WinSxS and system locations). If you sum subtree sizes naively, you double count hardlinked file sizes.
- **Directory “size”**: There’s no single universal definition. Your C++ baseline clearly uses a defined algorithm that:
  - propagates *all* stream sizes (including internal) up the tree (channel A)
  - but prints directory rows using a “channel B” that excludes the directory’s own internal streams (while still counting them in the parent totals)

Matching that behavior exactly is the key to parity.

---

## 8) Patch summary (high level)

- `cpp_tree.rs`
  - add `delta()` exact formula
  - apply `name_info = name_count - 1 - name_index`
  - apply delta per stream
  - orphan sweep after root traversal
  - stamp printed metrics with two-channel rules

- `index.rs`
  - root path canonicalization to `X:\`
  - robust join for hardlink path building

- `cpp_types.rs`
  - precompute `record_idx_to_frs` in O(n)

---

## 9) Next steps if any parity gaps remain

If you still see a tiny set of mismatches after these changes, the next most likely culprits are:

1. **Record mutation during LIVE** (scan window still too long or no snapshot)  
2. **Reparse-point handling** differences (junctions, mount points)  
3. **Namespace selection** differences for FILE_NAME (WIN32 vs WIN32+DOS)  

But based on the *shape* of the remaining diffs, the three fixes above are the right first strike.

