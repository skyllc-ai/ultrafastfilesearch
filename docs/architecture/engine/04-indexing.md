# In-Memory Index (`MftIndex`)

## Introduction

This document provides exhaustive detail on how UFFS builds and maintains its in-memory file index. After reading this document, you should be able to:

1. Understand the `MftIndex` data structures and their memory layout
2. Implement FRS-to-record lookup, name resolution, and hard link chains
3. Build parent-child relationships from MFT records
4. Compute tree metrics (treesize, descendants) in O(n) without recursion
5. Resolve full paths from FRS numbers efficiently

---

## Overview: Index Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          MftIndex                                        │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │ records: Vec<FileRecord>  (224 bytes each, contiguous)             │  │
│  │  [0]: frs=0 ($MFT), stdinfo, first_name, first_stream, ...        │  │
│  │  [1]: frs=5 (root), first_child→children[0], descendants=14M      │  │
│  │  [2]: frs=42, "Users", directory, treesize=80GB                    │  │
│  │  [3]: frs=1234, "readme.txt", file, size=4096                     │  │
│  │  ...                                                               │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │ frs_to_idx: Vec<u32>  (sparse array, indexed by FRS)               │  │
│  │  frs_to_idx[0] = 0,  frs_to_idx[5] = 1,  frs_to_idx[42] = 2      │  │
│  │  frs_to_idx[3] = NO_ENTRY (deleted record, no index entry)        │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌────────────────────────────────────────────────────────────────────┐  │
│  │ names: String  (all filenames concatenated, UTF-8)                  │  │
│  │  "$MFT\0root\0Users\0readme.txt\0Desktop\0..."                     │  │
│  │  ^0    ^4    ^9     ^14          ^24                                │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌──────────────┐  ┌────────────────────┐  ┌────────────────────────┐   │
│  │ links:       │  │ streams:           │  │ children:              │   │
│  │ Vec<LinkInfo>│  │ Vec<IndexStream>   │  │ Vec<ChildInfo>         │   │
│  │ (hardlinks)  │  │ (ADS overflow)     │  │ (dir contents)        │   │
│  └──────────────┘  └────────────────────┘  └────────────────────────┘   │
│                                                                          │
│  ┌──────────────┐  ┌────────────────────┐                               │
│  │ extensions:  │  │ extension_index:   │                               │
│  │ ExtTable     │  │ Option<ExtIndex>   │                               │
│  │ (interned)   │  │ (O(1) *.ext lookup)│                               │
│  └──────────────┘  └────────────────────┘                               │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Core Data Structures

### MftIndex Container

**Source:** `index/model.rs`

```rust
pub struct MftIndex {
    pub volume: char,                           // Drive letter ('C')
    pub records: Vec<FileRecord>,               // All file/dir records
    pub frs_to_idx: Vec<u32>,                   // FRS → records index (O(1))
    pub names: String,                          // Concatenated filenames
    pub links: Vec<LinkInfo>,                   // Overflow hard links
    pub streams: Vec<IndexStreamInfo>,          // Overflow ADS
    pub internal_streams: Vec<InternalStreamInfo>, // Internal NTFS streams
    pub children: Vec<ChildInfo>,               // Directory children
    pub stats: MftStats,                        // Parsing statistics
    pub extensions: ExtensionTable,             // File extension interning
    pub extension_index: Option<ExtensionIndex>, // O(1) *.ext lookup
    pub forensic_mode: bool,                    // Include deleted/corrupt
    pub reserved_allocated_bytes: u64,          // NTFS reserved clusters
}
```

### FileRecord (224 bytes)

**Source:** `index/types.rs`

The core per-file/directory record. Designed for cache-friendly sequential scan:

