# C++ Parsing Parity - Implementation Guide

> **Goal**: Bring the Rust MFT parsing implementation to 100% parity with the C++ implementation, focusing on the **synchronization model** and **data structure management** that makes C++ work correctly.

**Branch**: `feature/cpp-parsing-algorithm-port`  
**Status**: Analysis complete, scaffolding document created  
**Date**: 2026-01-31

---

## Executive Summary

The C++ and Rust implementations use **fundamentally different synchronization models**:

| Aspect | C++ Implementation | Rust Implementation |
|--------|-------------------|---------------------|
| **I/O Concurrency** | 2 reads in flight | 8 reads in flight |
| **Parsing** | **Serialized under mutex lock** | **Parallel (Rayon)** |
| **Shared State** | Direct mutation during parsing | Thread-local, merge at end |
| **Extension Records** | Merged immediately via `at()` | Collected, merged post-parsing |
| **Parent Placeholders** | Created on-demand during parsing | Must exist or child skipped |

**The Rust approach is NOT working** because:
1. Extension records may arrive before their base records - Rust silently drops them
2. Parent placeholders are not created on-demand - child entries are skipped
3. Parallel parsing creates race conditions for dependent records

---

## 1. Resources

> **IMPORTANT**: This implementation is based EXCLUSIVELY on C++ source code.
> All resources are located in `docs/architecture/C++_resources/`.

### 1.1 Primary C++ Source Files

| File | Purpose | Key Lines |
|------|---------|-----------|
| `UltraFastFileSearch-code/src/index/ntfs_index.hpp` | Main parsing + index management | 106-728 |
| `UltraFastFileSearch-code/src/io/mft_reader.hpp` | IOCP-based async reading | 1-498 |
| `UltraFastFileSearch-code/src/io/io_completion_port.hpp` | IOCP wrapper | 1-351 |
| `UltraFastFileSearch-code/src/util/lock_ptr.hpp` | RAII lock wrapper | 1-50 |

### 1.2 Critical C++ Code Sections

| Section | File | Lines | Description |
|---------|------|-------|-------------|
| `at()` function | ntfs_index.hpp | 106-129 | **Lazy allocation** - creates placeholder if record doesn't exist |
| `preload_concurrent()` | ntfs_index.hpp | 424-475 | **NO LOCK** - USA fixup, max FRS discovery |
| `load()` | ntfs_index.hpp | 477-728 | **WITH LOCK** - Serialized attribute parsing |
| Concurrency init | mft_reader.hpp | 482-485 | Starts with 2 concurrent reads |
| `queue_next()` | mft_reader.hpp | 321-387 | Issues next read after completion |
| Buffer recycling | mft_reader.hpp | 98-158 | Custom operator new/delete |

---

## 2. The C++ Synchronization Model

### 2.1 The Two-Phase Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                    C++ MFT Processing Pipeline                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────┐    ┌──────────────┐                                       │
│  │   Read #1    │    │   Read #2    │   (Only 2 in flight!)                 │
│  │  (IOCP)      │    │  (IOCP)      │                                       │
│  └──────┬───────┘    └──────┬───────┘                                       │
│         │                   │                                                │
│         ▼                   ▼                                                │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  PHASE 1: preload_concurrent() - NO LOCK (can run in parallel)       │   │
│  │  ────────────────────────────────────────────────────────────────    │   │
│  │  • Apply USA fixup (modifies buffer in-place)                        │   │
│  │  • Find max FRS in chunk for pre-allocation                          │   │
│  │  • Brief lock: at(max_frs) to pre-allocate records vector            │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│         │                   │                                                │
│         ▼                   ▼                                                │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  PHASE 2: lock(q->p)->load() - WITH LOCK (serialized!)               │   │
│  │  ────────────────────────────────────────────────────────────────    │   │
│  │  • Only ONE chunk parsed at a time (mutex held)                      │   │
│  │  • Parse $STANDARD_INFORMATION → base_record->stdinfo                │   │
│  │  • Parse $FILE_NAME → base_record->first_name + ChildInfo            │   │
│  │  • Parse streams → base_record->first_stream                         │   │
│  │  • at(frs_parent) creates parent placeholder if needed               │   │
│  │  • at(frs_base) creates base placeholder for extension records       │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│         │                                                                    │
│         ▼                                                                    │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  queue_next() - Issue next read (maintains 2 in flight)              │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Why Serialized Parsing?

