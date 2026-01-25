# Enhanced MFT Parsing: Statistics Collection & Filename Component Extraction

**Status**: Planning
**Date**: 2026-01-25
**Owner**: uffs_mft crate
**Consumers**: uffs-cli, uffs-tui, uffs-gui

---

## 1. Overview

### Motivation

During MFT reading and parsing, we have the opportunity to pre-digest data that will be valuable for downstream tools. By collecting rich statistics and parsing filename components **once during indexing**, we enable:

- **Instant statistics display** without re-scanning the index
- **Faster extension-based queries** (O(1) instead of O(n) string scans)
- **Rich analytics** for TUI/GUI tools (size charts, type distribution, etc.)
- **Better user insights** into disk usage patterns

This follows the **"pay once, benefit forever"** philosophy: invest minimal CPU/memory during indexing (~5-15 ms per million files) to enable fast queries and rich features downstream.

### Scope

This enhancement is implemented entirely in the **`uffs_mft` crate** during the MFT reading/parsing phase. The enriched `MftIndex` and `MftStats` are then passed to downstream tools (`uffs-cli`, `uffs-tui`, `uffs-gui`) which can leverage the pre-digested data without additional processing.

---

## 2. Current State

### Existing Statistics (in `MftStats`)

```rust
pub struct MftStats {
    pub record_count: u32,           // Total records
    pub dir_count: u32,              // Directories
    pub file_count: u32,             // Files
    pub max_frs: u64,                // Max FRS number
    pub total_name_bytes: u64,       // Total filename bytes
    pub multi_name_count: u32,       // Files with hard links
    pub ads_count: u32,              // Files with ADS
    pub system_metafile_count: u32,  // System metafiles
    pub system_child_count: u32,     // Children of system metafiles
}
```

### Existing Filename Storage

```rust
pub struct IndexNameRef {
    pub offset: u32,   // Offset in MftIndex::names buffer
    pub length: u16,   // Total length of filename
    pub flags: u16,    // is_ascii, etc.
}
```

Filenames are stored as complete strings in a single buffer (`MftIndex::names`). No component parsing is performed.

### Limitations

1. **No extension parsing**: Queries like "*.txt" require scanning every filename
2. **Limited attribute stats**: Only count ADS and hard links, not other attributes
3. **No size distribution**: Can't answer "how many files are > 1GB?"
4. **No extension histogram**: Can't show "top 10 file types"
5. **No depth statistics**: Can't analyze directory tree structure

---

## 3. Proposed Enhancements

### 3.1 Filename Component Parsing

**Goal**: Split filenames into components during parsing for O(1) extension queries.

**Components**:
- **Base name**: "doc1_hardlink" (filename without extension)
- **Extension**: ".txt" (including the dot)
- **Stream name**: ":comments" (Alternate Data Stream - already handled separately)

**Implementation**: Add `ext_dot_pos` to `IndexNameRef`:

```rust
pub struct IndexNameRef {
    pub offset: u32,      // Offset in names buffer
    pub length: u16,      // Total length
    pub flags: u16,       // is_ascii, etc.
    pub ext_dot_pos: u16, // Position of '.', or 0 if no extension
}
```

**Parsing logic**:
```rust
fn find_extension_dot(name: &str) -> u16 {
    // Find last '.' that's not at the start (to handle ".gitignore")
    name.rfind('.').filter(|&pos| pos > 0).unwrap_or(0) as u16
}
```

