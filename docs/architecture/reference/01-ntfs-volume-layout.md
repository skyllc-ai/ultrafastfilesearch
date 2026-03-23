# NTFS Volume Layout

## Introduction

This document describes the on-disk layout of an NTFS volume, focusing on the structures needed to locate and read the Master File Table (MFT). All information is based on publicly available Microsoft NTFS documentation and Windows SDK headers.

---

## Volume Structure

An NTFS volume has the following high-level layout:

```
┌──────────────────────────────────────────────────────────────────────┐
│ Sector 0: Boot Sector (512 bytes)                                    │
│   Contains: BIOS Parameter Block, MFT location, geometry             │
├──────────────────────────────────────────────────────────────────────┤
│ MFT Zone (variable location and size)                                │
│   Contains: Master File Table — all file/directory metadata          │
│   Location: Specified by MftStartLcn in boot sector                  │
│   Size: Grows as files are created                                   │
├──────────────────────────────────────────────────────────────────────┤
│ Data Area                                                            │
│   Contains: File data clusters, directory indexes                    │
│   Occupies: Remainder of volume                                      │
├──────────────────────────────────────────────────────────────────────┤
│ MFT Mirror (first 4 MFT records, location varies)                   │
│   Contains: Backup of FRS 0-3 for recovery                          │
├──────────────────────────────────────────────────────────────────────┤
│ Last Sector: Backup Boot Sector                                      │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Boot Sector (NTFS_BOOT_SECTOR)

**Location:** Sector 0 of the volume (byte offset 0)
**Size:** Exactly 512 bytes

The boot sector contains the BIOS Parameter Block (BPB) and NTFS-specific parameters needed to locate all other structures on the volume.

### Field Layout

```
Offset  Size  Field                          Description
──────  ────  ─────────────────────────────  ──────────────────────────────────
0x000   3     Jump                           x86 jump instruction
0x003   8     OemId                          "NTFS    " (must start with "NTFS")
0x00B   2     BytesPerSector                 Usually 512
0x00D   1     SectorsPerCluster              Power of 2: 1, 2, 4, 8, 16, ...128
0x00E   2     ReservedSectors                Always 0 in NTFS
0x010   3     Padding1                       Always 0
0x013   2     Unused1                        Always 0
0x015   1     MediaDescriptor                0xF8 for hard disks
0x016   2     Padding2                       Always 0
0x018   2     SectorsPerTrack                CHS geometry
0x01A   2     NumberOfHeads                  CHS geometry
0x01C   4     HiddenSectors                  Sectors before this partition
0x020   4     Unused2                        Always 0
0x024   4     Unused3                        Always 0x80008000
0x028   8     TotalSectors                   Total sectors on volume (i64)
0x030   8     MftStartLcn                    ★ LCN of $MFT start (i64)
0x038   8     MftMirrorStartLcn              LCN of $MFTMirr start (i64)
0x040   1     ClustersPerFileRecord          ★ Record size encoding (i8)
0x041   3     Padding3                       Unused
0x044   4     ClustersPerIndexBlock          Usually 1
0x048   8     VolumeSerialNumber             Unique volume identifier (i64)
0x050   4     Checksum                       Boot sector checksum
0x054   426   BootstrapCode                  x86 boot code
0x1FE   2     BootSignature                  0xAA55 (standard MBR signature)
```

### Key Derived Values

**Cluster size:**
```
cluster_size = SectorsPerCluster × BytesPerSector
```
Typical values: 4096 bytes (4 KB) for volumes > 2 GB.

**File record size:**
```
if ClustersPerFileRecord >= 0:
    record_size = ClustersPerFileRecord × cluster_size
else:
    record_size = 2 ^ (-ClustersPerFileRecord)
