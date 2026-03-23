# MFT Reading Pipeline

## Introduction

This document provides an exhaustive technical analysis of how UFFS reads the NTFS Master File Table (MFT) directly from disk using Windows I/O Completion Ports (IOCP). After reading this document, you should be able to:

1. Understand the complete I/O pipeline from volume open to parsed records
2. Implement bitmap-guided skip optimization
3. Build extent-aware read chunk planning
4. Use IOCP for overlapped async I/O with a sliding window

---

## Why Direct MFT Reading?

### The Problem with Standard APIs

Traditional file enumeration using `FindFirstFile`/`FindNextFile` is slow:

| Operation | Syscalls per file | Notes |
|-----------|-------------------|-------|
| Enumerate | 1 | `FindNextFile` |
| Get size | 1 | `GetFileSize` (or from `WIN32_FIND_DATA`) |
| Get timestamps | 0-1 | Often in find data |
| **Total** | **~2** | **Per file** |

For 2 million files: **~4 million syscalls** with security checks, handle management, and directory traversal.

### The MFT Advantage

The MFT is a single file (`$MFT`) containing ALL file metadata:

| Approach | I/O Operations | Time (NVMe, 2M files) |
|----------|---------------|----------------------|
| Standard APIs | ~4,000,000 | 60-120s |
| Direct MFT read | ~2,000 (1MB chunks) | **5-8s** |
| **Speedup** | | **~15×** |

---

## Pipeline Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                      MFT Reading Pipeline                               │
└─────────────────────────────────────────────────────────────────────────┘

Phase 1: VOLUME ACCESS
  VolumeHandle::open('C') → CreateFile("\\\\.\\C:", GENERIC_READ, OVERLAPPED)
       │
Phase 2: METADATA COLLECTION
  ├─► FSCTL_GET_NTFS_VOLUME_DATA → cluster_size, record_size, mft_capacity
  ├─► FSCTL_GET_RETRIEVAL_POINTERS on $MFT::$DATA → MftExtentMap
  └─► FSCTL_GET_RETRIEVAL_POINTERS on $MFT::$BITMAP → bitmap extents
       │
Phase 3: BITMAP READ
  Read $MFT::$BITMAP → MftBitmap (1 bit per record, which are in-use)
       │
Phase 4: CHUNK PLANNING
  generate_read_chunks(extent_map, bitmap, chunk_size) → Vec<ReadChunk>
  Each chunk: { disk_offset, start_frs, record_count, skip_begin, skip_end }
       │
Phase 5: IOCP SLIDING WINDOW
  IoCompletionPort::new() + associate(volume_handle)
  Issue N concurrent ReadFile operations
  Loop: GetQueuedCompletionStatus → parse buffer → issue next read
       │
Phase 6: INLINE PARSING
  For each 1KB record in completed buffer:
    fixup_file_record() → parse_record_to_index() → MftIndex
```

---

## Phase 1: Volume Access

### Opening the Volume

```rust
// platform/volume.rs — VolumeHandle::open()
pub fn open(drive_letter: char) -> Result<Self> {
    let path = format!("\\\\.\\{}:", drive_letter.to_ascii_uppercase());

    let handle = CreateFile(
        &path,
        GENERIC_READ.0,                           // Read-only access
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
        OPEN_EXISTING,
        FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED, // Direct + async
        None,
    )?;

    Ok(Self { handle, drive_letter })
}
```

**Critical flags:**
- **`FILE_FLAG_NO_BUFFERING`**: Bypasses the file system cache. Required for raw volume access and ensures we get current data (not stale cache). Requires sector-aligned buffers and read sizes.
- **`FILE_FLAG_OVERLAPPED`**: Enables asynchronous I/O via IOCP. Without this, `ReadFile` blocks until completion.

**Privilege requirement:** Opening `\\.\C:` for `GENERIC_READ` requires **Administrator privileges** or `SE_BACKUP_PRIVILEGE`.

### Volume Metadata

```rust
// FSCTL_GET_NTFS_VOLUME_DATA returns NtfsVolumeData
pub struct NtfsVolumeData {
    pub bytes_per_sector: u32,        // Usually 512
    pub bytes_per_cluster: u32,       // Usually 4096
    pub bytes_per_file_record: u32,   // Usually 1024
    pub mft_start_lcn: i64,           // Where $MFT begins on disk
    pub mft_valid_data_length: u64,   // Used size of $MFT
    pub mft_zone_start: i64,          // MFT reserved zone
    pub mft_zone_end: i64,            // MFT reserved zone end
    pub total_reserved: u64,          // Reserved clusters
}

