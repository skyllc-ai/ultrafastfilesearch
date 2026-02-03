# UFFS Tree Metrics: Why we’re still not at 100% (and the fix)

**Date:** 2026-02-03

This document explains why the current Rust port still fails to match the C++ tree-metrics output 100%, even after the two‑channel `cpp_tree.rs` drop-in, and provides a concrete Rust patch that closes the remaining gap (targeting the **offline** MFT flow first, but it also helps live scans).

---

## What the latest parity report is telling us

From your latest report:

- **Offline scan**
  - Row count ✅
  - Path match ✅
  - ADS count ✅
  - **Tree metrics**: **~20 directories** differ, but **descendants match** and size deltas are **tiny (±1..±12 bytes)**.

- **Live scan**
  - Row count ⚠️ (+19 rows)
  - Path match ✅
  - ADS count ✅
  - **Tree metrics**: 20 directories where Rust has **Size=0 / Desc=0** (uninitialized / never visited), while C++ has small non‑zero values.

Key observation:

> In offline mode, the traversal is correct (descendants match), and the remaining size mismatches are **very small**.

That pattern is a classic fingerprint of **hardlink delta-distribution rounding** being “almost right” but not *bit‑for‑bit identical* to the C++ logic.

---

## Root cause

### 1) C++ distributes size across hardlinks *per stream*

The C++ algorithm doesn’t just sum a record’s stream lengths and split once. It iterates **each stream individually** and applies the `Accumulator::delta(...)` function per stream:

- default stream (type_name_id == 0)
- each ADS stream
- **each internal NTFS stream** (e.g. `$REPARSE`, `$OBJECT_ID`, `$SECURITY_DESCRIPTOR`, `$EA`, …)

Then it sums all of those per-stream deltas.

### 2) Our Rust port still “groups” internal streams into one delta

In the Rust port we currently have (and in the earlier drop-in):

- ADS streams are stored individually → **delta per ADS** ✅
- but *internal* streams are **filtered out** of the stream list and collapsed into:
  - `internal_streams_size`
  - `internal_streams_allocated`

…and we do:

```
result.length += delta(sum_internal_lengths, name_info, total_names)
```

### 3) Why that breaks parity

The delta function is **not linear** because of integer division rounding:

```
delta(v, i, n) = floor(v*(i+1)/n) - floor(v*i/n)
```

So in general:

```
delta(a+b, i, n) != delta(a, i, n) + delta(b, i, n)
```

Concrete example (n=2, i=0):

- internal stream A length = 1
- internal stream B length = 1

C++ does:

- delta(1,0,2) + delta(1,0,2)
- 0 + 0
- = 0

Rust grouped does:

- delta(2,0,2)
- = 1

That’s exactly the kind of **±1 byte** directory-level error you’re seeing in WinSxS / WindowsApps paths.

### 4) Why it only shows up in ~20 dirs

You only see this when **both** are true:

1. `total_names > 1` (hardlinks exist)
2. there are **2+ internal streams** on the record (or other combinations where rounding differs)

That’s rare → you get a small number of mismatched directories, with tiny diffs.

---

## The fix

### Make internal streams first-class for the tree pass (but still not printed)

We need to preserve internal streams as **individual stream entries (size only)** so that the tree algorithm can apply `delta()` per internal stream, exactly like C++.

Important: this does **NOT** mean we print internal streams as ADS rows. It just means the tree pass has access to them as separate size atoms.

---

## Patch overview

You’ll make **two changes**:

1) **Index structure change**: store internal streams as a separate per-record linked list.

2) **Tree algorithm change**: iterate the internal stream list and apply delta per internal stream.

I’m including:

- a complete replacement `cpp_tree.rs` (drop-in)
- the minimal `index.rs`/record changes needed to support `first_internal_stream` and `internal_streams`

---

## 1) `index.rs` (and record struct) patch

### A) Add an internal-stream entry type

Where you define the other index arrays (streams/children/etc):

