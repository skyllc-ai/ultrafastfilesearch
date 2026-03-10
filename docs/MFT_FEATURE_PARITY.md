# MFT Feature Parity: Reference baseline vs Rust implementation

> **Document Purpose:** Detailed gap analysis and implementation roadmap to achieve 100% feature parity
> between the reference baseline (`old_cpp_reference/uffs/UltraFastFileSearch-code/file.cpp`, local-only and never pushed) and the Rust
> implementation (`crates/uffs-mft/`).

---

## Executive Summary

| Category | Reference Features | Rust Implemented | Parity |
|----------|--------------|------------------|--------|
| I/O Layer | 5 | 5 | ✅ 100% |
| Data Extraction | 10 | 10 | ✅ 100% |
| Data Representation (MFT) | 5 | 5 | ✅ 100% |
| **Overall (uffs-mft)** | **20** | **20** | **✅ 100%** |

> **Status:** MFT reading implementation completed on 2026-01-16. All MFT extraction features implemented.

### Architecture Separation

| Crate | Responsibility | Status |
|-------|----------------|--------|
| `uffs-mft` | MFT reading, DataFrame building, CSV/Parquet/RAW persistence | ✅ 100% |
| `uffs-core` | Post-processing (tree structure, treesize, bulkiness), queries | ✅ 100% |

> **Design Principle:** `uffs-mft` does pure MFT reading and storage. No post-processing.
> Tree calculations and derived metrics belong in `uffs-core` query engine.

---

## Milestone Tracker

### Phase 1: Extension Record Merging ✅ COMPLETE
| ID | Task | Status | File(s) |
|----|------|--------|---------|
| 1.1 | Add `ExtensionAttributes` struct to hold pending attributes | [x] | `io.rs` |
| 1.2 | Add `ParseResult` enum (Base/Extension/Skip) | [x] | `io.rs` |
| 1.3 | Create `parse_record_full()` for full parsing | [x] | `io.rs` |
| 1.4 | Create `MftRecordMerger` for two-pass processing | [x] | `io.rs` |
| 1.5 | Merge extension attributes into base records | [x] | `io.rs` |

### Phase 2: Hard Link Support ✅ COMPLETE
| ID | Task | Status | File(s) |
|----|------|--------|---------|
| 2.1 | Add `NameInfo` struct matching the baseline | [x] | `ntfs.rs` |
| 2.2 | Add `names: Vec<NameInfo>` to `ParsedRecord` | [x] | `io.rs` |
| 2.3 | Add `name_count()` method to `ParsedRecord` | [x] | `io.rs` |
| 2.4 | Collect ALL `$FILE_NAME` attributes (except DOS-only) | [x] | `io.rs` |
| 2.5 | Add `name_count` column to DataFrame | [x] | `reader.rs` |

### Phase 3: Alternate Data Streams (ADS) ✅ COMPLETE
| ID | Task | Status | File(s) |
|----|------|--------|---------|
| 3.1 | Add `StreamInfo` struct matching the baseline | [x] | `ntfs.rs` |
| 3.2 | Add `streams: Vec<StreamInfo>` to `ParsedRecord` | [x] | `io.rs` |
| 3.3 | Add `stream_count()` method to `ParsedRecord` | [x] | `io.rs` |
| 3.4 | Parse ALL `$DATA` attributes (named and unnamed) | [x] | `io.rs` |
| 3.5 | Extract stream name from attribute header | [x] | `io.rs` |
| 3.6 | Add `stream_count` column to DataFrame | [x] | `reader.rs` |

### Phase 4: Extended Size Information ✅ COMPLETE
| ID | Task | Status | File(s) |
|----|------|--------|---------|
| 4.1 | Add `allocated_size` field to `ParsedRecord` | [x] | `io.rs` |
| 4.2 | Add `allocated_size` to `StreamInfo` | [x] | `ntfs.rs` |
| 4.3 | Handle `CompressionUnit` for compressed files | [x] | `io.rs` |
| 4.4 | Add `allocated_size` column to DataFrame | [x] | `reader.rs` |

