# UFFS Parity Fix Plan: Tree Metrics Mismatches (Live + Offline)

**Date:** 2026-02-05  
**Scope:** Fix tree metrics mismatches between legacy baseline output and Rust output (live + offline).  
**Focus symptoms:** Live-only large size deltas (e.g., `65712 → 184`, `48 → 8` with same descendants), plus offline tiny ±1–±8 byte drift with identical descendants.

> Sources used for the baseline symptoms and constraints:
> - f_disk parity report (row-count drift + 20 live issues + 20 offline issues) fileciteturn4file1  
> - g_disk parity report (historical “root=0” live bug, fixed tripwire discussion) fileciteturn4file0  
> - h_disk parity report + live trace that shows the `[TRIP]` tripwire lines are present in the runtime logs fileciteturn4file5 fileciteturn4file8  

---

## 0) What is broken (symptoms you’re seeing)

### 0.1 Live scan mismatches: “big size deltas, same descendant counts”
From your live mismatch list (also present in the f_disk parity report) fileciteturn4file1:

- Many directories show sizes like:
  - `48 → 8` (desc `1 → 1`)
  - `65712 → 184` (desc `5 → 5`)
  - `196800 → 131272` (desc `5 → 5`)
- Descendant counts are often identical, strongly suggesting:
  - The **tree shape is correct** (same children count / stream-count propagation),
  - But we’re losing (or mis-attributing) part of the **directory’s own “default stream” size**, which in NTFS is usually the `$I30` index allocation (cluster-multiple sizes), *or* we’re swapping which stream becomes the “primary stream” for that record.

This is consistent with your hypothesis: “missing `$INDEX_ALLOCATION` streams or similar internal stream issues”.

### 0.2 Offline scan mismatches: “tiny ±1..±8, identical descendant counts”
From your offline mismatch list (also present in the f_disk parity report) fileciteturn4file11:

- Size deltas are tiny, typically ±1..±8 bytes.
- Descendants are identical.
- This screams **hardlink remainder distribution**: the total byte count is being partitioned across multiple parent directories slightly differently than C++ (same total, but remainder bytes land in different siblings/parents).

### 0.3 Row count drift (live only on f_disk)
The f_disk parity report shows:

- C++ rows: `2,221,315`
- Rust live rows: `2,221,334` (**+19**)
- Path match is still **100%** fileciteturn4file1

This usually implies **duplicate rows** (same path repeated), or extra rows that the analyzer collapses as “matching path set” due to normalization. This is often caused by:
- A record being parsed more than once (duplicate FILE_NAME insertion),
- Or duplicate stream enumeration lines being emitted for the same file/stream.

---

## 1) The most likely root cause behind the *live* size deltas

### 1.1 Why “out-of-order live parsing” is uniquely dangerous in your current Rust port
Your Rust “legacy port” parsing path is **stateful and order-dependent**:

- `parse_stream(...)` uses “merge if same stream, else push previous first_stream into overflow and make the new one the first_stream”.
- That is effectively “**last stream wins**” for `record.first_stream`.

For directories, the default stream that C++ expects to be “primary” is effectively the directory-index stream (`$I30`, represented as a special `type_name_id=0/name_len=0` in your port). If that stream is parsed and then later a different stream is parsed (reparse, EA, etc.), the later stream can become `first_stream` and the `$I30` bytes get pushed into overflow.

Then, when you compute/print directory “Size”, you’ll end up using the wrong “primary stream” length and the directory appears to shrink by **exactly one or more cluster-sized blocks** (64K on your volume, which matches the `65536 → 184` style symptom).

### 1.2 Why live can reorder when offline doesn’t
Offline scan reads a contiguous MFT file; it is naturally parsed in sequential buffer order.

Live scan uses `CppIoPipeline` (sliding window IOCP). In the current implementation, **completed reads are processed immediately in completion order**. If IO finishes out-of-order, the parser will ingest chunks out-of-order.

That makes:
- Extension records,
- Later record ranges,
- Or later attributes

… potentially land into the index **before** earlier ones, which flips the “last stream wins” outcome.

