# Cache Revival Plan — Deep Dive & Resolution

## Executive Summary

The caching infrastructure is **fully wired and mostly functional** — the code paths exist for save, load, TTL, USN journal incremental updates, and multi-drive coordination. However, after the performance refactoring (SlidingIocpInline, streaming output), there are **two concrete bugs** and **one missing feature** that prevent cached indexes from being search-ready after deserialization.

---

## 1. Current Architecture (What Exists)

### Cache File Format
- **Location**: `%TEMP%\uffs_index_cache\{DRIVE}_index.uffs`
- **Format version**: 8 (latest)
- **Header**: magic, version, volume, volume_serial, usn_journal_id, next_usn, created_at, counts
- **Data**: frs_to_idx table → records → names → links → streams → children → extension table
- **Serialization**: `index/storage/serialize.rs` (184 lines)
- **Deserialization**: `index/storage/deserialize.rs` (496 lines)

### Cache Flow
```
load_live_index(drive, no_cache=false)
  → MftReader::open(drive)
  → reader.read_index_cached(INDEX_TTL_SECONDS=600)
      → check_cache_status(drive, ttl)
         → CacheStatus::Fresh  → load from disk, apply USN updates
         → CacheStatus::Stale  → full MFT read + save to cache
         → CacheStatus::Missing → full MFT read + save to cache
```

### USN Journal Integration (Already Implemented)
- `read_index_cached()` in `reader/index_cache.rs` already:
  1. Loads cached index from disk
  2. Queries current USN journal state
  3. Validates journal ID hasn't changed
  4. Validates checkpoint USN is still in journal range
  5. Reads USN changes since checkpoint
  6. Aggregates changes per-FRS
  7. Calls `index.apply_usn_changes()` — handles creates, deletes, renames, metadata
  8. Saves updated index back to cache with new USN checkpoint

### What `apply_usn_changes()` Does
- **Deletes**: Sets bit 31 of `stdinfo.flags` (DELETED_FLAG)
- **Creates**: Adds placeholder FileRecord (limited: only FRS, parent, name from USN)
- **Renames**: Updates filename in names buffer + parent_frs
- **Size/Metadata changes**: Marks as modified but **cannot update actual values** (would need selective MFT read)

---

## 2. What's Broken — Root Causes

### Bug 1: `extension_index` Not Rebuilt After Deserialization

**Severity**: HIGH — breaks all `*.ext` filtered queries on cached indexes

**Location**: `index/storage/deserialize.rs:479`

```rust
extension_index: None,  // ← Always None after load
```

The `ExtensionIndex` (the per-extension → record-indices lookup) is **not serialized** and is set to `None` on deserialization. Neither `load_cached_index()` nor `read_index_cached()` calls `build_extension_index()` after loading.

Compare with the fresh-build paths that DO call it:
- `from_parsed_records()` → `build_extension_index()` (builder.rs:426)
- `read_mft_index_internal()` SlidingIocpInline → `build_extension_index()` (index_read.rs:716)
- `read_all_sliding_window_iocp_to_index()` → `build_extension_index()` (raw_iocp.rs:656)

**Fix**: Call `index.build_extension_index()` after deserialization in `deserialize()` or in `read_index_cached()` after loading.

**Best location**: In `deserialize()` itself (one fix covers all callers):
```rust
// After index.recompute_stats(); at line 485:
index.build_extension_index();
```

### Bug 2: `reserved_allocated_bytes` Always Zero on Cached Index

**Severity**: MEDIUM — tree_allocated for root directory will be wrong

**Location**: `index/storage/deserialize.rs:481`

```rust
reserved_allocated_bytes: 0,  // ← Always 0 after load
```

This field is set from `volume_data.reserved_allocated_bytes()` during live reads (index_read.rs:691) but is **not serialized/deserialized**. It affects the root directory's `tree_allocated` computation.

However, since tree metrics (descendants, treesize, tree_allocated) **are** serialized and persisted in the cache file, this only matters if `compute_tree_metrics()` is called again (e.g., after USN updates). The USN update path in `incremental.rs:556` does call `compute_tree_metrics()` — so after USN updates, the root's tree_allocated will be off.

**Fix options** (pick one):
1. Serialize `reserved_allocated_bytes` in the cache file (adds 8 bytes to format, bump version to 9)
2. Re-read it from VolumeHandle after cache load (adds one syscall, no format change)
3. Accept the minor inaccuracy (only affects root tree_allocated after USN updates)

**Recommended**: Option 2 — re-read from VolumeHandle in `read_index_cached()` after loading:
```rust
CacheStatus::Fresh { mut index, header, age_seconds } => {
    // Restore reserved_allocated_bytes from live volume data
    let handle = VolumeHandle::open(drive)?;
    index.reserved_allocated_bytes = handle.volume_data().reserved_allocated_bytes();
    // ... rest of USN update logic
}
```

### Missing Feature: `internal_streams_size` / `internal_streams_allocated` Not Serialized

**Severity**: LOW — only affects specific treesize edge cases

**Location**: `index/storage/deserialize.rs:298-300`

```rust
internal_streams_size: 0,
internal_streams_allocated: 0,
```

These fields track $I30 and other internal NTFS streams that contribute to directory treesize. They're computed during parsing but not serialized. After cache load + USN update, recomputed tree metrics may differ slightly from fresh reads.

**Fix**: Serialize these two u64 fields per record (adds 16 bytes × N records, format version bump).

---

## 3. What's Actually Working

