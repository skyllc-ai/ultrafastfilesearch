# NTFS MFT LIVE Parity – Root `H:\` Tree Metrics = 0 (Why it happens + Fix)

**Date:** 2026-02-05  
**Scope:** The remaining LIVE/ONLINE parity bug where only the **volume root** row prints:

- `Size = 0`
- `Size on Disk = 0`
- `Descendants = 0`

…while **all other directories/files match C++**, and **OFFLINE** scan matches the legacy baseline.

---

## Symptom recap (what is “still wrong”)

From your H-drive parity report:

- LIVE scan: **only `H:\`** has `Size/Desc = 0`
- OFFLINE scan: everything correct, including `H:\`

From `rust_live_trace_h.txt`:

- Tree metrics run completes
- `root_looks_bad=false` and `bad_dir_count=0`

So **tree metrics are being computed**, yet the **exported root row** still prints zeros.

That combination is the key clue.

---

## The real meaning of `H:` vs `H:\` on Windows (this matters)

On Windows, these are *not equivalent*:

- `H:`  → *drive-relative current directory on H* (Win32 maintains a per-drive “current directory”)
- `H:\` → the actual NTFS volume root directory

Many code paths “normalize” to `X:\` because:

- it is unambiguous
- it matches the legacy baseline/Win32 APIs
- it matches your legacy baseline output format

If *any* part of your pipeline uses `H:` as a key and another part uses `H:\` as a key, **only the root** will collide/miss, because every non-root path has at least one `\` separator.

---

## Root cause (confirmed in your current `index.rs`)

### 1) `PathResolver::materialize_path()` returns `H:` for root

Your current `crates/uffs-mft/src/index.rs` (the one you uploaded in this thread) builds the path like this:

- starts with `"{volume}:"`
- appends `\name` only for non-empty names
- explicitly skips `"."` (which is the common root directory `FILE_NAME`)

That means for the root record (FRS=5), the chain components are effectively empty → path remains **`"H:"`**.

This is exactly what we see in the current file:

```rust
path.push(self.volume.to_ascii_uppercase());
path.push(':');

for &chain_idx in chain.iter().rev() {
    if let Some(record) = index.records.get(chain_idx) {
        let name = index.record_name(record);
        if !name.is_empty() && name != "." {
            path.push('\\');
            path.push_str(name);
        }
    }
}
path
```

There is **no final normalization** step to convert `H:` into `H:\`.

### 2) `materialize_path_for_name()` also returns `H:` when the parent is root

You also still have this branch:

```rust
} else if parent_frs == ROOT_FRS {
    format!("{}:", self.volume.to_ascii_uppercase())
}
```

So hardlink path materialization has the same issue.

---

## Why this produces *exactly* your observed parity failure

You have a classic “only-root breaks” failure mode:

- Most directories: `materialize_path()` yields `H:\temp_test` etc → downstream keys match → metrics show up
- Root directory: `materialize_path()` yields `H:` → downstream expects `H:\` → lookup fails → defaults to 0

This matches your observation perfectly:

- All rows match C++
- Only `H:\` prints zeros for the tree metrics

And it also matches your trace:

- Tree metrics **are computed** (root is fine internally)
- Export stage is producing the wrong values for root **because the root key/path string is inconsistent**

---

## The fix (drop-in, minimal, safe)

### Fix 1: Normalize root in `materialize_path()`

After building the path string, add:

```rust
// Normalize volume root to "X:\"
if path.len() == 2 && path.as_bytes().last() == Some(&b':') {
    path.push('\\');
}
```

This converts the ambiguous `H:` into canonical `H:\`.

### Fix 2: Normalize root parent path in `materialize_path_for_name()`

Replace the `parent_frs == ROOT_FRS` branch so it returns `H:\`:

```rust
} else if parent_frs == ROOT_FRS {
    let mut s = String::with_capacity(3);
    s.push(self.volume.to_ascii_uppercase());
    s.push(':');
    s.push('\\');
    s
}
```

### Fix 3: Avoid double separators when joining

If the parent path is already `H:\`, do **not** add another `\`:

```rust
let ends_with_sep = parent_path.as_bytes().last() == Some(&b'\\');
if !ends_with_sep {
    path.push('\\');
}
```

---

## How to verify you really compiled + are running the fixed binary

Because this bug is purely *string normalization*, it’s easy to prove:

Add a one-time log right after you build the path cache / resolver (or right before export):

- Print the root FRS (5) path as produced by the fast resolver/cache

Expected after the fix:

- `PathCache.get(5)` prints `H:\`

If it prints:

- `H:` → you are still running a binary that includes the old behavior

### “I applied the patch but it still happens” checklist

1. **Make sure you replaced the correct file**
   - `crates/uffs-mft/src/index.rs` (not a similarly named file in another crate)

2. **Force rebuild**
   - `cargo clean`
   - rebuild the binary you actually run (debug vs release, workspace vs crate)

3. **Make sure the CLI uses the new build output**
   - If you copy an `.exe` around, make sure you copied the rebuilt one

---

## Drop-in replacement provided

I generated a zip with the exact drop-in source file layout:

- `crates/uffs-mft/src/index.rs`  ✅ patched root normalization
- plus the other relevant files you included (so you can overwrite consistently)

File: `uffs_fix_dropin_v3.zip`

---

## Expected result after applying this patch

On the next LIVE scan of H:

- The root row should match C++:

| Path | Size | Descendants |
|------|------|-------------|
| `H:\` | ~42168722 | 57 |

…and the LIVE parity report should show:

- **Tree Metrics:** ✅ OK

---

## Optional hardening (recommended)

1. **Unit test for root normalization**
   - Build a tiny synthetic index with root-only and ensure:
     - `materialize_path(root) == "H:\\"`
     - `materialize_path_for_name(child_hardlink_to_root) == "H:\\child"`

2. **Single canonical path builder**
   - Right now you have multiple path constructors:
     - `build_path()`
     - `PathResolver::materialize_path()`
   - Ensure they agree on root formatting so this can’t regress.

---

## Patch summary (what changed)

Only `PathResolver` path output behavior for root:

- Before: root → `H:`
- After:  root → `H:\`

No changes to:

- MFT parsing
- tree-metrics algorithm
- orphan sweep logic

So risk is extremely low, and the fix is fully deterministic.