---

## 2) Fix #1 (highest priority): Make IOCP parsing order deterministic

### 2.1 Goal
Guarantee that `pipeline.process_chunk(...)` is called in **ascending logical MFT virtual offset** order, even if IO completions arrive out-of-order.

This turns the live scan into “offline-like ordering”, while still keeping overlapped reads for throughput.

### 2.2 Where to change
File: `cpp_io_pipeline.rs`  
Function: `CppIoPipeline::run(...)`  

The current logic processes completions immediately and feeds the buffer directly into `pipeline.process_chunk(...)`.

### 2.3 Implementation strategy: “sequence numbers + bounded reorder buffer”
1. When building `io_ops`, assign each op a `seq` in push order (which should reflect increasing `virtual_offset`).
2. Track:
   - `next_issue` (the next seq to submit)
   - `next_process` (the next seq to parse)
3. When an IO completes:
   - Store the returned buffer in `completed[seq]`.
   - Process buffers in order: while `completed[next_process].is_some()`, parse and increment.
4. **Bound memory** by not issuing reads too far ahead:
   - Only allow `next_issue < next_process + concurrency`.

This ensures you buffer at most `concurrency-1` completed chunks waiting for the earliest chunk.

### 2.4 Concrete patch (illustrative diff)

> This is written as a “guide diff” (you may need minor adjustments for exact module paths / imports).

