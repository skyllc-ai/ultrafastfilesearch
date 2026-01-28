# MftIndex Deep Dive: Building and Tree Representation

> A comprehensive analysis of how the Rust UFFS builds its in-memory MFT index and computes tree metrics.

## 1. Overall Architecture

The `uffs-mft` crate is the core library for NTFS MFT reading. Key modules:

| Module | Purpose |
|--------|---------|
| `index.rs` | Core `MftIndex` data structure and tree metrics computation |
| `parse.rs` | Cross-platform NTFS record parsing (attributes, timestamps, flags) |
| `io.rs` | Windows-only IOCP-based async I/O for maximum throughput |
| `ntfs.rs` | NTFS structure definitions (boot sector, record headers, attributes) |
| `reader.rs` | High-level MFT reading API with mode selection |

---

## 2. MFT Reading Pipeline

```
┌─────────────┐    ┌──────────────┐    ┌────────────────┐    ┌──────────────┐    ┌──────────┐
│  Raw Disk   │───▶│  IOCP Async  │───▶│ Sector Buffers │───▶│ Parse Records│───▶│ MftIndex │
│  (\\.\C:)   │    │     I/O      │    │  (aligned)     │    │  (inline)    │    │          │
└─────────────┘    └──────────────┘    └────────────────┘    └──────────────┘    └──────────┘
```

**Key stages:**

1. **Volume Handle**: Open `\\.\C:` with admin privileges (direct disk access)
2. **MFT Extent Mapping**: Parse `$MFT`'s data runs to find all MFT fragments on disk
3. **IOCP Async I/O**: Issue parallel `ReadFile()` calls with sector-aligned buffers
4. **Inline Parsing**: Parse MFT records as they arrive (no intermediate storage)
5. **Index Building**: Populate `MftIndex` with parsed records and build tree structure

---

## 3. MftIndex Data Structure

The `MftIndex` is a compact, cache-friendly in-memory index matching C++ `NtfsIndex` architecture:

```
MftIndex
├── volume: char                         // Volume letter (e.g., 'C')
├── records: Vec<FileRecord>             // Core file metadata (~232 bytes each)
├── frs_to_idx: Vec<u32>                 // FRS → record index (O(1) lookup)
├── names: String                        // All filenames concatenated (single alloc)
├── links: Vec<LinkInfo>                 // Hard link chain (overflow only)
├── streams: Vec<IndexStreamInfo>        // ADS chain (overflow only)
├── children: Vec<ChildInfo>             // Directory child entries
├── stats: MftStats                      // Statistics collected during parsing
├── extensions: ExtensionTable           // Extension interning for fast *.ext queries
├── extension_index: Option<ExtensionIndex>  // CSR index for O(matches) queries
└── forensic_mode: bool                  // Whether deleted/corrupt records included
```

### FileRecord Structure (~232 bytes per record)

```
FileRecord
├── frs: u64                             // File Record Segment number (primary key)
├── sequence_number: u16                 // Incremented when FRS is reused
├── namespace: u8                        // 0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS
├── forensic_flags: u8                   // bit 0=deleted, bit 1=corrupt, bit 2=extension
├── lsn: u64                             // Log File Sequence Number (correlates with $LogFile)
├── reparse_tag: u32                     // Reparse point tag (symlink, junction, OneDrive, etc.)
├── base_frs: u64                        // Base FRS for extension records
├── stdinfo: StandardInfo                // Timestamps + 17 attribute flags from $STANDARD_INFO
├── name_count: u16                      // Number of hard links (usually 1)
├── stream_count: u16                    // Number of data streams (usually 1)
├── first_child: u32                     // Index into children vector (directories only)
├── first_name: LinkInfo                 // Primary filename (inline, no allocation)
├── first_stream: IndexStreamInfo        // Primary data stream (inline, no allocation)
├── fn_created/modified/accessed: i64    // $FILE_NAME timestamps (often differ from $SI)
├── descendants: u32                     // Tree metric: count of all descendants
├── treesize: u64                        // Tree metric: sum of sizes in subtree
└── tree_allocated: u64                  // Tree metric: sum of allocated in subtree
```

