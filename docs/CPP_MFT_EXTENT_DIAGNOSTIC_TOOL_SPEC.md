# C++ MFT Extent Diagnostic Tool Specification

## Purpose

We discovered that the Rust MFT reader is only reading the **first extent** of fragmented MFTs, while C++ correctly reads all 28 extents on the F: drive. This tool will dump the complete extent map so we can:

1. Compare C++ extent data vs Rust extent data
2. Verify both tools see the same physical disk layout
3. Debug the Rust `get_mft_extents()` / `FSCTL_GET_RETRIEVAL_POINTERS` implementation

## Evidence of the Bug

```
FRS 0 to 1,071,679:     IDENTICAL (first extent = 16,745 clusters = 1,097,400,320 bytes)
FRS 1,071,680 onward:   Rust has ZEROS, C++ has valid FILE records
```

Rust is missing **77% of the MFT** (3,584,704 records out of 4,656,384).

---

## Tool: `uffs.com --dump-extents=<drive>`

### Output Format: JSON

```json
{
  "drive": "F",
  "timestamp": "2025-01-22T11:17:00Z",
  "volume_info": {
    "bytes_per_sector": 512,
    "bytes_per_cluster": 65536,
    "bytes_per_file_record": 1024,
    "mft_start_lcn": 7949042,
    "mft_valid_data_length": 4768137216,
    "total_clusters": 72789647
  },
  "mft_extents": [
    {
      "index": 0,
      "vcn": 0,
      "lcn": 7949042,
      "cluster_count": 16745,
      "start_frs": 0,
      "end_frs": 1071679,
      "byte_offset": 520919859200,
      "byte_length": 1097400320
    },
    {
      "index": 1,
      "vcn": 16745,
      "lcn": 12345678,
      "cluster_count": 5000,
      "start_frs": 1071680,
      "end_frs": 1391679,
      "byte_offset": 809123020800,
      "byte_length": 327680000
    }
  ],
  "summary": {
    "extent_count": 28,
    "total_clusters": 72789,
    "total_records": 4656384,
    "total_bytes": 4768137216,
    "is_fragmented": true
  }
}
```

### Field Definitions

| Field | Description |
|-------|-------------|
| `vcn` | Virtual Cluster Number (logical offset within MFT file) |
| `lcn` | Logical Cluster Number (physical location on disk) |
| `cluster_count` | Number of clusters in this extent |
| `start_frs` | First FRS (File Record Segment) number in this extent |
| `end_frs` | Last FRS number in this extent (inclusive) |
| `byte_offset` | Physical byte offset on disk (`lcn * bytes_per_cluster`) |
| `byte_length` | Extent size in bytes (`cluster_count * bytes_per_cluster`) |

---

## Implementation Reference

### Getting Retrieval Pointers

```cpp
// Open $MFT to get its extents
HANDLE hMft = CreateFileW(
    L"\\\\.\\C:\\$MFT",           // or use drive letter
    0,                            // No access needed
    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    NULL,
    OPEN_EXISTING,
    0,
    NULL
);

// Input: starting VCN
STARTING_VCN_INPUT_BUFFER startVcn = { 0 };

// Output buffer - start large enough
std::vector<BYTE> buffer(64 * 1024);
DWORD bytesReturned = 0;

BOOL success = DeviceIoControl(
    hMft,
    FSCTL_GET_RETRIEVAL_POINTERS,
    &startVcn, sizeof(startVcn),
    buffer.data(), (DWORD)buffer.size(),
    &bytesReturned,
    NULL
);

// Handle ERROR_MORE_DATA (234) by growing buffer and retrying
// Do NOT advance StartingVcn on ERROR_MORE_DATA!

if (success) {
    RETRIEVAL_POINTERS_BUFFER* rpb = (RETRIEVAL_POINTERS_BUFFER*)buffer.data();
    
    LONGLONG prevVcn = rpb->StartingVcn.QuadPart;
    for (DWORD i = 0; i < rpb->ExtentCount; i++) {
        LONGLONG nextVcn = rpb->Extents[i].NextVcn.QuadPart;
        LONGLONG lcn = rpb->Extents[i].Lcn.QuadPart;
        LONGLONG clusterCount = nextVcn - prevVcn;
        
        // Output extent info
        printf("Extent %d: VCN=%lld, LCN=%lld, Clusters=%lld\n",
               i, prevVcn, lcn, clusterCount);
        
        prevVcn = nextVcn;
    }
}

CloseHandle(hMft);
```

