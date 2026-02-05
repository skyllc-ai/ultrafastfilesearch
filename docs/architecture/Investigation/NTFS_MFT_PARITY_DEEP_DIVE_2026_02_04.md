# NTFS / MFT Parity Deep Dive (Rust Live vs Rust Offline vs C++ Baseline)
_Date: 2026-02-04_  
_Focus drive: H: (simplest repro)_

This document is written as a **developer-facing forensic + implementation report**. It explains **why the Rust LIVE run shows a bad root row (`H:\`)** even though the rest of the tree matches the C++ reference, and provides a **drop‑in fix**.

---

## 1) What is failing (observed symptom)

From `PARITY_ANALYSIS_2026_02_04.md`, the **only** mismatch on drive **H:** is the root directory row:

- **C++** and **Rust OFFLINE** agree:

  - `H:\`  
    `Size = 42168722`  
    `Size on Disk = 42524672`  
    `Descendants = 57`

- **Rust LIVE** prints:

  - `H:\`  
    `Size = 0`  
    `Size on Disk = 0`  
    `Descendants = 0`

Everything else (timestamps, flags, and all other directory rows like `temp_test\`) matches the C++ baseline.

---

## 2) Key clue: tree metrics were *actually computed correctly* during LIVE

In the LIVE trace (`rust_live_trace_h.txt`) you have this sequence:

- `compute_tree_metrics_with_algo dispatching algo=CppPort`
- `[TRIP] compute_tree_metrics_cpp_port ENTER ... records=47`
- `[TRIP] compute_tree_metrics_cpp_port ... Self-heal check: bad_dir_count=0 root_looks_bad=false`
- `[TRIP] compute_tree_metrics_cpp_port EXIT`

That **self-heal check** is important:

```rust
let root_looks_bad = self
    .frs_to_idx_opt(ROOT_FRS)
    .and_then(|root_idx| self.records.get(root_idx))
    .is_some_and(|root| {
        root.stdinfo.is_directory() && (root.descendants == 0 || root.treesize == 0)
    });
```

`root_looks_bad=false` while `root.stdinfo.is_directory()==true` can only happen if:
- `root.descendants != 0` **and**
- `root.treesize != 0`

So **inside the `MftIndex`**, right after the LIVE tree pass, the root record **already had non‑zero** descendants and tree size.

Yet the printed row is `0/0/0`.

➡️ **Therefore the bug is not in tree-metric computation.**  
It is almost certainly in **path materialization / record selection** when producing output rows.

---

## 3) The most likely root cause (with NTFS‑grounded evidence)

### 3.1 Root directory ($MFT record 5) “name” is `"."` in NTFS

On NTFS, the root directory record (#5) often has a `FILE_NAME` attribute whose name is `"."` and whose parent reference is itself (FRS=5).

I validated this directly from your `H_mft.raw` (256 records × 1024 bytes). Record 5 has:

- `flags = 0x0003` (`in_use | directory`)
- a `FILE_NAME` attribute (`type 0x30`) where:
  - `name_len = 1`
  - `name = "."`
  - `namespace = 3` (“Win32 & DOS”)

So root’s name is `"."` and your code **correctly** filters it out when materializing paths.

### 3.2 PathResolver returns `"H:"` for root, not `"H:\"`

In `index.rs`, the fast path resolver does:

```rust
path.push(self.volume.to_ascii_uppercase());
path.push(':');

for &chain_idx in chain.iter().rev() {
    let name = index.record_name(record);
    if !name.is_empty() && name != "." {
        path.push('\\');
        path.push_str(name);
    }
}
return path;
```

For the root record:
- `chain` contains only the root itself
- `record_name(root)` is `"."` → filtered out
- result becomes `"H:"`

This is a subtle but very real Windows semantic trap:

- `"H:"` is **not** an absolute path to the volume root
- `"H:"` means **“current directory on drive H”** (drive‑relative)
- `"H:\"` is the absolute root directory path

### 3.3 Why that produces exactly the observed failure pattern

The parity gap is **only** for the root row. That is exactly what you’d see if:

1. the underlying index contains the real root record (#5) with correct tree metrics, but its path key is `"H:"`
2. downstream formatting / search / matching expects `"H:\"`
3. the root record is therefore not selected / not matched, and you fall back to some synthetic “drive root” entry (or `std::fs` metadata on `"H:\"`), which naturally reports:
   - directory “size” = 0
   - “descendants” = 0

Meanwhile, every non-root path (e.g. `H:\temp_test\...`) **does** include at least one real component, so it comes out with backslashes and matches normally.

This perfectly matches:
- “tree metrics computed correctly”
- “only root row prints zeros”

---

## 4) Fix: normalize root paths to `"X:\"` in PathResolver

### Design goal

Make **all absolute paths** canonical and consistent:

- root must be `"X:\"` (absolute)
- never return `"X:"` as a fully materialized absolute path

This avoids:
- mismatched keys in maps keyed by path string
- root record missing from query outputs
- falling back to filesystem metadata (size=0)

### Implementation

Modify **two functions** in `index.rs`:

1. `PathResolver::materialize_path()`
2. `PathResolver::materialize_path_for_name()`

After building the path, if it’s just `"<drive>:"`, append a single `\`.

---

## 5) Drop‑in replacement file(s)

### 5.1 `index.rs` (fixed)

I prepared a drop‑in `index.rs` with only the minimal, targeted change.

- Fix location: inside `PathResolver`
- Behavior change: **root path becomes `X:\`**

**Download:** `uffs_fix_dropin.zip` (contains fixed `index.rs` + the other files you provided, unchanged)

---

## 6) Suggested regression tests (fast + deterministic)

Add unit tests that build a tiny in-memory `MftIndex` containing:

- record 5 (root)
  - name `"."`
  - parent_frs = 5
  - volume = `'H'`
- plus one child file with parent_frs=5 (e.g., `"foo.txt"`)

Then assert:

- `PathResolver.materialize_path(root)` returns `"H:\"` (not `"H:"`)
- `PathResolver.materialize_path(child)` returns `"H:\foo.txt"`
- if you have any path-keyed caches/maps, root lookups should succeed with `"H:\"`

This test is lightweight: it doesn’t need real MFT bytes.

---

## 7) Notes on drive F (still “not 100%”)

You mentioned drive **F** is still not perfect even OFFLINE. Without its parity report / traces in this upload, I can’t prove the exact failure mode, but based on NTFS/MFT reality, the highest-probability remaining sources are:

1. **ADS (alternate data streams)**  
   - C++ output often emits named `$DATA` streams as separate rows or separate “stream views”
   - The current C++-port tree traversal hardcodes `output_stream_idx = 0` when recursing  
     That will be wrong if you want exact parity per-stream per-hardlink.

2. **Extension records / attribute list ($ATTRIBUTE_LIST)**  
   - Large or heavily-featured files may store attributes across multiple file record segments
   - Your C++-port parser maps extensions to the base FRS (`frs_base`), which is correct, but parity issues often come from:
     - not importing every extension attribute into the correct stream bucket
     - stream merge order differences
     - missing internal-stream sizes in the delta distribution math

3. **Hardlink delta distribution edge cases**  
   - Files with many links: C++ model assigns a hardlink “delta” per link to make the total sum match
   - Any mismatch in:
     - link ordering
     - name_index assignment
     - total_names count
     will show up as small but systematic differences

If you provide the F drive parity diff (even just “top 20 mismatches” with path + expected vs actual), I can pin this down to a concrete code change in the tree traversal (usually the `output_stream_idx` / ADS handling).

---

## 8) What changed exactly (summary)

- **Root bug fix:** ensure `PathResolver` returns `X:\` for volume root, not `X:`.
- No changes to parsing, IO pipeline, or tree-metric math.

---

## Appendix A: Patch excerpt (conceptual)

Inside `PathResolver::materialize_path`:

```rust
if path.len() == 2 && path.as_bytes().last() == Some(&b':') {
    path.push('\\');
}
```

And inside `materialize_path_for_name`, when returning `parent_path` for empty/"." names:

```rust
if parent_path.len() == 2 && parent_path.as_bytes().last() == Some(&b':') {
    let mut p = parent_path;
    p.push('\\');
    return p;
}
```

---

## Downloads

- `uffs_fix_dropin.zip` — drop‑in replacement folder containing:
  - `index.rs` (fixed)
  - `reader.rs` (unchanged)
  - `cpp_tree.rs` (unchanged)
  - `cpp_types.rs` (unchanged)
  - `cpp_io_pipeline.rs` (unchanged)