### Phase 5: Extended Attribute Flags ✅ COMPLETE
| ID | Task | Status | File(s) |
|----|------|--------|---------|
| 5.1 | Add `ExtendedStandardInfo` struct with 18 boolean flags | [x] | `ntfs.rs` |
| 5.2 | Add `from_attributes()` constructor | [x] | `ntfs.rs` |
| 5.3 | Add `to_raw_flags()` method | [x] | `ntfs.rs` |
| 5.4 | Parse all flags from `$STANDARD_INFORMATION` | [x] | `io.rs` |
| 5.5 | Add 11 individual flag columns to DataFrame | [x] | `reader.rs` |

### Phase 6: Directory Tree Structure ✅ COMPLETE (in `uffs-core`)
| ID | Task | Status | File(s) | Notes |
|----|------|--------|---------|-------|
| 6.1 | Add `NodeInfo` struct | [x] | `uffs-core/tree.rs` | Post-processing |
| 6.2 | Build `TreeIndex` from DataFrame | [x] | `uffs-core/tree.rs` | Query-time |
| 6.3 | Calculate `treesize` | [x] | `uffs-core/tree.rs` | Lazy computation |
| 6.4 | Add `bulkiness` calculation | [x] | `uffs-core/tree.rs` | Derived metric |
| 6.5 | Add tree columns via query | [x] | `uffs-core/tree.rs` | On-demand |
| 6.6 | Add `descendants` column | [x] | `uffs-core/tree.rs` | Count of items |
| 6.7 | Add `tree_allocated` column | [x] | `uffs-core/tree.rs` | Sum of allocated |

> **Architecture Decision:** Tree structure is post-processing, not MFT reading.
> Moved to `uffs-core` crate which handles queries and derived calculations.

### Phase 8: RAW MFT Persistence ✅ COMPLETE
| ID | Task | Status | File(s) |
|----|------|--------|---------|
| 8.1 | Add `save_raw_mft()` to save complete MFT bytes | [x] | `raw.rs`, `reader.rs` |
| 8.2 | Add `load_raw_mft()` to load saved MFT bytes | [x] | `raw.rs`, `reader.rs` |
| 8.3 | Handle fragmented MFT (reassemble extents) | [x] | `reader.rs` |
| 8.4 | Add `save-raw` / `load-raw` CLI commands | [x] | `uffs-cli/commands.rs` |
| 8.5 | Compress raw MFT (optional, zstd) | [x] | `raw.rs` |

> **Purpose:** Allow saving/loading the complete raw MFT bytes for offline analysis
> without requiring admin privileges or access to the original volume.
>
> **Implementation:** New `raw.rs` module with:
> - `RawMftHeader` - 64-byte header with magic, version, flags, sizes
> - `RawMftData` - Loaded raw MFT with record iteration
> - `save_raw_mft()` / `load_raw_mft()` - File I/O with optional zstd compression
> - `MftReader::read_raw()` - Read MFT as raw bytes (handles fragmented MFT)
> - `MftReader::save_raw_to_file()` - Convenience method to read and save
> - `MftReader::load_raw_to_dataframe()` - Load saved MFT and parse to DataFrame

### Phase 7: MFT Change Time ✅ COMPLETE
| ID | Task | Status | File(s) |
|----|------|--------|---------|
| 7.1 | Add `mft_changed` field to `ExtendedStandardInfo` | [x] | `ntfs.rs` |
| 7.2 | Extract from `$STANDARD_INFORMATION` | [x] | `io.rs` |
| 7.3 | Add `mft_changed` column to DataFrame | [x] | `reader.rs` |

---

## Detailed Gap Analysis

### Gap 1: Extension Record Merging (🔴 CRITICAL)

