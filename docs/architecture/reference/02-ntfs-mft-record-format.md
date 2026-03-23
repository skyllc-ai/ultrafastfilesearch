# MFT Record Format

## Introduction

This document provides a complete byte-level reference for NTFS Master File Table (MFT) records. All information is based on publicly available Microsoft NTFS documentation, Windows SDK/WDK headers, and published NTFS specifications.

---

## Record Overview

Every file and directory on an NTFS volume is represented by one or more fixed-size **File Record Segments** (FRS) in the MFT.

| Property | Value | Notes |
|----------|-------|-------|
| **Record size** | 1024 bytes (typical) | Can be 512, 1024, 2048, or 4096 |
| **Fixed size** | Yes | All records same size on a given volume |
| **Addressing** | By FRS number | `byte_offset = FRS × record_size` |
| **Magic number** | `0x454C4946` | "FILE" in little-endian |
| **Alignment** | Record-size boundary | Records are contiguous, no delimiters |

---

## FILE_RECORD_SEGMENT_HEADER

The first 48 bytes of every MFT record form the fixed header.

```
Offset  Size  Type   Field                        Description
──────  ────  ─────  ───────────────────────────  ──────────────────────────────────
0x000   4     u32    Magic                        "FILE" = 0x454C4946 (LE)
0x004   2     u16    USAOffset                    Offset to Update Sequence Array
0x006   2     u16    USACount                     Number of USA entries (incl. check)
0x008   8     u64    LogFileSequenceNumber        $LogFile journal correlation
0x010   2     u16    SequenceNumber               Incremented each time FRS is reused
0x012   2     u16    LinkCount                    Number of hard links
0x014   2     u16    FirstAttributeOffset         Byte offset to first attribute
0x016   2     u16    Flags                        See flags table below
0x018   4     u32    BytesInUse                   Actual bytes used in this record
0x01C   4     u32    BytesAllocated               Total record size (= record_size)
0x020   8     u64    BaseFileRecordSegment        Base FRS for extension records (0 if base)
0x028   2     u16    NextAttributeNumber          Next available attribute instance ID
0x02A   2     u16    Reserved                     Padding (Windows XP+: SegmentNumberUpper)
0x02C   4     u32    SegmentNumberLower           FRS number (lower 32 bits)
```

**Total header size: 48 bytes (0x30)**

### Flags

| Bit | Mask | Name | Meaning |
|-----|------|------|---------|
| 0 | 0x0001 | `FRH_IN_USE` | Record contains an active file/directory |
| 1 | 0x0002 | `FRH_DIRECTORY` | Record is a directory (has index) |

### Validation Checks

A valid MFT record must satisfy:
1. `Magic == 0x454C4946` ("FILE")
2. `Flags & 0x0001` is set (record in use) — for active files
3. `FirstAttributeOffset < BytesInUse`
4. `BytesInUse <= BytesAllocated`
5. USA fixup passes (see below)

### Base vs Extension Records

- **Base record**: `BaseFileRecordSegment == 0` — this is the primary record for the file
- **Extension record**: `BaseFileRecordSegment != 0` — points to the base FRS; holds overflow attributes that didn't fit in the base record

---

## Update Sequence Array (USA)

NTFS protects records against **torn writes** (partial sector writes due to power failure) using the Update Sequence Array.

### Mechanism

Every 512-byte sector has its last 2 bytes replaced with a **check value** when written to disk. The original 2 bytes are saved in the USA at the beginning of the record.

### USA Layout

```
Offset (from USAOffset)    Content
──────────────────────     ───────────────────────────────────
+0                         Check value (2 bytes)
+2                         Original bytes from end of sector 0
+4                         Original bytes from end of sector 1
+6                         Original bytes from end of sector 2
...                        (one entry per sector in the record)
```

For a 1024-byte record (2 sectors), `USACount = 3`:
- Entry 0: Check value
- Entry 1: Original bytes from offset 0x1FE–0x1FF (end of sector 0)
- Entry 2: Original bytes from offset 0x3FE–0x3FF (end of sector 1)

### Fixup Algorithm

```
1. Read check_value from USA[0]
2. For each sector i (1 to USACount-1):
   a. Verify: last 2 bytes of sector i == check_value
      → If mismatch: record is CORRUPT (torn write detected)
   b. Replace: last 2 bytes of sector i ← USA[i]
3. Record is now safe to parse
```

**If fixup fails**, the record should be treated as corrupt. NTFS marks such records with magic `0x44414142` ("BAAD").

---

## Attribute Record Structure

After the fixed header and USA, the record contains a list of **attributes** — each carrying a specific type of metadata.

### Attribute List Termination

Attributes are listed sequentially. The list ends when:
- An attribute with `TypeCode == 0xFFFFFFFF` (AttributeEnd) is encountered
- The current offset reaches `BytesInUse`
- An attribute has `Length == 0` (corrupt — stop parsing)

### Common Attribute Record Header (16 bytes)

Every attribute begins with this header:

```
Offset  Size  Type   Field              Description
──────  ────  ─────  ─────────────────  ──────────────────────────────────
0x000   4     u32    TypeCode           Attribute type (see attribute types doc)
0x004   4     u32    Length             Total length of this attribute record
0x008   1     u8     IsNonResident      0 = resident, 1 = non-resident
0x009   1     u8     NameLength         Attribute name length in UTF-16 chars
0x00A   2     u16    NameOffset         Offset to attribute name (from record start)
0x00C   2     u16    Flags              Compressed (0x0001), Encrypted (0x4000), Sparse (0x8000)
0x00E   2     u16    Instance           Unique ID within this FRS
```