**Edge cases**:
- No extension: "README" → `ext_dot_pos = 0`
- Hidden files: ".gitignore" → `ext_dot_pos = 0` (leading '.' doesn't count)
- Multiple dots: "archive.tar.gz" → `ext_dot_pos = 11` (last '.')
- Directories: Usually no extension → `ext_dot_pos = 0`

**Cost**:
- Memory: +2 bytes per record (for 1M files: +2 MB)
- CPU: ~2-4 ms per 1M files (backward scan to find '.')

### 3.2 Extended Statistics Collection

#### A. Attribute Counters (Simple Counters)

**Goal**: Count files by Windows attributes for security/analytics.

**New fields in `MftStats`**:
```rust
pub struct MftStats {
    // ... existing fields ...

    // Attribute counters (Windows FILE_ATTRIBUTE_*)
    pub hidden_count: u32,        // Hidden attribute
    pub system_count: u32,        // System attribute
    pub readonly_count: u32,      // Read-only attribute
    pub compressed_count: u32,    // Compressed attribute
    pub encrypted_count: u32,     // Encrypted attribute
    pub sparse_count: u32,        // Sparse attribute
    pub reparse_count: u32,       // Reparse point (symlink, junction)
    pub dotfile_count: u32,       // Unix-style hidden (starts with '.')
}
```

**Cost**:
- Memory: 8 counters × 4 bytes = 32 bytes
- CPU: ~1 ns per increment × 8 = ~8 ns per file → ~8 ms per 1M files

#### B. Size Distribution Buckets

**Goal**: Understand file size distribution for capacity planning.

**New field in `MftStats`**:
```rust
pub struct MftStats {
    // ... existing fields ...

    // Size distribution (8 buckets)
    pub size_buckets: [u32; 8],
    // Buckets: [0-1KB, 1-10KB, 10-100KB, 100KB-1MB, 1-10MB, 10-100MB, 100MB-1GB, >1GB]
}
```


---

## 4. Technical Design

### 4.1 Where to Implement

**Primary locations** (in `crates/uffs-mft/src/io.rs`):

1. **`parse_record_to_index()`** (line ~1430)
   - Single-threaded inline parsing during IOCP
   - Add filename component parsing here
   - Update stats counters

2. **`parse_record_to_fragment()`** (line ~1730)
   - Parallel parsing variant
   - Each worker builds its own `MftIndexFragment` and `MftStats`
   - Fragments are merged at the end

**Data flow**:
```
MFT Record → parse_record_to_index() → {
    1. Extract filename → find_extension_dot() → store ext_dot_pos
    2. Extract attributes → update attribute counters
    3. Extract size → update size_buckets
    4. Record extension → ExtensionStats::record()
    5. Store in MftIndex
}
```

### 4.2 Enhanced Data Structures

#### IndexNameRef (in `crates/uffs-mft/src/index.rs`)

```rust
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IndexNameRef {
    pub offset: u32,      // Offset in names buffer (4 bytes)
    pub length: u16,      // Total length (2 bytes)
    pub flags: u16,       // is_ascii, etc. (2 bytes)
    pub ext_dot_pos: u16, // Position of '.', or 0 if no extension (2 bytes)
}
// Total: 10 bytes (was 8 bytes, +2 bytes overhead)
```

**Helper methods**:
```rust
impl MftIndex {
    /// Get the base name (without extension)
    pub fn base_name(&self, name_ref: &IndexNameRef) -> &str {
        let full_name = self.get_name(name_ref);
        if name_ref.ext_dot_pos > 0 {
            &full_name[..name_ref.ext_dot_pos as usize]
        } else {
            full_name
        }
    }

    /// Get the extension (including the dot, e.g., ".txt")
    pub fn extension(&self, name_ref: &IndexNameRef) -> &str {
        let full_name = self.get_name(name_ref);
        if name_ref.ext_dot_pos > 0 {
            &full_name[name_ref.ext_dot_pos as usize..]
        } else {
            ""
        }
    }
}
```

#### MftStats (in `crates/uffs-mft/src/index.rs`)

```rust
#[derive(Debug, Clone, Default)]
pub struct MftStats {
    // ===== Existing fields =====
    pub record_count: u32,
    pub dir_count: u32,
    pub file_count: u32,
    pub max_frs: u64,
    pub total_name_bytes: u64,
    pub multi_name_count: u32,
    pub ads_count: u32,
    pub system_metafile_count: u32,
    pub system_child_count: u32,

    // ===== NEW: Attribute counters =====
    pub hidden_count: u32,        // Windows Hidden attribute
    pub system_count: u32,        // Windows System attribute
    pub readonly_count: u32,      // Read-only attribute
    pub compressed_count: u32,    // Compressed attribute
    pub encrypted_count: u32,     // Encrypted attribute
    pub sparse_count: u32,        // Sparse attribute
    pub reparse_count: u32,       // Reparse point (symlink, junction)
    pub dotfile_count: u32,       // Unix-style hidden (starts with '.')

    // ===== NEW: Size distribution =====
    pub size_buckets: [u32; 8],   // [0-1KB, 1-10KB, ..., >1GB]
}
```

**Memory overhead**: 8 counters × 4 bytes + 8 buckets × 4 bytes = **64 bytes**

#### ExtensionStats (new file: `crates/uffs-mft/src/extension_stats.rs`)

```rust
/// Extension statistics collected during MFT parsing.
///
/// Uses Vec instead of HashMap for better cache locality and performance
/// with small number of unique extensions (typically 100-1000).
#[derive(Debug, Clone, Default)]
pub struct ExtensionStats {
    /// Extension counts (unsorted during collection)
    /// Format: [(".txt", 123456), (".jpg", 98765), ...]
    extensions: Vec<(String, u32)>,
}

impl ExtensionStats {
    /// Create new empty stats
    pub fn new() -> Self {
        Self {
            extensions: Vec::with_capacity(500), // Pre-allocate for typical case
        }
    }

    /// Record a file extension (called during parsing)
    ///
    /// Uses linear search which is faster than HashMap for small n
    /// due to cache locality and Zipf distribution of extensions.
    pub fn record(&mut self, ext: &str) {
        if let Some(entry) = self.extensions.iter_mut().find(|(e, _)| e == ext) {
            entry.1 += 1;
        } else {
            self.extensions.push((ext.to_string(), 1));
        }
    }

    /// Finalize stats (sort by count descending)
    ///
    /// Call this once after all files are parsed.
    pub fn finalize(&mut self) {
        self.extensions.sort_by(|a, b| b.1.cmp(&a.1));
    }

    /// Get top N extensions
    pub fn top_n(&self, n: usize) -> &[(String, u32)] {
        &self.extensions[..n.min(self.extensions.len())]
    }

    /// Get all extensions (sorted by count if finalized)
    pub fn all(&self) -> &[(String, u32)] {
        &self.extensions
    }

    /// Get count for a specific extension
    pub fn count(&self, ext: &str) -> u32 {
        self.extensions.iter()
            .find(|(e, _)| e == ext)
            .map(|(_, count)| *count)
            .unwrap_or(0)
    }

    /// Total number of unique extensions
    pub fn unique_count(&self) -> usize {
        self.extensions.len()
    }

    /// Merge another ExtensionStats into this one (for parallel parsing)
    pub fn merge(&mut self, other: &ExtensionStats) {
        for (ext, count) in &other.extensions {
            if let Some(entry) = self.extensions.iter_mut().find(|(e, _)| e == ext) {
                entry.1 += count;
            } else {
                self.extensions.push((ext.clone(), *count));
            }
        }
    }
}
```

### 4.3 Integration with MftIndex

**Add to `MftIndex` structure**:
```rust
pub struct MftIndex {
    // ... existing fields ...
    pub stats: MftStats,

    // NEW: Extension statistics
    pub extension_stats: ExtensionStats,
}
```

**Merge logic for parallel parsing**:
```rust
impl MftIndex {
    pub fn merge_fragments(fragments: Vec<MftIndexFragment>) -> Result<Self> {
        // ... existing merge logic ...

        // Merge extension stats from all fragments
        let mut extension_stats = ExtensionStats::new();
        for fragment in &fragments {
            extension_stats.merge(&fragment.extension_stats);
        }
        extension_stats.finalize(); // Sort by count

        // ... rest of merge ...
    }
}
```

---

## 5. Performance Analysis

### 5.1 Memory Overhead

| Component | Per Record | Per 1M Files | Notes |
|-----------|------------|--------------|-------|
| `ext_dot_pos` in IndexNameRef | +2 bytes | +2 MB | Position of '.' |
| Attribute counters | 0 bytes | 32 bytes | Shared counters |
| Size buckets | 0 bytes | 32 bytes | Shared buckets |
| Extension histogram | 0 bytes | ~2-20 KB | ~100-1000 unique extensions |
| **TOTAL** | **+2 bytes** | **+2 MB + 64 bytes + 20 KB** | **~2.02 MB per 1M files** |

**Impact**: For a typical 1M file index (~600 MB), this adds **~2 MB (0.3% overhead)** - negligible.

### 5.2 CPU Overhead

| Operation | Per File | Per 1M Files | Notes |
|-----------|----------|--------------|-------|
| Find extension dot | ~2 ns | ~2 ms | Backward scan to find '.' |
| Update attribute counters | ~8 ns | ~8 ms | 8 counter increments |
| Update size bucket | ~2 ns | ~2 ms | Match + increment |
| Record extension | ~5-10 ns | ~5-10 ms | Linear search in Vec |
| **TOTAL** | **~17-22 ns** | **~17-22 ms** | **Per 1M files** |

**Impact**: For a typical indexing time of 40-50 seconds, this adds **~20 ms (0.04% overhead)** - negligible.

### 5.3 Query Speedup

| Query Type | Before | After | Speedup |
|------------|--------|-------|---------|
| "*.txt" search | O(n) string scan | O(1) comparison | **10-100x faster** |
| "Show top file types" | Full scan + sort | Pre-computed | **Instant** |
| "Files > 1GB" | Full scan | Bucket lookup | **1000x faster** |
| "Hidden files count" | Full scan | Counter lookup | **Instant** |

---

## 6. Implementation Milestones

### Phase 1: Filename Component Parsing ⬜ NOT STARTED

**Goal**: Add `ext_dot_pos` to `IndexNameRef` and parse during MFT reading.

**Tasks**:
- [ ] Add `ext_dot_pos: u16` field to `IndexNameRef` struct
- [ ] Implement `find_extension_dot()` helper function
- [ ] Update `parse_record_to_index()` to extract and store extension position
- [ ] Update `parse_record_to_fragment()` for parallel parsing
- [ ] Add `base_name()` and `extension()` helper methods to `MftIndex`
- [ ] Update serialization/deserialization for new field
- [ ] Add unit tests for edge cases (no ext, hidden files, multiple dots)

**Estimated effort**: 4-6 hours
**Dependencies**: None
**Validation**: Run on test MFT, verify extension parsing for all edge cases

---

### Phase 2: Basic Attribute Counters ⬜ NOT STARTED

**Goal**: Add attribute counters to `MftStats` and update during parsing.

**Tasks**:
- [ ] Add 8 new counter fields to `MftStats` struct
- [ ] Update `MftStats::new()` to initialize new counters
- [ ] Add counter update logic in `parse_record_to_index()`
- [ ] Add counter update logic in `parse_record_to_fragment()`
- [ ] Implement counter merging in `merge_fragments()`
- [ ] Add display formatting for new stats
- [ ] Add unit tests for counter accuracy

**Estimated effort**: 3-4 hours
**Dependencies**: None (can be done in parallel with Phase 1)
**Validation**: Run on test MFT, verify counts match manual inspection

---

### Phase 3: Size Distribution Buckets ⬜ NOT STARTED

**Goal**: Add size distribution tracking to `MftStats`.

**Tasks**:
- [ ] Add `size_buckets: [u32; 8]` field to `MftStats`
- [ ] Implement `size_bucket()` helper function
- [ ] Update parsing functions to classify files into buckets
- [ ] Implement bucket merging for parallel parsing
- [ ] Add display formatting (histogram, percentages)
- [ ] Add unit tests for bucket classification

**Estimated effort**: 2-3 hours
**Dependencies**: None (can be done in parallel with Phase 1 & 2)
**Validation**: Run on test MFT, verify distribution makes sense

---

### Phase 4: Extension Histogram ⬜ NOT STARTED

**Goal**: Implement `ExtensionStats` and collect extension counts.

**Tasks**:
- [ ] Create new file `crates/uffs-mft/src/extension_stats.rs`
- [ ] Implement `ExtensionStats` struct with Vec-based counting
- [ ] Add `extension_stats` field to `MftIndex` and `MftIndexFragment`
- [ ] Update parsing functions to record extensions
- [ ] Implement `merge()` for parallel parsing
- [ ] Implement `finalize()` to sort by count
- [ ] Add helper methods (`top_n()`, `count()`, etc.)
- [ ] Add to module exports in `lib.rs`
- [ ] Add unit tests for counting and merging

**Estimated effort**: 4-5 hours
**Dependencies**: Phase 1 (needs extension parsing)
**Validation**: Run on test MFT, verify top extensions match expectations

---

### Phase 5: Integration & CLI Display ⬜ NOT STARTED

**Goal**: Display rich statistics in `uffs-cli` after indexing.

**Tasks**:
- [ ] Add `--show-stats` flag to `uffs index` command
- [ ] Implement stats display formatting in `uffs-cli/src/commands.rs`
- [ ] Add color-coded output for better readability
- [ ] Add percentage calculations for distributions
- [ ] Add optional JSON output format for scripting
- [ ] Update documentation with examples
- [ ] Add integration tests

**Example output**:
```
📊 Index Statistics for C:\

  Files & Directories:
    Total records: 1,234,567
    Files: 1,188,889 (96.3%)
    Directories: 45,678 (3.7%)

  Special Files:
    Hard links: 123 files
    Alternate Data Streams: 45 files

  Attributes:
    Hidden: 12,345 files (1.0%)
    System: 567 files (0.05%)
    Read-only: 23,456 files (1.9%)
    Compressed: 56,789 files (4.6%)
    Encrypted: 1,234 files (0.1%)
    Sparse: 567 files (0.05%)
    Reparse points: 89 files (0.01%)
    Unix hidden (.): 234 files (0.02%)

  Size Distribution:
    0-1 KB:        234,567 files (19.7%) ████████████████████
    1-10 KB:       345,678 files (29.1%) █████████████████████████████
    10-100 KB:     234,567 files (19.7%) ████████████████████
    100 KB-1 MB:   123,456 files (10.4%) ██████████
    1-10 MB:        45,678 files (3.8%)  ████
    10-100 MB:      12,345 files (1.0%)  █
    100 MB-1 GB:     2,345 files (0.2%)
    > 1 GB:            234 files (0.02%)

  Top 10 File Types:
    .txt:     123,456 files (10.4%)
    .jpg:      98,765 files (8.3%)
    .pdf:      45,678 files (3.8%)
    .docx:     34,567 files (2.9%)
    .xlsx:     23,456 files (2.0%)
    .log:      12,345 files (1.0%)
    .dll:      11,234 files (0.9%)
    .exe:       9,876 files (0.8%)
    .png:       8,765 files (0.7%)
    (no ext):   7,654 files (0.6%)

  Indexing completed in 42.3s
```

**Estimated effort**: 6-8 hours
**Dependencies**: Phases 1-4
**Validation**: Manual testing, user feedback

---

### Phase 6: Per-Directory Sorting ⬜ NOT STARTED

**Goal**: Sort children within each directory by filename for natural directory listings.

**Tasks**:
- [ ] Implement `sort_directory_children()` method on `MftIndex`
- [ ] Add helper method `get_children_frs_mut()` for mutable access to children
- [ ] Add helper method `get_name_for_frs()` for quick name lookup by FRS
- [ ] Call `sort_directory_children()` after `from_parsed_records()`
- [ ] Call `sort_directory_children()` after `merge_fragments()`
- [ ] Add unit tests for sorting correctness (case-insensitive, edge cases)
- [ ] Add benchmark to measure sorting overhead
- [ ] Update documentation

**Estimated effort**: 3-4 hours
**Dependencies**: None (can be done in parallel with other phases)
**Validation**: Verify directory listings are sorted, overhead < 50 ms per 1M files

---

### Phase 7: Eager Tree Metrics Computation ⬜ NOT STARTED

**Goal**: Compute and store tree metrics directly in `FileRecord` during index building.

**Rationale**:
Currently, tree metrics (descendants count, total size, total allocated size) are computed on-demand by the separate `TreeIndex` module in `uffs-core`. This requires:
- Converting MftIndex to DataFrame
- Building a separate TreeIndex structure
- Computing metrics lazily when needed

For CLI use cases, we **always** need tree metrics for C++ parity (to populate Size/Size on Disk columns for directories). By computing these metrics eagerly during index building, we:
- Eliminate the separate TreeIndex step
- Make metrics immediately available in MftIndex
- Simplify the API for downstream tools
- Follow the "pay once, benefit forever" philosophy

**Tasks**:
- [ ] Add `descendants: u64` field to `FileRecord` (count of all descendants)
- [ ] Add `treesize: u64` field to `FileRecord` (sum of real sizes in subtree)
- [ ] Add `tree_allocated: u64` field to `FileRecord` (sum of disk sizes in subtree)
- [ ] Implement `compute_tree_metrics()` method on `MftIndex`
  - Use bottom-up traversal with memoization (same algorithm as TreeIndex)
  - Populate tree metrics for all directory records
  - Files get their own size/allocated_size as tree metrics
- [ ] Call `compute_tree_metrics()` after building index in `from_parsed_records()`
- [ ] Call `compute_tree_metrics()` after merging in `merge_fragments()`
- [ ] Update serialization/deserialization for new fields
- [ ] Update `results_to_dataframe()` in CLI to use built-in tree metrics
- [ ] Remove dependency on separate `TreeIndex` in CLI code
- [ ] Add unit tests comparing results with TreeIndex
- [ ] Add benchmark to measure computation overhead

**Implementation details**:

```rust
// In crates/uffs-mft/src/index.rs

#[derive(Debug, Clone, Default)]
#[repr(C)]
pub struct FileRecord {
    pub frs: u64,
    pub stdinfo: StandardInfo,
    pub name_count: u16,
    pub stream_count: u16,
    pub first_child: u32,
    pub first_name: LinkInfo,
    pub first_stream: IndexStreamInfo,

    // NEW: Tree metrics (computed after all records parsed)
    pub descendants: u64,      // Count of all descendants (0 for files)
    pub treesize: u64,         // Sum of real sizes in subtree
    pub tree_allocated: u64,   // Sum of disk sizes in subtree
}

impl MftIndex {
    /// Compute tree metrics for all records.
    ///
    /// This is called once after all records are parsed, before returning
    /// the index. It uses bottom-up traversal with memoization to compute
    /// descendants count and size totals for all directories.
    fn compute_tree_metrics(&mut self) {
        use std::collections::HashMap;

        // Cache for memoization
        let mut metrics_cache: HashMap<u64, (u64, u64, u64)> = HashMap::new();

        // Compute metrics for all records
        for idx in 0..self.records.len() {
            let frs = self.records[idx].frs;
            let (descendants, treesize, tree_allocated) =
                self.compute_metrics_recursive(frs, &mut metrics_cache);

            // Store in record
            self.records[idx].descendants = descendants;
            self.records[idx].treesize = treesize;
            self.records[idx].tree_allocated = tree_allocated;
        }
    }

    fn compute_metrics_recursive(
        &self,
        frs: u64,
        cache: &mut HashMap<u64, (u64, u64, u64)>,
    ) -> (u64, u64, u64) {
        // Check cache first
        if let Some(&metrics) = cache.get(&frs) {
            return metrics;
        }

        // Get record
        let record = &self.records[self.frs_to_idx[frs as usize] as usize];

        // Base metrics from this node
        let mut descendants = 0u64;
        let mut treesize = record.stdinfo.size;
        let mut tree_allocated = record.stdinfo.allocated_size;

        // If directory, add children's metrics
        if record.stdinfo.is_directory() {
            let children_frs = self.get_children_frs(frs);
            for child_frs in children_frs {
                let (child_desc, child_size, child_alloc) =
                    self.compute_metrics_recursive(child_frs, cache);
                descendants += 1 + child_desc;
                treesize += child_size;
                tree_allocated += child_alloc;
            }
        }

        // Cache and return
        let metrics = (descendants, treesize, tree_allocated);
        cache.insert(frs, metrics);
        metrics
    }
}
```

**Cost analysis**:
- **Memory**: +24 bytes per record (3 × u64)
  - For 1M files: +24 MB (~4% increase from 600 MB to 624 MB)
- **CPU**: ~50-100 ms per 1M files (same as TreeIndex)
  - O(n) time complexity with memoization
  - 0.1-0.2% of total indexing time (40-50 seconds)

**Benefits**:
- ✅ **Simpler API**: No separate TreeIndex step needed
- ✅ **Faster queries**: Tree metrics always available (no on-demand computation)
- ✅ **Better for CLI**: Always need tree metrics for C++ parity
- ✅ **Consistent**: Fits "pay once, benefit forever" philosophy
- ✅ **Less code**: Remove TreeIndex dependency from CLI

**Estimated effort**: 5-7 hours
**Dependencies**: None (uses existing parent-child relationships from `children` index)
**Validation**:
- Verify tree metrics match TreeIndex results exactly
- Verify overhead is ~50-100 ms per 1M files
- Verify memory increase is ~24 MB per 1M files

---

### Phase 8: Performance Validation ⬜ NOT STARTED

**Goal**: Verify overhead is within acceptable limits.

**Tasks**:
- [ ] Benchmark indexing with/without stats collection
- [ ] Benchmark indexing with/without per-directory sorting
- [ ] Benchmark indexing with/without eager tree metrics
- [ ] Verify memory overhead is < 5% of total index size
- [ ] Verify CPU overhead is < 0.2% of total indexing time
- [ ] Profile with Visual Studio CPU Usage tool
- [ ] Profile with Visual Studio File I/O tool
- [ ] Test on various drive types (HDD, SSD, NVMe)
- [ ] Test on various MFT sizes (100K, 1M, 10M files)
- [ ] Document performance results

**Estimated effort**: 4-6 hours
**Dependencies**: Phases 1-7
**Validation**: Performance meets targets (< 5% memory, < 0.2% CPU overhead)

---

## 7. Benefits for Downstream Tools

### 7.1 uffs-cli

**Immediate benefits**:
- Rich statistics display after indexing (see Phase 5 example)
- Faster extension-based searches: `uffs "*.txt"` uses O(1) comparison
- Instant answers to queries like "how many hidden files?"
- JSON output for scripting and automation

**Future enhancements**:
- `uffs stats` command to show statistics without searching
- `uffs analyze` command for disk usage analysis
- Extension-based filtering: `uffs --ext txt,pdf,docx`

### 7.2 uffs-tui

**Immediate benefits**:
- Display statistics in sidebar or status bar
- Extension-based filtering in search UI
- Size distribution histogram visualization
- Attribute-based filtering (show only hidden files, etc.)

**Future enhancements**:
- Interactive charts (size distribution, extension pie chart)
- Drill-down from stats to file list
- Real-time stats updates during search

### 7.3 uffs-gui

**Immediate benefits**:
- Visual analytics dashboard with charts
- Extension pie chart (top 10 file types)
- Size distribution histogram
- Attribute breakdown (hidden, compressed, encrypted, etc.)

**Future enhancements**:
- Interactive treemap of disk usage by extension
- Drill-down from charts to file explorer
- Export charts as images for reports
- Comparison between multiple drives

---

## 8. Summary

This enhancement adds **rich statistics collection, filename component parsing, per-directory sorting, and eager tree metrics** to the MFT reading phase in the `uffs_mft` crate. The overhead is **minimal** (~26 MB memory, ~110-165 ms CPU per 1M files), while the benefits are **significant**:

- ✅ **10-100x faster** extension-based queries
- ✅ **Instant** statistics display (no re-scanning)
- ✅ **Rich analytics** for TUI/GUI tools
- ✅ **Better user insights** into disk usage
- ✅ **Natural sorted directory listings** (like Windows Explorer)
- ✅ **Tree metrics always available** (no separate TreeIndex step)
- ✅ **Simpler API** for downstream tools

The implementation is **incremental** (8 phases) and can be done over ~33-46 hours of development time. Each phase is independently testable and provides immediate value.

### Implementation Phases Summary

| Phase | Feature | Effort | Memory Overhead | CPU Overhead | Benefits |
|-------|---------|--------|-----------------|--------------|----------|
| 1 | Filename component parsing | 4-6h | +2 MB | ~2-4 ms | O(1) extension queries |
| 2 | Attribute counters | 3-4h | +32 bytes | ~8 ms | Instant attribute stats |
| 3 | Size distribution buckets | 2-3h | +32 bytes | ~2-3 ms | Instant size analytics |
| 4 | Extension histogram | 4-5h | +2-20 KB | ~5-10 ms | Top file types display |
| 5 | CLI display integration | 6-8h | 0 | 0 | Rich stats output |
| 6 | Per-directory sorting | 3-4h | 0 | ~43 ms | Natural directory listings |
| 7 | Eager tree metrics | 5-7h | +24 MB | ~50-100 ms | Simpler API, always available |
| 8 | Performance validation | 4-6h | 0 | 0 | Verify targets met |
| **TOTAL** | **All enhancements** | **33-46h** | **~26 MB** | **~110-165 ms** | **Comprehensive** |

**Total overhead**: For 1M files:
- **Memory**: ~26 MB (~4.3% increase from 600 MB to 626 MB)
- **CPU**: ~110-165 ms (~0.2-0.4% of 40-50 second indexing time)

**Impact**: Negligible overhead with massive benefits for user experience and API simplicity.

### Key Design Decisions

1. ✅ **Vec instead of HashMap** for extension counting (4-8x faster due to cache locality)
2. ✅ **Per-directory sorting** instead of global sorting (43 ms vs 1+ second)
3. ✅ **Eager tree metrics** instead of lazy computation (simpler API, always available)
4. ✅ **Inline parsing** during MFT read (no separate pass needed)
5. ✅ **Merge-friendly** for parallel parsing with fragments

### Next Steps

**Recommended implementation order**:

**Independent phases** (can be done in parallel):
- Phase 1: Filename Component Parsing
- Phase 2: Attribute Counters
- Phase 3: Size Distribution Buckets
- Phase 6: Per-Directory Sorting
- Phase 7: Eager Tree Metrics

**Dependent phases** (require earlier phases):
- Phase 4: Extension Histogram (requires Phase 1)
- Phase 5: CLI Display Integration (requires Phases 1-4)
- Phase 8: Performance Validation (requires all phases)

**Suggested start**: Begin with Phase 1 (Filename Component Parsing) or Phase 7 (Eager Tree Metrics) as they provide the most value and are completely independent.
        0..=1_024 => 0,
        1_025..=10_240 => 1,
        10_241..=102_400 => 2,
        102_401..=1_048_576 => 3,
        1_048_577..=10_485_760 => 4,
        10_485_761..=104_857_600 => 5,
        104_857_601..=1_073_741_824 => 6,
        _ => 7,
    }
}
```

**Cost**:
- Memory: 8 buckets × 4 bytes = 32 bytes
- CPU: ~2-3 ns per file → ~2-3 ms per 1M files

#### C. Extension Histogram

**Goal**: Count files per extension for "top file types" display and fast extension filtering.

**New structure** (separate from `MftStats` due to size):
```rust
/// Extension statistics collected during MFT parsing.
/// Uses Vec instead of HashMap for cache-friendly counting.
pub struct ExtensionStats {
    /// Extension counts (unsorted during collection, sorted at end)
    /// Format: [(".txt", 123456), (".jpg", 98765), ...]
    extensions: Vec<(String, u32)>,
}

