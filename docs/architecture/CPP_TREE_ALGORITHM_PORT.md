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

### 4.1 Critical Structure Differences

> **⚠️ WARNING**: The current Rust structures are **fundamentally different** from C++.
> All previous Rust tree algorithm attempts are wrong. This section documents the exact differences.

#### C++ `SizeInfo` (22 bytes, packed):
```cpp
#pragma pack(push, 1)
struct SizeInfo {
    file_size_type length;     // 6 bytes (packed 48-bit)
    file_size_type allocated;  // 6 bytes (packed 48-bit)
    file_size_type bulkiness;  // 6 bytes (packed 48-bit) ← USED BY TREE ALGO
    unsigned int treesize;     // 4 bytes ← STORED PER-STREAM
};
#pragma pack(pop)
```

#### Current Rust `SizeInfo` (16 bytes) - WRONG:
```rust
pub struct SizeInfo {
    pub length: u64,     // 8 bytes (not packed)
    pub allocated: u64,  // 8 bytes (not packed)
    // MISSING: bulkiness ← NEEDED FOR TREE ALGO
    // MISSING: treesize  ← WRONG LOCATION (in FileRecord instead)
}
```

### 4.2 What is `bulkiness`?

`bulkiness` represents file size **including slack space** (wasted space at end of last cluster).

Example for a 5KB file on 4KB cluster NTFS:
- `length` = 5,120 bytes (actual data)
- `allocated` = 8,192 bytes (2 clusters × 4KB)
- `bulkiness` = 8,192 bytes (includes ~3KB slack)