**Current Rust Behavior:**
```rust
// crates/uffs-mft/src/io.rs:535-538
if !header.is_base_record() {
    return None;  // ❌ Discards extension records entirely
}
```

**Reference Behavior:**
```cpp
// old_cpp_reference/uffs/file.cpp:2371-2372
unsigned int const frs_base = frsh->BaseFileRecordSegment 
    ? static_cast<unsigned int>(frsh->BaseFileRecordSegment) : frs;
Records::iterator base_record = this->at(frs_base);
// All attributes from extension record are merged into base_record
```

**Why This Matters:**
- NTFS files can span multiple MFT records when they have many attributes
- Large files with many alternate data streams use extension records
- Files with very long names or many hard links use extension records
- Without merging, these files have INCOMPLETE data

**Implementation Strategy:**
1. First pass: Parse all records, storing extension records separately
2. Second pass: Merge extension attributes into their base records
3. Use `base_file_record_segment` field to find the base record

**New Structs Required:**
```rust
/// Attributes extracted from an extension record, pending merge.
pub struct ExtensionAttributes {
    /// The base FRS this extension belongs to.
    pub base_frs: u64,
    /// Additional file names found in this extension.
    pub names: Vec<NameInfo>,
    /// Additional streams found in this extension.
    pub streams: Vec<StreamInfo>,
    /// Size contributions from this extension.
    pub size_delta: u64,
    pub allocated_delta: u64,
}
```

---

### Gap 2: Hard Link Support (🔴 CRITICAL)

**Current Rust Behavior:**
```rust
// crates/uffs-mft/src/io.rs:663-671
let is_better_name = match namespace {
    1 | 3 => true,                                   // Win32 or Win32+DOS
    0 => result.name.is_empty(),                     // POSIX only if no name yet
    2 => result.name.is_empty() && namespace != 1,   // DOS only if no better name
    _ => false,
};
if !is_better_name && !result.name.is_empty() {
    return;  // ❌ Discards additional names (hard links)
}
```

**Reference Behavior:**
```cpp
// old_cpp_reference/uffs/file.cpp:2394-2419
if (fn->Flags != 0x02 /* FILE_NAME_DOS */) {
    // Push existing first_name to nameinfos list
    if (LinkInfos::value_type *const si = this->nameinfo(&*base_record)) {
        this->nameinfos.push_back(base_record->first_name);
        base_record->first_name.next_entry = link_index;
    }
    // Store new name
    info->name.offset(...);
    info->parent = frs_parent;
    ++base_record->name_count;  // ✅ Tracks ALL names
}
```

**Why This Matters:**
- Hard links are common in Windows (e.g., `C:\Windows\System32` has many)
- Each hard link has a DIFFERENT parent directory
- Without tracking all names, files appear to be in only one location
- Security/forensics tools need to see all file locations

**Implementation Strategy:**
1. Store ALL `$FILE_NAME` attributes (except DOS-only namespace=2)
2. Add `names: Vec<NameInfo>` to `ParsedRecord`
3. Each `NameInfo` contains: name, parent_frs, namespace
4. DataFrame can either:
   - Expand to multiple rows (one per name) - easier for queries
   - Store as List column - more compact

**New Structs Required:**
```rust
/// Information about a single file name (hard link).
#[derive(Debug, Clone)]
pub struct NameInfo {
    /// The file name.
    pub name: String,
    /// Parent directory FRS.
    pub parent_frs: u64,
    /// Namespace (0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS).
    pub namespace: u8,
}
```

---

### Gap 3: Alternate Data Streams (🔴 CRITICAL)

**Current Rust Behavior:**
```rust
// crates/uffs-mft/src/io.rs:583-596
Some(AttributeType::Data) => {
    if attr_header.is_non_resident != 0 {
        // Only extracts size from unnamed $DATA
        let data_size = ...;
        result.size = data_size as u64;
    }
}
// ❌ Named streams (ADS) are completely ignored
```

