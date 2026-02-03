# Tree Metrics 100% Parity (C++ ↔ Rust) — Remaining Root Cause + Final Fix (Offline MFT)

**Scope:** Offline flow (`--mft-file ...`) only.  
**Goal:** Make Rust’s `cpp_tree` produce **bit-for-bit identical** output (tree metrics) to the C++ implementation, including tricky NTFS cases (junctions, symlinks, internal streams).

---

## 1) What’s still not matching (symptom recap)

After fixing the *extension record stream merging* issue (large files showing `size=0`), the remaining mismatches cluster around **tree metrics**:

- Root / certain directories show **descendants off by ~N reparse points / junctions**
- Some directory **Size / Allocated** values are “weird” (too small or too large)
- The patterns correlate strongly with:
  - **Reparse points (junctions/symlinks)**
  - **Internal NTFS streams** that C++ counts but Rust may filter
  - **Directories with more than one stream** (e.g., `$I30` + `$REPARSE_POINT`)

These are *not* missing records. They’re *metric accounting* differences.

---

## 2) The real issue: C++ uses a Two‑Channel Model

The C++ tree algorithm intentionally maintains **two different “channels” of values**:

### Channel A — propagation (recursion return values)
These are the numbers returned from `preprocess(child)` and accumulated into the parent:

- `result.length`, `result.allocated`: **include ALL streams**
  - default stream
  - ADS streams
  - internal streams (`$REPARSE_POINT`, `$SECURITY_DESCRIPTOR`, `$OBJECT_ID`, …)
- `result.treesize`: **counts ALL streams** (each stream contributes `+1`)

This is where junctions/reparse streams *must* count, even if they are not printed as part of the directory’s own “Size”.

### Channel B — printed (stored into the record for output)
This is what the C++ code writes into the **directory’s default stream** (`type_name_id == 0`) and what shows up in the CSV output:

For **directories**, C++ updates only the **default stream** with children aggregates:

- `printed_descendants = children_streams + 1`
- `printed_size        = children_length + default_stream_length`
- `printed_allocated   = children_allocated + default_stream_allocated`

**Crucially:** A directory’s *own* non-default streams (ADS/internal) **do NOT get folded into the directory’s printed size**. Those streams contribute to the parent via Channel A.

For **files**, printed values are just the **default stream**; ADS/internal appear as separate streams (and affect parents via Channel A).

---

## 3) Why Rust diverged: we stored Channel A as if it were Channel B

In the current Rust `cpp_tree` port (offline):

- We correctly compute Channel A propagation:
  - `result.length += delta(default_stream)`
  - `result.length += delta(internal_streams_size)`
  - `result.length += delta(ADS streams)`
  - `result.treesize += total_stream_count`

…but **we stored the wrong values back into the record** for output:

### Bug #1 — Descendants was stored from `result.treesize` (Channel A)
```rust
// WRONG for directories:
record_mut.descendants = result.treesize;
```

That makes the printed "Descendants" include the directory’s own **non-default** streams (and internal streams), which is not what C++ prints.

### Bug #2 — Directory "Size" was stored as children + *ALL own streams*
```rust
// WRONG for directories:
record_mut.treesize = children_size.length + own_total_length; // includes internal + ADS
```

But C++ prints `children + default_stream_only`.

### Bug #3 — File "Size" was treated as sum of all streams
This can distort file rows and directory aggregates depending on how the exporter maps fields.

---

## 4) The junction / reparse point intuition (why the mismatch count looks like “number of junctions”)

A junction is the canonical example where Channel A and Channel B must differ.

- A junction directory can have:
  - directory stream (`$I30`) — default stream for printing
  - internal stream (`$REPARSE_POINT`) — counted for propagation

C++ behavior (simplified):
- **Printed (Channel B):**
  - `Descendants = 1`
  - `Size = size($I30)` (often 48 bytes)
- **Propagated to parent (Channel A):**
  - `treesize += 2` (counts `$I30` + `$REPARSE_POINT`)
  - `length += size($I30) + size($REPARSE_POINT)` (delta’d)

If Rust stores Channel A values into output fields, you will observe:
- parents “off by number of junctions”
- directory rows showing descendants/size that don’t match C++

