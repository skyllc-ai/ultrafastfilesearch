# NTFS Attributes Reference

## Introduction

This document provides a complete reference for all NTFS attribute types, their field layouts, and how they are stored in MFT records. All information is based on publicly available Microsoft NTFS documentation and Windows SDK/WDK headers.

---

## Attribute Type Codes

Attributes are identified by a 32-bit type code. They appear in ascending order within an MFT record.

| Type Code | Name | Resident? | Description |
|-----------|------|-----------|-------------|
| `0x10` | `$STANDARD_INFORMATION` | Always | Timestamps, file attributes, security ID |
| `0x20` | `$ATTRIBUTE_LIST` | Usually | Lists attributes across extension records |
| `0x30` | `$FILE_NAME` | Always | Filename, parent reference, duplicate timestamps |
| `0x40` | `$OBJECT_ID` | Always | Distributed link tracking GUID |
| `0x50` | `$SECURITY_DESCRIPTOR` | Either | ACLs (usually in `$Secure` instead) |
| `0x60` | `$VOLUME_NAME` | Always | Volume label (only on FRS 3) |
| `0x70` | `$VOLUME_INFORMATION` | Always | NTFS version info (only on FRS 3) |
| `0x80` | `$DATA` | Either | File content / data streams |
| `0x90` | `$INDEX_ROOT` | Always | B-tree root for directory indexes |
| `0xA0` | `$INDEX_ALLOCATION` | Always NR | B-tree nodes for large directories |
| `0xB0` | `$BITMAP` | Either | Allocation bitmap for indexes or MFT |
| `0xC0` | `$REPARSE_POINT` | Always | Symlink, junction, OneDrive, etc. |
| `0xD0` | `$EA_INFORMATION` | Always | Extended attribute metadata |
| `0xE0` | `$EA` | Either | Extended attribute data |
| `0xF0` | `$PROPERTY_SET` | — | Obsolete (never used in production) |
| `0x100` | `$LOGGED_UTILITY_STREAM` | Either | EFS encryption metadata |
| `0xFFFFFFFF` | End marker | — | Terminates the attribute list |

**NR** = Non-Resident (always stored outside the MFT record)

---

## $STANDARD_INFORMATION (0x10)

Present on every file and directory. Always resident.

### NTFS 1.2 Layout (48 bytes)

```
Offset  Size  Type   Field                Description
──────  ────  ─────  ───────────────────  ──────────────────────────────────
0x00    8     i64    CreationTime         File creation time (FILETIME)
0x08    8     i64    ModificationTime     Last data modification (FILETIME)
0x10    8     i64    MftChangeTime        Last MFT record change (FILETIME)
0x18    8     i64    AccessTime           Last access time (FILETIME)
0x20    4     u32    FileAttributes       FILE_ATTRIBUTE_* flags
0x24    4     u32    MaxVersions          Maximum allowed versions (usually 0)
0x28    4     u32    VersionNumber        Current version (usually 0)
0x2C    4     u32    ClassId              Class ID (usually 0)
```

### NTFS 3.0+ Layout (72 bytes)

Extends the 1.2 layout with additional fields:

```
Offset  Size  Type   Field                Description
──────  ────  ─────  ───────────────────  ──────────────────────────────────
0x00    48    —      (Same as NTFS 1.2)   See above
0x30    4     u32    OwnerId              Quota tracking owner ID
0x34    4     u32    SecurityId           Index into $Secure descriptor store
0x38    8     u64    QuotaCharged         Bytes charged to user's quota
0x40    8     u64    UpdateSequenceNumber Correlates with USN Journal ($UsnJrnl)
```

### FILETIME Format

Windows FILETIME is a 64-bit value representing 100-nanosecond intervals since January 1, 1601 UTC.

```
Unix timestamp (seconds) = (FILETIME - 116444736000000000) / 10000000
Unix timestamp (microseconds) = (FILETIME - 116444736000000000) / 10
```

### FILE_ATTRIBUTE Flags

