[# MFT Architecture Deep Dive: C++ vs Rust]()

## Executive Summary

This document analyzes the architectural differences between the C++ UltraFastFileSearch implementation and the Rust rewrite, identifying key optimizations that make C++ fast and proposing strategies to make Rust **even faster**.

**Current State:**
- C++ (legacy): ~43.6s for `*.txt` search, 735K files found
- Rust (current): ~56.1s for same search, fewer files due to path resolution bug

**Goal:** Make Rust 2-5x faster than the legacy baseline through modern architecture.

---

## 🎉 IMPORTANT: MFT Reader is Already Highly Optimized!

Before diving into optimizations, it's critical to understand that **the MFT reading layer (`uffs-mft`) is already extremely fast** thanks to extensive optimization work documented in [uffs-mft-optimization-plan.md](../uffs-mft-optimization-plan.md).

### What's Already Done (v0.1.39):

| Optimization | Description | Impact |
|--------------|-------------|--------|
| ✅ Bitmap skip | Fixed sector alignment, now skips ~33% of records | 20% faster |
| ✅ Rayon fold/reduce | Eliminated per-record atomics | 5-15% parse time |
| ✅ Fused stats + DF build | Single pass over records | 10-20% df_build |
| ✅ Reusable aligned buffer | No per-chunk allocation | 2-5% read time |
| ✅ Merge adjacent chunks | Fewer I/O operations | 2-10% read time |
| ✅ MftReadMode | Auto-selects Parallel (SSD) or Prefetch (HDD) | Optimal I/O |
| ✅ SoA layout (ParsedColumns) | Direct-to-columns parsing | 90% df_build reduction |
| ✅ Fast path | Skips extension record merging | 18% total |
| ✅ PrefetchMftReader | Double-buffered I/O with prefetch | HDD optimization |

### Current MFT Reader Performance:

| Metric | v0.1.30 (Baseline) | v0.1.39 (Current) | Improvement |
|--------|-------------------|-------------------|-------------|
| **Total (7 drives)** | 315s | **142s** | **55% faster** |
| SSD C: | 11.3s | **3.1s** | **73% faster** |
| SSD Throughput | 400-550 MB/s | **1,472-1,839 MB/s** | **~4x** |
| HDD S: | 160.6s | **45.9s** | **71% faster** |

**Key Insight:** SSD throughput of 1,839 MB/s **exceeds typical NVMe sequential read limits**, indicating the MFT reader is CPU-efficient and I/O-optimal.

### What This Means for the Full Search Pipeline:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        FULL SEARCH PIPELINE                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐    ┌───────────┐ │
│  │  MFT Read    │───▶│    Path      │───▶│   Pattern    │───▶│  Output   │ │
│  │  (uffs-mft)  │    │  Resolution  │    │   Matching   │    │ Formatting│ │
│  └──────────────┘    └──────────────┘    └──────────────┘    └───────────┘ │
│        │                    │                   │                   │       │
│        ▼                    ▼                   ▼                   ▼       │
│   ✅ OPTIMIZED         ❌ BROKEN           ⚠️ NEEDS WORK      ⚠️ BASIC    │
│   (55% faster)         (uses filtered      (no SIMD,          (no streaming)│
│   (4x throughput)       data, paths        no early term)                   │
│                         fail)                                               │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Reuse Assessment:

| Component | Location | Reuse % | Notes |
|-----------|----------|---------|-------|
| AlignedBuffer | uffs-mft/io.rs | **100%** | Sector-aligned for direct I/O |
| MftExtentMap | uffs-mft/io.rs | **100%** | VCN→LCN for fragmented MFTs |
| generate_read_chunks() | uffs-mft/io.rs | **100%** | Bitmap-based skip optimization |
| ParallelMftReader | uffs-mft/io.rs | **100%** | Rayon parallel parsing |
| PrefetchMftReader | uffs-mft/io.rs | **100%** | Double-buffered I/O (already pipelined!) |
| ParsedColumns | uffs-mft/io.rs | **100%** | SoA layout, direct-to-columns |
| MftReader | uffs-mft/reader.rs | **100%** | High-level API |

**Bottom Line:** The MFT reader is **95%+ reusable**. The optimizations in this document primarily apply to **uffs-core** (path resolution, pattern matching), not uffs-mft.

---

## Part 1: C++ Architecture Analysis

### 1.1 Core Data Structures (`NtfsIndex` class)

The legacy implementation uses a highly optimized, cache-friendly data layout:

```cpp
class NtfsIndex : public RefCounted<NtfsIndex> {
    std::tvstring names;           // Single contiguous buffer for ALL names
    Records records_data;          // Vector of Record structs
    RecordsLookup records_lookup;  // FRS → record index (vector, not map!)
    LinkInfos nameinfos;           // Hard link info
    StreamInfos streaminfos;       // Alternate data streams
    ChildInfos childinfos;         // Parent-child relationships
};
```

**Key insight:** Everything is stored in contiguous vectors, not hash maps.

### 1.2 Packed Structures (Memory Efficiency)

```cpp
#pragma pack(push, 1)
struct NameInfo {
    unsigned int _offset;      // Offset into names buffer (with ASCII flag in LSB)
    unsigned char length;      // Name length (max 255 chars)
};

struct LinkInfo {
    next_entry_type next_entry;  // Index to next hard link
    NameInfo name;               // Offset+length into names buffer
    unsigned int parent;         // Parent FRS (32-bit, not 64!)
};

struct Record {
    StandardInfo stdinfo;        // Timestamps + flags (bit-packed)
    unsigned short name_count;   // Hard link count
    unsigned short stream_count; // ADS count
    next_entry_type first_child; // First child index
    LinkInfo first_name;         // Primary name (inline, not pointer!)
    StreamInfo first_stream;     // Primary stream (inline!)
};
#pragma pack(pop)
```

**Size analysis:**
- `Record`: ~64 bytes (tightly packed)
- `LinkInfo`: ~10 bytes
- `NameInfo`: ~5 bytes

### 1.3 Name Storage Strategy

```cpp
// Names stored in single contiguous buffer
std::tvstring names;  // All names concatenated

// Access via offset + length
void* name_ptr = &names[info.name.offset()];
size_t name_len = info.name.length;
bool is_ascii = info.name.ascii();  // LSB of offset
```

**ASCII Compression:** If all characters are ASCII, stores as 1 byte per char (50% savings).

### 1.4 Path Resolution (`get_path`)

```cpp
size_t get_path(key_type key, std::tvstring& result, bool name_only) const {
    size_t old_size = result.size();
    
    // Walk parent chain using ParentIterator
    for (ParentIterator pi(this, key); pi.next() && !(name_only && pi.icomponent());) {
        // Append name component (in REVERSE order)
        append_directional(result, s, pi->second, pi->ascii ? -1 : 0, true);
    }
    
    // Single reverse at the end
    std::reverse(result.begin() + old_size, result.end());
    return result.size() - old_size;
}
```

**Key optimizations:**
1. **Single buffer allocation** - reuses `result` buffer
2. **Append in reverse** - avoids shifting data
3. **One reverse at end** - O(n) instead of O(n²)
4. **Direct array access** - `records_lookup[frs]` is O(1)

### 1.5 MFT Reading Strategy

```cpp
void load(unsigned long long virtual_offset, void* buffer, size_t size, ...) {
    for (size_t i = 0; i + mft_record_size <= size; i += mft_record_size) {
        unsigned int frs = (virtual_offset + i) >> mft_record_size_log2;
        ntfs::FILE_RECORD_SEGMENT_HEADER* frsh = 
            reinterpret_cast<FILE_RECORD_SEGMENT_HEADER*>(&buffer[i]);
        
        if (frsh->Magic == 'ELIF' && (frsh->Flags & FRH_IN_USE)) {
            // Parse attributes inline
            for (auto* ah = frsh->begin(); ah < frsh_end && ah->Type != AttributeEnd; ah = ah->next()) {
                switch (ah->Type) {
                    case AttributeStandardInformation: /* ... */ break;
                    case AttributeFileName: /* ... */ break;
                    case AttributeData: /* ... */ break;
                }
            }
        }
    }
}
```

**Key insight:** Parsing happens inline during the read loop, not as a separate pass.

---

## Part 2: Current Rust Architecture

### 2.1 Data Structures

```rust
// PathResolver uses HashMap (slower than vector lookup)
pub struct PathResolver {
    entries: HashMap<u64, (u64, String)>,  // FRS → (parent_frs, name)
    cache: HashMap<u64, String>,           // Resolved path cache
    volume: char,
}

// ParsedRecord uses separate String allocations
pub struct ParsedRecord {
    pub frs: u64,
    pub parent_frs: u64,
    pub name: String,              // Heap allocation per record!
    pub names: Vec<NameInfo>,      // Vec allocation per record!
    pub streams: Vec<StreamInfo>,  // Vec allocation per record!
    // ...
}
```

### 2.2 Path Resolution (Current)

```rust
pub fn resolve(&mut self, frs: u64) -> Result<String> {
    let mut components = Vec::new();  // Allocation!
    let mut current = frs;
    
    while current != 0 && current != 5 {
        if let Some((parent, name)) = self.entries.get(&current) {
            components.push(name.clone());  // String clone!
            current = *parent;
        } else {
            return Err(CoreError::PathResolution(current));
        }
    }
    
    components.reverse();
    let path = format!("{}:\\{}", self.volume, components.join("\\"));  // Allocation!
    Ok(path)
}
```

**Problems:**
1. `Vec::new()` allocation per path
2. `name.clone()` per component
3. `components.join()` creates new String
4. HashMap lookup overhead vs direct indexing

### 2.3 MFT Reading (Current)

```rust
// Rust parses into intermediate ParsedRecord, then converts to columns
pub fn parse_record_full(data: &mut [u8], frs: u64) -> ParseResult {
    let record = ParsedRecord {
        frs,
        parent_frs: ...,
        name: String::from_utf16(&name_u16)?,  // Allocation!
        names: Vec::new(),                      // Allocation!
        streams: Vec::new(),                    // Allocation!
        ...
    };
    ParseResult::Base(record)
}

// Then converted to columns (another pass)
pub fn push_record(&mut self, record: &ParsedRecord) {
    self.frs.push(record.frs);
    self.name.push(record.name.clone());  // Clone!
    // ... 28 more pushes
}
```

**Problems:**
1. Two-pass processing (parse → convert)
2. Intermediate `ParsedRecord` allocation
3. String cloning during column push
4. 28 separate `Vec::push()` calls per record

---

## Part 3: Performance Comparison

| Aspect | C++ Approach | Rust Approach | Impact |
|--------|--------------|---------------|--------|
| **FRS Lookup** | `vector[frs]` O(1) | `HashMap::get(frs)` O(1)* | ~3-5x slower |
| **Name Storage** | Single buffer + offset | Separate String per name | ~10x more allocations |
| **Path Building** | Append-reverse in-place | Vec + clone + join | ~5x slower |
| **Record Parsing** | Inline during read | Separate parse pass | ~2x slower |
| **Memory Layout** | Packed structs (1-byte align) | Standard alignment | ~2x memory |
| **Attribute Flags** | Bit-packed u32 | 18 separate bools | ~18x memory for flags |

*HashMap has O(1) average but with significant constant factor vs array indexing.

---

## Part 4: Proposed Optimizations for Rust

> **Note:** The optimizations below are organized by which layer they apply to.
> The MFT reading layer (`uffs-mft`) is already highly optimized.
> Most remaining gains come from the post-processing layer (`uffs-core`).

### Layer Summary

| Layer | Status | Key Optimizations Needed |
|-------|--------|--------------------------|
| **uffs-mft** (MFT Reading) | ✅ **DONE** | Already 55% faster, 4x throughput |
| **uffs-core** (Path Resolution) | ❌ **BROKEN** | NameArena, Vec-based lookup, in-place paths |
| **uffs-core** (Pattern Matching) | ⚠️ **BASIC** | SIMD, early termination, extension index |
| **uffs-cli** (Output) | ⚠️ **BASIC** | Streaming output, buffered writes |

---

### 4.1 Arena-Based Name Storage (HIGH IMPACT) — `uffs-core`

Replace individual String allocations with a single arena:

```rust
pub struct NameArena {
    buffer: String,  // All names concatenated
}

pub struct NameRef {
    offset: u32,     // Offset into arena
    length: u16,     // Name length
    is_ascii: bool,  // ASCII compression flag
}

impl NameArena {
    pub fn push(&mut self, name: &str) -> NameRef {
        let offset = self.buffer.len() as u32;
        self.buffer.push_str(name);
        NameRef { offset, length: name.len() as u16, is_ascii: name.is_ascii() }
    }

    pub fn get(&self, r: NameRef) -> &str {
        &self.buffer[r.offset as usize..(r.offset as usize + r.length as usize)]
    }
}
```

**Expected impact:** 10x fewer allocations, better cache locality.

### 4.2 Vector-Based FRS Lookup (HIGH IMPACT) — `uffs-core`

Replace HashMap with direct vector indexing:

```rust
pub struct FastPathResolver {
    // Index = FRS, Value = (parent_frs, name_ref)
    entries: Vec<Option<(u32, NameRef)>>,  // Use u32 for parent (like C++)
    names: NameArena,
    volume: char,
}

impl FastPathResolver {
    pub fn get(&self, frs: u64) -> Option<(u32, NameRef)> {
        self.entries.get(frs as usize).copied().flatten()
    }
}
```

**Expected impact:** 3-5x faster lookups.

### 4.3 In-Place Path Building (HIGH IMPACT) — `uffs-core`

Build paths without intermediate allocations:

```rust
pub fn resolve_into(&self, frs: u64, buffer: &mut String) -> Result<()> {
    let start_len = buffer.len();
    let mut current = frs;

    // Append in reverse order (like C++)
    while current != 0 && current != 5 {
        if let Some((parent, name_ref)) = self.get(current) {
            buffer.push('\\');
            buffer.push_str(self.names.get(name_ref));
            current = parent as u64;
        } else {
            return Err(CoreError::PathResolution(current));
        }
    }

    // Write drive prefix at the end, then reverse
    buffer.push(':');
    buffer.push(self.volume);

    // Reverse only the new portion
    unsafe {
        let bytes = buffer.as_bytes_mut();
        bytes[start_len..].reverse();
    }

    Ok(())
}
```

**Expected impact:** 5x faster path resolution.

### 4.4 Zero-Copy MFT Parsing (MEDIUM IMPACT) — `uffs-mft` ✅ ALREADY DONE

> **Status:** This optimization is already implemented as `read_all_parallel_to_columns()`
> in `uffs-mft/io.rs`. The `ParsedColumns` struct provides direct-to-column parsing.

Parse directly into columnar format without intermediate structs:

```rust
pub struct DirectParser<'a> {
    data: &'a [u8],
    columns: &'a mut ParsedColumns,
    names: &'a mut NameArena,
}

impl<'a> DirectParser<'a> {
    pub fn parse_record(&mut self, frs: u64) {
        // Parse directly into columns - no intermediate ParsedRecord
        self.columns.frs.push(frs);

        // Parse $STANDARD_INFORMATION directly into timestamp columns
        if let Some(si) = self.find_standard_info() {
            self.columns.created.push(si.created);
            self.columns.modified.push(si.modified);
            // ...
        }

        // Parse $FILE_NAME directly into name arena
        if let Some(fn_attr) = self.find_best_filename() {
            let name_ref = self.names.push(&fn_attr.name);
            self.columns.name_ref.push(name_ref);
            self.columns.parent_frs.push(fn_attr.parent);
        }
    }
}
```

**Expected impact:** 2x fewer allocations, better cache utilization.

### 4.5 SIMD Pattern Matching (MEDIUM IMPACT) — `uffs-core`

Use SIMD for wildcard pattern matching:

```rust
use std::arch::x86_64::*;

pub fn wildcard_match_simd(pattern: &[u8], text: &[u8]) -> bool {
    // Use AVX2 for 32-byte parallel comparison
    // Compare 32 characters at once for common prefix/suffix patterns
    unsafe {
        let pattern_vec = _mm256_loadu_si256(pattern.as_ptr() as *const __m256i);
        let text_vec = _mm256_loadu_si256(text.as_ptr() as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(pattern_vec, text_vec);
        _mm256_movemask_epi8(cmp) == -1
    }
}
```

**Expected impact:** 4-8x faster for simple patterns like `*.txt`.

### 4.6 Bit-Packed Attributes (LOW IMPACT) — `uffs-mft` ✅ ALREADY DONE

> **Status:** The MFT reader already uses `FileAttributes` bitflags from the raw NTFS data.
> The `ParsedColumns` struct stores attributes efficiently.

Pack all 18 boolean flags into a single u32:

```rust
bitflags::bitflags! {
    pub struct FileAttributes: u32 {
        const READONLY     = 0x0001;
        const HIDDEN       = 0x0002;
        const SYSTEM       = 0x0004;
        const DIRECTORY    = 0x0010;
        const ARCHIVE      = 0x0020;
        const COMPRESSED   = 0x0800;
        const ENCRYPTED    = 0x4000;
        const SPARSE       = 0x0200;
        // ... etc
    }
}

// Store single u32 instead of 18 bools
pub struct CompactRecord {
    pub frs: u64,
    pub parent_frs: u32,      // u32 sufficient (like C++)
    pub name_ref: NameRef,    // 8 bytes
    pub size: u64,
    pub attributes: FileAttributes,  // 4 bytes instead of 18 bytes
    pub timestamps: [i64; 4], // Inline array
}
// Total: ~56 bytes vs current ~200+ bytes
```

**Expected impact:** 3-4x less memory, better cache efficiency.

### 4.7 Pipelined I/O with Inline Processing — `uffs-mft` ⚠️ PARTIALLY DONE

> **Status:** We already have `PrefetchMftReader` with double-buffering, which provides
> **some** I/O overlap. However, it's not true pipelining (reads are still sequential,
> just with prefetch). The current implementation is already very fast on SSDs (1,839 MB/s).
> True pipelining would mainly help HDDs where I/O dominates.

