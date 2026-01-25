# Tree Metrics Computation Algorithm Question

**Context**: High-performance file indexing system (Rust) that reads NTFS Master File Table (MFT) and builds an in-memory index.

**Date**: 2026-01-25

---

## Problem Statement

I need to compute **tree metrics** (descendants count, total size, total allocated size) for all directories in a file system tree. The challenge is that the input data (MFT records) arrives in **FRS order** (essentially random), not in tree order.

### Input Data

- **~1 million file/directory records** (typical case, can be up to 10M+)
- Each record has:
  - `frs`: Unique file record number (0, 1, 2, 3, ...)
  - `parent_frs`: FRS of parent directory
  - `is_directory`: Boolean flag
  - `size`: Logical file size (bytes)
  - `allocated_size`: Disk space used (bytes)
- Records arrive in **FRS order**, NOT tree order (child might come before parent)

### Output Required

For each directory, compute:
1. **descendants**: Count of all files/subdirectories in subtree
2. **treesize**: Sum of logical sizes of all files in subtree
3. **tree_allocated**: Sum of disk sizes of all files in subtree

For files, these values are trivial:
- `descendants = 0`
- `treesize = size`
- `tree_allocated = allocated_size`

---

## Current Algorithm (Bottom-Up with Memoization)

```rust
// Step 1: Build parent-child map (O(n))
let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
for record in records {
    children.entry(record.parent_frs).or_default().push(record.frs);
}

// Step 2: Recursive computation with memoization (O(n) amortized)
fn compute_metrics(frs: u64, cache: &mut HashMap<u64, Metrics>) -> Metrics {
    if let Some(&metrics) = cache.get(&frs) {
        return metrics;  // Already computed
    }
    
    let record = get_record(frs);
    let mut metrics = Metrics {
        descendants: 0,
        treesize: record.size,
        tree_allocated: record.allocated_size,
    };
    
    if record.is_directory {
        for &child_frs in children.get(&frs) {
            let child_metrics = compute_metrics(child_frs, cache);
            metrics.descendants += 1 + child_metrics.descendants;
            metrics.treesize += child_metrics.treesize;
            metrics.tree_allocated += child_metrics.tree_allocated;
        }
    }
    
    cache.insert(frs, metrics);
    metrics
}

// Step 3: Compute for all records
for record in records {
    let metrics = compute_metrics(record.frs, &mut cache);
    record.descendants = metrics.descendants;
    record.treesize = metrics.treesize;
    record.tree_allocated = metrics.tree_allocated;
}
```

**Performance**: ~50-100 ms for 1M files

---

## Questions

### 1. Is this the optimal algorithm?

- Time complexity: O(n) with memoization
- Space complexity: O(n) for cache + O(n) for children map
- Is there a better approach?

### 2. Can we avoid recursion?

- Current approach uses recursive calls (potential stack overflow for deep trees?)
- Typical directory depth: 5-15 levels, max ~50 levels
- Is iterative bottom-up traversal better?

### 3. Can we parallelize this?

- Records arrive in random order
- Can we process subtrees in parallel?
- How to handle shared cache/memoization?

### 4. Memory layout optimization?

- Currently using `HashMap<u64, Vec<u64>>` for children
- Would a different data structure be more cache-friendly?
- Should we sort children by FRS for better locality?

### 5. Alternative: Topological sort?

- Could we do a topological sort (bottom-up) and process in that order?
- Would this eliminate the need for memoization?
- What's the cost of the sort vs. memoization?

---

## Constraints

1. **Performance target**: < 100 ms for 1M files (currently ~50-100 ms)
2. **Memory target**: < 50 MB overhead (currently ~16-32 MB for cache + children map)
3. **Must handle**:
   - Orphaned files (parent_frs points to non-existent record)
   - Circular references (shouldn't happen in NTFS, but defensive)
   - Very deep trees (up to 50 levels)
4. **Language**: Rust (can use Rayon for parallelism)

---

## Example Data

```
FRS  Parent  IsDir  Size  AllocSize
---  ------  -----  ----  ---------
5    5       true   0     0          // Root directory (parent = self)
100  5       true   0     0          // C:\Users
101  100     true   0     0          // C:\Users\Alice
200  101     false  1024  4096       // C:\Users\Alice\file1.txt
201  101     false  2048  4096       // C:\Users\Alice\file2.txt
102  100     true   0     0          // C:\Users\Bob
202  102     false  512   4096       // C:\Users\Bob\file3.txt
```

Expected output:
```
FRS  Descendants  TreeSize  TreeAllocated
---  -----------  --------  -------------
5    6            3584      16384          // Root: all files
100  5            3584      16384          // Users: all files
101  2            3072      8192           // Alice: 2 files
200  0            1024      4096           // file1.txt
201  0            2048      4096           // file2.txt
102  1            512       4096           // Bob: 1 file
202  0            512       4096           // file3.txt
```

---

## What I'm Looking For

1. **Algorithm review**: Is the current approach optimal, or is there a better way?
2. **Performance optimization**: How to make it faster (parallelization, better data structures)?
3. **Memory optimization**: How to reduce memory overhead?
4. **Edge case handling**: Best practices for orphaned files, circular refs, etc.
5. **Rust-specific tips**: Any Rust idioms or libraries that could help?

Any insights, alternative algorithms, or optimization suggestions would be greatly appreciated!

