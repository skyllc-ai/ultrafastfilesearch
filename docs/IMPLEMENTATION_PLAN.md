# UFFS (Ultra Fast File Search) Implementation Plan

## Executive Summary

This document outlines the comprehensive plan to implement UFFS (Ultra Fast File Search) in Rust using a **modern workspace architecture** with **Polars-based data processing**. The project is structured as multiple independent crates, enabling:

- **Modularity**: Each crate has a single responsibility
- **Reusability**: External tools can consume MFT data as a library
- **Flexibility**: CLI, TUI, and GUI as separate binaries
- **Maintainability**: Clear boundaries and dependencies
- **Performance**: Polars provides SIMD, parallelism, and lazy evaluation
- **Compilation Speed**: Facade crate isolates heavy Polars compilation

## Table of Contents

1. [Workspace Architecture](#workspace-architecture)
2. [Polars Integration Strategy](#polars-integration-strategy)
3. [Crate Specifications](#crate-specifications)
4. [Implementation Phases](#implementation-phases)
5. [Technical Specifications](#technical-specifications)
6. [Dependencies](#dependencies)
7. [Testing Strategy](#testing-strategy)
8. [External Integration](#external-integration)

---

## Workspace Architecture

### Design Philosophy

Following Rust best practices, the project uses a **Cargo workspace** with distinct crates and a **Polars facade crate** for compilation isolation:

```
UltraFastFileSearch/
├── Cargo.toml                    # Workspace manifest
├── crates/
│   ├── uffs-polars/              # 🔧 Polars facade (compilation isolation)
│   ├── uffs-mft/                 # 📦 MFT reading → Polars DataFrame
│   ├── uffs-core/                # 📦 Query engine using Polars lazy API
│   ├── uffs-cli/                 # 🔧 Command-line interface binary (produces `uffs`)
│   ├── uffs-tui/                 # 🖥️  Terminal UI binary
│   └── uffs-gui/                 # 🪟 Graphical UI binary
├── examples/                     # Usage examples
├── benches/                      # Benchmarks
└── docs/                         # Documentation
```

### Dependency Graph

```
                    ┌──────────────┐
                    │ uffs-polars  │  ← Polars facade (compiles ONCE)
                    │   (facade)   │     70+ features consolidated
                    └──────┬───────┘
                           │
                           ▼
                    ┌─────────────┐
                    │  uffs-mft   │  ← MFT → Polars DataFrame
                    │  (library)  │     External tools depend here
                    └──────┬──────┘
                           │
                           ▼
                    ┌─────────────┐
                    │  uffs-core  │  ← Polars lazy queries
                    │  (library)  │     Filter/sort/export
                    └──────┬──────┘
                           │
           ┌───────────────┼───────────────┐
           │               │               │
           ▼               ▼               ▼
    ┌─────────────┐ ┌─────────────┐ ┌─────────────┐
    │  uffs-cli   │ │  uffs-tui   │ │  uffs-gui   │
    │  (binary)   │ │  (binary)   │ │  (binary)   │
    └─────────────┘ └─────────────┘ └─────────────┘
```

### Original C++ Architecture (Reference)

The C++ UFFS achieves its exceptional performance through:

1. **Direct MFT Reading**: Bypasses Windows file enumeration APIs
2. **I/O Completion Ports (IOCP)**: Async I/O with thread pool
3. **Compact Memory Structures**: 6-byte file sizes, bit-packed attributes
4. **Streaming Processing**: Starts matching while MFT is still loading
5. **Bitmap Pre-filtering**: Uses $MFT::$BITMAP to skip invalid records

---

## Polars Integration Strategy

### Why Polars?

Polars provides a **superior foundation** for file search operations:

| Feature | C++ Approach | Polars Approach | Benefit |
|---------|--------------|-----------------|---------|
| Memory Layout | Custom packed structs | Columnar storage | Better cache locality |
| Parallelism | Manual thread pool | Built-in SIMD + threading | Zero-cost parallelism |
| Filtering | Custom iterators | Lazy predicates | Query optimization |
| Sorting | Custom comparators | Optimized sort | Faster multi-column sort |
| Persistence | Custom binary format | Parquet | Industry standard, compressed |
| String Matching | Boyer-Moore-Horspool | SIMD string ops | Hardware acceleration |

### MFT as DataFrame Schema

```
┌─────────┬────────────┬──────────┬──────────┬──────────────┬──────────────┬──────────────┬───────┐
│ frs     │ parent_frs │ name     │ size     │ created      │ modified     │ accessed     │ flags │
│ u64     │ u64        │ str      │ u64      │ datetime[μs] │ datetime[μs] │ datetime[μs] │ u16   │
└─────────┴────────────┴──────────┴──────────┴──────────────┴──────────────┴──────────────┴───────┘
```

**Additional columns (optional):**
- `path` - Full resolved path (computed lazily)
- `extension` - File extension (extracted from name)
- `is_directory` - Boolean flag (derived from flags)

### Compilation Isolation Pattern

The `uffs-polars` crate follows the **facade pattern** for heavy dependencies:

```rust
// crates/uffs-polars/src/lib.rs

//! Pre-compiled Polars wrapper for UFFS
//!
//! This crate isolates Polars compilation to prevent recompilation
//! during development. Polars compiles ONCE and is reused.

pub use polars::prelude::*;
pub use polars::{
    chunked_array,
    datatypes,
    error,
    frame,
    lazy,
    series,
};

// Re-export commonly used types
pub use polars::prelude::{
    DataFrame,
    LazyFrame,
    Series,
    Column,
    PolarsResult,
    PolarsError,
};
```

### Build Time Benefits

| Scenario | Without Facade | With Facade |
|----------|----------------|-------------|
| Initial build | ~4 min | ~4 min |
| Code change in uffs-core | ~4 min (Polars recompiles) | ~25 sec |
| Code change in uffs-cli | ~4 min (Polars recompiles) | ~15 sec |
| Code change in uffs-mft | ~4 min (Polars recompiles) | ~30 sec |

---

## Crate Specifications

### 0. `uffs-polars` - Polars Facade Crate

**Location**: `crates/uffs-polars/`
**Type**: Library crate (facade)
**Purpose**: Compilation isolation for Polars - compiles ONCE, reused everywhere

#### Cargo.toml

```toml
[package]
name = "uffs-polars"
version.workspace = true
edition.workspace = true

[dependencies]
polars = { version = "0.46", features = [
    # Core features
    "lazy",
    "streaming",
    "parquet",
    "dtype-full",

    # String operations
    "strings",
    "regex",

    # Temporal operations
    "temporal",
    "timezones",

    # Performance
    "simd",
    "performant",

    # I/O
    "json",
    "csv",

    # Additional
    "is_in",
    "rows",
    "concat_str",
    "arg_where",
] }
```

#### Public API

```rust
// crates/uffs-polars/src/lib.rs

//! Pre-compiled Polars wrapper for UFFS
//!
//! This crate isolates Polars compilation. All other crates
//! depend on this instead of Polars directly.

// Re-export everything from polars::prelude
pub use polars::prelude::*;

// Re-export specific modules for advanced usage
pub use polars::{
    chunked_array,
    datatypes,
    error,
    frame,
    lazy,
    series,
};

// Convenience type aliases
pub type MftDataFrame = DataFrame;
pub type MftLazyFrame = LazyFrame;
```

---

### 1. `uffs-mft` - MFT Reading & Parsing Library

**Location**: `crates/uffs-mft/`
**Type**: Library crate
**Purpose**: Read NTFS MFT directly, parse structures, output Polars DataFrame

#### Public API

```rust
use uffs_polars::prelude::*;

// ===== Main Entry Point =====
pub struct MftReader { ... }

impl MftReader {
    /// Open a volume for MFT reading (requires admin privileges)
    pub fn open(volume: char) -> Result<Self>;

    /// Read entire MFT and return as DataFrame
    pub fn read_all(&self) -> Result<DataFrame>;

    /// Read with progress callback
    pub fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(MftProgress);

    /// Save DataFrame to Parquet file
    pub fn save_parquet(df: &DataFrame, path: &Path) -> Result<()>;

    /// Load DataFrame from Parquet file
    pub fn load_parquet(path: &Path) -> Result<DataFrame>;
}

/// Progress information during MFT reading
pub struct MftProgress {
    pub records_read: u64,
    pub total_records: Option<u64>,
    pub bytes_read: u64,
    pub elapsed: Duration,
}

// ===== High-Performance I/O =====
pub struct ParallelMftReader { ... }  // Rayon-based parallel reader
pub struct BatchMftReader { ... }     // 1MB batch I/O
pub struct MftExtentMap { ... }       // VCN-to-LCN mapping for fragmented MFT
pub struct MftRecordReader { ... }    // Single record reader with extent support

// ===== Platform Types =====
pub struct VolumeHandle { ... }       // Windows volume handle
pub struct MftBitmap { ... }          // $MFT::$BITMAP for record validity
pub struct MftExtent { ... }          // Single extent (VCN, cluster_count, LCN)
pub struct NtfsVolumeData { ... }     // FSCTL_GET_NTFS_VOLUME_DATA result

// ===== NTFS Structures =====
pub struct NtfsBootSector { ... }           // Boot sector parsing
pub struct FileRecordSegmentHeader { ... }  // FILE record header
pub struct AttributeRecordHeader { ... }    // Attribute header
pub struct StandardInformation { ... }      // $STANDARD_INFORMATION (0x10)
pub struct FileNameAttribute { ... }        // $FILE_NAME (0x30)
pub struct DataRun { ... }                  // Non-resident data run
pub struct AttributeListEntry { ... }       // $ATTRIBUTE_LIST entry
pub struct IndexHeader { ... }              // Directory index header
pub struct IndexRoot { ... }                // $INDEX_ROOT (0x90)
pub struct ReparsePointHeader { ... }       // $REPARSE_POINT (0xC0)
pub struct ReparseMountPointBuffer { ... }  // Junction/symlink target

// ===== Iteration =====
pub struct AttributeIterator<'a> { ... }    // Iterate attributes in a record
pub struct AttributeRef<'a> { ... }         // Reference to an attribute

// ===== Functions =====
pub fn apply_usa_fixup(data: &mut [u8]) -> bool;           // USA fixup
pub fn fixup_file_record(data: &mut [u8]) -> bool;         // Convenience wrapper
pub fn parse_data_runs(data: &[u8]) -> Vec<DataRun>;       // Parse run list
pub fn generate_read_chunks(...) -> Vec<ReadChunk>;        // Optimized read chunks

// ===== DataFrame Schema =====
// The returned DataFrame has these columns:
//
// | Column     | Type         | Description                    |
// |------------|--------------|--------------------------------|
// | frs        | UInt64       | File Record Segment number     |
// | parent_frs | UInt64       | Parent directory FRS           |
// | name       | String       | File/directory name            |
// | size       | UInt64       | File size in bytes             |
// | created    | Datetime[μs] | Creation timestamp             |
// | modified   | Datetime[μs] | Modification timestamp         |
// | accessed   | Datetime[μs] | Access timestamp               |
// | flags      | UInt16       | Bit-packed attributes          |

// ===== Persistence =====
impl MftReader {
    /// Save DataFrame to Parquet file
    pub fn save_parquet(df: &DataFrame, path: &Path) -> Result<()>;

    /// Load DataFrame from Parquet file
    pub fn load_parquet(path: &Path) -> Result<DataFrame>;
}

// ===== File Flags (for filtering) =====
pub mod flags {
    pub const READONLY: u16    = 0x0001;
    pub const HIDDEN: u16      = 0x0002;
    pub const SYSTEM: u16      = 0x0004;
    pub const DIRECTORY: u16   = 0x0010;
    pub const ARCHIVE: u16     = 0x0020;
    pub const COMPRESSED: u16  = 0x0800;
    pub const ENCRYPTED: u16   = 0x4000;
    pub const SPARSE: u16      = 0x0200;
    pub const REPARSE: u16     = 0x0400;
}
```

#### Internal Modules

```
crates/uffs-mft/src/
├── lib.rs              # Public API exports
├── reader.rs           # MftReader implementation (uses ParallelMftReader)
├── ntfs.rs             # NTFS structure definitions (boot sector, attributes, data runs)
├── io.rs               # Low-level I/O (aligned buffers, extent map, parallel reader)
├── platform.rs         # Windows API wrappers (volume handle, bitmap, extents)
├── flags.rs            # File attribute flags
└── error.rs            # Error types with thiserror
```

#### High-Performance MFT Reading Architecture

The MFT reading implementation matches the historical baseline for performance:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         ParallelMftReader                                    │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────────┐  │
│  │  MftExtentMap   │  │   MftBitmap     │  │    ReadChunk Generator      │  │
│  │  VCN → LCN      │  │  Skip unused    │  │    1MB batch chunks         │  │
│  │  Fragmented MFT │  │  clusters       │  │    skip_begin/skip_end      │  │
│  └────────┬────────┘  └────────┬────────┘  └──────────────┬──────────────┘  │
│           │                    │                          │                  │
│           └────────────────────┴──────────────────────────┘                  │
│                                │                                             │
│                    ┌───────────▼───────────┐                                 │
│                    │   Batch I/O (1MB)     │                                 │
│                    │   AlignedBuffer       │                                 │
│                    │   Sector-aligned      │                                 │
│                    └───────────┬───────────┘                                 │
│                                │                                             │
│                    ┌───────────▼───────────┐                                 │
│                    │   Rayon par_iter()    │                                 │
│                    │   Parallel parsing    │                                 │
│                    │   apply_fixup()       │                                 │
│                    │   parse_record()      │                                 │
│                    └───────────┬───────────┘                                 │
│                                │                                             │
│                    ┌───────────▼───────────┐                                 │
│                    │   Vec<ParsedRecord>   │                                 │
│                    │   → DataFrame         │                                 │
│                    └───────────────────────┘                                 │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Key Components:**

| Component | File | Description |
|-----------|------|-------------|
| `MftExtentMap` | `io.rs` | Maps VCN to LCN for fragmented MFT support |
| `MftBitmap` | `platform.rs` | Tracks which MFT records are in use |
| `calculate_skip_range()` | `platform.rs` | Calculates cluster-level skip ranges |
| `in_use_cluster_ranges()` | `platform.rs` | Iterator over clusters with in-use records |
| `generate_read_chunks()` | `io.rs` | Creates optimized 1MB read chunks |
| `ParallelMftReader` | `io.rs` | Orchestrates parallel reading and parsing |
| `BatchMftReader` | `io.rs` | Reads multiple records per I/O operation |
| `AlignedBuffer` | `io.rs` | Sector-aligned buffer for direct I/O |

**Performance Features:**

1. **Fragmented MFT Support**: The MFT can be scattered across multiple non-contiguous extents. `MftExtentMap` handles VCN-to-LCN translation transparently.

2. **Cluster-Level Bitmap Skipping**: Uses `$MFT::$BITMAP` to skip entire clusters where all records are unused, reducing I/O.

3. **Batch I/O**: Reads 1MB chunks instead of individual records, reducing syscall overhead.

4. **Parallel Processing**: Uses Rayon to parse records in parallel across all CPU cores.

5. **USA Fixup**: Applies Update Sequence Array fixup to detect torn writes.

---

### 2. `uffs-core` - Processing & Filtering Library

**Location**: `crates/uffs-core/`
**Type**: Library crate
**Purpose**: Query, filter, sort, search using Polars lazy API

#### Public API (Polars-Based)

```rust
use uffs_polars::prelude::*;
use uffs_mft::flags;

// ===== Query Builder (Wraps Polars LazyFrame) =====
pub struct MftQuery {
    lazy: LazyFrame,
}

impl MftQuery {
    /// Create a new query from MFT DataFrame
    pub fn new(df: DataFrame) -> Self {
        Self { lazy: df.lazy() }
    }

    /// Load from Parquet file
    pub fn from_parquet(path: &Path) -> Result<Self>;

    // ----- Pattern Matching -----

    /// Match files by name pattern (glob syntax)
    pub fn glob(self, pattern: &str) -> Self {
        // Converts glob to regex internally
        let regex = glob_to_regex(pattern);
        Self {
            lazy: self.lazy.filter(col("name").str().contains(lit(regex), false))
        }
    }

    /// Match files by regex pattern
    pub fn regex(self, pattern: &str) -> Self {
        Self {
            lazy: self.lazy.filter(col("name").str().contains(lit(pattern), false))
        }
    }

    /// Exact substring match (fastest)
    pub fn contains(self, substring: &str) -> Self {
        Self {
            lazy: self.lazy.filter(col("name").str().contains_literal(lit(substring)))
        }
    }

    // ----- Type Filters -----

    /// Only files (not directories)
    pub fn files_only(self) -> Self {
        Self {
            lazy: self.lazy.filter(
                col("flags").bitand(lit(flags::DIRECTORY)).eq(lit(0u16))
            )
        }
    }

    /// Only directories
    pub fn directories_only(self) -> Self {
        Self {
            lazy: self.lazy.filter(
                col("flags").bitand(lit(flags::DIRECTORY)).neq(lit(0u16))
            )
        }
    }

    // ----- Size Filters -----

    pub fn min_size(self, bytes: u64) -> Self {
        Self { lazy: self.lazy.filter(col("size").gt_eq(lit(bytes))) }
    }

    pub fn max_size(self, bytes: u64) -> Self {
        Self { lazy: self.lazy.filter(col("size").lt_eq(lit(bytes))) }
    }

    // ----- Date Filters -----

    pub fn modified_after(self, date: NaiveDateTime) -> Self {
        Self { lazy: self.lazy.filter(col("modified").gt(lit(date))) }
    }

    pub fn modified_before(self, date: NaiveDateTime) -> Self {
        Self { lazy: self.lazy.filter(col("modified").lt(lit(date))) }
    }

    // ----- Sorting -----

    pub fn sort_by_size(self, descending: bool) -> Self {
        Self {
            lazy: self.lazy.sort(
                ["size"],
                SortMultipleOptions::default().with_order_descending(descending)
            )
        }
    }

    pub fn sort_by_name(self) -> Self {
        Self { lazy: self.lazy.sort(["name"], SortMultipleOptions::default()) }
    }

    pub fn sort_by_modified(self, descending: bool) -> Self {
        Self {
            lazy: self.lazy.sort(
                ["modified"],
                SortMultipleOptions::default().with_order_descending(descending)
            )
        }
    }

    // ----- Limiting -----

    pub fn limit(self, n: u32) -> Self {
        Self { lazy: self.lazy.limit(n) }
    }

    // ----- Execution -----

    /// Execute query and return DataFrame
    pub fn collect(self) -> PolarsResult<DataFrame> {
        self.lazy.collect()
    }

    /// Execute with streaming (memory efficient for large results)
    pub fn collect_streaming(self) -> PolarsResult<DataFrame> {
        self.lazy.with_streaming(true).collect()
    }

    /// Get underlying LazyFrame for advanced operations
    pub fn into_lazy(self) -> LazyFrame {
        self.lazy
    }
}

// ===== Path Resolution =====
pub struct PathResolver {
    frs_to_parent: HashMap<u64, u64>,
    frs_to_name: HashMap<u64, String>,
}

impl PathResolver {
    /// Build resolver from MFT DataFrame
    pub fn from_dataframe(df: &DataFrame) -> Result<Self>;

    /// Resolve FRS to full path
    pub fn resolve(&self, frs: u64) -> PathBuf;

    /// Add resolved paths as column to DataFrame
    pub fn add_path_column(&self, df: DataFrame) -> Result<DataFrame>;
}

// ===== Export Functions =====
pub fn export_table(df: &DataFrame, writer: impl Write) -> Result<()>;
pub fn export_json(df: &DataFrame, writer: impl Write) -> Result<()>;
pub fn export_csv(df: &DataFrame, writer: impl Write) -> Result<()>;
```

#### Internal Modules

```
crates/uffs-core/src/
├── lib.rs              # Public API exports
├── query.rs            # MftQuery builder (wraps LazyFrame)
├── path_resolver.rs    # Full path reconstruction
├── glob.rs             # Glob to regex conversion
└── export.rs           # Export functions (table, json, csv)
```

#### Example Usage

```rust
use uffs_mft::MftReader;
use uffs_core::MftQuery;

#[tokio::main]
async fn main() -> Result<()> {
    // Read MFT from C: drive
    let df = MftReader::open('C').await?.read_all().await?;

    // Query using fluent API
    let results = MftQuery::new(df)
        .glob("*.rs")
        .files_only()
        .min_size(1024)
        .sort_by_size(true)
        .limit(100)
        .collect()?;

    // Export results
    uffs_core::export_table(&results, std::io::stdout())?;

    Ok(())
}
```

---

### 3. `uffs-cli` - Command Line Interface

**Location**: `crates/uffs-cli/`
**Type**: Binary crate
**Purpose**: User-facing CLI tool

#### Commands

```bash
# Search for files
uffs search "*.rs" --drive C --sort size --limit 100

# Build/refresh index
uffs index --drive C --output index.uffs

# Load existing index and search
uffs search "*.log" --index index.uffs

# Export results
uffs search "*.dll" --drive C --format json > results.json

# Show statistics
uffs stats --drive C
```

#### Structure

```
crates/uffs-cli/src/
├── main.rs             # Entry point
├── commands/           # Command implementations
│   ├── mod.rs
│   ├── search.rs
│   ├── index.rs
│   └── stats.rs
└── output.rs           # Output formatting
```

---

### 4. `uffs-tui` - Terminal User Interface

**Location**: `crates/uffs-tui/`
**Type**: Binary crate
**Purpose**: Interactive terminal UI with ratatui

#### Features

- Real-time search as you type
- File browser with tree view
- Progress indicators during indexing
- Keyboard navigation

---

### 5. `uffs-gui` - Graphical User Interface

**Location**: `crates/uffs-gui/`
**Type**: Binary crate
**Purpose**: Desktop GUI application (future)

#### Technology Options

- **egui**: Pure Rust, cross-platform, immediate mode
- **Tauri**: Web-based UI with Rust backend
- **iced**: Elm-inspired, native rendering

---

## Implementation Phases

### Phase 0: Workspace Setup (Week 0-1)

**Goal**: Establish modern Rust workspace with Polars facade

**Deliverables**:
- [ ] Create workspace `Cargo.toml` with `[workspace]` section
- [ ] Create `crates/` directory structure
- [ ] **Set up `uffs-polars` facade crate** (Polars compilation isolation)
- [ ] Set up `uffs-mft` crate skeleton
- [ ] Set up `uffs-core` crate skeleton
- [ ] Set up `uffs-cli` crate skeleton
- [ ] Configure workspace-level dependencies
- [ ] Set up `rustfmt.toml` and `clippy.toml`
- [ ] Configure GitHub Actions CI/CD
- [ ] Establish MSRV (Minimum Supported Rust Version)

---

### Phase 1: uffs-mft Foundation (Weeks 1-4)

**Goal**: Core MFT reading infrastructure in `uffs-mft` crate

**Week 1-2: NTFS Structures**
- [ ] `NtfsBootSector` - Boot sector parsing
- [ ] `FileRecordHeader` - MFT record header (magic 'FILE')
- [ ] `AttributeHeader` - Common attribute header
- [ ] Resident attribute parsing
- [ ] Non-resident attribute parsing

**Week 3-4: Raw Disk Access**
- [ ] Windows volume opening (`\\.\X:`)
- [ ] `FSCTL_GET_NTFS_VOLUME_DATA` wrapper
- [ ] `FSCTL_GET_RETRIEVAL_POINTERS` wrapper
- [ ] Raw cluster reading
- [ ] Error handling with `thiserror`

---

### Phase 2: uffs-mft DataFrame (Weeks 5-8)

**Goal**: Complete MFT parsing with Polars DataFrame output

**Week 5-6: Core Attributes**
- [ ] `$STANDARD_INFORMATION` (0x10) - Timestamps, flags
- [ ] `$FILE_NAME` (0x30) - Name, parent reference
- [ ] `$DATA` (0x80) - Resident and non-resident
- [ ] Multi-sector fixup (unfixup algorithm)

**Week 7-8: DataFrame Construction**
- [ ] `$BITMAP` (0xB0) - Valid record bitmap
- [ ] `$REPARSE_POINT` (0xC0) - Symlinks, junctions
- [ ] Run list (mapping pairs) parsing
- [ ] **Build Polars DataFrame from parsed records**
- [ ] **Parquet persistence (save/load)**

---

### Phase 3: uffs-core Processing (Weeks 9-12)

**Goal**: Query engine using Polars lazy API

**Week 9-10: Query Engine**
- [ ] `MftQuery` builder wrapping `LazyFrame`
- [ ] Polars-based filter predicates (size, date, type)
- [ ] Path resolution (FRS → full path)
- [ ] `PathResolver` struct

**Week 11-12: Pattern Matching & Export**
- [ ] Glob to regex conversion
- [ ] Polars string matching (SIMD-accelerated)
- [ ] Table export format
- [ ] JSON export format
- [ ] CSV export format

---

### Phase 4: uffs-cli & Performance (Weeks 13-16)

**Goal**: CLI tool and performance optimization

**Week 13-14: CLI Implementation**
- [x] Argument parsing with clap derive
- [x] `search` command
- [x] `index` command
- [x] `stats` command
- [x] Progress reporting (indicatif)
- [x] Error messages (anyhow + context)

**Week 15-16: High-Performance MFT Reading**
- [x] **Fragmented MFT support** - `MftExtentMap` for VCN-to-LCN mapping
- [x] **Batch I/O** - `BatchMftReader` with 1MB chunks
- [x] **Parallel record processing** - `ParallelMftReader` with Rayon
- [x] **Cluster-level bitmap skipping** - `calculate_skip_range()`, `in_use_cluster_ranges()`
- [x] Polars streaming mode for large datasets
- [ ] Benchmark suite (criterion)
- [ ] Profile and optimize hot paths

---

### Phase 5: uffs-tui & Polish (Weeks 17-20)

**Goal**: Terminal UI and production readiness

**Week 17-18: TUI Implementation**
- [ ] ratatui-based interface
- [ ] Real-time search
- [ ] File browser view
- [ ] Progress indicators
- [ ] Keyboard navigation

**Week 19-20: Polish**
- [ ] Comprehensive unit tests
- [ ] Integration tests
- [ ] Documentation (rustdoc)
- [ ] README and usage guide
- [ ] Release builds and packaging

---

## Technical Specifications

### Polars DataFrame vs Custom Structs

| Aspect | Custom Structs (C++ style) | Polars DataFrame |
|--------|---------------------------|------------------|
| Memory layout | Manual packed structs | Columnar (cache-friendly) |
| Parallelism | Manual threading | Built-in SIMD + threading |
| String matching | Boyer-Moore-Horspool | SIMD string ops |
| Filtering | Iterator chains | Lazy predicates (optimized) |
| Persistence | Custom binary format | Parquet (compressed) |
| Memory per file | ~32 bytes | ~40-50 bytes (columnar overhead) |
| Query speed | Fast | **Faster** (SIMD, parallelism) |

### Performance Targets

| Metric | C++ Baseline | Rust/Polars Target |
|--------|--------------|---------------------|
| MFT read speed | ~500 MB/s | ≥500 MB/s |
| Index build time | ~2s for 1M files | ≤1.5s (parallel) |
| Search latency | <10ms | <5ms (SIMD) |
| Memory per file | ~32 bytes | ~45 bytes (acceptable for benefits) |
| Parquet file size | N/A | ~60% of raw (compressed) |

### Why Accept Higher Memory Per File?

The ~40% memory increase is offset by:
1. **10x faster queries** - Polars SIMD operations
2. **Zero-copy filtering** - No intermediate allocations
3. **Lazy evaluation** - Query optimization before execution
4. **Streaming mode** - Process larger-than-RAM datasets
5. **Parquet compression** - Smaller on-disk footprint

---

## Dependencies

### Workspace Cargo.toml Structure

```toml
[workspace]
resolver = "2"
members = [
    "crates/uffs-polars",  # Facade crate (compiles first)
    "crates/uffs-mft",
    "crates/uffs-core",
    "crates/uffs-cli",
    "crates/uffs-tui",
    "crates/uffs-gui",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.80"  # MSRV (Polars requires recent Rust)
license = "MIT OR Apache-2.0"
repository = "https://github.com/githubrobbi/UltraFastFileSearch"

[workspace.dependencies]
# Internal crates
uffs-polars = { path = "crates/uffs-polars" }
uffs-mft = { path = "crates/uffs-mft" }
uffs-core = { path = "crates/uffs-core" }

# Polars (via facade crate)
polars = { version = "0.46", default-features = false }

# Async runtime
tokio = { version = "1.43", features = ["full"] }

# Windows APIs (Windows only)
windows = { version = "0.58", features = [
    "Win32_Storage_FileSystem",
    "Win32_System_Ioctl",
    "Win32_Foundation",
] }

# Data structures
bitflags = "2.6"
bitvec = "1.0"

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# CLI
clap = { version = "4.5", features = ["derive"] }
indicatif = "0.17"  # Progress bars

# TUI
ratatui = "0.29"
crossterm = "0.28"

# Error handling
thiserror = "2.0"
anyhow = "1.0"
miette = { version = "7.4", features = ["fancy"] }

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Testing
criterion = "0.5"
```

### Per-Crate Dependencies

| Crate | Key Dependencies |
|-------|------------------|
| `uffs-polars` | polars (with all features) |
| `uffs-mft` | uffs-polars, windows, tokio, bitflags, bitvec, thiserror |
| `uffs-core` | uffs-polars, uffs-mft |
| `uffs-cli` | uffs-core, clap, indicatif, miette |
| `uffs-tui` | uffs-core, ratatui, crossterm |
| `uffs-gui` | uffs-core, egui (future) |

---

## External Integration

### Using uffs-mft as a Library

External tools can depend on `uffs-mft` for raw MFT access as Polars DataFrame:

```toml
# In external tool's Cargo.toml
[dependencies]
uffs-mft = { git = "https://github.com/githubrobbi/UltraFastFileSearch" }
uffs-polars = { git = "https://github.com/githubrobbi/UltraFastFileSearch" }
```

```rust
use uffs_mft::MftReader;
use uffs_polars::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Read MFT from C: drive as DataFrame
    let df = MftReader::open('C').await?.read_all().await?;

    // Save as Parquet for later use
    MftReader::save_parquet(&df, "c_drive.parquet")?;

    // Use Polars directly for custom analysis
    let dirs = df.clone()
        .lazy()
        .filter(col("flags").bitand(lit(0x0010u16)).neq(lit(0u16)))
        .collect()?;

    println!("Found {} directories", dirs.height());

    Ok(())
}
```

### Using uffs-core for Processing

```rust
use uffs_mft::MftReader;
use uffs_core::MftQuery;

#[tokio::main]
async fn main() -> Result<()> {
    // Load from Parquet
    let query = MftQuery::from_parquet("c_drive.parquet")?;

    // Build a query using fluent API
    let results = query
        .glob("*.rs")
        .files_only()
        .min_size(1024)
        .sort_by_size(true)
        .limit(100)
        .collect()?;

    // Export as table
    uffs_core::export_table(&results, std::io::stdout())?;

    Ok(())
}
```

### Example: Tree Tool Integration

```rust
use uffs_mft::MftReader;
use uffs_polars::prelude::*;
use std::collections::HashMap;

fn build_tree(df: &DataFrame, root_frs: u64) -> TreeNode {
    // Extract columns for tree building
    let frs_col = df.column("frs").unwrap().u64().unwrap();
    let parent_col = df.column("parent_frs").unwrap().u64().unwrap();
    let name_col = df.column("name").unwrap().str().unwrap();

    // Build parent -> children map
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    for i in 0..df.height() {
        let frs = frs_col.get(i).unwrap();
        let parent = parent_col.get(i).unwrap();
        children.entry(parent).or_default().push(frs);
    }

    // Recursively build tree from root
    build_node(root_frs, &children, name_col)
}
```

---

## Testing Strategy

### Unit Tests (per crate)

| Crate | Test Focus |
|-------|------------|
| `uffs-mft` | NTFS parsing, attribute extraction, serialization |
| `uffs-core` | Pattern matching, filtering, sorting, path resolution |
| `uffs-cli` | Argument parsing, output formatting |

### Integration Tests

```
tests/
├── mft_reading.rs      # Full MFT read on test volume
├── search_accuracy.rs  # Verify search results
└── persistence.rs      # Save/load index
```

### Benchmarks

```
benches/
├── mft_read.rs         # MFT reading throughput
├── index_build.rs      # Index construction time
├── search.rs           # Search latency
└── pattern_match.rs    # Pattern matching speed
```

---

## Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|------------|
| Windows API complexity | High | Use `windows` crate, extensive testing |
| Performance regression | High | Continuous benchmarking against C++ |
| Memory safety with raw I/O | Critical | Careful buffer management, fuzzing |
| Cross-platform support | Medium | Abstract platform layer in `uffs-mft` |
| Breaking API changes | Medium | Semantic versioning, deprecation warnings |

---

## Next Steps

1. ✅ Review and approve this implementation plan
2. **Set up workspace structure** (Phase 0)
3. Create crate skeletons with proper dependencies
4. Configure CI/CD pipeline
5. Begin Phase 1: uffs-mft foundation

