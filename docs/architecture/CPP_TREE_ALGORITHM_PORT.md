# C++ Tree Algorithm Port - Implementation Guide

> **Goal**: Implement a 100% faithful port of the C++ tree metrics algorithm as an **alternative** to the current Rust implementation, with a switch to toggle between them.

**Branch**: `feature/cpp-tree-algorithm-port`  
**Status**: Scaffolding complete, placeholder implementation in place  
**Date**: 2026-01-30

---

## 1. Resources

### 1.1 Primary C++ Source

| File | Purpose | Key Lines |
|------|---------|-----------|
| `reference/uffs/UltraFastFileSearch-code/UltraFastFileSearch.cpp` | Main C++ implementation | See sections below |

**Critical C++ Code Sections:**

| Section | Lines | Description |
|---------|-------|-------------|
| `ChildInfo` struct | 3819-3827 | Core structure for parent-child relationships |
| Child entry creation | 4480-4490 | How C++ builds linked list of children during MFT parsing |
| Tree traversal | 4688-4702 | Recursive traversal through child linked list |
| Delta formula | 4729-4743 | `Accumulator::delta` for proportional hardlink shares |
| Stream processing | 4748-4798 | How each stream contributes to treesize |
| Entry point | 4820 | `preprocessor(this->find(0x000000000005), 0, 1)` - starts from root (FRS 5) |

### 1.2 Existing Rust Documentation

| Document | Purpose |
|----------|---------|
| `docs/architecture/MFTINDEX_DEEP_DIVE.md` | Current Rust implementation details |
| `docs/architecture/FIX_IT_PLAN.md` | Task 4.1 shows C++ ChildInfo structure |
| `docs/architecture/tree-metrics-algorithm-question.md` | Algorithm analysis and questions |
| `docs/architecture/Investigation/TREE_METRICS_PARITY_ANALYSIS.md` | Parity analysis results |

### 1.3 Verification Tool

| Tool | Purpose |
|------|---------|
| `docs/architecture/Investigation/trial_run.ps1` | Compares Rust vs C++ output for parity verification |

---

## 2. C++ Algorithm Deep Dive

### 2.1 ChildInfo Structure (Lines 3819-3827)

```cpp
struct ChildInfo {
    ChildInfo() : next_entry(negative_one), record_number(negative_one), name_index(negative_one) {}
    
    typedef small_t<size_t>::type next_entry_type;
    next_entry_type next_entry;           // Next sibling in linked list (or ~0 for end)
    small_t<Records::size_type>::type record_number;  // FRS of the child
    unsigned short name_index;            // Which hardlink (for proportional shares)
};
```

**Key insight**: Each hardlink creates a **separate** `ChildInfo` entry. A file with 3 hardlinks in 3 different directories creates 3 `ChildInfo` entries.

### 2.2 Child Entry Creation (Lines 4480-4490)

```cpp
if (frs_parent != frs_base) {
    Records::iterator const parent = this->at(frs_parent, &base_record);
    size_t const child_index = this->childinfos.size();
    this->childinfos.push_back(empty_child_info);
    ChildInfo* const child_info = &this->childinfos.back();
    child_info->record_number = frs_base;
    child_info->name_index = base_record->name_count;  // BEFORE incrementing name_count
    child_info->next_entry = parent->first_child;
    parent->first_child = static_cast<ChildInfos::value_type::next_entry_type>(child_index);
}
// ... later ...
++base_record->name_count;  // Increment AFTER setting name_index
```

**Critical detail**: `name_index` is set to `name_count` BEFORE incrementing, so first hardlink gets `name_index=0`, second gets `name_index=1`, etc.

### 2.3 Tree Traversal Algorithm (Lines 4670-4812)

The C++ algorithm is a **recursive depth-first traversal** starting from root (FRS 5):