**Reference Behavior:**
```cpp
// old_cpp_reference/uffs/file.cpp:2464-2512
StreamInfo *info = NULL;
// ... creates StreamInfo for EACH $DATA attribute
info->type_name_id = ah->Type >> 4;
info->name.length = ah->NameLength;  // ✅ Stores stream name
info->name.offset(...);
++base_record->stream_count;  // ✅ Tracks ALL streams
```

**Why This Matters:**
- Alternate Data Streams are used for:
  - `Zone.Identifier` - marks files downloaded from internet
  - Thumbnails and metadata
  - Malware hiding (security concern!)
- Without ADS support, these are invisible to searches
- Critical for security/forensics use cases

**Implementation Strategy:**
1. Parse ALL `$DATA` attributes, not just unnamed
2. Extract stream name from attribute header
3. Add `streams: Vec<StreamInfo>` to `ParsedRecord`
4. Each `StreamInfo` contains: name, size, allocated_size, is_sparse

**New Structs Required:**
```rust
/// Information about a single data stream.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// Stream name (empty for default stream).
    pub name: String,
    /// Logical size in bytes.
    pub size: u64,
    /// Allocated size on disk.
    pub allocated_size: u64,
    /// Whether this stream is sparse.
    pub is_sparse: bool,
    /// Attribute type (0x80 for $DATA, 0xC0 for $REPARSE_POINT, etc.).
    pub attribute_type: u8,
}
```

---

### Gap 4: Extended Size Information (🟡 IMPORTANT)

**Current Rust Behavior:**
```rust
// crates/uffs-mft/src/io.rs:589-593
let data_size = i64::from_le_bytes(...);  // Only DataSize
result.size = data_size as u64;
// ❌ No allocated_size, no compressed_size handling
```

**Reference Behavior:**
```cpp
// old_cpp_reference/uffs/file.cpp:2517-2525
info->allocated += ah->IsNonResident
    ? ah->NonResident.CompressionUnit
        ? static_cast<file_size_type>(ah->NonResident.CompressedSize)
        : static_cast<file_size_type>(ah->NonResident.AllocatedSize)
    : 0;
info->length += ah->IsNonResident
    ? static_cast<file_size_type>(ah->NonResident.DataSize)
    : ah->Resident.ValueLength;
info->bulkiness += info->allocated;
```

**Why This Matters:**
- `allocated_size` shows actual disk usage (important for disk analysis)
- `compressed_size` is needed for compressed files (NTFS compression)
- Without these, disk usage calculations are wrong
- `bulkiness` helps identify fragmented files

**Implementation Strategy:**
1. Add `allocated_size` and `compressed_size` to `ParsedRecord`
2. Check `CompressionUnit` to determine which size to use
3. Sum sizes across all streams for total file size

**Fields to Add:**
```rust
pub struct ParsedRecord {
    // ... existing fields ...
    /// Allocated size on disk (physical).
    pub allocated_size: u64,
    /// Compressed size (for compressed files).
    pub compressed_size: u64,
    /// Bulkiness metric (for fragmentation analysis).
    pub bulkiness: u64,
}
```

---

### Gap 5: Extended Attribute Flags (🟡 IMPORTANT)

**Current Rust Behavior:**
```rust
// crates/uffs-mft/src/io.rs:634
result.flags = (si.file_attributes & 0xFFFF) as u16;
// Stores as single u16 bitmask
```

**Reference Behavior:**
```cpp
// old_cpp_reference/uffs/file.cpp:1920-1935 (StandardInfo struct)
struct StandardInfo {
    unsigned long long created, written, accessed : 0x40 - 6;
    is_readonly : 1, is_archive : 1, is_system : 1, is_hidden : 1,
    is_offline : 1, is_notcontentidx : 1, is_noscrubdata : 1,
    is_integretystream : 1, is_pinned : 1, is_unpinned : 1,
    is_directory : 1, is_compressed : 1, is_encrypted : 1,
    is_sparsefile : 1, is_reparsepoint : 1;
};
```

