# TUI Architecture Design

> **Status**: Implemented — Wave 1 ✅, Wave 2 ✅
> **Date**: 2026-03-24
> **Decision**: Option 1 (load all drives into RAM) for Phase 1

---

## Vision

The TUI is an interactive terminal interface for UFFS that provides:

- **Real-time search-as-you-type** against NTFS MFT data (<10ms per keystroke) ✅
- **Multi-drive by default** — all NTFS drives loaded and searchable ✅
- **Sortable columns** (7 columns: Name, Size, Modified, Path, Drive, Extension, Type) ✅
- **Devicons** — Nerd Font file-type icons with per-type colors ✅
- **Search term highlighting** in filename and path ✅
- **Per-drive color coding** with optimized palettes (1–10 drives) ✅
- **Full text editing** — cursor, selection, clipboard, undo/redo via `ratatui_textarea` ✅
- **Auto-refresh** every ~60s to pick up file changes (Wave 3 — planned)
- **Cross-platform**: live drives (Windows) or saved MFT files (Mac/Linux) ✅

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
| Mac/Linux | Must specify `--mft-file` or `--data-dir` | `--mft-file C.iocp D.iocp` |
| Any | Auto-discover `drive_*` subdirectories | `--data-dir ~/uffs_data` |

#### `--data-dir` Auto-Discovery

```bash
# Load all drives from a data directory
cargo run --release --bin uffs_tui -- --data-dir ~/uffs_data

# Directory structure expected:
~/uffs_data/
├── drive_c/
│   └── C_mft.iocp    # auto-detected (prefers .iocp > .bin > .mft)
├── drive_d/
│   └── D_mft.iocp
└── drive_s/
    └── S_mft.bin
```

The `--data-dir` flag scans for subdirectories named `drive_X` (where X is a
single letter), finds the best MFT file in each (preferring `.iocp` > `.bin`
> `.mft`), and loads them all in parallel.

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

## Phase 1 Implementation: Load All Into RAM ✅

### Core Data Structures (as implemented)

```rust
/// Multi-drive search backend (concrete struct, not a trait).
pub struct MultiDriveBackend {
    pub drives: Vec<DriveIndex>,
    pub last_results: Vec<DisplayRow>,  // kept for re-sorting without re-searching
    pub sort_column: SortColumn,
    pub sort_desc: bool,
}

/// A loaded drive with its MftIndex and trigram search index.
pub struct DriveIndex {
    pub letter: char,
    pub index: MftIndex,
    pub trigram: TrigramIndex,          // trigram inverted index for <10ms search
    pub paths_lower: Vec<String>,       // pre-resolved lowercase full paths
    pub source: IndexSource,
}

/// Trigram inverted index: maps 3-byte sequences to sorted record indices.
pub struct TrigramIndex {
    postings: HashMap<[u8; 3], Vec<u32>>,
}

pub enum IndexSource {
    MftFile(PathBuf),  // raw/IOCP/compressed MFT file
}
```

### DisplayRow (as implemented)

```rust
pub struct DisplayRow {
    pub drive: char,       // drive letter
    pub path: String,      // full resolved path (lowercase)
    pub name: String,      // filename only (original case)
    pub size: u64,         // file size in bytes
    pub is_directory: bool,// for future --files-only/--dirs-only
    pub modified: i64,     // last modified (Unix microseconds)
}
```

### Sort Columns (7 implemented)

```rust
pub enum SortColumn {
    Name,       // filename (case-insensitive)
    Size,       // file size
    Modified,   // last modified time
    Path,       // full path (case-insensitive)
    Drive,      // drive letter
    Extension,  // file extension
    Type,       // devicon file type (groups similar types)
}
```

### Search Implementation (as implemented)