```cpp
PreprocessResult operator()(Records::value_type* const fr, 
                           key_type::name_info_type const name_info, 
                           unsigned short const total_names) {
    PreprocessResult result;
    PreprocessResult children_size;
    
    // 1. Recursively process all children
    for (ChildInfo* i = me->childinfo(fr); i && ~i->record_number; i = me->childinfo(i->next_entry)) {
        Records::value_type* const fr2 = me->find(i->record_number);
        if (fr2 != fr) {  // Skip root self-reference
            PreprocessResult const subresult = this->operator()(
                fr2, 
                fr2->name_count - 1 - i->name_index,  // name_info calculation
                fr2->name_count                        // total_names
            );
            children_size.length += subresult.length;
            children_size.allocated += subresult.allocated;
            children_size.treesize += subresult.treesize;
        }
    }
    
    // 2. Process own streams with delta formula
    result = children_size;
    for (StreamInfo* k = me->streaminfo(fr); k; k = me->streaminfo(k->next_entry)) {
        unsigned long long const length_delta = Accumulator::delta(k->length, name_info, total_names);
        unsigned long long const allocated_delta = Accumulator::delta(k->allocated, name_info, total_names);
        
        result.length += length_delta;
        result.allocated += allocated_delta;
        result.treesize += 1;  // Each stream adds 1 to treesize
        
        // Default stream (unnamed $DATA) accumulates children's metrics
        if (!k->type_name_id) {
            k->length += children_size.length;
            k->allocated += children_size.allocated;
            k->treesize += children_size.treesize;
        }
    }
    
    return result;
}
```

### 2.4 Delta Formula (Lines 4729-4743)

```cpp
static unsigned long long delta_impl(unsigned long long const value, 
                                     unsigned short const i, 
                                     unsigned short const n) {
    return value * (i + 1) / n - value * i / n;
}
```

This ensures **no rounding errors** when dividing a file's size among multiple hardlinks.

---

## 3. Key Differences: C++ vs Current Rust

| Aspect | C++ Algorithm | Current Rust Algorithm |
|--------|---------------|------------------------|
| **Traversal** | Recursive DFS from root | Bottom-up leaf-peeling (Kahn-style) |
| **Data Structure** | Linked list (`first_child` → `next_entry`) | `HashMap<u64, Vec<u64>>` |
| **Entry Point** | `preprocessor(find(0x5), 0, 1)` | Iterate all records, process leaves first |
| **Memoization** | None (single pass, recursive) | Implicit via topological order |
| **Stack Usage** | Recursive call stack (depth = tree depth) | Explicit stack in loop |
| **Stream Counting** | Each stream adds +1 to treesize | Same |
| **Hardlink Handling** | Separate ChildInfo per hardlink | Same |

---

## 4. Implementation Plan

### Phase 1: Study C++ Code (2-3 hours) ✅ COMPLETE

- [x] Document `ChildInfo` structure
- [x] Document child entry creation during parsing
- [x] Document tree traversal algorithm
- [x] Document delta formula
- [x] Document stream processing

### Phase 2: Design Rust Structures (1-2 hours)

Create Rust structures that **exactly match** C++ layout:

```rust
/// C++ ChildInfo equivalent
#[derive(Debug, Clone, Copy)]
pub struct CppChildInfo {
    pub next_entry: u32,      // ~0 = end of list
    pub record_number: u32,   // FRS of child
    pub name_index: u16,      // Which hardlink
}

/// C++ Record fields needed for tree metrics
pub struct CppRecordView<'a> {
    pub first_child: u32,     // Index into childinfos
    pub name_count: u16,      // Number of hardlinks
    pub streams: &'a [StreamInfo],
}
```

### Phase 3: Build Child Entries During Parsing (2-3 hours)

Modify `MftIndex::from_parsed_records()` to build `childinfos` vector:

```rust
// For each $FILE_NAME attribute (hardlink):
let child_index = self.childinfos.len() as u32;
self.childinfos.push(CppChildInfo {
    next_entry: parent_record.first_child,
    record_number: frs as u32,
    name_index: record.name_count,  // BEFORE incrementing
});
parent_record.first_child = child_index;
record.name_count += 1;  // Increment AFTER
```

### Phase 4: Implement Tree Metrics Computation (3-4 hours)

Implement `compute_tree_metrics_cpp_port()` as recursive DFS:

```rust
fn compute_tree_metrics_cpp_port(&mut self, debug: bool) {
    // Start from root (FRS 5)
    let root_idx = self.frs_to_idx[5];
    self.preprocess_recursive(root_idx, 0, 1);
}

fn preprocess_recursive(&mut self, idx: usize, name_info: u16, total_names: u16) -> SizeInfo {
    let mut children_size = SizeInfo::default();

    // Traverse child linked list
    let mut child_entry = self.records[idx].first_child;
    while child_entry != NO_ENTRY {
        let child_info = &self.childinfos[child_entry as usize];
        let child_idx = self.frs_to_idx[child_info.record_number as usize];

        if child_idx != idx {  // Skip root self-reference
            let child_name_count = self.records[child_idx].name_count;
            let child_name_info = child_name_count - 1 - child_info.name_index;

            let subresult = self.preprocess_recursive(child_idx, child_name_info, child_name_count);
            children_size += subresult;
        }

        child_entry = child_info.next_entry;
    }

    // Process own streams with delta formula
    let mut result = children_size;
    // ... stream processing with Accumulator::delta ...

    result
}
```

### Phase 5: Testing Against C++ Output (2-3 hours)

1. Run `trial_run.ps1` to compare Rust vs C++ output
2. Verify 100% match on:
   - `descendants` count for root directory
   - `treesize` for root directory
3. If mismatch, add debug logging to trace differences

### Phase 6: Integration and Cleanup (1 hour)

1. Wire up `TreeAlgorithm::CppPort` to use new implementation
2. Remove placeholder warning message
3. Add unit tests
4. Update documentation

---

## 5. Success Criteria

| Metric | Target |
|--------|--------|
| Root descendants | 100% match with C++ |
| Root treesize | 100% match with C++ |
| All directory descendants | 100% match with C++ |
| All directory treesize | 100% match with C++ |

**Verification command:**
```powershell
cd docs/architecture/Investigation
.\trial_run.ps1 -Drives G
# Check: RustDescendants == CppDescendants AND RustTreesize == CppTreesize
```

---

## 6. Current State

### Scaffolding Complete

The switch mechanism is in place in `crates/uffs-mft/src/index.rs`:

```rust
pub enum TreeAlgorithm {
    Current,   // Existing Rust leaf-peeling algorithm
    CppPort,   // C++ port (placeholder - zeros metrics)
}

impl MftIndex {
    pub fn compute_tree_metrics_with_algo(&mut self, algo: TreeAlgorithm, debug: bool) {
        match algo {
            TreeAlgorithm::Current => self.compute_tree_metrics(debug),
            TreeAlgorithm::CppPort => self.compute_tree_metrics_cpp_port(debug),
        }
    }
}
```

**Usage:**
```bash
UFFS_TREE_ALGO=current uffs index    # Use current algorithm (default)
UFFS_TREE_ALGO=cpp_port uffs index   # Use C++ port (placeholder)
```

---

## 7. Rollback Plan

If the C++ port causes issues:
1. The original algorithm remains untouched
2. Simply use `TreeAlgorithm::Current` (default)
3. All changes are isolated to the `feature/cpp-tree-algorithm-port` branch

---

## 8. Open Questions

1. **Stack depth**: C++ uses recursive calls. For very deep trees (50+ levels), should we use an explicit stack in Rust?
2. **Bulkiness calculation**: C++ has complex bulkiness logic with heap operations. Do we need this for parity?
3. **WofCompressedData handling**: C++ has special handling for Windows Overlay Filter compressed files. Is this needed?
4. **Reserved clusters**: C++ adds `reserved_clusters * cluster_size` at depth 0. Do we have this data?

---

## 9. Estimated Effort

| Phase | Effort |
|-------|--------|
| Phase 1: Study C++ code | ✅ Complete |
| Phase 2: Design Rust structures | 1-2 hours |
| Phase 3: Build child entries | 2-3 hours |
| Phase 4: Implement tree metrics | 3-4 hours |
| Phase 5: Testing | 2-3 hours |
| Phase 6: Integration | 1 hour |
| **Total** | **~10-13 hours** |

