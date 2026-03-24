# TUI Architecture Design

> **Status**: Design phase — not yet implemented
> **Date**: 2026-03-23
implementation > **Decision**: Option 1 (load all drives into RAM) for Phase 1

---

## Vision

The TUI is an interactive terminal interface for UFFS that provides:

- **Real-time search-as-you-type** against NTFS MFT data (<10ms per keystroke)
- **Multi-drive by default** — all NTFS drives loaded and searchable
- **Sortable columns** (any/all: name, size, modified, created, etc.)
- **Configurable output columns** (same as CLI `--columns`)
- **Attribute filtering** (hidden, system, compressed, etc.)
- **Auto-refresh** every ~60s to pick up file changes
- **Cross-platform**: live drives (Windows) or saved MFT files (Mac/Linux)

---

## Architecture Decision: Why NOT CLI-Wrapper

Spawning `uffs` as a subprocess per search was considered and **rejected**:

- Even with cached `.uffs` files (~2s load), that's **2 seconds per keystroke**
- Every character typed → stop old process → spawn new one → wait for output
- Parsing CSV/JSON output adds overhead on top of the process spawn
- No incremental display — wait for full output before showing anything
- **Verdict**: Unusable for search-as-you-type. CLI-wrapper only viable as
  an optional fallback for features not yet in the library.

**Chosen approach**: Load `MftIndex` directly into memory. One-time upfront
cost (2-8s), then every search is <10ms walking an in-memory array.

---

## Multi-Drive Strategy

### Default Behavior

| Platform | Default | CLI Override |
|----------|---------|-------------|
| Windows  | Auto-detect all NTFS drives, load all | `--drive C` or `--drive C,D` |
| Mac/Linux | Must specify `--mft-file` paths | `--mft-file C.iocp,D.iocp` |

### Memory Footprint Analysis

Current MftIndex RAM usage per drive (measured from `.uffs` cache sizes):

| Drive | Files | MftIndex RAM | Cache File |
|-------|-------|-------------|------------|
| C | 3.4M | ~800 MB | 734 MB |
| D | 7.1M | ~1.6 GB | 1,161 MB |
| E | 2.9M | ~650 MB | 644 MB |
| F | 2.2M | ~500 MB | 481 MB |
| G | 15K | ~4 MB | 3 MB |
| M | 1.9M | ~420 MB | 420 MB |
| S | 8.3M | ~1.9 GB | 1,823 MB |
| **ALL 7** | **25.9M** | **~5.9 GB** | **5,266 MB** |

Each `FileRecord` ≈ 128 bytes + name strings + link/stream chains.
The `.uffs` cache file ≈ 1:1 with in-memory size (serialized `MftIndex`).

### Why 6 GB is Acceptable (Phase 1)

- Everything.exe uses ~400 MB per large drive — same ballpark
- Modern machines: 16-32 GB RAM. 6 GB for an active search tool is fine
- Memory freed instantly when TUI exits
- The alternative (process-per-search) is 0 MB but unusable

---

## Memory Optimization Approaches (Future Phases)

### Approach A: LRU Drive Eviction

Keep only the 2-3 most recently searched drives in memory, evict the rest.

```
User searches on C: → load C index (~800 MB)
User searches on D: → load D index (~1.6 GB), C stays
User searches on S: → load S index (~1.9 GB), evict C (oldest)
```

- **Max RAM**: ~4 GB (largest 2-3 drives) instead of 6 GB
- **Tradeoff**: First search on a cold drive has 2-8s latency
- **Best for**: Machines with 16 GB RAM where 6 GB is tight

### Approach B: Memory-Mapped Index (Zero-Copy)

Memory-map the `.uffs` cache files instead of loading into heap.

```
┌─────────────────────────────────────────┐
│              TUI Process                │
│  Virtual memory: 6 GB (all drives)      │
│  Physical RAM:   ~2 GB (active pages)   │
│                                         │
│  mmap(C_index.uffs) → 734 MB virtual    │
│  mmap(D_index.uffs) → 1.1 GB virtual    │
│  ... OS pages in/out as needed          │
└─────────────────────────────────────────┘
```

**How it works:**
- `mmap()` the `.uffs` files — no explicit load, OS manages paging
- Virtual memory = 6 GB, physical RAM adapts to available memory
- Hot pages (recently searched) stay in RAM, cold pages get paged out
- Startup is **instant** — no load time, just map the files

