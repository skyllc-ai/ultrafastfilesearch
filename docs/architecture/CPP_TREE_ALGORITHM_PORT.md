# C++ Tree Algorithm Port - Implementation Guide

> **Goal**: Implement a 100% faithful port of the C++ tree metrics algorithm as an **alternative** to the current Rust implementation, with a switch to toggle between them.

**Branch**: `feature/cpp-tree-algorithm-port`  
**Status**: Scaffolding complete, placeholder implementation in place  
**Date**: 2026-01-30

---

## 1. Resources

> **IMPORTANT**: This implementation is based EXCLUSIVELY on C++ source code and C++ documentation.
> All resources are located in `docs/architecture/C++_resources/`.

### 1.1 Primary C++ Source

| File | Purpose | Key Lines |
|------|---------|-----------|
| `docs/architecture/C++_resources/UltraFastFileSearch-code/UltraFastFileSearch.cpp` | Main C++ implementation (~13,400 lines) | See sections below |

**Critical C++ Code Sections:**

| Section | Lines | Description |
|---------|-------|-------------|
| `ChildInfo` struct | 3819-3827 | Core structure for parent-child relationships |
| Child entry creation | 4480-4490 | How C++ builds linked list of children during MFT parsing |
| Tree traversal | 4688-4702 | Recursive traversal through child linked list |
| Delta formula | 4729-4743 | `Accumulator::delta` for proportional hardlink shares |
| Stream processing | 4748-4798 | How each stream contributes to treesize |
| Entry point | 4820 | `preprocessor(this->find(0x000000000005), 0, 1)` - starts from root (FRS 5) |

### 1.2 C++ Architecture Documentation

| Document | Purpose |
|----------|---------|
| `docs/architecture/C++_resources/docs/architecture/01-overview.md` | System architecture, NtfsIndex class, data flow |
| `docs/architecture/C++_resources/docs/architecture/04-mft-parsing.md` | MFT record parsing, attribute handling, in-memory structures |
| `docs/architecture/C++_resources/docs/architecture/07-indexing.md` | **KEY**: ChildInfo, Record, StreamInfo, LinkInfo structures |

### 1.3 Key Structures from C++ Documentation (07-indexing.md)

**ChildInfo** (lines 177-191):
```cpp
struct ChildInfo {
    typedef small_t<size_t>::type next_entry_type;
    next_entry_type next_entry;                    // Next child in linked list
    small_t<Records::size_type>::type record_number;  // FRS of child
    unsigned short name_index;                     // Which name (for hard links)
};
```

**Record** (lines 56-75):
```cpp
struct Record {
    StandardInfo stdinfo;                    // Timestamps and attributes
    unsigned short name_count;               // Number of hard links (≤1024)
    unsigned short stream_count;             // Number of data streams (≤4106)
    ChildInfo::next_entry_type first_child;  // Index of first child (directories)
    LinkInfo first_name;                     // First/primary filename
    StreamInfo first_stream;                 // First/primary data stream
};
```

**StreamInfo/SizeInfo** (lines 152-175):
```cpp
struct SizeInfo {
    file_size_type length;     // Logical file size
    file_size_type allocated;  // Allocated size on disk
    file_size_type bulkiness;  // Size including slack space
    unsigned int treesize;     // For directories: descendant count
};

struct StreamInfo : SizeInfo {
    next_entry_type next_entry;  // Index of next StreamInfo
    NameInfo name;               // Stream name (empty for default $DATA)
    unsigned char type_name_id : 6;  // Attribute type identifier
};
```

### 1.4 Verification Tool

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

## 3. C++ Algorithm Characteristics

| Aspect | C++ Implementation |
|--------|-------------------|
| **Traversal** | Recursive DFS from root (FRS 5) |
| **Data Structure** | Linked list (`first_child` → `next_entry` chain) |
| **Entry Point** | `preprocessor(find(0x5), 0, 1)` |
| **Processing Order** | Top-down: parent before children |
| **Stack Usage** | Recursive call stack (depth = tree depth) |
| **Stream Counting** | Each stream adds +1 to treesize |
| **Hardlink Handling** | Separate ChildInfo per hardlink, delta formula for proportional shares |

