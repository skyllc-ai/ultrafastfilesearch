# NTFS System Metafiles

## Introduction

This document describes the reserved system metafiles that NTFS creates on every volume. These are internal structures that manage the filesystem itself. All information is based on publicly available Microsoft NTFS documentation.

---

## Reserved File Record Segments (FRS 0–15)

NTFS reserves the first 16 File Record Segments for system metafiles. These records are created when the volume is formatted and are always present.

| FRS | Name | Purpose |
|-----|------|---------|
| 0 | `$MFT` | The Master File Table itself |
| 1 | `$MFTMirr` | Mirror of the first 4 MFT records (disaster recovery) |
| 2 | `$LogFile` | NTFS transaction journal (write-ahead log) |
| 3 | `$Volume` | Volume label, NTFS version, volume flags |
| 4 | `$AttrDef` | Attribute type definitions (names, sizes, flags) |
| **5** | **`.` (root)** | **Root directory — parent of all user files** |
| 6 | `$Bitmap` | Cluster allocation bitmap (1 bit per cluster) |
| 7 | `$Boot` | Boot sector and bootstrap code |
| 8 | `$BadClus` | Bad cluster tracking |
| 9 | `$Secure` | Security descriptor database |
| 10 | `$UpCase` | Unicode uppercase mapping table |
| 11 | `$Extend` | Container for extended system metadata |
| 12–15 | (Reserved) | Reserved for future NTFS versions |

---

## Detailed Descriptions

### $MFT (FRS 0)

The Master File Table is the central database of NTFS. Every file and directory on the volume has at least one record in the MFT.

- **`$DATA` attribute**: Contains the MFT data itself — the entire table of FILE records
- **`$BITMAP` attribute**: Tracks which FRS slots are in use (1 bit per record)
- The MFT is self-referential: FRS 0 describes the MFT's own location on disk
- Can be fragmented: its `$DATA` data runs describe all physical extents

### $MFTMirr (FRS 1)

A partial mirror of the MFT containing copies of the first 4 records (FRS 0–3). Located at the middle of the volume (specified by `Mft2StartLcn` in the boot sector). Used for recovery if the primary MFT is damaged.

### $LogFile (FRS 2)

The NTFS transaction journal. Records metadata operations (file creation, deletion, rename, attribute changes) before they are committed. Enables:
- Crash recovery (replay uncommitted transactions)
- Consistent state after unexpected shutdown
- Typically 64 MB on modern volumes

### $Volume (FRS 3)

Contains volume-level metadata:
- **`$VOLUME_NAME`** (0x60): The volume label (e.g., "Windows", "Data")
- **`$VOLUME_INFORMATION`** (0x70): NTFS version (major.minor) and volume flags

| NTFS Version | Windows Version |
|-------------|-----------------|
| 1.2 | Windows NT 4.0 |
| 3.0 | Windows 2000 |
| 3.1 | Windows XP through Windows 11 |

### $AttrDef (FRS 4)

Defines all valid attribute types for the volume. Each entry specifies:
- Attribute name (e.g., `$STANDARD_INFORMATION`)
- Type code (e.g., `0x10`)
- Minimum and maximum sizes
- Flags (resident-only, indexed, etc.)

### Root Directory (FRS 5)

The root directory is the ancestor of all user-visible files and directories. It is special in several ways:

- Its name is `.` (single dot)
- It is the only system metafile that is user-visible
- It is the starting point for all path resolution (every file's parent chain leads here)
- Its `$I30` index contains entries for all top-level files and folders
- In path display, the root name is typically suppressed: `C:\folder` not `C:\.\folder`

### $Bitmap (FRS 6)

The volume-wide cluster allocation bitmap. Contains one bit per cluster on the volume:
- Bit set (1) = cluster is allocated
- Bit clear (0) = cluster is free

**Not to be confused with** `$MFT::$BITMAP` (the MFT's own bitmap tracking which FRS records are in use).

### $Boot (FRS 7)

Contains a copy of the boot sector and additional bootstrap code. The primary boot sector is at sector 0 of the volume; this metafile provides a backup.

### $BadClus (FRS 8)

Tracks clusters that have been identified as bad (unreadable/unreliable).

- Has an unnamed `$DATA` stream (empty, zero-length)
- Has a named `$DATA` stream called `$Bad`
- `$Bad` has a `DataSize` equal to the entire volume size (sparse — doesn't actually consume space)
- `InitializedSize` of `$Bad` indicates actual bad cluster data written

**Important for MFT readers:** When computing file sizes, `$BadClus:$Bad` should use `InitializedSize` rather than `DataSize`, as `DataSize` would incorrectly report the entire volume size.

### $Secure (FRS 9)

Centralized security descriptor storage. Instead of storing a full ACL on every file, NTFS stores unique security descriptors in `$Secure` and references them by `SecurityId` from each file's `$STANDARD_INFORMATION` attribute.

Contains two indexes:
- `$SII` (Security ID Index): Maps SecurityId → descriptor offset
- `$SDH` (Security Descriptor Hash): Maps hash → SecurityId for deduplication

### $UpCase (FRS 10)

Contains a 128 KB table mapping every Unicode code point to its uppercase equivalent. Used by NTFS for case-insensitive filename comparisons. This table ensures consistent collation behavior across different Windows versions.

### $Extend (FRS 11)

A directory that contains extended system metafiles added in NTFS 3.0+:

| Name | Purpose |
|------|---------|
| `$ObjId` | Object ID index (distributed link tracking) |
| `$Quota` | Disk quota data per user |
| `$Reparse` | Reparse point index (symlinks, junctions) |
| `$UsnJrnl` | USN Change Journal |
| `$RmMetadata` | Resource Manager metadata (transactional NTFS) |

### FRS 12–15 (Reserved)

Reserved for future use. These records exist but contain no active data on current NTFS versions.

---

## System Metafile Visibility

By default, system metafiles (FRS 0–4, 6–15) are hidden from normal directory enumeration. Only FRS 5 (root directory) is visible in standard file listings.

| FRS | Name | Visible in Explorer? | Visible in `dir /a`? |
|-----|------|---------------------|---------------------|
| 0 | `$MFT` | No | Yes (with /a) |
| 1–4 | System | No | Yes (with /a) |
| **5** | **Root** | **Yes** | **Yes** |
| 6–15 | System | No | Yes (with /a) |
| ≥ 16 | User files | Yes | Yes |

File search tools typically exclude FRS 0–4 and 6–15 from results by default, showing only FRS 5 (root) and FRS ≥ 16 (user files).

---

## The USN Change Journal ($UsnJrnl)

Located under `$Extend`, the USN (Update Sequence Number) Journal records all changes to files and directories on the volume.

### Structure

- `$UsnJrnl:$J` — The journal data stream (append-only log of change records)
- `$UsnJrnl:$Max` — Configuration data (maximum size, allocation delta)

### USN Record Fields

Each change record contains:
- File reference (FRS + sequence)
- Parent file reference
- USN (monotonically increasing sequence number)
- Timestamp
- Reason flags (file created, deleted, renamed, data changed, etc.)
- Filename at time of change

### Use Cases

- **Incremental indexing**: Instead of re-reading the entire MFT, check the USN Journal for changes since the last scan
- **Backup software**: Identify changed files efficiently
- **File system monitoring**: Track real-time changes

---

## References

- Microsoft: [NTFS Technical Reference](https://learn.microsoft.com/en-us/windows-server/storage/file-server/ntfs-overview)
- Microsoft: [NTFS System Files](https://learn.microsoft.com/en-us/windows-server/storage/file-server/ntfs-overview#system-files)
- Microsoft: [Change Journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals)
- Linux-NTFS Project: [System Files](https://flatcap.github.io/linux-ntfs/ntfs/files/)

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
