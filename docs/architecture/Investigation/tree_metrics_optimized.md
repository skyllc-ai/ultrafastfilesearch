
# High‑Performance Tree Metrics Computation (Rust)

This document proposes a **maximally optimized yet maintainable** algorithm for computing directory tree metrics (descendant count, total size, total allocated size) from NTFS‑style records arriving in random order.  
The approach preserves **O(n)** complexity while significantly improving **constant factors, cache locality, and memory behavior**.

---

## Executive Summary

Your original recursive + memoization algorithm is **asymptotically optimal** but not **constant‑factor optimal**.  
The main costs come from:

- HashMap lookups in hot paths
- Per‑parent `Vec` allocations for children
- Recursive traversal overhead
- Poor cache locality due to pointer chasing

The recommended upgrade is a **bottom‑up, non‑recursive “leaf‑peeling” dynamic programming pass** (Kahn‑style), which:

- Eliminates recursion entirely
- Avoids storing child lists
- Touches each edge exactly once
- Uses only tight `Vec`‑based data structures
- Is trivial to reason about and debug

This is about as fast as the problem can be solved without sacrificing maintainability.

---

## Core Idea: Bottom‑Up Leaf Peeling

A node is *ready* once all of its children have been processed.

If we know, for each directory:

- its **parent index**
- how many **direct children remain unprocessed**

then we can compute metrics bottom‑up in a single pass.

### High‑Level Steps

1. Map `frs → dense index`
2. Precompute:
   - `parent_idx[i]`
   - `pending_children[i]`
3. Initialize each node’s base metrics (its own file contribution)
4. Push all nodes with `pending_children == 0` into a stack
5. Repeatedly:
   - Pop a ready node
   - Accumulate its metrics into its parent
   - Decrement the parent’s pending count
   - Push parent when it becomes ready

This naturally produces a post‑order traversal **without recursion or sorting**.

---

## Algorithmic Properties

| Property | Value |
|--------|------|
| Time Complexity | **O(n)** |
| Memory Overhead | **O(n)** |
| Traversals | Exactly one parent update per node |
| Recursion | None |
| Stack Safety | Guaranteed |
| Order Sensitivity | None |
| Cache Locality | Excellent |
| Parallel Safety | Deterministic |

---

## Rust Implementation (Production‑Ready Skeleton)

```rust
use hashbrown::HashMap;
use ahash::RandomState;

const NONE: u32 = u32::MAX;

pub struct Record {
    pub frs: u64,
    pub parent_frs: u64,
    pub is_directory: bool,
    pub size: u64,
    pub allocated_size: u64,

    // outputs
    pub descendants: u32,
    pub treesize: u64,
    pub tree_allocated: u64,
}

pub fn compute_tree_metrics(records: &mut [Record]) {
    let n = records.len();

    // 1) Build FRS -> dense index
    let mut idx_of: HashMap<u64, u32, RandomState> =
        HashMap::with_capacity_and_hasher(n * 4 / 3, RandomState::new());

    for (i, r) in records.iter().enumerate() {
        idx_of.insert(r.frs, i as u32);
    }

    // Snapshot directory flags
    let is_dir: Vec<bool> = records.iter().map(|r| r.is_directory).collect();

    // 2) Parent links + child counts
    let mut parent_idx = vec![NONE; n];
    let mut pending_children = vec![0u32; n];

    for i in 0..n {
        let (frs, parent_frs, size, alloc) = {
            let r = &records[i];
            (r.frs, r.parent_frs, r.size, r.allocated_size)
        };

        // Base contribution
        records[i].descendants = 0;
        records[i].treesize = size;
        records[i].tree_allocated = alloc;

        // Root or self-parent
        if parent_frs == frs {
            continue;
        }

        if let Some(&p) = idx_of.get(&parent_frs) {
            let p = p as usize;
            if is_dir[p] && p != i {
                parent_idx[i] = p as u32;
                pending_children[p] += 1;
            }
        }
    }

    // 3) Initialize ready stack
    let mut stack: Vec<u32> = Vec::with_capacity(n);
    for i in 0..n {
        if pending_children[i] == 0 {
            stack.push(i as u32);
        }
    }

    // 4) Bottom-up accumulation
    let mut processed = 0usize;

    while let Some(i_u32) = stack.pop() {
        let i = i_u32 as usize;
        processed += 1;

        let p_u32 = parent_idx[i];
        if p_u32 == NONE {
            continue;
        }

        let p = p_u32 as usize;

        let (d, s, a) = {
            let c = &records[i];
            (c.descendants, c.treesize, c.tree_allocated)
        };

        records[p].descendants += 1 + d;
        records[p].treesize += s;
        records[p].tree_allocated += a;

        pending_children[p] -= 1;
        if pending_children[p] == 0 {
            stack.push(p_u32);
        }
    }

    // 5) Defensive corruption detection
    if processed != n {
        // Cycles or broken parent links detected.
        // Leave partial aggregates and optionally log diagnostics.
    }
}
```

---

## Why This Is Faster Than Recursive Memoization

| Aspect | Recursive Approach | Leaf‑Peeling |
|-----|------------------|-------------|
| HashMap lookups | Many | One‑time |
| Heap allocations | Child Vecs | None |
| Recursion | Yes | No |
| Cache locality | Poor | Excellent |
| Stack safety | Risky | Guaranteed |
| Data movement | High | Minimal |

The hot loop is reduced to **three integer additions and one decrement**.

---

## Parallelization Guidance

Do **not** parallelize this pass unless absolutely necessary.

Reasons:
- Parent aggregation creates write contention
- Atomics slow things down
- Memory bandwidth becomes the bottleneck

Best practice:
- Parallelize *earlier pipeline stages* (MFT parsing, path decoding, hashing)
- Keep this aggregation single‑threaded and extremely fast

If parallelism is mandatory:
- Partition by root directories
- Run subtrees independently
- Merge at the top

This increases complexity and usually isn’t worth it.

---

## Memory Layout Optimizations

High‑impact, low‑complexity improvements:

1. **Avoid child adjacency lists entirely**
2. Use `u32` indices instead of `usize` when possible
3. Prefer SoA scratch buffers for hot metrics
4. Use LIFO stack (`Vec`) over queues for cache warmth

Optional (advanced):
- If FRS space is dense, replace `HashMap` with a direct index array

---

## Edge Case Handling

### Orphans
- Missing parent → treated as independent root
- Optional: attach to a synthetic “orphan root”

### Non‑directory parents
- Ignore edge, treat child as orphan

### Cycles / corruption
- Detected automatically (`processed != n`)
- Do **not** attempt repair
- Log + continue safely

---

## Final Verdict

This approach:

- Matches theoretical optimality
- Minimizes real‑world runtime
- Is easy to audit and maintain
- Scales cleanly to 10M+ nodes
- Avoids all recursion and heavy data structures

If you implement only one change: **replace recursion + children maps with leaf‑peeling DP**.

That’s as far as this problem can be pushed without turning it into a science project.