---

## 4. Tree Representation Building

The tree structure is built during `from_parsed_records()` by creating `ChildInfo` entries:

```rust
// Build parent-child relationship for ALL hardlinks
// C++ creates a SEPARATE child entry for EACH $FILE_NAME attribute (hardlink)
// This is crucial for correct tree metrics calculation with hardlinks.
for (name_idx, name_info) in parsed.names.iter().enumerate() {
    let parent_frs = name_info.parent_frs;
    
    // Ensure parent exists (create placeholder if needed)
    let parent_idx = index.get_or_create_parent(parent_frs);
    
    // Add child entry with name_index for proportional share calculation
    index.children.push(ChildInfo {
        next_entry: old_first_child,    // Linked list pointer
        child_frs: parsed.frs,          // Child's FRS
        name_index: name_idx as u16,    // Which hardlink (for proportional shares)
    });
    
    // Update parent's first_child to point to new entry
    parent.first_child = child_idx;
}
```

**Key insight**: Each hardlink creates a **separate** `ChildInfo` entry in its parent directory. This enables correct proportional size attribution when a file has multiple hard links.

### Directory Child Linked List

```
Directory (FRS 5)
    first_child ──▶ ChildInfo[0] ──▶ ChildInfo[1] ──▶ ChildInfo[2] ──▶ NO_ENTRY
                    (child_frs=100)  (child_frs=101)  (child_frs=100)
                    (name_index=0)   (name_index=0)   (name_index=1)
                                                       ↑
                                                       Same file, different hardlink!
```

---

## 5. Tree Metrics Algorithm (Kahn-style Leaf-Peeling)

The algorithm computes `descendants`, `treesize`, and `tree_allocated` for every directory using a topological sort approach:

### Phase 1: Calculate Base Metrics
```rust
// Sum ALL streams' sizes (default + ADS) for each record
let base_metrics: Vec<_> = records.iter().map(|record| {
    let mut total_size = record.first_stream.size.length;
    // Walk the stream linked list to sum ADS sizes
    let mut next = record.first_stream.next_entry;
    while next != NO_ENTRY {
        total_size += streams[next].size.length;
        next = streams[next].next_entry;
    }
    (is_directory, stream_count, name_count, total_size, total_allocated)
}).collect();
```

### Phase 2: Build Pending Children Count
```rust
// Count child entries for each parent (not unique children - each hardlink counts!)
let mut pending_children = vec![0_u32; n];
for record in &records {
    let mut child_entry = record.first_child;
    while child_entry != NO_ENTRY {
        pending_children[parent_idx] += 1;
        child_entry = children[child_entry].next_entry;
    }
}
```

### Phase 3: Initialize Records
```rust
for record in &mut records {
    if is_directory {
        // Directories: descendants = stream_count (each stream = +1 in C++)
        record.descendants = stream_count;
        record.treesize = own_index_size;  // $INDEX_ROOT + $INDEX_ALLOCATION + $BITMAP
    } else {
        // Files: descendants = 0, treesize = own size
        record.descendants = 0;
        record.treesize = own_size;
    }
}
```

### Phase 4: Build Reverse Mapping
```rust
// For each record, which parents have child entries pointing to it?
// Structure: child_to_parents[child_idx] = Vec<(parent_idx, name_index)>
let mut child_to_parents: Vec<Vec<(usize, u16)>> = vec![Vec::new(); n];

for (parent_idx, record) in records.iter().enumerate() {
    let mut child_entry_idx = record.first_child;
    while child_entry_idx != NO_ENTRY {
        let child_entry = &children[child_entry_idx];
        let child_record_idx = frs_to_idx[child_entry.child_frs];
        child_to_parents[child_record_idx].push((parent_idx, child_entry.name_index));
        child_entry_idx = child_entry.next_entry;
    }
}
```