#### The C++ Pattern: Overlapped I/O with IOCP

The C++ code uses Windows I/O Completion Ports (IOCP) with a critical pattern:

```cpp
// Line 7245-7250 in UltraFastFileSearch.cpp
int ReadOperation::operator()(size_t const size, ...) {
    this->q->queue_next();  // IMMEDIATELY queue the NEXT read!
    void* const buffer = this + 1;
    // ... then process the current buffer while next read is in flight
}
```

This creates a **pipeline** where I/O and CPU work overlap:

```
C++ (Pipelined - I/O and CPU overlap):
Read 1 ──────▶
     Parse 1 ──────▶
     Read 2 ──────▶
          Parse 2 ──────▶
          Read 3 ──────▶
               Parse 3 ──────▶
─────────────────────────────────▶ Time: max(I/O, CPU)

Rust PrefetchMftReader (Current - Double-buffered but sequential):
Read 1 ──────▶ Parse 1 ──────▶ Read 2 ──────▶ Parse 2 ──────▶
───────────────────────────────────────────────────────────▶ Time: I/O + CPU (but with prefetch)
```

#### What We Already Have

The `PrefetchMftReader` in `uffs-mft/io.rs` already implements:
- Double-buffering (buffer A and buffer B)
- Prefetch of next chunk while processing current
- Drive-type aware chunk sizing (4MB HDD, 8MB SSD)