- **Cache save**: ✅ `save_to_cache()` works, called from `read_and_cache_index()`
- **Cache load**: ✅ `load_from_file()` / `load_cached_index()` works
- **TTL checking**: ✅ `is_cache_fresh()`, 10-minute default
- **USN journal query**: ✅ `query_usn_journal()` via `FSCTL_QUERY_USN_JOURNAL`
- **USN journal read**: ✅ `read_usn_journal()` via `FSCTL_READ_USN_JOURNAL`
- **USN change aggregation**: ✅ `aggregate_changes()` groups by FRS
- **USN apply to index**: ✅ `apply_usn_changes()` handles CRUD
- **Journal ID validation**: ✅ Detects recreated journals → full rebuild
- **Journal wrap detection**: ✅ Detects USN out of range → full rebuild
- **Multi-drive coordination**: ✅ `check_multi_drive_cache()` rebuilds all if any stale
- **Cache CLI commands**: ✅ `cache-status`, `cache-get`, `cache-clear`
- **`--no-cache` flag**: ✅ Bypasses cache, reads fresh

---

## 4. Resolution Plan — Minimal Fixes

### Fix 1: Rebuild extension_index after deserialization (HIGH priority)

**File**: `crates/uffs-mft/src/index/storage/deserialize.rs`

After line 485 (`index.recompute_stats();`), add:
```rust
index.build_extension_index();
```

This is a single line that makes all cached indexes fully search-ready. The `build_extension_index()` call is fast (O(n) scan) and takes ~50-100ms on large drives.

### Fix 2: Restore reserved_allocated_bytes in cached path (MEDIUM priority)

**File**: `crates/uffs-mft/src/reader/index_cache.rs`

In the `CacheStatus::Fresh` branch of `read_index_cached()`, after loading the index, re-read the volume data:
```rust
CacheStatus::Fresh { mut index, header, age_seconds } => {
    // Restore reserved_allocated_bytes (not serialized in cache)
    if let Ok(handle) = VolumeHandle::open(drive) {
        index.reserved_allocated_bytes = handle.volume_data().reserved_allocated_bytes();
    }
    // ... existing USN update logic ...
}
```

### Fix 3 (Optional): Serialize internal_streams fields (LOW priority)

This requires a version bump to 9 and adds 16 bytes per record to the cache file. Only needed for exact treesize parity after USN-based cache updates. Can be deferred.

---

## 5. USN Journal — Current Limitations & Improvements

### What USN Updates CAN Do (already implemented)
- Detect new files → create placeholder records
- Detect deleted files → mark with DELETED_FLAG  
- Detect renames → update filename and parent_frs
- Detect size/metadata changes → mark as modified (no value update)

### What USN Updates CANNOT Do (limitations)
1. **No actual size/timestamp updates** — USN only tells us THAT something changed, not the new values. Would need selective MFT read for changed FRS records.
2. **Placeholder records are incomplete** — created records only have FRS, parent, and name. Missing: size, timestamps, attributes, extension classification.
3. **No extension table updates** — new files from USN don't get their extensions added to the ExtensionTable.
4. **No tree metrics update for creates** — new placeholder records have descendants=0, treesize=0.

### Recommended Phase 2: Selective MFT Read for Changed Records

For records where USN reports size/metadata changes or new creates:
1. Collect FRS numbers of changed records
2. Read those specific MFT records directly (random I/O, but typically <1000 records)
3. Parse them with the existing parser
4. Update the index entries with real data

This would give **near-perfect accuracy** on cached indexes with USN updates, at minimal I/O cost (typically <1MB of MFT reads for a few hundred changes).

### Implementation Sketch
```rust
// In read_index_cached(), after apply_usn_changes():
let frs_to_reread: Vec<u64> = changes.iter()
    .filter(|c| c.created || c.size_changed || c.metadata_changed)
    .map(|c| c.frs)
    .collect();

if !frs_to_reread.is_empty() {
    let handle = VolumeHandle::open(drive)?;
    let fresh_records = read_specific_mft_records(&handle, &frs_to_reread)?;
    index.merge_fresh_records(fresh_records);
}
```

---

## 6. Benchmark Impact Estimate

| Scenario | Current (no cache) | After fix (cache hit, no changes) | After fix (cache + USN) |
|----------|-------------------|----------------------------------|------------------------|
| Single NVMe | ~5-8s | **~0.3-0.5s** (deserialize + ext_index) | **~0.5-1.0s** (+USN read/apply) |
| Single HDD | ~30-70s | **~0.3-0.5s** | **~0.5-1.0s** |
| All drives parallel | ~70s | **~0.5-1.0s** | **~1.0-2.0s** |

The cache format is already compact (~100-200MB for large drives). Deserialization is I/O bound (sequential file read), much faster than MFT parsing.

---

## 7. Files to Change

| File | Change | Priority |
|------|--------|----------|
| `crates/uffs-mft/src/index/storage/deserialize.rs` | Add `build_extension_index()` after `recompute_stats()` | **HIGH** |
| `crates/uffs-mft/src/reader/index_cache.rs` | Restore `reserved_allocated_bytes` from VolumeHandle | MEDIUM |
| `crates/uffs-mft/src/index/storage/serialize.rs` | (Optional) Add `internal_streams_*` fields | LOW |
| `crates/uffs-mft/src/index/storage/deserialize.rs` | (Optional) Read `internal_streams_*` fields | LOW |
| `crates/uffs-mft/src/index/storage/header.rs` | (Optional) Bump version to 9 | LOW |

**Total effort for HIGH+MEDIUM fixes: ~5 lines of code changes.**