```rust
#[repr(C)]
pub struct FileRecord {
    // ===== Identity (18 bytes) =====
    pub frs: u64,                    // Primary key (MFT record number)
    pub sequence_number: u16,        // Incremented on FRS reuse
    pub namespace: u8,               // 0=POSIX, 1=Win32, 2=DOS, 3=Both
    pub forensic_flags: u8,          // Bit-packed: deleted|corrupt|extension|
                                     //   has_default_data|has_i30|is_unified

    // ===== Forensic/linking (20 bytes) =====
    pub lsn: u64,                    // Log File Sequence Number
    pub reparse_tag: u32,            // Symlink/junction type (0 if none)
    pub base_frs: u64,               // Base FRS for extensions (0 for base)

    // ===== Timestamps & Attributes (48 bytes) =====
    pub stdinfo: StandardInfo,       // 4 timestamps + flags + USN + IDs

    // ===== Counts (6 bytes) =====
    pub name_count: u16,             // Hard link count (usually 1)
    pub stream_count: u16,           // User-visible streams (usually 1)
    pub total_stream_count: u16,     // All streams (including internal)

    // ===== Linked list heads (8 bytes) =====
    pub first_internal_stream: u32,  // → internal_streams[], or NO_ENTRY
    pub first_child: u32,            // → children[], or NO_ENTRY

    // ===== Inline primary data (53 bytes) =====
    pub first_name: LinkInfo,        // Primary filename (24 bytes)
    pub first_stream: IndexStreamInfo, // Primary data stream (29 bytes)

    // ===== $FILE_NAME timestamps (32 bytes) =====
    pub fn_created: i64,
    pub fn_modified: i64,
    pub fn_accessed: i64,
    pub fn_mft_changed: i64,

    // ===== Tree metrics (20 bytes) =====
    pub descendants: u32,            // Files + subdirs in subtree
    pub treesize: u64,               // Sum of logical sizes in subtree
    pub tree_allocated: u64,         // Sum of allocated sizes in subtree

    // ===== Internal stream sizes (16 bytes) =====
    pub internal_streams_size: u64,
    pub internal_streams_allocated: u64,
}
```

### StandardInfo (48 bytes)

Bit-packed file attributes using a single `u32` for 17 boolean flags:

```rust
#[repr(C)]
pub struct StandardInfo {
    pub created: i64,        // Unix microseconds
    pub modified: i64,
    pub accessed: i64,
    pub mft_changed: i64,
    pub flags: u32,          // 17 bit-packed flags
    pub usn: u64,            // USN journal correlation
    pub security_id: u32,    // Index into $Secure
    pub owner_id: u32,       // Quota tracking
}

// Flag bits:
// 0: IS_READONLY    1: IS_ARCHIVE     2: IS_SYSTEM
// 3: IS_HIDDEN      4: IS_OFFLINE     5: IS_NOT_INDEXED
// 6: IS_NO_SCRUB    7: IS_INTEGRITY   8: IS_PINNED
// 9: IS_UNPINNED   10: IS_DIRECTORY  11: IS_COMPRESSED
// 12: IS_ENCRYPTED  13: IS_SPARSE    14: IS_REPARSE
// 15: IS_TEMPORARY  16: IS_VIRTUAL
```

### IndexNameRef (8 bytes)

Compact reference into the `names` buffer:

```rust
#[repr(C)]
pub struct IndexNameRef {
    pub offset: u32,     // Byte offset into MftIndex::names
    pub meta: u32,       // Packed: length(10) | flags(6) | extension_id(16)
}

// Bit layout of meta:
// [0-9]:   UTF-8 length (max 1023 bytes)
// [10-15]: Flags (bit 0 = is_ascii)
// [16-31]: Extension ID (65K unique extensions, 0 = no extension)
```

### LinkInfo (24 bytes)

Hard link chain entry. Most files have one name (inline in `first_name`). Files with multiple hard links form a singly-linked list:

```rust
#[repr(C)]
pub struct LinkInfo {
    pub next_entry: u32,       // → links[], or NO_ENTRY
    pub name: IndexNameRef,    // Filename reference (8 bytes)
    pub parent_frs: u64,       // Parent directory FRS
}

// Chain example for a file with 3 hard links:
// FileRecord.first_name → LinkInfo { name="file.txt", parent=42, next=0 }
// links[0] → LinkInfo { name="alias1.txt", parent=99, next=1 }
// links[1] → LinkInfo { name="alias2.txt", parent=200, next=NO_ENTRY }
```

### IndexStreamInfo (29 bytes)

Alternate Data Stream chain. Most files have only the default `$DATA` stream (inline in `first_stream`):

```rust
#[repr(C)]
pub struct IndexStreamInfo {
    pub size: SizeInfo,        // { length: u64, allocated: u64 } = 16 bytes
    pub next_entry: u32,       // → streams[], or NO_ENTRY
    pub name: IndexNameRef,    // Stream name (empty for default $DATA)
    pub flags: u8,             // bit0=sparse, bit1=resident, bits2-7=type_name_id
}
```

### ChildInfo (14 bytes)