#### Why It's Not Full Pipelining

The current implementation alternates buffers but doesn't truly overlap I/O and CPU:
```rust
// Current PrefetchMftReader (simplified)
for chunk in chunks {
    let buffer = if use_buffer_a { &mut buffer_a } else { &mut buffer_b };
    let bytes_read = self.read_chunk_into_buffer(handle, &chunk, record_size, buffer)?;
    // Process records AFTER read completes
    for record in buffer.records() {
        parse_record(record);
    }
    use_buffer_a = !use_buffer_a;
}
```

True pipelining would spawn a reader thread that queues reads while the main thread parses.

#### Impact Assessment

| Scenario | Current (Prefetch) | True Pipeline | Improvement |
|----------|-------------------|---------------|-------------|
| SSD (I/O << CPU) | 3.1s | ~3.0s | ~3% (minimal) |
| HDD (I/O >> CPU) | 23.3s | ~18s | ~20% (moderate) |
| HDD (I/O ≈ CPU) | 45.9s | ~30s | ~35% (significant) |

**Recommendation:** True pipelining is a **nice-to-have** for HDD optimization, but the
current `PrefetchMftReader` is already very effective. Focus on path resolution first.

#### Proposed Rust Implementation

Use `crossbeam` channels for a bounded pipeline:

```rust
use crossbeam::channel::{bounded, Sender, Receiver};
use std::thread;

pub struct PipelinedMftReader {
    buffer_count: usize,  // Number of in-flight buffers (e.g., 4)
    buffer_size: usize,   // Size per buffer (e.g., 1MB)
}

impl PipelinedMftReader {
    pub fn read_and_parse(&self, volume: &VolumeHandle) -> Result<ParsedColumns> {
        // Bounded channel creates backpressure
        let (tx, rx): (Sender<ReadBuffer>, Receiver<ReadBuffer>) =
            bounded(self.buffer_count);

        // Reader thread - queues reads as fast as possible
        let reader_handle = thread::spawn(move || {
            for chunk in mft_chunks {
                let buffer = read_chunk(&volume, chunk);
                tx.send(ReadBuffer { vcn: chunk.vcn, data: buffer }).unwrap();
            }
        });

        // Parser thread(s) - process as buffers arrive
        let mut columns = ParsedColumns::new();
        while let Ok(buffer) = rx.recv() {
            parse_records_inline(&buffer.data, buffer.vcn, &mut columns);
        }

        reader_handle.join().unwrap();
        Ok(columns)
    }
}
```

#### Advanced: Multiple Reader Threads for Fragmented MFT

For fragmented MFTs, spawn one reader per extent:

```rust
pub fn read_fragmented_parallel(&self, extents: &[MftExtent]) -> Result<ParsedColumns> {
    let (tx, rx) = bounded(self.buffer_count * extents.len());

    // One reader thread per extent (parallel I/O)
    let handles: Vec<_> = extents.iter().map(|extent| {
        let tx = tx.clone();
        let extent = extent.clone();
        thread::spawn(move || {
            for chunk in extent.chunks(BUFFER_SIZE) {
                let buffer = read_chunk(&extent.volume, chunk);
                tx.send(ReadBuffer { vcn: chunk.vcn, data: buffer }).unwrap();
            }
        })
    }).collect();

    drop(tx);  // Close sender so receiver knows when done

    // Single parser thread (or use Rayon for parallel parsing)
    let mut columns = ParsedColumns::new();
    while let Ok(buffer) = rx.recv() {
        parse_records_inline(&buffer.data, buffer.vcn, &mut columns);
    }

    for h in handles { h.join().unwrap(); }
    Ok(columns)
}
```

#### Memory Advantage

| Approach | Memory for 1M files |
|----------|---------------------|
| C++ (pipelined) | ~4MB (4 × 1MB buffers) |
| Rust (current) | ~1GB (entire MFT in memory) |
| Rust (pipelined) | ~4MB (4 × 1MB buffers) |

**250x memory reduction!**

**Expected impact:**
- 2x faster total time (I/O + CPU overlap)
- 250x less memory usage
- Streaming results (time to first result: ~10ms vs ~30s)

---

## Part 5: Implementation Roadmap (Revised)

> **Key Insight:** The MFT reading layer (`uffs-mft`) is already highly optimized.
> The roadmap below focuses on the **remaining bottlenecks** in `uffs-core`.

### What's Already Done (uffs-mft) ✅

| Optimization | Status | Impact |
|--------------|--------|--------|
| Bitmap skip optimization | ✅ Done | 20% faster |
| Rayon fold/reduce | ✅ Done | 5-15% parse time |
| SoA layout (ParsedColumns) | ✅ Done | 90% df_build reduction |
| PrefetchMftReader | ✅ Done | HDD optimization |
| Drive-type aware tuning | ✅ Done | Optimal I/O |
| Fast path (skip extensions) | ✅ Done | 18% total |

**Result:** MFT reading is 55% faster, 4x throughput on SSDs.

---

### Phase 0: Benchmark Infrastructure (Before Starting) 📊
**Goal:** Establish baseline measurements and automated comparison tools.
**Layer:** All layers + `justfile`

#### Deliverables

1. **Integration benchmarks** (`just bench-vs-cpp`)
   - Compare Rust vs C++ (`~/bin/uffs.com`) for `*.txt` search
   - Measure time, file count, and output correctness
   - Run on multiple drives (SSD + HDD)

2. **Criterion micro-benchmarks** (fill in placeholders)
   - `uffs-mft/benches/mft_read.rs` - Real MFT reading benchmarks
   - `uffs-core/benches/query.rs` - Real query benchmarks
   - Add `resolve_path()` benchmark to `search_benchmarks.rs`

3. **End-to-end search benchmark** (`just bench-search`)
   - Full pipeline: MFT Read → Path Resolution → Pattern Match → Output
   - Use cached MFT file for reproducibility
   - Track regression over time

4. **Baseline tracking**
   - Store baseline results in `benchmarks/baseline.json`
   - Compare against baseline on each run
   - Alert on regression > 10%

#### Just Commands

```bash
# Compare Rust vs C++ (integration benchmark)
just bench-vs-cpp

# Run criterion micro-benchmarks
just bench-micro

# Run end-to-end search benchmark
just bench-search

# Run all benchmarks and compare to baseline
just bench-all-compare

# Update baseline with current results
just bench-update-baseline
```

---

### Phase 1: Fix Path Resolution (Week 1-2) ⭐ CRITICAL
**Goal:** Fix the broken path resolution that causes `<unknown>` paths.
**Layer:** `uffs-core`

1. **Build PathResolver from FULL MFT data** - Not filtered data
2. **Implement FastPathResolver** - Vec-based O(1) lookup
3. **Implement NameArena** - Single buffer for all names
4. **Implement in-place path building** - Append-reverse strategy

**Benchmark targets:**
- `just bench-vs-cpp` should show file count matching C++
- `just bench-micro` should show path resolution 3-5x faster

### Phase 2: Query Optimization (Week 3-4)
**Goal:** Accelerate pattern matching.
**Layer:** `uffs-core`

1. **SIMD pattern matching** - AVX2 for `*.ext` patterns
2. **Early termination** - Stop when limit reached
3. **Extension index** - Pre-built extension → FRS mapping
4. **Parallel path resolution** - Rayon for multi-threaded resolution

**Benchmark targets:**
- `just bench-vs-cpp` should show Rust 2x faster than the legacy baseline
- `just bench-micro` should show pattern matching 4-8x faster

### Phase 3: Output Optimization (Week 5-6)
**Goal:** Efficient result streaming.
**Layer:** `uffs-cli`

1. **Streaming output** - Write results as they're found
2. **Buffered writes** - Reduce syscall overhead
3. **Format optimization** - Avoid unnecessary allocations

**Benchmark targets:**
- `just bench-vs-cpp` should show Rust 4x faster than the legacy baseline
- `just bench-search` should show first result in <100ms

### Phase 4: True Pipelining for HDD Optimization (Week 7-8) ✅ COMPLETE
**Goal:** Overlap I/O and CPU work for additional HDD speedup.
**Layer:** `uffs-mft` (enhancement)
**Expected Impact:** ~20-35% additional speedup on HDDs

> **Implementation:** `PipelinedMftReader` uses `crossbeam-channel` bounded channels
> to overlap I/O and CPU work. Reader thread queues chunks while parser thread processes.
> Auto mode now selects `Pipelined` for HDDs instead of `Prefetch`.

#### Current vs True Pipelining