// Derived values:
let mft_capacity = mft_valid_data_length / bytes_per_file_record;
// e.g., 2GB / 1024 = ~2M records
```

---

## Phase 2: Extent Mapping

The MFT itself can be fragmented across the disk. We need its "run list" — the mapping from virtual offsets within `$MFT` to physical positions on disk.

### Retrieval Pointers

```rust
// platform/volume.rs
pub fn get_retrieval_pointers(path: &str) -> Vec<MftExtent> {
    // Opens $MFT::$DATA (or $MFT::$BITMAP)
    // Calls FSCTL_GET_RETRIEVAL_POINTERS
    // Returns Vec<MftExtent { vcn, lcn, cluster_count }>
}
```

### MftExtent and MftExtentMap

```rust
// platform/extents.rs
pub struct MftExtent {
    pub vcn: u64,           // Virtual Cluster Number (offset within $MFT)
    pub lcn: i64,           // Logical Cluster Number (physical disk position)
                            // Negative = sparse (virtual gap, no data on disk)
    pub cluster_count: u64, // Number of contiguous clusters
}

// io/extent_map.rs
pub struct MftExtentMap {
    extents: Vec<MftExtent>,       // Sorted by VCN
    pub bytes_per_cluster: u32,    // e.g., 4096
    pub bytes_per_record: u32,     // e.g., 1024
}
```

**Example:** A fragmented MFT with 3 extents:

```
Extent 0: VCN 0     → LCN 786432,  clusters=500000  (first 500K records)
Extent 1: VCN 500000 → LCN 1200000, clusters=300000  (next 300K records)
Extent 2: VCN 800000 → LCN 2000000, clusters=200000  (final 200K records)
```

Each extent maps a range of MFT records to a contiguous physical region on disk. Between extents, the disk head may need to seek.

---

## Phase 3: Bitmap Skip Optimization

### The Problem

The MFT contains slots for ALL files ever created, including deleted ones:

```
Typical 2TB drive:
  MFT capacity:  5,000,000 records
  Records in use: 2,000,000
  Deleted/free:   3,000,000 (60% wasted I/O)
```

### The Solution: `$MFT::$BITMAP`

NTFS maintains a bitmap where each bit indicates whether an MFT record is in use:

```rust
// platform/bitmap.rs
pub struct MftBitmap {
    data: Vec<u8>,          // Raw bitmap bytes
    record_count: usize,    // Total records covered
}

impl MftBitmap {
    pub fn is_record_in_use(&self, frs: u64) -> bool {
        let byte_index = frs as usize / 8;
        let bit_index = frs as usize % 8;
        (self.data[byte_index] & (1 << bit_index)) != 0
    }

    pub fn count_in_use(&self) -> usize {
        self.data.iter()
            .map(|&byte| byte.count_ones() as usize)
            .sum()
    }

    pub fn max_frs_in_use(&self) -> u64 {
        // Scan backwards to find highest set bit
        // Used for pre-allocation sizing
    }
}
```

### Skip Calculation

For each MFT extent, the bitmap is scanned to find how many **contiguous unused records** exist at the beginning and end:

```rust
// io/chunking.rs — inside generate_read_chunks()
for extent in extent_map.extents() {
    let extent_start_frs = extent.vcn * records_per_cluster;
    let extent_records = extent.cluster_count * records_per_cluster;

    // Scan from beginning: how many unused records?
    let mut skip_begin = 0;
    while skip_begin < extent_records {
        if bitmap.is_record_in_use(extent_start_frs + skip_begin) {
            break;
        }
        skip_begin += 1;
    }

    // Scan from end: how many unused records?
    let mut skip_end = 0;
    while skip_end < extent_records - skip_begin {
        let frs = extent_start_frs + extent_records - 1 - skip_end;
        if bitmap.is_record_in_use(frs) {
            break;
        }
        skip_end += 1;
    }

    // Adjust read range to skip unused regions
    chunk.skip_begin = skip_begin;
    chunk.skip_end = skip_end;
}
```

### Impact

```
Before bitmap skip:  Read 5GB of MFT data
After bitmap skip:   Read 2GB of MFT data (60% reduction)
Speedup on HDD:     2.5× (I/O-bound)
Speedup on NVMe:    1.3× (CPU-bound, I/O savings smaller)
```

---

## Phase 4: Chunk Planning

### ReadChunk Structure

```rust
// io/chunking.rs
pub struct ReadChunk {
    pub disk_offset: u64,    // Physical byte offset on disk (LCN * cluster_size)
    pub start_frs: u64,      // First FRS in this chunk
    pub record_count: u64,   // Total records in chunk
    pub skip_begin: u64,     // Unused records at start (trimmed from I/O)
    pub skip_end: u64,       // Unused records at end (trimmed from I/O)
}