**The C++ tree algorithm USES `bulkiness`** (see `ntfs_index.hpp` lines 787-810):
1. Collects each child's `bulkiness` into a scratch heap
2. Filters out large files (>1% of folder's allocated size)
3. Small files contribute more "slack waste" proportionally

This is a **heuristic for accurate disk space accounting**.

### 4.3 Structure Collision Problem

| Issue | Description |
|-------|-------------|
| **Wrong `SizeInfo`** | Missing `bulkiness` and `treesize` fields |
| **Wrong tree metrics location** | Rust stores in `FileRecord`, C++ stores in `SizeInfo` (per-stream) |
| **Wrong packing** | Rust uses `u64` (8 bytes), C++ uses packed 6-byte `file_size_type` |
| **Existing wrong algorithm** | Current Rust tree code uses these wrong structures |

### 4.4 Isolation Strategy

To avoid collision with existing (wrong) code:

1. **Create new module**: `crates/uffs-mft/src/cpp_tree.rs`
2. **Define C++ port structures** that exactly match C++ layout
3. **Don't modify existing structures** until C++ port is verified
4. **Switch via `TreeAlgorithm` enum** - existing code untouched

#### New C++ Port Structures (to be created):

```rust
// crates/uffs-mft/src/cpp_tree.rs

/// C++ file_size_type equivalent (6 bytes packed)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct PackedFileSize {
    low: u32,   // 4 bytes
    high: u16,  // 2 bytes
}

/// C++ SizeInfo equivalent (22 bytes)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct CppSizeInfo {
    pub length: PackedFileSize,     // 6 bytes
    pub allocated: PackedFileSize,  // 6 bytes
    pub bulkiness: PackedFileSize,  // 6 bytes ← INCLUDED
    pub treesize: u32,              // 4 bytes ← PER-STREAM
}

/// C++ ChildInfo equivalent
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct CppChildInfo {
    pub next_entry: u32,      // ~0 = end of list
    pub record_number: u32,   // FRS of child (C++ uses u32)
    pub name_index: u16,      // Which hardlink
}

/// Result of preprocessing a subtree (matches C++ PreprocessResult)
#[derive(Debug, Clone, Copy, Default)]
pub struct PreprocessResult {
    pub length: u64,
    pub allocated: u64,
    pub bulkiness: u64,
    pub treesize: u32,
}
```

### 4.5 Full Structure Mapping

| C++ Structure | C++ Field | C++ Size | Rust Port | Notes |
|---------------|-----------|----------|-----------|-------|
| `file_size_type` | low + high | 6 bytes | `PackedFileSize` | Packed 48-bit |
| `SizeInfo` | length | 6 bytes | `CppSizeInfo.length` | ✅ |
| `SizeInfo` | allocated | 6 bytes | `CppSizeInfo.allocated` | ✅ |
| `SizeInfo` | bulkiness | 6 bytes | `CppSizeInfo.bulkiness` | ✅ NOW INCLUDED |
| `SizeInfo` | treesize | 4 bytes | `CppSizeInfo.treesize` | ✅ PER-STREAM |
| `ChildInfo` | next_entry | 4 bytes | `CppChildInfo.next_entry` | ✅ |
| `ChildInfo` | record_number | 4 bytes | `CppChildInfo.record_number` | ✅ u32 like C++ |
| `ChildInfo` | name_index | 2 bytes | `CppChildInfo.name_index` | ✅ |

### 4.6 What the C++ Algorithm Needs

| Requirement | C++ Source | Available in Rust? | Action |
|-------------|------------|-------------------|--------|
| `bulkiness` field | `SizeInfo.bulkiness` | ❌ Missing | Add to `CppSizeInfo` |
| `treesize` per-stream | `SizeInfo.treesize` | ❌ Wrong location | Add to `CppSizeInfo` |
| Heap for bulkiness filter | `scratch` vector | ❌ Not implemented | Implement in C++ port |
| `record_number` as u32 | `ChildInfo.record_number` | ⚠️ Rust uses u64 | Use `CppChildInfo` with u32 |
| Delta formula | `Accumulator::delta()` | ❌ Not implemented | Implement exactly |
| Reserved clusters | `reserved_clusters * cluster_size` | ❓ Need to check | May need to add |

### 4.7 Conclusion

**The current Rust structures CANNOT be used for a faithful C++ port.**

We must:
1. ✅ Create new `cpp_tree.rs` module with exact C++ structures
2. ✅ Include `bulkiness` field (it IS used by the algorithm)
3. ✅ Store `treesize` per-stream in `CppSizeInfo`, not per-record
4. ✅ Implement the heap-based bulkiness filtering algorithm
5. ✅ Keep existing code isolated until C++ port is verified working

---

## 5. Data Flow: Transformer vs Parsing Modification

### 5.1 The Question

Do we need to modify MFT parsing to match C++ data structures, or can we use a transformer?

### 5.2 How C++ Computes `bulkiness`

In C++ (`ntfs_index.hpp` lines 709-718), `bulkiness` is computed DURING MFT parsing:

```cpp
// For each $DATA attribute run encountered:
info->allocated += ah->IsNonResident ? ... : 0;
info->length += ah->IsNonResident ? ... : ah->Resident.ValueLength;
info->bulkiness += info->allocated;  // ← CUMULATIVE!
info->treesize = isdir;
```

**Key insight**: `bulkiness += allocated` runs for EACH attribute run. For fragmented files:
- Run 1: allocated=100, bulkiness=100
- Run 2: allocated=200, bulkiness=100+200=300
- Run 3: allocated=300, bulkiness=300+300=600
- Final: allocated=300, bulkiness=600 (penalizes fragmentation)

For single-extent files (vast majority): `bulkiness = allocated`

### 5.3 How C++ Uses `treesize`

In C++ (`ntfs_index.hpp` line 879):
```cpp
result.treesize += 1;  // For EACH stream in the record
```

`treesize` counts **streams** in the subtree, not files. Each ADS adds +1.

### 5.4 Two Approaches

#### Option A: Transformer Approach (Recommended to start)

**No changes to MFT parsing.** Create a transformer that:

| Step | Action |
|------|--------|
| 1 | Read from existing `MftIndex` structures |
| 2 | Build `CppSizeInfo { length, allocated, bulkiness: allocated, treesize: is_dir ? 1 : 0 }` |
| 3 | Build `CppChildInfo { next_entry, record_number: child_frs as u32, name_index }` |
| 4 | Run C++ tree algorithm |
| 5 | Write results back to `FileRecord.descendants`, `FileRecord.treesize`, `FileRecord.tree_allocated` |

**Pros**:
- ✅ Isolated - no risk to existing code
- ✅ Easy to verify against C++ output
- ✅ Can switch between algorithms via `TreeAlgorithm` enum

**Cons**:
- ⚠️ `bulkiness = allocated` is approximate for fragmented files
- ⚠️ Extra memory for transformed structures

**Accuracy**: For the bulkiness heuristic (filtering files >1% of folder size), the approximation is acceptable. 99%+ of files are single-extent where `bulkiness = allocated` is exact.

#### Option B: Modify MFT Parsing (For exact C++ parity)

Add `bulkiness` computation during parsing:

| Change | Location |
|--------|----------|
| Add `bulkiness` field to `SizeInfo` | `crates/uffs-mft/src/index.rs` |
| Compute `bulkiness += allocated` for each attribute | `crates/uffs-mft/src/io.rs` |
| Initialize `treesize` per-stream | `crates/uffs-mft/src/io.rs` |

**Pros**:
- ✅ Exact C++ parity for all files
- ✅ No transformation overhead

**Cons**:
- ⚠️ Changes core parsing code
- ⚠️ Affects all users (not just C++ port)
- ⚠️ Harder to verify in isolation

### 5.5 Decision: Option A (Transformer) First

**We will implement Option A (Transformer) first.**

Rationale:
1. ✅ Keeps existing code untouched - no risk of breaking current functionality
2. ✅ Allows isolated verification of C++ algorithm against C++ output
3. ✅ `bulkiness = allocated` approximation is acceptable for 99%+ of files
4. ✅ Can switch between algorithms via `TreeAlgorithm` enum for A/B testing
5. ✅ Once verified working, we can decide whether Option B is needed

### 5.6 Transformer Design

```rust
// crates/uffs-mft/src/cpp_tree.rs

/// Transform MftIndex data for C++ tree algorithm
pub fn prepare_cpp_tree_input(index: &MftIndex) -> CppTreeInput {
    let mut input = CppTreeInput::new();

    for record in &index.records {
        // Transform SizeInfo → CppSizeInfo
        let cpp_size = CppSizeInfo {
            length: record.first_stream.size.length,
            allocated: record.first_stream.size.allocated,
            bulkiness: record.first_stream.size.allocated,  // Approximate
            treesize: if record.is_directory() { 1 } else { 0 },
        };

        // Transform ChildInfo → CppChildInfo (truncate FRS to u32)
        // ... build child list ...

        input.add_record(record.frs, cpp_size, children);
    }

    input
}

/// Run C++ tree algorithm and write results back
pub fn compute_tree_metrics_cpp(index: &mut MftIndex) {
    let input = prepare_cpp_tree_input(index);
    let results = cpp_tree_traverse(&input);

    // Write results back to FileRecord
    for (frs, metrics) in results {
        if let Some(record) = index.find_mut(frs) {
            record.descendants = metrics.descendants;
            record.treesize = metrics.treesize as u64;
            record.tree_allocated = metrics.allocated;
        }
    }
}
```

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
2. ~~**Bulkiness calculation**: C++ has complex bulkiness logic with heap operations. Do we need this for parity?~~
   - ✅ ANSWERED: Yes, we need it. Using `bulkiness = allocated` approximation for Option A (transformer). See Section 5.
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

---

## 11. Future Enhancements

### 11.1 Option B: Modify MFT Parsing for Exact Parity

If testing reveals that the `bulkiness = allocated` approximation causes significant differences for fragmented files, we can implement Option B:

#### Changes Required:

| File | Change |
|------|--------|
| `crates/uffs-mft/src/index.rs` | Add `bulkiness: u64` field to `SizeInfo` |
| `crates/uffs-mft/src/index.rs` | Add `treesize: u32` field to `IndexStreamInfo` (per-stream) |
| `crates/uffs-mft/src/io.rs` | Compute `bulkiness += allocated` for each attribute run |
| `crates/uffs-mft/src/io.rs` | Initialize `treesize = is_directory ? 1 : 0` per-stream |
| `crates/uffs-mft/src/parse.rs` | Same changes for `parse_record_full()` |

#### Implementation Steps:

1. **Add fields to structures**:
   ```rust
   pub struct SizeInfo {
       pub length: u64,
       pub allocated: u64,
       pub bulkiness: u64,  // NEW: accumulated allocated for fragmentation penalty
   }

   pub struct IndexStreamInfo {
       pub size: SizeInfo,
       pub treesize: u32,   // NEW: per-stream (1 for dirs, 0 for files)
       // ... existing fields ...
   }
   ```

2. **Update parsing** (`io.rs` and `parse.rs`):
   ```rust
   // For each $DATA attribute run:
   stream.size.allocated += attribute_allocated;
   stream.size.bulkiness += stream.size.allocated;  // Cumulative!
   stream.treesize = if is_directory { 1 } else { 0 };
   ```

3. **Update serialization** (cache format):
   - Bump cache version
   - Add `bulkiness` and per-stream `treesize` to binary format

### 11.2 Merging Isolated Code into Main Codebase

Once the C++ port is verified working and we're confident it's correct:

#### Phase 1: Verification Complete
- [ ] C++ port matches C++ output 100% for all test volumes
- [ ] Performance is acceptable (within 2x of current algorithm)
- [ ] No regressions in existing functionality

#### Phase 2: Deprecate Old Algorithm
1. Change default from `TreeAlgorithm::Current` to `TreeAlgorithm::CppPort`
2. Add deprecation warning when `TreeAlgorithm::Current` is used
3. Update documentation to recommend C++ port

#### Phase 3: Remove Old Algorithm
1. Remove `TreeAlgorithm::Current` variant
2. Remove `compute_tree_metrics()` (old algorithm)
3. Rename `compute_tree_metrics_cpp_port()` to `compute_tree_metrics()`
4. Remove `TreeAlgorithm` enum entirely (only one algorithm)

#### Phase 4: Merge Structures (if Option B implemented)
1. Replace `SizeInfo` with `CppSizeInfo` (add `bulkiness`)
2. Move `treesize` from `FileRecord` to `IndexStreamInfo` (per-stream)
3. Replace `ChildInfo.child_frs: u64` with `record_number: u32`
4. Update all callers

#### Phase 5: Cleanup
1. Remove `cpp_tree.rs` module (merged into `index.rs`)
2. Remove transformer code (no longer needed)
3. Update tests to use new structure names
4. Final documentation update

### 11.3 Migration Checklist

```markdown
## C++ Port Migration Checklist

### Verification
- [ ] Root descendants match C++ output
- [ ] Root treesize match C++ output
- [ ] All directory metrics match C++ output
- [ ] Performance benchmarks acceptable
- [ ] No regressions in search/filter functionality

### Deprecation
- [ ] Default changed to CppPort
- [ ] Deprecation warning added
- [ ] Documentation updated

### Removal
- [ ] Old algorithm removed
- [ ] TreeAlgorithm enum removed
- [ ] Method renamed

### Structure Merge (if Option B)
- [ ] SizeInfo updated with bulkiness
- [ ] treesize moved to per-stream
- [ ] ChildInfo uses u32 record_number
- [ ] Cache format updated
- [ ] All callers updated

### Cleanup
- [ ] cpp_tree.rs merged/removed
- [ ] Transformer code removed
- [ ] Tests updated
- [ ] Documentation finalized
```

