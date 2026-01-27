# UFFS-MFT Field Enhancements

> **Scope**: `uffs_mft` crate - MftIndex structure, CSV/Parquet exports
> **Status**: In Progress (P1+P2 Complete, P3 Pending)
> **Created**: 2026-01-26
> **Last Updated**: 2026-01-26

## Overview

This document tracks enhancements to the raw MFT (Master File Table) fields exported by `uffs_mft`. The MFT is the heart of NTFS - every file and directory on an NTFS volume has at least one entry (record) in the MFT. Each record is typically 1024 bytes and contains:

- **File Record Header**: Magic signature, LSN, sequence number, flags
- **$STANDARD_INFORMATION**: Timestamps, file attributes, security ID, USN
- **$FILE_NAME**: Name, parent reference, timestamps (can have multiple for DOS/Win32 names)
- **$DATA**: File content (resident if small, or extent pointers if large)
- **$REPARSE_POINT**: Symlink/junction target information (if applicable)
- **Other attributes**: $ATTRIBUTE_LIST, $SECURITY_DESCRIPTOR, $INDEX_ROOT, etc.

### What is `uffs_mft`?

`uffs_mft` is a power-user/forensic tool that reads raw MFT data and exports it to structured formats (CSV, Parquet). Unlike the main `uffs` CLI which focuses on fast file search, `uffs_mft` exposes the raw NTFS metadata for:

- **Forensic analysis**: Timeline reconstruction, deleted file recovery, tampering detection
- **System administration**: Permission auditing, quota analysis, storage optimization
- **Development**: Understanding NTFS internals, debugging file system issues

### Design Principles

1. **Truthful to MFT**: Export what's actually in the MFT record, not interpreted values
2. **Cheap by default**: Include fields with negligible CPU/memory cost always
3. **Expensive opt-in**: Fields with significant cost (deleted records, extensions) behind `--forensic` flag
4. **Forensic-grade**: Preserve all information needed for incident response

---

## Current Export (37 columns - Index v6)

| Category | Fields | Description |
|----------|--------|-------------|
| **Identity** | frs, sequence_number, lsn, parent_frs, name, namespace, path | Unique identifiers and location in file system tree |
| **Size** | size, allocated_size | Logical size vs. disk clusters allocated |
| **Timestamps ($SI)** | si_created, si_modified, si_accessed, si_mft_changed | User-visible timestamps (can be modified by apps) |
| **Forensic (NTFS 3.0+)** | usn, security_id, owner_id | Journal correlation and security analysis |
| **Timestamps ($FN)** | fn_created, fn_modified, fn_accessed, fn_mft_changed | Kernel-maintained timestamps (harder to tamper) |
| **Flags** | is_directory, is_readonly, is_hidden, is_system, is_archive, is_compressed, is_encrypted, is_sparse, is_reparse, is_offline, is_not_indexed, is_temporary, flags | File attributes from $STANDARD_INFORMATION |
| **Reparse/Resident** | reparse_tag, is_resident | Symlink/junction type and MFT-resident data flag |
| **Counts** | link_count, stream_count | Hard links and alternate data streams |

### Field Details

#### Identity Fields

| Field | Type | Source | CPU Cost | Description |
|-------|------|--------|----------|-------------|
| `frs` | u64 | Record position | None | File Record Segment number - unique ID within MFT |
| `sequence_number` | u16 | Header @0x10 | None | Incremented on reuse; FRS+seq = unique file reference |
| `lsn` | u64 | Header @0x08 | None | Log Sequence Number - correlates with $LogFile journal |
| `parent_frs` | u64 | $FILE_NAME | None | Parent directory's FRS (5 = root) |
| `name` | String | $FILE_NAME | None | File/directory name (UTF-16 decoded) |
| `namespace` | u8 | $FILE_NAME | None | 0=POSIX, 1=Win32, 2=DOS, 3=Win32+DOS |
| `path` | String | Derived | O(depth) | Full path resolved by walking parent chain |

#### Forensic Fields (NTFS 3.0+, Windows 2000+)

| Field | Type | Source | CPU Cost | Description |
|-------|------|--------|----------|-------------|
| `usn` | u64 | $STD_INFO @0x40 | None | Update Sequence Number - correlates with $UsnJrnl |
| `security_id` | u32 | $STD_INFO @0x34 | None | Index into $Secure file for ACL lookup |
| `owner_id` | u32 | $STD_INFO @0x30 | None | Quota owner ID for multi-user tracking |

