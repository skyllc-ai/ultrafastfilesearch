# LIVE Tree Metrics Parity (C++ algos) – Final Checklist & Triage Notes

This note is focused on the **LIVE/ONLINE** MFT path (IOCP + C++-style pipeline) when running with:

- `--parse-algo=cpp_port`
- `--io-algo=cpp`
- `--chunk-algo=cpp`
- `--tree-algo=cpp`
- `--no-cache`

It assumes **OFFLINE** (saved MFT file) is already at **100% parity** vs the legacy baseline.

---

## The Symptom Pattern (what “broken LIVE tree” looks like)

The exact symptom you showed is very diagnostic:

| Path | C++ Size | Rust LIVE Size | C++ Desc | Rust LIVE Desc |
|------|----------|----------------|----------|----------------|
| `G:\` | huge | `0` | huge | `0` |
| junction dirs | correct size | correct size | `1` | `0` |

When **root and (some) junctions have `Size=0` and/or `Descendants=0` while most other directories look fine**, it almost always means:

1. **The correct `cpp_tree` implementation was *not actually executed*** in the LIVE path, **or**
2. A **historically broken** cpp-tree variant ran (single-channel / wrong descendants writeback), **or**
3. The traversal ran but **did not “touch”** some components (needs an orphan sweep).

---

## What MUST be true for LIVE parity to match OFFLINE parity

### 1) Tree dispatch must call the correct module (no “org” shim)

**Required invariant**

`MftIndex::compute_tree_metrics_cpp_port()` must dispatch to the *current* `cpp_tree` module.

If this still points to `cpp_tree_org`, LIVE can silently run a broken older implementation even when the binary compiles.

**Minimal change**

```rust
fn compute_tree_metrics_cpp_port(&mut self, debug: bool) {
    // ✅ must call the fixed implementation
    crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
}
```

If you want to make this bulletproof: add a **one-line log** inside the function that prints which implementation you’re running.

---

### 2) Directory descendants must use the “printed” channel, not the propagation channel

A junction directory is a perfect canary:
- It has **no children** → printed descendants should be **exactly `1`**
- If you write the wrong value (propagation channel or stream-count), you’ll see `0` or `2` (or other nonsense).

**Correct formula (printed channel B)**

```rust
rec.descendants = children_agg.treesize + 1;
```

**NOT**
- `rec.descendants = result.treesize` (channel A)
- `rec.descendants = children_agg.treesize` (missing the `+1` → leaf dirs become `0`)

---

### 3) LIVE needs an orphan sweep after ROOT traversal

Even with perfect parsing, the LIVE pipeline can occasionally end up with “unreachable” islands (timing/out-of-order effects, placeholder parent records, name merge ordering, etc.).

**Fix**
- Track a `visited: Vec<bool>` for record indices.
- Set `visited[idx] = true` in `preprocess()`.
- After `ROOT` traversal, scan all records and call `preprocess()` for any record that was never visited.

**Important guardrail**

Do **not** use `record.descendants == 0` to detect “unvisited”, because:
- Files legitimately have `descendants=0`
- Leaf directories should be `1` (but a bug can make them `0`, masking the real root cause)

Use a dedicated `visited` array.

---

### 4) The delta function must be EXACTLY the C++ delta (don’t “first rem gets +1”)

If you ever see **1–4 byte** directory size differences that don’t correlate to rounding boundaries, it’s often a delta mismatch.

**Correct C++ delta (the only safe one)**

```rust
const fn delta(value: u64, i: u32, n: u32) -> u64 {
    if n <= 1 { return value; }
    let n64 = n as u64;
    let i64 = i as u64;
    value * (i64 + 1) / n64 - value * i64 / n64
}
```

The common shortcut:

```rust
base + if i < rem { 1 } else { 0 }
```

is **NOT equivalent** to the C++ formula (e.g. with `n=2`, the extra byte goes to the *second* link in C++).

---

## “If the new test still fails” – the next three likely culprits

If, after the above, LIVE is still off in a way OFFLINE is not, it almost always falls into one of these:

### A) The binary you executed isn’t the one you built

Concrete check:
- log `uffs --version` into `uffs_version.log`
- print the computed algorithm selection at startup (tree/io/parse/chunk) as one line

### B) ROOT exists but has **no children** in LIVE index

This yields exactly: `G:\ Size=0 Desc=0/1`, while `G:\MFT_TEST\` looks correct (computed as an orphan component).

Add a one-time debug after `sort_directory_children()`:

```rust
let root_idx = index.frs_to_idx.get(5).copied().unwrap_or(NO_ENTRY);
tracing::info!(?root_idx, root_children = index.records[root_idx as usize].children_count,
               root_first_child = index.records[root_idx as usize].first_child,
               "root linkage");
```

If `children_count == 0` on LIVE but not OFFLINE, focus on **LIVE pipeline index construction** (childinfos population / placeholder merge).

### C) Placeholder-parent merge bug in the LIVE pipeline

In out-of-order parsing, it’s easy to:
- create a placeholder record for parent P (to hang childinfos off it),
- then later **allocate a second “real” record** for P,
- leaving children on placeholder and names on real record.

That produces “looks fine” paths (using the real record) but “empty” traversal (because children are on the placeholder).

Concrete debugging:
- count how many records have `frs == 5`
- ensure it’s exactly `1`
- spot-check for any duplicate FRS in `records`

---

## Recommended “one-shot” validation run

After you apply fixes:

1. Run LIVE scan with `--no-cache`
2. Immediately run OFFLINE scan with the saved MFT
3. Run `scripts/analyze_trial_parity.rs`

Expected:
- **Quick Comparison:** Live Tree Metrics ✅, Offline Tree Metrics ✅
- `G:\` matches exactly (size + descendants)
- junctions show `Descendants=1`

---

## Optional: add a hard assertion in debug builds

This catches regressions instantly:

```rust
debug_assert!(
    index.records.iter().any(|r| r.is_directory && r.descendants > 0),
    "tree metrics not computed at all (all dir descendants are 0)"
);
```

And specifically for ROOT:

```rust
if let Some(&root_idx) = index.frs_to_idx.get(5) {
    if root_idx != NO_ENTRY {
        let root = &index.records[root_idx as usize];
        debug_assert!(root.descendants > 0, "ROOT descendants not computed");
    }
}
```

---

## Bottom line

Given your changelog, you already hit the **three** key issues:

1. wrong dispatch (`cpp_tree_org` vs `cpp_tree`)
2. leaf-dir descendants stored incorrectly in historical cpp-tree variants
3. missing orphan sweep in LIVE (components can stay unvisited)

If the current test still shows root/junction zeros, the fastest path to resolution is:

- **prove which tree implementation is running**
- **print ROOT linkage** (`children_count`, `first_child`)
- **prove there aren’t duplicate FRS records** (placeholder merge)