```
Current PrefetchMftReader (sequential with prefetch):
Read A ──────▶ Parse A ──────▶ Read B ──────▶ Parse B ──────▶
───────────────────────────────────────────────────────────▶ Time: I/O + CPU

True Pipelining (overlapped I/O and CPU):
Read A ──────▶
     Parse A ──────▶
     Read B ──────▶
          Parse B ──────▶
               Read C ──────▶
─────────────────────────────────▶ Time: max(I/O, CPU)
```

#### Deliverables

1. **Channel-based pipeline** - `crossbeam` bounded channels for backpressure
2. **Reader thread** - Dedicated thread queuing reads as fast as possible
3. **Parser thread(s)** - Process buffers as they arrive (can use Rayon)
4. **Overlapped I/O** - Windows `FILE_FLAG_OVERLAPPED` for true async
5. **Multi-extent parallelism** - One reader per MFT extent for fragmented MFTs

#### Expected HDD Impact

| HDD Scenario | Current (Prefetch) | True Pipeline | Improvement |
|--------------|-------------------|---------------|-------------|
| I/O >> CPU (slow HDD) | 45.9s | ~40s | ~13% |
| I/O ≈ CPU (balanced) | 30s | ~20s | ~33% |
| I/O << CPU (fast HDD) | 25s | ~22s | ~12% |

**Note:** SSD impact is minimal (~3%) since I/O is already faster than CPU parsing.

---

## Part 6: Expected Performance Gains (Revised)

### For Full Search Pipeline (MFT Read → Path Resolution → Pattern Match → Output)

| Optimization | Layer | Status | Expected Speedup |
|--------------|-------|--------|------------------|
| MFT Reading | uffs-mft | ✅ **DONE** | Already 55% faster |
| PrefetchMftReader | uffs-mft | ✅ **DONE** | HDD: 71% faster |
| Fix PathResolver | uffs-core | ✅ **DONE** | 1.0x (correctness fix) |
| FastPathResolver (Vec) | uffs-core | ✅ **DONE** | 3-5x path resolution |
| NameArena | uffs-core | ✅ **DONE** | 2-3x fewer allocations |
| Parallel path resolution | uffs-core | ✅ **DONE** | Rayon for >10K rows |
| ExtensionIndex | uffs-core | ✅ **DONE** | Fast `*.ext` queries |
| SIMD pattern matching | uffs-core | ⏳ Deferred | 4-8x pattern matching |
| Early termination | uffs-core | ✅ **DONE** | Streaming mode has early termination |
| Streaming output | uffs-cli | ✅ **DONE** | 1.5-2x output |
| Format optimization | uffs-cli | ✅ **DONE** | Cached cols, reusable buffers |
| **True Pipelining** | uffs-mft | ✅ **DONE** | HDD: 20-35% additional |

### Realistic Targets

| Metric | Current | After Phase 1-2 | After Phase 3 | After Phase 4 (HDD) |
|--------|---------|-----------------|---------------|---------------------|
| `*.txt` search (SSD C:) | ~56s | ~20s | ~10s | ~10s (no change) |
| `*.txt` search (HDD S:) | ~90s | ~50s | ~35s | **~25s** |
| Files found | Broken | 735K (matches the legacy baseline) | 735K | 735K |
| vs C++ (43.6s) | 1.3x slower | 2x faster | **4x faster** | **4x faster** |

### Memory Usage

| Component | Current | Optimized |
|-----------|---------|-----------|
| MFT DataFrame | ~500MB | ~500MB (unchanged) |
| PathResolver | ~200MB (HashMap) | ~50MB (Vec + Arena) |
| Pattern Matcher | ~10MB | ~10MB |
| **Total** | ~710MB | ~560MB |

> **Note:** Memory reduction from pipelining (250x) only applies if we switch to
> streaming mode. The current approach loads the full MFT into a DataFrame, which
> is fine for most use cases and enables powerful queries.

---

## Part 7: Architecture Diagram

### Current Architecture (Sequential)

```
┌─────────────────────────────────────────────────────────────────────┐
│                     CURRENT RUST ARCHITECTURE                        │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Phase 1: Read ALL MFT data into memory                             │
│  ════════════════════════════════════════════════════════▶          │
│                                                                      │
│                          Phase 2: Parse ALL records                  │
│                          ══════════════════════════════════════════▶│
│                                                                      │
│  Total Time = I/O Time + Parse Time                                 │
│  Memory = Entire MFT (~1GB for large drives)                        │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Proposed Architecture (Pipelined)

```
┌─────────────────────────────────────────────────────────────────────┐
│                     PROPOSED RUST ARCHITECTURE                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                    PIPELINED I/O + PARSING                   │   │
│  ├─────────────────────────────────────────────────────────────┤   │
│  │                                                              │   │
│  │  Reader Thread(s)          Bounded Channel    Parser Thread  │   │
│  │  ┌──────────────┐         ┌──────────┐      ┌─────────────┐ │   │
│  │  │ Read Chunk 1 │────────▶│ Buffer 1 │─────▶│ Parse Chunk │ │   │
│  │  │ Read Chunk 2 │────────▶│ Buffer 2 │─────▶│ Parse Chunk │ │   │
│  │  │ Read Chunk 3 │────────▶│ Buffer 3 │─────▶│ Parse Chunk │ │   │
│  │  │     ...      │────────▶│ Buffer 4 │─────▶│     ...     │ │   │
│  │  └──────────────┘         └──────────┘      └─────────────┘ │   │
│  │        │                       │                   │         │   │
│  │        │    OVERLAP: I/O and CPU work in parallel  │         │   │
│  │        ▼                       ▼                   ▼         │   │
│  │  ══════════════════════════════════════════════════════════ │   │
│  │  Read 1 ──▶ Parse 1 ──▶                                      │   │
│  │       Read 2 ──▶ Parse 2 ──▶                                 │   │
│  │            Read 3 ──▶ Parse 3 ──▶                            │   │
│  │                 Read 4 ──▶ Parse 4 ──▶                       │   │
│  │  ══════════════════════════════════════════════════════════ │   │
│  │                                                              │   │
│  │  Total Time = max(I/O Time, Parse Time)  ← 2x faster!        │   │
│  │  Memory = 4 × 1MB buffers = 4MB          ← 250x less!        │   │
│  │                                                              │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                    │                                │
│                                    ▼                                │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐  │
│  │  NameArena   │    │FastPathResolver│   │   Columnar Storage   │  │
│  │ (contiguous) │    │(Vec<Option<>>)│   │   (SoA + Arena)      │  │
│  └──────────────┘    └──────────────┘    └──────────────────────┘  │
│                                                     │               │
│                                                     ▼               │
│                              ┌──────────────────────────────────┐  │
│                              │      SIMD Pattern Matcher        │  │
│                              │   (AVX2 wildcard acceleration)   │  │
│                              └──────────────────────────────────┘  │
│                                                     │               │
│                                                     ▼               │
│                              ┌──────────────────────────────────┐  │
│                              │       Streaming Output           │  │
│                              │   (zero-copy path formatting)    │  │
│                              └──────────────────────────────────┘  │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Part 8: Key Takeaways (Revised)

