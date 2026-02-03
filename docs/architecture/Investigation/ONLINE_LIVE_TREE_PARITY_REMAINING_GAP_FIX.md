# LIVE / ONLINE Tree Metrics Remaining Gap (Root + Junction descendants = 0)

This document explains why the LIVE/ONLINE scan can still show:

- `G:\` **Size = 0, Descendants = 0**
- Junction directories (reparse) **Descendants = 0** (should be 1)

even after the internal-stream linked-list fixes, and what to change to eliminate the remaining gap.

## 1) What the symptom actually means

When you see `Descendants = 0` for a **directory** (especially ROOT), it almost never means “a real empty directory”.

It means the **printed tree metric fields** on `FileRecord` were never initialized by the tree pass:

- `record.descendants`
- `record.treesize`
- `record.tree_allocated`

Those fields default to zero in the LIVE index conversion.

Offline often “looks correct” because the offline path either:
- runs the correct tree pass, or
- uses a different (correct) tree implementation / dispatch than LIVE.

## 2) The most likely remaining root-cause in the current codebase

### 2.1 Wrong tree implementation is still being dispatched

In your `index.rs`, `compute_tree_metrics_cpp_port()` dispatches to:

```rs
crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
```

If `cpp_tree` is **not** the file you patched (or still contains the pre-fix logic), then:

- LIVE runs a **different** tree algorithm than the one you updated.
- Offline may run the updated one (depending on the call path), which explains:
  - Offline tree metrics ✅
  - LIVE root/junction tree metrics ❌

**Fix:** switch the dispatch to the module/file you are actually patching (`cpp_tree.rs`).

### 2.2 Leaf directory descendants bug (junctions)

The historical broken “single-channel” cpp tree versions stored:

- `record.descendants = result.treesize`  (stream-count propagation channel)

This can produce `0` for some leaf directories (especially if their `total_stream_count` ends up `0`
after stream partitioning), even though C++ prints `1`.

**Fix:** implement the **two-channel** model:
- **Propagation channel (A)**: includes *all* streams for parent roll-up.
- **Printed channel (B)**: for directories, prints `children_stream_count + 1` and excludes record’s own internal streams.

### 2.3 LIVE occasionally leaves components unvisited

Even with correct code, LIVE parsing can occasionally yield a component that’s not reachable from ROOT
(due to linkage gaps). Those nodes never get initialized → remain at 0.

**Fix:** add an **orphan sweep** after ROOT traversal:
- iterate all records
- for any not visited by the traversal, run `preprocess(idx, 0, 1)`

This is a pure safety net and does not change results for already visited nodes.

## 3) The concrete code changes you should apply

### 3.1 Patch the tree dispatch in index.rs

Change:

```rs
crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
```

to:

```rs
crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
```

(Or, alternatively, copy your patched file into `cpp_tree.rs` if you want to keep the old module name.)

### 3.2 Replace cpp_tree.rs with the fully fixed implementation

The fixed implementation must do all of the following:

- **Two-channel printing**
  - directories print: `descendants = children_treesize + 1`
  - directories print: `treesize = children_length + first_stream_length`
  - files print: `descendants = 0`, `treesize = first_stream_length`

- **Per-stream delta distribution**
  - internal streams are iterated and delta’d **per stream**
  - overflow ADS streams are iterated and delta’d per stream

- **Orphan sweep**
  - track a `seen[idx]` bitset
  - after root traversal, call preprocess on anything not seen

I provided a drop-in `cpp_tree.rs` in this patch set implementing exactly that.

### 3.3 Optional but recommended: force cpp tree in LIVE Cpp I/O pipeline

In `io.rs` (CppPort I/O pipeline path), replace:

```rs
index.compute_tree_metrics();
```

with:

```rs
use crate::index::TreeAlgorithm;
index.compute_tree_metrics_with_algo(TreeAlgorithm::CppPort, false);
```

This removes any ambiguity about env var / CLI dispatch and ensures LIVE is always using the C++ tree when requested.

## 4) Patch files produced with this answer

- `cpp_tree_online_fixed.rs`  
  → copy to `crates/uffs-mft/src/cpp_tree.rs`

- `index_tree_dispatch_fixed.rs`  
  → copy to `crates/uffs-mft/src/index.rs` (or apply the one-line dispatch change)

## 5) Expected outcome

After these two patches, the specific remaining issues in the G: parity report should disappear:

- ROOT row `G:\` will have non-zero size/descendants again.
- Junction directories will print `Descendants = 1` (not 0).
- Offline and LIVE will be using the **same** tree code.
