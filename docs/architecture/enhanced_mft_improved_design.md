# Improved Design for Enhanced MFT Parsing (Rust)

> Authoritative review and redesign by an algorithmic & systems specialist  
> Scope: memory layout, extension indexing, sorting, and tree metrics  
> Status: Proposal for immediate adoption

---

## Executive Summary

The original proposal is directionally correct ("parse once, reuse everywhere"), but it leaves **major performance and memory wins on the table** and introduces a few **silent costs** that will matter at scale.

This document presents a **significantly improved design** that:

- Preserves the original goals and architecture
- Eliminates hidden padding and allocator costs
- Makes extension queries *truly* O(matches), not O(total files)
- Removes recursion and HashMap usage from hot paths
- Produces more valuable analytics for nearly zero extra cost

This is not a rewrite — it is a **surgical upgrade** to the proposal.  
Original reference design: fileciteturn0file0

---

## 1. Critical Fix: `IndexNameRef` Is Not 10 Bytes

### Problem

The proposal adds:

```rust
pub ext_dot_pos: u16
```

to `IndexNameRef` and claims the struct becomes 10 bytes.  
With `#[repr(C)]` and a `u32` field, the compiler **pads the struct to 12 bytes**, not 10.

That means:
- +4 bytes per filename reference
- Silent 40% increase instead of 25%
- Millions of wasted bytes at scale

### Solution: Pack Metadata Into a Single `u32`

Keep `IndexNameRef` **exactly 8 bytes** by packing metadata.

```rust
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IndexNameRef {
    pub offset: u32, // names buffer offset
    pub meta: u32,   // packed fields
}
```

**Bit layout:**

| Bits | Field |
|----|----|
| 0–9 | UTF-8 length (≤1023) |
| 10–15 | flags |
| 16–31 | extension ID |

Why this works:
- NTFS filenames max at 255 UTF-16 chars
- UTF-8 worst case = 1020 bytes
- 10 bits is sufficient

**Result:**  
✅ No padding  
✅ No extra memory  
✅ More expressive metadata than `ext_dot_pos`

---

## 2. Real O(1) Extension Queries Require an Index

### Problem

Storing `ext_dot_pos` only makes *extracting* the extension faster.

It **does not** make:
```text
*.txt
```
queries O(1).

You still scan **every filename**.

### Solution: Intern Extensions + CSR Posting Lists

Replace dot positions with an **extension ID**, then build a compact inverted index.

```rust
pub struct ExtIndex {
    pub offsets: Vec<u32>,  // ext_id → range
    pub postings: Vec<u32> // name_id or record_id
}
```

Query becomes:

```rust
fn files_with_ext(idx: &ExtIndex, ext_id: u16) -> &[u32] {
    &idx.postings[
        idx.offsets[ext_id as usize] as usize ..
        idx.offsets[ext_id as usize + 1] as usize
    ]
}
```

### Why This Matters

| Approach | Cost |
|------|------|
| String scan | O(N × len) |
| Dot position | O(N) |
| Posting list | **O(matches)** |

Memory cost:
- `ext_id`: 2 bytes/name (packed)
- postings: 4 bytes/name

➡ ~6 MB per 1M files for **instant extension filtering**

This is the single biggest UX improvement in the entire design.

---

## 3. ExtensionStats: Replace Linear Search with Interning

The original design uses:

```rust
Vec<(String, u32)>
```

with linear search **per file**.

Even if "small", this is unnecessary.

### Solution: Central `ExtensionTable`

```rust
pub struct ExtensionTable {
    names: Vec<Arc<str>>,
    counts: Vec<u32>,
    bytes: Vec<u64>,
    map: HashMap<Arc<str>, u16>,
}
```

During parse:
- normalize extension (lowercase, no dot)
- intern → `ext_id`
- bump counters

Benefits:
- O(1) updates
- no String churn
- byte statistics come for free
- perfect alignment with `ExtIndex`

---

## 4. Sorting: Remove Allocations From Comparators

### Problem

This comparator:

```rust
name_a.to_lowercase().cmp(&name_b.to_lowercase())
```

allocates **on every comparison**.

At scale, this is orders of magnitude slower than estimated.

### Solution A: ASCII Fast Path

You already track `is_ascii`.

```rust
fn cmp_ascii_ci(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}
```

### Solution B: Decorate–Sort–Undecorate

For large directories:
- compute a small lowercase fingerprint once
- sort on fingerprint
- tie-break on full name only if needed

Result:
- zero allocations
- stable performance
- Windows‑Explorer‑like feel

---

## 5. Tree Metrics: Kill Recursion and HashMaps

### Problem

Recursive DFS + `HashMap<u64, …>`:
- stack overflow risk
- poor cache locality
- unnecessary indirection

### Solution: Bottom‑Up Pending‑Child Reduction

Algorithm:
1. Track parent index and pending child counts
2. Push leaves into a stack
3. Bubble metrics upward
4. O(n), iterative, cache‑friendly

This is the fastest possible subtree aggregation approach.

```text
Leaves → Parents → Root
```

### Bonus: Explicit Hardlink Semantics

Define two views:
- **Path view**: count links
- **Disk view**: count unique records

Avoids double‑count bugs and user confusion.

---

## 6. Track Bytes Everywhere (Nearly Free)

If you already count things, also count **bytes**.

Add:
- per‑extension bytes
- per‑bucket bytes
- per‑attribute bytes

This enables:
- “top file types by space”
- accurate disk usage charts
- compression/encryption analysis

Cost: a few extra `u64` increments.

Value: massive.

---

## 7. Architecture Upgrade: Parse‑Time Augmenters

Instead of stuffing logic into parsing functions:

```rust
trait IndexAugmenter {
    type Local;

    fn on_record(local: &mut Self::Local, r: &ParsedRecord);
    fn merge(dst: &mut Self::Local, src: Self::Local);
    fn finalize(index: &mut MftIndex, local: Self::Local);
}
```

Benefits:
- composable features
- easy benchmarking
- clean separation of concerns
- future‑proof design

---

## Final Outcome

### What You Gain

- True O(matches) extension queries
- Zero wasted padding bytes
- Faster directory sorting
- Safer tree aggregation
- Richer analytics
- Cleaner architecture

### What It Costs

- ~6 MB / 1M files (extension index)
- ~24 MB / 1M files (tree metrics, if enabled)
- Negligible CPU overhead

### Verdict

This version keeps your philosophy intact — **pay once, benefit forever** —  
but upgrades it from *good* to **production‑grade, scale‑proof, and future‑ready**.

---

*End of document.*