**Why This Matters:**
- Individual boolean columns are easier to query in Polars
- `df.filter(col("is_hidden") & col("is_system"))` is cleaner than bit manipulation
- Some flags are not in the standard 16-bit mask (e.g., `is_pinned`, `is_unpinned`)

**Implementation Strategy:**
1. Extract all 15+ flags from `$STANDARD_INFORMATION`
2. Add individual boolean fields to `ParsedRecord`
3. Add individual boolean columns to DataFrame

**Flags to Extract:**
| Flag | Bit | Description |
|------|-----|-------------|
| `is_readonly` | 0x0001 | Read-only file |
| `is_hidden` | 0x0002 | Hidden file |
| `is_system` | 0x0004 | System file |
| `is_archive` | 0x0020 | Archive flag |
| `is_directory` | 0x0010 | Directory (from record flags) |
| `is_device` | 0x0040 | Device |
| `is_normal` | 0x0080 | Normal file |
| `is_temporary` | 0x0100 | Temporary file |
| `is_sparse` | 0x0200 | Sparse file |
| `is_reparse` | 0x0400 | Reparse point |
| `is_compressed` | 0x0800 | Compressed |
| `is_offline` | 0x1000 | Offline |
| `is_notcontentidx` | 0x2000 | Not content indexed |
| `is_encrypted` | 0x4000 | Encrypted |
| `is_integritystream` | 0x8000 | Integrity stream |
| `is_virtual` | 0x10000 | Virtual |
| `is_noscrubdata` | 0x20000 | No scrub data |
| `is_pinned` | 0x80000 | Pinned |
| `is_unpinned` | 0x100000 | Unpinned |

---

### Gap 6: Directory Tree Structure (🟢 OPTIONAL)

**Current Rust Behavior:**
- Only stores `parent_frs` for each record
- No child tracking
- No tree traversal capability

**Reference Behavior:**
```cpp
// old_cpp_reference/uffs/file.cpp:2407-2417
if (frs_parent != frs_base) {
    Records::iterator const parent = this->at(frs_parent, &base_record);
    ChildInfo *const child_info = &this->childinfos.back();
    child_info->record_number = frs_base;
    child_info->name_index = base_record->name_count;
    child_info->next_entry = parent->first_child;
    parent->first_child = child_index;
}
```

**Why This Matters:**
- Enables efficient tree traversal from root
- Required for `treesize` calculation (items in subtree)
- Useful for directory size calculations

**Implementation Strategy:**
1. Build child index as HashMap<parent_frs, Vec<child_frs>>
2. Post-process to calculate `treesize` for each directory
3. Add `treesize` column to DataFrame

**Note:** This is marked OPTIONAL because:
- Polars can do parent-child joins efficiently
- Tree traversal can be done with recursive queries
- The main use case (file search) doesn't need tree structure

---

### Gap 7: MFT Change Time (🟢 MODERATE)

**Current Rust Behavior:**
- Extracts: `created`, `modified`, `accessed`
- Missing: `mft_changed` (when MFT record was last modified)

**Reference Behavior:**
- Has access to all 4 timestamps from `$STANDARD_INFORMATION`

**Implementation Strategy:**
1. Add `mft_changed` field to `ParsedRecord`
2. Extract from `$STANDARD_INFORMATION.LastChangeTime`
3. Add `mft_changed` column to DataFrame

---

## DataFrame Schema Comparison

### Current Rust Schema (8 columns)
```
┌─────────────┬───────────┬─────────────────────────────────────┐
│ Column      │ Type      │ Description                         │
├─────────────┼───────────┼─────────────────────────────────────┤
│ frs         │ UInt64    │ File Record Segment number          │
│ parent_frs  │ UInt64    │ Parent directory FRS                │
│ name        │ String    │ File name (ONE only)                │
│ size        │ UInt64    │ Logical file size                   │
│ created     │ Datetime  │ Creation time                       │
│ modified    │ Datetime  │ Modification time                   │
│ accessed    │ Datetime  │ Access time                         │
│ flags       │ UInt16    │ File attributes bitmask             │
└─────────────┴───────────┴─────────────────────────────────────┘
```