#### Reparse/Resident Fields

| Field | Type | Source | CPU Cost | Description |
|-------|------|--------|----------|-------------|
| `reparse_tag` | u32 | $REPARSE_POINT @0x00 | ~1% | Identifies symlink/junction/OneDrive type (0 if none) |
| `is_resident` | bool | $DATA header | None | True if file data stored inline in MFT record (<~700 bytes) |

---

## Impact Analysis

Understanding the cost of each field helps decide what to include by default vs. behind flags.

### Cost Categories

| Category | Description | Example |
|----------|-------------|---------|
| **CPU: None** | Data already in memory, just copy bytes | Reading `lsn` from header we already parsed |
| **CPU: Negligible** | Trivial additional work | Extracting 4 more bytes from $STD_INFO |
| **CPU: Low (~1-2%)** | New attribute to locate/parse | Finding $REPARSE_POINT in attribute list |
| **CPU: High (+10-50%)** | Changes record count or requires extra I/O | Including deleted records |
| **Memory: Per-field** | Fixed bytes per record | +8 bytes × 10M files = +80 MB |
| **Memory: Per-record** | More records in index | Deleted files can add 10-50% more records |

### Memory Impact per Record

| Field | Size | 1M files | 10M files | Notes |
|-------|------|----------|-----------|-------|
| `lsn` | 8 bytes | +8 MB | +80 MB | P1 - already added |
| `usn` | 8 bytes | +8 MB | +80 MB | P1 - stored in StandardInfo |
| `security_id` | 4 bytes | +4 MB | +40 MB | P1 - stored in StandardInfo |
| `owner_id` | 4 bytes | +4 MB | +40 MB | P1 - stored in StandardInfo |
| `reparse_tag` | 4 bytes | +4 MB | +40 MB | P2 - added to FileRecord |
| `is_resident` | 1 bit | ~125 KB | ~1.25 MB | P2 - packed in stream flags |
| `is_deleted` | 1 bit | ~125 KB | ~1.25 MB | P3 - but adds MORE RECORDS |
| `is_corrupt` | 1 bit | ~125 KB | ~1.25 MB | P3 - but adds MORE RECORDS |
| `is_extension` | 1 bit | ~125 KB | ~1.25 MB | P3 - packed in flags |
| `base_frs` | 8 bytes | +8 MB | +80 MB | P3 - only for extensions |

### FileRecord Size Evolution

```
Before P1:  192 bytes/record
After P1:   200 bytes/record (+8 bytes for lsn)
After P2:   216 bytes/record (+16 bytes for reparse_tag + alignment padding)
After P3:   224 bytes/record (+8 bytes for base_frs, forensic_flags packed in existing u8)

Total growth: 192 → 224 = +32 bytes (+17%)
At 10M files: 1.92 GB → 2.24 GB = +320 MB
```

### CPU/Speed Impact

| Field | Parse Cost | Why | Mitigation |
|-------|------------|-----|------------|
| `lsn` | None | Already reading header | - |
| `usn` | None | Already reading $STD_INFO | - |
| `security_id` | None | Already reading $STD_INFO | - |
| `owner_id` | None | Already reading $STD_INFO | - |
| `reparse_tag` | ~1-2% | Must locate $REPARSE_POINT attribute | Only ~1% of files have reparse points |
| `is_resident` | None | Already checking $DATA header | - |
| `is_deleted` | **+10-50%** | Currently skip deleted records entirely | Behind `--forensic` flag |
| `is_corrupt` | **+5-10%** | Currently skip corrupt records | Behind `--forensic` flag |
| `is_extension` | None | Already detecting extensions | - |
| `base_frs` | None | Already have value during parsing | - |

### Decision Matrix

| Field | Memory | CPU | Forensic Value | Default? |
|-------|--------|-----|----------------|----------|
| `lsn` | +8B | None | ⭐⭐⭐ Timeline | ✅ Always |
| `usn` | +8B | None | ⭐⭐⭐ Journal correlation | ✅ Always |
| `security_id` | +4B | None | ⭐⭐⭐ ACL analysis | ✅ Always |
| `owner_id` | +4B | None | ⭐⭐ Quota tracking | ✅ Always |
| `reparse_tag` | +4B | ~1% | ⭐⭐⭐ Symlink detection | ✅ Always |
| `is_resident` | +1b | None | ⭐⭐ MFT analysis | ✅ Always |
| `is_deleted` | +records | +10-50% | ⭐⭐⭐⭐ Recovery | ⚠️ `--forensic` |
| `is_corrupt` | +records | +5-10% | ⭐⭐ Tampering | ⚠️ `--forensic` |
| `is_extension` | +1b | None | ⭐ MFT internals | ⚠️ `--forensic` |
| `base_frs` | +8B | None | ⭐ MFT internals | ⚠️ `--forensic` |