```rust
impl MultiDriveBackend {
    pub fn search(&mut self, pattern: &str, name_only: bool) -> SearchResult {
        // 1. Parse + compile pattern once
        let parsed = ParsedPattern::parse(pattern)?;
        let compiled = compile_parsed_pattern(&parsed)?;

        // 2. Search all drives (parallel via rayon)
        let drive_results: Vec<Vec<DisplayRow>> = self.drives.par_iter()
            .map(|drive| search_drive(drive, &compiled, &needle_lower, ...))
            .collect();

        // 3. Merge, truncate, sort
        rows.truncate(limit);  // 200 for short patterns, 1000 for long
        sort_rows(&mut rows, self.sort_column, self.sort_desc);
        self.last_results = rows.clone();  // cache for re-sorting
    }
}
```

### Data Flow (as implemented)

```
TUI startup → terminal enters alternate screen immediately
    ↓
Spawn background threads (std::thread::scope + mpsc channel)
  ├─ MFT file threads: load_mft_file() per file
  └─ Live drive threads: load_live_drive() per drive (Windows)
    ↓
While loading: UI renders progress, textarea accepts input
  ├─ Each drive sends (DriveIndex, LoadTiming) via channel
  └─ Progress: "✅ C: 3,400,000 rec │ mft: 2.1s paths: 8.3s tri: 1.2s"
    ↓
Loading complete → if user typed during loading, search immediately
    ↓
User types "hallo"
    ↓
Event loop drains ALL buffered keystrokes, re-renders, THEN searches
    ↓
MultiDriveBackend::search("hallo", name_only)
    ↓
For each drive (parallel via rayon):
  1. Trigram index lookup: intersect posting lists for ["hal","all","llo"]
  2. Verify candidates: paths_lower[idx].contains("hallo")
  3. Build DisplayRow from MftIndex record
    ↓
Merge results, truncate (200 short / 1000 long patterns), sort
    ↓
UI renders list with devicons, highlighted matches, drive colors
    ↓
User presses ↓/↑ to navigate, Tab to cycle sort, Shift+Tab to reverse
```

### Key Design Decisions

**1. Don't use Polars DataFrame** ✅ (implemented)

Polars DataFrame was removed. The TUI searches directly on `MftIndex`
records with a trigram inverted index. No DataFrame creation, no Polars
lazy API — just array walks and posting list intersections.

**2. Limit display results** ✅ (implemented)

- Short patterns (1-2 chars): 200 results max
- Long patterns (3+ chars): 1,000 results max
- Early termination — stops scanning after limit reached

**3. Debounce search input** ✅ (implemented)

200ms poll timeout. The event loop drains ALL buffered keystrokes before
searching, so the textarea stays responsive even during slow searches.
The user sees their typed text immediately; search runs after input settles.

**4. Background refresh** (Wave 3 — planned)

Ctrl+R shows a placeholder message. Full USN + incremental trigram
update is planned for Wave 3.

**5. Sort is instant (in-memory)** ✅ (implemented)

After search produces `Vec<DisplayRow>`, re-sorting is just
`sort_unstable_by()` on the cached `last_results` — no re-search needed.
Tab cycles through 7 sort columns, Shift+Tab toggles direction.

---

## UI Layout (as implemented)

```
┌─────────────────────────────────────────────────────────────────────┐
│ Search NTFS Drives [C D E F M S] 25,900,000 Files                  │
│ hallo█                                                              │
├─────────────────────────────────────────────────────────────────────┤
│ Status: 72 matches │ 3ms │ 25,900,000 records across 7 drives      │
├─────────────────────────────────────────────────────────────────────┤
│ Results (72) — Sort: Name ▲                                         │
│ ▶ M:  Hallo.txt  16 B  …dropbox\docs\hallo.txt                    │
│   M:  Halloween.pub  238.00 KB  …drop\wholesale\publisher\...     │
│   M:  Shallow.Seas  990.00 MB  …media\tv shows\planet...          │
│   M:  Aber Hallo.mp3  55.00 MB  …myaudio\itunes\fips...           │
│   M:  Hallo Welt.mp3  5.60 MB  …myaudio\itunes\rolf...            │
│                                                                     │
├─────────────────────────────────────────────────────────────────────┤
│ ↑↓ Nav  PgUp/Dn Page  Enter Path  Tab Sort  F2 Name-only           │
│ Ctrl+R Refresh  Ctrl+Q Quit                                         │
└─────────────────────────────────────────────────────────────────────┘
```