**Requirements:**
- The `.uffs` serialized format must be **directly queryable** without
  deserialization. Currently it's a packed binary blob that requires
  `deserialize()` into heap-allocated Vecs.
- Would need a new "zero-copy index format" where `FileRecord`, names,
  links, streams are laid out as flat arrays that can be read directly
  from the mmap'd region via `zerocopy::FromBytes`.

**Estimated effort**: Medium-large. Requires redesigning the storage format.
But the payoff is huge — instant startup, automatic memory management.

### Approach C: Lightweight Name Index

Keep only a minimal search index in memory, resolve full metadata on demand.

```
NameIndex (in memory, ~200 MB for ALL drives):
  ├── frs: u64
  ├── name_offset: u32  (into mmap'd names buffer)
  ├── name_len: u16
  ├── extension_id: u16
  ├── parent_frs: u64
  └── flags: u8  (is_directory, is_hidden, etc.)

Full metadata (on demand from .uffs file):
  ├── size, allocated, created, modified, accessed
  ├── full path (resolved from parent chain)
  ├── tree metrics (descendants, treesize)
  └── all attributes
```

**How it works:**
- Load only ~24 bytes per record into RAM (vs ~128+ bytes for full MftIndex)
- Pattern matching works on names from the mmap'd names buffer
- When displaying results (10K rows), resolve full metadata from the
  `.uffs` file for just those records
- Sort by name/extension is instant (in-memory). Sort by size/date
  requires loading metadata for the result set.

**Memory**: ~200 MB for all 7 drives (25.9M records × ~8 bytes essential fields)
**Tradeoff**: Sorting by size/date requires on-demand metadata fetch (~5ms)

---

## Phase 1 Implementation: Load All Into RAM

### Core Data Structure

```rust
struct MultiDriveIndex {
    drives: Vec<DriveIndex>,
}

struct DriveIndex {
    letter: char,
    index: MftIndex,
    source: IndexSource,
}

enum IndexSource {
    LiveDrive,                     // Windows IOCP
    MftFile(PathBuf),              // raw/IOCP/compressed
    CachedIndex(PathBuf),          // .uffs cache file
}
```

### SearchBackend Trait

```rust
trait SearchBackend {
    /// Search across all loaded drives. Returns up to `limit` results.
    fn search(&self, pattern: &str, filters: &SearchFilters, limit: usize)
        -> Vec<DisplayRow>;

    /// Re-sort the last result set by a different column.
    fn sort(&mut self, column: SortColumn, descending: bool);

    /// Total record count across all drives.
    fn record_count(&self) -> usize;

    /// Reload indexes from source (picks up file changes).
    fn refresh(&mut self) -> Result<()>;

    /// List of loaded drives with record counts.
    fn drives(&self) -> Vec<(char, usize)>;
}
```

### DisplayRow

```rust
struct DisplayRow {
    drive: char,
    path: String,          // full resolved path
    name: String,          // filename only
    size: u64,
    allocated: u64,
    created: i64,          // Unix micros
    modified: i64,
    accessed: i64,
    descendants: u32,
    is_directory: bool,
    attributes: u32,       // NTFS attribute flags
    extension: String,     // file extension
}
```

### Search Implementation

```rust
impl SearchBackend for MultiDriveIndex {
    fn search(&self, pattern: &str, filters: &SearchFilters, limit: usize)
        -> Vec<DisplayRow>
    {
        // 1. Compile pattern once
        let compiled = compile_parsed_pattern(parsed)?;

        // 2. Search all drives (parallel via rayon)
        let results: Vec<DisplayRow> = self.drives.par_iter()
            .flat_map(|drive| {
                // Try extension index pre-filter
                let ext_indices = try_get_extension_indices(&drive.index, ...);

                // Walk records, apply pattern + filters, collect
                search_drive(&drive.index, &compiled, filters, ext_indices, limit)
            })
            .collect();

        // 3. Apply global limit + sort
        results.truncate(limit);
        results
    }
}
```

### Data Flow