---

## Implementation Details

### Priority 1: Easy Wins ✅ COMPLETE

These fields were already being read but not exported. Zero additional CPU cost.

#### `lsn` - Log Sequence Number

```
Location: MFT Record Header, offset 0x08, 8 bytes (u64)
Source:   FileRecordSegmentHeader.lsn
```

**What it is**: Every MFT record change is logged to `$LogFile` with an LSN. This number lets you correlate MFT state with the transaction log.

**Forensic use**:
- Reconstruct exact sequence of file system operations
- Identify which files were modified in same transaction
- Detect if MFT was modified without proper logging (tampering)

#### `usn` - Update Sequence Number

```
Location: $STANDARD_INFORMATION, offset 0x40, 8 bytes (u64)
Source:   StandardInformationExtended.usn (NTFS 3.0+ only, 72-byte $STD_INFO)
```

**What it is**: Monotonically increasing number assigned to each file change. Correlates with `$UsnJrnl` (USN Journal).

**Forensic use**:
- Timeline reconstruction: USN journal has timestamps for each change
- Detect anti-forensics: gaps in USN sequence indicate journal tampering
- Incident response: quickly find all files modified in time window

**Note**: Only present in NTFS 3.0+ (Windows 2000+). We detect version by checking $STD_INFO size (48 bytes = NTFS 1.x, 72 bytes = NTFS 3.0+).

#### `security_id` - Security Descriptor Index

```
Location: $STANDARD_INFORMATION, offset 0x34, 4 bytes (u32)
Source:   StandardInformationExtended.security_id
```

**What it is**: Index into `$Secure` file's $SII stream. The actual ACL/DACL is stored centrally and referenced by this ID.

**Forensic use**:
- Find all files with same permissions (same security_id)
- Identify files with unusual/custom ACLs (unique security_id)
- Audit permission changes over time

#### `owner_id` - Quota Owner

```
Location: $STANDARD_INFORMATION, offset 0x30, 4 bytes (u32)
Source:   StandardInformationExtended.owner_id
```

**What it is**: User ID for disk quota tracking. Maps to SID in `$Quota` file.

**Forensic use**:
- Identify which user created/owns files
- Track storage usage per user
- Detect files created by deleted user accounts

---

### Priority 2: Medium Effort ✅ COMPLETE

These required parsing new attributes or checking additional header fields.

#### `reparse_tag` - Reparse Point Type

```
Location: $REPARSE_POINT attribute (type 0xC0), offset 0x00, 4 bytes (u32)
Source:   ReparsePointHeader.reparse_tag
```

**What it is**: Identifies the type of reparse point. Only present on files/directories with `is_reparse=true` flag.

**Implementation details**:
- Must iterate attributes to find $REPARSE_POINT (0xC0)
- Attribute is usually resident (small)
- First 4 bytes of attribute value = reparse tag
- Returns 0 if no $REPARSE_POINT attribute

**CPU cost**: ~1-2% overhead because we must scan for a new attribute type. However, only ~1% of files have reparse points, so the actual parsing is rare.

**Common values**:
| Tag | Hex | Meaning |
|-----|-----|---------|
| `IO_REPARSE_TAG_MOUNT_POINT` | 0xA0000003 | Junction (directory symlink) |
| `IO_REPARSE_TAG_SYMLINK` | 0xA000000C | Symbolic link |
| `IO_REPARSE_TAG_CLOUD` | 0x9000001A | OneDrive placeholder |
| `IO_REPARSE_TAG_DEDUP` | 0x80000013 | Data deduplication |
| `IO_REPARSE_TAG_WOF` | 0x80000017 | Windows Overlay (compressed) |
| `IO_REPARSE_TAG_APPEXECLINK` | 0x8000001B | UWP app link |

#### `is_resident` - Data Stored in MFT

```
Location: $DATA attribute header, is_non_resident field (byte 0x08, bit 0)
Source:   AttributeRecordHeader.is_non_resident == 0
```

**What it is**: True if file's data is stored directly in the MFT record (inline), false if stored in separate clusters.