**Layout** (4 vertical sections, `ratatui::Layout`):

| Section | Height | Content |
|---------|--------|---------|
| Search bar | 3 lines | `TextArea` widget + colored drive letters in title |
| Status bar | 3 lines | Match count, search time, record count, trigram count |
| Results table | fills remaining | `Table` with column headers (Drv, Name, Size, Modified, Path) + devicon + highlighting |
| Help bar | 3 lines | Keybinding hints |

**Visual features:**
- **Drive letters** in search bar title are color-coded (optimized palettes for 1–10 drives)
- **Devicons** — Nerd Font file-type icons with per-type colors (via `devicons` crate)
- **Search term highlighting** — matching text in filename and path shown in bold white
- **Path truncation** — long paths truncated from the left with `…` prefix (60 char max)
- **Sort indicator** — current sort column + direction (▲/▼) shown in results title
- **Selection highlight** — `▶` marker + dark gray background on selected row
- **Name-only indicator** — `[NAME]` shown in search bar title when F2 toggled

### Keybindings (as implemented)

| Key | Action |
|-----|--------|
| Any text | Search-as-you-type (via `TextArea` widget) |
| `↑` / `↓` | Navigate results |
| `PageUp` / `PageDown` | Page through results |
| `Enter` | Show selected file's full path in status bar |
| `Tab` | Cycle sort column: Name → Size → Modified → Path → Drive → Extension → Type |
| `Shift+Tab` | Toggle sort direction (ascending/descending) |
| `F2` | Toggle name-only matching mode |
| `F3` | Cycle filter: All → Files Only → Dirs Only |
| `Ctrl+U` | Clear search input |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| `Ctrl+A` | Select all |
| `Ctrl+R` | Refresh (placeholder — Wave 3) |
| `Ctrl+Q` | Quit |

---

## File Structure (as implemented)

```
crates/uffs-tui/
├── src/
│   ├── main.rs       # CLI args (clap), terminal setup, event loop, UI rendering
│   │                 # Includes: ui(), run_app(), highlight_matches(), devicon_color(),
│   │                 # format_size(), truncate_path(), find_best_mft_file(),
│   │                 # init_logging() (dual terminal+file with rotation)
│   ├── app.rs        # App state, TextArea, search dispatch, navigation,
│   │                 # sort cycling, name-only toggle
│   └── backend.rs    # MultiDriveBackend, DriveIndex, TrigramIndex, DisplayRow,
│                     # SortColumn, search_drive(), resolve_path(), load_mft_file(),
│                     # load_live_drive() (cfg(windows)), build_drive_colors(),
│                     # PALETTES (1-10 drive color palettes)
└── Cargo.toml
```

**Note**: The original design proposed a nested `backend/` and `ui/` module
structure. The implementation uses a flat 3-file layout instead — simpler
and sufficient for the current feature set.

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

## Loading Flow (as implemented)

### Windows Auto-Detect (no args)

```
uffs_tui.exe (no args)
    ↓
detect_ntfs_drives() → [C, D, E, F, G, M, S]
    ↓ (--drive C,D filters to subset)
For each drive (parallel threads via std::thread::scope + mpsc):
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
  └─ Build TrigramIndex from paths_lower (parallel via rayon chunks)
    ↓
DriveIndex ready → send to UI via mpsc channel
    ↓
UI receives, shows: "✅ C: 3,400,000 rec │ mft:2.1s paths:8.3s tri:1.2s"
```

### Cross-Platform: `--data-dir` or `--mft-file`

```
uffs_tui --data-dir ~/uffs_data
    ↓
Scan ~/uffs_data/ for drive_* subdirectories
    ↓
find_best_mft_file(drive_c/) → C_mft.iocp  (prefers .iocp > .bin > .mft)
find_best_mft_file(drive_d/) → D_mft.iocp
    ↓
For each file (parallel threads):
    ↓
load_mft_file(path, drive_letter)
  ├─ load_raw_mft() → parse records → MftRecordMerger → MftIndex
  └─ build_drive_index() → paths_lower + TrigramIndex
    ↓
DriveIndex ready → send to UI via mpsc channel
```