### Target Schema for 100% Parity (25+ columns)
```
┌──────────────────┬────────────────┬─────────────────────────────────────┐
│ Column           │ Type           │ Description                         │
├──────────────────┼────────────────┼─────────────────────────────────────┤
│ frs              │ UInt64         │ File Record Segment number          │
│ parent_frs       │ UInt64         │ Primary parent directory FRS        │
│ name             │ String         │ Primary file name                   │
│ name_count       │ UInt16         │ Number of hard links                │
│ names            │ List[Struct]   │ All names [{name, parent, ns}]      │
│ stream_count     │ UInt16         │ Number of data streams              │
│ streams          │ List[Struct]   │ All streams [{name, size, alloc}]   │
│ size             │ UInt64         │ Logical file size                   │
│ allocated_size   │ UInt64         │ Physical allocation                 │
│ compressed_size  │ UInt64         │ Compressed size (if applicable)     │
│ bulkiness        │ UInt64         │ Fragmentation metric                │
│ treesize         │ UInt32         │ Items in subtree (directories)      │
│ created          │ Datetime       │ Creation time                       │
│ modified         │ Datetime       │ Modification time                   │
│ accessed         │ Datetime       │ Access time                         │
│ mft_changed      │ Datetime       │ MFT record change time              │
│ is_readonly      │ Boolean        │ Read-only flag                      │
│ is_hidden        │ Boolean        │ Hidden flag                         │
│ is_system        │ Boolean        │ System flag                         │
│ is_archive       │ Boolean        │ Archive flag                        │
│ is_directory     │ Boolean        │ Directory flag                      │
│ is_compressed    │ Boolean        │ Compressed flag                     │
│ is_encrypted     │ Boolean        │ Encrypted flag                      │
│ is_sparse        │ Boolean        │ Sparse file flag                    │
│ is_reparse       │ Boolean        │ Reparse point flag                  │
│ is_offline       │ Boolean        │ Offline flag                        │
│ is_notcontentidx │ Boolean        │ Not content indexed flag            │
└──────────────────┴────────────────┴─────────────────────────────────────┘
```

---

## Implementation Details

### Phase 1: Extension Record Merging

**File: `crates/uffs-mft/src/io.rs`**

**Step 1.1: Add ExtensionRecord struct**
```rust
/// Attributes extracted from an extension record.
#[derive(Debug, Clone, Default)]
pub struct ExtensionAttributes {
    /// The base FRS this extension belongs to.
    pub base_frs: u64,
    /// The extension's own FRS.
    pub extension_frs: u64,
    /// File names found in this extension.
    pub names: Vec<NameInfo>,
    /// Streams found in this extension.
    pub streams: Vec<StreamInfo>,
}
```

**Step 1.2: Modify parse_record() signature**
```rust
/// Result of parsing an MFT record.
pub enum ParseResult {
    /// A base record with all its data.
    Base(ParsedRecord),
    /// An extension record with attributes to merge.
    Extension(ExtensionAttributes),
    /// Record is not in use or invalid.
    Skip,
}

pub fn parse_record(data: &[u8], frs: u64) -> ParseResult {
    // ... existing validation ...

    if !header.is_base_record() {
        // Parse extension record instead of skipping
        return parse_extension_record(data, frs, header);
    }

    // ... rest of base record parsing ...
}
```