```diff
diff --git a/crates/uffs-mft/src/cpp_io_pipeline.rs b/crates/uffs-mft/src/cpp_io_pipeline.rs
--- a/crates/uffs-mft/src/cpp_io_pipeline.rs
+++ b/crates/uffs-mft/src/cpp_io_pipeline.rs
@@
-        // Generate I/O operations for each chunk, including skip ranges at start/end
-        let mut io_ops = VecDeque::new();
+        // Generate I/O operations for each chunk, including skip ranges at start/end
+        // IMPORTANT: In live IOCP mode, completions can arrive out-of-order.
+        // To keep parsing deterministic (and match offline / C++), we assign a
+        // sequence number and process buffers strictly in seq order.
+        let mut io_ops: Vec<IoOp> = Vec::new();

@@
-                    io_ops.push_back(IoOp {
+                    let seq = io_ops.len();
+                    io_ops.push(IoOp {
+                        seq,
                         chunk_idx,
                         disk_offset: data_chunk.disk_offset + offset_in_chunk,
                         virtual_offset: data_chunk.virtual_offset + offset_in_chunk,
                         size_bytes: io_size,
                         skip_begin,
                         skip_end,
                     });
                 }
             }
         }

+        let total_ops = io_ops.len();
+        let mut completed: Vec<Option<Vec<u8>>> = vec![None; total_ops];
+        let mut next_issue: usize = 0;
+        let mut next_process: usize = 0;

         struct InFlightOp {
             overlapped: windows_sys::Win32::System::IO::OVERLAPPED,
             buffer: Vec<u8>,
+            seq: Option<usize>,
         }

@@
-        // Issue initial operations up to concurrency
-        for slot_idx in 0..concurrency {
-            if let Some(io_op) = io_ops.pop_front() {
+        // Issue initial operations up to concurrency
+        for slot_idx in 0..concurrency {
+            if next_issue < total_ops {
+                let io_op = &io_ops[next_issue];
                 let mut in_flight_op = Box::pin(InFlightOp {
                     overlapped: unsafe { std::mem::zeroed() },
                     buffer: vec![0u8; io_size as usize],
+                    seq: Some(next_issue),
                 });
-                issue_read(&drive_handle, in_flight_op.as_mut(), &io_op, io_size)?;
+                issue_read(&drive_handle, in_flight_op.as_mut(), io_op, io_size)?;
                 in_flight[slot_idx] = Some(in_flight_op);
-                outstanding += 1;
+                next_issue += 1;
             }
         }

-        while outstanding > 0 {
+        while next_process < total_ops {
             // Wait for next completion
             let mut bytes_transferred: u32 = 0;
             let mut completion_key: usize = 0;
             let mut overlapped_ptr: *mut windows_sys::Win32::System::IO::OVERLAPPED = std::ptr::null_mut();

@@
             // Find which slot completed
             let completed_overlapped = overlapped_ptr;
             let slot_idx = in_flight
                 .iter()
                 .position(|slot| {
                     slot.as_ref().map_or(false, |op| {
                         let op_ptr = &op.overlapped as *const _ as *mut _;
                         op_ptr == completed_overlapped
                     })
                 })
                 .ok_or_else(|| anyhow::anyhow!("Completion for unknown OVERLAPPED"))?;

-            // Recover virtual_offset from OVERLAPPED offsets (old approach)
-            // let virtual_offset = ...
-
             let in_flight_op = in_flight[slot_idx].as_mut().unwrap();
+            let seq = in_flight_op.seq.expect("completed slot must have a seq");

             // Complete and get buffer
             let buffer = complete_read(in_flight_op.as_mut(), bytes_transferred as usize)?;
-            let buffer_slice = &buffer[(io_op.skip_begin as usize)..(buffer.len() - io_op.skip_end as usize)];
-            pipeline.process_chunk(buffer_slice, virtual_offset);
-            outstanding -= 1;

+            completed[seq] = Some(buffer);
+            in_flight_op.seq = None; // slot now idle

+            // Process in-order as far as possible
+            while next_process < total_ops {
+                let Some(buf) = completed[next_process].take() else { break; };
+                let io_op = &io_ops[next_process];
+
+                // Safety: skip ranges should never create misaligned record boundaries.
+                // (Optional) debug assert:
+                // debug_assert_eq!((io_op.virtual_offset + io_op.skip_begin as u64) % BYTES_PER_RECORD as u64, 0);
+
+                let start = io_op.skip_begin as usize;
+                let end = buf.len().saturating_sub(io_op.skip_end as usize);
+                let slice = &buf[start..end];
+                pipeline.process_chunk(slice, io_op.virtual_offset);
+                next_process += 1;
+            }

-            // Issue next op immediately (old)
-            if let Some(next_op) = io_ops.pop_front() { ... issue_read ... } else { ... }
+            // Refill idle slots, but do not run too far ahead of next_process
+            for slot in in_flight.iter_mut() {
+                if next_issue >= total_ops { break; }
+                if next_issue >= next_process + concurrency { break; } // bound reorder buffer
+
+                let op = slot.get_or_insert_with(|| {
+                    Box::pin(InFlightOp {
+                        overlapped: unsafe { std::mem::zeroed() },
+                        buffer: vec![0u8; io_size as usize],
+                        seq: None,
+                    })
+                });
+
+                if op.seq.is_some() { continue; } // still in-flight
+
+                let io_op = &io_ops[next_issue];
+                op.seq = Some(next_issue);
+                issue_read(&drive_handle, op.as_mut(), io_op, io_size)?;
+                next_issue += 1;
+            }
         }
```

### 2.5 Why this fix should eliminate the “48→8 / 65712→184” style mismatches
Because it prevents the stateful “last stream wins” update path from being driven by non-deterministic completion timing.

Specifically, it stops:
- Extension record data from being applied before base record data,
- Later record ranges from being applied before earlier ones,
- And “secondary stream” parsing from overriding “primary stream” selection due to reorder.

This is the single highest-leverage fix for live-only discrepancies.

---

## 3) Fix #2 (strongly recommended): Stabilize “primary stream selection” inside `parse_stream`

Fix #1 ensures *ordering*, but it’s still worth making the core stream update logic resilient. Even with sequential order, you can still get:
- Attribute ordering differences across Windows builds,
- Or weird records where a non-default stream appears last.

### 3.1 Goal
Make sure these invariants hold, regardless of parse order:

- **Directories:** the directory-index stream (your “dir index” collapsed key) must end up as `first_stream`.
- **Files:** unnamed `$DATA` must end up as `first_stream` (not an ADS, not an internal stream, not a reparse/EA stream).

### 3.2 Where to change
File: `cpp_types.rs`  
Function: `parse_stream(...)`