| Bit | Value | Name | Description |
|-----|-------|------|-------------|
| 0 | 0x0001 | READONLY | File is read-only |
| 1 | 0x0002 | HIDDEN | File is hidden |
| 2 | 0x0004 | SYSTEM | System file |
| 4 | 0x0010 | DIRECTORY | Entry is a directory |
| 5 | 0x0020 | ARCHIVE | File modified since last backup |
| 6 | 0x0040 | DEVICE | Reserved for system use |
| 7 | 0x0080 | NORMAL | No other attributes set |
| 8 | 0x0100 | TEMPORARY | Temporary file |
| 9 | 0x0200 | SPARSE_FILE | Sparse file |
| 10 | 0x0400 | REPARSE_POINT | Has reparse point data |
| 11 | 0x0800 | COMPRESSED | Compressed file or directory |
| 12 | 0x1000 | OFFLINE | Data not immediately available |
| 13 | 0x2000 | NOT_CONTENT_INDEXED | Excluded from content indexing |
| 14 | 0x4000 | ENCRYPTED | Encrypted (EFS) |
| 15 | 0x8000 | INTEGRITY_STREAM | Integrity stream (ReFS, Windows 8+) |
| 16 | 0x10000 | VIRTUAL | Reserved for system use |
| 17 | 0x20000 | NO_SCRUB_DATA | Excluded from data integrity scan |
| 19 | 0x80000 | PINNED | Pinned to local storage (Windows 10+) |
| 20 | 0x100000 | UNPINNED | Unpinned from local storage (Windows 10+) |

---

## $FILE_NAME (0x30)

Contains a filename and parent directory reference. Always resident. A file typically has 1-2 `$FILE_NAME` attributes (one Win32 name, optionally one DOS 8.3 name).

### Layout (66 bytes fixed + variable name)

```
Offset  Size  Type   Field                 Description
──────  ────  ─────  ────────────────────  ──────────────────────────────────
0x00    8     u64    ParentDirectory       Parent FRS + sequence (file reference)
0x08    8     i64    CreationTime          Duplicate timestamp (FILETIME)
0x10    8     i64    ModificationTime      Duplicate timestamp
0x18    8     i64    MftChangeTime         Duplicate timestamp
0x20    8     i64    AccessTime            Duplicate timestamp
0x28    8     i64    AllocatedSize         Allocated size (from $FILE_NAME)
0x30    8     i64    DataSize              Logical size (from $FILE_NAME)
0x38    4     u32    FileAttributes        Duplicate attributes
0x3C    2     u16    PackedEaSize          Extended attributes / reparse tag
0x3E    2     u16    Reserved              Padding
0x40    1     u8     FileNameLength        Name length in UTF-16 characters
0x41    1     u8     FileNameNamespace     Namespace flag (see below)
0x42    var   [u16]  FileName              UTF-16LE filename (FileNameLength chars)
```

### Parent Reference

The `ParentDirectory` field is a **file reference** (see MFT Record Format doc):
```
parent_frs = ParentDirectory & 0x0000FFFFFFFFFFFF   (lower 48 bits)
parent_seq = ParentDirectory >> 48                    (upper 16 bits)
```

### Filename Namespaces

| Value | Name | Description |
|-------|------|-------------|
| 0 | POSIX | Case-sensitive, allows most Unicode characters |
| 1 | Win32 | Standard Windows long filename |
| 2 | DOS | 8.3 short filename (for backward compatibility) |
| 3 | Win32+DOS | Single name valid for both namespaces |

**Important:** Files often have two `$FILE_NAME` attributes — one Win32 (or Win32+DOS) and one DOS. The DOS-only name (namespace 2) is a compatibility alias and is typically skipped during indexing to avoid duplicate entries.

### Duplicate Timestamps

The timestamps and sizes in `$FILE_NAME` are **duplicates** of those in `$STANDARD_INFORMATION` and `$DATA`. They are updated when the file is created or renamed, but may become stale after subsequent modifications. For accurate timestamps, always prefer `$STANDARD_INFORMATION`.

---

## $DATA (0x80)

Contains the file's actual content. Can be resident (small files, typically < 700 bytes) or non-resident (data stored in clusters on disk).

### Unnamed vs Named

- **Unnamed** `$DATA`: The default data stream — what you read when you open a file normally
- **Named** `$DATA`: An Alternate Data Stream (ADS), accessed via `filename:streamname`

### Resident Data

For small files, the data is stored directly in the MFT record:
- Size available: approximately `record_size - header - other_attributes - end_marker`
- Typical threshold: ~700 bytes for a 1024-byte record

