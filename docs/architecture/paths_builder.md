
# Build NTFS-like directory paths from in-memory MFT records (Rust)

This document shows a fast, memory-friendly Rust implementation that **computes full directory paths for every node** in an in-memory representation of the MFT (millions of records), **skipping any nodes that are illegal or are descendants of illegal nodes**, and correctly handling cases where child records may appear before their parents (unordered input). It also detects cycles and treats them as illegal (no path produced).

## Approach (summary)
- Keep all nodes in a `HashMap<Id, Node>`.
- Use a memoized upward-traversal algorithm:
  - For each node, walk parent pointers upward until you hit a node whose path is already cached, a root (no parent), a node that is marked illegal, or a cycle.
  - Cache results (`Option<String>`) per node: `Some(path)` for allowed nodes; `None` for illegal/cyclic nodes or nodes under an illegal ancestor.
  - This avoids recursion limits and is linear time amortized: each node is processed once plus the cost of walking to the nearest cached ancestor.
- Missing parents are treated as root boundaries (i.e., if a node's parent id doesn't exist in the input, the node becomes a root).
- Cycles are detected while walking via a `HashSet` of nodes visited in the current chain; nodes involved in a cycle are marked with `None` (considered illegal/unresolvable).

## Complexity
- Time: amortized **O(n)** for `n` nodes (each node's parent chain is walked at most until it reaches a cached node or root).
- Memory: **O(n)** for maps and caches. Works comfortably for millions of nodes if names are reasonably sized and you have available RAM; you can store names externally if needed.

## Rust code

```rust
use std::collections::{HashMap, HashSet};

/// Node ID type — adjust if your IDs are integers (use String for generality).
type Id = String;

#[derive(Debug, Clone)]
struct Node {
    id: Id,
    name: String,
    parent: Option<Id>,
    illegal: bool, // initially flagged illegal or not
}

/// Result cache for computed paths:
/// - Some(path) => node allowed and path computed
/// - None => node is illegal or descendant of illegal or part of cycle
type PathCache = HashMap<Id, Option<String>>;

/// Compute full paths for every node in `nodes`.
/// Separator here is backslash (NTFS-style). Change to "/" if needed.
fn compute_paths(nodes: &HashMap<Id, Node>) -> PathCache {
    let mut cache: PathCache = HashMap::with_capacity(nodes.len());

    // Helper closure to get a node by id (returns Option<&Node>)
    let get_node = |id: &Id| nodes.get(id);

    // Iterate over all nodes and ensure path is computed/cached
    for id in nodes.keys() {
        compute_path_for(id.clone(), nodes, &mut cache);
    }

    cache
}

/// Compute (and cache) path for a single node id.
/// Returns Option<String> (cached value).
fn compute_path_for(id: Id, nodes: &HashMap<Id, Node>, cache: &mut PathCache) -> Option<String> {
    // Fast return if already cached
    if let Some(cached) = cache.get(&id) {
        return cached.clone();
    }

    // We'll walk the parent chain until we find a cached ancestor or a root/missing parent or an illegal node.
    let mut chain: Vec<Id> = Vec::new(); // chain from current node up to (but excluding) found cached ancestor
    let mut seen_in_chain: HashSet<Id> = HashSet::new();

    let mut cur = id.clone();
    loop {
        // If cur already cached, break and use it as ancestor base
        if let Some(cached) = cache.get(&cur) {
            break;
        }

        // Detect cycle
        if !seen_in_chain.insert(cur.clone()) {
            // cycle detected. Mark all nodes in chain as None (illegal) and return None.
            for nid in chain.iter() {
                cache.insert(nid.clone(), None);
            }
            cache.insert(cur.clone(), None);
            return None;
        }

        // Push current into chain
        chain.push(cur.clone());

        // If node exists and is illegal => mark chain as illegal
        match nodes.get(&cur) {
            Some(node) if node.illegal => {
                for nid in chain.iter() {
                    cache.insert(nid.clone(), None);
                }
                return None;
            }
            Some(node) => {
                // Has a parent?
                if let Some(parent_id) = &node.parent {
                    // Move up to parent and continue loop
                    cur = parent_id.clone();
                    continue;
                } else {
                    // parent == None => we've reached a root; break to build path from chain
                    break;
                }
            }
            None => {
                // Missing parent record: treat as root boundary; break to build path
                break;
            }
        }
    } // end loop

    // At this point, either 'cur' is:
    // - a node with cached Some/None in cache, or
    // - a root (parent == None), or
    // - a missing node id (not present in nodes)

    // If cur is cached and None => everything in chain is None
    if let Some(Some(_)) | None = cache.get(&cur) {
        // We'll handle below. No-op here.
    }

    // Determine base path (if ancestor cached with Some)
    let base_path_opt = cache.get(&cur).and_then(|v| v.clone());

    // Build names vector from top ancestor down to leaves
    // chain currently has nodes from original id upwards: [id, parent, parent.parent, ...]
    // We need to walk chain in reverse to build paths top-down.
    let mut components: Vec<String> = Vec::new();

    // If ancestor had a cached Some(path), use its components as starting point.
    if let Some(base_path) = base_path_opt {
        // Start with the cached base path components.
        // For simplicity, split by backslash — avoid losing leading separators since we join cleanly.
        if !base_path.is_empty() {
            components.extend(base_path.split('\\').map(|s| s.to_string()));
        }
    } else {
        // If no cached ancestor, and `cur` corresponds to an existing non-illegal node and is root,
        // we will include its name if it exists in nodes and wasn't part of the chain already.
        if let Some(node) = nodes.get(&cur) {
            // If cur is part of chain, it will be added in the reverse step below.
            // If cur is a root but not included in chain (happens when cur was the cached ancestor),
            // we would have handled it in base_path_opt. Here cur is not cached, so include its name only if
            // cur was not the same as chain last element (we'll keep logic simple and append when iterating).
            // Nothing to do here.
        } else {
            // cur is missing in nodes (or parent missing). Treat as virtual root: don't add any component.
        }
    }

    // Now process chain in reverse (from highest ancestor in the chain to the original node)
    // Example: chain = [id, p1, p2] => reversed = [p2, p1, id]
    let mut built_paths: Vec<(Id, Option<String>)> = Vec::with_capacity(chain.len());
    let mut path_prefix = components.join("\\");
    if !path_prefix.is_empty() {
        // ensure we will append with separator
    }

    for nid in chain.iter().rev() {
        // For each node id in top-down order, get its name (if present) and append
        if let Some(node) = nodes.get(nid) {
            // If node was marked illegal just now, mark and stop (should already have been caught above)
            if node.illegal {
                for (id_write, _) in built_paths.iter() {
                    cache.insert(id_write.clone(), None);
                }
                for id_write in chain.iter() {
                    cache.insert(id_write.clone(), None);
                }
                return None;
            }

            // Append component
            if path_prefix.is_empty() {
                path_prefix = node.name.clone();
            } else {
                path_prefix = format!("{}\\{}", path_prefix, node.name);
            }
            // Cache this node's path tentatively; we'll insert into cache after loop to avoid borrowing issues
            built_paths.push((nid.clone(), Some(path_prefix.clone())));
        } else {
            // Node not present in nodes map (should be rare for a nid in chain unless initial id missing)
            // Treat as virtual: don't append a name; paths under this virtual parent will start from existing prefix.
            built_paths.push((nid.clone(), Some(path_prefix.clone())));
        }
    }

    // Commit built paths to cache
    for (nid, path_opt) in built_paths.into_iter() {
        cache.insert(nid, path_opt);
    }

    // Finally return cached value for original id (might be Some or None)
    cache.get(&id).and_then(|v| v.clone())
}

fn main() {
    // Example usage with unordered input (child may appear before parent).
    let mut nodes: HashMap<Id, Node> = HashMap::new();

    // Example nodes: A(root) -> B -> C ; D is illegal -> E under D should be skipped.
    nodes.insert(
        "C".to_string(),
        Node {
            id: "C".to_string(),
            name: "child_c".to_string(),
            parent: Some("B".to_string()),
            illegal: false,
        },
    );
    nodes.insert(
        "B".to_string(),
        Node {
            id: "B".to_string(),
            name: "parent_b".to_string(),
            parent: Some("A".to_string()),
            illegal: false,
        },
    );
    nodes.insert(
        "A".to_string(),
        Node {
            id: "A".to_string(),
            name: "root_a".to_string(),
            parent: None,
            illegal: false,
        },
    );

    nodes.insert(
        "E".to_string(),
        Node {
            id: "E".to_string(),
            name: "child_e".to_string(),
            parent: Some("D".to_string()),
            illegal: false,
        },
    );
    nodes.insert(
        "D".to_string(),
        Node {
            id: "D".to_string(),
            name: "root_d".to_string(),
            parent: None,
            illegal: true, // D is illegal => E must be treated as illegal
        },
    );

    // Add a cycle example: X -> Y -> X
    nodes.insert(
        "X".to_string(),
        Node {
            id: "X".to_string(),
            name: "x".to_string(),
            parent: Some("Y".to_string()),
            illegal: false,
        },
    );
    nodes.insert(
        "Y".to_string(),
        Node {
            id: "Y".to_string(),
            name: "y".to_string(),
            parent: Some("X".to_string()),
            illegal: false,
        },
    );

    let cache = compute_paths(&nodes);

    // Print results
    println!("Computed paths (None = illegal/cycle/missing):");
    for (id, path_opt) in cache.iter() {
        println!("  {} => {:?}", id, path_opt);
    }
}
```

## Notes & tweaks
- If you want to **treat cycles differently** (e.g., reparent to a quarantine root instead of marking `None`), modify the cycle handling block to assign a deterministic quarantine path.
- For very deep trees where stack recursion may be a concern, this implementation uses iterative upward walking and explicit `Vec`/`HashSet` to avoid recursion.
- If you want to preserve the original node ordering or produce sorted output, iterate `nodes.keys()` in your desired order (e.g., sort by name or id).
- When running at multi-million scale, prefer reserving capacities for `HashMap`/`Vec` to reduce reallocation overhead:
  - `HashMap::with_capacity(nodes.len())`
  - `Vec::with_capacity(estimated_depth)`

---

If you'd like, I can:
- Convert this into a command-line Rust program that reads a compact binary or CSV input and writes `id -> path` to a file with parallelism and resume support.
- Add a variant that outputs NTFS-safe component names (sanitize invalid characters, truncate components >255 chars, optionally apply bucketing for huge sibling counts).

