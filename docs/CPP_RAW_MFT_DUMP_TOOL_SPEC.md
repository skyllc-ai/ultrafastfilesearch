


> Goal: produce a **canonical raw $MFT snapshot** from Windows (C++ tooling) that we can compare 1:1 against the Rust implementation.
>
> You **do not** need access to our Rust code. This document provides all file formats and expectations you need.

## 1. High-Level Requirements

You will build a small C++ console program that runs on Windows and:

1. Opens a given NTFS volume (e.g. `F:`) using `CreateFileW("\\\\.\\F:", ...)`.
2. Uses the **NTFS control codes** to obtain volume/MFT geometry:
   - `FSCTL_GET_NTFS_VOLUME_DATA`.
3. Uses `FSCTL_GET_RETRIEVAL_POINTERS` to obtain the complete runlist (extents) of the `$MFT` stream.
4. Reads **all clusters that belong to `$MFT`** and arranges them as a contiguous byte array representing FRS `0..N-1`.
5. Writes this array to disk in a simple binary format we define below (`UFFS-MFT`), including a small header.

We will then:

- Load your file in our Rust tools.
- Compare it against what our Rust reader currently produces for the same volume.
- Use any differences to pinpoint whether our issues are in **raw reading** or in **higher-level interpretation**.

## 2. Output File Format: `UFFS-MFT`

Your program should write a single binary file with this structure:

```text
[Header: 64 bytes]
  - Magic: "UFFS-MFT" (ASCII, 8 bytes)
  - Version: u32 (4 bytes, little-endian), currently 1
  - Flags: u32 (4 bytes, little-endian)
      - bit 0 (0x00000001): data is zstd-compressed (we will NOT use this; keep 0)
  - Record size: u32 (4 bytes, little-endian)
      - Size in bytes of a single MFT record (e.g. 1024)
  - Record count: u64 (8 bytes, little-endian)
      - Number of records represented in the data region
  - Original size: u64 (8 bytes, little-endian)
      - Total size in bytes of the uncompressed data region (= record_size * record_count)
  - Compressed size: u64 (8 bytes, little-endian)
      - If Flags & 0x00000001 != 0, this is the size of compressed data in bytes
      - For our tool, this MUST be 0 (no compression)
  - Reserved: 20 bytes (set to 0)

[Data: variable]
  - Raw MFT bytes (uncompressed)
  - Length MUST be exactly `record_size * record_count`
  - Record i (FRS = i) begins at offset `i * record_size`
```

### 2.1 Concrete header writing in C++ (pseudo-code)

```c++
// 64-byte header buffer
unsigned char header[64] = {};

// Magic "UFFS-MFT"
const char magic[8] = {'U','F','F','S','-','M','F','T'};
memcpy(header + 0, magic, 8);

// Version = 1 (u32 LE)
uint32_t version = 1;
memcpy(header + 8, &version, 4);

// Flags = 0 (no compression)
uint32_t flags = 0;
memcpy(header + 12, &flags, 4);

// Record size in bytes (e.g. 1024)
uint32_t record_size = bytesPerFileRecordSegment; // from NTFS volume data
memcpy(header + 16, &record_size, 4);

// Record count (u64) = total_bytes / record_size
uint64_t record_count = totalBytes / record_size;
memcpy(header + 20, &record_count, 8);

// Original size (u64) = total_bytes
uint64_t original_size = totalBytes;
memcpy(header + 28, &original_size, 8);

// Compressed size (u64) = 0 (no compression)
uint64_t compressed_size = 0;
memcpy(header + 36, &compressed_size, 8);

// Bytes 44..63 remain zero (reserved)
// Now write header[0..63] to the output file, then the data region.
```

We are fine with plain `memcpy` as long as you compile for little-endian Windows (standard). If you prefer, you can explicitly encode to LE using bit operations.

## 3. NTFS Structures and APIs You Need

### 3.1. NTFS Volume data: `FSCTL_GET_NTFS_VOLUME_DATA`

Use `DeviceIoControl` with control code `FSCTL_GET_NTFS_VOLUME_DATA` on the volume handle (e.g. `\\.\\F:`) to fill an `NTFS_VOLUME_DATA_BUFFER`.

From that structure, you will use at least:

- `BytesPerSector`
- `BytesPerCluster`
- `BytesPerFileRecordSegment`
- `MftValidDataLength` (LONGLONG)
- `MftStartLcn` (LONGLONG)

You can also log:

- `MftZoneStart`, `MftZoneEnd`, `Mft2StartLcn`, etc. for debugging, but they are not required for the dump.

### 3.2. Retrieving MFT extents: `FSCTL_GET_RETRIEVAL_POINTERS`

To find where `$MFT` lives physically on disk, call `DeviceIoControl` on a handle to `$MFT` itself with `FSCTL_GET_RETRIEVAL_POINTERS`.

Typical steps:

1. Open the `$MFT` file:
   - `CreateFileW(L"\\\\.\\F:\\:$MFT", ...)` or an equivalent way your existing C++ code already uses to get the MFT stream.
2. Initialize an `STARTING_VCN_INPUT_BUFFER` with `StartingVcn = 0`.
3. Allocate a reasonably sized buffer for `RETRIEVAL_POINTERS_BUFFER`.
4. Call `DeviceIoControl` with `FSCTL_GET_RETRIEVAL_POINTERS` in a loop until you have all extents.

Important semantics for the loop:

- If `DeviceIoControl` returns `FALSE` with `GetLastError() == ERROR_MORE_DATA`:
  - This means your buffer is too small to hold all the extents **for this `StartingVcn`**.
  - **Correct behavior**: increase the buffer size and retry **with the same `StartingVcn`**.
  - Do NOT treat partial data as final, and do NOT advance `StartingVcn` in this case.