### What's Already Done ✅

The MFT reading layer (`uffs-mft`) is already highly optimized:
1. **55% faster** overall (315s → 142s for 7 drives)
2. **4x throughput** on SSDs (400-550 → 1,472-1,839 MB/s)
3. **SoA layout** - Direct-to-columns parsing (ParsedColumns)
4. **PrefetchMftReader** - Double-buffered I/O with prefetch
5. **Bitmap optimization** - Skips ~33% of unused records
6. **Drive-type aware** - Auto-selects optimal read mode

### What Still Needs Work ❌

The post-processing layer (`uffs-core`) is the bottleneck:
1. **Path resolution is broken** - Builds from filtered data, causes `<unknown>` paths
2. **HashMap-based lookup** - O(1) average but poor cache locality
3. **String allocations** - Each path is a new allocation
4. **No SIMD** - Pattern matching is scalar
5. **No early termination** - Scans all files even for `--limit 10`

### Why C++ is Fast

1. **Contiguous memory** - All data in vectors, not scattered heap allocations
2. **Direct indexing** - `vector[frs]` instead of hash lookups
3. **Packed structures** - Minimal memory footprint, cache-friendly
4. **In-place operations** - Reuse buffers, avoid allocations
5. **Pipelined I/O** - Overlapped I/O with IOCP (we have PrefetchMftReader)

### How Rust Can Be Faster

1. **Fix path resolution** ⭐ - The #1 priority (correctness + performance)
2. **Vec-based lookup** - O(1) with perfect cache locality
3. **NameArena** - Single buffer for all names
4. **SIMD pattern matching** - AVX2 for `*.ext` patterns
5. **Early termination** - Stop when limit reached
6. **Rayon parallelism** - Already used in MFT reader, extend to path resolution

### The Single Most Important Optimization

**Fix Path Resolution** is the single most impactful optimization because:
- It's currently **broken** (produces `<unknown>` paths)
- It's the **bottleneck** for the full search pipeline
- Vec-based lookup provides **3-5x speedup** over HashMap
- NameArena provides **2-3x fewer allocations**
- Combined with SIMD, we can achieve **4x faster than the legacy baseline**

> **Note:** Pipelined I/O was previously highlighted as critical, but the current
> `PrefetchMftReader` already provides most of the benefit. True pipelining would
> only help HDD-bound workloads where I/O dominates CPU time.

---

## Appendix A: Benchmark Commands

### PowerShell (Windows)

```powershell
# Build release with optimizations
$env:RUSTFLAGS="-C target-cpu=native"
cargo build --release -p uffs-cli

# Copy to bin folder for easy access
Copy-Item target/release/uffs.exe $HOME/bin/uffs.exe

# Simple timing comparison
Measure-Command { & uffs.com "*.txt" | Out-Null } | Select-Object TotalSeconds
Measure-Command { & $HOME/bin/uffs.exe "*.txt" | Out-Null } | Select-Object TotalSeconds

# Multiple runs with statistics (PowerShell function)
function Benchmark-Command {
    param([string]$Command, [int]$Runs = 5)
    $times = @()
    for ($i = 1; $i -le $Runs; $i++) {
        Write-Host "Run $i of $Runs..." -NoNewline
        $elapsed = (Measure-Command { Invoke-Expression $Command | Out-Null }).TotalSeconds
        $times += $elapsed
        Write-Host " $([math]::Round($elapsed, 2))s"
    }
    $avg = ($times | Measure-Object -Average).Average
    $min = ($times | Measure-Object -Minimum).Minimum
    $max = ($times | Measure-Object -Maximum).Maximum
    Write-Host "`nResults: avg=$([math]::Round($avg, 2))s min=$([math]::Round($min, 2))s max=$([math]::Round($max, 2))s"
}

# Usage:
Benchmark-Command 'uffs.com "*.txt"' -Runs 5
Benchmark-Command '$HOME/bin/uffs.exe "*.txt"' -Runs 5

# Compare output correctness
$env:RUST_LOG="error"
& uffs.com "*.txt" > output_cpp.txt 2>&1
& $HOME/bin/uffs.exe "*.txt" > output_rust.txt 2>&1
Compare-Object (Get-Content output_cpp.txt) (Get-Content output_rust.txt)
```

### Python Benchmark Script (Cross-Platform)

Save as `scripts/benchmark.py`:

```python
#!/usr/bin/env python3
"""
UFFS Benchmark Script - Compare C++ and Rust implementations
Usage: python scripts/benchmark.py "*.txt" --runs 5
"""

