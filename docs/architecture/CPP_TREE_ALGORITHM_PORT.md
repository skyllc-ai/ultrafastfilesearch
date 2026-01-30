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

/// C++ ChildInfo equivalent - with 64-bit FRS for NTFS spec compliance
/// NOTE: C++ uses 32-bit record_number, but Rust uses 64-bit for future-proofing.
/// See Section 4.8 for detailed rationale.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct CppChildInfo {
    pub next_entry: u32,       // ~0 = end of list (matches C++)
    pub record_number: u64,    // FRS of child (64-bit, improved from C++ 32-bit)
    pub name_index: u16,       // Which hardlink (matches C++)
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
| `ChildInfo` | record_number | 4 bytes | `CppChildInfo.record_number` | ✅ **u64** (improved from C++ u32, see §4.8) |
| `ChildInfo` | name_index | 2 bytes | `CppChildInfo.name_index` | ✅ |

### 4.6 What the C++ Algorithm Needs

| Requirement | C++ Source | Available in Rust? | Action |
|-------------|------------|-------------------|--------|
| `bulkiness` field | `SizeInfo.bulkiness` | ❌ Missing | Add to `CppSizeInfo` |
| `treesize` per-stream | `SizeInfo.treesize` | ❌ Wrong location | Add to `CppSizeInfo` |
| Heap for bulkiness filter | `scratch` vector | ❌ Not implemented | Implement in C++ port |
| `record_number` as u64 | `ChildInfo.record_number` | ✅ Rust uses u64 | Use `CppChildInfo` with u64 (see §4.8) |
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

### 4.8 FRS Representation: C++ (32-bit) vs Rust (64-bit)

> **IMPORTANT**: This section documents a deliberate improvement in the Rust implementation.

#### NTFS Specification

Per the NTFS specification, a **FILE_REFERENCE** is a 64-bit value:
- **Lower 48 bits**: File Record Segment (FRS) number - index into the MFT
- **Upper 16 bits**: Sequence number - incremented when FRS is reused

```
┌─────────────────────────────────────────────────────────────────┐
│                      FILE_REFERENCE (64 bits)                   │
├─────────────────────────────────────────────────────────────────┤
│  Sequence Number (16 bits)  │     FRS Number (48 bits)          │
│         bits 63-48          │         bits 47-0                 │
└─────────────────────────────────────────────────────────────────┘
```

**Maximum FRS value**: 2^48 - 1 = 281,474,976,710,655 (281 trillion entries)

#### C++ Implementation (32-bit FRS)

The C++ implementation uses 32-bit `unsigned int` for FRS numbers:

```cpp
// From ntfs_key_type.hpp
typedef unsigned int frs_type;  // 32-bit

// From ntfs_record_types.hpp
struct ChildInfo {
    small_t<size_t>::type record_number;  // 32-bit unsigned int
    // ...
};
```

**C++ Limitation**: Maximum FRS = 2^32 - 1 = 4,294,967,295 (~4.3 billion entries)

This was acceptable when the C++ code was written, but modern large disks can exceed this:
- A 100TB drive with 1KB average file size = 100 billion files
- Enterprise storage arrays can have trillions of files

#### Rust Implementation (64-bit FRS) - CORRECT

The Rust implementation correctly uses 64-bit for FRS numbers:

```rust
// From crates/uffs-mft/src/ntfs.rs
pub fn file_reference_to_frs(file_reference: u64) -> u64 {
    file_reference & 0x0000_FFFF_FFFF_FFFF  // Extract lower 48 bits
}

// FRS stored as u64 throughout the codebase
pub struct FileRecord {
    pub frs: u64,  // 64-bit FRS
    // ...
}
```

**Rust Advantage**: Future-proof for larger disks and storage systems.

#### Impact on Tree Algorithm

**The tree algorithm is NOT affected by the FRS size difference.**

The tree algorithm uses FRS numbers only as:
1. **HashMap keys** for record lookup (`frs_to_idx: HashMap<u64, usize>`)
2. **Index values** in `ChildInfo.record_number`

Both operations work identically with 32-bit or 64-bit values. The algorithm logic (delta formula, bulkiness calculation, treesize accumulation) is completely independent of FRS size.

#### Updated CppChildInfo Structure

For the Rust port, we use 64-bit FRS to match Rust's correct representation:

```rust
/// C++ ChildInfo equivalent - with 64-bit FRS for future-proofing
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct CppChildInfo {
    pub next_entry: u32,       // ~0 = end of list (matches C++)
    pub record_number: u64,    // FRS of child (64-bit, improved from C++ 32-bit)
    pub name_index: u16,       // Which hardlink (matches C++)
}
```

