# Output & Streaming

## Introduction

This document describes how UFFS formats and streams search results to the user. After reading this document, you should be able to:

1. Understand the output configuration system and available columns
2. Implement streaming output for multi-drive results
3. Add new output columns or formats

---

## Output Configuration

**Source:** `uffs-core/src/output/config.rs`

### OutputConfig

```rust
pub struct OutputConfig {
    pub columns: Option<Vec<OutputColumn>>,  // None = all available
    pub separator: String,                    // Default: ","
    pub quote: String,                        // Default: "\""
    pub header: bool,                         // Include header row
    pub pos: String,                          // Boolean true: "1"
    pub neg: String,                          // Boolean false: "0"
    pub timezone_offset_secs: i32,            // Local timezone offset
}
```

### Available Output Columns

**Source:** `uffs-core/src/output/column.rs`

| Column | Type | Description |
|--------|------|-------------|
| `name` | String | Filename |
| `path` | String | Full resolved path |
| `size` | u64 | Logical file size |
| `allocated` | u64 | Allocated disk size |
| `created` | DateTime | Creation timestamp |
| `modified` | DateTime | Last modification timestamp |
| `accessed` | DateTime | Last access timestamp |
| `attributes` | u32 | Windows FILE_ATTRIBUTE flags |
| `is_directory` | bool | Directory flag |
| `descendants` | u32 | Count of all items in subtree |
| `treesize` | u64 | Sum of sizes in subtree |
| `tree_allocated` | u64 | Sum of allocated in subtree |
| `frs` | u64 | File Record Segment number |
| `parent_frs` | u64 | Parent directory FRS |
| `drive` | char | Volume letter |
| `extension` | String | File extension |
| `stream_count` | u16 | Number of data streams |
| `name_count` | u16 | Number of hard links |
| `reparse_tag` | u32 | Reparse point type |

### Default Column Order

A predefined column order is available for structured output:

```rust
pub const DEFAULT_COLUMN_ORDER: &[OutputColumn] = &[
    OutputColumn::Name,
    OutputColumn::Path,
    OutputColumn::Size,
    OutputColumn::Allocated,
    OutputColumn::Created,
    OutputColumn::Modified,
    OutputColumn::Accessed,
    OutputColumn::Attributes,
    // ... etc
];
```

### Special Separator Handling

The `--separator` flag supports symbolic names:

| Input | Output |
|-------|--------|
| `TAB` | `\t` |
| `NEWLINE` | `\n` |
| `SPACE` | ` ` |
| `RETURN` | `\r` |
| `NULL` | `\0` |
| Anything else | Used literally |

---

## Timestamp Formatting

UFFS converts Unix microseconds to local time display using a **fixed timezone offset** captured at startup:

```rust
// Captured once at startup
let timezone_offset_secs = chrono::Local::now().offset().local_minus_utc();

// Applied to every timestamp
fn format_timestamp(unix_micros: i64, offset_secs: i32) -> String {
    // Convert Unix µs → chrono DateTime
    // Apply fixed timezone offset
    // Format as "YYYY-MM-DD HH:MM:SS"
}
```

**Timezone note:** Windows' `FileTimeToLocalFileTime()` uses the CURRENT timezone offset for ALL timestamps, ignoring historical DST transitions. UFFS follows the same Windows convention by capturing the offset once at startup and reusing it, rather than computing per-timestamp DST.

---

## Streaming Output Architecture

### The Problem

When searching multiple drives, results should appear as soon as each drive finishes — not after ALL drives are done. This is especially important when one drive is fast (NVMe, 5s) and another is slow (HDD, 70s).

### Channel-Based Streaming

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ C: NVMe      │  │ D: HDD       │  │ F: NVMe      │
│ MFT Read     │  │ MFT Read     │  │ MFT Read     │
│ (5s)         │  │ (30s)        │  │ (5s)         │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                  │                  │
       ▼                  ▼                  ▼
┌──────────────────────────────────────────────────┐
│          Channel (results sender per drive)        │
│          Sends MftIndex as each drive completes    │
└──────────────────────┬───────────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────────┐
│               Writer Thread                        │
│  Receives MftIndex, applies pattern filter,        │
│  resolves paths, formats output, writes to stdout  │
│  OUTPUT-AS-READY: C results at 5s, F at 5s,       │
│  D at 30s (user sees results immediately)          │
└──────────────────────────────────────────────────┘
```

### Extension Index Pre-Filter

For `*.ext` patterns, each drive's extension index is extracted in the writer thread to enable O(matches) filtering without scanning the full index:

```rust
// In writer thread, per drive:
fn write_streaming_output(index: &MftIndex, pattern: &IndexPattern) {
    // Try extension index fast path
    if let Some(ext_indices) = try_get_extension_indices(index, pattern) {
        // O(matches): iterate only matching records
        for &record_idx in ext_indices {
            format_and_write_record(index, record_idx);
        }
    } else {
        // Full scan: check every record
        for (idx, record) in index.records.iter().enumerate() {
            if pattern.matches(get_name(index, record), case_sensitive) {
                format_and_write_record(index, idx);
            }
        }
    }
}
```

---

## Export Formats

**Source:** `uffs-core/src/export.rs`

### Table Format

Human-readable aligned table for terminal display:

```
Name              Size       Modified
readme.txt        4,096      2026-03-20 14:30:00
src/              -          2026-03-19 10:15:00
Cargo.toml        12,288     2026-03-18 09:00:00
```

### CSV Format

Standard CSV with configurable separator and quoting:

```csv
"name","path","size","modified"
"readme.txt","C:\project\readme.txt",4096,"2026-03-20 14:30:00"
```

### JSON Format

JSON array of objects:

```json
[
  {"name": "readme.txt", "path": "C:\\project\\readme.txt", "size": 4096},
  {"name": "Cargo.toml", "path": "C:\\project\\Cargo.toml", "size": 12288}
]
```

---

## Path Resolution in Output

Path resolution is **lazy** — paths are only computed for records that pass all filters:

```
1. Pattern filter → 50K matches out of 2M records
2. Size filter    → 10K matches
3. Type filter    → 8K matches
4. Path resolution → only 8K paths computed (not 2M)
5. Format + output → 8K rows written
```

### FastPathResolver

For bulk output, `FastPathResolver` caches resolved paths to avoid redundant parent-chain walks:

```rust
pub struct FastPathResolver {
    arena: NameArena,           // Pre-allocated string arena
    cache: Vec<Option<CachedPath>>,  // record_idx → cached path
}

// Multi-drive variant merges paths across drive indices:
pub struct FastPathResolverMultiDrive {
    resolvers: Vec<FastPathResolver>,  // One per drive
}
```

---

## Output Buffering

UFFS uses `BufWriter` for stdout to minimize syscall overhead:

```rust
let stdout = std::io::stdout();
let mut writer = std::io::BufWriter::with_capacity(64 * 1024, stdout.lock());

for record in matching_records {
    writeln!(writer, "{}", format_record(record))?;
}

writer.flush()?;
```

The 64KB buffer size reduces `write()` syscalls from millions (one per line) to thousands (one per buffer fill).

---

## Attribute Filters

The CLI supports inline attribute filtering during output:

```rust
pub struct StreamingRecordFilter {
    pub files_only: bool,
    pub dirs_only: bool,
    pub hide_system: bool,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub attr_filters: Vec<AttrRequirement>,  // --attr hidden,!system
    pub newer_than: Option<i64>,             // --newer 7d
    pub older_than: Option<i64>,             // --older 2026-01-01
}
```

All filters are applied **inline** during the scan, before path resolution. This ensures only matching records incur the path resolution cost.

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62*