---

## Additional Diagnostic: Raw Extent Verification

Also useful: a mode that **reads the first record from each extent** and prints the FRS number from the record header to verify the extent mapping is correct:

```
--dump-extents=F --verify
```

Output:
```
Extent 0: VCN=0, LCN=7949042 -> Read FRS 0, header says FRS=0 ✓
Extent 1: VCN=16745, LCN=12345678 -> Read FRS 1071680, header says FRS=1071680 ✓
...
```

This confirms the extent LCNs actually point to the expected FRS ranges.

---

## Why This Matters

The Rust code has this fallback when `CreateFileW` fails:

```rust
// WRONG for fragmented MFT!
return Ok(vec![MftExtent {
    vcn: 0,
    cluster_count: mft_valid_data_length / bytes_per_cluster,  // Full size
    lcn: mft_start_lcn,  // But only first LCN!
}]);
```

This creates ONE extent covering the entire MFT size but using only the first fragment's LCN. Records beyond the first extent read from wrong disk locations (or past EOF), returning zeros.

---

## Deliverable

1. `uffs.com --dump-extents=F` → outputs JSON to stdout (or `--dump-extents-out=file.json`)
2. Test on F: drive (28 extents expected)
3. Share the JSON output so we can compare against Rust's extent retrieval

---

## Analysis of C++ vs Rust Differences

### C++ Extent Data (28 extents, all correct)

The C++ tool successfully retrieved all 28 extents. Key observations:

| Extent | VCN | LCN | Clusters | FRS Range |
|--------|-----|-----|----------|-----------|
| 0 | 0 | 7,949,042 | 3,202 | 0-204,927 |
| 1 | 3,202 | 8,378,124 | 3,345 | 204,928-419,007 |
| 2 | 6,547 | 8,748,739 | 1,865 | 419,008-538,367 |
| 3 | 8,412 | 9,237,441 | 4,683 | 538,368-838,079 |
| 4 | 13,095 | 9,469,388 | 3,650 | 838,080-1,071,679 |
| **5** | **16,745** | **3,367,678** | 3,199 | **1,071,680**-1,276,415 |
| ... | ... | ... | ... | ... |
| 27 | 70,496 | 12,474,645 | 2,260 | 4,511,744-4,656,383 |

**Critical finding**: Rust stops at FRS 1,071,679 (end of extent 4). Extent 5 starts at FRS 1,071,680.

### Rust Bugs Identified and Fixed (2026-01-22)

| Bug | Before | After | Status |
|-----|--------|-------|--------|
| Path format | `\\.\F:\$MFT` | `F:\$MFT` | ✅ Fixed |
| File access | `FILE_READ_ATTRIBUTES.0` | `0` (no access needed) | ✅ Fixed |
| File flags | `FILE_FLAG_OPEN_REPARSE_POINT \| FILE_FLAG_NO_BUFFERING` | `FILE_FLAGS_AND_ATTRIBUTES(0)` | ✅ Fixed |
| HRESULT extraction | `err.code().0 as u32 == 234` (never matches) | Extract Win32 from HRESULT: `(hresult & 0xFFFF)` | ✅ Fixed |

### Fix Details

**Commit**: `f3f356dff` on `main`

**Root cause**: `CreateFileW` was failing silently due to incorrect path format and flags, triggering the single-extent fallback. This fallback created one extent covering the full MFT size but using only the first LCN, causing reads beyond the first physical extent to return zeros.

**HRESULT issue**: Windows returns HRESULT `0x800700EA` for ERROR_MORE_DATA, not raw Win32 error `234`. The comparison `error_code == 234` never matched because `0x800700EA as u32 = 2147942634 ≠ 234`.

### Rust Code Location

`crates/uffs-mft/src/platform.rs`:
- Lines 312-350: `get_mft_extents()` - path format and file flags fix
- Lines 668-705: `get_retrieval_pointers()` - HRESULT to Win32 extraction fix

---

## Verification Steps

After pulling the fix, run on Windows:

```powershell
# 1. Verify Rust now sees all 28 extents
dump_mft_extents F

# 2. Re-dump MFT with fixed code
uffs_mft save --drive F --output f_mft_rust_fixed.raw --no-compress

# 3. Compare against C++ dump
compare_raw_mft f_mft_cpp.raw f_mft_rust_fixed.raw
```

Expected result: **100% match** (0 differing records)

