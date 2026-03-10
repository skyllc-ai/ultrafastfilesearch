# UFFS NTFS Tree-Metrics Parity: Final Remaining Gaps & Fix (2026-02-05)

This note explains **why parity is still not 100%** even after the large set of fixes you listed, and provides a **drop‑in patch** that addresses the two remaining “last‑mile” failure modes that match your current report:

- **Live scan:** some directories show **`Size=0` + `Descendants=0`** (even though C++ reports non‑zero).
- **Offline scan:** a small, stubborn set of directories show **tiny size deltas** (± a few bytes) while descendants match.

A ready-to-apply drop-in ZIP is included alongside this doc.

---

## 1) What your latest parity report is telling us

### A) Live scan: `Size=0` and `Descendants=0` on real directories

Example from your report:

- `F:\Windows\WinSxS\Temp\InFlight\...\r\`  
  C++: `Size=48` `Desc=1`  
  Rust: `Size=0` `Desc=0`

A directory with no children should **still** have descendants ≥ 1 under the C++ semantics (stream_count), and its own directory-stream footprint is typically non-zero (often you see `48`, `65592`, `65712`, etc.).

So **`0/0` is not a tree-walk bug**. It’s almost always an **index population bug**: the directory record is present, but the parser never attached its directory streams (IndexRoot/IndexAllocation/etc), so the tree code sees “no streams, no children”.

That points directly at **ParseAlgorithm mismatch** (details below).

### B) Offline scan: small size deltas (descendants equal)

Example from your report:

- `F:\Windows\Microsoft.NET\Framework\v2.0.50727\`  
  C++: `31095906`  
  Rust: `31095893` (Δ=13)  
  Descendants: equal

These tiny deltas are the signature of **hardlink remainder distribution** being assigned to a different link than C++ does. This doesn’t change descendants counts, only the propagated **bytes** (and only when `value % hardlink_count != 0`), which is why it tends to show up as a *small list* of mismatching directories.

In your codebase, the one place that can silently flip hardlink distribution globally (even if the main parser was correct) is the **LIVE self-heal** edge rebuild: `rebuild_children_from_names()`.

---

## 2) Root cause #1: ParseAlgorithm defaults to CURRENT while TreeAlgorithm defaults to CPP_PORT

### What’s happening

- `TreeAlgorithm::default()` is `CppPort` (good for parity).
- But **`ParseAlgorithm::default()` is `Current`** (lean/optimized parser).
- In `reader.rs` (inline mode), you select the parse path via `ParseAlgorithm::from_env()`.

If you **don’t explicitly set** `UFFS_PARSE_ALGO=cpp_port`, your live scan can run:

- **parse=current** (does not necessarily populate all directory streams in the exact C++ way)
- **tree=cpp_port** (expects the full C++-style directory stream model)

That configuration mismatch produces exactly what your live report shows: **directories missing their stream footprint ⇒ size/descendants can be 0**.

### Fix strategy

Make the default behavior “sane”:

- If the user explicitly sets `UFFS_PARSE_ALGO`, respect it.
- If they *don’t* set it, automatically **couple parse algo to tree algo**:
  - tree=cpp_port ⇒ parse=cpp_port
  - tree=current ⇒ parse=current

This makes parity runs work “out of the box” without relying on a fragile env-var matrix.

✅ Implemented in the drop‑in patch: `ParseAlgorithm::from_env()` now couples to `TreeAlgorithm::from_env()` when `UFFS_PARSE_ALGO` is not set.

---

## 3) Root cause #2: `rebuild_children_from_names()` assigns the wrong `name_index` semantics

### Why this matters

In the legacy port, **hardlink delta distribution** depends on `ChildInfo.name_index`.

Key detail:

- **C++ `name_index` semantics** = the **parse-order index** (FILE_NAME encounter order).
- But the record’s `first_name + next_entry` chain is stored as a **reverse-parse-order** list:
  - each newly parsed name becomes `first_name`
  - previous `first_name` is pushed into the overflow list

So the linked list order is:

```
list_index 0  == last parsed name
list_index 1  == second-last parsed name
...
```

### The bug

`rebuild_children_from_names()` currently does this:

- Walk link-chain in stored order (reverse parse order)
- Uses the walk index directly as `ChildInfo.name_index`

That means the rebuilt edges use **list-order indices**, not **parse-order indices**.

Then, in `cpp_tree.rs`, you compute:

```
name_info = (name_count - 1) - name_index
```

If `name_index` is already in reverse order, this **double reverses** and flips which hardlink gets the remainder bytes.

### The symptom it explains

That exact flip yields:

- descendants match
- directory bytes differ by a few bytes here and there (only where remainders exist)

Which matches your offline report.

### Fix

When rebuilding edges, map from list order back to parse order:

```
parse_index = (name_count - 1) - list_index
```

✅ Implemented in the drop‑in patch:
`rebuild_children_from_names()` now uses `parse_index` for `ChildInfo.name_index`.

✅ Added a regression unit test:
`test_rebuild_children_from_names_hardlinks()` now asserts the rebuilt `name_index` values are in parse order.

---

## 4) Delivered drop‑in patch

### Files included

The attached ZIP contains drop‑in replacements (same layout as your previous drop‑in zips):

- `crates/uffs-mft/src/index.rs`  ✅ **updated**
- `crates/uffs-mft/src/cpp_tree.rs` (unchanged from v4)
- `crates/uffs-mft/src/cpp_types.rs` (unchanged from v4)
- `crates/uffs-mft/src/reader.rs` (unchanged from v4)
- `crates/uffs-mft/src/cpp_io_pipeline.rs` (unchanged from v4)

### What changed in code

#### A) `ParseAlgorithm::from_env()` now couples to `TreeAlgorithm::from_env()` by default

- If `UFFS_PARSE_ALGO` is set ⇒ honor it.
- If not set ⇒ parse follows tree.

#### B) `rebuild_children_from_names()` now remaps list index → parse index

- Old: `edges.push((parent, child, list_index))`
- New: `edges.push((parent, child, name_count-1-list_index))`

#### C) Updated unit test to lock this behavior in

---

## 5) How to validate quickly

### Recommended: don’t set `UFFS_PARSE_ALGO` at all
With this patch, the default will become:

- tree default: cpp_port
- parse default (now coupled): cpp_port

So you should be able to run the same commands you used for your trial directory without extra env config.

### If you want to explicitly force:
```text
set UFFS_TREE_ALGO=cpp_port
set UFFS_PARSE_ALGO=cpp_port
```

### Expected improvements

- **Live scan:** the `Size=0 / Desc=0` directories should disappear (directory stream parsing present).
- **Offline scan:** the “± few bytes” directories should converge, because the self-heal no longer flips hardlink remainder assignment.

---

## 6) If anything still remains after this patch

If you still see a *very small* number of ±1..±N byte deltas after this, the next most likely remaining cause is:

- **non-deterministic merge ordering** of FILE_NAME attributes across extension records in a parallel parse path

…but based on the exact pattern in your report, the two fixes above are the most direct explanation and the cheapest/cleanest to resolve first.

---

## Attached artifacts

- `uffs_fix_dropin_v5.zip` – drop-in patch ZIP with the changes described above.