import subprocess
import time
import statistics
import argparse
import sys
from pathlib import Path

def run_command(cmd: list[str], capture_output: bool = True) -> tuple[float, int]:
    """Run command and return (elapsed_seconds, line_count)."""
    start = time.perf_counter()
    result = subprocess.run(cmd, capture_output=capture_output, text=True)
    elapsed = time.perf_counter() - start
    line_count = len(result.stdout.splitlines()) if capture_output else 0
    return elapsed, line_count

def benchmark(cmd: list[str], runs: int = 5, warmup: int = 1) -> dict:
    """Run benchmark with warmup and multiple runs."""
    # Warmup runs
    for _ in range(warmup):
        run_command(cmd, capture_output=False)

    # Timed runs
    times = []
    for i in range(runs):
        elapsed, lines = run_command(cmd)
        times.append(elapsed)
        print(f"  Run {i+1}/{runs}: {elapsed:.3f}s ({lines:,} lines)")

    return {
        'mean': statistics.mean(times),
        'median': statistics.median(times),
        'stdev': statistics.stdev(times) if len(times) > 1 else 0,
        'min': min(times),
        'max': max(times),
        'runs': times
    }

def main():
    parser = argparse.ArgumentParser(description='UFFS Benchmark')
    parser.add_argument('pattern', help='Search pattern (e.g., "*.txt")')
    parser.add_argument('--runs', type=int, default=5, help='Number of runs')
    parser.add_argument('--warmup', type=int, default=1, help='Warmup runs')
    parser.add_argument('--cpp', default='uffs.com', help='C++ executable')
    parser.add_argument('--rust', default='uffs.exe', help='Rust executable')
    args = parser.parse_args()

    print(f"Benchmarking pattern: {args.pattern}")
    print(f"Runs: {args.runs}, Warmup: {args.warmup}\n")

    # Benchmark C++
    print(f"C++ ({args.cpp}):")
    cpp_results = benchmark([args.cpp, args.pattern], args.runs, args.warmup)

    print(f"\nRust ({args.rust}):")
    rust_results = benchmark([args.rust, args.pattern], args.runs, args.warmup)

    # Summary
    print("\n" + "="*60)
    print("SUMMARY")
    print("="*60)
    print(f"{'Metric':<20} {'C++':<15} {'Rust':<15} {'Ratio':<10}")
    print("-"*60)

    ratio = rust_results['mean'] / cpp_results['mean']
    print(f"{'Mean':<20} {cpp_results['mean']:.3f}s{'':<8} {rust_results['mean']:.3f}s{'':<8} {ratio:.2f}x")
    print(f"{'Median':<20} {cpp_results['median']:.3f}s{'':<8} {rust_results['median']:.3f}s")
    print(f"{'Min':<20} {cpp_results['min']:.3f}s{'':<8} {rust_results['min']:.3f}s")
    print(f"{'Max':<20} {cpp_results['max']:.3f}s{'':<8} {rust_results['max']:.3f}s")
    print(f"{'Stdev':<20} {cpp_results['stdev']:.3f}s{'':<8} {rust_results['stdev']:.3f}s")

    if ratio > 1:
        print(f"\n⚠️  Rust is {ratio:.2f}x SLOWER than C++")
    else:
        print(f"\n✅ Rust is {1/ratio:.2f}x faster than the legacy baseline")

if __name__ == '__main__':
    main()
```

### Usage Examples

```powershell
# Run Python benchmark
python scripts/benchmark.py "*.txt" --runs 5

# Quick comparison
python scripts/benchmark.py "*.rs" --runs 3 --cpp uffs.com --rust $HOME/bin/uffs.exe

# Memory profiling (requires Windows Performance Toolkit)
# Or use Process Explorer to monitor memory during execution
```

### Memory Profiling (Windows)

```powershell
# Using Windows Performance Recorder (WPR) - requires Windows SDK
wpr -start GeneralProfile
& $HOME/bin/uffs.exe "*.txt" | Out-Null
wpr -stop uffs_trace.etl

# Or use simple PowerShell memory monitoring
$proc = Start-Process -FilePath "$HOME/bin/uffs.exe" -ArgumentList '"*.txt"' -PassThru -NoNewWindow
while (!$proc.HasExited) {
    $mem = $proc.WorkingSet64 / 1MB
    Write-Host "Memory: $([math]::Round($mem, 1)) MB"
    Start-Sleep -Milliseconds 500
}
Write-Host "Peak Memory: $([math]::Round($proc.PeakWorkingSet64 / 1MB, 1)) MB"
```

### Profiling with Tracy (Recommended for Rust)

```powershell
# Install Tracy profiler
# https://github.com/wolfpld/tracy

# Build with Tracy support (add to Cargo.toml)
# [dependencies]
# tracy-client = { version = "0.16", features = ["enable"] }

# Run with Tracy capture
tracy-capture -o uffs_trace.tracy -f & $HOME/bin/uffs.exe "*.txt"

# Open trace in Tracy GUI
tracy uffs_trace.tracy
```

## Appendix B: References

- [C++ Source (local-only `old_cpp_reference/` tree)](../../old_cpp_reference/uffs/UltraFastFileSearch-code/UltraFastFileSearch.cpp)
- [Rust MFT Reader](../../crates/uffs-mft/src/reader.rs)
- [Rust I/O Implementation](../../crates/uffs-mft/src/io.rs)
- [Path Resolver](../../crates/uffs-core/src/path_resolver.rs)

