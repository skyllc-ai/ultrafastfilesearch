# LIVE/ONLINE Tree Metrics Parity: Root + Junctions Showing Descendants=0

**Date:** 2026-02-03  
**Scope:** Fix remaining LIVE (online MFT) parity gap where OFFLINE is correct but LIVE still shows tree-metric zeros for:
- `G:\` (root) → **Size=0, Descendants=0**
- `G:\MFT_TEST\PhotosJunction\` → **Descendants=0** (C++ shows 1)
- `G:\MFT_TEST\ReportsJunction\` → **Descendants=0** (C++ shows 1)

This document explains **why** this happens and provides the **drop-in code** to fix it.

---

## 1) What the data is telling us

From the fresh parity report:

- **Offline scan:** ✅ Tree metrics OK  
- **Live scan:** 🔴 3 issues  
  - Root `G:\` has **Size=0** and **Descendants=0**
  - Both junctions have **Size correct (48)** but **Descendants=0**

Key implication:

- A directory with **Descendants=0** is almost always a record whose `descendants` field was **never written by the tree pass** (it stayed at its default value).
- Junction sizes still being 48 strongly suggests that **file record parsing is fine** (directory stream size was captured), but the **tree post-processing did not stamp computed descendants** for those records.

So this is **not** “MFT parsing is broken” – it’s **the live tree stage not applying to the right implementation / not updating those records**.

---

## 2) Root cause: LIVE is calling the wrong C++ tree implementation

Your codebase contains *two* C++ tree ports:

- A “legacy/original” module (commonly named something like `cpp_tree_org`)
- The “fixed” module you’ve been iterating on (two-channel model + internal stream delta distribution)

In the LIVE pipeline (`io.rs`) you **do** call:

- `index.compute_tree_metrics();`

…but inside `index.rs`, the C++ tree dispatch was still wired like:

```rust
fn compute_tree_metrics_cpp_port(&mut self) -> Result<()> {
    crate::cpp_tree_org::compute_tree_metrics_cpp_port(self)
}
```

Meaning:

✅ your “fixed” `cpp_tree.rs` can be perfect  
❌ but it will never run in LIVE mode if `index.rs` still routes to `cpp_tree_org`

That explains the exact symptom pattern:

- Root record not updated → stays 0/0
- Junctions skipped / not stamped → descendants stays 0
- Everything else looks “mostly correct” because those records get values from either:
  - the legacy tree pass, or
  - pre-existing non-tree fields that happen to match for non-edge cases

---

## 3) The required fix

### 3.1 Fix the tree dispatch

Update `index.rs` so the C++ tree algorithm uses the *patched* implementation:

```rust
fn compute_tree_metrics_cpp_port(&mut self) -> Result<()> {
    crate::cpp_tree::compute_tree_metrics_cpp_port(self)
}
```

That is the minimum wiring change required so the LIVE run uses the correct tree implementation.

### 3.2 Ensure the C++ tree implementation *stamps* root + junctions

The patched `cpp_tree.rs` should include:

- **Two-channel model**
  - **Propagate** internal streams to parent aggregation
  - **Print/store** per-directory metrics excluding internal streams for the directory itself  
    (this is why junction prints Descendants=1 but still contributes its internal stream to parent)
- **Per-internal-stream delta distribution**
  - Distribute `(size - allocated)` (or delta) **per internal stream**, then sum  
    (avoids 1–4 byte rounding differences in some directories)
- **Orphan sweep**
  - After processing from root, sweep the record array and compute any unvisited nodes  
    (this guarantees every directory gets a stamped `descendants`, even if some linking differs)

---

## 4) Drop-in files

Two drop-in replacements are provided:

1. `index_dropin_live_tree_fix.rs`  
   - Same as your patched `index.rs`, but with the **tree dispatch fix** (uses `crate::cpp_tree` not `crate::cpp_tree_org`)

2. `cpp_tree_dropin_live_tree_fix.rs`  
   - The C++ tree port with:
     - two-channel logic
     - per-internal-stream delta distribution
     - orphan sweep

---

## 5) How to apply in your repo

Replace these files:

- `crates/uffs-mft/src/index.rs`  
  ⟶ replace with **index_dropin_live_tree_fix.rs**

- `crates/uffs-mft/src/cpp_tree.rs`  
  ⟶ replace with **cpp_tree_dropin_live_tree_fix.rs**

Important:

- If your crate root (`lib.rs` / `mod.rs`) does not already include `mod cpp_tree;`, add it.
- You can keep `cpp_tree_org` around for reference, but it should **not** be used by `compute_tree_metrics_cpp_port()`.

---

## 6) Verification steps

After rebuild/deploy, re-run:

- `uffs.exe "*" --drive G --parse-algo=cpp_port --tree-algo=cpp --io-algo=cpp --chunk-algo=cpp --no-cache`

Then confirm:

- `G:\` now has **non-zero** `Size` and **Descendants=15106** (matching C++)
- `PhotosJunction` / `ReportsJunction` now have **Descendants=1**
- Parity report: **Tree Metrics = ✅ OK**

If tree metrics still come out as zeros, that means the binary you ran is still using the old dispatch (or an older build). Verify the version log and ensure the new binary is the one being executed.

---