**Note**: This is a deliberate improvement over C++. The extra 4 bytes per ChildInfo is negligible compared to the benefit of supporting larger volumes.

#### Verification

To verify the tree algorithm produces identical results regardless of FRS size:
1. All test volumes have FRS values < 2^32, so C++ and Rust use the same values
2. The algorithm logic is identical - only the storage type differs
3. Unit tests verify delta formula and accumulation work correctly with u64

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

Create Rust structures that match C++ layout with 64-bit FRS improvement (see §4.8):

```rust
/// C++ ChildInfo equivalent - with 64-bit FRS for NTFS spec compliance
/// NOTE: C++ uses 32-bit record_number, Rust uses 64-bit for future-proofing.
#[derive(Debug, Clone, Copy)]
pub struct CppChildInfo {
    pub next_entry: u32,       // ~0 = end of list (matches C++)
    pub record_number: u64,    // FRS of child (64-bit, improved from C++ 32-bit)
    pub name_index: u16,       // Which hardlink (matches C++)
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
    record_number: frs,  // u64 - no truncation needed
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
- [ ] ChildInfo uses u64 record_number (NTFS spec compliant)
- [ ] Cache format updated
- [ ] All callers updated

### Cleanup
- [ ] cpp_tree.rs merged/removed
- [ ] Transformer code removed
- [ ] Tests updated
- [ ] Documentation finalized
```

---

## 12. Unit Tests and Validation

> **Goal**: Comprehensive test coverage to ensure the C++ port produces identical results to the C++ implementation.

### 12.1 Delta Formula Tests

The delta formula is the core of proportional hardlink share calculation. Test it exhaustively:

```rust
#[cfg(test)]
mod delta_tests {
    use super::*;

    /// Delta formula: value * (i + 1) / n - value * i / n
    /// Ensures no rounding errors when dividing among hardlinks
    fn delta(value: u64, i: u16, n: u16) -> u64 {
        let n = n as u64;
        let i = i as u64;
        value * (i + 1) / n - value * i / n
    }

    #[test]
    fn test_delta_single_hardlink() {
        // Single hardlink: file gets 100% of its size
        assert_eq!(delta(1000, 0, 1), 1000);
        assert_eq!(delta(u64::MAX, 0, 1), u64::MAX);
    }

    #[test]
    fn test_delta_two_hardlinks_even() {
        // Two hardlinks, even split: 1000 / 2 = 500 each
        assert_eq!(delta(1000, 0, 2), 500);  // First hardlink
        assert_eq!(delta(1000, 1, 2), 500);  // Second hardlink
        // Verify sum equals original
        assert_eq!(delta(1000, 0, 2) + delta(1000, 1, 2), 1000);
    }

    #[test]
    fn test_delta_two_hardlinks_odd() {
        // Two hardlinks, odd value: 1001 / 2 = 500 + 501
        assert_eq!(delta(1001, 0, 2), 500);  // First hardlink
        assert_eq!(delta(1001, 1, 2), 501);  // Second hardlink (gets extra)
        // Verify sum equals original
        assert_eq!(delta(1001, 0, 2) + delta(1001, 1, 2), 1001);
    }

    #[test]
    fn test_delta_three_hardlinks() {
        // Three hardlinks: 100 / 3 = 33 + 33 + 34
        assert_eq!(delta(100, 0, 3), 33);
        assert_eq!(delta(100, 1, 3), 33);
        assert_eq!(delta(100, 2, 3), 34);
        // Verify sum equals original
        assert_eq!(delta(100, 0, 3) + delta(100, 1, 3) + delta(100, 2, 3), 100);
    }

    #[test]
    fn test_delta_max_hardlinks() {
        // Maximum hardlinks (1023 per C++ limit)
        let value = 1_000_000u64;
        let n = 1023u16;
        let mut sum = 0u64;
        for i in 0..n {
            sum += delta(value, i, n);
        }
        assert_eq!(sum, value);  // Sum must equal original
    }

    #[test]
    fn test_delta_large_values() {
        // Large file sizes (petabyte scale)
        let petabyte = 1_000_000_000_000_000u64;
        assert_eq!(delta(petabyte, 0, 2) + delta(petabyte, 1, 2), petabyte);
    }
}
```

### 12.2 Bulkiness Algorithm Tests

Test the heap-based bulkiness filtering:

```rust
#[cfg(test)]
mod bulkiness_tests {
    use super::*;

    #[test]
    fn test_bulkiness_single_file() {
        // Single file: bulkiness = allocated
        let allocated = 4096u64;
        let bulkiness = allocated;  // Single extent
        assert_eq!(bulkiness, allocated);
    }

    #[test]
    fn test_bulkiness_fragmented_file() {
        // Fragmented file: bulkiness > allocated
        // Run 1: allocated=100, bulkiness=100
        // Run 2: allocated=200, bulkiness=100+200=300
        // Run 3: allocated=300, bulkiness=300+300=600
        let mut allocated = 0u64;
        let mut bulkiness = 0u64;

        // Simulate 3 attribute runs
        allocated = 100; bulkiness += allocated;  // Run 1
        allocated = 200; bulkiness += allocated;  // Run 2
        allocated = 300; bulkiness += allocated;  // Run 3

        assert_eq!(allocated, 300);   // Final allocated
        assert_eq!(bulkiness, 600);   // Cumulative bulkiness
    }

    #[test]
    fn test_bulkiness_filter_threshold() {
        // Files > 1% of folder size are excluded from bulkiness calculation
        let folder_allocated = 1_000_000u64;
        let threshold = folder_allocated / 100;  // 1% = 10,000

        let small_file = 5_000u64;   // < 1%, included
        let large_file = 50_000u64;  // > 1%, excluded

        assert!(small_file < threshold);
        assert!(large_file > threshold);
    }
}
```

### 12.3 Tree Traversal Tests

Test the recursive DFS traversal:

```rust
#[cfg(test)]
mod tree_traversal_tests {
    use super::*;

    #[test]
    fn test_empty_directory() {
        // Empty directory: descendants=0, treesize=1 (just itself)
        let mut index = create_test_index_empty_dir();
        index.compute_tree_metrics_cpp_port(false);

        let root = index.find(5).unwrap();
        assert_eq!(root.descendants, 0);
        assert_eq!(root.treesize, 1);
    }

    #[test]
    fn test_single_file() {
        // Directory with one file: descendants=1, treesize=2
        let mut index = create_test_index_single_file();
        index.compute_tree_metrics_cpp_port(false);

        let root = index.find(5).unwrap();
        assert_eq!(root.descendants, 1);
        assert_eq!(root.treesize, 2);  // dir + file
    }

    #[test]
    fn test_nested_directories() {
        // /root/subdir/file.txt
        // root: descendants=2, treesize=3
        // subdir: descendants=1, treesize=2
        let mut index = create_test_index_nested();
        index.compute_tree_metrics_cpp_port(false);

        let root = index.find(5).unwrap();
        assert_eq!(root.descendants, 2);

        let subdir = index.find(100).unwrap();
        assert_eq!(subdir.descendants, 1);
    }

    #[test]
    fn test_hardlink_proportional_share() {
        // File with 2 hardlinks in different directories
        // Each directory should get 50% of file size
        let mut index = create_test_index_hardlink();
        index.compute_tree_metrics_cpp_port(false);

        let dir1 = index.find(100).unwrap();
        let dir2 = index.find(200).unwrap();

        // File size = 1000, each dir gets 500
        assert_eq!(dir1.tree_allocated, 500);
        assert_eq!(dir2.tree_allocated, 500);
    }

    #[test]
    fn test_alternate_data_streams() {
        // File with ADS: each stream adds +1 to treesize
        let mut index = create_test_index_ads();
        index.compute_tree_metrics_cpp_port(false);

        let root = index.find(5).unwrap();
        // dir(1) + file_default_stream(1) + file_ads(1) = 3
        assert_eq!(root.treesize, 3);
    }
}
```

### 12.4 Comparison Tests Against C++ Output

The ultimate validation is comparing against actual C++ output:

```rust
#[cfg(test)]
mod cpp_comparison_tests {
    use super::*;

    /// Load C++ output from trial_run.ps1 and compare
    #[test]
    #[ignore]  // Run manually with: cargo test cpp_comparison -- --ignored
    fn test_against_cpp_output() {
        // Load C++ reference data
        let cpp_data = load_cpp_reference("test_data/cpp_output.json");

        // Run Rust C++ port
        let mut index = MftIndex::from_cache("test_data/test_volume.cache").unwrap();
        index.compute_tree_metrics_cpp_port(false);

        // Compare every directory
        for (frs, cpp_metrics) in cpp_data.directories {
            let rust_record = index.find(frs).expect(&format!("FRS {} not found", frs));

            assert_eq!(
                rust_record.descendants, cpp_metrics.descendants,
                "FRS {}: descendants mismatch (Rust={}, C++={})",
                frs, rust_record.descendants, cpp_metrics.descendants
            );

            assert_eq!(
                rust_record.treesize, cpp_metrics.treesize,
                "FRS {}: treesize mismatch (Rust={}, C++={})",
                frs, rust_record.treesize, cpp_metrics.treesize
            );

            assert_eq!(
                rust_record.tree_allocated, cpp_metrics.tree_allocated,
                "FRS {}: tree_allocated mismatch (Rust={}, C++={})",
                frs, rust_record.tree_allocated, cpp_metrics.tree_allocated
            );
        }
    }
}
```

