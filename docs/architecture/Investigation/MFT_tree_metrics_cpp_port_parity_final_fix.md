# NTFS / MFT Tree Metrics 100% Parity — Final Root Cause + Fix (Reparse Points, Junctions, Two‑Channel Model)

> **Goal**: Make the Rust `cpp_tree` (C++-port) tree-metrics implementation produce **100% output parity** with the C++ UltraFastFileSearch (UFFS) implementation for the **OFFLINE** flow (MFT parse → index build → tree metrics → report).
>
> **Status of the investigation so far**: You already fixed several “real NTFS” mismatches (notably **internal stream size propagation** and **`$SECURITY_DESCRIPTOR` stream counting**). The remaining gap shows up primarily when a directory contains **junctions / reparse points**.

This document explains:

1. **The exact remaining mismatch pattern**
2. **What the C++ code really does** (the part that is *not obvious* and causes people to go in circles)
3. **Why the current Rust `cpp_tree` port cannot hit 100% parity**
4. **The correct fix** (no hacks)
5. **How to validate** and lock it in with invariants + regression tests

---

## 0. One-page executive summary

### The reason you can’t “just make junctions match”
Because the C++ algorithm intentionally uses **two different metric channels**:

- **Channel A (propagation)**: values returned from recursion and accumulated into parents  
  → counts **all** streams of a record (including `$REPARSE_POINT`, `$SECURITY_DESCRIPTOR`, etc.)

- **Channel B (printed)**: values stored into the **directory stream** (`$I30`, identified by `type_name_id == 0`) and printed as the directory’s own **Size / Descendants**  
  → **excludes** “extra streams” on the directory (e.g., `$REPARSE_POINT`) from the directory’s printed metrics

**A junction directory is the minimal real-world case where Channel A and Channel B diverge.**

So:
- A junction should **print** `Descendants = 1` and `Size = 48` (only `$I30`)
- But it must still **contribute** **2** streams to its parent ( `$I30` + `$REPARSE_POINT` ) and contribute the `$REPARSE_POINT` bytes upward

If Rust stores Channel A into the printed fields, the junction prints `Desc=2` and mismatches.  
If Rust “fixes” that by suppressing `$I30` for reparse dirs, the junction prints right but the parents become **short by 1 per junction**.

**Correct fix**: keep propagation totals (Channel A) unchanged, but store printed directory totals from the directory stream only (Channel B).

---

## 1. The mismatch pattern you’re stuck on

After the earlier fixes, you reached a frustrating near-final state:

- The **junction rows themselves** can be made to match the C++ output (e.g., `PhotosJunction\`, `ReportsJunction\` show `Desc=1`, `Size=48`)
- But then **the parents are wrong**:
  - `G:\MFT_TEST\` descendants are **short by 2**
  - `G:\` descendants are **short by 2**
  - sometimes `G:\` size is **short by 48 bytes** (depending on the workaround applied)

This “junction lines match, parent is short by number-of-junctions” pattern is the signature of **incorrect channel mixing** (Channel A mistakenly used for printing or vice versa).

---

## 2. Key NTFS model facts (so we don’t get tricked by terminology)

### 2.1 What UFFS calls a “stream”
UFFS “streams” are not just `$DATA` / ADS. The C++ code creates `StreamInfo` entries from multiple NTFS attributes, including:

- `$DATA` (0x80)
- directory index attributes (`$INDEX_ROOT`, `$INDEX_ALLOCATION`, `$BITMAP`) **when named `$I30`**
- `$REPARSE_POINT` (0xC0)
- `$SECURITY_DESCRIPTOR` (0x50) (in practice, via “default:” fallthrough)
- EA / object ID / property set / etc.

So “stream count” in the algorithm is really “how many attribute-derived stream entries the engine models for that record”.

### 2.2 Directories have a special “directory stream” (`$I30`)
UFFS identifies the directory index as the “directory stream” like this:

- Attribute type ∈ { IndexRoot, IndexAllocation, Bitmap }
- Attribute name == `$I30`

When that condition is met, C++ sets:
- `type_name_id = 0`
- stream name suppressed
- multiple `$I30` index attributes are merged into one stream

That is the stream the C++ code treats as the directory’s “default stream” for printing.

### 2.3 Reparse points add an extra stream
A junction directory has:
- the directory stream (`$I30`)
- plus `$REPARSE_POINT`

In the UFFS stream model, those are **two streams**.

That’s why junctions expose the bug: they are directories with “extra streams”.

---

## 3. The decisive detail: the C++ algorithm has two channels

This is the core and it’s what makes parity feel “impossible” until you see it.

### 3.1 Channel A — propagation (the recursion return value)

In the C++ preprocessor loop:

- The recursion returns a `SizeInfo result` for each node.
- That returned `result` is what parents add up.
- For every stream in a record, the C++ code does:

```cpp
result.treesize += 1;  // per stream, unconditional
result.length   += length_delta;
result.allocated+= allocated_delta;
```

That means **propagation counts ALL streams**, including `$REPARSE_POINT`, `$SECURITY_DESCRIPTOR`, etc.

So a junction directory with `$I30` + `$REPARSE_POINT` contributes **2** “descendants units” to its parent, even if it prints `Desc=1`.

### 3.2 Channel B — printed/stored (the directory stream only)

Still in C++:

- Only the directory stream (`type_name_id == 0`) absorbs children totals:

```cpp
if (!k->type_name_id) {
    k->length   += children_size.length;
    k->allocated+= children_size.allocated;
    k->treesize += children_size.treesize;
}
```

Crucially:
- `k->treesize` starts at **1** for the directory stream (set during parsing via `info->treesize = isdir;`)
- and then adds `children_size.treesize`

So the directory’s **printed** Descendants are:
- `1 + children_size.treesize`

**Not**:
- `result.treesize`

And this is intentional: it prints the “directory stream” metrics, not the whole-record propagation result.

### 3.3 Why junctions print 1 but contribute 2

For an empty junction:
- `$I30` stream exists → directory stream treesize starts at 1
- `$REPARSE_POINT` stream exists → contributes to propagation totals
- children_size == 0

Therefore:
- **Printed (Channel B)** descendants = `1 + 0 = 1`
- **Propagated (Channel A)** result.treesize = `children(0) + streams(2) = 2`

That is exactly the behavior you must reproduce.

---

## 4. Why the “skip `$I30` for reparse directories” hack will never work

That hack forces junctions to have only one modeled stream, so junction row prints correctly.

But it changes Channel A too:

- Without `$I30`, the junction returns `result.treesize = 1`
- Parent totals become short by 1 per junction
- With two junctions: parent is short by 2

This is why you keep bouncing between:
- junction wrong **or**
- parent wrong

The hack destroys the one thing the parent needs: junction’s true propagation stream count.

So the right fix must:
- keep `$I30` present in the modeled streams for junctions
- keep `result.treesize` counting both streams
- but print only the directory stream’s metrics for the junction row

---

## 5. What the Rust `cpp_tree` port is doing wrong

The classic porting mistake is to collapse the two channels into one.

### 5.1 The anti-parity move
In the current Rust port, directories store output fields from the **propagation** result:

```rust
// WRONG for C++ parity in the presence of extra streams on directories
record_mut.descendants    = result.treesize;
record_mut.treesize       = children_size.length + own_total_length;
record_mut.tree_allocated = children_size.allocated + own_total_allocated;
```

This forces printed == propagated.

That happens to match most “normal” directories (single `$I30` stream), but fails for:
- junction directories
- directories with EA / object ID / security descriptor streams
- any directory with “extra” streams beyond `$I30`

---

## 6. The correct fix: implement Channel B storage, keep Channel A propagation

### 6.1 Keep propagation totals exactly as today
Propagation should continue to:
- include delta(first stream)
- include delta(internal streams sizes / allocated) (your earlier fix)
- include delta(stored ADS streams)
- and count **all streams** in `result.treesize`

This part is Channel A and must stay intact.

### 6.2 Change only what you store for directory output
For **directories**, the values that end up in the output row must be derived from:

- the **directory stream** (the `$I30` synthetic stream in your Rust model, i.e. the “first stream”)
- plus children totals

So for a directory record:

```text
printed_descendants = children_size.treesize + 1
printed_size        = children_size.length + first_stream_length
printed_allocated   = children_size.allocated + first_stream_allocated
```

**Do not include internal streams (e.g. `$REPARSE_POINT`) in printed_size / printed_descendants.**

That matches C++.

### 6.3 Undo any `$I30` suppression for reparse dirs
Once you implement proper Channel B printing, you no longer need parsing hacks.

A junction must have:
- `$I30` directory stream present
- `$REPARSE_POINT` stream present
- `total_stream_count == 2`

---

## 7. Concrete Rust patch (minimum change)

> The exact filenames/line numbers depend on your repo layout, but the change is conceptually surgical: it’s a **storage** fix, not a traversal fix.

### 7.1 `cpp_tree.rs` — store printed metrics correctly for directories

#### Before (wrong for parity on junctions)
```rust
if is_directory {
    record_mut.descendants = result.treesize;
    record_mut.treesize = children_size.length + own_total_length;
    record_mut.tree_allocated = children_size.allocated + own_total_allocated;
} else {
    record_mut.descendants = 0;
}
```

#### After (matches the C++ “two-channel” semantics)
```rust
if is_directory {
    // Channel B (printed): directory stream absorbs children totals
    // Directory stream treesize starts at 1 (the `$I30` stream itself)
    record_mut.descendants = children_size.treesize + 1;

    // Printed size/alloc for a directory: directory stream only + children
    record_mut.treesize = children_size.length + first_stream_length;
    record_mut.tree_allocated = children_size.allocated + first_stream_allocated;
} else {
    record_mut.descendants = 0;
}
```

### 7.2 (Optional but very close to C++): update the first stream’s stored treesize/length too

If your report writer uses the first stream fields for printing (or you want internal consistency), also do:

```rust
if is_directory {
    record_mut.first_stream.size.length += children_size.length;
    record_mut.first_stream.size.allocated += children_size.allocated;
    record_mut.first_stream.size.treesize += children_size.treesize;
}
```

That mirrors the C++ behavior of mutating `k->length/allocated/treesize` on the directory stream.

### 7.3 Parsing: remove the hack
If you have any logic like:

- “if directory is reparse point, skip `$I30` stream”

Delete it.

With the correct two-channel storage, the correct behavior is:

- junction prints `Desc=1`, `Size=48` (Channel B)
- but contributes `result.treesize=2` upward (Channel A)

---

## 8. Validation — how to prove you got it right in under 5 minutes

### 8.1 Add one invariant debug print (temporary)
Pick one known junction FRS (e.g., PhotosJunction) and print:

- stored/printed descendants
- returned propagation treesize

You want to see:

```text
PhotosJunction:
  printed descendants = 1
  returned (propagation) treesize = 2
  total_stream_count = 2
```

This is the single best “yes/no” test that you’ve implemented the two channels correctly.

### 8.2 Re-run the offline parity analysis
Use your existing workflow:

1. **Delete** the cached offline output (so you don’t compare stale results)
2. Regenerate Rust offline scan output for the same MFT
3. Run the parity compare script against the C++ reference

Expected outcome:
- 0 mismatches for `descendants`
- 0 mismatches for `size`
- 0 mismatches for `allocated_size`

### 8.3 Sanity checks to ensure you didn’t break normal dirs
Pick a normal directory with no reparse streams.
For such dirs:

- `total_stream_count == 1`
- Channel A == Channel B

So printed metrics should remain unchanged.

---

## 9. Regression test you should add (locks the bug permanently)

Create a synthetic test index with a single reparse directory:

- Directory record `J`
- Streams:
  - directory stream (`$I30`) length = 48, allocated = 48
  - internal stream (`$REPARSE_POINT`) length = 24, allocated = 24
- No children

**Expected behavior:**

- Printed:
  - `J.descendants == 1`
  - `J.treesize == 48`
  - `J.tree_allocated == 48`
- Propagated (what parent would see):
  - return length == 72
  - return treesize == 2

If this passes, you won’t regress on junctions again.

---

## 10. Notes on symlinks vs junctions (to avoid future confusion)

### File symlinks
C++ prints `Size=0` for file symlinks when there is no default `$DATA` stream, but still propagates `$REPARSE_POINT` bytes upward through Channel A.

Rust must do the same:
- don’t print the reparse length as the file’s own size
- but still include it in propagation totals

This is exactly why your **internal_streams_size/internal_streams_allocated** fix mattered.

### Junction directories
Junctions are directories, so they have the `$I30` directory stream.
They print the `$I30` size (often 48 for empty) but still propagate the reparse bytes and stream count upward.

---

## 11. Checklist (what to change, in order)

1. **Undo any `$I30` suppression** for reparse directories
2. Confirm junction records have:
   - `$I30` directory stream present (first stream length ~48)
   - `$REPARSE_POINT` present as an internal stream
   - `total_stream_count == 2`
3. In `cpp_tree`:
   - keep propagation `result` unchanged (Channel A)
   - store directory output using **Channel B**:
     - `desc = children_size.treesize + 1`
     - `size = children_size.length + first_stream_length`
     - `alloc = children_size.allocated + first_stream_allocated`
4. Run offline parity analysis vs C++ reference
5. Add the synthetic regression test

---

## 12. If parity is still off after this

If *anything* remains after the two-channel fix, the next likely buckets are:

- **WOF compressed data** and how C++ treats `WofCompressedData` in its bulkiness / allocated logic
- **directories with “unusual” metadata streams** (EA, object ID, property set) where printed vs propagated may diverge similarly
- **hardlink distribution** corner cases (delta arithmetic and integer division boundaries)

But junctions + parents short by N is almost always the two-channel storage bug.

---

## Appendix A — the invariants you can use as a compass

These hold in the C++ algorithm and should hold in Rust once fixed.

### A.1 Propagation treesize counts all streams (including internal)
For any record:

```text
returned_treesize == sum(child_returned_treesize) + total_stream_count
```

### A.2 Printed directory descendants count only `$I30` + children
For any directory:

```text
printed_descendants == 1 + sum(child_returned_treesize)
```

### A.3 Relationship between the two channels (directory)
For directories:

```text
returned_treesize == printed_descendants + (total_stream_count - 1)
```

For a junction with 2 streams and no children:

```text
2 == 1 + (2 - 1)
```

If that identity holds for junctions, you’re aligned with the C++ semantics.

---

## Appendix B — Why this is “correct” even though it feels weird

It feels odd that the junction prints Descendants=1 but contributes 2 to the parent.

But in UFFS terminology:
- printed descendants come from the directory stream’s `treesize`
- propagation descendants come from the recursive return value `result.treesize`

And those are intentionally different when a record has streams beyond the directory stream.

Once you accept that, everything stops being mysterious.

---

**End of document**