---

## 5) The fix: store printed metrics using Channel B (default stream only)

### ✅ Correct behavior to implement in Rust (drop‑in)

When storing into `FileRecord`:

#### For directories
```rust
record_mut.descendants    = children_size.treesize + 1;
record_mut.treesize       = children_size.length + first_stream_length;
record_mut.tree_allocated = children_size.allocated + first_stream_allocated;
```

#### For files
```rust
record_mut.descendants    = 0;
record_mut.treesize       = first_stream_length;
record_mut.tree_allocated = first_stream_allocated;
```

**Everything else stays Channel A** (the returned `result` still includes all streams via `total_stream_count`, internal stream sizes, ADS, etc.).

---

## 6) Why this fix also stabilizes “weird” directory sizes (e.g., iCloud Photos)

Cases like “directory row too small but descendants correct” happen when:
- we propagate correctly (Channel A),
- but print the wrong stored value (Channel B not implemented), or
- we fold in the wrong “own stream set” into the directory’s displayed size.

Once printing is *strictly* `children + default stream`, the directory’s displayed “Size” becomes consistent with C++ even when the directory has internal streams, ADS, or reparse attributes.

---

## 7) Validation plan (offline flow)

### 7.1 Regenerate Rust offline scan
```bash
cargo run --release -p uffs-cli -- "F:*" \
  --mft-file docs/trial_runs/f_disk/F_mft.bin \
  --parse-algo cpp_port --tree-algo cpp \
  --out /tmp/rust_offline_f_two_channel_fix.txt
```

### 7.2 Compare against C++ reference
```bash
cargo run --release -p uffs-diag --bin compare_scan_parity -- \
  docs/trial_runs/f_disk/cpp_f.txt \
  /tmp/rust_offline_f_two_channel_fix.txt -v
```

**Pass criteria:** `size`, `allocated_size`, `descendants` are **100.0000%** with max diff `-`.

### 7.3 Targeted “junction sanity” checks
Pick a known junction (or any `reparse=1` directory that is a junction):

- Junction row should show:
  - `Descendants == 1`
  - `Size == size($I30)` (commonly `48`)
- Parent directory descendants should increase by:
  - `+2` for that junction record (default `$I30` + internal `$REPARSE_POINT`)

---

## 8) Notes / guardrails (so we don’t regress again)

1. **Never store Channel A into printed fields.**  
   Channel A exists to make parent totals correct; Channel B exists to match output.

2. **Directory printed size must not include the directory’s own internal streams.**  
   Internal streams contribute to parent totals, not the directory’s printed “Size”.

3. **Keep using `total_stream_count` for propagation treesize.**  
   This ensures internal streams count even if they are filtered from storage.

4. **Keep adding `internal_streams_size` and `internal_streams_allocated` via `delta()`**  
   Otherwise parents will be undercounted for reparse points, security descriptors, etc.

---

## 9) Deliverables

- **Drop‑in Rust file:** `cpp_tree_fixed.rs` (provided separately)  
- This markdown explains:
  - the precise C++ semantics,
  - why Rust diverged,
  - the minimal fix,
  - and how to validate with offline parity tooling.


---

## Appendix A — Exact C++ behavior (annotated)

The C++ implementation conceptually does this per record:

### A.1 Compute `children_size` (post-order)
```cpp
children_size = 0
for (child in record.children):
    // IMPORTANT: child_name_info depends on which hardlink entry this child is
    child_name_info = child.name_count - 1 - child_info.name_index
    sub = preprocess(child, child_name_info, child.name_count)
    children_size += sub   // add length/allocated/treesize
```

### A.2 Add this record’s own streams into the propagated result (Channel A)
For each stream `k` belonging to this record:
```cpp
result.length    += delta(k.length,    name_info, total_names);
result.allocated += delta(k.allocated, name_info, total_names);
result.treesize  += 1;
```

That includes:
- default stream (file `$DATA` or directory `$I30`)
- ADS `$DATA:streamname`
- internal attribute streams (`$REPARSE_POINT`, `$SECURITY_DESCRIPTOR`, …)