### 12.5 Edge Case Tests

```rust
#[cfg(test)]
mod edge_case_tests {
    use super::*;

    #[test]
    fn test_root_self_reference() {
        // Root directory (FRS 5) has parent = itself
        // Algorithm must skip this to avoid infinite loop
        let mut index = create_test_index_root_only();
        index.compute_tree_metrics_cpp_port(false);  // Should not hang
    }

    #[test]
    fn test_deep_tree() {
        // 100-level deep tree (tests stack depth)
        let mut index = create_test_index_deep(100);
        index.compute_tree_metrics_cpp_port(false);

        let root = index.find(5).unwrap();
        assert_eq!(root.descendants, 100);
    }

    #[test]
    fn test_wide_tree() {
        // Directory with 10,000 children
        let mut index = create_test_index_wide(10_000);
        index.compute_tree_metrics_cpp_port(false);

        let root = index.find(5).unwrap();
        assert_eq!(root.descendants, 10_000);
    }

    #[test]
    fn test_orphan_files() {
        // Files with missing parent (orphans)
        // Should be skipped, not cause panic
        let mut index = create_test_index_orphans();
        index.compute_tree_metrics_cpp_port(false);  // Should not panic
    }

    #[test]
    fn test_zero_size_file() {
        // Empty file: length=0, allocated=0
        let mut index = create_test_index_zero_size();
        index.compute_tree_metrics_cpp_port(false);

        let root = index.find(5).unwrap();
        assert_eq!(root.tree_allocated, 0);
    }

    #[test]
    fn test_max_frs_value() {
        // FRS at 48-bit boundary (tests u64 handling)
        let max_frs = 0x0000_FFFF_FFFF_FFFFu64;  // 48-bit max
        let mut index = create_test_index_with_frs(max_frs);
        index.compute_tree_metrics_cpp_port(false);  // Should handle correctly
    }
}
```

### 12.6 Running the Tests

```bash
# Run all unit tests
cargo test -p uffs-mft tree

# Run delta formula tests only
cargo test -p uffs-mft delta_tests

# Run comparison tests against C++ output (requires test data)
cargo test -p uffs-mft cpp_comparison -- --ignored

# Run with verbose output for debugging
cargo test -p uffs-mft tree -- --nocapture
```

### 12.7 Test Data Generation

To generate test data for comparison tests:

```powershell
# Generate C++ reference output
cd docs/architecture/Investigation
.\trial_run.ps1 -Drives G -ExportJson test_data/cpp_output.json

# Generate Rust cache for the same volume
uffs index --drive G --cache-path test_data/test_volume.cache
```

### 12.8 Validation Checklist

Before marking the C++ port as complete, verify:

- [ ] All delta formula tests pass
- [ ] All bulkiness tests pass
- [ ] All tree traversal tests pass
- [ ] All edge case tests pass
- [ ] Comparison test matches C++ output 100% for:
  - [ ] Root directory descendants
  - [ ] Root directory treesize
  - [ ] Root directory tree_allocated
  - [ ] All subdirectory metrics
- [ ] Performance is within 2x of current algorithm
- [ ] No memory leaks (run with `cargo test` under valgrind/ASAN)

---

## 13. Performance Benchmarking

> **Goal**: Measure and compare tree metrics computation performance between C++ and Rust implementations.

### 13.1 Benchmark Commands Overview

| Implementation | Command | What It Measures |
|----------------|---------|------------------|
| **C++** | `uffs.com --benchmark-mft=C:` | Raw MFT I/O only |
| **C++** | `uffs.com --benchmark-index=C:` | I/O + Parse + Preprocess (tree metrics) |
| **Rust** | `uffs_mft benchmark-mft --drive C` | Raw MFT I/O only |
| **Rust** | `uffs_mft benchmark-index-lean --drive C` | I/O + Parse + Index Build + Tree Metrics (with phase breakdown) |
| **Rust** | `uffs_mft benchmark-tree --drive C` | **Isolated tree metrics only** (for direct comparison) |