### 3.3 Implementation approach
After you compute:
- `type_name_id`
- `stream_name_length`
- `is_dir_index`

Add a “default stream” classifier:

```rust
fn is_default_stream(is_directory: bool, type_name_id: u32, name_len: u16) -> bool {
    if is_directory {
        // Cpp-port collapses directory index streams to this key:
        return type_name_id == 0 && name_len == 0;
    }

    // For files, detect unnamed $DATA:
    // type_name_id packs (attr_type << 16) | name_id
    // If name_len == 0 => name_id should be 0.
    const ATTR_DATA: u32 = 0x80;
    (type_name_id >> 16) == ATTR_DATA && name_len == 0
}
```

Then, when you hit “new stream that doesn't merge” logic:

- If `new_is_default && !first_is_default` → swap: push current `first_stream` to overflow, make this new default the `first_stream`.
- If `!new_is_default && first_is_default` → do **not** evict first; instead push the new stream into overflow.
- Else → keep your existing “last wins” logic.

### 3.4 Concrete patch sketch
```diff
diff --git a/crates/uffs-mft/src/cpp_types.rs b/crates/uffs-mft/src/cpp_types.rs
@@ fn parse_stream(...)
-    // Existing logic:
-    // - Merge if same stream
-    // - Otherwise push current first_stream into overflow and make this new stream the first_stream
+    // New: Default-stream stability. Prevent non-default streams from evicting the default stream.
+    let record_is_dir = record.stdinfo.flags2 & FILE_ATTRIBUTE_DIRECTORY != 0;
+    let new_is_default = is_default_stream(record_is_dir, type_name_id, stream_name_length);
+    let first_is_default = is_default_stream(
+        record_is_dir,
+        record.first_stream.type_name_id(),
+        record.first_stream.stream_name_length(),
+    );

     if current_stream_count > 0 {
         // ...existing merge checks...

-        // Existing eviction behavior (last stream wins)
-        let link_idx = u16::from(index.records_data[record_idx].stream_count) - 1;
-        index.records_data[record_idx].streaminfos.push(record.first_stream);
-        record.first_stream.next_entry = link_idx;
-        record.first_stream = stream;
+        if new_is_default && !first_is_default {
+            // Promote default stream to primary
+            let link_idx = u16::from(index.records_data[record_idx].stream_count) - 1;
+            index.records_data[record_idx].streaminfos.push(record.first_stream);
+            stream.next_entry = link_idx;
+            record.first_stream = stream;
+        } else if !new_is_default && first_is_default {
+            // Keep default stream as primary; stash the new stream in overflow
+            index.records_data[record_idx].streaminfos.push(stream);
+        } else {
+            // Preserve previous behavior for two non-default streams or two defaults
+            let link_idx = u16::from(index.records_data[record_idx].stream_count) - 1;
+            index.records_data[record_idx].streaminfos.push(record.first_stream);
+            stream.next_entry = link_idx;
+            record.first_stream = stream;
+        }
     } else {
         record.first_stream = stream;
     }
```

### 3.5 Why this helps with the live mismatches
Even if some chunk-order or record-order weirdness remains, this prevents “wrong stream becomes primary” from collapsing directory sizes down to small resident fragments like 8 bytes or ~184 bytes.

It also protects against file rows accidentally using an ADS as the primary stream.

---

## 4) Fix #3: Remove offline ±1..±8 drift by enforcing hardlink invariants + canonical child rebuild

This is about the *offline* mismatches that survive even when live I/O is eliminated fileciteturn4file11.

### 4.1 Why tiny drift happens
Hardlink-aware tree sizing usually:
1. Computes “true file size” once per record,
2. Distributes it across the record’s hardlinks so that directory totals don’t double-count.

A standard pattern is:
- `delta(i) = ceil(size * (i+1) / N) - ceil(size * i / N)`  
where `i` is “which hardlink this is” among `N`.

If Rust and C++ disagree by even a tiny bit about:
- which hardlink gets which `i`, or
- what `N` is,

…then **a few remainder bytes** land in a different parent directory, producing ±1..±8 differences at directory boundaries.