impl ReadChunk {
    pub fn effective_start_frs(&self) -> u64 {
        self.start_frs + self.skip_begin
    }

    pub fn effective_record_count(&self) -> u64 {
        self.record_count.saturating_sub(self.skip_begin + self.skip_end)
    }

    pub fn read_size(&self, record_size: u32) -> u64 {
        self.effective_record_count() * u64::from(record_size)
    }
}
```

### Chunk Generation

The `generate_read_chunks()` function:

1. Iterates over each MFT extent
2. Skips sparse extents (LCN < 0)
3. Calculates bitmap skip ranges (begin/end)
4. Splits large extents into I/O-sized chunks
5. Aligns chunk boundaries to sector size

```
Extent 0 (LCN 786432, 500K records):
  ├─► Chunk 0: offset=3.0GB, FRS 0-1023,     skip_begin=0, skip_end=0
  ├─► Chunk 1: offset=3.0GB+1MB, FRS 1024-2047, skip=0/0
  ├─► ...
  └─► Chunk N: offset=..., FRS 499K-500K,     skip_begin=0, skip_end=312

Total chunks: ~2000 (1MB each)
After skip:   ~1200 effective chunks (40% reduction)
```

### LCN-Ordered Reading

Chunks are **sorted by `disk_offset`** (LCN order) before reading. This minimizes disk seeks on HDDs:

```rust
sorted_chunks.sort_by_key(|c| c.disk_offset);
```

On NVMe/SSD this has no effect (random access is fast), but on HDDs it can improve throughput by 20-30%.

---

## Phase 5: IOCP Sliding Window

### I/O Completion Port Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    IOCP Sliding Window (concurrency = N)                 │
│                                                                         │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐       ┌─────────┐              │
│  │Buffer 0 │  │Buffer 1 │  │Buffer 2 │  ...  │Buffer N │  In-flight   │
│  │ReadFile │  │ReadFile │  │ReadFile │       │ReadFile │  reads       │
│  └────┬────┘  └────┬────┘  └────┬────┘       └────┬────┘              │
│       │            │            │                  │                    │
│       └────────────┴────────────┴──────────────────┘                   │
│                                │                                        │
│                                ▼                                        │
│       ┌────────────────────────────────────────────┐                   │
│       │    GetQueuedCompletionStatus (blocks)      │                   │
│       └────────────────────┬───────────────────────┘                   │
│                            │                                            │
│                            ▼                                            │
│       ┌────────────────────────────────────────────┐                   │
│       │ 1. Parse completed buffer (1KB records)    │                   │
│       │ 2. Return buffer to pool                   │                   │
│       │ 3. Issue next ReadFile (maintain N flights) │                   │
│       └────────────────────────────────────────────┘                   │
└─────────────────────────────────────────────────────────────────────────┘
```

### IoCompletionPort Wrapper

```rust
// io/readers/iocp/shared.rs
pub struct IoCompletionPort {
    handle: HANDLE,
}

impl IoCompletionPort {
    pub fn new(concurrency: u32) -> Result<Self> {
        // CreateIoCompletionPort(INVALID_HANDLE_VALUE, NULL, 0, concurrency)
        // concurrency=0 means use number of processors
    }

    pub fn associate(&self, file_handle: HANDLE, key: usize) -> Result<()> {
        // CreateIoCompletionPort(file_handle, self.handle, key, 0)
        // Associates the volume handle with this IOCP
    }
}

impl Drop for IoCompletionPort {
    fn drop(&mut self) {
        CloseHandle(self.handle); // RAII cleanup
    }
}
```

### OverlappedRead Structure

```rust
// io/readers/iocp/shared.rs
#[repr(C)]  // OVERLAPPED must be first field for pointer casting
pub struct OverlappedRead {
    overlapped: OVERLAPPED,      // Windows async I/O state
    pub buffer: AlignedBuffer,   // Sector-aligned read buffer
    pub chunk: ReadChunk,        // Which chunk this reads
    pub record_size: u32,        // For parsing
    pub bytes_read: usize,       // Filled on completion
    pub pool_index: usize,       // For buffer recycling
}
```

**Critical:** `OverlappedRead` is **pinned** in memory because Windows holds a pointer to the `OVERLAPPED` field until the I/O completes. Moving the struct would invalidate the pointer.

### Aligned Buffers