### 13.2 Apples-to-Apples Comparison

The C++ `--benchmark-index` command measures the full indexing pipeline including the "Preprocess" phase, which computes tree metrics (descendants, treesize, tree_allocated).

To compare tree metrics performance specifically:

#### C++ Tree Metrics Timing

```powershell
# Run C++ benchmark-index and look for "Preprocess" timing
C:\Users\$env:USERNAME\bin\uffs.com --benchmark-index=C:
```

The output includes a line like:
```
Preprocess: 123 ms
```

#### Rust Tree Metrics Timing

```powershell
# Option 1: Full pipeline with phase breakdown
uffs_mft benchmark-index-lean --drive C
# Look for "Tree Metrics: XXX ms" in the output

# Option 2: Isolated tree metrics (recommended for comparison)
uffs_mft benchmark-tree --drive C --iterations 5
# Reports min/max/avg/median for tree metrics only
```

### 13.3 Phase Breakdown (Rust)

The `benchmark-index-lean` command now shows detailed phase timing with **accurate instrumentation**:

```
=== Phase Timing Breakdown ===
Open/Metadata:    ...ms
I/O (read):       ...ms  ✓ accurate
Parse:            ...ms  ✓ accurate
Merge:            ...ms  ✓ accurate
Index Build:      ...ms  (record insertion + ext index + sort)
Tree Metrics:     ...ms  (C++ 'preprocessing' equivalent)
─────────────────────────────────────────
Total:            ...ms

=== C++ Comparison ===
I/O + Parse + Merge:  ...ms  (compare to C++ 'Read + Parse')
Tree Metrics:         ...ms  (compare to C++ 'Preprocess')
```

The **Tree Metrics** line corresponds directly to C++'s **Preprocess** phase.

> **Note**: The I/O, Parse, and Merge timings are now **accurately instrumented** (not estimated). The reader has been refactored to measure each phase separately.

### 13.4 Isolated Tree Metrics Benchmark

For the most accurate comparison, use `benchmark-tree`:

```powershell
# Run 5 iterations, use cached index
uffs_mft benchmark-tree --drive C --iterations 5

# Run 3 iterations, build fresh index (no cache)
uffs_mft benchmark-tree --drive C --no-cache
```

Output:
```
=== Tree Metrics Timing Results ===
Min:      45 ms
Max:      52 ms
Avg:      48 ms
Median:   47 ms

=== Throughput ===
Entries processed: 1234567
Throughput: 25720562 entries/sec
```

### 13.5 Automated Comparison Script

Use the `benchmark_tree_comparison.ps1` script for automated C++ vs Rust comparison:

```powershell
# Compare on drive C
.\benchmark_tree_comparison.ps1 -Drive C

# Compare on multiple drives
.\benchmark_tree_comparison.ps1 -Drives C,D,E

# Run more iterations for statistical significance
.\benchmark_tree_comparison.ps1 -Drive C -Iterations 10
```

The script is located at: `docs/architecture/Investigation/benchmark_tree_comparison.ps1`

### 13.6 Expected Performance Characteristics

| Metric | C++ | Rust | Notes |
|--------|-----|------|-------|
| **Raw I/O** | ~500 MB/s | ~500 MB/s | Limited by disk speed |
| **Parse** | ~1M records/sec | ~1.5M records/sec | Rust slightly faster |
| **Tree Metrics** | ~20M entries/sec | ~25M entries/sec | Target: within 2x of C++ |

### 13.7 Interpreting Results

When comparing C++ and Rust tree metrics performance:

1. **Same volume**: Always compare on the same drive to eliminate I/O variance
2. **Warm cache**: Run each benchmark 2-3 times; use the fastest run
3. **Isolated timing**: Use `benchmark-tree` for Rust to isolate tree metrics from I/O
4. **Entry count**: Verify both implementations process the same number of entries

#### Example Comparison

```
C++ --benchmark-index=C:
  Preprocess: 156 ms
  Total entries: 1,234,567

Rust benchmark-tree --drive C:
  Tree Metrics: 142 ms (avg of 5 runs)
  Entries processed: 1,234,567

Result: Rust is 1.10x faster (156/142 = 1.10)
```

### 13.8 Troubleshooting

| Issue | Cause | Solution |
|-------|-------|----------|
| Rust much slower | Cold cache | Run `uffs_mft cache-get --drive C` first |
| Entry count mismatch | Different filtering | Ensure both use same MFT source |
| High variance | Background I/O | Close other applications, run more iterations |
| C++ crashes | Large MFT | C++ has 32-bit FRS limit (~4B entries) |