### 4.2 What to enforce (invariants)
For every record `R` that appears in the directory graph:

1. `R.name_count` must equal the actual number of non-DOS names we will treat as hardlinks.
2. Every `ChildInfo` referencing `R` must have `name_index < R.name_count`.
3. Directory child lists should be rebuilt from the name graph to avoid any “stale” edges introduced by parsing order quirks.

### 4.3 Practical fix: always rebuild children from names in CppPort mode (or behind a flag)
You already *can* rebuild children from names (`rebuild_children_from_names()`), but currently it only runs as a self-heal when “bad dirs” are detected.

For parity, it’s cheap insurance to make it deterministic:

#### Option A (recommended for parity builds):
Call rebuild unconditionally in `compute_tree_metrics_cpp_port` **before** first pass.

```diff
diff --git a/crates/uffs-mft/src/index.rs b/crates/uffs-mft/src/index.rs
@@ pub fn compute_tree_metrics_cpp_port(&mut self, debug: bool) {
-    tracing::debug!("[TRIP] ... ENTER (first pass)");
+    tracing::debug!("[TRIP] ... ENTER (first pass)");
+
+    // Parity-first: rebuild directory children from name graph unconditionally.
+    // This removes any parse-order artifacts from live mode and stabilizes name_index mapping.
+    //
+    // Gate this behind env/feature if you want to keep current fast-path.
+    if std::env::var_os("UFFS_REBUILD_CHILDREN_ALWAYS").is_some() {
+        tracing::debug!("[TRIP] compute_tree_metrics_cpp_port -> rebuilding children from names (forced)");
+        self.rebuild_children_from_names();
+    }

     crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
```

#### Option B:
Run rebuild only when doing parity validation runs (CI / trial harness), not in normal production.

### 4.4 Tighten `compute_name_info` to expose bugs instead of hiding them
In `cpp_tree.rs`, `compute_name_info(...)` currently clamps `name_index` to `name_count-1`.

That is safe, but it also:
- **hides** out-of-range indices,
- and can cause two hardlinks to map to the same `i`, which can skew totals.

For parity, I recommend:

- Keep clamping in release, but
- In debug/parity mode, log when this happens and include record FRS + name_count + name_index.

Pseudo-change:

```rust
fn compute_name_info(name_index: u16, name_count: u16, frs: u32, debug: bool) -> u64 {
    if name_count == 0 { return 0; }
    if name_index >= name_count {
        if debug {
            tracing::warn!(
                "[TRIP] name_index out of range; clamping (parity risk) frs={} name_index={} name_count={}",
                frs, name_index, name_count
            );
        }
        return 0; // or clamp; but *log*
    }
    (name_count as u64 - 1) - (name_index as u64)
}
```

This will tell you if the offline drift is actually coming from an invariant violation.

---

## 5) Fix #4: Prevent “double-parse adds sizes twice” (addresses live row drift + root inflation scenarios)

### 5.1 Why this matters
Your stream merge logic adds sizes when a stream repeats:

- If the same record or extension record is parsed twice (due to IO overlap, retry, or ordering bug),
  the size can be added again,
  while stream_count remains stable.

That can inflate directory totals (including root) without obvious stream-count/descendant changes.

The f_disk parity report shows live row-count drift (+19) while path set still matches fileciteturn4file1, which is consistent with “a small number of duplicates”.

### 5.2 Add a “record parsed once” guard inside the parse pipeline
File: `cpp_types.rs`  
Area: where the “cpp port parse pipeline” iterates records inside a buffer and calls `parse_record_header/attrs`.

Add to parse state:

```rust
struct CppParsePipelineState {
    ...
    parsed_record_seen: Vec<bool>, // length = max_records
}
```

Then in record loop:

```rust
let frs = record_number as usize;
if state.parsed_record_seen[frs] {
    if debug {
        tracing::warn!("[TRIP] duplicate parse of record frs={} (skipping)", frs);
    }
    continue;
}
state.parsed_record_seen[frs] = true;
```

Important:
- This should only be used when you are sure each record must be presented exactly once.
- If you have a legitimate multi-pass parse design, scope it to the pass or reset it between passes.