### A.3 Store printed metrics into the *default* stream only (Channel B)
C++ then “folds” child totals into the *default* stream only (type_name_id == 0):
```cpp
default_stream.length    += children_size.length;
default_stream.allocated += children_size.allocated;
default_stream.treesize  += children_size.treesize;
```

These default-stream fields are what you see as:
- **Size** (default_stream.length)
- **Size on Disk** (default_stream.allocated)
- **Descendants** (default_stream.treesize)

**Key consequence:** a directory’s *own* non-default streams do not inflate the directory’s printed size/descendants. They only inflate **the parent’s totals** via propagation.

---

## Appendix B — Rust field mapping (what must mean what)

Your Rust `MftIndex`/`FileRecord` model has three relevant “counts/sizes”:

- `record.first_stream.size.{length,allocated}`  
  → C++ default stream size fields (for *printing*)
- `record.total_stream_count`  
  → number of streams **including internal streams** (for propagation `result.treesize += stream_count`)
- `record.internal_streams_{size,allocated}`  
  → bytes for streams filtered from storage, but required for propagation via `delta()`

And the three **output-facing** fields you are writing in `cpp_tree`:

- `record.descendants` should match **default_stream.treesize** (Channel B)
- `record.treesize` should match **default_stream.length** (Channel B)
- `record.tree_allocated` should match **default_stream.allocated** (Channel B)

The bug was treating `record.descendants` as “total streams in subtree” (Channel A), and treating `record.treesize/tree_allocated` as “children + all own streams”.

---

## Appendix C — Concrete worked examples

### C.1 Junction (directory reparse point)

Assume junction has:
- default directory stream `$I30` = 48 bytes
- internal stream `$REPARSE_POINT` = 24 bytes
- no children

**Channel A (propagation):**
- `result.treesize = 2`
- `result.length = 48 + 24`

**Channel B (printed):**
- `Descendants = children_streams + 1 = 0 + 1 = 1`
- `Size = children_length + $I30 = 0 + 48 = 48`

This is the “it prints 1 but contributes 2” behavior that makes parent totals correct.

### C.2 Directory with internal streams (e.g., cloud sync roots)

Assume directory has:
- default directory stream `$I30` = 32 MiB
- internal streams (e.g. `$SECURITY_DESCRIPTOR`) totaling 1 KiB
- children totaling 649 GiB (including their ADS/internal via propagation)

**Channel A (propagation):**
- contributes children + 32 MiB + 1 KiB + (any other streams)

**Channel B (printed directory row):**
- prints **children + 32 MiB**
- does **not** print `+ 1 KiB` (its own internal streams)

If Rust prints children + (32 MiB + 1 KiB), you’ll see tiny diffs (bytes/KB).  
If Rust prints only 32 MiB, you’ll see “directory too small” diffs that can be enormous.

---

## Appendix D — Drop-in patch summary (what changed in code)

In `cpp_tree.rs`:

1. **Removed** “own_total_length/allocated” (sum of all own streams) from the *printed* directory size.
2. **Stored** directory printed metrics using Channel B:
   - descendants = `children_size.treesize + 1`
   - size = `children_size.length + first_stream_length`
   - allocated = `children_size.allocated + first_stream_allocated`
3. **Stored** file printed metrics as default stream only:
   - size = `first_stream_length`
   - allocated = `first_stream_allocated`
4. **Kept** propagation logic intact:
   - continue adding internal stream sizes via `delta()`
   - continue adding ADS deltas via `delta()`
   - continue adding `stream_count` to propagated `result.treesize`

---

## Appendix E — Minimal regression tests worth adding

Even without a full NTFS fixture, you can add deterministic unit tests with a tiny synthetic `MftIndex`:

1. **Single directory with two files** (no ADS)  
   Verify directory printed size = sum of file sizes; printed descendants = 1 + (file stream counts).

2. **File with ADS**  
   Verify parent directory size includes ADS bytes, while file row prints only default stream size.

3. **Junction-like directory**  
   Directory prints descendants=1, but propagated `result.treesize` adds `stream_count=2`.

4. **Directory with internal streams**  
   Ensure directory’s printed size does not include its own internal streams, but parent size does.

These tests will catch exactly the class of regressions that caused the last ~N mismatch patterns.