**Step 1.3: Create MftRecordMerger**
```rust
/// Merges extension record attributes into base records.
pub struct MftRecordMerger {
    /// Base records indexed by FRS.
    base_records: HashMap<u64, ParsedRecord>,
    /// Pending extension attributes.
    extensions: Vec<ExtensionAttributes>,
}

impl MftRecordMerger {
    pub fn new() -> Self { ... }

    pub fn add_result(&mut self, result: ParseResult) {
        match result {
            ParseResult::Base(record) => {
                self.base_records.insert(record.frs, record);
            }
            ParseResult::Extension(ext) => {
                self.extensions.push(ext);
            }
            ParseResult::Skip => {}
        }
    }

    pub fn merge(mut self) -> Vec<ParsedRecord> {
        // Merge all extensions into their base records
        for ext in self.extensions {
            if let Some(base) = self.base_records.get_mut(&ext.base_frs) {
                base.names.extend(ext.names);
                base.streams.extend(ext.streams);
            }
        }
        self.base_records.into_values().collect()
    }
}
```

---

### Phase 2: Hard Link Support

**File: `crates/uffs-mft/src/ntfs.rs`**

**Step 2.1: Add NameInfo struct**
```rust
/// Information about a single file name (hard link).
#[derive(Debug, Clone)]
pub struct NameInfo {
    /// The file name.
    pub name: String,
    /// Parent directory FRS.
    pub parent_frs: u64,
    /// Namespace (0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS).
    pub namespace: u8,
}
```

**File: `crates/uffs-mft/src/io.rs`**

**Step 2.2-2.4: Update ParsedRecord and parsing**
```rust
pub struct ParsedRecord {
    pub frs: u64,
    /// All file names (hard links). First is "primary".
    pub names: Vec<NameInfo>,
    /// Convenience: primary parent FRS.
    pub parent_frs: u64,
    /// Convenience: primary name.
    pub name: String,
    // ... other fields ...
}

fn parse_file_name(...) {
    // Don't skip additional names!
    if namespace != 2 {  // Not DOS-only
        let name_info = NameInfo {
            name: extract_name(...),
            parent_frs: fn_attr.parent_directory,
            namespace,
        };
        result.names.push(name_info);

        // Update primary name if this is better
        if is_better_name {
            result.name = name_info.name.clone();
            result.parent_frs = name_info.parent_frs;
        }
    }
}
```

**File: `crates/uffs-mft/src/reader.rs`**

**Step 2.5: Add names column to DataFrame**
```rust
// Option A: Expand to multiple rows (one per name)
// Easier for queries but larger DataFrame

// Option B: Store as List column (recommended)
use polars::prelude::*;

let names_series = Series::new(
    "names".into(),
    parsed_records.iter().map(|r| {
        Series::new("".into(), r.names.iter().map(|n| &n.name).collect::<Vec<_>>())
    }).collect::<Vec<_>>()
);
```

---

### Phase 3: Alternate Data Streams

**File: `crates/uffs-mft/src/ntfs.rs`**

**Step 3.1: Add StreamInfo struct**
```rust
/// Information about a single data stream.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// Stream name (empty for default stream).
    pub name: String,
    /// Logical size in bytes.
    pub size: u64,
    /// Allocated size on disk.
    pub allocated_size: u64,
    /// Whether this stream is sparse.
    pub is_sparse: bool,
    /// Attribute type code.
    pub attribute_type: u32,
}
```

**File: `crates/uffs-mft/src/io.rs`**

**Step 3.3-3.5: Parse all $DATA attributes**
```rust
fn parse_data_attribute(
    data: &[u8],
    attr_offset: usize,
    header: &AttributeRecordHeader,
    result: &mut ParsedRecord,
) {
    // Extract stream name from attribute header
    let name = if header.name_length > 0 {
        let name_offset = attr_offset + header.name_offset as usize;
        let name_bytes = &data[name_offset..name_offset + header.name_length as usize * 2];
        String::from_utf16_lossy(
            &name_bytes.chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>()
        )
    } else {
        String::new()
    };

    let (size, allocated_size) = if header.is_non_resident != 0 {
        // Non-resident: get sizes from header
        let nr = parse_non_resident_header(...);
        (nr.data_size, nr.allocated_size)
    } else {
        // Resident: size is value_length, allocated is 0
        (header.value_length as u64, 0)
    };

    let stream_info = StreamInfo {
        name,
        size,
        allocated_size,
        is_sparse: (header.flags & 0x8000) != 0,
        attribute_type: header.type_code,
    };

    result.streams.push(stream_info);

    // Update primary size from default stream
    if stream_info.name.is_empty() && stream_info.attribute_type == 0x80 {
        result.size = stream_info.size;
        result.allocated_size = stream_info.allocated_size;
    }
}
```