```rust
// io/aligned_buffer.rs
pub struct AlignedBuffer {
    ptr: *mut u8,
    len: usize,
    capacity: usize,
}

impl AlignedBuffer {
    pub fn new(capacity: usize) -> Self {
        // Allocates sector-aligned memory (512-byte aligned)
        // Required by FILE_FLAG_NO_BUFFERING
    }
}
```

`FILE_FLAG_NO_BUFFERING` requires:
- Buffer address aligned to sector size (512 bytes)
- Read size aligned to sector size
- File offset aligned to sector size

### The Read Loop

```rust
// io/readers/iocp/reader.rs — IocpMftReader::read_all_iocp()

// 1. Create IOCP and associate volume handle
let iocp = IoCompletionPort::new(0)?;
iocp.associate(handle, 0)?;

// 2. Sort chunks by disk offset (minimize seeks)
chunks.sort_by_key(|c| c.disk_offset);

// 3. Issue initial N concurrent reads
for i in 0..concurrency {
    let chunk = pending_chunks.pop_front();
    let mut op = OverlappedRead::new(buffer, chunk, record_size, i);
    op.set_offset(chunk.disk_offset + chunk.skip_begin * record_size);
    ReadFile(handle, op.buffer.as_mut_slice(), None, Some(op.as_overlapped_ptr()));
    in_flight.push(op);
}

// 4. Process completions and issue new reads
while in_flight_count > 0 {
    // Block until a read completes
    GetQueuedCompletionStatus(iocp.handle, &mut bytes, &mut key, &mut overlapped, INFINITE);

    // Parse the completed buffer
    let op = /* recover OverlappedRead from overlapped pointer */;
    for record_offset in (0..op.bytes_read).step_by(record_size) {
        let record_buf = &op.buffer[record_offset..record_offset + record_size];
        let frs = op.chunk.effective_start_frs() + (record_offset / record_size);
        parse_record_to_index(record_buf, frs, &mut index);
    }

    // Issue next read (maintain concurrency)
    if let Some(next_chunk) = pending_chunks.pop_front() {
        op.set_offset(next_chunk.disk_offset + ...);
        ReadFile(handle, ...);
    } else {
        in_flight_count -= 1;
    }
}
```

---

## Drive-Type-Aware Tuning

UFFS detects the drive type via WMI queries and selects optimal parameters:

### Detection

```rust
// platform/system.rs
pub fn detect_drive_type(drive_letter: char) -> DriveType {
    // WMI query: SELECT MediaType FROM MSFT_PhysicalDisk
    // Maps to: Nvme, Ssd, Hdd, Unknown
}
```

### Optimal Settings

```rust
// platform/system.rs
impl DriveType {
    pub fn optimal_concurrency(&self) -> usize {
        match self {
            Nvme    => 32,  // NVMe can handle many parallel reads
            Ssd     => 8,   // SSD benefits from moderate parallelism
            Hdd     => 4,   // HDD: more reads = more seeks = slower
            Unknown => 4,
        }
    }

    pub fn optimal_io_size(&self) -> usize {
        match self {
            Nvme    => 4 * 1024 * 1024,  // 4 MB chunks
            Ssd     => 2 * 1024 * 1024,  // 2 MB chunks
            Hdd     => 1 * 1024 * 1024,  // 1 MB chunks (sequential)
            Unknown => 1 * 1024 * 1024,
        }
    }

    pub fn benefits_from_parallel_parsing(&self) -> bool {
        matches!(self, Nvme)  // Only NVMe is fast enough for parsing to be bottleneck
    }
}
```

### HDD Extent-Aware Concurrency

For HDDs, concurrency is further tuned based on MFT fragmentation:

```rust
pub fn optimal_concurrency_for_hdd(extent_count: usize) -> usize {
    if extent_count > 50 { 2 }       // Heavily fragmented → minimize seeks
    else if extent_count > 20 { 4 }  // Moderately fragmented
    else { 6 }                        // Few extents → can parallelize more
}
```

---

## Read Mode Selection

UFFS supports multiple read strategies, automatically selected based on drive type:

| Mode | Description | Best For |
|------|-------------|----------|
| `SlidingIocpInline` | IOCP + inline parsing directly to `MftIndex` | **Default (all drives)** |
| `SlidingIocp` | IOCP + collect to `Vec<ParsedRecord>` → DataFrame | Analytics path |
| `PipelinedParallel` | Separate I/O and parse threads + Rayon | Multi-core HDD |
| `Parallel` | Read all, then parse with Rayon | SSD batch processing |
| `Streaming` | Sequential read + immediate parse | Low memory |
| `Prefetch` | Double-buffered read-ahead | HDD overlap |

### Auto Mode Resolution