impl ExtensionStats {
    /// Record a file extension (called during parsing)
    pub fn record(&mut self, ext: &str) {
        // Linear search in small vec (cache-friendly)
        if let Some(entry) = self.extensions.iter_mut().find(|(e, _)| e == ext) {
            entry.1 += 1;
        } else {
            self.extensions.push((ext.to_string(), 1));
        }
    }

    /// Finalize stats (sort by count descending)
    pub fn finalize(&mut self) {
        self.extensions.sort_by(|a, b| b.1.cmp(&a.1));
    }

    /// Get top N extensions
    pub fn top_n(&self, n: usize) -> &[(String, u32)] {
        &self.extensions[..n.min(self.extensions.len())]
    }
}
```

**Why Vec instead of HashMap?**
- **Cache locality**: Vec is sequential, HashMap is scattered
- **Small n**: Only 100-1000 unique extensions, linear search is fast
- **Zipf distribution**: Common extensions (.txt, .jpg) found in first few comparisons
- **4-8x faster**: ~5-10 ms vs ~40-50 ms for HashMap
- **Less memory**: ~15 KB vs ~20 KB

**Cost**:
- Memory: ~100-1000 entries × ~20 bytes = ~2-20 KB
- CPU: ~5-10 ns per file (linear search) → ~5-10 ms per 1M files
- Final sort: ~1 ms (500 × log2(500) ≈ 4500 comparisons)

---

## 3.3 Per-Directory Sorting (Option 3)

**Goal**: Sort each directory's children by filename for natural directory listings (like Windows Explorer).

**Rationale**:
- Users expect directory contents to be alphabetically sorted
- Matches Windows Explorer behavior
- Minimal cost compared to global sorting
- Doesn't break FRS-based parent-child relationships

**Implementation approach**:

After building the `MftIndex` (either from inline parsing or fragment merging), sort the children within each directory:

```rust
impl MftIndex {
    /// Sort children within each directory by filename.
    ///
    /// This is called once after the index is fully built, before returning
    /// it to the caller. It provides natural sorted directory listings without
    /// breaking FRS-based indexing.
    pub fn sort_directory_children(&mut self) {
        // For each directory, sort its children by name
        // The children index maps parent_frs -> Vec<child_frs>

        for dir_record in &self.records {
            if !dir_record.is_directory() {
                continue;
            }

            // Get mutable access to this directory's children list
            let children_frs = self.get_children_frs_mut(dir_record.frs);

            // Sort by filename (case-insensitive for natural ordering)
            children_frs.sort_by(|&frs_a, &frs_b| {
                let name_a = self.get_name_for_frs(frs_a);
                let name_b = self.get_name_for_frs(frs_b);
                name_a.to_lowercase().cmp(&name_b.to_lowercase())
            });
        }
    }
}
```

**Cost analysis**:

For 1M files distributed across ~50K directories:
- Average children per directory: 20 files
- Sort cost per directory: 20 × log2(20) × 10 ns ≈ 860 ns
- Total CPU time: 50K directories × 860 ns ≈ **43 ms**
- Memory overhead: **0 bytes** (just reorder existing children vectors)

**Benefits**:
- ✅ **Natural UX**: Directory listings match Explorer behavior
- ✅ **Minimal cost**: ~43 ms for 1M files (0.1% of indexing time)
- ✅ **Preserves FRS indexing**: Records stay in FRS order
- ✅ **Cache-friendly**: Children are already grouped by parent
- ✅ **No breaking changes**: Doesn't affect existing APIs

**When to call**:
- After `MftIndex::from_parsed_records()` completes
- After `MftIndex::merge_fragments()` completes
- Before returning the index to the caller

**Alternative considered: Global sorting**

Global sorting by full path was considered but rejected:
- ❌ Cost: ~1-1.1 seconds (vs 43 ms for per-directory)
- ❌ Breaks FRS indexing (parent-child relationships use FRS numbers)
- ❌ Requires building full paths (expensive)
- ❌ Not what C++ does (breaks parity)

**Alternative considered: Dual index**

Maintaining both FRS order and a sorted index was considered:
- ⚠️ Cost: ~1.1 seconds + 4 MB per 1M files
- ✅ Enables binary search by full path
- ✅ Enables sorted iteration without query-time sorting
- ⚠️ Only worthwhile if sorted output is a common use case

**Decision**: Implement per-directory sorting (Option 3) as the default behavior. Consider dual index as a future optimization if profiling shows query-time sorting is a bottleneck.

---

## 3.4 Eager Tree Metrics Computation

**Goal**: Compute and store tree metrics directly in `FileRecord` during index building, eliminating the need for a separate `TreeIndex` step.

**Current approach** (in `uffs-core/src/tree.rs`):
- Tree metrics are computed **on-demand** by a separate `TreeIndex` module
- Requires converting MftIndex to DataFrame
- Builds parent-child map from DataFrame columns
- Computes metrics lazily with memoization
- Performance: ~50-100 ms for 1M files

**Tree metrics**:
1. **Descendants count**: How many files/subdirectories under a directory
2. **Tree size**: Total real size of all files in subtree
3. **Tree allocated**: Total disk size of all files in subtree

**Why current approach is separate**:
- MFT records are in **FRS order** (0, 1, 2, 3, ...), not tree order
- A child might be parsed before its parent
- Can't compute directory totals in a single pass during parsing
- Requires bottom-up traversal after all records are parsed

**Proposed approach**: **Eager computation during index building**

Instead of computing tree metrics on-demand, compute them once after all records are parsed but before returning the MftIndex:

```rust
impl MftIndex {
    pub fn from_parsed_records(volume: char, records: Vec<ParsedRecord>) -> Self {
        // 1. Build index structure (existing code)
        let mut index = Self::build_from_records(volume, records);

        // 2. Compute tree metrics (NEW - integrated into build phase)
        index.compute_tree_metrics();

        // 3. Return index with tree metrics already populated
        index
    }
}
```

**Add to `FileRecord`**:
```rust
pub struct FileRecord {
    // ... existing fields ...