Directory child list entry. Directories form a singly-linked list of children:

```rust
#[repr(C)]
pub struct ChildInfo {
    pub next_entry: u32,    // → children[], or NO_ENTRY
    pub child_frs: u64,     // FRS of the child file/directory
    pub name_index: u16,    // Which hard link this child uses
}
```

---

## FRS Lookup: O(1) Access

The `frs_to_idx` array provides constant-time lookup from FRS to record index:

```rust
// frs_to_idx is a sparse array indexed by FRS number
// frs_to_idx[frs] = index into records[], or NO_ENTRY

pub fn get_or_create(&mut self, frs: u64) -> &mut FileRecord {
    let frs_usize = frs as usize;

    // Expand lookup table if needed
    if frs_usize >= self.frs_to_idx.len() {
        self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
    }

    let idx = self.frs_to_idx[frs_usize];
    if idx == NO_ENTRY {
        // Create new record
        let new_idx = self.records.len() as u32;
        self.frs_to_idx[frs_usize] = new_idx;
        self.records.push(FileRecord::new(frs));
        &mut self.records[new_idx as usize]
    } else {
        &mut self.records[idx as usize]
    }
}
```

**Trade-off:** `frs_to_idx` uses O(max_frs) memory but provides O(1) lookup. For a typical drive with max FRS of 5M, this costs ~20MB — acceptable for the speed benefit.

---

## Pre-Allocation Strategy

**Source:** `index/base.rs` — `with_capacity_optimized()`

UFFS pre-allocates all vectors based on the MFT bitmap popcount to eliminate resizing during parsing:

```rust
pub fn with_capacity_optimized(volume: char, estimated_records: usize, max_frs: u64) -> Self {
    Self {
        records:           Vec::with_capacity(estimated_records * 105 / 100),  // +5% margin
        frs_to_idx:        Vec::with_capacity(max_frs as usize + 1),
        names:             String::with_capacity(estimated_records * 23),      // ~23 chars avg
        links:             Vec::with_capacity(estimated_records / 16),         // ~6% hardlinks
        streams:           Vec::with_capacity(estimated_records / 4),          // ~25% have ADS
        internal_streams:  Vec::with_capacity(estimated_records / 20),         // ~5% internal
        children:          Vec::with_capacity(estimated_records * 3 / 2),      // dir children
        ..
    }
}
```

These ratios are based on empirical analysis of typical NTFS volume characteristics.

---

## Extension Table

**Source:** `index/extensions.rs`

File extensions are **interned** — each unique extension gets a numeric ID. This enables O(1) `*.ext` pattern matching.

```rust
pub struct ExtensionTable {
    // Maps extension string → numeric ID
    // "txt" → 1, "rs" → 2, "cpp" → 3, ...
    ext_to_id: HashMap<String, u16>,
    id_to_ext: Vec<String>,    // Reverse lookup
}
```

The `IndexNameRef::extension_id` field (16 bits) stores the interned ID for each filename. This allows:

```rust
// Building the extension index (after parsing):
pub struct ExtensionIndex {
    // ext_id → Vec<record_index>
    // Enables O(matches) lookup for *.txt, *.rs, etc.
}

// Query: find all *.rs files
let ext_id = extensions.get_id("rs")?;
let record_indices = extension_index.get(ext_id);
// → directly yields matching records without scanning
```

---

## Tree Metrics

### What Are Tree Metrics?

For each directory, UFFS computes:
- **`descendants`**: Total count of all files and subdirectories in the subtree
- **`treesize`**: Sum of all logical file sizes in the subtree
- **`tree_allocated`**: Sum of all allocated disk sizes in the subtree

These enable "folder size" display without re-scanning.

### Algorithm: Leaf-Peeling (Kahn-style Topological Sort)

**Source:** `index/tree.rs` + `tree_metrics.rs`

```
Time:  O(n) — each node processed exactly once
Space: O(n) — two temporary arrays
No recursion — guaranteed stack safety
```