### Resident Attribute Data

Follows the common header when `IsNonResident == 0`. The attribute value is stored directly in the MFT record.

```
Offset  Size  Type   Field              Description
──────  ────  ─────  ─────────────────  ──────────────────────────────────
0x010   4     u32    ValueLength        Length of attribute value in bytes
0x014   2     u16    ValueOffset        Offset to value (from attribute start)
0x016   2     u16    ResidentFlags      0x0001 = Indexed
```

The attribute value is at `attribute_start + ValueOffset`, length `ValueLength`.

### Non-Resident Attribute Data

Follows the common header when `IsNonResident == 1`. The attribute value is stored in clusters elsewhere on disk.

```
Offset  Size  Type   Field                Description
──────  ────  ─────  ───────────────────  ──────────────────────────────────
0x010   8     i64    LowestVCN            Starting VCN of this record's portion
0x018   8     i64    HighestVCN           Ending VCN of this record's portion
0x020   2     u16    MappingPairsOffset   Offset to data run list
0x022   1     u8     CompressionUnit      Log2 of compression unit (0 = uncompressed)
0x023   5     [u8;5] Reserved             Padding
0x028   8     i64    AllocatedSize        Allocated space on disk (bytes)
0x030   8     i64    DataSize             Logical data size (bytes)
0x038   8     i64    InitializedSize      Valid data length (bytes)
0x040   8     i64    CompressedSize       Compressed size (only if compressed)
```

**Key size fields:**
- **`DataSize`**: The logical file size (what Explorer shows)
- **`AllocatedSize`**: Actual space consumed on disk (rounded up to cluster boundary)
- **`InitializedSize`**: How much of the allocated space contains valid data

### Named Attributes

Attributes can have a **name** (UTF-16LE string). The name is located at `NameOffset` bytes from the attribute start, with length `NameLength` characters.

- Unnamed `$DATA` attribute = the default data stream
- Named `$DATA` attribute = an Alternate Data Stream (ADS)
- `$I30` = the standard directory index name

---

## Data Runs (Mapping Pairs)

Non-resident attributes store their cluster locations as **data runs** — a compact variable-length encoding.

### Format

Data runs start at `MappingPairsOffset` within the non-resident attribute. Each run is encoded as:

```
┌─────────────┬──────────────────────┬──────────────────────┐
│ Header byte │ Length field          │ Offset field         │
│ (1 byte)    │ (variable, 1-4 bytes)│ (variable, 0-4 bytes)│
└─────────────┴──────────────────────┴──────────────────────┘
```

**Header byte encoding:**
```
Bits 0-3: Size of length field (in bytes)
Bits 4-7: Size of offset field (in bytes)
```

**Termination:** A header byte of `0x00` marks the end of the data run list.

### Offset Interpretation

- The offset field is **signed** and **relative to the previous run's LCN**
- First run: offset is absolute LCN
- Subsequent runs: offset is delta from previous LCN
- Zero-length offset field = **sparse run** (no physical clusters allocated)

### Example

```
Raw bytes: 31 01 40 00 00 0C  11 10 00  00

Run 1: Header=0x31 → length_size=1, offset_size=3
  Length: 0x01 = 1 cluster
  Offset: 0x0C0040 = LCN 786496 (absolute, first run)

Run 2: Header=0x11 → length_size=1, offset_size=1
  Length: 0x10 = 16 clusters
  Offset: 0x00 = delta 0 → LCN 786496 (continues from previous)

Run 3: Header=0x00 → END
```

---

## File Reference Format

Several NTFS fields store **file references** — a combination of FRS number and sequence number packed into 8 bytes:

```
┌────────────────────────────────────────────────────────┐
│ Bits 0-47:  FRS number (48 bits, max ~281 trillion)    │
│ Bits 48-63: Sequence number (16 bits)                  │
└────────────────────────────────────────────────────────┘
```

**Extraction:**
```
frs = file_reference & 0x0000FFFFFFFFFFFF
sequence = file_reference >> 48
```

The sequence number is incremented each time the FRS is reused for a different file. This allows detecting stale references (e.g., a parent directory that has been deleted and its FRS reused for a different file).

---

## Record Size Variants

| Record Size | Sectors | USA Entries | Max Attribute Space |
|-------------|---------|-------------|---------------------|
| 512 bytes | 1 | 2 | ~440 bytes |
| **1024 bytes** | **2** | **3** | **~950 bytes** |
| 2048 bytes | 4 | 5 | ~1970 bytes |
| 4096 bytes | 8 | 9 | ~4020 bytes |

The "max attribute space" is approximate: `record_size - header_size - USA_size - end_marker`.

When attributes exceed the available space, NTFS creates **extension records** and links them via the `$ATTRIBUTE_LIST` attribute.

---

## References

- Microsoft: [FILE_RECORD_SEGMENT_HEADER](https://learn.microsoft.com/en-us/windows/win32/devnotes/file-record-segment-header) (WDK)
- Microsoft: [NTFS Technical Reference](https://learn.microsoft.com/en-us/windows-server/storage/file-server/ntfs-overview)
- Linux-NTFS Project: [MFT Record Layout](https://flatcap.github.io/linux-ntfs/ntfs/concepts/file_record.html)
- Carrier, B. *File System Forensic Analysis* (Addison-Wesley, 2005) — Chapter 13

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