### Non-Resident Data

For larger files, the `$DATA` attribute contains:
- **Data runs** (mapping pairs): Describe which clusters on disk hold the data
- **Size fields**: `DataSize`, `AllocatedSize`, `InitializedSize`

### Special Case: $BadClus:$Bad

The `$BadClus` metafile (FRS 8) has a named `$DATA` stream called `$Bad` whose `DataSize` equals the entire volume size. The meaningful size is `InitializedSize`, which indicates how much data has actually been written (tracking bad clusters found).

---

## $ATTRIBUTE_LIST (0x20)

When a file's attributes exceed the space available in a single MFT record, an `$ATTRIBUTE_LIST` attribute is created that catalogs all attributes and which FRS contains each one.

### Entry Layout (26 bytes fixed + variable name)

```
Offset  Size  Type   Field              Description
──────  ────  ─────  ─────────────────  ──────────────────────────────────
0x00    4     u32    AttributeType      Type code of the listed attribute
0x04    2     u16    EntryLength        Total size of this list entry
0x06    1     u8     NameLength         Attribute name length (UTF-16 chars)
0x07    1     u8     NameOffset         Offset to attribute name
0x08    8     u64    StartVcn           Starting VCN (for non-resident extents)
0x10    8     u64    FileReference      FRS + sequence of record containing this attr
0x18    2     u16    AttributeId        Instance number of the attribute
0x1A    var   [u16]  Name              Attribute name (if NameLength > 0)
```

The list is typically resident for files with a moderate number of extensions, or non-resident for files with very many attributes.

---

## $INDEX_ROOT (0x90), $INDEX_ALLOCATION (0xA0), $BITMAP (0xB0)

These three attributes together implement **B-tree indexes**. The most common use is the `$I30` directory index that maps filenames to file references.

### $INDEX_ROOT (0x90)

Always resident. Contains the B-tree root node.

```
Offset  Size  Type   Field                  Description
──────  ────  ─────  ─────────────────────  ──────────────────────────────────
0x00    4     u32    AttributeType          Type of indexed attribute (0x30 for filenames)
0x04    4     u32    CollationRule          Sorting rule (1 = filename collation)
0x08    4     u32    IndexBlockSize         Size of each index allocation block
0x0C    1     u8     ClustersPerIndexBlock  Clusters per index block
0x0D    3     [u8;3] Padding
0x10    —     —      IndexHeader            Start of index entries
```

### Index Header

```
Offset  Size  Type   Field                  Description
──────  ────  ─────  ─────────────────────  ──────────────────────────────────
0x00    4     u32    FirstEntryOffset       Offset to first index entry
0x04    4     u32    TotalSizeOfEntries     Total size of all entries
0x08    4     u32    AllocatedSize          Allocated size for entries
0x0C    4     u32    Flags                  0x01 = has sub-nodes (large index)
```

### $INDEX_ALLOCATION (0xA0)

Always non-resident. Contains the B-tree non-root nodes for large directories. Each node is an INDX record (magic `0x58444E49` = "INDX") with its own USA fixup.

### $BITMAP (0xB0)

Tracks which index allocation blocks are in use. One bit per block.

### The $I30 Index

For directories, the index named `$I30` stores sorted filename-to-file-reference mappings. The sizes of `$INDEX_ROOT`, `$INDEX_ALLOCATION`, and `$BITMAP` with name `$I30` together represent the total space consumed by the directory's index structure.

---

## $REPARSE_POINT (0xC0)

Contains reparse point data for symlinks, junctions, mount points, and other types. Always resident.

### Header (8 bytes)

```
Offset  Size  Type   Field              Description
──────  ────  ─────  ─────────────────  ──────────────────────────────────
0x00    4     u32    ReparseTag         Identifies the reparse type
0x04    2     u16    DataLength         Length of reparse data (excl. header)
0x06    2     u16    Reserved           Padding
```

### Common Reparse Tags