```rust
pub fn compute_tree_metrics(&mut self) {
    let n = self.records.len();

    // Step 1: Build parent_idx and pending_children arrays
    let mut parent_idx: Vec<u32> = vec![NO_ENTRY; n];  // → parent record index
    let mut pending: Vec<u32> = vec![0; n];              // remaining children count

    for (i, record) in self.records.iter().enumerate() {
        let parent_frs = record.first_name.parent_frs;
        if let Some(pidx) = self.lookup_frs(parent_frs) {
            parent_idx[i] = pidx;
            pending[pidx] += 1;
        }
    }

    // Step 2: Initialize base metrics (own size)
    for record in &mut self.records {
        record.treesize = record.first_stream.size.length;
        record.tree_allocated = record.first_stream.size.allocated;
        // + internal stream sizes for tree metrics
    }

    // Step 3: Push all leaf nodes (pending_children == 0)
    let mut stack: Vec<u32> = Vec::new();
    for i in 0..n {
        if pending[i] == 0 {
            stack.push(i as u32);
        }
    }

    // Step 4: Process bottom-up
    while let Some(idx) = stack.pop() {
        let pidx = parent_idx[idx as usize];
        if pidx == NO_ENTRY { continue; }

        // Accumulate child metrics into parent
        let child = &self.records[idx as usize];
        let (desc, tsize, talloc) = (child.descendants, child.treesize, child.tree_allocated);

        let parent = &mut self.records[pidx as usize];
        parent.descendants += 1 + desc;  // This child + its descendants
        parent.treesize += tsize;
        parent.tree_allocated += talloc;

        // Decrement parent's pending count
        pending[pidx as usize] -= 1;
        if pending[pidx as usize] == 0 {
            stack.push(pidx);  // Parent is now a leaf → process it
        }
    }
}
```

### Hard Link Size Attribution

Files with multiple hard links require proportional size division to avoid double-counting:

```rust
fn hardlink_delta(value: u64, name_info: u16, total_names: u16) -> u64 {
    if total_names <= 1 { return value; }
    let i = u64::from(name_info);
    let n = u64::from(total_names);
    (value * (i + 1) / n) - (value * i / n)
    // Integer division formula: gives each link a proportional share
    // Sum of all shares == original value (no rounding error)
}
```

### Self-Healing for Live Scans

IOCP live scans can produce incomplete child lists due to record processing order. Tree metrics includes a self-healing mechanism:

```rust
// If first pass leaves directories with descendants == 0,
// rebuild children from FILE_NAME parent references and re-run
if has_empty_directories {
    self.rebuild_children_from_names();
    // Re-run tree metrics with corrected child lists
}
```

---

## Path Resolution

### The Problem

Each file stores only its parent's FRS. To display full paths (e.g., `C:\Users\John\file.txt`), we must walk up the parent chain.

### PathResolver

**Source:** `index/paths.rs`

```rust
pub struct PathResolver<'a> {
    index: &'a MftIndex,
    cache: PathCache,  // Caches resolved paths
}

impl PathResolver<'_> {
    pub fn resolve_path(&mut self, record_idx: usize) -> String {
        // Walk parent chain: file → parent → grandparent → ... → root (FRS 5)
        let mut components: Vec<&str> = Vec::new();
        let mut current = record_idx;

        loop {
            let record = &self.index.records[current];
            let name = self.get_name(record);
            components.push(name);

            let parent_frs = record.first_name.parent_frs;
            if parent_frs == ROOT_FRS || parent_frs == 0 {
                break;  // Reached root
            }

            match self.index.lookup_frs(parent_frs) {
                Some(pidx) => current = pidx as usize,
                None => break,  // Orphan (parent not in index)
            }
        }

        // Reverse and join: ["file.txt", "John", "Users"] → "C:\Users\John\file.txt"
        components.reverse();
        format!("{}:\\{}", self.index.volume, components.join("\\"))
    }
}
```

### FastPathResolver (Optimized)

For bulk path resolution (output formatting), `FastPathResolver` pre-computes and caches paths:

```rust
pub struct FastPathResolver {
    // Pre-allocated arena for path strings
    arena: NameArena,
    // Cached resolved paths indexed by record index
    cache: Vec<Option<CachedPath>>,
}

pub struct CachedPath {
    pub offset: u32,   // Into arena
    pub length: u16,   // Path length
}
```

**Performance:** Resolving 2M paths takes ~200ms with caching (vs ~2s without).

---

## Name Retrieval

Getting a filename from a record:

```rust
pub fn get_name<'a>(index: &'a MftIndex, record: &FileRecord) -> &'a str {
    let name_ref = &record.first_name.name;
    if !name_ref.is_valid() {
        return "";
    }

    let offset = name_ref.offset as usize;
    let length = name_ref.length() as usize;
    &index.names[offset..offset + length]
}
```