- If `DeviceIoControl` returns `FALSE` with `GetLastError() == ERROR_HANDLE_EOF`:
  - You have reached the end of the runlist: stop collecting extents.
- On success, the `RETRIEVAL_POINTERS_BUFFER` contains:
  - A `StartingVcn` (the first VCN of this query),
  - A `NumberOfExtents`,
  - An array of `RETRIEVAL_POINTERS_BUFFER::Extents[i]` with fields:
    - `NextVcn`
    - `Lcn`

From this, you build a list of extents for `$MFT`:

```text
Extent[0]: VcnStart = StartingVcn
           LcnStart = Extents[0].Lcn
           ClusterCount = Extents[0].NextVcn - StartingVcn
Extent[1]: VcnStart = Extents[0].NextVcn
           LcnStart = Extents[1].Lcn
           ClusterCount = Extents[1].NextVcn - Extents[0].NextVcn
...
```

Continue from the last `NextVcn` until `ERROR_HANDLE_EOF`.

### 3.3. Reading the MFT bytes

Once you have the extents for the `$MFT` stream and the `BytesPerCluster`, you can compute:

- For extent `i`:
  - `LcnStart` (cluster index)
  - `ClusterCount`
  - This extent covers byte offset:
    - `byteOffset = LcnStart * BytesPerCluster`
    - `byteLength = ClusterCount * BytesPerCluster`

Your goal: read all these bytes from the **same file/handle you used for `FSCTL_GET_RETRIEVAL_POINTERS`** (the `$MFT` stream).

Use `SetFilePointerEx` and `ReadFile` (or their C++ equivalents) to:

1. Seek to `byteOffset`.
2. Read `byteLength` bytes into an output buffer, appended sequentially.

Do this for all extents in ascending VCN order, so the resulting buffer corresponds to `$MFT` VCN 0..end in order.

### 3.4. Mapping to File Record Segments (FRS)

By NTFS design:

- `BytesPerFileRecordSegment` (let's call it `R`) is typically 1024.
- The MFT stream is conceptually a contiguous array of `R`-byte records (FRS).

From `NTFS_VOLUME_DATA_BUFFER`:

- Let `R = BytesPerFileRecordSegment`.
- Let `totalBytes` be the total number of bytes you read from all `$MFT` extents.

Define:

- `record_size = R`.
- `record_count = totalBytes / record_size` (integer division; drop any incomplete tail if it exists).

We expect this `record_count` to be very close to the `MftValidDataLength / BytesPerFileRecordSegment` you see in `FSCTL_GET_NTFS_VOLUME_DATA`.

When writing the data region:

- Write the extents in VCN order into one contiguous buffer.
- Do **not** reorder, skip, or parse records.
- The i-th record (FRS i) is the `record_size` bytes starting at offset `i * record_size` in your final data buffer.

## 4. Program Interface & Usage

### 4.1. Command-line interface

A simple, pragmatic interface is enough:

- `RawMftDump.exe F f_mft_cpp.raw`

Where:

- First argument: a drive letter (without colon), e.g. `F`.
- Second argument: path to the output file, e.g. `C:\Users\rn\GitHub\UltraFastFileSearch\docs\trial_runs\UltraFastFileSearch\f_mft_cpp.raw`.

Behavior:

1. Parse drive letter.
2. Build volume path: `L"\\\\.\\F:"`.
3. Open volume and NTFS `$MFT` stream using whatever method your existing code uses.
4. Retrieve NTFS volume data and MFT extents.
5. Read the entire `$MFT` stream into memory or write it chunked to the output file.
6. Write the 64-byte `UFFS-MFT` header.
7. Write the raw data bytes immediately afterwards.
8. Print a short summary to stdout, e.g.:
   - `BytesPerFileRecordSegment = 1024`
   - `Total extents = 28, totalBytes = 4,547,123,200, recordCount = 4,656,384`
   - `Output file = f_mft_cpp.raw`

You can assume the tool is run from PowerShell with administrator rights.

## 5. What We Will Do With Your File

On our side (Rust tooling), we will:

1. Copy your `f_mft_cpp.raw` into our repo under:
   - `docs/trial_runs/UltraFastFileSearch/f_mft_cpp.raw`
2. Load it with our `RawMftData` loader, which expects precisely the `UFFS-MFT` format defined above.
3. Compare it against our own snapshot (from our Rust reader), e.g. `f_mft_rust.raw`.

We will run diagnostics that:

- Verify header fields (`record_size`, `record_count`, `original_size`).
- Compare magic distribution (`FILE`, `RCRD`, `ZERO`, etc.) by FRS bucket.
- Compute hashes or diffs of individual records to see if/where raw data diverges.

If the two raw snapshots match bit-for-bit, we can be confident that:

- Our extent and read logic is equivalent to yours.
- Any remaining difference vs C++ behavior comes from **how we interpret those bytes**, not from how we read them.

If they diverge, the diff will tell us exactly which FRS ranges behave differently, and we can feed that back into the NTFS I/O layer.

## 6. Edge Cases and Notes

- The volume is live; the MFT can change while you are reading it. In practice, this usually produces a small number of differences, not an entirely different tail.
- We are primarily interested in whether your snapshot and ours:
  - Agree on the **layout** (same `record_count`), and
  - Have the same data for the vast majority of FRS, especially in high-FRS regions where we observe problems.
- Please do **not** perform multi-sector fixup or any form of parsing on the records in this tool. We want the raw bytes.

That should be all you need on the C++ side. If any part of the Windows/NTFS API usage is unclear, you can implement it according to standard Microsoft documentation; we will adapt on the Rust side as long as the final file format is exactly as specified in section 2.