| Tag | Name | Description |
|-----|------|-------------|
| `0xA0000003` | `IO_REPARSE_TAG_MOUNT_POINT` | Junction / mount point |
| `0xA000000C` | `IO_REPARSE_TAG_SYMLINK` | Symbolic link |
| `0x80000017` | `IO_REPARSE_TAG_WOF` | Windows Overlay Filter (compressed) |
| `0x80000018` | `IO_REPARSE_TAG_WCI` | Windows Container Image |
| `0x8000001B` | `IO_REPARSE_TAG_APPEXECLINK` | App execution link (Store apps) |
| `0x9000001A` | `IO_REPARSE_TAG_CLOUD` | Cloud/OneDrive placeholder |
| `0x9000001C` | `IO_REPARSE_TAG_GVFS` | Git Virtual File System |
| `0xA000001D` | `IO_REPARSE_TAG_LX_SYMLINK` | WSL symbolic link |

### Mount Point / Symlink Data

For junctions and symlinks, the reparse data contains:

```
Offset  Size  Type   Field                    Description
──────  ────  ─────  ───────────────────────  ──────────────────────────────
0x00    2     u16    SubstituteNameOffset     Offset to substitute name
0x02    2     u16    SubstituteNameLength     Length of substitute name (bytes)
0x04    2     u16    PrintNameOffset          Offset to display name
0x06    2     u16    PrintNameLength          Length of display name (bytes)
0x08    var   [u16]  PathBuffer               UTF-16LE path strings
```

Symlinks additionally have a `Flags` field (4 bytes) before `PathBuffer` where bit 0 indicates a relative symlink.

---

## $OBJECT_ID (0x40)

Contains a 16-byte GUID for distributed link tracking. Always resident.

```
Offset  Size  Type    Field              Description
──────  ────  ──────  ─────────────────  ──────────────────────────────────
0x00    16    GUID    ObjectId           Object identifier
0x10    16    GUID    BirthVolumeId      Volume where object was created (optional)
0x20    16    GUID    BirthObjectId      Original object ID (optional)
0x30    16    GUID    DomainId           Domain identifier (optional)
```

Only the first 16 bytes (ObjectId) are required; the rest are optional.

---

## $EA_INFORMATION (0xD0) and $EA (0xE0)

Extended Attributes provide OS/2 and POSIX compatibility.

### $EA_INFORMATION (always resident, 8 bytes)

```
Offset  Size  Type   Field              Description
──────  ────  ─────  ─────────────────  ──────────────────────────────────
0x00    2     u16    PackedEaSize       Total packed EA size
0x02    2     u16    NeedEaCount        Number of EAs with NEED_EA flag
0x04    4     u32    UnpackedEaSize     Total unpacked EA size
```

### $EA (variable)

Contains the actual extended attribute data as a linked list of name-value pairs.

---

## $LOGGED_UTILITY_STREAM (0x100)

Used by EFS (Encrypting File System) to store encryption metadata. The stream name is typically `$EFS`.

---

## Attribute Residency Rules

| Attribute | Always Resident | Can Be Non-Resident |
|-----------|----------------|---------------------|
| `$STANDARD_INFORMATION` | ✅ | No |
| `$FILE_NAME` | ✅ | No |
| `$OBJECT_ID` | ✅ | No |
| `$VOLUME_NAME` | ✅ | No |
| `$VOLUME_INFORMATION` | ✅ | No |
| `$INDEX_ROOT` | ✅ | No |
| `$REPARSE_POINT` | ✅ | No |
| `$EA_INFORMATION` | ✅ | No |
| `$DATA` | Small files only | ✅ (> ~700 bytes) |
| `$ATTRIBUTE_LIST` | Usually | ✅ (many extensions) |
| `$INDEX_ALLOCATION` | Never | ✅ Always |
| `$BITMAP` | Small dirs | ✅ (large dirs) |
| `$SECURITY_DESCRIPTOR` | Rare | ✅ (usually in `$Secure`) |
| `$EA` | Small EAs | ✅ |
| `$LOGGED_UTILITY_STREAM` | Small | ✅ |

---

## References

- Microsoft: [NTFS Technical Reference](https://learn.microsoft.com/en-us/windows-server/storage/file-server/ntfs-overview)
- Microsoft: [FILE_ATTRIBUTE Constants](https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants)
- Microsoft: [Reparse Point Tags](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/c8e77b37-3909-4fe6-a4ea-2b9d423b1ee4)
- Linux-NTFS Project: [Attribute Definitions](https://flatcap.github.io/linux-ntfs/ntfs/attributes/)
- Carrier, B. *File System Forensic Analysis* (Addison-Wesley, 2005)

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