The C++ implementation uses **shared mutable state** during parsing:

```cpp
// These vectors are mutated during parsing (under lock):
Records records_data;           // All file records
RecordsLookup records_lookup;   // FRS → record index
LinkInfos nameinfos;            // Overflow hard links
StreamInfos streaminfos;        // Overflow streams
ChildInfos childinfos;          // Parent-child relationships
std::tvstring names;            // All filenames concatenated
```

**Key insight**: Because parsing mutates shared state, it MUST be serialized.
The lock ensures:
1. `at(frs)` can safely resize vectors and create placeholders
2. Parent-child links are built atomically
3. Extension records find their base records (or create placeholders)

---

## 3. The `at()` Function - Lazy Allocation

This is the **most critical function** for C++ parity:

```cpp
// ntfs_index.hpp lines 106-129
Records::iterator at(size_t const frs, Records::iterator* const existing_to_revalidate = nullptr) {
    // Expand lookup table if needed
    if (frs >= this->records_lookup.size()) {
        this->records_lookup.resize(frs + 1, ~RecordsLookup::value_type());
    }

    RecordsLookup::iterator const k = this->records_lookup.begin() + frs;
    if (!~*k) {  // Record doesn't exist yet (value is ~0)
        ptrdiff_t const j = (existing_to_revalidate 
            ? *existing_to_revalidate 
            : this->records_data.end()) - this->records_data.begin();
        
        *k = static_cast<unsigned int>(this->records_data.size());
        this->records_data.resize(this->records_data.size() + 1);  // CREATE PLACEHOLDER!
        
        if (existing_to_revalidate) {
            *existing_to_revalidate = this->records_data.begin() + j;
        }
    }

    return this->records_data.begin() + static_cast<ptrdiff_t>(*k);
}
```

**What this does:**
1. If FRS doesn't exist in lookup table → expand table
2. If record doesn't exist → **create empty placeholder record**
3. Return iterator to the record (existing or newly created)

**Why this matters:**
- Extension record arrives before base → `at(base_frs)` creates placeholder
- Child record arrives before parent → `at(parent_frs)` creates placeholder
- No record is ever "lost" due to out-of-order processing


---

## 4. Extension Record Handling

### 4.1 C++ Approach (Immediate Merge)

```cpp
// ntfs_index.hpp lines 520-522
unsigned int const frs_base = frsh->BaseFileRecordSegment 
    ? static_cast<unsigned int>(frsh->BaseFileRecordSegment) 
    : frs;

auto base_record = this->at(frs_base);  // Creates placeholder if base doesn't exist!
```

**Key insight**: Extension records are merged **immediately** during parsing.

### 4.2 Current Rust Approach (Deferred Merge)

```rust
pub enum ParseResult {
    Base(BaseRecord),
    Extension(ExtensionAttributes),  // Stored separately!
    Skip,
}
```

**Problem**: If base record never arrives, extension records are **lost forever**.

### 4.3 The Fix: Rust `get_or_create()` with Placeholder

```rust
impl MftIndex {
    pub fn get_or_create(&mut self, frs: u64) -> &mut FileRecord {
        let frs_idx = frs as usize;
        if frs_idx >= self.frs_to_idx.len() {
            self.frs_to_idx.resize(frs_idx + 1, NO_ENTRY);
        }
        if self.frs_to_idx[frs_idx] == NO_ENTRY {
            let record_idx = self.records.len() as u32;
            self.frs_to_idx[frs_idx] = record_idx;
            self.records.push(FileRecord::placeholder(frs));
        }
        &mut self.records[self.frs_to_idx[frs_idx] as usize]
    }
}
```

---

## 5. Parent-Child Link Building

### 5.1 C++ Approach (Placeholder Parents)

```cpp
// ntfs_index.hpp lines 563-578
if (frs_parent != frs_base) {
    Records::iterator const parent = this->at(frs_parent, &base_record);
    // at(frs_parent) creates parent placeholder if needed!
    
    child_info->record_number = frs_base;
    child_info->next_entry = parent->first_child;
    parent->first_child = child_index;
}
```

### 5.2 Current Rust Approach (Skip if Parent Missing)

```rust
pub fn add_child_entry(&mut self, child_frs: u64, parent_frs: u64) {
    let Some(parent_idx) = self.frs_to_idx_opt(parent_frs) else {
        return;  // Parent doesn't exist - child entry NOT created!
    };
}
```