---

## 4. Data Structure Comparison: C++ vs Rust

> **Analysis Date**: 2026-01-30
> **Conclusion**: ✅ All required data is available. The C++ algorithm can be a drop-in replacement.

### 4.1 Structure Mapping Table

| C++ Structure | C++ Field | Rust Structure | Rust Field | Status |
|---------------|-----------|----------------|------------|--------|
| **ChildInfo** | `next_entry` (u32) | `ChildInfo` | `next_entry` (u32) | ✅ Identical |
| **ChildInfo** | `record_number` (u32) | `ChildInfo` | `child_frs` (u64) | ✅ Rust wider for 48-bit FRS |
| **ChildInfo** | `name_index` (u16) | `ChildInfo` | `name_index` (u16) | ✅ Identical |
| **SizeInfo** | `length` (6-byte packed) | `SizeInfo` | `length` (u64) | ✅ Rust wider |
| **SizeInfo** | `allocated` (6-byte packed) | `SizeInfo` | `allocated` (u64) | ✅ Rust wider |
| **SizeInfo** | `bulkiness` (6-byte packed) | - | - | ⚠️ Not in Rust (not needed for tree metrics) |
| **SizeInfo** | `treesize` (u32) | `FileRecord` | `treesize` (u64) | ⚠️ Different location, Rust wider |
| **StreamInfo** | inherits `SizeInfo` | `IndexStreamInfo` | `size: SizeInfo` | ✅ Composition vs inheritance |
| **StreamInfo** | `next_entry` (u32) | `IndexStreamInfo` | `next_entry` (u32) | ✅ Identical |
| **StreamInfo** | `name` | `IndexStreamInfo` | `name: IndexNameRef` | ✅ Similar |
| **StreamInfo** | `is_sparse` (1 bit) | `IndexStreamInfo` | `flags` bit 0 | ✅ Packed differently |
| **StreamInfo** | `type_name_id` (6 bits) | `IndexStreamInfo` | `flags` bits 2-7 | ✅ Packed differently |
| **LinkInfo** | `next_entry` (u32) | `LinkInfo` | `next_entry` (u32) | ✅ Identical |
| **LinkInfo** | `name` | `LinkInfo` | `name: IndexNameRef` | ✅ Similar |
| **LinkInfo** | `parent` (u32) | `LinkInfo` | `parent_frs` (u64) | ✅ Rust wider for 48-bit FRS |
| **Record** | `stdinfo` | `FileRecord` | `stdinfo` | ✅ Similar |
| **Record** | `name_count` (u16) | `FileRecord` | `name_count` (u16) | ✅ Identical |
| **Record** | `stream_count` (u16) | `FileRecord` | `stream_count` (u16) | ✅ Identical |
| **Record** | `first_child` (u32) | `FileRecord` | `first_child` (u32) | ✅ Identical |
| **Record** | `first_name` | `FileRecord` | `first_name` | ✅ Similar |
| **Record** | `first_stream` | `FileRecord` | `first_stream` | ✅ Similar |

### 4.2 Extra Fields in Rust (Not in C++)