### 5.3 Add alignment assertions for skip slicing
Still in `CppIoPipeline::run`:

Before calling `process_chunk`, assert:

- `io_op.virtual_offset % bytes_per_record == 0`
- `skip_begin` and `skip_end` are multiples of bytes_per_record
- `(virtual_offset + skip_begin) % bytes_per_record == 0`

If any are violated, it means you are slicing the buffer such that record boundaries are broken — a guaranteed source of weirdness.

---

## 6) Fix #5: Make the tripwire unmissable (so the parity harness never says “tripwire not found” incorrectly)

The parity reports currently claim “cpp_tree tripwire NOT FOUND” fileciteturn4file1, but the runtime trace log clearly contains the `[TRIP]` lines fileciteturn4file8.

This likely means the analyzer is searching the wrong log file (e.g., `*_mft_save*.log` vs the `rust_live_trace_*.txt`).

### Recommendation
Put a tripwire string into a file that the parity harness **always** reads, such as the rust output itself:

- Prepend a comment line at the top of `rust_live_*.txt` and `rust_offline_*.txt` like:

```
# TRIPWIRE: UFFS cpp_tree FIXED v0.2.195 commit=<hash>
```

This avoids any dependence on trace logs.

---

## 7) Expected outcomes by fix

| Fix | Primary target | Expected improvement |
|---|---|---|
| Fix #1 IOCP ordered processing | Live-only big deltas (`48→8`, `65712→184`) | Should eliminate most or all “missing $I30/index alloc” size deltas |
| Fix #2 Default stream stability | Live big deltas + safety net | Prevent wrong primary stream even if ordering weirdness remains |
| Fix #3 Rebuild children + name_index invariants | Offline ±1..±8 drift | Should remove remainder drift if caused by index mapping differences |
| Fix #4 Duplicate parse guard | Live row-count + size inflation edge cases | Eliminates “parse twice adds sizes twice” |
| Fix #5 Tripwire in output | Harness diagnostics | Stops false “tripwire not found” reports |

---

## 8) Verification checklist (fast, concrete)

### 8.1 Instrumentation you should see (after fixes)
- New debug counters:
  - `out_of_order_completion_count`
  - `max_completed_buffer_depth`
- Optional warnings:
  - `name_index out of range; clamping`
  - `duplicate parse of record frs=... (skipping)`

### 8.2 Parity rerun expectations for f_disk
After Fix #1 + Fix #2, the f_disk “Live Scan Mismatches” table should collapse dramatically (the large deltas should disappear) fileciteturn4file1.

After Fix #3, the f_disk “Offline Scan Mismatches” should also go to zero (if drift is purely mapping-related) fileciteturn4file11.

If offline still shows tiny drift:
- Turn on the `name_index out of range` warnings,
- Identify which record(s) contribute to those directories,
- Confirm that record.name_count equals actual number of non-DOS names.

---

## 9) Notes on risk / performance

- Fix #1 can reduce peak throughput on wildly out-of-order IO because it intentionally bounds “run ahead”. In practice, for near-sequential reads of MFT extents, completion order should be close to submission order, so overhead is minimal.
- Fix #2 slightly changes semantics if the previous “last stream wins” behavior was being relied on (unlikely for correct filesystem sizing).
- Fix #3 unconditional rebuild is O(total_records × avg_name_count). On a 2.2M record MFT, this is still usually acceptable in Rust (avg_name_count is small), but gate it behind `UFFS_REBUILD_CHILDREN_ALWAYS` if needed.
- Fix #4 duplicate parse guard is extremely cheap (bitset lookup), and can be gated behind a debug feature if you’re worried about edge cases.

---

## Appendix: How the f_disk parity report frames the issues

- Live scan: row drift + 20 tree metric issues, ADS + timestamps OK fileciteturn4file1  
- Offline scan: 20 tree metric issues, ADS + timestamps OK fileciteturn4file11  
- The specific “WinSxS Temp InFlight … 65712→184” patterns appear directly in the report table fileciteturn4file7  