---

## Testing Strategy

### Unit Tests

Each phase should include unit tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Phase 1: Extension record merging
    #[test]
    fn test_extension_record_detection() {
        let mut header = FileRecordSegmentHeader::default();
        header.base_file_record_segment = 0x12345;
        assert!(!header.is_base_record());
    }

    #[test]
    fn test_extension_merge() {
        let mut merger = MftRecordMerger::new();
        merger.add_result(ParseResult::Base(base_record));
        merger.add_result(ParseResult::Extension(ext_attrs));
        let records = merger.merge();
        assert_eq!(records[0].names.len(), 2);
    }

    // Phase 2: Hard links
    #[test]
    fn test_multiple_names() {
        let record = parse_record(&data_with_two_names, 100);
        assert_eq!(record.names.len(), 2);
    }

    // Phase 3: Alternate data streams
    #[test]
    fn test_alternate_data_stream() {
        let record = parse_record(&data_with_ads, 100);
        assert!(record.streams.iter().any(|s| s.name == "Zone.Identifier"));
    }
}
```

### Integration Tests

Test with real MFT data:

```rust
#[tokio::test]
#[cfg(windows)]
async fn test_hard_link_detection() {
    // Create a file with hard link
    // Read MFT
    // Verify both names are found
}

#[tokio::test]
#[cfg(windows)]
async fn test_ads_detection() {
    // Create a file with ADS
    // Read MFT
    // Verify stream is found
}
```

---

## Estimated Effort

| Phase | Complexity | Estimated Time | Priority |
|-------|------------|----------------|----------|
| Phase 1: Extension Records | High | 4-6 hours | 🔴 Critical |
| Phase 2: Hard Links | Medium | 2-3 hours | 🔴 Critical |
| Phase 3: Alternate Streams | Medium | 2-3 hours | 🔴 Critical |
| Phase 4: Extended Sizes | Low | 1-2 hours | 🟡 Important |
| Phase 5: Extended Flags | Low | 1-2 hours | 🟡 Important |
| Phase 6: Directory Tree | High | 4-6 hours | ✅ Complete |
| Phase 7: MFT Change Time | Low | 0.5 hours | 🟢 Moderate |
| **Total** | | **15-23 hours** | |

---

## Success Criteria

The implementation is complete when:

1. ✅ Extension records are merged into base records
2. ✅ All hard links are captured (multiple names per file)
3. ✅ All alternate data streams are captured
4. ✅ Allocated and compressed sizes are tracked
5. ✅ All 15+ attribute flags are extracted
6. ✅ DataFrame schema matches target schema
7. ✅ All unit tests pass
8. ✅ Integration tests with real MFT data pass
9. ✅ Performance is not significantly degraded

---

## References

- **Reference baseline:** `old_cpp_reference/uffs/UltraFastFileSearch-code/file.cpp` (local-only, never pushed)
  - Lines 939-1193: NTFS structures
  - Lines 1884-2090: NtfsIndex data structures
  - Lines 2370-2530: Record parsing logic

- **Rust Implementation:** `crates/uffs-mft/src/`
  - `ntfs.rs`: NTFS structures
  - `io.rs`: Record parsing
  - `reader.rs`: DataFrame building
  - `platform.rs`: Windows API wrappers