```
The negative encoding is used because the record size (typically 1024 bytes) is often smaller than a cluster (typically 4096 bytes). A value of `-10` means 2^10 = 1024 bytes.

**MFT byte offset on disk:**
```
mft_offset = MftStartLcn × cluster_size
```

### Validation

A valid NTFS boot sector must satisfy:
- `OemId` starts with "NTFS"
- `BytesPerSector` is a power of 2, typically 512
- `SectorsPerCluster` is a power of 2 (1–128)
- `MftStartLcn` > 0

---

## Cluster Geometry

NTFS organizes disk space in **clusters** — the fundamental allocation unit.

| Concept | Definition |
|---------|------------|
| **Sector** | Smallest physical I/O unit (usually 512 bytes) |
| **Cluster** | Smallest NTFS allocation unit (1–128 sectors) |
| **LCN** | Logical Cluster Number — absolute cluster position on volume |
| **VCN** | Virtual Cluster Number — logical offset within a file's data |

### Typical Cluster Sizes

| Volume Size | Default Cluster Size |
|-------------|---------------------|
| ≤ 512 MB | 512 bytes |
| ≤ 1 GB | 1024 bytes |
| ≤ 2 GB | 2048 bytes |
| > 2 GB | 4096 bytes |
| > 16 TB | 8192+ bytes |

### Address Translation

```
Physical byte offset = LCN × cluster_size
File byte offset     = VCN × cluster_size
```

---

## Master File Table (MFT)

The MFT is a special file (`$MFT`, FRS 0) that contains one fixed-size record for every file and directory on the volume.

### Location

The MFT starts at the cluster specified by `MftStartLcn` in the boot sector. It can be **fragmented** — meaning it may occupy multiple non-contiguous extents on disk.

### Size

The MFT grows dynamically as files are created. Its current size is available via:
- `FSCTL_GET_NTFS_VOLUME_DATA` → `MftValidDataLength`
- The `$DATA` attribute of FRS 0 (`$MFT` itself)

### Capacity

```
mft_capacity = MftValidDataLength / record_size
```

For a typical 2 TB drive with 2 million files:
- MFT size: ~2 GB
- Record count: ~2,000,000
- Record size: 1024 bytes

### Fragmentation

The MFT can become fragmented over time. Its physical layout is described by its **data runs** (the `$DATA` attribute of FRS 0), or equivalently by the retrieval pointers returned by `FSCTL_GET_RETRIEVAL_POINTERS`.

Each fragment is an **extent** — a contiguous range of clusters:

```
Extent 0: VCN 0       → LCN 786432,   500000 clusters
Extent 1: VCN 500000  → LCN 1200000,  300000 clusters
Extent 2: VCN 800000  → LCN 2000000,  200000 clusters
```

The VCN→LCN mapping allows reading any MFT record by computing:
1. Which extent contains the target VCN
2. The physical disk offset within that extent

### MFT Zone

NTFS reserves a contiguous region of disk space for MFT growth called the **MFT Zone**. This reduces MFT fragmentation by keeping space available for new records. The zone boundaries are reported by `FSCTL_GET_NTFS_VOLUME_DATA`:
- `MftZoneStart` — first cluster of the reserved zone
- `MftZoneEnd` — last cluster of the reserved zone

---

## MFT Bitmap ($MFT::$BITMAP)

The MFT has a `$BITMAP` attribute that tracks which records are **in use** (allocated to an active file or directory) and which are **free** (available for reuse).

### Format

- One bit per MFT record
- Bit set (1) = record is in use
- Bit clear (0) = record is free/deleted
- Bit 0 corresponds to FRS 0, bit 1 to FRS 1, etc.

### Size

```
bitmap_size = ceil(mft_capacity / 8) bytes
```

For 2 million records: ~250 KB.

### Use Cases

- **Space management**: NTFS uses the bitmap to find free slots for new files
- **Read optimization**: A reader can skip free records to reduce I/O
- **Forensics**: Cleared bits indicate deleted files whose records may still contain data

---

## NTFS Volume Data

The `FSCTL_GET_NTFS_VOLUME_DATA` control code returns an `NTFS_VOLUME_DATA_BUFFER` structure with comprehensive volume metadata:

| Field | Type | Description |
|-------|------|-------------|
| `VolumeSerialNumber` | i64 | Unique volume identifier |
| `NumberSectors` | i64 | Total sectors on volume |
| `TotalClusters` | i64 | Total clusters on volume |
| `FreeClusters` | i64 | Available clusters |
| `TotalReserved` | i64 | Reserved clusters |
| `BytesPerSector` | u32 | Sector size (usually 512) |
| `BytesPerCluster` | u32 | Cluster size |
| `BytesPerFileRecordSegment` | u32 | MFT record size (usually 1024) |
| `ClustersPerFileRecordSegment` | u32 | Clusters per MFT record |
| `MftValidDataLength` | i64 | Used size of `$MFT` in bytes |
| `MftStartLcn` | i64 | Starting LCN of `$MFT` |
| `Mft2StartLcn` | i64 | Starting LCN of `$MFTMirr` |
| `MftZoneStart` | i64 | Start of MFT reserved zone |
| `MftZoneEnd` | i64 | End of MFT reserved zone |

---

## References

- Microsoft: [NTFS Technical Reference](https://learn.microsoft.com/en-us/windows-server/storage/file-server/ntfs-overview)
- Microsoft: [FSCTL_GET_NTFS_VOLUME_DATA](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_volume_data)
- Microsoft: [NTFS_VOLUME_DATA_BUFFER](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-ntfs_volume_data_buffer)
- Linux-NTFS Project: [NTFS Documentation](https://flatcap.github.io/linux-ntfs/ntfs/)

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