**Problem**: If parent record is in a later chunk, child entry is **never created**.

### 5.3 The Fix: Always Create Parent Placeholder

```rust
pub fn add_child_entry(&mut self, child_frs: u64, parent_frs: u64, name_index: u16) {
    let parent = self.get_or_create(parent_frs);  // Create placeholder if needed
    parent.children.push(ChildEntry { record_frs: child_frs, name_index });
}
```

---

## 6. Concurrency Level

### 6.1 C++ Default: 2 Concurrent Reads

```cpp
// mft_reader.hpp lines 482-485
for (int concurrency = 0; concurrency < 2; ++concurrency) {
    this->queue_next();
}
```

**Why 2?** Optimal for HDD sequential reads - one read in flight while previous is processed.

### 6.2 Current Rust Default: 8 Concurrent Reads

Can cause HDD head thrashing and I/O scheduler confusion.

### 6.3 The Fix: Match C++ Default

```rust
const DEFAULT_CONCURRENCY: usize = 2;
```

---

## 7. Implementation Plan: `CppParseMode`

### 7.1 New Module Structure

```
crates/uffs-mft/src/
├── cpp_parse/
│   ├── mod.rs           # Public API
│   ├── index.rs         # CppMftIndex with get_or_create()
│   ├── parser.rs        # Serialized parsing under lock
│   └── pipeline.rs      # Two-phase pipeline orchestration
```

### 7.2 Core Types

```rust
use std::sync::Mutex;

pub struct CppMftIndex {
    frs_to_idx: Vec<u32>,
    records: Vec<FileRecord>,
    child_entries: Vec<ChildEntry>,
}

impl CppMftIndex {
    pub fn get_or_create(&mut self, frs: u64) -> &mut FileRecord {
        // ... (see section 4.3)
    }
}
```

### 7.3 Two-Phase Pipeline

```rust
pub struct CppParsePipeline {
    index: Arc<Mutex<CppMftIndex>>,
    concurrency: usize,
}

impl CppParsePipeline {
    pub fn process_chunk(&self, chunk: &mut [u8], base_frs: u64, record_size: u32) {
        // PHASE 1: Pre-processing (NO LOCK)
        self.preload_concurrent(chunk, base_frs, record_size);
        
        // PHASE 2: Parsing (WITH LOCK - serialized)
        let mut index = self.index.lock().unwrap();
        self.load(&mut index, chunk, base_frs, record_size);
    }
}
```

---

## 8. Implementation Phases

### Phase 1: Core Infrastructure (2-3 days)
- [ ] Create `cpp_parse/` module structure
- [ ] Implement `CppMftIndex` with `get_or_create()`
- [ ] Implement `FileRecord::placeholder()`
- [ ] Add unit tests for placeholder behavior

### Phase 2: Serialized Parser (3-4 days)
- [ ] Implement `CppParsePipeline` with two-phase processing
- [ ] Port attribute parsing to work with mutable index
- [ ] Implement `add_child_entry()` with placeholder creation
- [ ] Add unit tests for extension/parent handling

### Phase 3: Integration (2-3 days)
- [ ] Wire up `ParseAlgorithm::CppPort` to use new pipeline
- [ ] Add concurrency configuration (default: 2)
- [ ] Integration tests with real MFT data

### Phase 4: Verification (1-2 days)
- [ ] Compare output with C++ implementation
- [ ] Test edge cases (extension-only records, orphans)

**Total estimated time: 8-12 days**

---

## 9. Key Differences Summary

| Aspect | C++ | Current Rust | Fixed Rust |
|--------|-----|--------------|------------|
| Extension records | Merged immediately | Collected, merged post | Merged via `get_or_create()` |
| Parent placeholders | Created on-demand | Skipped if missing | Created on-demand |
| Parsing | Serialized under lock | Parallel (Rayon) | Serialized under Mutex |
| Concurrency | 2 reads | 8 reads | 2 reads (configurable) |

---

## 10. References

- `docs/architecture/C++_resources/UltraFastFileSearch-code/src/index/ntfs_index.hpp`
- `docs/architecture/C++_resources/UltraFastFileSearch-code/src/io/mft_reader.hpp`
- `docs/architecture/CPP_PARSE_ALGORITHM_PORT.md` (attribute parsing details)
- `docs/architecture/CPP_TREE_ALGORITHM_PORT.md` (tree algorithm reference)
