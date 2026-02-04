# LIVE tree metrics still failing while OFFLINE is correct — root cause + final fix

**Context (from your 2026‑02‑04 parity run):**
- **Offline scan:** ✅ Tree metrics match C++.
- **Live scan:** ❌ Only **3 directories** have broken tree metrics (everything else matches):
  - `G:\` → **Size = 0**, **Descendants = 0**
  - `G:\MFT_TEST\PhotosJunction\` → **Descendants = 0** (should be 1)
  - `G:\MFT_TEST\ReportsJunction\` → **Descendants = 0** (should be 1)

This pattern is *not* a delta math / internal-stream distribution issue anymore.  
It’s the signature of **“the correct tree pass is not being applied to those nodes”**, which almost always comes down to **dispatch / code-path mismatch** in LIVE.

---

## What the symptom pattern means

### 1) Junction descendants == 0
A junction folder is a **leaf directory**. In the C++ output it always comes out as:
- `Descendants = 1` (the directory itself)

If you see `Descendants = 0`, one of these is true:
- The LIVE run is still calling an **old C++-port tree implementation** that does **not** add `+1` for leaf directories, *or*
- The directory is **never stamped by the tree pass** (tree didn’t visit it + no orphan sweep).

### 2) Root `G:\` has Size/Descendants == 0 while all subtrees look correct
This almost always means:
- The LIVE run computed tree metrics for many directories, **but did not compute the true root**, because:
  - The **wrong tree algo module** is being called (or an old shim), or
  - Root traversal didn’t start at FRS=5 and there was **no orphan sweep**, or
  - The tree algo ran, but later you output a **different record** than the one being stamped (rare; dispatch mismatch is far more common).

Given that only 3 directories are wrong, dispatch mismatch is the top suspect.

---

## The fix that stops the “circling”: force LIVE to use the fixed tree module (and prove it in logs)

### Step A — Kill `cpp_tree_org` from the *LIVE* dispatch

In **`crates/uffs-mft/src/index.rs`** (or wherever `MftIndex::compute_tree_metrics_cpp_port()` lives),
make sure it calls the fixed module:

```rust
fn compute_tree_metrics_cpp_port(&mut self, debug: bool) {
    tracing::debug!("[tree] dispatch -> cpp_tree (FIXED)");
    crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
}
```

✅ **Do NOT** call `crate::cpp_tree_org::...` anywhere for `--tree-algo=cpp`.

Then run:

```bash
rg "cpp_tree_org" crates/uffs-mft/src
```

This should return **zero** hits for production dispatch (tests can keep it, but LIVE must not).

---

### Step B — Add a “tripwire” log inside the fixed tree algorithm

In **`crates/uffs-mft/src/cpp_tree.rs`** (the fixed implementation), add:

```rust
pub fn compute_tree_metrics_cpp_port(index: &mut MftIndex, debug: bool) {
    tracing::debug!("[cpp_tree] FIXED implementation is running");
    // ...
}
```

Now a LIVE trace run *must* include that line.  
If it doesn’t, you’re still executing the wrong code path/binary.

This is the fastest way to stop “we implemented it but results didn’t change”.

---

## Optional but recommended: make LIVE self-heal missing child linkage

Even once you’re on the fixed module, LIVE can still have a partial child-list if the C++ IO/parse pipeline
occasionally produces incomplete `childinfos` for the root.

Add a post-pass guard:

```rust
let debug = false; // or your existing flag
crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);

// If root still looks uninitialized, rebuild child lists from names and re-run.
if let Some(root_idx) = self.frs_to_idx_opt(5) {
    let root = &self.records[usize::from(root_idx)];
    if root.stdinfo.is_directory() && (root.descendants == 0 || root.treesize == 0) {
        tracing::warn!(
            "[tree] ROOT metrics look uninitialized after cpp_tree; rebuilding children from names and rerunning"
        );
        self.rebuild_children_from_names();
        crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
    }
}
```

This guarantees root aggregation even if LIVE child pointers were incomplete on first build.

---

## Verification (what should change immediately)

After Step A + Step B, run LIVE with trace enabled.

You should see:
- `[tree] dispatch -> cpp_tree (FIXED)`
- `[cpp_tree] FIXED implementation is running`

And your parity report should change like this:

| Path | Before | After (expected) |
|------|--------|------------------|
| `G:\MFT_TEST\ReportsJunction\` | Desc=0 | **Desc=1** |
| `G:\MFT_TEST\PhotosJunction\` | Desc=0 | **Desc=1** |
| `G:\` | Size=0, Desc=0 | **Size=609893968, Desc=15106** |

If the two junctions fix but root stays wrong:
- Your tree algo is now correct, but LIVE root children linkage is incomplete → enable the rebuild fallback.

---

## Why OFFLINE can be perfect while LIVE is wrong

OFFLINE uses a stable, fully materialized MFT snapshot and tends to build/link children deterministically.
LIVE is sensitive to:
- ordering, partial linkage, transient gaps, and any difference in which module gets called.

When OFFLINE is correct and LIVE is wrong **with only a handful of nodes wrong**,
it’s almost always **a code-path mismatch** (wrong tree module for LIVE) or a missing “orphan sweep” / rebuild guard.

---

If you want, paste the LIVE trace section around “Computing tree metrics…” after adding the two tripwire logs above;
it will immediately prove whether you’re still running `cpp_tree_org` in LIVE or not.