**Implementation details**:
- Check `is_non_resident` field in $DATA attribute header
- Resident = data follows header in same MFT record
- Non-resident = header contains extent list pointing to clusters
- Directories have no $DATA, so `is_resident=false`
- For files with ADS, we check the unnamed (default) $DATA stream

**CPU cost**: None - we already parse the $DATA header, just store the flag.

**Practical use**:
- Small files (<~700 bytes) are resident → faster access, no cluster allocation
- Resident files contribute to MFT size/fragmentation
- Performance analysis: many small resident files = bloated MFT

---

### Priority 3: Architecture Changes ⬜ PENDING

These require the `--forensic` flag because they significantly change output.

#### `is_deleted` - Deleted File Records

```
Location: MFT Record Header, flags field (offset 0x16), bit 0
Source:   FileRecordSegmentHeader.flags & 0x01 == 0
```

**What it is**: True if the MFT record is marked as "not in use" (deleted).

**Why it's hard**:
1. **Currently skipped**: `parse_record_full()` returns `Skip` for deleted records
2. **Incomplete data**: Deleted records may have:
   - No $FILE_NAME (name unknown)
   - Corrupted attributes (partially overwritten)
   - Orphaned parent (parent also deleted)
3. **Record count explosion**: Deleted records can add 10-50% more rows
4. **Path resolution**: Can't resolve path if parent chain is broken

**Proposed implementation**:
- Add `--forensic` flag to include deleted records
- Add `is_deleted` column (bool)
- Use `<DELETED>` or FRS number for unknown names
- Use `<ORPHAN>` for unresolvable paths

#### `is_corrupt` - Corrupted Records

```
Location: MFT Record Header, magic field (offset 0x00)
Source:   magic != "FILE" or USA fixup failed
```

**What it is**: True if the record failed validation:
- Magic is "BAAD" instead of "FILE" (known corruption)
- USA (Update Sequence Array) fixup failed (sector boundary corruption)

**Why it's hard**:
1. **Currently skipped**: We return `Skip` for corrupt records
2. **Unparseable**: Corrupt records may have garbage attributes
3. **Forensic value**: Corruption patterns can indicate:
   - Disk failure
   - Anti-forensics (intentional corruption)
   - Ransomware (encrypted MFT)

**Proposed implementation**:
- Add `--forensic` flag to include corrupt records
- Add `is_corrupt` column (bool)
- Export whatever fields we can parse, null for others

#### `is_extension` / `base_frs` - Extension Records

```
Location: MFT Record Header, base_file_record_segment (offset 0x20), 8 bytes
Source:   FileRecordSegmentHeader.base_file_record_segment
```

**What it is**: Large files with many attributes (lots of ADS, many extents) need multiple MFT records. The first is the "base" record, others are "extensions".

**Current behavior**: Extensions are transparently merged into base record during parsing. User sees one row per file.

**Why it's hard**:
1. **Changes data model**: Currently 1 row = 1 file. With extensions, 1 file = N rows.
2. **Confusing for users**: Most users expect 1 row per file
3. **Low value**: Only power users care about MFT internals

**Proposed implementation**:
- Add `--forensic` flag to expose extension records
- Add `is_extension` column (bool)
- Add `base_frs` column (u64, 0 for base records)
- Keep default behavior: merge extensions into base

---

## Implementation Tracking

### Summary

| Priority | Fields | Status | Index Version | Columns |
|----------|--------|--------|---------------|---------|
| P1 | lsn, usn, security_id, owner_id | ✅ Complete | v5 | 35 |
| P2 | reparse_tag, is_resident | ✅ Complete | v6 | 37 |
| P3 | is_deleted, is_corrupt, is_extension, base_frs | ✅ Complete | v7 | 37 (normal) / 41 (forensic) |

> **Note**: P3 forensic columns are only included in output when `--forensic` flag is used.
> Normal mode outputs 37 columns; forensic mode outputs 41 columns.

### Priority 1: Easy Wins ✅

| Field | Status | Date | Implementation |
|-------|--------|------|----------------|
| `lsn` | ✅ | 2026-01-26 | `FileRecord.lsn` from header @0x08 |
| `usn` | ✅ | 2026-01-26 | `StandardInfo` packed field, from $STD_INFO @0x40 |
| `security_id` | ✅ | 2026-01-26 | `StandardInfo` packed field, from $STD_INFO @0x34 |
| `owner_id` | ✅ | 2026-01-26 | `StandardInfo` packed field, from $STD_INFO @0x30 |