```rust
#[derive(Clone, Copy, Debug, Default)]
pub struct InternalStreamInfo {
    pub next_entry: u32,   // NO_ENTRY terminator; uses 1-based index like StreamInfo.next_entry
    pub size: SizeInfo,    // length / allocated / bulkiness
}
```

### B) Extend `MftIndex`

```rust
pub struct MftIndex {
    // ...existing fields...

    pub internal_streams: Vec<InternalStreamInfo>,
}
```

Initialize it wherever you build the index:

```rust
internal_streams: Vec::new(),
```

### C) Extend your per-record struct

Where your record type lives (the one with `first_stream`, `stream_count`, `total_stream_count`, etc):

```rust
pub struct FileRecord {
    // ...existing fields...

    pub first_internal_stream: u32,
}
```

Ensure constructors/defaults set it to `NO_ENTRY`.

### D) Build the internal-stream linked list while filtering

In the section where you currently do the internal filter + sum:

- keep filtering internal streams out of `named_streams`
- but instead of only summing, also push each internal stream into `index.internal_streams` and link them

Pseudo-code (drop into your existing stream loop):

```rust
let mut first_internal = NO_ENTRY;
let mut last_internal  = NO_ENTRY;

for st in parsed.streams.iter() {
    if st.name.is_empty() {
        continue; // default stream handled elsewhere
    }

    let is_internal = st.name
        .strip_prefix('$')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_uppercase());

    if is_internal {
        // Keep your existing totals if you still want them:
        record.internal_streams_size += st.size.length;
        record.internal_streams_allocated += st.size.allocated;

        // NEW: store as individual internal stream entry
        let idx0 = self.internal_streams.len();
        self.internal_streams.push(InternalStreamInfo {
            next_entry: NO_ENTRY,
            size: st.size,
        });

        let entry = (idx0 as u32) + 1; // 1-based

        if first_internal == NO_ENTRY {
            first_internal = entry;
        }

        if last_internal != NO_ENTRY {
            self.internal_streams[(last_internal - 1) as usize].next_entry = entry;
        }

        last_internal = entry;
        continue;
    }

    // non-internal named ADS stream path continues as before...
}

record.first_internal_stream = first_internal;
```

That’s the entire structural part.

---

## 2) Drop-in `cpp_tree.rs` replacement

A complete replacement file is provided here:

- `cpp_tree_internal_stream_delta_fix.rs` (copy to `crates/uffs-mft/src/cpp_tree.rs`)

It implements:

- **per internal stream** delta contribution (fixes the ±1..±12 byte tree-size drift)
- uses `bulkiness` properly (no longer aliases it to allocated)
- includes a safe extra pass that initializes any unvisited nodes (helps live scan “Size=0/Desc=0” cases)

---

## Validation procedure

### Offline flow (what you asked for)

1. Delete the cached offline result (so it re-runs):

```bash
rm -f docs/trial_runs/f_disk/rust_offline_f.txt
```

2. Re-run the offline scan:

```bash
cargo run --release -- scan --offline-mft docs/trial_runs/f_disk/F_mft.bin --out docs/trial_runs/f_disk/rust_offline_f.txt
```

3. Run parity:

```bash
cargo run --release --bin compare_scan_parity -- \
  docs/trial_runs/f_disk/cpp_f.txt \
  docs/trial_runs/f_disk/rust_offline_f.txt

cargo run --release --bin analyze_trial_parity -- docs/trial_runs/f_disk
```

Expected result:

- **Tree Metrics issues: 0**
- the specific directories previously showing ±1..±12 byte drift should match

### Live scan follow-up

If live scan still shows “Size=0 / Desc=0” for some directories:

- ensure the live pipeline is using the same `compute_tree_metrics()` pass
- check whether those nodes are actually linked into the parent-child index (parent FRS exists and was indexed)

The new `cpp_tree.rs` includes a “process uninitialized components” pass. If you want to disable it for perf, wrap it behind a feature flag or a debug option.

---

## Files included

- `cpp_tree_internal_stream_delta_fix.rs` — full updated `cpp_tree.rs` replacement