### Phase 5: Leaf-Peeling (Topological Sort)
```rust
// Start with all leaf nodes (records with no children)
let mut stack: Vec<usize> = pending_children.iter()
    .enumerate()
    .filter(|(_, &count)| count == 0)
    .map(|(idx, _)| idx)
    .collect();

while let Some(child_idx) = stack.pop() {
    let (child_descendants, child_treesize, child_tree_allocated) =
        (records[child_idx].descendants, records[child_idx].treesize, records[child_idx].tree_allocated);

    // For each parent that has a child entry pointing to this record
    for &(parent_idx, name_index) in &child_to_parents[child_idx] {
        // C++ formula: name_info = name_count - 1 - name_index
        let name_info = name_count.saturating_sub(1).saturating_sub(name_index);

        // Calculate proportional share using delta formula
        let size_share = hardlink_delta(child_treesize, name_info, name_count);
        let allocated_share = hardlink_delta(child_tree_allocated, name_info, name_count);

        // Add to parent's metrics
        records[parent_idx].descendants += descendants_contribution;
        records[parent_idx].treesize += size_share;
        records[parent_idx].tree_allocated += allocated_share;

        // Decrement parent's pending count
        pending_children[parent_idx] -= 1;
        if pending_children[parent_idx] == 0 {
            stack.push(parent_idx);  // Parent is now ready to propagate
        }
    }
}
```

### Hardlink Delta Formula

The delta formula ensures **no rounding errors** when dividing a file's size among multiple hardlinks:

```rust
/// C++ `Accumulator::delta` formula for proportional hardlink size division.
///
/// When a file has N hardlinks, each hardlink parent gets a proportional share
/// of the file's size. This ensures the total size across all parents equals
/// the file's actual size.
///
/// Formula: value * (i + 1) / n - value * i / n
fn hardlink_delta(value: u64, name_info: u16, total_names: u16) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let i = u64::from(name_info);
    let n = u64::from(total_names);
    (value * (i + 1) / n) - (value * i / n)
}
```

**Example**: 99-byte file with 3 hardlinks:
```
Hardlink 0: delta(99, 0, 3) = 99*1/3 - 99*0/3 = 33
Hardlink 1: delta(99, 1, 3) = 99*2/3 - 99*1/3 = 33
Hardlink 2: delta(99, 2, 3) = 99*3/3 - 99*2/3 = 33
Total: 33 + 33 + 33 = 99 ✓ (no rounding error!)
```

---

## 6. NTFS Attribute Parsing

The parser extracts data from these NTFS attributes:

| Attribute | Type ID | Data Extracted |
|-----------|---------|----------------|
| `$STANDARD_INFORMATION` | 0x10 | Timestamps, 17 file attribute flags, USN, security ID |
| `$FILE_NAME` | 0x30 | Filename, parent FRS, namespace, $FN timestamps |
| `$DATA` | 0x80 | File size, allocated size, resident flag, sparse flag |
| `$INDEX_ROOT` | 0x90 | Directory index (merged with $INDEX_ALLOCATION for $I30) |
| `$INDEX_ALLOCATION` | 0xA0 | Directory index continuation |
| `$BITMAP` | 0xB0 | Directory index bitmap |
| `$REPARSE_POINT` | 0xC0 | Symlink/junction target, reparse tag |

### Stream Counting (C++ Parity)

For accurate tree metrics, these are counted as "streams":
- `$DATA` streams (default + ADS)
- `$REPARSE_POINT` (counted as stream)
- Non-`$I30` index attributes (`$SDH`, `$SII`, `$O`, `$Q`, `$R`)
- `$OBJECT_ID`, `$EA`, `$EA_INFORMATION`, `$PROPERTY_SET`, `$LOGGED_UTILITY_STREAM`
- `$VOLUME_NAME`, `$VOLUME_INFORMATION`
- Unnamed `$BITMAP`

---