```
TUI startup
    ↓
Load all drives in parallel (2-8s, show progress bar)
    ↓
User types "hallo"
    ↓
UI debounces (50ms after last keypress)
    ↓
SearchBackend::search("hallo", filters, 10_000)
    ↓
For each drive (parallel via rayon):
  1. compile_parsed_pattern("hallo") → Contains("hallo")
  2. Walk MftIndex.records
  3. For each record: resolve name, apply match + filters
  4. Collect into Vec<DisplayRow>
    ↓
Merge results from all drives
    ↓
Apply current sort order
    ↓
UI renders sortable table (top 10K results)
    ↓
User presses ↓/↑ to navigate, Enter to act, Tab to change sort
```

### Key Design Decisions

**1. Don't use Polars DataFrame**

The current TUI uses `MftQuery` over a Polars `DataFrame`. Problems:
- DataFrame creation from MftIndex: ~500ms for large drives
- Every search creates a new filtered DataFrame: slow, allocates
- Polars pulls in 50+ crates of dependencies
- **Better**: Search directly on `MftIndex` — it IS the data store

**2. Limit display results to 10,000**

Stop scanning after `limit` matches. Nobody scrolls through millions of
rows in a terminal. 10K is generous. This also means partial patterns
like "h" (matching millions) complete in <100ms instead of seconds.

**3. Debounce search input (50ms)**

Don't search on every keystroke — wait 50ms after the last keypress.
Prevents wasting CPU on "h", "ha", "hal" when the user will type "hallo".

**4. Background refresh**

Refresh runs on a separate thread. UI stays responsive with a spinner
in the status bar. Configurable interval (default 60s, 0 = manual only).

**5. Sort is instant (in-memory)**

After search produces `Vec<DisplayRow>`, sorting is just
`results.sort_by()` — no re-search needed. User can click/key on any
column header to re-sort instantly.

---

## UI Layout

```
┌─────────────────────────────────────────────────────────────────┐
│ UFFS Search: hallo█                          [C D E F M S] 25.9M│
├────────┬──────────────────────────────┬──────────┬──────────────┤
│ Name ▼ │ Path                         │ Size     │ Modified     │
├────────┼──────────────────────────────┼──────────┼──────────────┤
│ 📁 Hallo.txt        │ M:\Dropbox\Docs\             │     16 B │ 2008-10-27 │
│ 📄 Halloween.pub    │ M:\Drop\Wholesale\Publisher\  │  238 KB │ 2015-10-28 │
│ 📁 Shallow.Seas\    │ M:\Media\TV Shows\Planet...   │  990 MB │ 2015-06-01 │
│ 📄 Aber Hallo.mp3   │ M:\MyAudio\iTunes\Fips...     │   55 MB │ 2015-10-30 │
│ 📄 Hallo Welt.mp3   │ M:\MyAudio\iTunes\Rolf...     │  5.6 MB │ 2015-10-29 │
│                      │                               │         │            │
├──────────────────────────────────────────────────────────────────┤
│ 72 matches │ 7 drives │ Search: 3ms │ Last refresh: 12s ago     │
│ [F1]Help [F5]Refresh [Tab]Sort [Esc]Quit [Enter]Open           │
└──────────────────────────────────────────────────────────────────┘
```

- **Search bar**: top, always focused, search-as-you-type
- **Drive indicators**: show which drives are loaded, with record counts
- **Results table**: sortable columns, scrollable, truncated paths
- **Status bar**: match count, search latency, refresh timer
- **Key hints**: bottom row with available actions

---

## File Structure

```
crates/uffs-tui/
├── src/
│   ├── main.rs              # CLI args, terminal setup, event loop
│   ├── app.rs               # App state, keybindings, dispatch
│   ├── backend/
│   │   ├── mod.rs           # SearchBackend trait + SearchFilters
│   │   ├── index.rs         # MultiDriveIndex (MftIndex direct search)
│   │   └── display.rs       # DisplayRow, column defs, formatting
│   ├── ui/
│   │   ├── mod.rs           # Main render function
│   │   ├── search_bar.rs    # Search input widget
│   │   ├── results_table.rs # Sortable results table
│   │   ├── status_bar.rs    # Status, timing, key hints
│   │   └── drive_bar.rs     # Drive indicators
│   └── loading.rs           # Progress bar during index loading
└── Cargo.toml
```

---

## Findings & Performance Analysis (2026-03-24)

### Search Performance: Trigram Index

Linear scan of 25M records took **2800ms** — unusable for interactive search.
Trigram inverted index reduced this to **<10ms** — a **280× speedup**.

