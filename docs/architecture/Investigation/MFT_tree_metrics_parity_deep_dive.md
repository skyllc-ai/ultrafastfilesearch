# NTFS / MFT Tree Metrics Parity Deep Dive  
## Reparse points, `$I30`, and the “two-channel” tree metrics model in the C++ code

This document explains **why Rust cannot currently match the C++ output 100%** for tree metrics when the dataset contains **symlinks / junctions / reparse points**, and provides a **precise fix** for the Rust `cpp_tree` port that restores **full output parity** without hacks.

---

## Table of contents

- [1. The exact symptom pattern](#1-the-exact-symptom-pattern)  
- [2. Terminology](#2-terminology)  
- [3. What the C++ code actually does](#3-what-the-c-code-actually-does)  
  - [3.1 Stream model](#31-stream-model)  
  - [3.2 Two-channel metrics](#32-two-channel-metrics)  
  - [3.3 The decisive C++ excerpt](#33-the-decisive-c-excerpt)  
- [4. Why reparse points expose the mismatch](#4-why-reparse-points-expose-the-mismatch)  
- [5. What the Rust port is doing wrong](#5-what-the-rust-port-is-doing-wrong)  
- [6. The real fix](#6-the-real-fix)  
  - [6.1 Store *printed* directory metrics from Channel B](#61-store-printed-directory-metrics-from-channel-b)  
  - [6.2 Keep propagating *full* metrics via Channel A](#62-keep-propagating-full-metrics-via-channel-a)  
  - [6.3 Undo the `$I30` suppression hack](#63-undo-the-i30-suppression-hack)  
  - [6.4 Ensure total stream count counts filtered internal streams](#64-ensure-total-stream-count-counts-filtered-internal-streams)  
- [7. Patch: concrete Rust diff](#7-patch-concrete-rust-diff)  
- [8. Validation plan](#8-validation-plan)  
- [9. After parity: next landmines](#9-after-parity-next-landmines)  
- [Appendix A: Junction walkthrough with numbers](#appendix-a-junction-walkthrough-with-numbers)  
- [Appendix B: Invariants that should always hold](#appendix-b-invariants-that-should-always-hold)  

---

## 1. The exact symptom pattern

At the “almost done” stage, the remaining mismatch pattern is:

- a directory subtree that contains **two junctions** ends up with:
  - **Descendants off by 2**
  - sometimes root **Size off by 48 bytes** as well

And locally you also observed:

- **junction record itself**: C++ prints Descendants = 1, Rust prints Descendants = 2  
- applying a hack (“don’t add `$I30` stream for reparse points”) makes the junction record print Desc=1, *but then parents are short by 2 descendants*

This is the signature of one specific class of bug:

> Rust is using the **propagation totals** as the **printed totals**, but in the C++ code those are intentionally different for directories that have **extra streams** (junctions).

---

## 2. Terminology

### Stream
In this context, a “stream” is a `StreamInfo` entry in the C++ model, constructed from multiple NTFS attribute types — not only `$DATA`.

Examples of attribute-to-stream mapping in the C++ code:

- `$DATA` (0x80)
- directory index attributes (`$INDEX_ROOT`, `$INDEX_ALLOCATION`, `$BITMAP`) when named `$I30`
- `$REPARSE_POINT` (0xC0)
- EA, object ID, property set, etc.

### Directory stream
The C++ code identifies a special “directory stream” when:

- attribute type ∈ { `$INDEX_ROOT`, `$INDEX_ALLOCATION`, `$BITMAP` }
- attribute name == `$I30`

For these, it sets:

- `type_name_id = 0`
- stream name suppressed
- and it merges multiple `$I30` index attributes into one stream

This directory stream is effectively “the default stream” for directories.

### Reparse stream
A `$REPARSE_POINT` attribute becomes a separate stream (nonzero `type_name_id`). This is present for:

- symlink files
- junction directories
- other reparse-point objects

### Two-channel metrics
This is the core concept:

- **Channel A (propagation)**: values returned by recursion and accumulated into parents  
- **Channel B (printed/default-stream)**: values stored into the record’s “default stream” and printed in output

In C++, these two channels diverge for junctions.

---

## 3. What the C++ code actually does

### 3.1 Stream model

The C++ parser creates `StreamInfo` entries for a subset of attributes. The directory index attributes are special-cased: if they are `$I30`, they become the directory stream (`type_name_id == 0`), and multiple `$I30` attributes are merged into a single stream.

A junction directory therefore typically has **two streams** in the algorithm’s model:

1) directory stream (`$I30`)  
2) reparse stream (`$REPARSE_POINT`)

### 3.2 Two-channel metrics

This is the part that is easy to miss:

- The preprocessing recursion returns a `result` that includes **all streams**
- Separately, it mutates the record’s directory stream (`type_name_id == 0`) to include children totals
- The output prints the directory stream’s values — not the returned `result`

This design means the directory’s printed metrics ignore the directory’s *own* non-directory streams, but the parent totals still include them.

### 3.3 The decisive C++ excerpt

The following excerpt is the “why” in code form. (The key lines are the unconditional `result.treesize += 1` per stream, and the conditional `k->treesize += children_size.treesize` only for the directory stream.)

```cpp
// for each stream in this record
for (auto k = me->streaminfo(fr); k; k = me->streaminfo(k->next_entry)) {

    // length_delta / allocated_delta computed (with delta() distribution)
    result.length += length_delta;
    result.allocated += allocated_delta;
    result.bulkiness += bulkiness_delta;
    result.treesize += 1;

    // only the directory stream absorbs children totals (this is what gets printed)
    if (!k->type_name_id) {
        k->length += children_size.length;
        k->allocated += children_size.allocated;
        k->bulkiness += children_size.bulkiness;
        k->treesize += children_size.treesize;
    }
}
```

Interpretation:

- `result.treesize` is the **propagation stream-count**. It counts *every stream* in the record (including `$REPARSE_POINT`). This is what the parent accumulates.
- `k->treesize` for the directory stream is the **printed descendants count**. It starts at 1 and adds `children_size.treesize`. It does **not** include the directory’s *own* non-directory streams.

So for an **empty junction**:

- propagation (returned) `result.treesize = 2` (dir + reparse)
- printed directory-stream `k->treesize = 1`

That apparent “contradiction” is exactly the C++ behavior you must reproduce.

---

## 4. Why reparse points expose the mismatch

### Symlink files
A symlink file’s payload lives in `$REPARSE_POINT`. C++ includes the reparse stream in **propagation totals** (parents get the bytes), but does not print it as the file’s own “Size” (it prints the default `$DATA` size, often 0).

If Rust filters the reparse stream and fails to add it back into propagation totals, directory sizes become too small. That was one of the earlier issues you already addressed with `internal_streams_size/allocated`.

### Junction directories
A junction directory has both:

- directory stream (`$I30`)
- reparse stream

Now the C++ design really matters:

- parents must see the junction as “2 streams” for propagation purposes
- but the junction itself must still print Descendants=1 (directory stream only)

If Rust prints the propagation treesize, it prints 2 and mismatches.

If Rust “fixes” by suppressing `$I30`, propagation becomes 1 and parents mismatch.

So junctions are the exact smallest real-world case where Channel A vs Channel B divergence is visible.

---

## 5. What the Rust port is doing wrong

In the current Rust port (`cpp_tree.rs`), directory output fields are stored using the **propagation result**:

```rust
if is_directory {
    record_mut.descendants = result.treesize;
    record_mut.treesize = children_size.length + own_total_length;
    record_mut.tree_allocated = children_size.allocated + own_total_allocated;
}
```

This merges the two channels into one.

For a normal directory with exactly one stream, that happens to work:

- result.treesize == 1 + children stream-count
- printed descendants should equal that too

But for a junction directory:

- result.treesize == 2 + children stream-count
- printed descendants should still be 1 + children stream-count

Rust currently prints 2, C++ prints 1.

The hack “skip `$I30` for reparse points” forces `total_stream_count` down by 1, making `result.treesize` match the printed value — but it necessarily breaks propagation totals for parents.

That is why the hack “fixes the junction line but breaks the parents”.

---

## 6. The real fix

### 6.1 Store *printed* directory metrics from Channel B

The values you store into the record for output **must match what C++ prints**.

For directories, that means:

- printed Descendants is **directory stream treesize**  
  `printed_desc = 1 + children_size.treesize`
- printed “tree size / tree allocated” is the **directory stream** length/alloc plus children totals  
  `printed_size = first_stream_length + children_size.length`  
  `printed_alloc = first_stream_alloc + children_size.allocated`

Critically: this does **not** include the directory’s own non-directory streams (like `$REPARSE_POINT`) in the printed values, matching C++.

### 6.2 Keep propagating *full* metrics via Channel A

Do *not* change propagation totals. Parents must still see:

- all streams (including internal/filtered ones)
- all bytes (including reparse payload lengths)

That means `result.*` must continue to include:

- delta(first_stream)  
- delta(internal_streams)  
- delta(other stored streams)  
- and treesize increments by total stream count

### 6.3 Undo the `$I30` suppression hack

Once output metrics use Channel B, you no longer need hacks to force propagation treesize to equal printed descendants.

You actually want the junction to behave as:

- propagation treesize = 2  
- printed descendants = 1

So any “skip `$I30` for reparse” hack must be reverted.

### 6.4 Ensure total stream count counts filtered internal streams

If internal streams are filtered out of the stored stream list, you must still count them for propagation treesize.

That means either:

- `record.total_stream_count` is computed *before filtering* (best), or  
- you add `internal_streams_count` and define:

```text
total_stream_count = stored_visible_stream_count + internal_streams_count
```

---

## 7. Patch: concrete Rust diff

Below is the minimal fix that restores the C++ two-channel semantics in Rust.

### 7.1 `cpp_tree.rs`: change what gets stored for directories

Replace this block:

```rust
if is_directory {
    record_mut.descendants = result.treesize;
    record_mut.treesize = children_size.length + own_total_length;
    record_mut.tree_allocated = children_size.allocated + own_total_allocated;
}
```

With this:

```rust
if is_directory {
    // C++ prints the directory stream (type_name_id == 0) after it absorbs children:
    //   printed_desc = 1 + children_size.treesize
    //   printed_size = dir_stream_len + children_size.length
    //   printed_alloc= dir_stream_alloc + children_size.allocated
    //
    // The directory's own other streams (e.g. $REPARSE_POINT) MUST still propagate
    // upward via `result`, but MUST NOT be included in the printed metrics for this
    // directory (exactly matching C++).
    record_mut.descendants = children_size.treesize + 1;
    record_mut.treesize = children_size.length + first_stream_length;
    record_mut.tree_allocated = children_size.allocated + first_stream_allocated;
} else {
    record_mut.descendants = 0;
    // (Optional) set record_mut.treesize/tree_allocated only if your output writer uses them for files.
}
```

### 7.2 Parsing/indexing: revert `$I30` suppression for reparse directories

Remove any conditional that avoids adding/counting the directory stream when the record is a reparse point.

You want junction directories to contribute both streams to parent totals.

### 7.3 (Optional but recommended) add `internal_streams_count`

If you are not 100% certain that `total_stream_count` is computed before filtering, add an explicit count:

- `internal_streams_count`
- and compute `total_stream_count = stored + internal`

This prevents “silent undercount” errors in descendants.

---

## 8. Validation plan

### 8.1 The simplest proof: show both numbers for a junction

For each junction record:

- printed Descendants must be 1
- but the returned `subresult.treesize` observed in the parent traversal must be 2

Add a one-time debug print for the junction’s preprocess return:

```rust
eprintln!(
  "junction frs={} printed_desc={} returned_treesize={} total_streams={}",
  record_frs,
  record.descendants,
  result.treesize,
  record.total_stream_count,
);
```

Expected for an empty junction:

- printed_desc = 1
- returned_treesize = 2
- total_streams = 2

### 8.2 Parent totals
After the fix, the parent directory that owns the two junction children should gain +2 descendants relative to the broken state.

### 8.3 Full parity
Run your existing parity tooling (offline and live) and expect 0 mismatches.

### 8.4 Regression check
Pick several ordinary directories with no reparse stream and confirm that:

- their printed metrics do not change
- because for a normal directory, `total_stream_count == 1` and Channel A == Channel B

---

## 9. After parity: next landmines

Once this junction fix lands, the next “real world Windows volume” parity issues to expect are:

1) **WOF compressed data** (`WofCompressedData`) special-casing  
2) **directories with unusual metadata streams** (EA/object ID/property set)  
3) **hardlink-heavy trees** and ensuring output is path-correct, not record-overwritten  

---

## Appendix A: Junction walkthrough with numbers

Assume an empty junction directory has:

- directory stream length = 48 bytes  
- reparse stream length = 24 bytes  
- no children  
- total streams = 2

### C++ propagation (Channel A)
Returned to parent:

- returned length = 48 + 24 = 72
- returned treesize = 2

### C++ printed (Channel B)
Printed for the junction record:

- Size = 48
- Descendants = 1

### Rust after the fix
Rust should:

- still return `result.length = 72` and `result.treesize = 2`
- store output fields:
  - `record.descendants = 1`
  - `record.treesize = 48`

---

## Appendix B: Invariants that should always hold

These invariants are extremely effective for debugging:

### B.1 Propagation treesize counts all streams
For any record:

```text
returned_treesize == sum(child_returned_treesize) + total_stream_count
```

### B.2 Printed directory descendants counts only directory stream + children
For any directory:

```text
printed_descendants == 1 + sum(child_returned_treesize)
```

### B.3 Therefore for directories:
```text
returned_treesize == printed_descendants + (total_stream_count - 1)
```

For a junction with 2 streams and no children:

```text
2 == 1 + (2 - 1)
```

If this identity does not hold, you do not yet match the C++ semantics.

---
