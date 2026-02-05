# LIVE Tree Metrics Remaining Gap – Deep Dive (Root + Junction Descendants = 0)

**Context:** Your `analyze_trial_parity.rs` output shows **OFFLINE = perfect parity** with C++, while **LIVE has 3 tree-metrics failures**:

| Path | C++ Size | LIVE Size | C++ Desc | LIVE Desc |
|------|----------|-----------|----------|-----------|
| `G:\` | 609,893,968 | 0 | 15,106 | 0 |
| `G:\MFT_TEST\PhotosJunction\` | 48 | 48 | 1 | 0 |
| `G:\MFT_TEST\ReportsJunction\` | 48 | 48 | 1 | 0 |

Everything else (row count, path match, ADS count, timestamps, boolean flags) matches.

---

## What the output *proves* (important)

### 1) This is not a delta / stream accounting problem anymore
- If it were delta math or internal stream counting, you’d see lots of ±1/±N differences across many paths.
- Instead, you have **exactly 3 directories** where the tree fields are effectively “unset”.

### 2) These 3 rows look like “tree metrics never stamped”
The two junction rows are especially telling:

- **LIVE Size = 48** matches C++.
- **LIVE Descendants = 0** is the default/uninitialized value in most implementations.
- A leaf directory, once the tree pass runs, should *never* end with descendants=0 — even with no children it should become **1** (the directory itself).

Root (`G:\`) having both Size=0 and Desc=0 is the same story, just more dramatic: it’s exactly what you get when the tree computation never “lands” on the root record (or it bails out early before writing).

So treat this as a “**record not stamped / record was stamped and later reset**” problem.

---

## The 3 realistic failure modes

### Failure mode A (most common): LIVE is still not running the fully-fixed cpp_tree code path
Even if the code exists, LIVE can still be using:
- `TreeAlgorithm::Current` (leaf-peeling / old tree), or
- an older module (`cpp_tree_org`), or
- an unintended default due to CLI/config.

**Your “Tripwire” change is the right move.** If the tripwire doesn’t appear in the live logs, nothing else matters.

**Why this failure mode matches the symptom:**
- The old/leaf-peeling tree implementations often leave:
  - **root unset** (root gets special-cased poorly)
  - **junctions unset** (reparse / edge cases)
- Meanwhile, “most other directories” still look fine.

✅ Your fix: **TreeAlgorithm::default() → CppPort** + tripwire  
✅ Also ensure any explicit dispatch uses `crate::cpp_tree`, not `cpp_tree_org`.

---

### Failure mode B: The records are stamped, then later replaced/reset (ordering bug)
This is the classic “tree computed on one `FileRecord`, but output prints a different one” problem, usually from:

- deferred name merges that *replace* entries in `index.records`
- rebuild/repair passes that create new `FileRecord`s and overwrite old ones
- fragment merging paths that reconstruct record structs

If that replacement happens **after** tree metrics, those replaced records will print:
- `descendants = 0`
- `treesize = 0`
…even though the rest of the tree looks correct.

**Why this fits the symptom extremely well:**
- Only a tiny set of records go through special merge/rebuild paths:
  - root
  - reparse/junction dirs  
- Everyone else stays in place and keeps their computed tree fields.

✅ Fix: ensure **any record replacement preserves tree fields**, or (simpler) ensure **tree metrics are computed after the last record-replacement step**.

Concrete rule:
> Anything that mutates or replaces `index.records[idx]` must happen **before** tree metrics, or must be followed by a tree recompute.

---

### Failure mode C: cpp_tree is bailing out early for these records (name_count/stream_count = 0 guard)
There is one code smell that exactly produces **“only a couple directories stuck at 0”**:

- `preprocess()` sets “visited/seen”
- then hits a guard like `if name_count == 0 { return ... }`
- and returns **before writing** `rec.descendants/rec.treesize`

Result:
- record remains with default `descendants=0`
- and since it’s marked visited, an orphan sweep won’t fix it

**Why root + junctions are prime targets:**
- LIVE pipelines sometimes leave `name_count` or `stream_count` as `0` for special records (root, reparse) due to:
  - partial parsing
  - placeholder/default CppRecord initialization
  - ordering differences in name harvesting

✅ Fix: treat these fields as invariants:
- `name_count` must behave as at least **1**
- `total_stream_count` must behave as at least **1** for counting purposes

…and never “return early without stamping” for a directory.

---

## The fastest way to disambiguate A vs B vs C

Add **one** post-tree check (even temporarily) right after tree metrics finishes — *before writing output*:

```rust
let mut bad = Vec::new();
for (idx, rec) in self.records.iter().enumerate() {
    if rec.stdinfo.is_directory() && rec.descendants == 0 {
        bad.push((idx, rec.frs, rec.first_child, rec.name_count, rec.total_stream_count, rec.stdinfo.is_reparse()));
    }
}
tracing::warn!(bad_dir_count = bad.len(), ?bad, "[tree] dirs with descendants==0 after tree metrics");
```

Interpretation:
- If `bad_dir_count == 3` and it includes root + junctions, then those records are truly unstamped at that point → **Failure mode A or C**.
- If `bad_dir_count == 0` there, but the CSV later shows 3 bad rows → **Failure mode B** (stamped then overwritten/reset later).

This single log line saves hours.

---

## Hard fix that covers B and C (and won’t break OFFLINE)

### Fix 1: Clamp invariants at the source (LIVE conversion)
In the LIVE conversion (CppRecord → FileRecord), clamp:

```rust
name_count: cpp_record.name_count.max(1),
total_stream_count: cpp_record.stream_count.max(1),
```

This prevents “special records” from ever being created with zeros that can trip guards.

### Fix 2: Never skip stamping a directory
In the cpp_tree traversal:
- do **not** return early for `name_count == 0` / `total_names == 0`
- treat `total_names = total_names.max(1)`
- always stamp:

```rust
if is_directory {
    rec.descendants = children.treesize.saturating_add(1);
    rec.treesize = children.length.saturating_add(first_len);
    rec.tree_allocated = children.allocated.saturating_add(first_alloc);
}
```

### Fix 3: If you *must* bail, don’t mark seen/visited first
If there is any unavoidable early return path, it must happen **before** setting the visited flag, otherwise the orphan sweep cannot recover.

---

## Why your latest changes are directionally correct

You implemented:
1. **TreeAlgorithm::default → CppPort**  
2. **Tripwire** to verify LIVE is actually running fixed cpp_tree  
3. **Root self-heal** (trigger on root `descendants==0 || treesize==0`)

These address failure mode A directly.

If LIVE still fails after that:
- prioritize the “post-tree dirs with descendants==0” diagnostic above
- then apply Fix 1 + Fix 2 (clamp + no-early-return stamp)

---

## Verification checklist (don’t skip)

1. Delete the old trial outputs (or regenerate them fresh).
2. Re-run LIVE scan **with logs enabled** and confirm the tripwire line prints.
3. Run `scripts/analyze_trial_parity.rs` again.
4. Expect: **Tree Metrics issues → 0**

If it stays at 3:
- use the post-tree diagnostic to determine whether this is (B) “reset after compute” or (C) “bail-out before stamp”.

---

## Appendix: What I can already verify from your provided outputs

From your `rust_live_g.txt`:

- The only directories with `Directory Flag = 1` and `Descendants = 0` are:
  - `G:\`
  - `G:\MFT_TEST\PhotosJunction\`
  - `G:\MFT_TEST\ReportsJunction\`

That’s exactly the “unstamped directory” signature — not a math drift signature.