All names are stored in the contiguous `names: String` buffer. This is cache-friendly for sequential scanning and avoids per-name heap allocation.

---

## Fragment Merging

### Why Fragments?

When reading the MFT with parallel/pipelined modes, each I/O chunk is parsed into a separate `MftIndexFragment`. These fragments must be merged into a single `MftIndex`.

**Source:** `index/merge.rs`

### MftIndexFragment

```rust
pub struct MftIndexFragment {
    pub records: Vec<FileRecord>,
    pub frs_to_idx: Vec<u32>,
    pub names: String,
    pub links: Vec<LinkInfo>,
    pub streams: Vec<IndexStreamInfo>,
    pub children: Vec<ChildInfo>,
    pub extensions: ExtensionTable,
}
```

### Merge Algorithm

```rust
pub fn merge_fragments(fragments: Vec<MftIndexFragment>, volume: char) -> MftIndex {
    // 1. Calculate total sizes for pre-allocation
    // 2. For each fragment:
    //    a. Remap name offsets (adjust by cumulative names length)
    //    b. Remap link/stream/child next_entry indices
    //    c. Remap extension IDs (fragment-local → global)
    //    d. Append to merged index
    // 3. Rebuild frs_to_idx for the merged index
    // 4. Handle extension records that reference base records in other fragments
}
```

Key complexity: extension records may arrive before their base records (different chunks). The merge handles this by:
1. Creating placeholder records for unknown base FRS
2. Merging placeholder data when the real base record is encountered
3. Using `has_base_data()` to determine which record has the real `$STANDARD_INFORMATION`

---

## DataFrame Conversion

**Source:** `index/dataframe.rs`

The lean `MftIndex` can be converted to a Polars `DataFrame` for analytics:

```rust
impl MftIndex {
    pub fn to_dataframe(&self) -> Result<DataFrame> {
        // Build column vectors from records
        let frs_col: Vec<u64> = self.records.iter().map(|r| r.frs).collect();
        let parent_col: Vec<u64> = self.records.iter().map(|r| r.first_name.parent_frs).collect();
        let name_col: Vec<String> = self.records.iter().map(|r| self.get_name(r)).collect();
        let size_col: Vec<u64> = self.records.iter().map(|r| r.first_stream.size.length).collect();
        // ... timestamps, flags, tree metrics ...

        DataFrame::new(vec![
            Series::new("frs", frs_col),
            Series::new("parent_frs", parent_col),
            Series::new("name", name_col),
            Series::new("size", size_col),
            // ...
        ])
    }
}
```

This is used for:
- Polars-based analytics queries
- Parquet export for caching
- Complex filtering with the `MftQuery` API

---

## Index Caching

**Source:** `cache.rs`, `reader/index_cache.rs`, `index/storage/`

UFFS persists the `MftIndex` to disk so that subsequent launches skip the expensive MFT read. The cache uses a custom binary format with TTL-based freshness and USN Journal incremental updates.

### Cache Location and Naming

```
{TEMP}/uffs_index_cache/
├── C_index.uffs    — Index for C: drive
├── D_index.uffs    — Index for D: drive
└── F_index.uffs    — Index for F: drive
```

### Binary File Format (`.uffs`)

**Source:** `index/storage/header.rs`, `index/storage/serialize.rs`, `index/storage/deserialize.rs`

Each cache file starts with a versioned header followed by the serialized index data:

```
┌─────────────────────────────────────────────────────┐
│ IndexHeader                                          │
│   magic: "UFFSIDX\0" (8 bytes)                      │
│   version: u32 (current: 8)                          │
│   volume: char                                       │
│   volume_serial: u64 (for validation)                │
│   usn_journal_id: u64 (for incremental updates)      │
│   next_usn: i64 (checkpoint for USN reading)         │
│   created_at: u64 (Unix epoch seconds)               │
│   record_count, names_size, links_count,             │
│   streams_count, children_count: u64                 │
├─────────────────────────────────────────────────────┤
│ frs_to_idx: Vec<u32>                                 │
├─────────────────────────────────────────────────────┤
│ records: Vec<FileRecord>                             │
├─────────────────────────────────────────────────────┤
│ names: String (raw UTF-8 bytes)                      │
├─────────────────────────────────────────────────────┤
│ links: Vec<LinkInfo>                                 │
├─────────────────────────────────────────────────────┤
│ streams: Vec<IndexStreamInfo>                        │
├─────────────────────────────────────────────────────┤
│ children: Vec<ChildInfo>                             │
├─────────────────────────────────────────────────────┤
│ extensions: ExtensionTable                           │
└─────────────────────────────────────────────────────┘
```