## 7. Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Inline first name/stream** | Most files have 1 name and 1 stream - avoid pointer indirection |
| **Linked lists for overflow** | Hard links and ADS are rare (~0.1%) - save memory for common case |
| **Contiguous names buffer** | Single allocation, cache-friendly sequential access |
| **O(1) FRS lookup table** | `frs_to_idx[frs]` gives record index directly |
| **Separate ChildInfo per hardlink** | Enables correct proportional share calculation |
| **CSR extension index** | Compressed Sparse Row format for O(matches) `*.ext` queries |
| **Bit-packed StandardInfo** | 17 flags in single u32, cache-friendly |
| **u64 for FRS values** | Support all valid NTFS volumes (48-bit FRS) |

---

## 8. C++ Parity Status

From `TREE_METRICS_PARITY_ANALYSIS.md`:

| Metric | C++ Value | Rust Value | Match |
|--------|-----------|------------|-------|
| Descendants | 15,119 | 15,119 | ✅ 100% |
| Treesize | 609,123,456 | 609,123,408 | ✅ 99.999992% (48 bytes diff) |

### Key Fixes Implemented for Parity

1. **System metafiles included**: FRS 0-15 are included in tree metrics (not excluded)
2. **$BadClus:$Bad handling**: Uses `InitializedSize` instead of `DataSize` for sparse files
3. **Stream counting**: Includes `$REPARSE_POINT`, non-`$I30` indexes, `$OBJECT_ID`, etc.
4. **Directory descendants**: Initialized to `stream_count` (not 1)
5. **MFT overhead removed**: No incorrect overhead for resident files

---

## 9. Memory Layout Visualization

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              MftIndex                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│  records: Vec<FileRecord>                                                    │
│  ┌─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┐   │
│  │ Rec[0]  │ Rec[1]  │ Rec[2]  │ Rec[3]  │ Rec[4]  │ Rec[5]  │  ...    │   │
│  │ 232 B   │ 232 B   │ 232 B   │ 232 B   │ 232 B   │ 232 B   │         │   │
│  └─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘   │
├─────────────────────────────────────────────────────────────────────────────┤
│  frs_to_idx: Vec<u32>  (sparse - indexed by FRS)                            │
│  ┌───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┐        │
│  │ 0 │ 1 │ 2 │ 3 │ 4 │ 5 │ 6 │ 7 │...│MAX│   │   │   │   │   │   │        │
│  └───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┘        │
├─────────────────────────────────────────────────────────────────────────────┤
│  names: String  (all filenames concatenated)                                 │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │ "$MFT\0$MFTMirr\0$LogFile\0$Volume\0.\0$AttrDef\0$Bitmap\0..."       │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
├─────────────────────────────────────────────────────────────────────────────┤
│  children: Vec<ChildInfo>  (directory contents)                              │
│  ┌─────────────┬─────────────┬─────────────┬─────────────┬─────────────┐   │
│  │ next: 1     │ next: 2     │ next: MAX   │ next: 4     │ ...         │   │
│  │ frs: 100    │ frs: 101    │ frs: 102    │ frs: 100    │             │   │
│  │ name_idx: 0 │ name_idx: 0 │ name_idx: 0 │ name_idx: 1 │             │   │
│  └─────────────┴─────────────┴─────────────┴─────────────┴─────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 10. Performance Characteristics

| Operation | Complexity | Notes |
|-----------|------------|-------|
| FRS lookup | O(1) | Direct index into `frs_to_idx` |
| Get filename | O(1) | Offset + length into `names` buffer |
| Iterate children | O(children) | Follow linked list |
| Extension query | O(matches) | CSR index lookup |
| Tree metrics | O(n) | Single pass leaf-peeling |
| Build index | O(n) | Single pass over parsed records |

---

## 11. Conclusion

The Rust `MftIndex` implementation achieves C++ performance parity through:

1. **Cache-friendly memory layout** - Contiguous vectors, inline common cases
2. **O(1) lookups** - Direct FRS indexing, extension interning
3. **Correct hardlink handling** - Separate child entries with proportional shares
4. **Efficient tree metrics** - Kahn-style topological sort with delta formula

The 48-byte difference in treesize (0.000008%) is likely due to edge cases in `$ATTRIBUTE_LIST` extension records or compressed stream handling - a topic for future investigation.