| Rust Structure | Rust Field | Purpose |
|----------------|------------|---------|
| `FileRecord` | `frs` (u64) | FRS stored in record (C++ uses lookup table) |
| `FileRecord` | `sequence_number` (u16) | Forensic: MFT sequence number |
| `FileRecord` | `namespace` (u8) | Forensic: filename namespace |
| `FileRecord` | `forensic_flags` (u8) | Forensic: deleted/corrupt/extension flags |
| `FileRecord` | `lsn` (u64) | Forensic: Log File Sequence Number |
| `FileRecord` | `reparse_tag` (u32) | Forensic: reparse point type |
| `FileRecord` | `base_frs` (u64) | Forensic: base record for extensions |
| `FileRecord` | `fn_created/modified/accessed/mft_changed` (i64) | $FILE_NAME timestamps |
| `FileRecord` | `descendants` (u32) | Tree metric: count of all descendants |
| `FileRecord` | `tree_allocated` (u64) | Tree metric: sum of allocated sizes (C++ doesn't have this) |

### 4.3 Key Differences

1. **FRS Width**: Rust uses `u64` for FRS values (C++ uses `u32`). This supports 48-bit NTFS FRS values on very large volumes.

2. **Tree Metrics Location**:
   - C++ stores `treesize` in `SizeInfo` (per-stream)
   - Rust stores `descendants`, `treesize`, `tree_allocated` in `FileRecord` (per-record)
   - This is a design difference but doesn't affect the algorithm

3. **Bulkiness**: C++ has `bulkiness` field for slack space calculation. Rust doesn't have this, but it's not needed for tree metrics.

4. **Extra Forensic Fields**: Rust has many forensic fields not in C++. These don't affect tree metrics.

### 4.4 What the C++ Algorithm Needs

| Requirement | C++ Source | Rust Equivalent | Available? |
|-------------|------------|-----------------|------------|
| Directory traversal | `first_child` → `childinfos[]` | `first_child` → `children[]` | ✅ Yes |
| Child FRS lookup | `ChildInfo.record_number` | `ChildInfo.child_frs` | ✅ Yes |
| Hardlink name_index | `ChildInfo.name_index` | `ChildInfo.name_index` | ✅ Yes |
| Name count for delta | `Record.name_count` | `FileRecord.name_count` | ✅ Yes |
| Stream count | `Record.stream_count` | `FileRecord.stream_count` | ✅ Yes |
| Stream sizes | `StreamInfo.length/allocated` | `IndexStreamInfo.size.length/allocated` | ✅ Yes |
| Stream type_name_id | `StreamInfo.type_name_id` | `IndexStreamInfo.type_name_id()` | ✅ Yes |
| Output: descendants | - | `FileRecord.descendants` | ✅ Yes |
| Output: treesize | `SizeInfo.treesize` | `FileRecord.treesize` | ✅ Yes |
| Output: tree_allocated | - | `FileRecord.tree_allocated` | ✅ Yes (bonus) |

### 4.5 Conclusion

**The C++ algorithm can be implemented as a drop-in replacement.** All required data is available in the Rust structures:

- ✅ `ChildInfo` linked list for directory traversal
- ✅ `name_index` for hardlink proportional share calculation
- ✅ `name_count` for delta formula
- ✅ Stream sizes and type information
- ✅ Output fields for tree metrics

The main differences (wider FRS types, extra forensic fields) are **additive** and don't prevent the C++ algorithm from working.

---

## 5. Implementation Plan

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

## 6. Success Criteria

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

## 7. Current State

### Scaffolding Complete

The switch mechanism is in place in `crates/uffs-mft/src/index.rs`:

```rust
pub enum TreeAlgorithm {
    Current,   // Current algorithm (default)
    CppPort,   // C++ port - 100% faithful port of C++ tree algorithm
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
UFFS_TREE_ALGO=cpp_port uffs index   # Use C++ port algorithm
```

---

## 8. Rollback Plan

If the C++ port causes issues:
1. The original algorithm remains untouched
2. Simply use `TreeAlgorithm::Current` (default)
3. All changes are isolated to the `feature/cpp-tree-algorithm-port` branch

---

## 9. Open Questions

1. **Stack depth**: C++ uses recursive calls. For very deep trees (50+ levels), should we use an explicit stack in Rust?
2. **Bulkiness calculation**: C++ has complex bulkiness logic with heap operations. Do we need this for parity?
3. **WofCompressedData handling**: C++ has special handling for Windows Overlay Filter compressed files. Is this needed?
4. **Reserved clusters**: C++ adds `reserved_clusters * cluster_size` at depth 0. Do we have this data?

---

## 10. Estimated Effort

| Phase | Effort |
|-------|--------|
| Phase 1: Study C++ code | ✅ Complete |
| Phase 2: Design Rust structures | 1-2 hours |
| Phase 3: Build child entries | 2-3 hours |
| Phase 4: Implement tree metrics | 3-4 hours |
| Phase 5: Testing | 2-3 hours |
| Phase 6: Integration | 1 hour |
| **Total** | **~10-13 hours** |