| Pattern Length | Before (linear) | After (trigram) | Speedup |
|---------------|-----------------|-----------------|---------|
| 1-2 chars | 5-14ms (hits limit fast) | 5-14ms (unchanged) | — |
| 3 chars | 150ms | <10ms | 15× |
| 4 chars | 890ms | <10ms | 89× |
| 5+ chars | 2800ms | <10ms | 280× |

### Loading Performance Breakdown (Windows, 7 drives, 23M records)

First run (cold, no cache):

| Phase | Time | Notes |
|-------|------|-------|
| MFT read (NVMe) | 2-4s per drive | IOCP sliding window |
| MFT read (HDD) | 20-60s per drive | Dominated by disk I/O wait |
| Tree metrics | 0.3-0.6s per drive | Parent chain computation |
| Cache save | 0.5-1s per drive | Serialize to `.uffs` file |
| **Path resolution** | **8-15s per drive** | **Main bottleneck** — walks parent chain for every record |
| Trigram build | 1-3s per drive | Extract trigrams from all paths |
| **Total (cold)** | **~70s** | Parallel across drives, limited by slowest HDD |

Second run (cached, USN):

| Phase | Time | Notes |
|-------|------|-------|
| Cache load | 0.5-2s per drive | Deserialize `.uffs` file |
| USN apply | <50ms | 18 changes applied |
| **Path resolution** | **8-15s per drive** | **Still the bottleneck** — must resolve ALL paths |
| Trigram build | 1-3s per drive | Must rebuild full trigram index |
| **Total (cached)** | **~40s** | HDD I/O eliminated, but path+trigram still slow |

**Key insight**: Path resolution + trigram build dominate the second run.
This is the target for Wave 3 incremental refresh optimization.

### USN Journal Findings

The `MftReader::read_index_cached()` API correctly:
1. Detects fresh cache (within 10 min TTL)
2. Queries USN Journal for changes since last checkpoint
3. Aggregates USN records and applies deltas to `MftIndex`
4. Saves updated cache with new USN checkpoint

**Known limitation**: `apply_usn_changes` returns `created=0` for newly
created files in some cases. The USN aggregation (`aggregate_changes`)
merges CREATE + DATA_EXTEND + CLOSE into a single entry that may be
classified as "modified" or "skipped" if the FRS was previously
deleted/reused. This affects both the CLI and TUI.

**Workaround**: Use `--no-cache` to force a full fresh MFT read.

**Root cause investigation needed**: The `aggregate_changes` function
in `usn.rs` needs to handle the CREATE→MODIFY sequence correctly,
ensuring new files with reused FRS numbers are classified as "created"
rather than "skipped" (FRS not in existing index → skip).

---

## Incremental Refresh Design (Wave 3)

### Current: Full Rebuild (slow)

```
USN applies 18 changes to MftIndex
  → rebuild ALL 3M paths_lower   (~10s)
  → rebuild ALL trigrams          (~2s)
  Total: ~12s for 18 file changes
```

### Target: Incremental Update (<50ms)

```
USN applies 18 changes to MftIndex
  → resolve paths for ONLY 18 changed records  (~1ms)
  → update paths_lower[frs] for those records   (~0ms)
  → append new trigrams to posting lists         (~1ms)
  → mark deleted trigrams as stale               (~0ms)
  Total: <10ms for 18 file changes
```

### Implementation

```rust
fn apply_usn_delta(drive: &mut DriveIndex, changes: &UsnStats) {
    // For each created/modified record:
    //   1. Resolve its path (single record, ~50μs)
    //   2. Remove old trigrams from posting lists (if modified)
    //   3. Extract new trigrams, append to posting lists
    //   4. Update paths_lower[frs]
    
    // For each deleted record:
    //   1. Clear paths_lower[frs] = ""
    //   2. Trigrams become stale — filtered during verification
    //      (lazy cleanup: posting lists cleaned on next full rebuild)
}
```

**Key property**: Trigram posting lists are **append-only** for new files.
Deletes are handled lazily — stale entries are filtered during the
verification step (`paths.get(idx).is_some_and(|p| p.contains(needle))`).
This means deletes have zero cost on the trigram index until a full rebuild.

### Refresh Timer

- Auto-refresh every 60s (configurable, 0 = manual only)
- Background thread queries USN Journal, applies delta
- UI shows spinner in status bar during refresh
- Results update seamlessly — no flicker or reset

---