    // NEW: Tree metrics (computed after all records parsed)
    pub descendants: u64,      // Count of all descendants (0 for files)
    pub treesize: u64,         // Sum of real sizes in subtree
    pub tree_allocated: u64,   // Sum of disk sizes in subtree
}
```

**Algorithm** (same as TreeIndex):
- Bottom-up traversal using existing `children` index
- Memoization to avoid recomputation
- O(n) time complexity
- For files: descendants=0, treesize=size, tree_allocated=allocated_size
- For directories: sum of all children's metrics

**Cost**:
- **Memory**: +24 bytes per record (3 × u64)
  - For 1M files: +24 MB (~4% increase from 600 MB to 624 MB)
- **CPU**: ~50-100 ms per 1M files (same as TreeIndex)
  - 0.1-0.2% of total indexing time (40-50 seconds)

**Benefits**:
- ✅ **Simpler API**: No separate TreeIndex step needed
- ✅ **Faster queries**: Tree metrics always available (no on-demand computation)
- ✅ **Better for CLI**: We ALWAYS need tree metrics for C++ parity
- ✅ **Consistent**: Fits "pay once, benefit forever" philosophy
- ✅ **Less code**: Remove TreeIndex dependency from CLI

**Trade-off**:
- Current: Compute tree metrics only when needed (lazy)
- Proposed: Compute tree metrics always (eager)
- For CLI use case: We ALWAYS need tree metrics, so eager makes sense

**Decision**: Implement eager tree metrics computation as part of the index building phase. This simplifies the API and ensures tree metrics are always available for downstream tools.

---