**Version history:** The format has evolved through 8 versions, adding tree metrics (v3), `$FILE_NAME` timestamps (v4), forensic fields (v5-v7), and `total_stream_count` (v8). The deserializer accepts versions 3–8 with graceful defaults for missing fields.

### TTL (Time-To-Live)

```rust
pub const INDEX_TTL_SECONDS: u64 = 600; // 10 minutes

pub enum CacheStatus {
    Fresh { index: MftIndex, header: IndexHeader, age_seconds: u64 },
    Stale { age_seconds: Option<u64> },
    Missing,
}
```

- **Fresh**: Cache exists and was written within TTL → load from disk
- **Stale**: Cache exists but TTL expired → full MFT re-read
- **Missing**: No cache file → full MFT re-read
- **Multi-drive**: If ANY drive's cache is stale, ALL caches are rebuilt (consistency)

### USN Journal Incremental Updates

When the cache is fresh, UFFS uses the NTFS **USN Change Journal** to apply only the changes since the cache was built — avoiding a full MFT re-read:

```
read_index_cached(ttl_seconds)
  │
  ├─► check_cache_status(drive, ttl)
  │     → Fresh: load index from disk
  │     → Stale/Missing: full MFT read → save to cache → return
  │
  ├─► query_usn_journal(drive) → current journal state
  │
  ├─► Validate journal continuity:
  │     Journal ID changed?  → full rebuild (journal was recreated)
  │     Checkpoint USN < FirstUsn?  → full rebuild (journal wrapped)
  │     Checkpoint USN >= NextUsn?  → no changes, return cached index
  │
  ├─► read_usn_journal(drive, journal_id, checkpoint_usn)
  │     → Vec<UsnRecord> (changes since checkpoint)
  │
  ├─► aggregate_changes(&records) → per-FRS change summary
  │
  ├─► index.apply_usn_changes(&changes)
  │     → Creates new records, updates modified records,
  │       marks deleted records, handles renames
  │
  └─► save_to_cache(updated_index, new_checkpoint_usn)
```

**Performance impact:** Loading from cache + applying USN updates typically takes **< 1 second** vs 5-8 seconds for a full MFT read on NVMe. This makes repeated searches near-instant.

### Read-Only Volume Optimization

For read-only volumes (e.g., mounted ISOs, write-protected USB drives), the cache never goes stale since nothing can change. UFFS detects this and uses the cache with infinite TTL, skipping USN journal queries entirely:

```rust
if is_volume_read_only(drive) {
    // Load cache with TTL=∞, skip USN, skip VolumeHandle
    return load_cached_index(drive, u64::MAX);
}
```

### Cache Serialization

The index is serialized on the calling thread (CPU-bound, fast) and the disk write is offloaded to a `spawn_blocking` task so it doesn't delay the search:

```rust
let cache_bytes = index.serialize(volume_serial, usn_journal_id, next_usn);
tokio::task::spawn_blocking(move || {
    std::fs::write(cache_file_path(drive), &cache_bytes)
});
// Return index immediately — cache write happens in background
```

---

## Statistics

**Source:** `index/stats.rs`

`MftStats` tracks detailed statistics during and after parsing:

```rust
pub struct MftStats {
    pub record_count: u64,           // Total records
    pub file_count: u64,             // Files only
    pub dir_count: u64,              // Directories only
    pub total_bytes: u64,            // Sum of all file sizes
    pub max_frs: u64,                // Highest FRS seen
    pub multi_name_count: u64,       // Files with hardlinks
    pub ads_count: u64,              // Files with ADS
    pub system_metafile_count: u64,  // FRS 0-15 metafiles
    pub corrupted_records: u64,      // Failed USA fixup
    pub total_name_bytes: u64,       // Names buffer size
    // Per-attribute byte counters
    pub hidden_bytes: u64,
    pub system_bytes: u64,
    pub compressed_bytes: u64,
    pub encrypted_bytes: u64,
    pub sparse_bytes: u64,
    pub reparse_bytes: u64,
    // Size distribution buckets
    pub size_bucket_counts: [u64; 10],
    pub size_bucket_bytes: [u64; 10],
}
```

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62*