## Windows Auto-Detect Loading Flow

```
uffs_tui.exe (no args)
    ↓
detect_ntfs_drives() → [C, D, E, F, G, M, S]
    ↓ (--drive C,D filters to subset)
For each drive (parallel threads):
    ↓
MftReader::open(drive_letter)
    ↓
read_index_cached(TTL=600s)
  ├─ Cache FRESH → load from .uffs + apply USN delta
  ├─ Cache STALE → full IOCP read + save to .uffs
  └─ No cache    → full IOCP read + save to .uffs
    ↓
build_drive_index(drive_letter, MftIndex)
  ├─ Resolve ALL paths (parent chain walk) → paths_lower
  └─ Build TrigramIndex from paths_lower
    ↓
DriveIndex ready → send to UI via channel
```

Same flow as `uffs.exe` — DRY, shared `MftReader` API. The only TUI-
specific part is `build_drive_index` (paths_lower + trigram).

---

## Implementation Wave Tracker

### Wave 1: Core Search (MVP) ✅

| Task | Status | Notes |
|------|--------|-------|
| CLI args: `--mft-file`, `--drive`, `--data-dir`, positional files | ✅ | Cross-platform |
| `MultiDriveBackend`: load MFT files with parallel loading | ✅ | `std::thread::scope` + mpsc |
| Trigram inverted index for <10ms search | ✅ | `HashMap<[u8;3], Vec<u32>>` |
| `DisplayRow` struct + path resolution | ✅ | Parent chain walk |
| ratatui table rendering with drive/name/size/path columns | ✅ | Replaced DataFrame-based UI |
| Search-as-you-type with debounce | ✅ | Drain keystrokes, render, then search |
| Result limit (200 short, 1000 long patterns) | ✅ | Early termination |
| Status bar: match count + search latency + trigram stats | ✅ | |
| Windows LIVE drive auto-detection | ✅ | `detect_ntfs_drives()` + `MftReader` |
| `--no-cache` flag for fresh MFT reads | ✅ | Bypasses cache + USN |
| Per-drive timing breakdown (mft/paths/tri) | ✅ | Shown during loading |
| In-TUI loading progress with input active | ✅ | Type while loading |
| Mouse capture disabled for text selection | ✅ | |

### Wave 2: Sort + Filter (partial ✅)

| Task | Status | Notes |
|------|--------|-------|
| Column sorting (Tab to cycle) | ✅ | Name → Size → Modified → Path |
| Sort direction toggle (Shift+Tab) | ✅ | Ascending/descending |
| `--name-only` toggle (F2) | ✅ | Filename-only matching |
| Multi-tier sort (size, then name) | ⏳ | |
| `--files-only`, `--dirs-only` toggle | ⏳ | |
| `--attr` filter toggle panel | ⏳ | |
| Column visibility toggle (F4) | ⏳ | |

### Wave 3: Refresh + Live

| Task | Status | Notes |
|------|--------|-------|
| Incremental USN refresh (delta trigram update) | ⏳ | <50ms per refresh cycle |
| Auto-refresh timer (60s default) | ⏳ | Background thread |
| Manual refresh (F5) | ⏳ | |
| Cache indicator (cached/fresh) in loading display | ⏳ | |
| Fix USN `created=0` for new files with reused FRS | ⏳ | `aggregate_changes` bug |
| `.uffs-tui` sidecar cache for trigram + paths_lower | ⏳ | Skip path resolve on cached restart |

### Wave 4: UX Polish

| Task | Status | Notes |
|------|--------|-------|
| Enter → copy path to clipboard | ⏳ | |
| Enter → open file/folder in explorer | ⏳ | Windows only |
| Detail panel (expand selected row) | ⏳ | All record fields |
| Extension stats panel | ⏳ | Top extensions by count/size |
| Tree view mode | ⏳ | Navigate directory hierarchy |
| Regex/glob mode indicator | ⏳ | Show pattern type in search bar |

### Wave 5: Memory Optimization (if needed)

| Task | Status | Notes |
|------|--------|-------|
| LRU drive eviction (Approach A) | ⏳ | Cap at 2-3 drives in RAM |
| Zero-copy mmap index (Approach B) | ⏳ | Requires new storage format |
| Lightweight name index (Approach C) | ⏳ | ~200 MB for all drives |
| Benchmark memory vs search latency tradeoffs | ⏳ | |