### Priority 2: Medium Effort ✅

| Field | Status | Date | Implementation |
|-------|--------|------|----------------|
| `reparse_tag` | ✅ | 2026-01-26 | `FileRecord.reparse_tag` from $REPARSE_POINT @0x00 |
| `is_resident` | ✅ | 2026-01-26 | `IndexStreamInfo.flags` bit 1, from $DATA header |

### Priority 3: Architecture Changes ✅

| Field | Status | Date | Implementation |
|-------|--------|------|----------------|
| `is_deleted` | ✅ | 2026-01-27 | `FileRecord.forensic_flags` bit 0, `--forensic` flag |
| `is_corrupt` | ✅ | 2026-01-27 | `FileRecord.forensic_flags` bit 1, `--forensic` flag |
| `is_extension` | ✅ | 2026-01-27 | `FileRecord.forensic_flags` bit 2, `--forensic` flag |
| `base_frs` | ✅ | 2026-01-27 | `FileRecord.base_frs` u64, `--forensic` flag |

#### P3 Test Results (G Drive MFT, 20MB)

| Mode | Records | File Size |
|------|---------|-----------|
| Normal | 15,085 | 5.96 MB |
| Forensic (`--forensic`) | 20,220 (+34%) | 7.98 MB |

| Forensic Type | Count |
|---------------|-------|
| Deleted | 4,970 |
| Corrupt | 165 |
| Extension | 0 |

---

## Appendix: Reparse Tag Reference

Complete list of known reparse tags for forensic analysis:

| Tag Name | Hex Value | Decimal | Description |
|----------|-----------|---------|-------------|
| `IO_REPARSE_TAG_MOUNT_POINT` | 0xA0000003 | 2684354563 | Junction point (directory symlink) |
| `IO_REPARSE_TAG_HSM` | 0xC0000004 | 3221225476 | Hierarchical Storage Management |
| `IO_REPARSE_TAG_HSM2` | 0x80000006 | 2147483654 | HSM variant |
| `IO_REPARSE_TAG_SIS` | 0x80000007 | 2147483655 | Single Instance Storage |
| `IO_REPARSE_TAG_DFS` | 0x8000000A | 2147483658 | Distributed File System |
| `IO_REPARSE_TAG_SYMLINK` | 0xA000000C | 2684354572 | Symbolic link (file or directory) |
| `IO_REPARSE_TAG_DFSR` | 0x80000012 | 2147483666 | DFS Replication |
| `IO_REPARSE_TAG_DEDUP` | 0x80000013 | 2147483667 | Data deduplication |
| `IO_REPARSE_TAG_NFS` | 0x80000014 | 2147483668 | NFS symlink |
| `IO_REPARSE_TAG_WOF` | 0x80000017 | 2147483671 | Windows Overlay Filter (compressed) |
| `IO_REPARSE_TAG_WCI` | 0x80000018 | 2147483672 | Windows Container Isolation |
| `IO_REPARSE_TAG_CLOUD` | 0x9000001A | 2415919130 | OneDrive placeholder |
| `IO_REPARSE_TAG_APPEXECLINK` | 0x8000001B | 2147483675 | UWP app execution link |
| `IO_REPARSE_TAG_GVFS` | 0x9000001C | 2415919132 | Git Virtual File System |
| `IO_REPARSE_TAG_LX_SYMLINK` | 0xA000001D | 2684354589 | WSL symbolic link |
| `IO_REPARSE_TAG_AF_UNIX` | 0x80000023 | 2147483683 | WSL Unix socket |
| `IO_REPARSE_TAG_LX_FIFO` | 0x80000024 | 2147483684 | WSL FIFO |
| `IO_REPARSE_TAG_LX_CHR` | 0x80000025 | 2147483685 | WSL character device |
| `IO_REPARSE_TAG_LX_BLK` | 0x80000026 | 2147483686 | WSL block device |

---

## Version History

| Date | Version | Index | Columns | Changes |
|------|---------|-------|---------|---------|
| 2026-01-26 | 1.0 | v4 | 31 | Initial document |
| 2026-01-26 | 1.1 | v5 | 35 | P1: lsn, usn, security_id, owner_id |
| 2026-01-26 | 1.2 | v6 | 37 | P2: reparse_tag, is_resident |
| 2026-01-26 | 1.3 | v6 | 37 | Enhanced documentation with technical details |