Same flow as `uffs.exe` — DRY, shared `MftReader` API. The only TUI-
specific part is `build_drive_index` (paths_lower + trigram).

---

## Implementation Wave Tracker

### Wave 1: Core Search (MVP) ✅

| Task | Status | Notes |
|------|--------|-------|
| CLI args: `--mft-file`, `--drive`, `--data-dir`, positional files | ✅ | Cross-platform |
| `--data-dir` auto-discovery of `drive_*` subdirectories | ✅ | Prefers `.iocp` > `.bin` > `.mft` |
| `MultiDriveBackend`: load MFT files with parallel loading | ✅ | `std::thread::scope` + mpsc |
| Trigram inverted index for <10ms search | ✅ | `HashMap<[u8;3], Vec<u32>>`, parallel build |
| `DisplayRow` struct + path resolution | ✅ | Parent chain walk via `resolve_path()` |
| ratatui list rendering with devicons + drive colors | ✅ | Replaced DataFrame-based UI |
| Search-as-you-type with debounce | ✅ | Drain keystrokes, render, then search |
| Result limit (200 short, 1000 long patterns) | ✅ | Early termination |
| Status bar: match count + search latency + trigram stats | ✅ | Comma-formatted numbers |
| Windows LIVE drive auto-detection | ✅ | `detect_ntfs_drives()` + `MftReader` |
| `--no-cache` flag for fresh MFT reads | ✅ | Bypasses cache + USN |
| Per-drive timing breakdown (mft/paths/tri) | ✅ | Compact format: `2.1s` / `350 ms` |
| In-TUI loading progress with input active | ✅ | Type while loading, search on complete |
| Mouse capture disabled for text selection | ✅ | |
| Devicons (Nerd Font file-type icons + color) | ✅ | `devicons` crate, per-type hex colors |
| Search term highlighting in filename and path | ✅ | Bold white on match, case-insensitive |
| Per-drive color coding (1–10 drive palettes) | ✅ | Hand-tuned maximally distinct colors |
| Path truncation with `…` prefix | ✅ | 60 char max, truncates from left |
| `TextArea` widget (cursor, selection, undo/redo) | ✅ | `ratatui_textarea` crate |
| PageUp/PageDown navigation | ✅ | Dynamic page size from terminal height |
| Ctrl+U/Z/Y/A keybindings | ✅ | Clear, undo, redo, select all |
| Structured dual logging (terminal + rolling file) | ✅ | `tracing_subscriber` + daily rotation |
| `-v` / `--verbose` flag | ✅ | Sets terminal log level to `info` |

### Wave 2: Sort + Filter + Table ✅

| Task | Status | Notes |
|------|--------|-------|
| Column sorting (Tab to cycle, 7 columns) | ✅ | Name → Size → Modified → Path → Drive → Extension → Type |
| Sort direction toggle (Shift+Tab) | ✅ | Ascending ▲ / descending ▼ |
| Sort by devicon Type (groups similar files) | ✅ | Music, images, code, etc. grouped |
| `--name-only` toggle (F2) | ✅ | Filename-only matching |
| Multi-tier sort (primary + name tiebreaker) | ✅ | Secondary sort by name when primary column is equal |
| `--files-only`, `--dirs-only` toggle (F3) | ✅ | Cycles: All → Files Only → Dirs Only |
| Table widget with column headers | ✅ | Replaced `List` → `Table` with Drv, Name, Size, Modified, Path columns |
| Sort indicator on active column header | ✅ | Active column highlighted yellow with ▲/▼ arrow |
| Modified timestamp column | ✅ | `YYYY-MM-DD HH:MM` format via Howard Hinnant civil calendar |
| `--attr` filter toggle panel | ⏳ | Deferred to Wave 3+ |
| Column visibility toggle (F4) | ⏳ | Deferred — Table widget now supports it |

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
| Enter → show path in status bar | ✅ | `📋 C:\path\to\file` |
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