```rust
// reader/read_mode.rs
pub fn index_effective_mode(mode: MftReadMode, drive_type: DriveType) -> MftReadMode {
    match mode {
        MftReadMode::Auto => MftReadMode::SlidingIocpInline, // All drive types
        other => other,
    }
}
```

The `SlidingIocpInline` mode is the production default because it:
1. Parses records **directly into `MftIndex`** (no intermediate allocation)
2. Uses IOCP for async I/O overlap
3. Works well on all drive types
4. Has the lowest memory overhead

---

## Buffer Recycling

UFFS recycles I/O buffers to avoid allocation overhead during the read loop:

```
Buffer Pool (size = concurrency):
  ┌──────────┐  ┌──────────┐  ┌──────────┐
  │ Buffer 0 │  │ Buffer 1 │  │ Buffer 2 │  ...
  │ 1-4 MB   │  │ 1-4 MB   │  │ 1-4 MB   │
  └──────────┘  └──────────┘  └──────────┘

After initial allocation:
  - Completed read → parse → return buffer to pool
  - Next read → take buffer from pool → issue ReadFile
  - Zero allocations during steady-state reading
```

---

## Multi-Drive Orchestration

### MultiDriveMftReader

When scanning multiple drives simultaneously:

```rust
// reader/multi_drive/mod.rs
pub struct MultiDriveMftReader {
    drives: Vec<char>,  // e.g., ['C', 'D', 'F']
}
```

### Bounded Parallelism

```rust
const MAX_CONCURRENT_DRIVE_READERS: usize = 4;

fn drive_reader_budget(total_drives: usize) -> usize {
    let hardware_budget = available_parallelism();
    total_drives
        .min(hardware_budget)
        .min(MAX_CONCURRENT_DRIVE_READERS)
}
```

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│            Multi-Drive Reader (tokio::spawn per drive)   │
│                                                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐               │
│  │  C: NVMe │  │  D: HDD  │  │  F: NVMe │               │
│  │  conc=32 │  │  conc=4  │  │  conc=32 │               │
│  │  4MB io  │  │  1MB io  │  │  4MB io  │               │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘               │
│       │              │              │                     │
│       ▼              ▼              ▼                     │
│  ┌──────────────────────────────────────────────────┐   │
│  │  Semaphore-bounded join (max 4 concurrent)       │   │
│  └──────────────────────────────────────────────────┘   │
│       │                                                  │
│       ▼                                                  │
│  Vec<MftIndex> (one per drive)                           │
└─────────────────────────────────────────────────────────┘
```

Each drive gets its own IOCP and its own tuning parameters based on detected drive type. Drives are read in parallel but bounded to avoid saturating system I/O.

---

## Error Handling and Robustness

### Common Failure Modes

| Failure | Handling |
|---------|----------|
| Access denied | `MftError::PermissionDenied` — requires admin |
| Volume locked | Retry with `FILE_SHARE_READ \| WRITE \| DELETE` |
| Corrupted record | USA fixup returns `false` → skip record |
| Short read | Process only complete records in buffer |
| MFT growth during read | Process records up to bitmap limit |
| Sparse extent | Skip (no physical data) |

### Graceful Degradation

```rust
// If bitmap read fails, fall back to reading everything
let bitmap = match read_bitmap(&handle, &bitmap_extents) {
    Ok(bm) => Some(bm),
    Err(_) => {
        warn!("Failed to read MFT bitmap, reading all records");
        None  // generate_read_chunks will skip optimization
    }
};
```

---

## Performance Characteristics

### Typical Performance (NVMe, 2M files)

| Phase | Time | Notes |
|-------|------|-------|
| Volume open | <1ms | One-time |
| Get volume data | ~2ms | FSCTL_GET_NTFS_VOLUME_DATA |
| Get retrieval pointers | ~5ms | Two calls ($DATA, $BITMAP) |
| Read bitmap | ~10ms | Small file (~250KB for 2M records) |
| Generate chunks | ~1ms | In-memory calculation |
| **IOCP MFT read + parse** | **5-7s** | **Dominant cost** |
| Post-processing | ~0.5s | Tree metrics, extension index |
| **Total** | **~6-8s** | |

### I/O Bandwidth Utilization

| Drive Type | Sequential Read BW | UFFS Achieved | Utilization |
|------------|-------------------|---------------|-------------|
| NVMe | 3-7 GB/s | 300-500 MB/s | ~10%* |
| SSD | 500 MB/s | 200-400 MB/s | ~60% |
| HDD | 100-200 MB/s | 60-150 MB/s | ~75% |

\* NVMe utilization is low because **parsing is the bottleneck**, not I/O. The parallel parse mode helps here.

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62*
